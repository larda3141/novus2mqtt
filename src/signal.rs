use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tracing::info;

/// Shared shutdown flag checked by all components.
pub type ShutdownFlag = Arc<AtomicBool>;

/// Create a new shutdown flag.
pub fn new_shutdown_flag() -> ShutdownFlag {
    Arc::new(AtomicBool::new(false))
}

/// Spawn a task that sets the shutdown flag on SIGINT (Ctrl+C) or SIGTERM (Unix).
pub fn spawn_signal_handler(shutdown: ShutdownFlag) {
    tokio::spawn(async move {
        wait_for_signal().await;
        info!("Signal handler: shutdown requested");
        shutdown.store(true, Ordering::SeqCst);
    });
}

/// Wait for a termination signal (cross-platform).
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    }
}
