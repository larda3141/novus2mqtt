use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;
use sysinfo::System;

/// Performance metrics tracked with atomic counters.
pub struct PerformanceMonitor {
    start_time: Instant,
    pub messages_processed: AtomicU64,
    pub mqtt_messages_sent: AtomicU64,
    pub mqtt_messages_received: AtomicU64,
    pub bus_messages_sent: AtomicU64,
    pub bus_messages_received: AtomicU64,
    pub write_queue_dropped: AtomicU64,
    pub write_queue_max_depth: AtomicU64,
    pub memory_guard_trips: AtomicU64,
    pub errors_count: AtomicU64,
}

#[derive(Debug, Serialize)]
pub struct MetricsSnapshot {
    pub uptime_seconds: f64,
    pub messages_processed: u64,
    pub mqtt_messages_sent: u64,
    pub mqtt_messages_received: u64,
    pub bus_messages_sent: u64,
    pub bus_messages_received: u64,
    pub write_queue_dropped: u64,
    pub write_queue_max_depth: u64,
    pub memory_guard_trips: u64,
    pub errors_count: u64,
    pub memory_usage_mb: f64,
    pub cpu_percent: f32,
    pub thread_count: usize,
}

impl PerformanceMonitor {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            messages_processed: AtomicU64::new(0),
            mqtt_messages_sent: AtomicU64::new(0),
            mqtt_messages_received: AtomicU64::new(0),
            bus_messages_sent: AtomicU64::new(0),
            bus_messages_received: AtomicU64::new(0),
            write_queue_dropped: AtomicU64::new(0),
            write_queue_max_depth: AtomicU64::new(0),
            memory_guard_trips: AtomicU64::new(0),
            errors_count: AtomicU64::new(0),
        }
    }

    /// Take a snapshot of current metrics including system stats.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let mut sys = System::new();
        let pid = sysinfo::get_current_pid().ok();

        let (memory_mb, cpu) = if let Some(pid) = pid {
            sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
            if let Some(proc_) = sys.process(pid) {
                (proc_.memory() as f64 / 1024.0 / 1024.0, proc_.cpu_usage())
            } else {
                (0.0, 0.0)
            }
        } else {
            (0.0, 0.0)
        };

        MetricsSnapshot {
            uptime_seconds: self.start_time.elapsed().as_secs_f64(),
            messages_processed: self.messages_processed.load(Ordering::Relaxed),
            mqtt_messages_sent: self.mqtt_messages_sent.load(Ordering::Relaxed),
            mqtt_messages_received: self.mqtt_messages_received.load(Ordering::Relaxed),
            bus_messages_sent: self.bus_messages_sent.load(Ordering::Relaxed),
            bus_messages_received: self.bus_messages_received.load(Ordering::Relaxed),
            write_queue_dropped: self.write_queue_dropped.load(Ordering::Relaxed),
            write_queue_max_depth: self.write_queue_max_depth.load(Ordering::Relaxed),
            memory_guard_trips: self.memory_guard_trips.load(Ordering::Relaxed),
            errors_count: self.errors_count.load(Ordering::Relaxed),
            memory_usage_mb: memory_mb,
            cpu_percent: cpu,
            thread_count: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
        }
    }

    /// Increment a counter by 1.
    pub fn increment(&self, counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Get current memory usage in MB.
    pub fn memory_usage_mb(&self) -> f64 {
        let mut sys = System::new();
        let pid = sysinfo::get_current_pid().ok();
        if let Some(pid) = pid {
            sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
            if let Some(proc_) = sys.process(pid) {
                return proc_.memory() as f64 / 1024.0 / 1024.0;
            }
        }
        0.0
    }
}
