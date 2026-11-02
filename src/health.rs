use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tracing::{error, info};

use crate::signal::ShutdownFlag;

/// Health monitor that writes a timestamp file for external health checks.
pub struct HealthMonitor {
    health_file: PathBuf,
}

impl HealthMonitor {
    pub fn new(health_file: Option<PathBuf>) -> Self {
        let path = health_file.unwrap_or_else(|| std::env::temp_dir().join("novus2mqtt_health"));
        Self { health_file: path }
    }

    /// Spawn a background task that updates the health file every 60 seconds.
    pub fn start(&self, shutdown: ShutdownFlag) {
        let path = self.health_file.clone();
        tokio::spawn(async move {
            info!("Health monitor: started");
            while !shutdown.load(Ordering::SeqCst) {
                if let Err(e) = write_health_file(&path) {
                    error!(error = %e, "Health monitor: error updating health file");
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            // Clean up on shutdown
            let _ = std::fs::remove_file(&path);
            info!("Health monitor: stopped");
        });
    }
}

fn write_health_file(path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    // O_NOFOLLOW + 0600: symlink-safe and only readable by us.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    writeln!(f, "{timestamp}")?;
    Ok(())
}

/// Perform a health check (for the `health-check` subcommand).
///
/// Returns 0 for healthy, 1 for unhealthy.
pub fn check_health(serial_port: &str) -> i32 {
    // Check 1: Serial device exists
    if !std::path::Path::new(serial_port).exists() {
        println!("Serial device {serial_port} not found");
        return 1;
    }

    // Check 2: Health file is recent (less than 5 minutes old)
    let health_file = std::env::temp_dir().join("novus2mqtt_health");
    if health_file.exists() {
        match std::fs::read_to_string(&health_file) {
            Ok(content) => match content.trim().parse::<f64>() {
                Ok(timestamp) => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64();
                    let age = now - timestamp;
                    if age > 300.0 {
                        println!("Health check file is stale ({age:.0}s old)");
                        return 1;
                    }
                }
                Err(e) => {
                    println!("Error parsing health file: {e}");
                    return 1;
                }
            },
            Err(e) => {
                println!("Error reading health file: {e}");
                return 1;
            }
        }
    } else {
        println!("Health file not found");
        return 1;
    }

    println!("Health check passed");
    0
}
