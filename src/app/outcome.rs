//! Pure decision logic for "should we publish this register to MQTT, and why?".
//!
//! Kept as free functions / plain enums with no IO, so it's straightforward
//! to test against synthetic inputs without spinning up a `NovusConnector`.

use std::time::Instant;

use super::registers::AddressMeta;

/// Why a publish happened — appears in the activity log and bus dump.
#[derive(Debug, Clone, Copy)]
pub(super) enum PublishReason {
    /// First decoded value for this register since startup.
    New,
    /// Value differs from the previous publish (bytes changed).
    Changed,
    /// Value hasn't changed but we published anyway: either the heartbeat
    /// (`max_interval`) elapsed, or no throttling is configured.
    Stale,
}

/// Whether and why we published — covers the skip cases too.
#[derive(Debug, Clone, Copy)]
pub(super) enum MqttOutcome {
    Published(PublishReason),
    SkippedDelta,
    SkippedHeld,
    SkippedRate,
    Omitted,
}

impl MqttOutcome {
    pub(super) fn tag(&self) -> &'static str {
        match self {
            Self::Published(PublishReason::New) => "[published: new]",
            Self::Published(PublishReason::Changed) => "[published: changed]",
            Self::Published(PublishReason::Stale) => "[published: stale]",
            Self::SkippedDelta => "[skipped: delta]",
            Self::SkippedHeld => "[skipped: held]",
            Self::SkippedRate => "[skipped: rate]",
            Self::Omitted => "[omitted]",
        }
    }
}

/// Last successful publish for a register — what we compare new values against.
#[derive(Debug, Clone)]
pub(crate) struct ThrottleState {
    /// Raw hex string of the previously published payload. Used for equality
    /// checks on non-numeric (and numeric) types — `hex` is the authoritative
    /// representation, so if these match the bytes didn't change.
    pub(crate) last_hex: String,
    /// Decoded value. Used for numeric delta comparison.
    pub(crate) last_value: serde_json::Value,
    pub(crate) last_published: Instant,
}

/// Decide whether to publish a decoded register to MQTT, given per-register
/// yaml config, global defaults, the previous state, and the new value.
#[allow(clippy::too_many_arguments)]
pub(super) fn decide_mqtt_outcome(
    register_info: &serde_json::Value,
    address_meta: Option<&AddressMeta>,
    prev_state: Option<&ThrottleState>,
    new_hex: &str,
    new_value: &serde_json::Value,
    now: Instant,
    global_max_interval: u64,
    global_min_change_pct: f64,
    disable_throttle: bool,
) -> MqttOutcome {
    if let Some(m) = address_meta {
        if !m.mqtt_publish {
            return MqttOutcome::Omitted;
        }
    }

    let prev = match prev_state {
        Some(s) => s,
        None => return MqttOutcome::Published(PublishReason::New),
    };

    // Global throttle bypass — every publish goes through, reason computed
    // from hex comparison alone.
    if disable_throttle {
        return if new_hex == prev.last_hex {
            MqttOutcome::Published(PublishReason::Stale)
        } else {
            MqttOutcome::Published(PublishReason::Changed)
        };
    }

    let throttle = register_info.get("throttle");
    let reg_max_interval = throttle
        .and_then(|t| t.get("max_interval"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let reg_min_interval = throttle
        .and_then(|t| t.get("min_interval"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let reg_min_change = throttle
        .and_then(|t| t.get("min_change"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let reg_min_change_pct = throttle
        .and_then(|t| t.get("min_change_pct"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let max_interval = if reg_max_interval > 0 {
        reg_max_interval
    } else {
        global_max_interval
    };
    let min_interval = reg_min_interval; // no global default for min_interval — per-register only
    let min_change = reg_min_change;
    let min_change_pct = if reg_min_change_pct > 0.0 {
        reg_min_change_pct
    } else {
        global_min_change_pct
    };

    // No throttle configured anywhere → publish every time (legacy behaviour).
    // Still distinguish "changed" vs "stale repeat" for the reason tag.
    if max_interval == 0 && min_interval == 0 && min_change == 0.0 && min_change_pct == 0.0 {
        return if new_hex == prev.last_hex {
            MqttOutcome::Published(PublishReason::Stale)
        } else {
            MqttOutcome::Published(PublishReason::Changed)
        };
    }

    let age = now.duration_since(prev.last_published).as_secs();

    // Rate-limit ceiling: never publish more often than min_interval.
    if min_interval > 0 && age < min_interval {
        return MqttOutcome::SkippedRate;
    }

    // Heartbeat: publish anyway if older than max_interval.
    if max_interval > 0 && age >= max_interval {
        return MqttOutcome::Published(PublishReason::Stale);
    }

    // Exact hex match → bytes didn't change.
    if new_hex == prev.last_hex {
        return MqttOutcome::SkippedHeld;
    }

    // Numeric comparison.
    if let (Some(a), Some(b)) = (prev.last_value.as_f64(), new_value.as_f64()) {
        let delta = (b - a).abs();
        if min_change > 0.0 && delta >= min_change {
            return MqttOutcome::Published(PublishReason::Changed);
        }
        let base = a.abs().max(1.0);
        let pct = (delta / base) * 100.0;
        if min_change_pct > 0.0 && pct >= min_change_pct {
            return MqttOutcome::Published(PublishReason::Changed);
        }
        return MqttOutcome::SkippedDelta;
    }

    // Non-numeric and hex differs → publish.
    MqttOutcome::Published(PublishReason::Changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn meta_publish(publish: bool) -> AddressMeta {
        AddressMeta {
            mqtt_publish: publish,
            command_topic: None,
        }
    }

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn outcome_omitted_when_mqtt_publish_false() {
        let info = json!({});
        let meta = meta_publish(false);
        let out = decide_mqtt_outcome(
            &info,
            Some(&meta),
            None,
            "01",
            &json!(1),
            now(),
            0,
            0.0,
            false,
        );
        assert!(matches!(out, MqttOutcome::Omitted));
    }

    #[test]
    fn outcome_new_when_no_prev_state() {
        let info = json!({});
        let out = decide_mqtt_outcome(&info, None, None, "01", &json!(1), now(), 0, 0.0, false);
        assert!(matches!(out, MqttOutcome::Published(PublishReason::New)));
    }

    #[test]
    fn outcome_no_throttle_emits_stale_or_changed() {
        let info = json!({});
        let prev = ThrottleState {
            last_hex: "01".into(),
            last_value: json!(1),
            last_published: now(),
        };
        let same = decide_mqtt_outcome(
            &info,
            None,
            Some(&prev),
            "01",
            &json!(1),
            now(),
            0,
            0.0,
            false,
        );
        assert!(matches!(same, MqttOutcome::Published(PublishReason::Stale)));

        let diff = decide_mqtt_outcome(
            &info,
            None,
            Some(&prev),
            "02",
            &json!(2),
            now(),
            0,
            0.0,
            false,
        );
        assert!(matches!(
            diff,
            MqttOutcome::Published(PublishReason::Changed)
        ));
    }

    #[test]
    fn outcome_disable_throttle_bypasses_thresholds() {
        let info = json!({"throttle": {"min_change": 100.0, "min_interval": 9999}});
        let prev = ThrottleState {
            last_hex: "01".into(),
            last_value: json!(1.0),
            last_published: now(),
        };
        // Even with strict thresholds, disable_throttle=true publishes.
        let out = decide_mqtt_outcome(
            &info,
            None,
            Some(&prev),
            "02",
            &json!(1.5),
            now(),
            0,
            0.0,
            true,
        );
        assert!(matches!(
            out,
            MqttOutcome::Published(PublishReason::Changed)
        ));
    }

    #[test]
    fn outcome_skipped_held_when_hex_matches_with_throttle() {
        let info = json!({"throttle": {"max_interval": 60}});
        let prev = ThrottleState {
            last_hex: "0a".into(),
            last_value: json!(10),
            last_published: now(),
        };
        let out = decide_mqtt_outcome(
            &info,
            None,
            Some(&prev),
            "0a",
            &json!(10),
            now(),
            0,
            0.0,
            false,
        );
        assert!(matches!(out, MqttOutcome::SkippedHeld));
    }

    #[test]
    fn outcome_skipped_rate_when_min_interval_not_elapsed() {
        let info = json!({"throttle": {"min_interval": 60}});
        let prev = ThrottleState {
            last_hex: "01".into(),
            last_value: json!(1),
            last_published: now(),
        };
        let out = decide_mqtt_outcome(
            &info,
            None,
            Some(&prev),
            "02",
            &json!(2),
            now(),
            0,
            0.0,
            false,
        );
        assert!(matches!(out, MqttOutcome::SkippedRate));
    }

    #[test]
    fn outcome_published_stale_when_max_interval_elapsed() {
        let info = json!({"throttle": {"max_interval": 1}});
        let prev = ThrottleState {
            last_hex: "01".into(),
            last_value: json!(1),
            last_published: now() - Duration::from_secs(2),
        };
        let out = decide_mqtt_outcome(
            &info,
            None,
            Some(&prev),
            "01",
            &json!(1),
            now(),
            0,
            0.0,
            false,
        );
        assert!(matches!(out, MqttOutcome::Published(PublishReason::Stale)));
    }

    #[test]
    fn outcome_skipped_delta_when_numeric_change_below_threshold() {
        let info = json!({"throttle": {"min_change": 5.0, "max_interval": 0}});
        let prev = ThrottleState {
            last_hex: "0a".into(),
            last_value: json!(10.0),
            last_published: now(),
        };
        let out = decide_mqtt_outcome(
            &info,
            None,
            Some(&prev),
            "0b",
            &json!(11.0),
            now(),
            0,
            0.0,
            false,
        );
        assert!(matches!(out, MqttOutcome::SkippedDelta));
    }

    #[test]
    fn outcome_published_when_numeric_change_above_threshold() {
        let info = json!({"throttle": {"min_change": 5.0, "max_interval": 0}});
        let prev = ThrottleState {
            last_hex: "0a".into(),
            last_value: json!(10.0),
            last_published: now(),
        };
        let out = decide_mqtt_outcome(
            &info,
            None,
            Some(&prev),
            "14",
            &json!(20.0),
            now(),
            0,
            0.0,
            false,
        );
        assert!(matches!(
            out,
            MqttOutcome::Published(PublishReason::Changed)
        ));
    }

    #[test]
    fn outcome_global_throttle_used_when_no_per_register_setting() {
        let info = json!({});
        let prev = ThrottleState {
            last_hex: "01".into(),
            last_value: json!(1),
            last_published: now(),
        };
        // Same value as before, no per-register throttle, but global max_interval
        // hasn't elapsed → since no throttling at all, falls through to Stale.
        let out = decide_mqtt_outcome(
            &info,
            None,
            Some(&prev),
            "01",
            &json!(1),
            now(),
            600,
            0.0,
            false,
        );
        // global max=600, age=0 → not elapsed, hex matches, so SkippedHeld.
        assert!(matches!(out, MqttOutcome::SkippedHeld));
    }

    #[test]
    fn outcome_tag_strings_are_stable() {
        // The tag strings appear in user-visible logs and dump files.
        // Lock them down so refactors don't silently change the activity log format.
        assert_eq!(
            MqttOutcome::Published(PublishReason::New).tag(),
            "[published: new]"
        );
        assert_eq!(
            MqttOutcome::Published(PublishReason::Changed).tag(),
            "[published: changed]"
        );
        assert_eq!(
            MqttOutcome::Published(PublishReason::Stale).tag(),
            "[published: stale]"
        );
        assert_eq!(MqttOutcome::SkippedDelta.tag(), "[skipped: delta]");
        assert_eq!(MqttOutcome::SkippedHeld.tag(), "[skipped: held]");
        assert_eq!(MqttOutcome::SkippedRate.tag(), "[skipped: rate]");
        assert_eq!(MqttOutcome::Omitted.tag(), "[omitted]");
    }
}
