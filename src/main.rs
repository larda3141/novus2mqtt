mod app;
mod config;
mod ha_discovery;
mod health;
mod logging;
mod metrics;
mod mqtt;
mod payload;
mod protocol;
mod serial;
mod signal;

use std::process;

use clap::{Parser, Subcommand};
use tracing::{error, info};

const APP_NAME: &str = "novus2mqtt";
const APP_VERSION: &str = env!("BUILD_VERSION");
const APP_GIT_HASH: &str = env!("BUILD_GIT_HASH");

#[derive(Parser, Debug)]
#[command(name = APP_NAME, version = APP_VERSION)]
#[command(about = "MQTT bridge for Paul Novus HVAC systems")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    // --- Logging ---
    /// Logging level
    #[arg(short = 'l', long, default_value = "info", env = "LOG_LEVEL")]
    log_level: String,

    /// Log output format: 'human' or 'json'
    #[arg(long, default_value = "human", env = "LOG_FORMAT")]
    log_format: String,

    /// Path to log file
    #[arg(long, env = "LOG_FILE")]
    log_file: Option<String>,

    /// Enable logging to file with rotation
    #[arg(long, env = "ENABLE_FILE_LOGGING")]
    enable_file_logging: bool,

    // --- MQTT ---
    /// MQTT broker hostname or IP
    #[arg(long, env = "MQTT_BROKER")]
    mqtt_broker: Option<String>,

    /// MQTT broker port
    #[arg(long, default_value = "1883", env = "MQTT_PORT")]
    mqtt_port: u16,

    /// MQTT broker username
    #[arg(long, env = "MQTT_USERNAME")]
    mqtt_username: Option<String>,

    /// MQTT broker password
    #[arg(long, env = "MQTT_PASSWORD")]
    mqtt_password: Option<String>,

    /// Base MQTT topic prefix
    #[arg(long, default_value = "novus2mqtt", env = "MQTT_BASE_PREFIX")]
    mqtt_base_prefix: String,

    /// Enable TLS for MQTT
    #[arg(long, env = "MQTT_TLS")]
    mqtt_tls: bool,

    /// Disable TLS certificate validation (insecure)
    #[arg(long, env = "MQTT_TLS_INSECURE")]
    mqtt_tls_insecure: bool,

    // --- Serial ---
    /// Serial port device
    #[arg(long, default_value = "/dev/ttyUSB0", env = "SERIAL_PORT")]
    serial_port: String,

    /// Serial baud rate
    #[arg(long, default_value = "9600", env = "SERIAL_BAUDRATE")]
    serial_baudrate: u32,

    // --- Device config ---
    /// Path to device configuration file
    #[arg(long, default_value = "./config/device.yaml", env = "NOVUS_CONFIG")]
    novus_config: String,

    // --- Home Assistant ---
    /// Enable Home Assistant MQTT discovery
    #[arg(long, env = "HASS_DISCOVERY")]
    hass_discovery: bool,

    /// Home Assistant discovery prefix
    #[arg(long, default_value = "homeassistant", env = "HASS_DISCOVERY_PREFIX")]
    hass_discovery_prefix: String,

    /// Use HA device discovery payload format (single payload, requires large max_packet_size on broker)
    #[arg(long, default_value = "false", env = "HASS_DEVICE_DISCOVERY")]
    hass_device_discovery: bool,

    /// When to publish HA discovery: 'startup' (all at once) or 'lazy' (on first data)
    #[arg(long, default_value = "startup", env = "HASS_DISCOVERY_MODE")]
    hass_discovery_mode: String,

    /// Clear all retained MQTT topics on startup before publishing
    #[arg(long, env = "MQTT_CLEAR_ON_START")]
    mqtt_clear_on_start: bool,

    /// Auto-sync Novus clock from system time (handles DST changes)
    #[arg(long, env = "AUTO_SYNC_TIME")]
    auto_sync_time: bool,

    /// Request a 0x83 memory-dump from the VMC at startup (populates HA faster).
    /// Causes the VMC to burst-broadcast every register — briefly disrupts the
    /// TFT display. Default off: the app listens passively and picks up values
    /// from the VMC's own periodic broadcasts within ~30 s.
    #[arg(long, env = "REQ_MEM_ON_STARTUP")]
    req_mem_on_startup: bool,

    /// Highlight UNKNOWN registers (not in registers.yaml) in magenta and TBC
    /// registers (marked with `tbc: true`) in cyan on the console, once per
    /// session. Useful during reverse-engineering.
    #[arg(long, default_value = "true", env = "HIGHLIGHT_UNKNOWN_REGS")]
    highlight_unknown_regs: bool,

    /// Global gate for per-register activity logging. When true, every
    /// decoded register value is printed as a single live line, EXCEPT for
    /// registers with `suppress_activity: true` in registers.yaml. When false
    /// (default), nothing is printed regardless of the yaml flag.
    #[arg(long, env = "ACTIVITY_LOG")]
    activity_log: bool,

    /// Global MQTT-publish heartbeat in seconds. Registers with no per-register
    /// `throttle.max_interval` fall back to this. 0 = no heartbeat.
    #[arg(long, default_value = "0", env = "THROTTLE_MAX_INTERVAL")]
    throttle_max_interval: u64,

    /// Global MQTT-publish minimum relative change in percent. Registers with
    /// no per-register `throttle.min_change_pct` fall back to this. 0 = no
    /// percent-threshold.
    #[arg(long, default_value = "0", env = "THROTTLE_MIN_CHANGE_PCT")]
    throttle_min_change_pct: f64,

    /// Bypass MQTT-publish throttling entirely — ignore per-register `throttle:`
    /// blocks and global defaults. Every RECV publishes (tagged new / changed /
    /// stale based on hex comparison). Default false.
    #[arg(long, env = "DISABLE_THROTTLE")]
    disable_throttle: bool,

    /// Interval in seconds for logging message summary (0 to disable)
    #[arg(long, default_value = "10", env = "SUMMARY_INTERVAL")]
    summary_interval: u64,

    /// Path to dump raw bus messages to a file (for protocol analysis)
    #[arg(long, env = "BUS_DUMP_FILE")]
    bus_dump_file: Option<String>,

    /// Memory guard threshold in MB (0 disables)
    #[arg(long, default_value = "0", env = "MAX_MEMORY_MB")]
    max_memory_mb: f64,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the main MQTT bridge service
    Run,
    /// Check application health
    HealthCheck,
    /// Validate device configuration
    ValidateConfig {
        /// Path to config file
        #[arg(default_value = "./config/device.yaml")]
        config: String,
    },
}

#[tokio::main]
async fn main() {
    // Load .env file before parsing CLI args (so env vars are available to clap)
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    // Initialize logging
    logging::init(&cli.log_level, &cli.log_format, cli.log_file.as_deref());

    // Handle subcommands
    match cli.command {
        Some(Commands::HealthCheck) => {
            let code = health::check_health(&cli.serial_port);
            process::exit(code);
        }
        Some(Commands::ValidateConfig { config }) => {
            match config::DeviceConfig::load(std::path::Path::new(&config)) {
                Ok(cfg) => {
                    let entities = cfg.all_entities().len();
                    println!("Configuration valid: {entities} entities loaded");
                    process::exit(0);
                }
                Err(e) => {
                    eprintln!("Configuration error: {e}");
                    process::exit(1);
                }
            }
        }
        Some(Commands::Run) | None => {
            run_bridge(cli).await;
        }
    }
}

async fn run_bridge(cli: Cli) {
    let mqtt_broker = match cli.mqtt_broker {
        Some(ref b) => b.clone(),
        None => {
            error!("--mqtt-broker is required");
            process::exit(1);
        }
    };

    // Print banner before logging starts
    // SAFETY: isatty is a standard POSIX function; fd 0 (stdin) is always valid in a running process.
    let is_tty = unsafe { libc::isatty(0) != 0 };
    if is_tty {
        eprintln!("novus2mqtt {APP_VERSION} ({APP_GIT_HASH})");
        eprintln!(
            "Keys: [d]ump [t]ime [s]tats [m]arker [f]lag [p]ause [b]lackout [r]esume [h]ighlight [c]lr [?]help [q/x]quit"
        );
        eprintln!(
            "      (uppercase of d/t/s/m/f/p/b/r = clear screen first; H = clear highlights)"
        );
        eprintln!();
    }

    info!(
        version = APP_VERSION,
        git = APP_GIT_HASH,
        "Starting {APP_NAME}"
    );

    // Single-instance lock. Open with O_NOFOLLOW + restrictive mode so a local
    // attacker can't pre-create a symlink at the lock path and trick us into
    // truncating an arbitrary file (e.g. ~/.ssh/authorized_keys).
    let lock_path = std::env::temp_dir().join("novus2mqtt.lock");
    let lock_file = {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW);
        match opts.open(&lock_path) {
            Ok(f) => f,
            Err(e) => {
                error!(error = %e, path = %lock_path.display(), "Failed to open lock file");
                process::exit(6);
            }
        }
    };

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = lock_file.as_raw_fd();
        // SAFETY: fd is valid (obtained from an open File); flock is a standard POSIX advisory lock.
        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            error!("Another novus2mqtt instance is already running");
            process::exit(6);
        }
    }

    let shutdown = signal::new_shutdown_flag();
    signal::spawn_signal_handler(shutdown.clone());

    let app_config = app::AppConfig {
        app_version: APP_VERSION.into(),
        log_level: cli.log_level.clone(),
        mqtt_broker,
        mqtt_port: cli.mqtt_port,
        mqtt_username: cli.mqtt_username.clone(),
        mqtt_password: cli.mqtt_password.clone(),
        mqtt_client_id: "novus2mqtt".into(),
        mqtt_base_prefix: cli.mqtt_base_prefix.clone(),
        mqtt_tls: cli.mqtt_tls,
        mqtt_tls_insecure: cli.mqtt_tls_insecure,
        serial_port: cli.serial_port.clone(),
        serial_baudrate: cli.serial_baudrate,
        novus_config: cli.novus_config.clone(),
        hass_discovery: cli.hass_discovery,
        hass_discovery_prefix: cli.hass_discovery_prefix.clone(),
        hass_device_discovery: cli.hass_device_discovery,
        hass_discovery_mode: cli.hass_discovery_mode.clone(),
        auto_sync_time: cli.auto_sync_time,
        req_mem_on_startup: cli.req_mem_on_startup,
        highlight_unknown_regs: cli.highlight_unknown_regs,
        activity_log: cli.activity_log,
        throttle_max_interval: cli.throttle_max_interval,
        throttle_min_change_pct: cli.throttle_min_change_pct,
        disable_throttle: cli.disable_throttle,
        summary_interval: cli.summary_interval,
        bus_dump_file: cli.bus_dump_file.clone(),
        mqtt_clear_on_start: cli.mqtt_clear_on_start,
        max_memory_mb: cli.max_memory_mb,
    };

    let mut connector = app::NovusConnector::new(app_config, shutdown.clone());

    match connector.initialize().await {
        Ok(true) => {
            connector.run().await;
            if connector.exit_code != 0 {
                error!(code = connector.exit_code, "Application exited with error");
                process::exit(connector.exit_code);
            }
            info!("Application exited cleanly");
        }
        Ok(false) => {
            error!("Application initialization failed");
            process::exit(1);
        }
        Err(e) => {
            error!(error = %e, "Critical application error");
            process::exit(3);
        }
    }

    drop(lock_file);
}
