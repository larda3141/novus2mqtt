use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime as TsSystemTime};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

static APP_START: OnceLock<Instant> = OnceLock::new();
/// Seconds since `APP_START` at the moment of the last marker/bookmark press.
/// Sentinel `u64::MAX` means "no marker yet" — Δ is rendered as `--:--:--`.
static LAST_MARKER_SECS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Returns (seconds_since_start, Option<seconds_since_last_marker>) AND updates
/// the last-marker timestamp to now. `None` means this is the first marker
/// since startup, so there is no prior reference point.
pub fn stamp_marker() -> (u64, Option<u64>) {
    let start = *APP_START.get_or_init(Instant::now);
    let total = start.elapsed().as_secs();
    let prev = LAST_MARKER_SECS.swap(total, Ordering::Relaxed);
    let delta = if prev == u64::MAX {
        None
    } else {
        Some(total.saturating_sub(prev))
    };
    (total, delta)
}

/// Read-only peek at elapsed counters — does NOT update last-marker. Use this
/// when you need the t+/Δ values for formatting but aren't actually setting a
/// new marker.
pub fn peek_elapsed() -> (u64, Option<u64>) {
    let start = *APP_START.get_or_init(Instant::now);
    let total = start.elapsed().as_secs();
    let prev = LAST_MARKER_SECS.load(Ordering::Relaxed);
    let delta = if prev == u64::MAX {
        None
    } else {
        Some(total.saturating_sub(prev))
    };
    (total, delta)
}

/// Custom timer: `HH:MM:SS.mmm [t+HH:MM:SS Δ00:XX:XX]` prepended to every log line.
/// The Δ resets each time `stamp_marker()` is called.
struct MarkerTimer;

impl FormatTime for MarkerTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        // Delegate the wall-clock date/time to tracing-subscriber's default
        // SystemTime timer (e.g. `2026-04-16T09:47:12.453123Z`).
        TsSystemTime.format_time(w)?;

        let start = *APP_START.get_or_init(Instant::now);
        let total = start.elapsed().as_secs();
        let last = LAST_MARKER_SECS.load(Ordering::Relaxed);
        let hms = |t: u64| (t / 3600, (t / 60) % 60, t % 60);
        let (th, tm, ts) = hms(total);
        if last == u64::MAX {
            write!(w, " [t+{:02}:{:02}:{:02} Δ--:--:--]", th, tm, ts)
        } else {
            let (dh, dm, ds) = hms(total.saturating_sub(last));
            write!(
                w,
                " [t+{:02}:{:02}:{:02} Δ{:02}:{:02}:{:02}]",
                th, tm, ts, dh, dm, ds
            )
        }
    }
}

/// Initialize the tracing/logging subsystem.
///
/// Supports human-readable or JSON output, optional file logging with rotation,
/// and log-level filtering via CLI or the `RUST_LOG` environment variable.
pub fn init(level: &str, format: &str, log_file: Option<&str>) {
    // Establish the process start instant early so MarkerTimer has a stable reference.
    APP_START.get_or_init(Instant::now);

    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));

    match format {
        "json" => {
            if let Some(path) = log_file {
                let file_appender = tracing_appender::rolling::daily(
                    std::path::Path::new(path)
                        .parent()
                        .unwrap_or(std::path::Path::new(".")),
                    std::path::Path::new(path)
                        .file_name()
                        .unwrap_or(std::ffi::OsStr::new("novus2mqtt.log")),
                );
                let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
                std::mem::forget(_guard);

                tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt::layer().json().with_writer(non_blocking))
                    .with(fmt::layer().json().with_timer(MarkerTimer))
                    .init();
            } else {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt::layer().json().with_timer(MarkerTimer))
                    .init();
            }
        }
        _ => {
            if let Some(path) = log_file {
                let file_appender = tracing_appender::rolling::daily(
                    std::path::Path::new(path)
                        .parent()
                        .unwrap_or(std::path::Path::new(".")),
                    std::path::Path::new(path)
                        .file_name()
                        .unwrap_or(std::ffi::OsStr::new("novus2mqtt.log")),
                );
                let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
                std::mem::forget(_guard);

                tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt::layer().with_writer(non_blocking))
                    .with(fmt::layer().with_timer(MarkerTimer))
                    .init();
            } else {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(fmt::layer().with_timer(MarkerTimer))
                    .init();
            }
        }
    }
}
