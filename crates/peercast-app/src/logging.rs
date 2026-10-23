use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use peercast_rpc::redact_secrets;

/// Severity level of a structured log record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

/// A structured log record with redacted content and bounded timestamp (REQ-067, REQ-065).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub timestamp_ms: u64,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
}

/// In-memory bounded FIFO log buffer preventing unbounded memory growth (Global Rule 3, REQ-067).
pub struct BoundedLogBuffer {
    capacity: usize,
    entries: Mutex<VecDeque<LogEntry>>,
}

impl BoundedLogBuffer {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "log buffer capacity must be positive");
        Self {
            capacity,
            entries: Mutex::new(VecDeque::with_capacity(capacity)),
        }
    }

    /// Records a structured log entry, redacting any sensitive tokens and enforcing bounded capacity.
    pub fn log(&self, level: LogLevel, target: &str, raw_message: &str) {
        self.log_with_secrets(level, target, raw_message, &[]);
    }

    /// Records a structured log entry with explicit secrets to redact (REQ-065).
    pub fn log_with_secrets(&self, level: LogLevel, target: &str, raw_message: &str, secrets: &[&str]) {
        // Redact secrets before storing (REQ-065)
        let redacted = redact_secrets(raw_message, secrets);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let entry = LogEntry {
            timestamp_ms: now_ms,
            level,
            target: target.to_string(),
            message: redacted,
        };

        let mut queue = self.entries.lock().unwrap();
        if queue.len() >= self.capacity {
            queue.pop_front(); // Evict oldest (Sliding Window FIFO)
        }
        queue.push_back(entry);
    }

    /// Current number of retained log entries.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// Returns `true` if no entries are currently stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Maximum capacity of the buffer.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns a snapshot of all retained log entries.
    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.entries.lock().unwrap().iter().cloned().collect()
    }

    /// Clears all entries from the buffer.
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }
}
