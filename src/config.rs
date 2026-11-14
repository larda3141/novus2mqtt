use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Top-level device configuration. Merged from `device.yaml` (unit identity)
/// and `registers.yaml` (static register map).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeviceYaml {
    pub device: DeviceInfo,
    #[serde(default)]
    pub sensors: Option<Vec<EntityEntry>>,
    #[serde(default)]
    pub controls: Option<Vec<EntityEntry>>,
    #[serde(default)]
    pub binary_sensors: Option<Vec<EntityEntry>>,
    /// Home Assistant child devices (parts of the unit). Taken from
    /// `registers.yaml` unless `device.yaml` defines its own list.
    #[serde(default)]
    pub child_devices: Option<Vec<ChildDeviceConfig>>,
}

/// A Home Assistant child device: one part of the unit (bypass, filter, ...)
/// shown as its own device below the main unit. Entities are assigned by
/// topic prefix; the longest matching prefix wins, unmatched entities stay on
/// the main device. Only `identifiers` and `name` reach Home Assistant — a
/// child device inherits the area of its parent.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChildDeviceConfig {
    /// Stable key, appended to the parent identifier. Changing it creates a
    /// new device in Home Assistant.
    pub id: String,
    /// Device name. Home Assistant prefixes entity names with it.
    pub name: String,
    /// Topic prefixes (as written in `registers.yaml`, e.g. `bus/bypass`).
    pub topics: Vec<String>,
    /// Leading words dropped from entity names in HA discovery so the
    /// friendly name doesn't repeat the device name ("Bypass Upper Threshold"
    /// on device "Novus Bypass" → "Upper Threshold"). Case-insensitive.
    #[serde(default)]
    pub strip_name_prefix: Vec<String>,
}

impl ChildDeviceConfig {
    /// Entity name as shown on this child device (prefix stripped).
    pub fn display_name(&self, name: &str) -> String {
        for prefix in &self.strip_name_prefix {
            let p = prefix.trim();
            if p.is_empty() || name.len() <= p.len() {
                continue;
            }
            let (head, rest) = name.split_at(p.len());
            if head.eq_ignore_ascii_case(p) && rest.starts_with(' ') {
                let rest = rest.trim();
                if !rest.is_empty() {
                    return rest.to_string();
                }
            }
        }
        name.to_string()
    }

    /// Length of the longest of this child's prefixes that matches `topic`.
    fn match_len(&self, topic: &str) -> Option<usize> {
        self.topics
            .iter()
            .filter(|p| !p.is_empty() && topic.starts_with(p.as_str()))
            .map(|p| p.len())
            .max()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    pub hw_version: Option<String>,
    pub sw_version: Option<String>,
    pub suggested_area: Option<String>,
    pub serial_number: Option<String>,
    pub model_id: Option<String>,
    pub configuration_url: Option<String>,
    /// Construction variant: "left" or "right". Determines temperature sensor mapping.
    pub variant: Option<String>,
    /// Per-unit override of the T1–T4 probe roles, for units whose probes don't
    /// follow the manufacturer's variant table. Keys `t1`..`t4`, values
    /// `intake` | `supply` | `extract` | `exhaust`; all four keys, each role once.
    pub temperature_sensors: Option<HashMap<String, String>>,
}

/// Air-flow role of a T1–T4 temperature probe: display name and MQTT topic.
fn temperature_role(role: &str) -> Option<(&'static str, &'static str)> {
    match role {
        "intake" => Some(("Intake Air Temperature", "bus/sensors/intake_air")),
        "supply" => Some(("Supply Air Temperature", "bus/sensors/supply_air")),
        "extract" => Some(("Extract Air Temperature", "bus/sensors/extract_air")),
        "exhaust" => Some(("Exhaust Air Temperature", "bus/sensors/exhaust_air")),
        _ => None,
    }
}

/// Register map file — loaded from `registers.yaml` alongside `device.yaml`.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct RegistersYaml {
    #[serde(default)]
    pub sensors: Option<Vec<EntityEntry>>,
    #[serde(default)]
    pub controls: Option<Vec<EntityEntry>>,
    #[serde(default)]
    pub binary_sensors: Option<Vec<EntityEntry>>,
    #[serde(default)]
    pub child_devices: Option<Vec<ChildDeviceConfig>>,
}

/// A sensor, control, or binary sensor entity entry from device.yaml.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EntityEntry {
    pub address: String,
    pub payload: Option<PayloadConfig>,
    pub topic: Option<String>,
    /// If false, this register is never published to MQTT. Shows up in the
    /// activity log as `[omitted]`. Default: true.
    #[serde(alias = "publish")]
    pub mqtt_publish: Option<bool>,
    pub expose_ha: Option<bool>,
    #[serde(rename = "convertTo")]
    pub convert_to: Option<String>,
    pub home_assistant: Option<HaEntityConfig>,
    pub unique_id: Option<String>,
    pub channel: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub operations: Option<String>,
    pub command_topic: Option<String>,
    /// If true, activity logging for this register is suppressed even when the
    /// global `ACTIVITY_LOG` env var is on. Use for very chatty registers
    /// (status bitmaps, high-frequency sensor broadcasts). Default false.
    pub suppress_activity: Option<bool>,
    /// Marks this register as "to be confirmed" — the meaning/decoding hasn't
    /// been fully verified. Drives the cyan TBC highlighter in the console.
    pub tbc: Option<bool>,
    /// MQTT publish throttling (deadband + heartbeat). Applies only when
    /// mqtt_publish is not explicitly false.
    pub throttle: Option<ThrottleConfig>,
    #[serde(rename = "type")]
    pub entity_type: Option<String>,
    pub options: Option<HashMap<String, String>>,
    pub device_class: Option<String>,
    pub state_class: Option<String>,
    pub unit: Option<String>,
    pub category: Option<String>,
    pub icon: Option<String>,
    // Allow extra fields
    #[serde(flatten)]
    pub extra: HashMap<String, JsonValue>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PayloadConfig {
    pub length: Option<u32>,
    #[serde(rename = "type")]
    pub payload_type: Option<String>,
    pub class: Option<String>,
    pub decimals: Option<i32>,
    pub signed: Option<bool>,
    pub unit_of_measurement: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub options: Option<HashMap<String, String>>,
    pub command: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, JsonValue>,
}

/// Per-register MQTT-publish throttling.
/// Publish happens if: first value OR (age >= max_interval) OR
/// (change >= min_change) OR (change_pct >= min_change_pct).
/// Skipped otherwise, OR if age < min_interval (rate-limit ceiling).
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ThrottleConfig {
    /// Absolute numeric change threshold (in the decoded unit).
    pub min_change: Option<f64>,
    /// Relative change threshold as percent of the previous value.
    pub min_change_pct: Option<f64>,
    /// Heartbeat in seconds — publish anyway if older than this.
    pub max_interval: Option<u64>,
    /// Rate-limit ceiling in seconds — never publish more often than this,
    /// regardless of change. First value always publishes (no prior to rate-limit).
    pub min_interval: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HaEntityConfig {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub config: Option<HashMap<String, JsonValue>>,
}

/// Loaded and validated device configuration.
#[derive(Debug, Clone)]
pub struct DeviceConfig {
    pub yaml: DeviceYaml,
}

impl DeviceConfig {
    /// Load configuration from `device.yaml`. The register map is loaded from
    /// `registers.yaml` in the same directory (or from `NOVUS_REGISTERS` if set).
    pub fn load(path: &Path) -> Result<Self> {
        let device_content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let mut yaml: DeviceYaml = serde_yaml_ng::from_str(&device_content)
            .with_context(|| format!("Invalid YAML in config file: {}", path.display()))?;

        let registers_path = std::env::var("NOVUS_REGISTERS")
            .map(std::path::PathBuf::from)
            .ok()
            .unwrap_or_else(|| {
                path.parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join("registers.yaml")
            });
        let registers_content = std::fs::read_to_string(&registers_path).with_context(|| {
            format!(
                "Failed to read registers file: {}",
                registers_path.display()
            )
        })?;
        let registers: RegistersYaml =
            serde_yaml_ng::from_str(&registers_content).with_context(|| {
                format!(
                    "Invalid YAML in registers file: {}",
                    registers_path.display()
                )
            })?;

        yaml.sensors = registers.sensors;
        yaml.controls = registers.controls;
        yaml.binary_sensors = registers.binary_sensors;
        if yaml.child_devices.is_none() {
            yaml.child_devices = registers.child_devices;
        }

        if yaml.sensors.is_none() && yaml.controls.is_none() && yaml.binary_sensors.is_none() {
            anyhow::bail!(
                "Registers file must have at least one of: sensors, controls, binary_sensors"
            );
        }

        let mut config = Self { yaml };
        config.validate_child_devices()?;
        config.apply_variant_variant()?;
        Ok(config)
    }

    /// Child device ids must be unique slugs and every child needs a name and
    /// at least one topic prefix.
    fn validate_child_devices(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for child in self.yaml.child_devices.iter().flatten() {
            if child.id.is_empty() || slugify(&child.id) != child.id {
                anyhow::bail!(
                    "child device id '{}' must be a non-empty slug (a-z, 0-9, _)",
                    child.id
                );
            }
            if !seen.insert(child.id.as_str()) {
                anyhow::bail!("duplicate child device id '{}'", child.id);
            }
            if child.name.trim().is_empty() {
                anyhow::bail!("child device '{}' has no name", child.id);
            }
            if child.topics.iter().all(|t| t.trim().is_empty()) {
                anyhow::bail!("child device '{}' has no topic prefixes", child.id);
            }
        }
        Ok(())
    }

    /// The child device an entity belongs to (longest topic-prefix match), or
    /// `None` if it stays on the main device.
    pub fn child_device_for(&self, entity: &EntityEntry) -> Option<&ChildDeviceConfig> {
        let topic = entity.topic.as_deref()?;
        self.yaml
            .child_devices
            .iter()
            .flatten()
            .filter_map(|c| c.match_len(topic).map(|n| (n, c)))
            .max_by_key(|(n, _)| *n)
            .map(|(_, c)| c)
    }

    /// Identifier of the main device (also the base of child identifiers).
    pub fn device_identifier(&self) -> String {
        let d = &self.yaml.device;
        format!(
            "{}_{}_{}",
            d.manufacturer,
            d.model,
            d.hw_version.as_deref().unwrap_or("unknown")
        )
    }

    /// Home Assistant identifier of a child device.
    pub fn child_identifier(&self, child: &ChildDeviceConfig) -> String {
        format!("{}_{}", self.device_identifier(), child.id)
    }

    /// Roles of the probes T1..T4, in that order: the per-unit
    /// `temperature_sensors` override if set, otherwise the manufacturer table
    /// for the variant (left/right).
    /// LEFT:  T1=Intake, T2=Supply, T3=Extract, T4=Exhaust
    /// RIGHT: T1=Extract, T2=Exhaust, T3=Intake, T4=Supply
    fn temperature_roles(&self) -> Result<[String; 4]> {
        let d = &self.yaml.device;
        if let Some(ref map) = d.temperature_sensors {
            for (key, role) in map {
                if !matches!(key.as_str(), "t1" | "t2" | "t3" | "t4") {
                    anyhow::bail!("temperature_sensors: unknown probe '{key}' (use t1..t4)");
                }
                if temperature_role(role).is_none() {
                    anyhow::bail!(
                        "temperature_sensors: {key} has unknown role '{role}' \
                         (use intake, supply, extract or exhaust)"
                    );
                }
            }
            let roles = ["t1", "t2", "t3", "t4"].map(|k| map.get(k).cloned().unwrap_or_default());
            if roles.iter().any(|r| r.is_empty()) {
                anyhow::bail!("temperature_sensors must set all of t1, t2, t3 and t4");
            }
            let unique: std::collections::HashSet<_> = roles.iter().collect();
            if unique.len() != 4 {
                anyhow::bail!("temperature_sensors must use each role exactly once");
            }
            return Ok(roles);
        }
        let is_right = d
            .variant
            .as_deref()
            .map(|c| c.eq_ignore_ascii_case("right"))
            .unwrap_or(false);
        let roles = if is_right {
            ["extract", "exhaust", "intake", "supply"]
        } else {
            ["intake", "supply", "extract", "exhaust"]
        };
        Ok(roles.map(String::from))
    }

    /// Apply temperature sensor name/topic overrides from the probe roles.
    /// The unique IDs (`status_sensor_t1`..`t4`) never change.
    fn apply_variant_variant(&mut self) -> Result<()> {
        let roles = self.temperature_roles()?;
        let mapping: Vec<(String, &str, &str)> = roles
            .iter()
            .enumerate()
            .map(|(i, role)| {
                // Safety: roles are validated (override) or constants (variant table).
                let (name, topic) = temperature_role(role).expect("validated role");
                (format!("status_sensor_t{}", i + 1), name, topic)
            })
            .collect();

        for section in [&mut self.yaml.sensors, &mut self.yaml.controls] {
            for entity in section.iter_mut().flatten() {
                let uid = entity.unique_id.as_deref().unwrap_or("");
                for (target_uid, name, topic) in &mapping {
                    if uid == target_uid {
                        entity.name = Some(name.to_string());
                        entity.topic = Some(topic.to_string());
                    }
                }
            }
        }
        Ok(())
    }

    /// Get all entities (sensors + controls + binary_sensors) as a flat list.
    pub fn all_entities(&self) -> Vec<&EntityEntry> {
        let mut entities = Vec::new();
        if let Some(ref sensors) = self.yaml.sensors {
            entities.extend(sensors.iter());
        }
        if let Some(ref controls) = self.yaml.controls {
            entities.extend(controls.iter());
        }
        if let Some(ref binary_sensors) = self.yaml.binary_sensors {
            entities.extend(binary_sensors.iter());
        }
        entities
    }

    /// Get device info for Home Assistant discovery.
    /// If `bridge_id` is provided, adds `via_device` to link to the bridge device.
    pub fn device_info_json(&self, bridge_id: Option<&str>) -> serde_json::Map<String, JsonValue> {
        let d = &self.yaml.device;
        let mut info = serde_json::Map::new();

        let id = self.device_identifier();
        info.insert("identifiers".into(), serde_json::json!([id]));
        info.insert("name".into(), JsonValue::String(d.name.clone()));
        info.insert("model".into(), JsonValue::String(d.model.clone()));
        info.insert(
            "manufacturer".into(),
            JsonValue::String(d.manufacturer.clone()),
        );

        if let Some(ref hw) = d.hw_version {
            info.insert("hw_version".into(), JsonValue::String(hw.clone()));
        }
        if let Some(ref sw) = d.sw_version {
            info.insert("sw_version".into(), JsonValue::String(sw.clone()));
        }
        let area = d.suggested_area.as_deref().unwrap_or("Ventilation");
        info.insert("suggested_area".into(), JsonValue::String(area.to_string()));

        if let Some(ref sn) = d.serial_number {
            info.insert("serial_number".into(), JsonValue::String(sn.clone()));
        }
        if let Some(ref mid) = d.model_id {
            info.insert("model_id".into(), JsonValue::String(mid.clone()));
        }
        if let Some(ref url) = d.configuration_url {
            info.insert("configuration_url".into(), JsonValue::String(url.clone()));
        }

        if let Some(bid) = bridge_id {
            info.insert("via_device".into(), JsonValue::String(bid.to_string()));
        }

        info
    }

    /// Resolve entity display name from top-level or HA config.
    pub fn entity_name(entity: &EntityEntry) -> String {
        if let Some(ref name) = entity.name {
            if !name.is_empty() {
                return name.clone();
            }
        }
        if let Some(ref ha) = entity.home_assistant {
            if let Some(ref config) = ha.config {
                if let Some(JsonValue::String(name)) = config.get("name") {
                    if !name.is_empty() {
                        return name.clone();
                    }
                }
            }
        }
        entity.address.clone()
    }

    /// Resolve entity unique_id.
    pub fn entity_unique_id(entity: &EntityEntry) -> String {
        if let Some(ref uid) = entity.unique_id {
            if !uid.is_empty() {
                return uid.clone();
            }
        }
        if let Some(ref ha) = entity.home_assistant {
            if let Some(ref config) = ha.config {
                if let Some(JsonValue::String(uid)) = config.get("uniq_id") {
                    if !uid.is_empty() {
                        return uid.clone();
                    }
                }
            }
        }
        slugify(&Self::entity_name(entity))
    }
}

/// Convert a string to a Home Assistant-safe entity ID token.
pub fn slugify(value: &str) -> String {
    let slug: String = value
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let slug = slug.trim_matches('_').to_string();
    // Collapse multiple underscores
    let mut result = String::new();
    let mut prev_underscore = false;
    for c in slug.chars() {
        if c == '_' {
            if !prev_underscore {
                result.push(c);
            }
            prev_underscore = true;
        } else {
            result.push(c);
            prev_underscore = false;
        }
    }
    if result.is_empty() {
        "entity".to_string()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Hello World"), "hello_world");
        assert_eq!(slugify("BUS Version"), "bus_version");
        assert_eq!(slugify("test--value"), "test_value");
        assert_eq!(slugify(""), "entity");
    }

    fn child(id: &str, name: &str, topics: &[&str], strip: &[&str]) -> ChildDeviceConfig {
        ChildDeviceConfig {
            id: id.into(),
            name: name.into(),
            topics: topics.iter().map(|s| s.to_string()).collect(),
            strip_name_prefix: strip.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn entity(topic: &str) -> EntityEntry {
        serde_yaml_ng::from_str(&format!("address: '0x01'\ntopic: {topic}\n")).unwrap()
    }

    fn config_with(children: Vec<ChildDeviceConfig>) -> DeviceConfig {
        let mut yaml: DeviceYaml = serde_yaml_ng::from_str(
            "device:\n  name: Unit\n  manufacturer: Maker\n  model: M1\n  hw_version: HW\n",
        )
        .unwrap();
        yaml.child_devices = Some(children);
        DeviceConfig { yaml }
    }

    #[test]
    fn child_display_name_strips_prefix() {
        let c = child("bypass", "Novus Bypass", &["bus/bypass"], &["Bypass"]);
        assert_eq!(c.display_name("Bypass Upper Threshold"), "Upper Threshold");
        assert_eq!(c.display_name("bypass mode"), "mode");
        // Only whole words, never the whole name.
        assert_eq!(c.display_name("Bypassed Hours"), "Bypassed Hours");
        assert_eq!(c.display_name("Bypass"), "Bypass");
        assert_eq!(c.display_name("Other Name"), "Other Name");
    }

    #[test]
    fn child_device_longest_prefix_wins() {
        let config = config_with(vec![
            child("auto", "Auto", &["bus/ventilation/automatic"], &[]),
            child(
                "sensor",
                "Sensor",
                &["bus/ventilation/automatic/sensor"],
                &[],
            ),
        ]);
        let pick = |t: &str| config.child_device_for(&entity(t)).map(|c| c.id.clone());
        assert_eq!(
            pick("bus/ventilation/automatic/sensor/output").as_deref(),
            Some("sensor")
        );
        assert_eq!(
            pick("bus/ventilation/automatic/mode").as_deref(),
            Some("auto")
        );
        assert_eq!(pick("bus/system/state"), None);
        assert_eq!(
            config.child_identifier(&config.yaml.child_devices.as_ref().unwrap()[1]),
            "Maker_M1_HW_sensor"
        );
    }

    #[test]
    fn child_device_validation() {
        assert!(config_with(vec![child("a", "A", &["x"], &[])])
            .validate_child_devices()
            .is_ok());
        assert!(config_with(vec![
            child("a", "A", &["x"], &[]),
            child("a", "B", &["y"], &[])
        ])
        .validate_child_devices()
        .is_err());
        assert!(config_with(vec![child("Bad Id", "A", &["x"], &[])])
            .validate_child_devices()
            .is_err());
        assert!(config_with(vec![child("a", "A", &[""], &[])])
            .validate_child_devices()
            .is_err());
    }

    fn config_with_device(extra: &str) -> DeviceConfig {
        let yaml: DeviceYaml = serde_yaml_ng::from_str(&format!(
            "device:\n  name: Unit\n  manufacturer: Maker\n  model: M1\n{extra}"
        ))
        .unwrap();
        DeviceConfig { yaml }
    }

    #[test]
    fn temperature_roles_follow_variant_table() {
        let left = config_with_device("  variant: left\n");
        assert_eq!(
            left.temperature_roles().unwrap(),
            ["intake", "supply", "extract", "exhaust"]
        );
        let right = config_with_device("  variant: right\n");
        assert_eq!(
            right.temperature_roles().unwrap(),
            ["extract", "exhaust", "intake", "supply"]
        );
    }

    #[test]
    fn temperature_roles_override() {
        let c = config_with_device(
            "  variant: left\n  temperature_sensors:\n    t1: intake\n    t2: supply\n    t3: exhaust\n    t4: extract\n",
        );
        assert_eq!(
            c.temperature_roles().unwrap(),
            ["intake", "supply", "exhaust", "extract"]
        );
        // Invalid: role twice, missing probe, unknown role, unknown probe.
        for bad in [
            "    t1: intake\n    t2: supply\n    t3: supply\n    t4: extract\n",
            "    t1: intake\n    t2: supply\n    t3: exhaust\n",
            "    t1: intake\n    t2: supply\n    t3: exhaust\n    t4: outdoor\n",
            "    t1: intake\n    t2: supply\n    t3: exhaust\n    t5: extract\n",
        ] {
            let c = config_with_device(&format!("  temperature_sensors:\n{bad}"));
            assert!(c.temperature_roles().is_err(), "accepted: {bad}");
        }
    }

    #[test]
    fn test_load_real_config() {
        let path = Path::new("config/device.yaml");
        if path.exists() {
            let config = DeviceConfig::load(path).unwrap();
            assert!(!config.yaml.device.name.is_empty());
            assert!(!config.all_entities().is_empty());
        }
    }
}
