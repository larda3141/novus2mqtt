use std::collections::HashMap;

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};

use crate::config::PayloadConfig;

/// Build a JSON MQTT payload from raw register bytes and payload configuration.
///
/// Returns a JSON object with `hex`, `converted`, and optionally `formatted` fields.
pub fn build_mqtt_payload(
    payload_cfg: &Option<PayloadConfig>,
    value_bytes: &[u8],
) -> Map<String, Value> {
    let hex_value: String = value_bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut mqtt_payload = Map::new();
    mqtt_payload.insert("hex".into(), Value::String(hex_value.clone()));

    let cfg = match payload_cfg {
        Some(c) => c,
        None => return mqtt_payload,
    };

    let payload_type = cfg.payload_type.as_deref().unwrap_or("byte");
    let mut converted: Option<Value> = None;

    // Try options lookup first (for enum/select entities)
    if let Some(ref options) = cfg.options {
        let compact_hex = hex_value.replace(' ', "").to_lowercase();
        if let Some(label) = options.get(&compact_hex) {
            converted = Some(Value::String(label.clone()));
        } else if let Some(label) = options.get(&compact_hex.to_uppercase()) {
            converted = Some(Value::String(label.clone()));
        }
    }

    // If options didn't match, check class-based conversions first
    if converted.is_none() {
        if let Some(ref class) = cfg.class {
            match class.as_str() {
                "version" if !value_bytes.is_empty() => {
                    // Version format: each byte is a version component, e.g. [1, 7, 1] → "1.7.1"
                    let version_str = value_bytes
                        .iter()
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join(".");
                    converted = Some(Value::String(version_str));
                }
                "bool" => {
                    // Boolean: 0 = false, non-zero = true
                    let val = value_bytes.iter().any(|&b| b != 0);
                    converted = Some(Value::Bool(val));
                }
                "code" if value_bytes.len() >= 2 => {
                    // Error code format: byte 0 = code, bytes 1-15 = ASCII message
                    let code = value_bytes[0];
                    let text: String = value_bytes[1..]
                        .iter()
                        .map(|&b| {
                            if (0x20..=0x7E).contains(&b) {
                                b as char
                            } else {
                                ' '
                            }
                        })
                        .collect::<String>()
                        .trim()
                        .to_string();
                    if text.is_empty() || text.chars().all(|c| c == ' ') {
                        converted = Some(Value::String(format!("0x{code:02X}")));
                    } else {
                        converted = Some(Value::String(format!("0x{code:02X}: {text}")));
                    }
                    mqtt_payload.insert("code".into(), json!(format!("0x{code:02X}")));
                    if !text.is_empty() {
                        mqtt_payload.insert("message".into(), Value::String(text));
                    }
                }
                "bitwise" if !value_bytes.is_empty() => {
                    // Time program: 2 bits per quarter hour, 8 bits per byte (8 bytes = 32 quarter-hours = 8 hours)
                    // 00=Off, 01=Level1, 10=Level2, 11=Level3
                    let level_names = ["Off", "L1", "L2", "L3"];
                    let mut schedule_parts: Vec<String> = Vec::new();
                    let mut current_level = 255u8;
                    let mut block_start = 0usize;

                    // Build per-slot array: each slot is a 15-minute quarter with its level (0-3)
                    let mut slots: Vec<u8> = Vec::new();
                    for &byte in value_bytes.iter() {
                        for bit_pair in (0..4).rev() {
                            let level = (byte >> (bit_pair * 2)) & 0x03;
                            slots.push(level);
                        }
                    }

                    // Build human-readable schedule summary
                    for (i, &level) in slots.iter().enumerate() {
                        if level != current_level {
                            if current_level != 255 && current_level != 0 {
                                let start_h = block_start / 4;
                                let start_m = (block_start % 4) * 15;
                                let end_h = i / 4;
                                let end_m = (i % 4) * 15;
                                schedule_parts.push(format!(
                                    "{:02}:{:02}-{:02}:{:02} {}",
                                    start_h,
                                    start_m,
                                    end_h,
                                    end_m,
                                    level_names[current_level as usize]
                                ));
                            }
                            current_level = level;
                            block_start = i;
                        }
                    }
                    // Close last block
                    if current_level != 255 && current_level != 0 {
                        let start_h = block_start / 4;
                        let start_m = (block_start % 4) * 15;
                        let end_h = slots.len() / 4;
                        let end_m = (slots.len() % 4) * 15;
                        schedule_parts.push(format!(
                            "{:02}:{:02}-{:02}:{:02} {}",
                            start_h, start_m, end_h, end_m, level_names[current_level as usize]
                        ));
                    }

                    let schedule = if schedule_parts.is_empty() {
                        "Off".to_string()
                    } else {
                        schedule_parts.join(", ")
                    };
                    converted = Some(Value::String(schedule.clone()));
                    mqtt_payload.insert("formatted".into(), Value::String(schedule));

                    // Add per-slot level array for programmatic access / HA use
                    let slot_levels: Vec<Value> = slots.iter().map(|&l| json!(l)).collect();
                    mqtt_payload.insert("slots".into(), Value::Array(slot_levels));
                }
                _ => {}
            }
        }
    }

    // If class didn't match, use type-based conversion
    if converted.is_none() {
        match payload_type {
            "char" => {
                let s: String = value_bytes
                    .iter()
                    .map(|&b| b as char)
                    .collect::<String>()
                    .trim_matches('\0')
                    .trim()
                    .to_string();
                converted = Some(Value::String(s));
            }
            "int" | "byte" | "word" => {
                // `word` = 2-byte unsigned (by convention). `int` can be signed.
                let signed = match payload_type {
                    "int" => cfg.signed.unwrap_or(false),
                    _ => false,
                };
                let raw = if signed {
                    i64::from(i32::from_le_bytes(pad_to_4(value_bytes, true)))
                } else {
                    i64::from(u32::from_le_bytes(pad_to_4(value_bytes, false)))
                };
                let decimals = cfg.decimals.unwrap_or(0);
                if decimals > 0 {
                    let div = 10_f64.powi(decimals);
                    converted = Some(json!(raw as f64 / div));
                } else {
                    converted = Some(json!(raw));
                }
            }
            "date" if value_bytes.len() >= 4 => {
                // Year byte is the offset from 2000 (00..=99). Clamp to two digits
                // so a misbehaving register can't produce e.g. "20200-MM-DD".
                let year = value_bytes[3].min(99);
                let s = format!("20{year:02}-{:02}-{:02}", value_bytes[2], value_bytes[1]);
                converted = Some(Value::String(s));
            }
            "time" if value_bytes.len() >= 3 => {
                let s = format!(
                    "{:02}:{:02}:{:02}",
                    value_bytes[2], value_bytes[1], value_bytes[0]
                );
                converted = Some(Value::String(s));
            }
            "counter" if value_bytes.len() >= 5 => {
                let years = value_bytes[4] as u64;
                let days = value_bytes[2] as u64;
                let hours = value_bytes[1] as u64;
                let minutes = value_bytes[0] as u64;
                let total_seconds =
                    years * 365 * 24 * 3600 + days * 24 * 3600 + hours * 3600 + minutes * 60;
                converted = Some(json!(total_seconds));
                mqtt_payload.insert(
                    "formatted".into(),
                    Value::String(format!(
                        "{years} years, {days} days, {hours} hours, {minutes} minutes"
                    )),
                );
            }
            "unknown" => {
                // Convert to all possible types for reverse engineering
                let uint_val = u32::from_le_bytes(pad_to_4(value_bytes, false));
                let int_val = i32::from_le_bytes(pad_to_4(value_bytes, true));
                mqtt_payload.insert("uint".into(), json!(uint_val));
                mqtt_payload.insert("int".into(), json!(int_val));

                // Text: replace non-printable with dots (hex viewer style)
                if value_bytes.len() > 1 {
                    let text: String = value_bytes
                        .iter()
                        .map(|&b| {
                            if (0x20..=0x7E).contains(&b) {
                                b as char
                            } else {
                                '.'
                            }
                        })
                        .collect();
                    mqtt_payload.insert("text".into(), Value::String(text));
                }

                // Char: strict ASCII (only if all bytes are printable)
                if value_bytes
                    .iter()
                    .all(|&b| b == 0 || (0x20..=0x7E).contains(&b))
                {
                    let s: String = value_bytes
                        .iter()
                        .map(|&b| b as char)
                        .collect::<String>()
                        .trim_matches('\0')
                        .trim()
                        .to_string();
                    if !s.is_empty() {
                        mqtt_payload.insert("char".into(), Value::String(s));
                    }
                }

                // Binary representation
                let bits: String = value_bytes
                    .iter()
                    .map(|b| format!("{b:08b}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                mqtt_payload.insert("bits".into(), Value::String(bits));

                // Date/time attempts if length matches
                if value_bytes.len() >= 4
                    && value_bytes[3] > 0
                    && value_bytes[3] <= 99
                    && value_bytes[2] >= 1
                    && value_bytes[2] <= 12
                    && value_bytes[1] >= 1
                    && value_bytes[1] <= 31
                {
                    mqtt_payload.insert(
                        "as_date".into(),
                        Value::String(format!(
                            "20{:02}-{:02}-{:02}",
                            value_bytes[3], value_bytes[2], value_bytes[1]
                        )),
                    );
                }
                if value_bytes.len() >= 3
                    && value_bytes[2] < 24
                    && value_bytes[1] < 60
                    && value_bytes[0] < 60
                {
                    mqtt_payload.insert(
                        "as_time".into(),
                        Value::String(format!(
                            "{:02}:{:02}:{:02}",
                            value_bytes[2], value_bytes[1], value_bytes[0]
                        )),
                    );
                }

                // Decimal with 1 digit
                if value_bytes.len() <= 2 {
                    mqtt_payload.insert("decimal_1".into(), json!(int_val as f64 / 10.0));
                }

                converted = Some(json!(uint_val));
            }
            _ => {}
        }
    } // end if converted.is_none()

    if let Some(ref conv) = converted {
        mqtt_payload.insert("converted".into(), conv.clone());
    }

    // Add available options for enum/select entities
    if let Some(ref options) = cfg.options {
        let values: Vec<Value> = options.values().map(|v| Value::String(v.clone())).collect();
        mqtt_payload.insert("options".into(), Value::Array(values));
    }

    // Formatted field with unit
    if let Some(ref unit) = cfg.unit_of_measurement {
        if let Some(ref conv) = converted {
            if (conv.is_number() || conv.is_f64()) && !mqtt_payload.contains_key("formatted") {
                mqtt_payload.insert(
                    "formatted".into(),
                    Value::String(format!("{} {unit}", conv)),
                );
            }
        }
    }

    mqtt_payload
}

/// Encode an MQTT command payload into register byte sequence.
pub fn encode_command_payload(
    payload_cfg: &Option<PayloadConfig>,
    payload_raw: &str,
) -> Result<Vec<u8>> {
    let cfg = match payload_cfg {
        Some(c) => c,
        None => bail!("No payload configuration for encoding"),
    };

    let payload_type = cfg.payload_type.as_deref().unwrap_or("byte");
    let length = cfg.length.unwrap_or(1) as usize;
    let value = payload_raw.trim();

    match payload_type {
        "char" => {
            let mut data = value.as_bytes().to_vec();
            data.truncate(length);
            data.resize(length, 0);
            return Ok(data);
        }
        "date" => {
            let parts: Vec<&str> = value.split('-').collect();
            if parts.len() != 3 {
                bail!("expected YYYY-MM-DD");
            }
            let year: i32 = parts[0].parse()?;
            let month: u8 = parts[1].parse()?;
            let day: u8 = parts[2].parse()?;
            // Compute weekday (Mon=0) using Tomohiko Sakamoto's algorithm
            let weekday = day_of_week(year, month, day);
            let mut result = vec![weekday, day, month, (year - 2000).max(0) as u8];
            result.truncate(length);
            return Ok(result);
        }
        "time" => {
            let parts: Vec<&str> = value.split(':').collect();
            if parts.len() < 2 {
                bail!("expected HH:MM or HH:MM:SS");
            }
            let hour: u8 = parts[0].parse()?;
            let minute: u8 = parts[1].parse()?;
            let second: u8 = if parts.len() >= 3 {
                parts[2].parse()?
            } else {
                0
            };
            let mut result = vec![second, minute, hour];
            result.truncate(length);
            return Ok(result);
        }
        "enum" | "byte" | "word" => {
            // Try options lookup first
            if let Some(ref options) = cfg.options {
                let lower_map: HashMap<String, String> = options
                    .iter()
                    .map(|(k, v)| (v.to_lowercase(), k.clone()))
                    .collect();
                if let Some(hex_key) = lower_map.get(&value.to_lowercase()) {
                    let clean = hex_key.to_lowercase().replace("0x", "").replace(' ', "");
                    if let Ok(bytes) = hex::decode_padded(&clean) {
                        let mut result = bytes;
                        result.truncate(length);
                        result.resize(length, 0);
                        return Ok(result);
                    }
                }
            }

            // Try hex parsing
            let clean = value.to_lowercase().replace("0x", "").replace(' ', "");
            let padded = if clean.len() % 2 == 1 {
                format!("0{clean}")
            } else {
                clean
            };
            if let Ok(bytes) = hex_decode(&padded) {
                let mut result = bytes;
                result.truncate(length);
                result.resize(length, 0);
                return Ok(result);
            }
        }
        _ => {}
    }

    // Fallback: numeric parsing
    let number: f64 = value
        .parse()
        .map_err(|_| anyhow::anyhow!("could not parse numeric payload '{value}'"))?;

    let decimals = cfg.decimals.unwrap_or(0);
    let scaled = if decimals > 0 {
        number * 10_f64.powi(decimals)
    } else {
        number
    };

    let signed = cfg
        .signed
        .unwrap_or(matches!(payload_type, "int" | "float"));
    let int_value = scaled.round() as i64;

    if length > 8 {
        bail!("payload length {length} too large for numeric encoding");
    }
    let bits = length * 8;
    let (min_val, max_val) = if signed {
        if bits >= 64 {
            (i64::MIN, i64::MAX)
        } else {
            (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)
        }
    } else {
        if bits >= 64 {
            (0i64, i64::MAX)
        } else {
            (0i64, (1i64 << bits) - 1)
        }
    };

    if int_value < min_val || int_value > max_val {
        bail!("value {int_value} out of range [{min_val}, {max_val}]");
    }

    let bytes = if signed {
        int_value.to_le_bytes()[..length].to_vec()
    } else {
        (int_value as u64).to_le_bytes()[..length].to_vec()
    };

    Ok(bytes)
}

/// Pad bytes to 4 bytes for integer conversion.
fn pad_to_4(bytes: &[u8], signed: bool) -> [u8; 4] {
    let mut result = if signed && !bytes.is_empty() && bytes[bytes.len() - 1] & 0x80 != 0 {
        [0xFF; 4]
    } else {
        [0; 4]
    };
    let len = bytes.len().min(4);
    result[..len].copy_from_slice(&bytes[..len]);
    result
}

/// Simple hex decode.
fn hex_decode(s: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(s.len() / 2);
    let chars: Vec<char> = s.chars().collect();
    if !chars.len().is_multiple_of(2) {
        bail!("odd-length hex string");
    }
    for chunk in chars.chunks(2) {
        let hi = hex_char(chunk[0])?;
        let lo = hex_char(chunk[1])?;
        bytes.push((hi << 4) | lo);
    }
    Ok(bytes)
}

fn hex_char(c: char) -> Result<u8> {
    match c {
        '0'..='9' => Ok(c as u8 - b'0'),
        'a'..='f' => Ok(c as u8 - b'a' + 10),
        'A'..='F' => Ok(c as u8 - b'A' + 10),
        _ => bail!("invalid hex character: {c}"),
    }
}

/// Helper module for hex decoding used in options lookup.
mod hex {
    pub fn decode_padded(s: &str) -> Result<Vec<u8>, String> {
        let padded = if s.len() % 2 == 1 {
            format!("0{s}")
        } else {
            s.to_string()
        };
        super::hex_decode(&padded).map_err(|e| e.to_string())
    }
}

/// Compute day of week (Monday=0, Sunday=6) for a given date.
/// Uses Tomohiko Sakamoto's algorithm. Returns 0 (Monday) for invalid `month`
/// values so callers don't panic on bad input from the MQTT side.
fn day_of_week(year: i32, month: u8, day: u8) -> u8 {
    if !(1..=12).contains(&month) {
        return 0;
    }
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = year;
    if month < 3 {
        y -= 1;
    }
    let dow = (y + y / 4 - y / 100 + y / 400 + t[(month - 1) as usize] + day as i32) % 7;
    // Sakamoto returns 0=Sunday, we need 0=Monday
    ((dow + 6) % 7) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_mqtt_payload_byte() {
        let cfg = PayloadConfig {
            length: Some(1),
            payload_type: Some("byte".into()),
            class: None,
            decimals: None,
            signed: None,
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: None,
            command: None,
            extra: Default::default(),
        };
        let payload = build_mqtt_payload(&Some(cfg), &[0x42]);
        assert_eq!(payload.get("hex").unwrap(), "42");
        assert_eq!(payload.get("converted").unwrap(), 0x42);
    }

    #[test]
    fn test_build_mqtt_payload_char() {
        let cfg = PayloadConfig {
            length: Some(5),
            payload_type: Some("char".into()),
            class: None,
            decimals: None,
            signed: None,
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: None,
            command: None,
            extra: Default::default(),
        };
        let payload = build_mqtt_payload(&Some(cfg), b"hello");
        assert_eq!(
            payload.get("converted").unwrap(),
            &Value::String("hello".into())
        );
    }

    #[test]
    fn test_encode_command_byte() {
        let cfg = PayloadConfig {
            length: Some(1),
            payload_type: Some("byte".into()),
            class: None,
            decimals: None,
            signed: None,
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: None,
            command: None,
            extra: Default::default(),
        };
        // "66" is valid hex, so for byte type it's decoded as 0x66
        let result = encode_command_payload(&Some(cfg), "66").unwrap();
        assert_eq!(result, vec![0x66]);
    }

    #[test]
    fn test_encode_decode_roundtrip_date() {
        let cfg = PayloadConfig {
            length: Some(4),
            payload_type: Some("date".into()),
            class: None,
            decimals: None,
            signed: None,
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: None,
            command: None,
            extra: Default::default(),
        };
        let encoded = encode_command_payload(&Some(cfg.clone()), "2025-03-15").unwrap();
        let payload = build_mqtt_payload(&Some(cfg), &encoded);
        assert_eq!(
            payload.get("converted").unwrap(),
            &Value::String("2025-03-15".into())
        );
    }

    #[test]
    fn test_version_class() {
        let cfg = PayloadConfig {
            length: Some(3),
            payload_type: Some("byte".into()),
            class: Some("version".into()),
            decimals: None,
            signed: None,
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: None,
            command: None,
            extra: Default::default(),
        };
        let payload = build_mqtt_payload(&Some(cfg), &[1, 7, 1]);
        assert_eq!(
            payload.get("converted").unwrap(),
            &Value::String("1.7.1".into())
        );
    }

    #[test]
    fn test_bool_class() {
        let cfg = PayloadConfig {
            length: Some(1),
            payload_type: Some("byte".into()),
            class: Some("bool".into()),
            decimals: None,
            signed: None,
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: None,
            command: None,
            extra: Default::default(),
        };
        let on = build_mqtt_payload(&Some(cfg.clone()), &[1]);
        assert_eq!(on.get("converted").unwrap(), &Value::Bool(true));

        let off = build_mqtt_payload(&Some(cfg), &[0]);
        assert_eq!(off.get("converted").unwrap(), &Value::Bool(false));
    }

    #[test]
    fn test_build_mqtt_payload_signed_int() {
        let cfg = PayloadConfig {
            length: Some(2),
            payload_type: Some("int".into()),
            class: None,
            decimals: Some(1),
            signed: Some(true),
            unit_of_measurement: Some("°C".into()),
            min: None,
            max: None,
            step: None,
            options: None,
            command: None,
            extra: Default::default(),
        };
        // -10.0°C = -100 as i16 = 0xFF9C in little-endian: [0x9C, 0xFF]
        let payload = build_mqtt_payload(&Some(cfg), &[0x9C, 0xFF]);
        let converted = payload.get("converted").unwrap().as_f64().unwrap();
        assert!((converted - (-10.0)).abs() < 0.01);
        assert!(payload
            .get("formatted")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("°C"));
    }

    #[test]
    fn test_build_mqtt_payload_enum_options() {
        let mut options = HashMap::new();
        options.insert("06".into(), "Auto".into());
        options.insert("07".into(), "Level 1".into());
        let cfg = PayloadConfig {
            length: Some(1),
            payload_type: Some("byte".into()),
            class: None,
            decimals: None,
            signed: None,
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: Some(options),
            command: None,
            extra: Default::default(),
        };
        let payload = build_mqtt_payload(&Some(cfg), &[0x06]);
        assert_eq!(payload.get("converted").unwrap(), "Auto");
        // Should include available options
        assert!(payload.get("options").is_some());
        let opts = payload.get("options").unwrap().as_array().unwrap();
        assert_eq!(opts.len(), 2);
    }

    #[test]
    fn test_build_mqtt_payload_code_class() {
        let cfg = PayloadConfig {
            length: Some(16),
            payload_type: Some("byte".into()),
            class: Some("code".into()),
            decimals: None,
            signed: None,
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: None,
            command: None,
            extra: Default::default(),
        };
        // 0xC7 + "Touch TFT 1    "
        let mut data = vec![0xC7];
        data.extend_from_slice(b"Touch TFT 1    ");
        let payload = build_mqtt_payload(&Some(cfg), &data);
        let converted = payload.get("converted").unwrap().as_str().unwrap();
        assert!(converted.contains("0xC7"));
        assert!(converted.contains("Touch TFT 1"));
        assert_eq!(payload.get("code").unwrap(), "0xC7");
    }

    #[test]
    fn test_build_mqtt_payload_time() {
        let cfg = PayloadConfig {
            length: Some(3),
            payload_type: Some("time".into()),
            class: None,
            decimals: None,
            signed: None,
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: None,
            command: None,
            extra: Default::default(),
        };
        // [second=45, minute=30, hour=21]
        let payload = build_mqtt_payload(&Some(cfg), &[0x2D, 0x1E, 0x15]);
        assert_eq!(payload.get("converted").unwrap(), "21:30:45");
    }

    #[test]
    fn test_build_mqtt_payload_empty_bytes() {
        // No config = just hex
        let payload = build_mqtt_payload(&None, &[0xAB, 0xCD]);
        assert_eq!(payload.get("hex").unwrap(), "ab cd");
        assert!(payload.get("converted").is_none());
    }

    #[test]
    fn test_encode_command_enum_reverse_lookup() {
        let mut options = HashMap::new();
        options.insert("06".into(), "Auto".into());
        let cfg = PayloadConfig {
            length: Some(1),
            payload_type: Some("byte".into()),
            class: None,
            decimals: None,
            signed: None,
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: Some(options),
            command: None,
            extra: Default::default(),
        };
        let result = encode_command_payload(&Some(cfg), "Auto").unwrap();
        assert_eq!(result, vec![0x06]);
    }

    #[test]
    fn test_encode_command_signed_negative() {
        let cfg = PayloadConfig {
            length: Some(2),
            payload_type: Some("int".into()),
            class: None,
            decimals: Some(1),
            signed: Some(true),
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: None,
            command: None,
            extra: Default::default(),
        };
        let result = encode_command_payload(&Some(cfg), "-2.0").unwrap();
        // -2.0 * 10 = -20 = 0xFFEC in LE: [0xEC, 0xFF]
        assert_eq!(result, vec![0xEC, 0xFF]);
    }

    #[test]
    fn test_encode_command_range_validation() {
        let cfg = PayloadConfig {
            length: Some(1),
            payload_type: Some("int".into()),
            class: None,
            decimals: None,
            signed: None,
            unit_of_measurement: None,
            min: None,
            max: None,
            step: None,
            options: None,
            command: None,
            extra: Default::default(),
        };
        // 256 doesn't fit in 1 byte (signed range: -128 to 127)
        assert!(encode_command_payload(&Some(cfg.clone()), "256").is_err());
        // -200 doesn't fit in 1 signed byte
        assert!(encode_command_payload(&Some(cfg), "-200").is_err());
    }

    #[test]
    fn test_pad_to_4_signed_extension() {
        // Negative 1-byte value should sign-extend
        let result = pad_to_4(&[0xF6], true); // -10 as i8
        assert_eq!(result, [0xF6, 0xFF, 0xFF, 0xFF]);
        let val = i32::from_le_bytes(result);
        assert_eq!(val, -10);
    }

    #[test]
    fn test_pad_to_4_unsigned() {
        let result = pad_to_4(&[0xF6], false);
        assert_eq!(result, [0xF6, 0x00, 0x00, 0x00]);
        let val = u32::from_le_bytes(result);
        assert_eq!(val, 246);
    }

    #[test]
    fn test_day_of_week() {
        // 2026-04-15 is a Wednesday = 2 (Monday=0)
        assert_eq!(day_of_week(2026, 4, 15), 2);
        // 2025-01-01 is a Wednesday = 2
        assert_eq!(day_of_week(2025, 1, 1), 2);
    }
}
