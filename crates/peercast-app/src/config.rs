use std::net::IpAddr;
use std::str::FromStr;

use peercast_rpc::ServentSettings;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current application configuration schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// Default listening port for PeerCast.
pub const DEFAULT_PORT: u16 = 7144;

/// Default bind address (loopback for security).
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1";

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
    #[error("remote administration is enabled but auth_token is missing or shorter than 8 characters")]
    InsecureRemoteAdminToken,
    #[error("invalid log level '{0}', expected one of: trace, debug, info, warn, error")]
    InvalidLogLevel(String),
    #[error("max_log_bytes ({0}) must be at least 1024 bytes")]
    LogBytesTooSmall(usize),
    #[error("shutdown_timeout_secs ({0}) must be >= 1 second")]
    ZeroShutdownTimeout(u64),
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
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind_address: DEFAULT_BIND_ADDRESS.to_string(),
            allow_remote_admin: false,
            auth_token: None,
            max_relays: 16,
            max_relays_per_channel: 4,
            max_directs: 8,
            max_directs_per_channel: 2,
            max_upstream_rate: 10000,
            max_upstream_rate_ipv6: 10000,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_file: Option<String>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            max_log_bytes: 5 * 1024 * 1024, // 5 MiB
            max_backups: 3,
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
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            server: ServerConfig::default(),
            logging: LogConfig::default(),
            lifecycle: LifecycleConfig::default(),
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
            "trace" | "debug" | "info" | "warn" | "error" => {}
            _ => return Err(ConfigValidationError::InvalidLogLevel(self.logging.level.clone())),
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
