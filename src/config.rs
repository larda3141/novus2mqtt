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

        if yaml.sensors.is_none() && yaml.controls.is_none() && yaml.binary_sensors.is_none() {
            anyhow::bail!(
                "Registers file must have at least one of: sensors, controls, binary_sensors"
            );
        }

        let mut config = Self { yaml };
        config.apply_variant_variant();
        Ok(config)
    }

    /// Apply temperature sensor name/topic overrides based on the variant variant (left/right).
    /// LEFT:  T1=Intake, T2=Supply, T3=Extract, T4=Exhaust
    /// RIGHT: T1=Extract, T2=Exhaust, T3=Intake, T4=Supply
    fn apply_variant_variant(&mut self) {
        let is_right = self
            .yaml
            .device
            .variant
            .as_deref()
            .map(|c| c.eq_ignore_ascii_case("right"))
            .unwrap_or(false);

        let mapping: &[(&str, &str, &str)] = if is_right {
            &[
                (
                    "status_sensor_t1",
                    "Extract Air Temperature",
                    "bus/sensors/extract_air",
                ),
                (
                    "status_sensor_t2",
                    "Exhaust Air Temperature",
                    "bus/sensors/exhaust_air",
                ),
                (
                    "status_sensor_t3",
                    "Intake Air Temperature",
                    "bus/sensors/intake_air",
                ),
                (
                    "status_sensor_t4",
                    "Supply Air Temperature",
                    "bus/sensors/supply_air",
                ),
            ]
        } else {
            // LEFT (default)
            &[
                (
                    "status_sensor_t1",
                    "Intake Air Temperature",
                    "bus/sensors/intake_air",
                ),
                (
                    "status_sensor_t2",
                    "Supply Air Temperature",
                    "bus/sensors/supply_air",
                ),
                (
                    "status_sensor_t3",
                    "Extract Air Temperature",
                    "bus/sensors/extract_air",
                ),
                (
                    "status_sensor_t4",
                    "Exhaust Air Temperature",
                    "bus/sensors/exhaust_air",
                ),
            ]
        };

        for section in [&mut self.yaml.sensors, &mut self.yaml.controls] {
            for entity in section.iter_mut().flatten() {
                let uid = entity.unique_id.as_deref().unwrap_or("");
                for &(target_uid, name, topic) in mapping {
                    if uid == target_uid {
                        entity.name = Some(name.to_string());
                        entity.topic = Some(topic.to_string());
                    }
                }
            }
        }
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

        let id = format!(
            "{}_{}_{}",
            d.manufacturer,
            d.model,
            d.hw_version.as_deref().unwrap_or("unknown")
        );
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
