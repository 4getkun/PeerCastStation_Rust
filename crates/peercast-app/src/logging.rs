use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use peercast_rpc::redact_secrets;
use serde::Serialize;

pub(crate) const DEFAULT_LOG_CAPACITY: usize = 1000;

/// Severity level of a structured log record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum LogLevel {
    #[serde(rename = "TRACE")]
    Trace,
    #[serde(rename = "DEBUG")]
    Debug,
    #[serde(rename = "INFO")]
    Info,
    #[serde(rename = "WARN")]
    Warn,
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "OFF")]
    Off,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Off => "OFF",
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Trace => 0,
            Self::Debug => 1,
            Self::Info => 2,
            Self::Warn => 3,
            Self::Error => 4,
            Self::Off => 5,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Trace,
            1 => Self::Debug,
            2 => Self::Info,
            3 => Self::Warn,
            4 => Self::Error,
            _ => Self::Off,
        }
    }

    pub fn from_config(value: &str) -> io::Result<Self> {
        value.parse()
    }
}

impl FromStr for LogLevel {
    type Err = io::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            "off" | "none" => Ok(Self::Off),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid log level: {value}"),
            )),
        }
    }
}

/// A structured log record with redacted content and bounded timestamp (REQ-067, REQ-065).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub timestamp_ms: u64,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
}

struct RotatingFile {
    path: PathBuf,
    file: Option<File>,
    current_bytes: u64,
    max_bytes: usize,
    max_backups: usize,
}

impl RotatingFile {
    fn open(path: PathBuf, max_bytes: usize, max_backups: usize) -> io::Result<Self> {
        if max_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_log_bytes must be positive",
            ));
        }
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let current_bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let mut rotating = Self {
            path,
            file: Some(file),
            current_bytes,
            max_bytes,
            max_backups,
        };
        if rotating.current_bytes > rotating.max_bytes as u64 {
            rotating.rotate()?;
        }
        Ok(rotating)
    }

    fn write_line(&mut self, line: &[u8]) -> io::Result<()> {
        if line.len() > self.max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "serialized log line exceeds max_log_bytes",
            ));
        }
        if self.current_bytes.saturating_add(line.len() as u64) > self.max_bytes as u64 {
            self.rotate()?;
        }
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file is unavailable"))?
            .write_all(line)?;
        self.current_bytes = self.current_bytes.saturating_add(line.len() as u64);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        if self.max_backups == 0 {
            self.file.take();
            self.file = Some(
                OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&self.path)?,
            );
            self.current_bytes = 0;
            return Ok(());
        }

        self.file.take();
        let oldest = backup_path(&self.path, self.max_backups);
        match fs::remove_file(oldest) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        for index in (1..self.max_backups).rev() {
            let source = backup_path(&self.path, index);
            let destination = backup_path(&self.path, index + 1);
            match fs::rename(source, destination) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }

        match fs::rename(&self.path, backup_path(&self.path, 1)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.file = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        );
        self.current_bytes = 0;
        Ok(())
    }
}

fn backup_path(path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.to_string_lossy(), index))
}

struct LogState {
    entries: VecDeque<LogEntry>,
    next_sequence: u64,
    file: Option<RotatingFile>,
    stderr: bool,
}

/// In-memory bounded FIFO log buffer preventing unbounded memory growth (Global Rule 3, REQ-067).
pub struct BoundedLogBuffer {
    capacity: usize,
    minimum_level: AtomicU8,
    known_secrets: Vec<String>,
    file_max_bytes: Option<usize>,
    production_sinks: bool,
    state: Mutex<LogState>,
}

impl Default for BoundedLogBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_LOG_CAPACITY)
    }
}

impl BoundedLogBuffer {
    /// Creates an in-memory-only logger that accepts every level.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "log buffer capacity must be positive");
        Self {
            capacity,
            minimum_level: AtomicU8::new(LogLevel::Trace.as_u8()),
            known_secrets: Vec::new(),
            file_max_bytes: None,
            production_sinks: false,
            state: Mutex::new(LogState {
                entries: VecDeque::with_capacity(capacity),
                next_sequence: 0,
                file: None,
                stderr: false,
            }),
        }
    }

    /// Creates a production logger using the configured memory, file and stderr sinks.
    pub fn from_config(config: &crate::config::LogConfig, secrets: &[&str]) -> io::Result<Self> {
        let minimum_level = LogLevel::from_config(&config.level)?;
        if config.max_in_memory_entries == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_in_memory_entries must be positive",
            ));
        }
        if config.max_log_bytes < 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_log_bytes must be at least 1024 bytes",
            ));
        }
        let file = config
            .log_file
            .as_ref()
            .map(|path| {
                RotatingFile::open(
                    PathBuf::from(path),
                    config.max_log_bytes,
                    config.max_backups,
                )
            })
            .transpose()?;
        Ok(Self {
            capacity: config.max_in_memory_entries,
            minimum_level: AtomicU8::new(minimum_level.as_u8()),
            known_secrets: secrets.iter().map(|secret| (*secret).to_owned()).collect(),
            file_max_bytes: config.log_file.as_ref().map(|_| config.max_log_bytes),
            production_sinks: true,
            state: Mutex::new(LogState {
                entries: VecDeque::with_capacity(config.max_in_memory_entries),
                next_sequence: 0,
                file,
                stderr: true,
            }),
        })
    }

    pub fn minimum_level(&self) -> LogLevel {
        LogLevel::from_u8(self.minimum_level.load(Ordering::Relaxed))
    }

    pub fn set_minimum_level(&self, level: LogLevel) {
        self.minimum_level.store(level.as_u8(), Ordering::Relaxed);
    }

    /// Records a structured log entry, redacting sensitive values and enforcing bounded capacity.
    pub fn log(&self, level: LogLevel, target: &str, raw_message: &str) {
        self.log_with_secrets(level, target, raw_message, &[]);
    }

    /// Records a structured log entry with additional explicit secrets to redact.
    pub fn log_with_secrets(
        &self,
        level: LogLevel,
        target: &str,
        raw_message: &str,
        secrets: &[&str],
    ) {
        if level == LogLevel::Off || level < self.minimum_level() {
            return;
        }
        let secrets: Vec<&str> = self
            .known_secrets
            .iter()
            .map(String::as_str)
            .chain(secrets.iter().copied())
            .collect();
        let mut entry = LogEntry {
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0),
            level,
            target: redact_secrets(target, &secrets),
            message: redact_secrets(raw_message, &secrets),
        };

        if let Some(max_bytes) = self.file_max_bytes {
            entry = bounded_json_entry(entry, max_bytes);
        }
        let json_line = self
            .production_sinks
            .then(|| serde_json::to_vec(&entry).ok().map(append_newline))
            .flatten();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let file_error = match (json_line.as_ref(), state.file.as_mut()) {
            (Some(line), Some(file)) => file.write_line(line).err(),
            _ => None,
        };
        if file_error.is_some() {
            state.file = None;
        }
        if state.stderr {
            if let Some(line) = json_line.as_ref() {
                let mut stderr = io::stderr().lock();
                let _ = stderr.write_all(line);
            }
        }
        if state.entries.len() >= self.capacity {
            state.entries.pop_front();
        }
        state.entries.push_back(entry);
        state.next_sequence = state.next_sequence.saturating_add(1);
        drop(state);
        if let Some(error) = file_error {
            self.log(LogLevel::Error, "logging", &format!("File logging disabled after write failure; memory/stderr remain available: {error}"));
        }
    }

    pub fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .iter()
            .cloned()
            .collect()
    }

    /// Returns a consistent cursor and retained records for incremental polling.
    pub fn window(&self) -> (u64, Vec<LogEntry>) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            state
                .next_sequence
                .saturating_sub(state.entries.len() as u64),
            state.entries.iter().cloned().collect(),
        )
    }

    pub fn clear(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.entries.clear();
        state.next_sequence = 0;
    }
}

fn bounded_json_entry(entry: LogEntry, max_bytes: usize) -> LogEntry {
    let mut candidate = entry;
    if let Ok(line) = serde_json::to_vec(&candidate) {
        if line.len().saturating_add(1) <= max_bytes {
            return candidate;
        }
    }

    candidate.target = truncate_utf8(&candidate.target, max_bytes / 4);
    let marker = "…[truncated]";
    let original_message = candidate.message.clone();
    let mut low = 0;
    let mut high = original_message.chars().count();
    let mut best = String::new();
    while low <= high {
        let mid = low + (high - low) / 2;
        let prefix: String = original_message.chars().take(mid).collect();
        let mut message = prefix;
        if mid < original_message.chars().count() {
            message.push_str(marker);
        }
        candidate.message = message;
        if serde_json::to_vec(&candidate)
            .is_ok_and(|line| line.len().saturating_add(1) <= max_bytes)
        {
            best.clone_from(&candidate.message);
            low = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            high = mid - 1;
        }
    }
    candidate.message = best;
    candidate
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn append_newline(mut line: Vec<u8>) -> Vec<u8> {
    line.push(b'\n');
    line
}
