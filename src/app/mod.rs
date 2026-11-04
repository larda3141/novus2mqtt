use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use rumqttc::QoS;
use serde_json::json;
use tracing::{error, info, warn};

use crate::config::DeviceConfig;
use crate::ha_discovery;
use crate::health::HealthMonitor;
use crate::metrics::PerformanceMonitor;
use crate::mqtt::{IncomingMessage, MqttClient};
use crate::payload;
use crate::protocol::{self, BUS_WRITE_AFTER_COMMANDS, MSG_HEADER_SIZE};
use crate::serial::SerialHandler;
use crate::signal::ShutdownFlag;
use tokio::sync::mpsc;

/// Application configuration from CLI args.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub app_version: String,
    pub log_level: String,
    pub mqtt_broker: String,
    pub mqtt_port: u16,
    pub mqtt_username: Option<String>,
    pub mqtt_password: Option<String>,
    pub mqtt_client_id: String,
    pub mqtt_base_prefix: String,
    pub mqtt_tls: bool,
    pub mqtt_tls_insecure: bool,
    pub serial_port: String,
    pub serial_baudrate: u32,
    pub novus_config: String,
    pub hass_discovery: bool,
    pub hass_discovery_prefix: String,
    pub hass_device_discovery: bool,
    pub mqtt_clear_on_start: bool,
    pub hass_discovery_mode: String,
    pub auto_sync_time: bool,
    pub req_mem_on_startup: bool,
    pub highlight_unknown_regs: bool,
    pub activity_log: bool,
    pub throttle_max_interval: u64,
    pub throttle_min_change_pct: f64,
    pub disable_throttle: bool,
    pub summary_interval: u64,
    pub bus_dump_file: Option<String>,
    pub max_memory_mb: f64,
}

/// Main application orchestrator.
pub struct NovusConnector {
    // Visibility: fields are `pub(super)` so sibling app/* submodules can
    // implement methods on this struct in their own impl blocks.
    pub(super) config: AppConfig,
    pub(super) device_config: Option<DeviceConfig>,
    pub(super) mqtt_client: Option<MqttClient>,
    pub(super) serial_handler: Option<SerialHandler>,
    pub(super) metrics: PerformanceMonitor,
    pub(super) shutdown: ShutdownFlag,

    // Runtime state
    pub(super) topic_prefix: String,
    pub(super) device_id: String,
    pub(super) novus_mar: HashMap<String, serde_json::Value>,
    pub(super) address_metadata: HashMap<String, AddressMeta>,
    pub(super) command_topic_to_address: HashMap<String, String>,
    pub(super) bus_write_stack: std::collections::VecDeque<WriteItem>,
    pub(super) write_queue_max_size: usize,
    pub(super) state_cache: HashMap<String, String>,
    pub(super) recent_message_counts: HashMap<String, u32>,
    pub(super) last_summary_ts: Instant,
    pub(super) last_memory_check_ts: Instant,
    pub(super) last_diagnostics_ts: Instant,
    pub(super) last_time_sync_ts: Instant,
    /// Ping interval for the systemd watchdog (None when not run with WatchdogSec=).
    watchdog_interval: Option<Duration>,
    last_watchdog_ts: Instant,
    pub(super) last_bus_message_ts: Instant,
    pub(super) boot_time: String,
    pub(super) registers_seen: std::collections::HashSet<String>,
    pub(super) tbc_addresses: std::collections::HashSet<String>,
    pub(super) reg_highlighter_seen: std::collections::HashSet<String>,
    pub(super) throttle_state: HashMap<String, ThrottleState>,
    pub(super) key_rx: Option<mpsc::Receiver<KeyCommand>>,
    pub(super) bus_dump: Option<std::io::BufWriter<std::fs::File>>,
    pub(super) marker_counter: u32,
    pub(super) bookmark_counter: u32,
    pub(super) display_mode: DisplayMode,
    pub(super) mode_entered_at: Option<Instant>,
    pub(super) pause_backlog: std::collections::VecDeque<String>,
    pub(super) pause_backlog_overflow: u32,
    pub(super) blackout_dropped: u32,
    pub(super) highlight_addresses: std::collections::HashSet<String>,
    /// True while the key listener is displaying an interactive prompt on the
    /// alternate screen. Activity lines are silently dropped during this time
    /// so they don't bleed onto the prompt view. Shared with the listener thread.
    pub(super) activity_muted: Arc<AtomicBool>,
    pub(super) command_rx: Option<mpsc::Receiver<IncomingMessage>>,
    pub exit_code: i32,
}

#[derive(Debug)]
pub(crate) enum KeyCommand {
    Dump,
    TimeSync,
    Stats,
    Help,
    Marker,
    Bookmark,
    Clear,
    EnterPause,
    EnterBlackout,
    Resume,
    /// User-typed register address to highlight in the activity log.
    HighlightInput(String),
    HighlightClear,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DisplayMode {
    /// Activity lines print live.
    Live,
    /// Activity lines are buffered; will be replayed on resume.
    Paused,
    /// Activity lines are dropped; only a count is kept.
    Blackout,
}

/// Upper bound on buffered lines during pause, to avoid runaway memory.
const PAUSE_BACKLOG_CAP: usize = 10_000;

mod diagnostics;
mod outcome;
mod registers;
mod tui;
mod write_queue;

use diagnostics::compute_device_id;
use outcome::{decide_mqtt_outcome, MqttOutcome, ThrottleState};
use registers::AddressMeta;
use write_queue::WriteItem;

impl NovusConnector {
    pub fn new(config: AppConfig, shutdown: ShutdownFlag) -> Self {
        let topic_prefix = config.mqtt_base_prefix.trim_matches('/').to_string();
        let boot_time = chrono_like_now();

        Self {
            config,
            device_config: None,
            mqtt_client: None,
            serial_handler: None,
            metrics: PerformanceMonitor::new(),
            shutdown,
            topic_prefix,
            device_id: "unknown".into(),
            novus_mar: HashMap::new(),
            address_metadata: HashMap::new(),
            command_topic_to_address: HashMap::new(),
            bus_write_stack: std::collections::VecDeque::new(),
            write_queue_max_size: 100,
            state_cache: HashMap::new(),
            recent_message_counts: HashMap::new(),
            last_summary_ts: Instant::now(),
            last_memory_check_ts: Instant::now(),
            last_diagnostics_ts: Instant::now(),
            last_time_sync_ts: Instant::now(),
            watchdog_interval: None,
            last_watchdog_ts: Instant::now(),
            last_bus_message_ts: Instant::now(),
            boot_time,
            registers_seen: std::collections::HashSet::new(),
            tbc_addresses: std::collections::HashSet::new(),
            reg_highlighter_seen: std::collections::HashSet::new(),
            throttle_state: HashMap::new(),
            key_rx: None,
            bus_dump: None,
            marker_counter: 0,
            bookmark_counter: 0,
            display_mode: DisplayMode::Live,
            mode_entered_at: None,
            pause_backlog: std::collections::VecDeque::new(),
            pause_backlog_overflow: 0,
            blackout_dropped: 0,
            highlight_addresses: std::collections::HashSet::new(),
            activity_muted: Arc::new(AtomicBool::new(false)),
            command_rx: None,
            exit_code: 0,
        }
    }

    /// Initialize all components. Returns true on success.
    pub async fn initialize(&mut self) -> Result<bool> {
        info!("Application: starting initialization");

        // Load device configuration
        let config_path = Path::new(&self.config.novus_config);
        let config_path = if config_path.is_absolute() {
            config_path.to_path_buf()
        } else {
            std::env::current_dir()?.join(config_path)
        };

        if !config_path.is_file() {
            error!(path = %config_path.display(), "Configuration file not found");
            return Ok(false);
        }

        let device_config = DeviceConfig::load(&config_path)?;
        self.device_id = compute_device_id(&device_config, &self.config.mqtt_client_id);
        self.topic_prefix = format!(
            "{}/{}",
            self.config.mqtt_base_prefix.trim_matches('/'),
            self.device_id
        );

        // Initialize serial handler
        let mut serial =
            SerialHandler::new(self.config.serial_port.clone(), self.config.serial_baudrate);
        serial.connect()?;

        // Initialize MQTT client
        let (mut mqtt, mqtt_rx) = MqttClient::new(
            &self.config.mqtt_broker,
            self.config.mqtt_port,
            &self.config.mqtt_client_id,
            self.config.mqtt_username.as_deref(),
            self.config.mqtt_password.as_deref(),
            &self.topic_prefix,
            self.config.mqtt_tls,
            self.config.mqtt_tls_insecure,
        )?;

        // Wait for MQTT connection
        let timeout = Duration::from_secs(30);
        let start = Instant::now();
        while !mqtt.is_connected() && start.elapsed() < timeout {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if !mqtt.is_connected() {
            error!("Application: MQTT connection timeout");
            return Ok(false);
        }

        // Build memory address register map
        self.build_mar_from_config(&device_config);
        self.build_command_topic_map();
        self.load_address_metadata(&device_config);

        // Subscribe to command topics
        self.configure_mqtt_subscriptions(&mut mqtt).await;

        // Clear old retained topics if requested
        if self.config.mqtt_clear_on_start {
            self.clear_retained_topics(&mqtt, &device_config).await;
        }

        // Publish HA discovery
        if self.config.hass_discovery {
            self.publish_ha_discovery(&mqtt, &device_config).await;
            self.publish_bridge_diagnostics_discovery(&mqtt).await;
        }

        // Publish status, info, config
        self.publish_initial_topics(&mqtt, &device_config).await;

        // Open bus dump file if configured (append number if file exists).
        // Mode 0600 — bus dumps can contain full device traffic; not world-readable.
        if let Some(ref path) = self.config.bus_dump_file {
            use std::os::unix::fs::OpenOptionsExt;
            let dump_path = find_available_dump_path(path);
            let result = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&dump_path);
            match result {
                Ok(f) => {
                    self.bus_dump = Some(std::io::BufWriter::new(f));
                    info!(path = %dump_path, "Bus dump: recording to file (mode 0600)");
                }
                Err(e) => {
                    error!(path = %dump_path, error = %e, "Bus dump: failed to open file");
                }
            }
        }

        // Start health monitor
        let health = HealthMonitor::new(None);
        health.start(self.shutdown.clone());

        self.device_config = Some(device_config);
        self.mqtt_client = Some(mqtt);
        self.serial_handler = Some(serial);

        // Store MQTT receiver for processing commands in main loop
        self.command_rx = Some(mqtt_rx);

        // Spawn interactive keyboard listener
        self.spawn_key_listener();

        // Send initial discovery command (opt-in via REQ_MEM_ON_STARTUP)
        if self.config.req_mem_on_startup {
            self.send_discovery_command().await;
        } else {
            info!("Application: skipping startup 0x83 memory-dump (REQ_MEM_ON_STARTUP=false)");
        }

        info!("Application: initialization complete");

        // systemd (Type=notify): signal readiness and arm the watchdog. Both
        // are no-ops when not started by systemd.
        let _ = sd_notify::notify(&[sd_notify::NotifyState::Ready]);
        self.watchdog_interval = sd_notify::watchdog_enabled().map(|timeout| timeout / 3);
        if let Some(interval) = self.watchdog_interval {
            info!(
                interval_secs = interval.as_secs(),
                "Application: systemd watchdog enabled"
            );
        }
        Ok(true)
    }

    /// Main application loop.
    pub async fn run(&mut self) {
        info!("Application: starting main loop");

        loop {
            if self.shutdown.load(Ordering::SeqCst) {
                break;
            }

            // Check fatal errors and recovery
            if let Some(ref serial) = self.serial_handler {
                if let Some(ref err) = serial.fatal_error {
                    error!(error = %err, "Application: fatal serial error");
                    self.exit_code = 5;
                    break;
                }
            }
            if let Some(ref mut serial) = self.serial_handler {
                if serial.needs_recovery {
                    serial.recover().await;
                }
            }

            // Check MQTT disconnect duration — escalate and eventually shut down
            if let Some(ref mqtt) = self.mqtt_client {
                let disc_secs = mqtt.disconnected_secs();
                if disc_secs > 0 {
                    // Escalating log levels: 30s warn, 90s error, 390s (6.5min) fatal
                    if disc_secs >= 390 {
                        error!(disconnected_secs = disc_secs,
                            "Application: MQTT broker unreachable for over 6 minutes, shutting down");
                        self.exit_code = 6;
                        break;
                    } else if disc_secs >= 90 && disc_secs % 30 < 1 {
                        error!(
                            disconnected_secs = disc_secs,
                            "Application: MQTT broker still unreachable, will shut down at 6m30s"
                        );
                    } else if disc_secs >= 30 && disc_secs % 30 < 1 {
                        warn!(
                            disconnected_secs = disc_secs,
                            "Application: MQTT broker unreachable, retrying"
                        );
                    }
                }
            }

            // Read messages from serial bus
            let messages = if let Some(ref mut serial) = self.serial_handler {
                serial.read_messages().await
            } else {
                Vec::new()
            };

            // Process each message
            for message in &messages {
                self.process_bus_message(message).await;
            }

            // Process incoming MQTT commands (non-blocking drain)
            if let Some(mut rx) = self.command_rx.take() {
                let mut pending = Vec::new();
                while let Ok(msg) = rx.try_recv() {
                    pending.push(msg);
                }
                self.command_rx = Some(rx);
                for msg in pending {
                    self.metrics.increment(&self.metrics.mqtt_messages_received);
                    let payload = String::from_utf8_lossy(&msg.payload).to_string();
                    self.handle_set_command(&msg.topic, &payload);
                }
            }

            // Process keyboard commands
            self.process_key_commands().await;

            self.log_message_summary_if_due();
            self.publish_diagnostics_if_due().await;
            self.sync_time_if_due().await;
            self.check_memory_guard();
            self.watchdog_ping_if_due();

            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        info!("Application: main loop ended");

        // Graceful shutdown
        if let Some(ref mut serial) = self.serial_handler {
            serial.disconnect();
        }
        if let Some(ref mqtt) = self.mqtt_client {
            mqtt.disconnect().await;
        }
    }

    async fn process_bus_message(&mut self, message: &[u8]) {
        if message.len() < MSG_HEADER_SIZE {
            return;
        }

        self.metrics.increment(&self.metrics.bus_messages_received);
        self.last_bus_message_ts = Instant::now();

        // Write raw message to dump file
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
            let _ = writeln!(dump, "{ts:.3} < {hex}");
            let _ = dump.flush();
        }

        let cmd = message[2];

        // Handle write-after-read operations
        if !self.bus_write_stack.is_empty() && BUS_WRITE_AFTER_COMMANDS.contains(&cmd) {
            self.execute_pending_write().await;
        }

        // Handle special commands (skip payload consumption for protocol messages)
        let skip_consume = matches!(cmd, 0x80 | 0x81 | 0x84 | 0x86 | 0x87);

        // Process data messages
        if !skip_consume && message.len() > MSG_HEADER_SIZE {
            self.consume_bus_data(message).await;
        }

        self.metrics.increment(&self.metrics.messages_processed);
    }

    async fn consume_bus_data(&mut self, message: &[u8]) {
        let cmd = message[2];
        let data = &message[MSG_HEADER_SIZE..];

        // Only consume data-carrying commands:
        // 0x85 = memory write, 0x00 = clock (carries date/time), 0x83 = dump response
        if !matches!(cmd, 0x85 | 0x00 | 0x83) || data.is_empty() {
            return;
        }

        // Skip write pings (payload length <= 3)
        if data.len() <= 3 {
            return;
        }

        // Split payload into individual register blocks.
        // Format per register: [address, 0x00, value_byte_0, value_byte_1, ...]
        // The Python heuristic splits at boundaries where the next register starts.
        let registers = self.split_register_blocks(data);

        for reg_data in &registers {
            if reg_data.len() < 2 {
                continue;
            }

            let register_address = format!("0x{:02x}", reg_data[0]);

            // Rev-eng aid: flag unknown / TBC registers in the console on first sight.
            self.highlight_reg_if_new(&register_address);

            // Note: mqtt_publish=false is handled inline below by emitting
            // an `[omitted]` tag on the activity line, so we don't early-return.

            // Look up register info
            let register_info = match self.novus_mar.get(&register_address) {
                Some(info) => info.clone(),
                None => continue,
            };

            let payload_length = register_info
                .pointer("/payload/length")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            if payload_length == 0 {
                continue;
            }

            // Value bytes start after address byte + 0x00 separator (offset 2)
            if reg_data.len() < 2 + payload_length {
                continue;
            }
            let value_bytes = &reg_data[2..2 + payload_length];
            if value_bytes.is_empty() {
                continue;
            }

            // Build payload config from register info
            let payload_cfg = extract_payload_config(&register_info);
            let mqtt_payload = payload::build_mqtt_payload(&payload_cfg, value_bytes);
            let payload_json = serde_json::to_string(&mqtt_payload).unwrap_or_default();

            // Decide MQTT outcome BEFORE emitting activity, so the tag is accurate.
            let now = Instant::now();
            let new_val = mqtt_payload
                .get("converted")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let new_hex = mqtt_payload
                .get("hex")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let outcome = decide_mqtt_outcome(
                &register_info,
                self.address_metadata.get(&register_address),
                self.throttle_state.get(&register_address),
                &new_hex,
                &new_val,
                now,
                self.config.throttle_max_interval,
                self.config.throttle_min_change_pct,
                self.config.disable_throttle,
            );

            // Per-register activity log (gated by global ACTIVITY_LOG + yaml suppress_activity).
            // Display mode decides where the line goes: live stream, pause buffer, or drop.
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
                let formatted = mqtt_payload
                    .get("formatted")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        mqtt_payload.get("converted").map(|v| match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                    })
                    .unwrap_or_default();
                let body = format!("{register_address} {name} <- {formatted} {}", outcome.tag());
                self.emit_activity_line(&register_address, &body);
            }

            // Only publish to MQTT if the outcome says so.
            if !matches!(outcome, MqttOutcome::Published(_)) {
                continue;
            }

            let state_topic = register_info
                .pointer("/topic")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if state_topic.is_empty() {
                continue;
            }

            let semantic_topic = format!("{state_topic}/state");
            if let Some(ref mqtt) = self.mqtt_client {
                mqtt.publish_retain(&semantic_topic, &payload_json).await;
                self.state_cache
                    .insert(semantic_topic.clone(), payload_json);
                *self
                    .recent_message_counts
                    .entry(state_topic.to_string())
                    .or_insert(0) += 1;
                self.metrics.increment(&self.metrics.mqtt_messages_sent);
                self.registers_seen.insert(register_address.clone());
                self.throttle_state.insert(
                    register_address.clone(),
                    ThrottleState {
                        last_hex: new_hex,
                        last_value: new_val,
                        last_published: now,
                    },
                );
            }
        }
    }

    /// Split a memory write payload into individual register blocks.
    ///
    /// Each register block is [address, 0x00, value_bytes...].
    /// Uses config-defined payload lengths to determine block boundaries.
    fn split_register_blocks(&self, data: &[u8]) -> Vec<Vec<u8>> {
        let mut registers: Vec<Vec<u8>> = Vec::new();
        let mut i = 0;

        while i < data.len() {
            let addr = data[i];
            let addr_key = format!("0x{:02x}", addr);

            // Look up expected payload length from config
            let expected_len = self
                .novus_mar
                .get(&addr_key)
                .and_then(|info| info.pointer("/payload/length"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            if expected_len == 0 {
                // Unknown register — skip this byte and try next
                i += 1;
                continue;
            }

            // Total block size: address(1) + separator(1) + value(expected_len)
            let block_size = 2 + expected_len;
            if i + block_size > data.len() {
                // Not enough data remaining
                break;
            }

            registers.push(data[i..i + block_size].to_vec());
            i += block_size;
        }

        registers
    }

    fn build_mar_from_config(&mut self, device_config: &DeviceConfig) {
        for entity in device_config.all_entities() {
            let address = entity.address.to_lowercase();
            // Store the entity as a JSON value for flexible access
            let json_val = serde_json::to_value(entity).unwrap_or_default();
            self.novus_mar.insert(address, json_val);
        }
    }

    fn build_command_topic_map(&mut self) {
        for entity in self.novus_mar.values() {
            let address = entity
                .get("address")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();

            if let Some(cmd_topic) = entity
                .pointer("/command/topic")
                .or_else(|| entity.get("command_topic"))
                .and_then(|v| v.as_str())
            {
                let topic = cmd_topic.trim_matches('/').to_string();
                if !topic.is_empty() {
                    self.command_topic_to_address.insert(topic, address.clone());
                }
            }
        }

        // Also from address metadata
        for (address, meta) in &self.address_metadata {
            if let Some(ref cmd_topic) = meta.command_topic {
                let topic = cmd_topic.trim_matches('/').to_string();
                if !topic.is_empty() {
                    self.command_topic_to_address.insert(topic, address.clone());
                }
            }
        }

        // Auto-derive `{topic}/set` for RW entities that didn't specify a
        // command_topic explicitly. Keeps registers.yaml concise.
        for entity in self.novus_mar.values() {
            let address = entity
                .get("address")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let is_rw = entity
                .get("operations")
                .and_then(|v| v.as_str())
                .map(|o| o.eq_ignore_ascii_case("RW"))
                .unwrap_or(false);
            if !is_rw {
                continue;
            }
            let state_topic = entity
                .get("topic")
                .and_then(|v| v.as_str())
                .map(|t| t.trim_matches('/'))
                .unwrap_or("");
            if state_topic.is_empty() {
                continue;
            }
            let derived = format!("{state_topic}/set");
            // Don't overwrite an explicit mapping
            self.command_topic_to_address
                .entry(derived)
                .or_insert(address);
        }

        info!(
            count = self.command_topic_to_address.len(),
            "Application: loaded command topic mappings"
        );
    }

    fn load_address_metadata(&mut self, device_config: &DeviceConfig) {
        for entity in device_config.all_entities() {
            let address = entity.address.to_lowercase();
            self.address_metadata.insert(
                address.clone(),
                AddressMeta {
                    mqtt_publish: entity.mqtt_publish.unwrap_or(true),
                    command_topic: entity.command_topic.clone(),
                },
            );
            // Flag TBC (unconfirmed) registers for the console highlighter.
            // Prefer the explicit `tbc: true` yaml flag; fall back to a
            // case-insensitive substring match on name/description for entries
            // that haven't been migrated yet.
            let is_tbc = entity.tbc.unwrap_or(false) || {
                let name = entity.name.as_deref().unwrap_or("").to_lowercase();
                let desc = entity.description.as_deref().unwrap_or("").to_lowercase();
                name.contains("unconfirm") || desc.contains("unconfirm")
            };
            if is_tbc {
                self.tbc_addresses.insert(address);
            }
        }
        info!(
            count = self.address_metadata.len(),
            tbc = self.tbc_addresses.len(),
            "Application: loaded address metadata"
        );
    }

    /// Emit a colored console alert the first time we see an unknown or TBC
    /// register on the bus. Dedupes per session. Gated by `HIGHLIGHT_UNKNOWN_REGS`.
    fn highlight_reg_if_new(&mut self, reg_addr: &str) {
        if !self.config.highlight_unknown_regs {
            return;
        }
        if self.reg_highlighter_seen.contains(reg_addr) {
            return;
        }

        use std::io::IsTerminal;
        let tty = std::io::stderr().is_terminal();
        // Different colours from markers (which use red/yellow).
        // UNKNOWN → bold magenta; TBC → bold cyan.
        let (mag_on, cyan_on, off) = if tty {
            ("\x1b[1;35m", "\x1b[1;36m", "\x1b[0m")
        } else {
            ("", "", "")
        };

        if !self.novus_mar.contains_key(reg_addr) {
            eprintln!(
                "{mag_on}⚠ UNKNOWN register {reg_addr}  — not in registers.yaml. Press 'm' + note what you just did!{off}"
            );
            tracing::warn!(addr = %reg_addr, status = "unknown", "unmapped register on bus");
            self.reg_highlighter_seen.insert(reg_addr.to_string());
        } else if self.tbc_addresses.contains(reg_addr) {
            let name = self
                .novus_mar
                .get(reg_addr)
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            eprintln!(
                "{cyan_on}⚠ TBC register {reg_addr} ({name})  — marked UNCONFIRMED. Press 'm' + note what triggered it!{off}"
            );
            tracing::warn!(addr = %reg_addr, status = "tbc", name = %name, "unconfirmed register on bus");
            self.reg_highlighter_seen.insert(reg_addr.to_string());
        }
    }

    async fn configure_mqtt_subscriptions(&self, mqtt: &mut MqttClient) {
        let topics: Vec<String> = self.command_topic_to_address.keys().cloned().collect();
        for topic in &topics {
            mqtt.subscribe(topic, true, QoS::AtLeastOnce).await;
        }

        // Subscribe to special command topics
        mqtt.subscribe("bus/system/dump/set", true, QoS::AtLeastOnce)
            .await;

        if !topics.is_empty() {
            info!(
                count = topics.len() + 1,
                "Application: subscribed to command topics"
            );
        }
    }

    async fn publish_ha_discovery(&self, mqtt: &MqttClient, device_config: &DeviceConfig) {
        // `lazy` mode (publish on first data) is not yet implemented — fall back to startup
        // and warn so the user knows the flag is currently a no-op.
        if self.config.hass_discovery_mode != "startup" {
            warn!(
                mode = %self.config.hass_discovery_mode,
                "HA discovery: only 'startup' mode is implemented; proceeding with startup behavior"
            );
        }
        let hass_prefix = &self.config.hass_discovery_prefix;
        const MAX_MQTT_PAYLOAD: usize = 10240;

        let messages = if self.config.hass_device_discovery {
            let device_msgs = ha_discovery::generate_device_discovery(
                device_config,
                hass_prefix,
                &self.topic_prefix,
            );
            // Check if the single device discovery payload fits within broker limits
            let too_large = device_msgs.iter().any(|msg| {
                serde_json::to_string(&msg.payload)
                    .unwrap_or_default()
                    .len()
                    > MAX_MQTT_PAYLOAD
            });
            if device_msgs.is_empty() || too_large {
                if too_large {
                    warn!("Application: device discovery payload exceeds broker limit, using per-component discovery");
                }
                ha_discovery::generate_discovery(device_config, hass_prefix, &self.topic_prefix)
            } else {
                device_msgs
            }
        } else {
            ha_discovery::generate_discovery(device_config, hass_prefix, &self.topic_prefix)
        };

        for msg in &messages {
            let payload = serde_json::to_string(&msg.payload).unwrap_or_default();
            mqtt.publish(&msg.topic, &payload, QoS::AtMostOnce, true, false)
                .await;
        }

        info!(
            count = messages.len(),
            "Application: published HA discovery messages"
        );
    }

    async fn clear_retained_topics(&self, mqtt: &MqttClient, device_config: &DeviceConfig) {
        info!("Application: clearing old retained MQTT topics");
        let mut cleared = 0u32;

        // Clear HA discovery topics (both per-component and device format)
        let hass_prefix = &self.config.hass_discovery_prefix;
        let discovery_node = self.topic_prefix.replace('/', "_");

        // Per-component discovery topics
        let component_messages =
            ha_discovery::generate_discovery(device_config, hass_prefix, &self.topic_prefix);
        for msg in &component_messages {
            mqtt.publish(&msg.topic, "", QoS::AtMostOnce, true, false)
                .await;
            cleared += 1;
        }

        // Device discovery topic
        let device_topic = format!("{hass_prefix}/device/{discovery_node}/config");
        mqtt.publish(&device_topic, "", QoS::AtMostOnce, true, false)
            .await;
        cleared += 1;

        // Clear state topics for all entities
        for entity in device_config.all_entities() {
            if let Some(ref topic) = entity.topic {
                let state_topic = format!("{}/state", topic);
                mqtt.publish_retain(&state_topic, "").await;
                cleared += 1;
            }
        }

        // Clear well-known app topics
        for topic in &["status", "info", "config", "diagnostics"] {
            mqtt.publish_retain(topic, "").await;
            cleared += 1;
        }

        info!(count = cleared, "Application: cleared retained topics");

        // Brief pause to let broker process the clears before publishing new data
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    async fn publish_initial_topics(&self, mqtt: &MqttClient, device_config: &DeviceConfig) {
        // Status: online/offline for availability tracking
        let status = json!({"connected": true});
        mqtt.publish("status", &status.to_string(), QoS::AtLeastOnce, true, true)
            .await;

        // Device info: static metadata about the hardware
        let info = json!({
            "device": {
                "name": device_config.yaml.device.name,
                "manufacturer": device_config.yaml.device.manufacturer,
                "model": device_config.yaml.device.model,
                "hw_version": device_config.yaml.device.hw_version,
                "sw_version": device_config.yaml.device.sw_version,
                "serial_number": device_config.yaml.device.serial_number,
                "device_id": self.device_id,
            },
            "bridge": {
                "name": "novus2mqtt",
                "version": self.config.app_version,
                "platform": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
            },
            "connection": {
                "mqtt_broker": self.config.mqtt_broker,
                "mqtt_port": self.config.mqtt_port,
                "serial_port": self.config.serial_port,
                "serial_baudrate": self.config.serial_baudrate,
                "topic_prefix": self.topic_prefix,
            },
            "config": {
                "log_level": self.config.log_level.to_uppercase(),
                "hass_discovery": self.config.hass_discovery,
                "entities_total": self.address_metadata.len(),
            },
            "boot_time": self.boot_time,
        });
        mqtt.publish_retain("info", &info.to_string()).await;
    }

    async fn send_discovery_command(&mut self) {
        let message = protocol::build_bus_message(&[0x01, 0x01], 0x83, &[]);
        self.bus_write_stack.push_back(WriteItem::Raw(message));
        info!("Application: queued discovery dump for next bus gap");
    }

    fn log_message_summary_if_due(&mut self) {
        let now = Instant::now();
        if self.config.summary_interval == 0
            || now.duration_since(self.last_summary_ts)
                < Duration::from_secs(self.config.summary_interval)
        {
            return;
        }

        if !self.recent_message_counts.is_empty() {
            let mut parts: Vec<String> = self
                .recent_message_counts
                .iter()
                .map(|(topic, count)| {
                    let short = topic.rsplit('/').next().unwrap_or(topic);
                    format!("{short} ({count})")
                })
                .collect();
            parts.sort();
            info!(summary = %parts.join(", "), "Application: message summary (last 10s)");
            self.recent_message_counts.clear();
        }

        self.last_summary_ts = now;
    }

    fn check_memory_guard(&mut self) {
        if self.config.max_memory_mb <= 0.0 {
            return;
        }

        let now = Instant::now();
        if now.duration_since(self.last_memory_check_ts) < Duration::from_secs(5) {
            return;
        }
        self.last_memory_check_ts = now;

        let current_mb = self.metrics.memory_usage_mb();
        if current_mb > self.config.max_memory_mb {
            self.metrics.increment(&self.metrics.memory_guard_trips);
            error!(
                current_mb,
                limit_mb = self.config.max_memory_mb,
                "Application: memory guard tripped, requesting shutdown"
            );
            self.exit_code = 7;
            self.shutdown.store(true, Ordering::SeqCst);
        }
    }

    /// Sync the Novus clock from system local time every hour.
    /// Publishes to MQTT command topics, which flow back through the normal
    /// command handler → encode → bus write path.
    /// Feed the systemd watchdog, but only while the bridge is healthy.
    ///
    /// Pinging from the main loop means a stalled main loop starves the
    /// watchdog. MQTT counts as healthy when its event loop showed activity in
    /// the last 2 minutes (keep-alive is 30 s), or during a disconnect that is
    /// still within the 6m30s grace period the main loop enforces itself.
    /// Anything else (a frozen event loop, or never connecting at all) stops
    /// the pings and systemd restarts the service.
    fn watchdog_ping_if_due(&mut self) {
        let Some(interval) = self.watchdog_interval else {
            return;
        };
        let now = Instant::now();
        if now.duration_since(self.last_watchdog_ts) < interval {
            return;
        }
        self.last_watchdog_ts = now;

        let Some(ref mqtt) = self.mqtt_client else {
            return;
        };
        let idle = mqtt.secs_since_activity();
        let disconnected = mqtt.disconnected_secs();
        let healthy = idle < 120 || (disconnected > 0 && disconnected < 420);
        if healthy {
            let _ = sd_notify::notify(&[sd_notify::NotifyState::Watchdog]);
        } else {
            error!(
                mqtt_idle_secs = idle,
                mqtt_connected = mqtt.is_connected(),
                "Application: MQTT event loop unresponsive, withholding watchdog ping"
            );
        }
    }

    async fn sync_time_if_due(&mut self) {
        if !self.config.auto_sync_time {
            return;
        }

        let now = Instant::now();
        if now.duration_since(self.last_time_sync_ts) < Duration::from_secs(3600) {
            return;
        }
        self.last_time_sync_ts = now;

        self.publish_time_sync().await;
    }

    /// Publish current system time and date to MQTT command topics.
    /// The MQTT messages loop back through handle_set_command → encode → bus write.
    async fn publish_time_sync(&mut self) {
        // Get local time via the `date` command (respects system timezone/DST).
        // Wrapped in spawn_blocking so it doesn't stall the tokio worker.
        let output = tokio::task::spawn_blocking(|| {
            std::process::Command::new("date")
                .args(["+%H:%M:%S %Y-%m-%d"])
                .output()
        })
        .await;

        let datetime_str = match output {
            Ok(Ok(ref o)) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            }
            _ => {
                warn!("Time sync: failed to get system time");
                return;
            }
        };

        let parts: Vec<&str> = datetime_str.split(' ').collect();
        if parts.len() != 2 {
            warn!("Time sync: unexpected date output: {datetime_str}");
            return;
        }

        let time_str = parts[0]; // HH:MM:SS
        let date_str = parts[1]; // YYYY-MM-DD

        // Publish to MQTT command topics — they loop back through handle_set_command
        if let Some(ref mqtt) = self.mqtt_client {
            mqtt.publish(
                "bus/system/time/set",
                time_str,
                QoS::AtLeastOnce,
                false,
                true,
            )
            .await;
            mqtt.publish(
                "bus/system/date/set",
                date_str,
                QoS::AtLeastOnce,
                false,
                true,
            )
            .await;
        }

        info!(time = %time_str, date = %date_str, "Time sync: published to MQTT command topics");
    }
}

/// Extract a PayloadConfig from a JSON register info value.
fn extract_payload_config(
    register_info: &serde_json::Value,
) -> Option<crate::config::PayloadConfig> {
    register_info
        .get("payload")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// Simple UTC timestamp string (avoids chrono dependency).
fn chrono_like_now() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    format!("{secs}")
}

/// Find an available dump file path, appending a number if the file already exists.
/// e.g., bus_dump.log -> bus_dump.1.log -> bus_dump.2.log -> ...
fn find_available_dump_path(base: &str) -> String {
    if !std::path::Path::new(base).exists() {
        return base.to_string();
    }

    let path = std::path::Path::new(base);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("bus_dump");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("log");
    let parent = path.parent().unwrap_or(std::path::Path::new("."));

    for i in 1..1000 {
        let candidate = parent.join(format!("{stem}.{i}.{ext}"));
        if !candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }

    // Fallback: overwrite the original
    base.to_string()
}
