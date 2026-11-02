use std::collections::HashMap;

use serde_json::{json, Map, Value};

use crate::config::{slugify, DeviceConfig, EntityEntry};

/// Generate Home Assistant MQTT discovery messages (component-per-entity).
pub fn generate_discovery(
    config: &DeviceConfig,
    hass_prefix: &str,
    mqtt_base_prefix: &str,
) -> Vec<DiscoveryMessage> {
    let mut messages = Vec::new();
    let discovery_node = mqtt_base_prefix.replace('/', "_");
    // Extract device_id from topic prefix (e.g., "novus2mqtt/novus300-xxx" -> "novus300-xxx")
    let device_id_part = mqtt_base_prefix
        .rsplit('/')
        .next()
        .unwrap_or(mqtt_base_prefix);
    let bridge_id = format!("novus2mqtt_{device_id_part}");
    let device_info = config.device_info_json(Some(&bridge_id));

    let origin = json!({
        "name": "novus2mqtt",
        "sw_version": config.yaml.device.sw_version.as_deref().unwrap_or("unknown"),
        "url": "https://github.com/larda3141/novus2mqtt"
    });

    let availability = json!([{
        "topic": format!("{mqtt_base_prefix}/status"),
        "qos": 1,
        "value_template": "{{ 'online' if value_json.connected | default(false) else 'offline' }}"
    }]);

    for entity in config.all_entities() {
        // Skip entities not exposed to Home Assistant
        if entity.expose_ha == Some(false) {
            continue;
        }

        let ha = match &entity.home_assistant {
            Some(ha) => ha,
            None => continue,
        };

        let entity_type = &ha.entity_type;
        let ha_config = ha.config.as_ref().cloned().unwrap_or_default();
        let unique_id = DeviceConfig::entity_unique_id(entity);
        let name = DeviceConfig::entity_name(entity);

        let mut enriched = Map::new();
        enriched.insert("~".into(), json!(mqtt_base_prefix));
        enriched.insert("uniq_id".into(), json!(unique_id));
        enriched.insert(
            "def_ent_id".into(),
            json!(format!("{entity_type}.{}", slugify(&unique_id))),
        );
        enriched.insert("o".into(), origin.clone());
        enriched.insert("dev".into(), Value::Object(device_info.clone()));
        enriched.insert("avty".into(), availability.clone());
        enriched.insert("avty_mode".into(), json!("all"));
        enriched.insert("payload_available".into(), json!("online"));
        enriched.insert("payload_not_available".into(), json!("offline"));
        enriched.insert("name".into(), json!(name));

        // State topic
        let stat_t = resolve_state_topic(&ha_config, entity, mqtt_base_prefix);
        enriched.insert("stat_t".into(), json!(stat_t));

        // Command topic
        if let Some(cmd_t) = resolve_command_topic(&ha_config, entity, mqtt_base_prefix) {
            enriched.insert("cmd_t".into(), json!(cmd_t));
        }

        // Value template
        let val_tpl = ha_config
            .get("val_tpl")
            .or_else(|| ha_config.get("value_template"))
            .cloned()
            .unwrap_or_else(|| {
                json!("{{ value_json.converted if value_json.converted is defined else value_json.hex }}")
            });
        enriched.insert("val_tpl".into(), val_tpl);

        // Copy standard HA fields
        copy_if_present(
            &ha_config,
            &mut enriched,
            &[
                ("unit_of_meas", "unit_of_meas"),
                ("unit_of_measurement", "unit_of_meas"),
                ("device_class", "dev_cla"),
                ("dev_cla", "dev_cla"),
                ("state_class", "stat_cla"),
                ("stat_cla", "stat_cla"),
                ("icon", "icon"),
                ("entity_category", "entity_category"),
                ("enabled_by_default", "enabled_by_default"),
                ("expire_after", "expire_after"),
                ("force_update", "force_update"),
                ("qos", "qos"),
                ("suggested_display_precision", "sug_dsp_prc"),
                ("mode", "mode"),
                ("min", "min"),
                ("max", "max"),
                ("step", "step"),
                ("options", "options"),
                ("payload_on", "payload_on"),
                ("payload_off", "payload_off"),
                ("payload_press", "payload_press"),
                ("retain", "retain"),
            ],
        );

        // Sanitize entity_category
        if let Some(cat) = enriched.get("entity_category").cloned() {
            if let Some(sanitized) = sanitize_entity_category(entity_type, cat.as_str()) {
                enriched.insert("entity_category".into(), json!(sanitized));
            } else {
                enriched.remove("entity_category");
            }
        }

        // No default `expire_after` — most registers are only pushed on the
        // startup memory dump or when the user changes a value, so a blanket
        // default would make them flap Unavailable. Registers that really ARE
        // continuously broadcast (live device state) opt in per-entry via
        // `home_assistant.config.expire_after` in registers.yaml.

        // Default command template for select entities
        if entity_type == "select" && enriched.contains_key("cmd_t") {
            enriched
                .entry("cmd_tpl".to_string())
                .or_insert(json!("{{ value }}"));
        }

        // Infer metadata from topic/payload when not explicitly set
        apply_inferred_metadata(entity, entity_type, &ha_config, &mut enriched);

        // Writable entity defaults
        if enriched.contains_key("cmd_t")
            && matches!(entity_type.as_str(), "switch" | "select" | "number")
        {
            enriched
                .entry("optimistic".to_string())
                .or_insert(json!(false));
        }

        // Switch state_on/state_off defaults
        if entity_type == "switch" {
            if let Some(on) = enriched.get("payload_on").cloned() {
                enriched.entry("state_on".to_string()).or_insert(on);
            }
            if let Some(off) = enriched.get("payload_off").cloned() {
                enriched.entry("state_off".to_string()).or_insert(off);
            }
        }

        if entity_type == "button" {
            enriched
                .entry("entity_category".to_string())
                .or_insert(json!("config"));
        }

        messages.push(DiscoveryMessage {
            topic: format!("{hass_prefix}/{entity_type}/{discovery_node}/{unique_id}/config"),
            payload: Value::Object(enriched),
        });
    }

    messages
}

/// Generate a single device-discovery payload with components map.
pub fn generate_device_discovery(
    config: &DeviceConfig,
    hass_prefix: &str,
    mqtt_base_prefix: &str,
) -> Vec<DiscoveryMessage> {
    let component_messages = generate_discovery(config, hass_prefix, mqtt_base_prefix);
    if component_messages.is_empty() {
        return Vec::new();
    }

    let discovery_node = mqtt_base_prefix.replace('/', "_");
    // Safety: component_messages is guaranteed non-empty (checked above) and each
    // payload is constructed as Value::Object in generate_discovery
    let first_payload = component_messages[0]
        .payload
        .as_object()
        .expect("discovery payload must be a JSON object");

    let mut device_payload = Map::new();
    device_payload.insert(
        "dev".into(),
        first_payload.get("dev").cloned().unwrap_or(json!({})),
    );
    device_payload.insert(
        "o".into(),
        first_payload.get("o").cloned().unwrap_or(json!({})),
    );
    device_payload.insert("~".into(), json!(mqtt_base_prefix));
    device_payload.insert(
        "avty".into(),
        first_payload.get("avty").cloned().unwrap_or(json!([])),
    );
    device_payload.insert("avty_mode".into(), json!("all"));
    device_payload.insert("payload_available".into(), json!("online"));
    device_payload.insert("payload_not_available".into(), json!("offline"));

    let mut cmps = Map::new();
    for msg in &component_messages {
        let Some(payload) = msg.payload.as_object() else {
            continue;
        };
        let unique_id = payload
            .get("uniq_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if unique_id.is_empty() {
            continue;
        }

        // Extract component platform from topic
        let parts: Vec<&str> = msg.topic.split('/').collect();
        let platform = if parts.len() > 1 { parts[1] } else { "sensor" };

        let mut component_cfg = payload.clone();
        for root_key in &[
            "dev",
            "o",
            "avty",
            "avty_mode",
            "payload_available",
            "payload_not_available",
            "def_ent_id",
        ] {
            component_cfg.remove(*root_key);
        }
        component_cfg.insert("p".into(), json!(platform));

        cmps.insert(unique_id.to_string(), Value::Object(component_cfg));
    }

    if cmps.is_empty() {
        return Vec::new();
    }

    device_payload.insert("cmps".into(), Value::Object(cmps));

    vec![DiscoveryMessage {
        topic: format!("{hass_prefix}/device/{discovery_node}/config"),
        payload: Value::Object(device_payload),
    }]
}

/// A Home Assistant discovery message (topic + JSON payload).
#[derive(Debug, Clone)]
pub struct DiscoveryMessage {
    pub topic: String,
    pub payload: Value,
}

fn resolve_state_topic(
    ha_config: &HashMap<String, Value>,
    entity: &EntityEntry,
    _mqtt_base_prefix: &str,
) -> String {
    let raw = ha_config
        .get("stat_t")
        .or_else(|| ha_config.get("state_topic"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| {
            let topic = entity.topic.as_deref().unwrap_or("unknown");
            format!("~/{topic}/state")
        });

    normalize_state_topic(&replace_base_prefix(&raw))
}

fn resolve_command_topic(
    ha_config: &HashMap<String, Value>,
    entity: &EntityEntry,
    _mqtt_base_prefix: &str,
) -> Option<String> {
    let raw = ha_config
        .get("cmd_t")
        .or_else(|| ha_config.get("command_topic"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| entity.command_topic.clone())
        // Fall back: if operations is RW and there's a state topic, derive
        // `{topic}/set` automatically so yaml entries can omit command_topic.
        .or_else(|| {
            let is_rw = entity
                .operations
                .as_deref()
                .map(|o| o.eq_ignore_ascii_case("RW"))
                .unwrap_or(false);
            if !is_rw {
                return None;
            }
            entity
                .topic
                .as_ref()
                .map(|t| format!("{}/set", t.trim_end_matches('/')))
        });

    raw.map(|t| normalize_command_topic(&replace_base_prefix(&t)))
}

fn replace_base_prefix(value: &str) -> String {
    value
        .replace("{{MQTT_BASE_PREFIX}}", "~")
        .replace("{MQTT_BASE_PREFIX}", "~")
}

fn normalize_state_topic(topic: &str) -> String {
    let t = topic.trim();
    let normalized = if t.starts_with("~/") {
        t.to_string()
    } else if let Some(rest) = t.strip_prefix('~') {
        format!("~/{}", rest.trim_start_matches('/'))
    } else {
        format!("~/{}", t.trim_start_matches('/'))
    };

    if normalized.ends_with("/state") {
        normalized
    } else {
        format!("{}/state", normalized.trim_end_matches('/'))
    }
}

fn normalize_command_topic(topic: &str) -> String {
    let t = topic.trim();
    if t.starts_with("~/") {
        t.to_string()
    } else if let Some(rest) = t.strip_prefix('~') {
        format!("~/{}", rest.trim_start_matches('/'))
    } else {
        format!("~/{}", t.trim_start_matches('/'))
    }
}

fn copy_if_present(
    source: &HashMap<String, Value>,
    dest: &mut Map<String, Value>,
    mappings: &[(&str, &str)],
) {
    for (src_key, dest_key) in mappings {
        if let Some(val) = source.get(*src_key) {
            dest.entry(dest_key.to_string())
                .or_insert_with(|| val.clone());
        }
    }
}

fn sanitize_entity_category(entity_type: &str, category: Option<&str>) -> Option<String> {
    let cat = category?.trim().to_lowercase();
    if cat.is_empty() {
        return None;
    }
    if matches!(entity_type, "sensor" | "binary_sensor") && cat != "diagnostic" {
        return Some("diagnostic".to_string());
    }
    Some(cat)
}

fn apply_inferred_metadata(
    entity: &EntityEntry,
    entity_type: &str,
    ha_config: &HashMap<String, Value>,
    enriched: &mut Map<String, Value>,
) {
    let topic = entity.topic.as_deref().unwrap_or("").to_lowercase();

    // Temperature inference
    if topic.contains("temperature") || topic.starts_with("temperatures/") {
        enriched
            .entry("unit_of_meas".to_string())
            .or_insert(json!("°C"));
        enriched
            .entry("dev_cla".to_string())
            .or_insert(json!("temperature"));
        enriched
            .entry("stat_cla".to_string())
            .or_insert(json!("measurement"));
        enriched
            .entry("sug_dsp_prc".to_string())
            .or_insert(json!(1));
    }

    // Humidity inference
    if topic.contains("humidity") {
        enriched
            .entry("unit_of_meas".to_string())
            .or_insert(json!("%"));
        enriched
            .entry("dev_cla".to_string())
            .or_insert(json!("humidity"));
        enriched
            .entry("stat_cla".to_string())
            .or_insert(json!("measurement"));
        enriched
            .entry("sug_dsp_prc".to_string())
            .or_insert(json!(1));
    }

    // Pressure inference
    if topic.contains("pressure") {
        enriched
            .entry("dev_cla".to_string())
            .or_insert(json!("pressure"));
        enriched
            .entry("stat_cla".to_string())
            .or_insert(json!("measurement"));
    }

    // Icon inference
    if !ha_config.contains_key("icon") && !enriched.contains_key("icon") {
        if topic.contains("temperature") {
            enriched.insert("icon".into(), json!("mdi:thermometer"));
        } else if topic.contains("humidity") {
            enriched.insert("icon".into(), json!("mdi:water-percent"));
        } else if topic.contains("frost") || topic.contains("defrost") {
            enriched.insert("icon".into(), json!("mdi:snowflake"));
        } else if topic.contains("bypass") {
            enriched.insert("icon".into(), json!("mdi:pipe"));
        } else if topic.contains("version") {
            enriched.insert("icon".into(), json!("mdi:information-outline"));
        } else if topic.contains("error") || topic.contains("message") {
            enriched.insert("icon".into(), json!("mdi:alert-circle-outline"));
        }
    }

    // Diagnostic category inference
    if !ha_config.contains_key("entity_category") && !enriched.contains_key("entity_category") {
        let diagnostic_tokens = ["version", "error", "message", "memory", "diagnostic"];
        if diagnostic_tokens.iter().any(|t| topic.contains(t)) {
            enriched.insert("entity_category".into(), json!("diagnostic"));
        }
    }

    // Numeric sensor defaults
    if entity_type == "sensor" {
        if let Some(ref cfg) = entity.payload {
            let pt = cfg.payload_type.as_deref().unwrap_or("");
            if matches!(pt, "float" | "int" | "uint" | "counter") {
                enriched
                    .entry("stat_cla".to_string())
                    .or_insert(json!("measurement"));
            }
            if pt == "float" {
                enriched
                    .entry("sug_dsp_prc".to_string())
                    .or_insert(json!(1));
            }
            if pt == "counter" {
                enriched.insert("stat_cla".into(), json!("total_increasing"));
            }

            // Enum sensor with options
            if let Some(ref options) = cfg.options {
                if !options.is_empty()
                    && !ha_config.contains_key("dev_cla")
                    && !ha_config.contains_key("device_class")
                    && !enriched.contains_key("dev_cla")
                {
                    enriched.insert("dev_cla".into(), json!("enum"));
                    enriched
                        .entry("options".to_string())
                        .or_insert(json!(options.values().collect::<Vec<_>>()));
                }
            }
        }
    }

    // Number entity default step
    if entity_type == "number" && !ha_config.contains_key("step") && !enriched.contains_key("step")
    {
        enriched.insert("step".into(), json!(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DeviceConfig;
    use std::path::Path;

    /// Golden test: exercise the real config through generate_discovery and
    /// assert basic invariants. Skips silently if the config files aren't
    /// present (e.g. in a stripped-down test environment).
    #[test]
    fn discovery_smoke_against_real_config() {
        let path = Path::new("config/device.yaml");
        if !path.exists() {
            return;
        }
        let config = DeviceConfig::load(path).unwrap();
        let messages = generate_discovery(&config, "homeassistant", "novus2mqtt/test-device");

        // At least one entity makes it through.
        assert!(!messages.is_empty(), "discovery emitted zero messages");

        // Topic uniqueness.
        let mut topics: Vec<&str> = messages.iter().map(|m| m.topic.as_str()).collect();
        topics.sort();
        let n_before = topics.len();
        topics.dedup();
        assert_eq!(n_before, topics.len(), "duplicate discovery topics emitted");

        // Every message has a well-formed payload with our required keys.
        for msg in &messages {
            let obj = msg
                .payload
                .as_object()
                .expect("payload must be a JSON object");
            assert!(
                obj.contains_key("uniq_id"),
                "missing uniq_id in {}",
                msg.topic
            );
            assert!(
                obj.contains_key("stat_t"),
                "missing stat_t in {}",
                msg.topic
            );
            assert!(obj.contains_key("dev"), "missing dev in {}", msg.topic);
            assert!(obj.contains_key("avty"), "missing avty in {}", msg.topic);
            assert!(obj.contains_key("name"), "missing name in {}", msg.topic);
            // Topic format: {hass_prefix}/{type}/{node}/{unique}/config
            assert!(msg.topic.starts_with("homeassistant/"));
            assert!(msg.topic.ends_with("/config"));
        }
    }

    /// Verify temperature inference: any entity whose topic contains
    /// "temperature" should get °C unit and temperature device class.
    #[test]
    fn discovery_infers_temperature_metadata() {
        let path = Path::new("config/device.yaml");
        if !path.exists() {
            return;
        }
        let config = DeviceConfig::load(path).unwrap();
        let messages = generate_discovery(&config, "homeassistant", "novus2mqtt/test-device");

        let temp_msgs: Vec<_> = messages
            .iter()
            .filter(|m| m.topic.contains("temperature") || m.topic.contains("/temp"))
            .collect();

        // If the config has any temperature entities, verify inference fired.
        for msg in temp_msgs {
            let obj = msg.payload.as_object().unwrap();
            // Auto-inference: unit_of_meas=°C, dev_cla=temperature.
            assert_eq!(
                obj.get("unit_of_meas").and_then(|v| v.as_str()),
                Some("°C"),
                "temperature entity {} missing °C unit",
                msg.topic
            );
            assert_eq!(
                obj.get("dev_cla").and_then(|v| v.as_str()),
                Some("temperature"),
                "temperature entity {} missing temperature device class",
                msg.topic
            );
        }
    }

    /// Diagnostic-token entities should auto-set entity_category=diagnostic.
    #[test]
    fn discovery_infers_diagnostic_category() {
        let path = Path::new("config/device.yaml");
        if !path.exists() {
            return;
        }
        let config = DeviceConfig::load(path).unwrap();
        let messages = generate_discovery(&config, "homeassistant", "novus2mqtt/test-device");

        for msg in &messages {
            let lower = msg.topic.to_lowercase();
            if ["version", "error", "message", "memory", "diagnostic"]
                .iter()
                .any(|t| lower.contains(t))
            {
                let obj = msg.payload.as_object().unwrap();
                let cat = obj.get("entity_category").and_then(|v| v.as_str());
                assert_eq!(
                    cat,
                    Some("diagnostic"),
                    "entity {} should have entity_category=diagnostic, got {:?}",
                    msg.topic,
                    cat
                );
            }
        }
    }

    /// Device-bundle discovery must produce a single payload with `cmps` map.
    #[test]
    fn device_discovery_bundles_components() {
        let path = Path::new("config/device.yaml");
        if !path.exists() {
            return;
        }
        let config = DeviceConfig::load(path).unwrap();
        let messages =
            generate_device_discovery(&config, "homeassistant", "novus2mqtt/test-device");

        assert_eq!(
            messages.len(),
            1,
            "device-bundle discovery should emit exactly one message"
        );
        let payload = messages[0].payload.as_object().unwrap();
        assert!(payload.contains_key("cmps"), "device payload missing cmps");
        let cmps = payload.get("cmps").unwrap().as_object().unwrap();
        assert!(!cmps.is_empty(), "cmps map is empty");
        assert!(messages[0].topic.contains("/device/"));
        assert!(messages[0].topic.ends_with("/config"));
    }
}
