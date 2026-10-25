use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;

use crate::runtime_config::RuntimeConfig;
use peercast_core::ContentRingBuffer;
use peercast_rpc::ServentSettings;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current application configuration schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// Default configuration file used when no CLI path is provided.
pub const DEFAULT_CONFIG_FILENAME: &str = "peercast.json";

/// Default listening port for PeerCast.
pub const DEFAULT_PORT: u16 = 7144;

/// Default bind address (loopback for security).
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1";

fn default_max_request_buffer_bytes() -> usize {
    8192
}

fn default_request_timeout_ms() -> u64 {
    5000
}

fn default_max_rpc_request_bytes() -> usize {
    peercast_rpc::server::DEFAULT_MAX_RPC_REQUEST_BYTES
}

fn default_ringbuffer_max_bytes() -> usize {
    peercast_core::ringbuffer::DEFAULT_RINGBUFFER_MAX_BYTES
}

fn default_ringbuffer_window_secs() -> u64 {
    peercast_core::ringbuffer::DEFAULT_RINGBUFFER_TIME_WINDOW.as_secs()
}

fn default_max_in_memory_entries() -> usize {
    crate::logging::DEFAULT_LOG_CAPACITY
}

/// Configuration validation errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigValidationError {
    #[error("port must be non-zero")]
    InvalidPort,
    #[error("bind address '{0}' is not a valid IP address")]
    InvalidBindAddress(String),
    #[error("max_relays ({0}) must be >= max_relays_per_channel ({1})")]
    MaxRelaysLessThanPerChannel(u32, u32),
    #[error("max_directs ({0}) must be >= max_directs_per_channel ({1})")]
    MaxDirectsLessThanPerChannel(u32, u32),
    #[error("max_upstream_rate must be > 0")]
    ZeroUpstreamRate,
    #[error(
        "remote administration is enabled but auth_token is missing or shorter than 8 characters"
    )]
    InsecureRemoteAdminToken,
    #[error("invalid log level '{0}', expected one of: trace, debug, info, warn, error")]
    InvalidLogLevel(String),
    #[error("max_log_bytes ({0}) must be at least 1024 bytes")]
    LogBytesTooSmall(usize),
    #[error("shutdown_timeout_secs ({0}) must be >= 1 second")]
    ZeroShutdownTimeout(u64),
    #[error("configuration field '{0}' must be greater than zero")]
    ZeroConfigValue(&'static str),
    #[error("invalid runtime configuration: {0}")]
    InvalidRuntimeConfig(String),
}

/// Server network and protocol limits configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub port: u16,
    pub bind_address: String,
    pub allow_remote_admin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    pub max_relays: u32,
    pub max_relays_per_channel: u32,
    pub max_directs: u32,
    pub max_directs_per_channel: u32,
    pub max_upstream_rate: u32,
    pub max_upstream_rate_ipv6: u32,
    #[serde(default = "default_max_request_buffer_bytes")]
    /// Maximum buffered HTTP request size, including headers and body.
    pub max_request_buffer_bytes: usize,
    #[serde(default = "default_request_timeout_ms")]
    /// Deadline for reading the complete HTTP request, in milliseconds.
    pub request_timeout_ms: u64,
    #[serde(default = "default_max_rpc_request_bytes")]
    /// JSON-RPC body limit, independently enforced by the dispatcher.
    pub max_rpc_request_bytes: usize,
    #[serde(default = "default_ringbuffer_max_bytes")]
    /// Byte cap for buffers constructed through `create_content_ring_buffer`.
    pub ringbuffer_max_bytes: usize,
    #[serde(default = "default_ringbuffer_window_secs")]
    /// Retention window for configured content buffers, in seconds.
    pub ringbuffer_window_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let settings = ServentSettings::default();
        Self {
            port: DEFAULT_PORT,
            bind_address: DEFAULT_BIND_ADDRESS.to_string(),
            allow_remote_admin: false,
            auth_token: None,
            max_relays: settings.max_relays,
            max_relays_per_channel: settings.max_relays_per_channel,
            max_directs: settings.max_directs,
            max_directs_per_channel: settings.max_directs_per_channel,
            max_upstream_rate: settings.max_upstream_rate,
            max_upstream_rate_ipv6: settings.max_upstream_rate_ipv6,
            max_request_buffer_bytes: default_max_request_buffer_bytes(),
            request_timeout_ms: default_request_timeout_ms(),
            max_rpc_request_bytes: default_max_rpc_request_bytes(),
            ringbuffer_max_bytes: default_ringbuffer_max_bytes(),
            ringbuffer_window_secs: default_ringbuffer_window_secs(),
        }
    }
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LogConfig {
    pub level: String,
    pub max_log_bytes: usize,
    pub max_backups: usize,
    #[serde(default = "default_max_in_memory_entries")]
    pub max_in_memory_entries: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_file: Option<String>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            max_log_bytes: 5 * 1024 * 1024, // 5 MiB
            max_backups: 3,
            max_in_memory_entries: default_max_in_memory_entries(),
            log_file: None,
        }
    }
}

/// Application lifecycle configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleConfig {
    pub headless: bool,
    pub shutdown_timeout_secs: u64,
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            headless: false,
            shutdown_timeout_secs: 5,
        }
    }
}

/// Top-level application configuration with versioned schema (REQ-067).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub schema_version: u32,
    pub server: ServerConfig,
    pub logging: LogConfig,
    pub lifecycle: LifecycleConfig,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    /// Optional desktop preferences, validated by the desktop application.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub desktop: serde_json::Value,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            server: ServerConfig::default(),
            logging: LogConfig::default(),
            lifecycle: LifecycleConfig::default(),
            runtime: RuntimeConfig::default(),
            desktop: serde_json::Value::Null,
        }
    }
}

impl AppConfig {
    /// Validates all configuration settings according to invariants (REQ-067, REQ-065).
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        // Server network invariants
        if self.server.port == 0 {
            return Err(ConfigValidationError::InvalidPort);
        }
        if IpAddr::from_str(&self.server.bind_address).is_err() {
            return Err(ConfigValidationError::InvalidBindAddress(
                self.server.bind_address.clone(),
            ));
        }
        if self.server.max_relays < self.server.max_relays_per_channel {
            return Err(ConfigValidationError::MaxRelaysLessThanPerChannel(
                self.server.max_relays,
                self.server.max_relays_per_channel,
            ));
        }
        if self.server.max_directs < self.server.max_directs_per_channel {
            return Err(ConfigValidationError::MaxDirectsLessThanPerChannel(
                self.server.max_directs,
                self.server.max_directs_per_channel,
            ));
        }
        if self.server.max_upstream_rate == 0 {
            return Err(ConfigValidationError::ZeroUpstreamRate);
        }
        if self.server.max_request_buffer_bytes == 0 {
            return Err(ConfigValidationError::ZeroConfigValue(
                "server.maxRequestBufferBytes",
            ));
        }
        if self.server.request_timeout_ms == 0 {
            return Err(ConfigValidationError::ZeroConfigValue(
                "server.requestTimeoutMs",
            ));
        }
        if self.server.max_rpc_request_bytes == 0 {
            return Err(ConfigValidationError::ZeroConfigValue(
                "server.maxRpcRequestBytes",
            ));
        }
        if self.server.ringbuffer_max_bytes == 0 {
            return Err(ConfigValidationError::ZeroConfigValue(
                "server.ringbufferMaxBytes",
            ));
        }
        if self.server.ringbuffer_window_secs == 0 {
            return Err(ConfigValidationError::ZeroConfigValue(
                "server.ringbufferWindowSecs",
            ));
        }
        if self.logging.max_in_memory_entries == 0 {
            return Err(ConfigValidationError::ZeroConfigValue(
                "logging.maxInMemoryEntries",
            ));
        }

        self.runtime
            .validate()
            .map_err(ConfigValidationError::InvalidRuntimeConfig)?;
        if self.runtime.port_mapping.enabled
            && !matches!(self.server.bind_address.parse::<IpAddr>(), Ok(IpAddr::V4(ip)) if !ip.is_loopback())
        {
            return Err(ConfigValidationError::InvalidRuntimeConfig(
                "portMapping requires an IPv4 LAN address or 0.0.0.0 listener".into(),
            ));
        }

        // Security invariants (REQ-065)
        if self.server.allow_remote_admin {
            match &self.server.auth_token {
                Some(token) if token.trim().len() >= 8 => {}
                _ => return Err(ConfigValidationError::InsecureRemoteAdminToken),
            }
        }

        // Logging invariants
        let level = self.logging.level.to_ascii_lowercase();
        match level.as_str() {
            "trace" | "debug" | "info" | "warn" | "error" | "off" | "none" => {}
            _ => {
                return Err(ConfigValidationError::InvalidLogLevel(
                    self.logging.level.clone(),
                ))
            }
        }
        if self.logging.max_log_bytes < 1024 {
            return Err(ConfigValidationError::LogBytesTooSmall(
                self.logging.max_log_bytes,
            ));
        }

        // Lifecycle invariants
        if self.lifecycle.shutdown_timeout_secs == 0 {
            return Err(ConfigValidationError::ZeroShutdownTimeout(
                self.lifecycle.shutdown_timeout_secs,
            ));
        }

        Ok(())
    }

    /// Creates a content ring buffer using the configured byte and time limits.
    pub fn create_content_ring_buffer(&self) -> ContentRingBuffer {
        ContentRingBuffer::new(
            self.server.ringbuffer_max_bytes,
            Duration::from_secs(self.server.ringbuffer_window_secs),
        )
    }

    /// Converts relevant server settings into RPC `ServentSettings` (REQ-064).
    pub fn to_servent_settings(&self) -> ServentSettings {
        ServentSettings {
            max_relays: self.server.max_relays,
            max_relays_per_channel: self.server.max_relays_per_channel,
            max_directs: self.server.max_directs,
            max_directs_per_channel: self.server.max_directs_per_channel,
            max_upstream_rate: self.server.max_upstream_rate,
            max_upstream_rate_ipv6: self.server.max_upstream_rate_ipv6,
        }
    }

    /// Updates internal settings from an RPC `ServentSettings` mutation.
    pub fn apply_servent_settings(&mut self, s: &ServentSettings) {
        self.server.max_relays = s.max_relays;
        self.server.max_relays_per_channel = s.max_relays_per_channel;
        self.server.max_directs = s.max_directs;
        self.server.max_directs_per_channel = s.max_directs_per_channel;
        self.server.max_upstream_rate = s.max_upstream_rate;
        self.server.max_upstream_rate_ipv6 = s.max_upstream_rate_ipv6;
    }
}
