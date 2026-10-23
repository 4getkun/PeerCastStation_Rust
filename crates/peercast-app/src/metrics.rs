use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Snapshot of system operational metrics (REQ-067).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub uptime_secs: u64,
    pub active_tasks: usize,
    pub active_connections: usize,
    pub active_channels: usize,
    pub bytes_sent: u64,
    pub bytes_received: u64,
}

/// RAII guard for an active network connection.
pub struct ConnectionGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Real-time metrics tracker for application resources and traffic (REQ-067, REQ-014).
pub struct AppMetrics {
    start_time: Instant,
    active_connections: Arc<AtomicUsize>,
    active_channels: AtomicUsize,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
}

impl Default for AppMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl AppMetrics {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            active_connections: Arc::new(AtomicUsize::new(0)),
            active_channels: AtomicUsize::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
        }
    }

    /// Registers a new active connection returning an RAII decrementing guard.
    pub fn track_connection(&self) -> ConnectionGuard {
        self.active_connections.fetch_add(1, Ordering::SeqCst);
        ConnectionGuard {
            counter: Arc::clone(&self.active_connections),
        }
    }

    /// Increments active channels count.
    pub fn inc_channels(&self) {
        self.active_channels.fetch_add(1, Ordering::SeqCst);
    }

    /// Decrements active channels count.
    pub fn dec_channels(&self) {
        self.active_channels.fetch_sub(1, Ordering::SeqCst);
    }

    /// Records egress bytes transmitted.
    pub fn add_bytes_sent(&self, bytes: u64) {
        self.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Records ingress bytes received.
    pub fn add_bytes_received(&self, bytes: u64) {
        self.bytes_received.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Captures a point-in-time metrics snapshot.
    pub fn snapshot(&self, current_tasks: usize) -> MetricsSnapshot {
        MetricsSnapshot {
            uptime_secs: self.start_time.elapsed().as_secs(),
            active_tasks: current_tasks,
            active_connections: self.active_connections.load(Ordering::SeqCst),
            active_channels: self.active_channels.load(Ordering::SeqCst),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
        }
    }
}
