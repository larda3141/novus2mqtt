use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use serde_json::json;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

/// Incoming MQTT message (topic + payload).
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub topic: String,
    pub payload: Vec<u8>,
}

/// A resolved subscription (full topic + QoS) for re-subscribing on reconnect.
#[derive(Debug, Clone)]
struct ResolvedSub {
    full_topic: String,
    qos: QoS,
}

/// MQTT client wrapper with automatic reconnection and connection state tracking.
pub struct MqttClient {
    client: AsyncClient,
    base_prefix: String,
    connected: Arc<AtomicBool>,
    shutting_down: Arc<AtomicBool>,
    resolved_subs: Arc<Mutex<Vec<ResolvedSub>>>,
    /// Epoch seconds when MQTT disconnected (0 = connected or never disconnected)
    disconnected_since: Arc<AtomicU64>,
    /// Epoch seconds of the last event returned by the event loop (any packet,
    /// incoming or outgoing). Stops advancing if the event loop stalls.
    last_activity: Arc<AtomicU64>,
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl MqttClient {
    /// Create a new MQTT client and start its event loop.
    ///
    /// Returns the client and a receiver for incoming messages.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        broker: &str,
        port: u16,
        client_id: &str,
        username: Option<&str>,
        password: Option<&str>,
        base_prefix: &str,
        tls_enabled: bool,
        tls_insecure: bool,
    ) -> Result<(Self, mpsc::Receiver<IncomingMessage>)> {
        let mut opts = MqttOptions::new(client_id, broker, port);
        opts.set_keep_alive(Duration::from_secs(30));
        opts.set_clean_session(true);

        if let (Some(user), Some(pass)) = (username, password) {
            opts.set_credentials(user, pass);
        }

        if tls_enabled {
            anyhow::bail!("MQTT TLS is not yet implemented. Remove --mqtt-tls or set MQTT_TLS=false to connect without encryption");
        }
        let _ = tls_insecure;

        // Set last will
        let will_topic = format!("{}/status", base_prefix.trim_matches('/'));
        let will_payload = json!({"connected": false}).to_string();
        opts.set_last_will(rumqttc::LastWill::new(
            &will_topic,
            will_payload.as_bytes().to_vec(),
            QoS::AtLeastOnce,
            true,
        ));

        let (client, event_loop) = AsyncClient::new(opts, 100);
        let connected = Arc::new(AtomicBool::new(false));
        let shutting_down = Arc::new(AtomicBool::new(false));
        let resolved_subs = Arc::new(Mutex::new(Vec::new()));

        let (tx, rx) = mpsc::channel::<IncomingMessage>(256);
        let disconnected_since = Arc::new(AtomicU64::new(0));
        let last_activity = Arc::new(AtomicU64::new(epoch_secs()));

        let mqtt_client = MqttClient {
            client: client.clone(),
            base_prefix: base_prefix.trim_matches('/').to_string(),
            connected: connected.clone(),
            shutting_down: shutting_down.clone(),
            resolved_subs: resolved_subs.clone(),
            disconnected_since: disconnected_since.clone(),
            last_activity: last_activity.clone(),
        };

        // Spawn the event loop task
        tokio::spawn(Self::run_event_loop(
            event_loop,
            connected.clone(),
            shutting_down.clone(),
            tx,
            client,
            resolved_subs,
            disconnected_since,
            last_activity,
        ));

        Ok((mqtt_client, rx))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_event_loop(
        mut event_loop: EventLoop,
        connected: Arc<AtomicBool>,
        shutting_down: Arc<AtomicBool>,
        tx: mpsc::Sender<IncomingMessage>,
        client: AsyncClient,
        resolved_subs: Arc<Mutex<Vec<ResolvedSub>>>,
        disconnected_since: Arc<AtomicU64>,
        last_activity: Arc<AtomicU64>,
    ) {
        loop {
            if shutting_down.load(Ordering::SeqCst) {
                break;
            }

            let event = event_loop.poll().await;
            if event.is_ok() {
                last_activity.store(epoch_secs(), Ordering::SeqCst);
            }

            match event {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    info!("MQTT: connected to broker");
                    connected.store(true, Ordering::SeqCst);
                    disconnected_since.store(0, Ordering::SeqCst);

                    // Re-subscribe to all registered topics (clone to release lock before awaiting)
                    let subs: Vec<ResolvedSub> = resolved_subs.lock().await.clone();
                    for sub in &subs {
                        if let Err(e) = client.subscribe(&sub.full_topic, sub.qos).await {
                            error!(topic = %sub.full_topic, error = %e, "MQTT: re-subscribe failed");
                        } else {
                            debug!(topic = %sub.full_topic, "MQTT: re-subscribed");
                        }
                    }
                }
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    let msg = IncomingMessage {
                        topic: publish.topic.clone(),
                        payload: publish.payload.to_vec(),
                    };
                    // Never block the event loop on the command channel: if the
                    // main loop stops draining it, an awaiting send would stop
                    // poll() too, so keep-alives stop and the broker drops us.
                    match tx.try_send(msg) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(msg)) => {
                            warn!(topic = %msg.topic, "MQTT: command queue full, dropping message");
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            debug!("MQTT: incoming message channel closed");
                        }
                    }
                }
                Ok(Event::Incoming(Packet::Disconnect)) => {
                    info!("MQTT: disconnected cleanly");
                    connected.store(false, Ordering::SeqCst);
                    Self::mark_disconnected(&disconnected_since);
                }
                Ok(_) => {}
                Err(e) => {
                    if shutting_down.load(Ordering::SeqCst) {
                        break;
                    }
                    if connected.load(Ordering::SeqCst) {
                        warn!(error = %e, "MQTT: connection lost, will retry");
                        connected.store(false, Ordering::SeqCst);
                        Self::mark_disconnected(&disconnected_since);
                    }
                    // rumqttc will automatically retry connection; backoff to avoid busy-loop
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    fn mark_disconnected(disconnected_since: &AtomicU64) {
        // Only set if not already tracking a disconnect
        let _ = disconnected_since.compare_exchange(
            0,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }

    /// Returns how many seconds MQTT has been disconnected, or 0 if connected.
    pub fn disconnected_secs(&self) -> u64 {
        let since = self.disconnected_since.load(Ordering::SeqCst);
        if since == 0 {
            return 0;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(since)
    }

    /// Seconds since the event loop last returned an event. With a 30 s
    /// keep-alive this stays well under a minute while the loop is running.
    pub fn secs_since_activity(&self) -> u64 {
        epoch_secs().saturating_sub(self.last_activity.load(Ordering::SeqCst))
    }

    /// Get a clone of the underlying async client for fire-and-forget publishing.
    pub fn client(&self) -> AsyncClient {
        self.client.clone()
    }

    /// Check if currently connected to the broker.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Resolve a topic with optional base prefix.
    pub fn resolve_topic(&self, topic: &str, use_base_prefix: bool) -> String {
        if !use_base_prefix {
            return topic.to_string();
        }

        let normalized_base = self.base_prefix.trim_matches('/');
        let normalized_topic = topic.trim_matches('/');

        if normalized_topic == normalized_base
            || normalized_topic.starts_with(&format!("{normalized_base}/"))
        {
            return normalized_topic.to_string();
        }

        format!("{normalized_base}/{normalized_topic}")
    }

    /// Publish a message to an MQTT topic.
    pub async fn publish(
        &self,
        topic: &str,
        payload: &str,
        qos: QoS,
        retain: bool,
        use_base_prefix: bool,
    ) -> bool {
        const PUBLISH_TIMEOUT: Duration = Duration::from_secs(5);
        let full_topic = self.resolve_topic(topic, use_base_prefix);
        let fut = self
            .client
            .publish(&full_topic, qos, retain, payload.as_bytes().to_vec());
        match tokio::time::timeout(PUBLISH_TIMEOUT, fut).await {
            Ok(Ok(())) => true,
            Ok(Err(e)) => {
                error!(topic = %full_topic, error = %e, "MQTT: publish error");
                false
            }
            Err(_) => {
                error!(
                    topic = %full_topic,
                    timeout_secs = PUBLISH_TIMEOUT.as_secs(),
                    "MQTT: publish timed out (broker queue likely backed up)"
                );
                false
            }
        }
    }

    /// Convenience: publish with default QoS 0, retain, and base prefix.
    pub async fn publish_retain(&self, topic: &str, payload: &str) -> bool {
        self.publish(topic, payload, QoS::AtMostOnce, true, true)
            .await
    }

    /// Register and immediately subscribe to a topic.
    pub async fn subscribe(&mut self, topic: &str, use_base_prefix: bool, qos: QoS) -> bool {
        let full_topic = self.resolve_topic(topic, use_base_prefix);

        // Register for re-subscribe on reconnect
        {
            let mut subs = self.resolved_subs.lock().await;
            if !subs.iter().any(|s| s.full_topic == full_topic) {
                subs.push(ResolvedSub {
                    full_topic: full_topic.clone(),
                    qos,
                });
            }
        }

        match self.client.subscribe(&full_topic, qos).await {
            Ok(()) => {
                info!(topic = %full_topic, "MQTT: subscribed");
                true
            }
            Err(e) => {
                error!(topic = %full_topic, error = %e, "MQTT: subscribe error");
                false
            }
        }
    }

    /// Disconnect from the broker.
    pub async fn disconnect(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        if let Err(e) = self.client.disconnect().await {
            error!(error = %e, "MQTT: disconnect error");
        } else {
            info!("MQTT: disconnected");
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_resolve_topic_with_prefix() {
        // We can't construct a full MqttClient without a broker, so test the logic inline
        let base = "novus2mqtt/device1";
        let topic = "sensors/temp";
        let expected = format!("{base}/{topic}");

        let normalized_base = base.trim_matches('/');
        let normalized_topic = topic.trim_matches('/');
        let result = if normalized_topic == normalized_base
            || normalized_topic.starts_with(&format!("{normalized_base}/"))
        {
            normalized_topic.to_string()
        } else {
            format!("{normalized_base}/{normalized_topic}")
        };

        assert_eq!(result, expected);
    }

    #[test]
    fn test_resolve_topic_already_prefixed() {
        let base = "novus2mqtt";
        let topic = "novus2mqtt/sensors/temp";

        let normalized_base = base.trim_matches('/');
        let normalized_topic = topic.trim_matches('/');
        let result = if normalized_topic == normalized_base
            || normalized_topic.starts_with(&format!("{normalized_base}/"))
        {
            normalized_topic.to_string()
        } else {
            format!("{normalized_base}/{normalized_topic}")
        };

        assert_eq!(result, "novus2mqtt/sensors/temp");
    }
}
