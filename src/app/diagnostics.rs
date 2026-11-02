//! Bridge diagnostics: periodic status publish, HA discovery for the bridge's
//! own diagnostic entities, plus the small system-info helpers used to fill them
//! (CPU temp, local IP, stable device-id).

use std::time::{Duration, Instant};

use blake2::digest::Digest;
use blake2::Blake2s256;
use rumqttc::QoS;
use serde_json::json;
use tracing::info;

use crate::config::DeviceConfig;
use crate::mqtt::MqttClient;

use super::NovusConnector;

impl NovusConnector {
    pub(super) async fn publish_diagnostics_if_due(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_diagnostics_ts) < Duration::from_secs(30) {
            return;
        }
        self.last_diagnostics_ts = now;

        let snapshot = self.metrics.snapshot();
        let mqtt_connected = self
            .mqtt_client
            .as_ref()
            .map(|m| m.is_connected())
            .unwrap_or(false);
        let serial_ok = self
            .serial_handler
            .as_ref()
            .map(|s| s.fatal_error.is_none())
            .unwrap_or(false);

        let uptime_secs = snapshot.uptime_seconds as u64;
        let uptime_str = format!(
            "{:03}+{:02}:{:02}:{:02}",
            uptime_secs / 86400,
            (uptime_secs % 86400) / 3600,
            (uptime_secs % 3600) / 60,
            uptime_secs % 60
        );

        let cpu_temp = read_cpu_temperature();
        let ip_address = get_local_ip();
        let bus_idle_secs = now.duration_since(self.last_bus_message_ts).as_secs();
        let bus_alive = bus_idle_secs < 60;
        let bus_status = if !serial_ok {
            "error"
        } else if !bus_alive {
            "no data"
        } else {
            "connected"
        };

        // Update status topic with connection health
        let status = json!({
            "connected": mqtt_connected && serial_ok,
            "bus": bus_status,
            "mqtt": if mqtt_connected { "connected" } else { "disconnected" },
        });
        if let Some(ref mqtt) = self.mqtt_client {
            mqtt.publish("status", &status.to_string(), QoS::AtLeastOnce, true, true)
                .await;
        }

        let diagnostics = json!({
            "version": self.config.app_version,
            "uptime": uptime_str,
            "uptime_seconds": uptime_secs,
            "bus_status": bus_status,
            "bus_rx": snapshot.bus_messages_received,
            "bus_tx": snapshot.bus_messages_sent,
            "bus_rx_errors": snapshot.errors_count,
            "bus_registers_seen": self.registers_seen.len(),
            "bus_registers_total": self.address_metadata.len(),
            "mqtt_status": if mqtt_connected { "connected" } else { "disconnected" },
            "mqtt_published": snapshot.mqtt_messages_sent,
            "mqtt_received": snapshot.mqtt_messages_received,
            "mqtt_write_queue": self.bus_write_stack.len(),
            "mqtt_write_dropped": snapshot.write_queue_dropped,
            "memory_mb": (snapshot.memory_usage_mb * 10.0).round() / 10.0,
            "cpu_percent": (snapshot.cpu_percent * 10.0).round() / 10.0,
            "cpu_temp": cpu_temp,
            "ip_address": ip_address,
        });

        if let Some(ref mqtt) = self.mqtt_client {
            mqtt.publish_retain("diagnostics", &diagnostics.to_string())
                .await;
        }
    }

    /// Publish HA discovery configs for bridge diagnostic entities.
    pub(super) async fn publish_bridge_diagnostics_discovery(&self, mqtt: &MqttClient) {
        let device_info = json!({
            "identifiers": [format!("novus2mqtt_{}", self.device_id)],
            "name": "novus2mqtt Bridge",
            "manufacturer": "novus2mqtt",
            "model": "MQTT Bridge",
            "sw_version": self.config.app_version,
        });

        let diag_topic = format!("{}/diagnostics", self.topic_prefix);
        let hass = &self.config.hass_discovery_prefix;
        let node = self.topic_prefix.replace('/', "_");

        type DiagEntity<'a> = (
            &'a str,
            &'a str,
            &'a str,
            &'a str,
            Option<&'a str>,
            Option<&'a str>,
            Option<&'a str>,
        );
        let entities: Vec<DiagEntity<'_>> = vec![
            // (unique_suffix, name, val_template, icon, device_class, state_class, unit)
            (
                "version",
                "Version",
                "{{ value_json.version }}",
                "mdi:tag",
                None,
                None,
                None,
            ),
            (
                "uptime",
                "Uptime",
                "{{ value_json.uptime }}",
                "mdi:clock-outline",
                None,
                None,
                None,
            ),
            (
                "uptime_sec",
                "Uptime (sec)",
                "{{ value_json.uptime_seconds }}",
                "mdi:timer-outline",
                Some("duration"),
                Some("total_increasing"),
                Some("s"),
            ),
            (
                "bus_status",
                "Bus Status",
                "{{ value_json.bus_status }}",
                "mdi:serial-port",
                None,
                None,
                None,
            ),
            (
                "bus_rx",
                "Bus Rx",
                "{{ value_json.bus_rx }}",
                "mdi:arrow-down",
                None,
                Some("total_increasing"),
                None,
            ),
            (
                "bus_tx",
                "Bus Tx",
                "{{ value_json.bus_tx }}",
                "mdi:arrow-up",
                None,
                Some("total_increasing"),
                None,
            ),
            (
                "bus_rx_errors",
                "Bus Rx Errors",
                "{{ value_json.bus_rx_errors }}",
                "mdi:alert-circle",
                None,
                Some("total_increasing"),
                None,
            ),
            (
                "mqtt_status",
                "MQTT Status",
                "{{ value_json.mqtt_status }}",
                "mdi:lan-connect",
                None,
                None,
                None,
            ),
            (
                "mqtt_pub",
                "MQTT Published",
                "{{ value_json.mqtt_published }}",
                "mdi:publish",
                None,
                Some("total_increasing"),
                None,
            ),
            (
                "mqtt_recv",
                "MQTT Received",
                "{{ value_json.mqtt_received }}",
                "mdi:download",
                None,
                Some("total_increasing"),
                None,
            ),
            (
                "memory",
                "Memory Usage",
                "{{ value_json.memory_mb }}",
                "mdi:memory",
                Some("data_size"),
                Some("measurement"),
                Some("MB"),
            ),
            (
                "cpu",
                "CPU Usage",
                "{{ value_json.cpu_percent }}",
                "mdi:cpu-64-bit",
                None,
                Some("measurement"),
                Some("%"),
            ),
            (
                "cpu_temp",
                "CPU Temperature",
                "{{ value_json.cpu_temp }}",
                "mdi:thermometer",
                Some("temperature"),
                Some("measurement"),
                Some("°C"),
            ),
            (
                "ip",
                "IP Address",
                "{{ value_json.ip_address }}",
                "mdi:ip-network",
                None,
                None,
                None,
            ),
        ];

        for (suffix, name, val_tpl, icon, dev_cla, stat_cla, unit) in &entities {
            let unique_id = format!("novus2mqtt_{}", suffix);
            let mut payload = json!({
                "name": name,
                "unique_id": unique_id,
                "stat_t": diag_topic,
                "val_tpl": val_tpl,
                "icon": icon,
                "entity_category": "diagnostic",
                "dev": device_info,
                "expire_after": 120,
            });

            if let Some(dc) = dev_cla {
                payload["dev_cla"] = json!(dc);
            }
            if let Some(sc) = stat_cla {
                payload["stat_cla"] = json!(sc);
            }
            if let Some(u) = unit {
                payload["unit_of_meas"] = json!(u);
            }

            let topic = format!("{hass}/sensor/{node}/novus2mqtt_{suffix}/config");
            let payload_str = serde_json::to_string(&payload).unwrap_or_default();
            mqtt.publish(&topic, &payload_str, QoS::AtMostOnce, true, false)
                .await;
        }

        info!(
            count = entities.len(),
            "Application: published bridge diagnostic discovery"
        );
    }
}

/// Compute a stable device identifier for MQTT topic namespacing.
pub(super) fn compute_device_id(device_config: &DeviceConfig, client_id: &str) -> String {
    let device = &device_config.yaml.device;

    if let Some(ref sn) = device.serial_number {
        let raw = format!("{}-{}", device.model, sn).to_lowercase();
        let sanitized: String = raw
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        return sanitized.trim_matches('-').to_string();
    }

    let fingerprint = format!(
        "{}|{}|{}|{}",
        device.name,
        device.model,
        device.hw_version.as_deref().unwrap_or("unknown"),
        client_id,
    );

    let mut hasher = Blake2s256::new();
    hasher.update(fingerprint.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();

    let model_lower = device.model.to_lowercase();
    let sanitized: String = model_lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    format!("{}-{hex}", sanitized.trim_matches('-'))
}

/// Read CPU temperature from sysfs (Linux/RPi). Returns 0.0 if unavailable.
pub(super) fn read_cpu_temperature() -> f64 {
    std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|t| (t / 100.0).round() / 10.0) // millidegrees -> °C, 1 decimal
        .unwrap_or(0.0)
}

/// Get the primary local IP address by opening a UDP socket toward a public
/// address (no packet is sent — this just resolves the routing-table choice).
fn get_local_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|sock| {
            sock.connect("8.8.8.8:80")?;
            sock.local_addr()
        })
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}
