//! Write queue and the MQTT-set handler.
//!
//! Writes to the bus are deferred until the next 0x84 (ping) or 0x87 (checksum
//! ACK) so they slot into the master's polling cycle. `WriteItem` represents
//! one queued write; `execute_pending_write` pops one and pushes it to serial;
//! `handle_set_command` is the MQTT-side entry point that validates a topic +
//! payload, encodes it, and enqueues it.

use serde_json::json;
use tracing::{error, info, warn};

use crate::payload;
use crate::protocol;

use super::{extract_payload_config, NovusConnector};

/// One queued bus write — either a structured register write (wrapped in an
/// 0x85 command frame at execute time) or a raw pre-built bus message
/// (e.g. dump command 0x83).
#[derive(Debug, Clone)]
pub(crate) enum WriteItem {
    Register {
        address: String,
        value_bytes: Vec<u8>,
    },
    Raw(Vec<u8>),
}

impl NovusConnector {
    pub(super) async fn execute_pending_write(&mut self) {
        let write_item = match self.bus_write_stack.pop_front() {
            Some(item) => item,
            None => return,
        };

        let message = match &write_item {
            WriteItem::Register {
                address,
                value_bytes,
            } => {
                let register_int = match u8::from_str_radix(address.trim_start_matches("0x"), 16) {
                    Ok(v) => v,
                    Err(e) => {
                        error!(address = %address, error = %e, "Application: invalid register address, skipping write");
                        return;
                    }
                };
                let mut data = vec![register_int, 0x00];
                data.extend_from_slice(value_bytes);
                protocol::build_bus_message(&[0x01, 0x00], 0x85, &data)
            }
            WriteItem::Raw(raw) => raw.clone(),
        };

        if let Some(ref mut serial) = self.serial_handler {
            if serial.write_data(&message).await {
                self.metrics.increment(&self.metrics.bus_messages_sent);

                // Log outgoing message to dump file
                if let Some(ref mut dump) = self.bus_dump {
                    use std::io::Write;
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64();
                    let hex: String = message
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let _ = writeln!(dump, "{ts:.3} > {hex}");
                    let _ = dump.flush();
                }

                match &write_item {
                    WriteItem::Register { address, .. } => {
                        info!(address = %address, "Application: executed register write");
                    }
                    WriteItem::Raw(_) => {
                        info!("Application: executed raw bus write");
                    }
                }
            } else {
                error!("Application: failed to write to bus");
            }
        }
    }

    pub(super) fn handle_set_command(&mut self, topic: &str, payload_raw: &str) {
        let trimmed = payload_raw.trim();

        // Ignore JSON payloads — these are our own echoes, not real commands
        if trimmed.starts_with('{') {
            return;
        }

        // Ignore empty payloads
        if trimmed.is_empty() {
            return;
        }

        let relative = self.topic_without_prefix(topic);

        // Special command: memory dump request
        if relative.ends_with("system/dump/set")
            || relative.ends_with("mem/dump")
            || relative.ends_with("Mem/dump")
        {
            info!("Application: memory dump requested via MQTT, queued for next bus gap");
            let dump_msg = protocol::build_bus_message(&[0x01, 0x01], 0x83, &[]);
            self.bus_write_stack.push_back(WriteItem::Raw(dump_msg));
            return;
        }

        let address = match self.resolve_command_address(&relative) {
            Some(a) => a,
            None => {
                warn!(
                    topic,
                    "Application: could not map command topic to register"
                );
                return;
            }
        };

        let register_info = match self.novus_mar.get(&address) {
            Some(info) => info.clone(),
            None => {
                warn!(address, "Application: no register metadata");
                return;
            }
        };

        // Reject writes to read-only registers
        let operations = register_info
            .get("operations")
            .and_then(|v| v.as_str())
            .unwrap_or("RO");
        if operations == "RO" {
            warn!(
                address,
                "Application: rejecting write to read-only register"
            );
            return;
        }

        let payload_cfg = extract_payload_config(&register_info);
        match payload::encode_command_payload(&payload_cfg, payload_raw) {
            Ok(value_bytes) => {
                // Per-register activity log on write path.
                if self.config.activity_log
                    && !register_info
                        .get("suppress_activity")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                {
                    let name = register_info
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let value = payload_raw.trim();
                    let body = format!("{address} {name} -> {value}");
                    self.emit_activity_line(&address, &body);
                }
                // Publish encoded value to a "last_set" topic for visibility
                // (not the command topic, to avoid feedback loop).
                let hex_str: String = value_bytes
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let set_json = json!({
                    "hex": hex_str,
                    "converted": payload_raw.trim(),
                    "timestamp": ts,
                });
                let state_topic = register_info
                    .pointer("/topic")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !state_topic.is_empty() {
                    if let Some(ref mqtt) = self.mqtt_client {
                        // handle_set_command is sync, so we spawn the publish.
                        // Bounded by a 5s timeout so a stuck broker can't accumulate
                        // tasks indefinitely under load.
                        let last_set_topic = format!("{state_topic}/last_set");
                        let client = mqtt.client();
                        let prefix = self.topic_prefix.clone();
                        let payload_str = set_json.to_string();
                        tokio::spawn(async move {
                            let full = format!(
                                "{}/{}",
                                prefix.trim_matches('/'),
                                last_set_topic.trim_matches('/')
                            );
                            let publish_fut = client.publish(
                                &full,
                                rumqttc::QoS::AtMostOnce,
                                true,
                                payload_str.as_bytes().to_vec(),
                            );
                            let _ = tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                publish_fut,
                            )
                            .await;
                        });
                    }
                }

                let item = WriteItem::Register {
                    address: address.clone(),
                    value_bytes,
                };
                if self.bus_write_stack.len() < self.write_queue_max_size {
                    info!(topic = %relative, address, "Application: queued write");
                    self.bus_write_stack.push_back(item);
                } else {
                    self.metrics.increment(&self.metrics.write_queue_dropped);
                    warn!("Application: write queue full, dropping");
                }
            }
            Err(e) => {
                error!(topic = %relative, error = %e, "Application: invalid command payload");
            }
        }
    }

    pub(super) fn topic_without_prefix(&self, topic: &str) -> String {
        let normalized = topic.trim_matches('/');
        let prefix = self.topic_prefix.trim_matches('/');
        if let Some(rest) = normalized.strip_prefix(&format!("{prefix}/")) {
            rest.to_string()
        } else {
            normalized.to_string()
        }
    }

    pub(super) fn resolve_command_address(&self, relative_topic: &str) -> Option<String> {
        let topic = relative_topic.trim_matches('/');

        if let Some(addr) = self.command_topic_to_address.get(topic) {
            return Some(addr.clone());
        }

        // Compatibility: Set/<path>
        if let Some(rest) = topic.strip_prefix("Set/") {
            let candidate = format!("{rest}/Set");
            if let Some(addr) = self.command_topic_to_address.get(&candidate) {
                return Some(addr.clone());
            }
        }

        // Direct address topic: 0x28/Set
        if topic.to_lowercase().ends_with("/set") {
            let parts: Vec<&str> = topic.split('/').collect();
            if parts.len() >= 2 {
                let addr = parts[parts.len() - 2].to_lowercase();
                if addr.starts_with("0x") && self.novus_mar.contains_key(&addr) {
                    return Some(addr);
                }
            }
        }

        None
    }
}
