//! Application orchestration, configuration persistence, CLI handling, lifecycle controls,
//! structured logging, and system metrics for PeerCastStation (REQ-014, REQ-067).

pub mod cli;
pub mod config;
pub mod lifecycle;
pub mod logging;
pub mod metrics;
pub mod persistence;
pub mod server;
pub mod shutdown;

pub use cli::AppCliArgs;
pub use config::{
    AppConfig, ConfigValidationError, LifecycleConfig, LogConfig, ServerConfig,
    CURRENT_SCHEMA_VERSION, DEFAULT_BIND_ADDRESS, DEFAULT_PORT,
};
pub use lifecycle::{
    AppLifecycleManager, LifecyclePhase, ShutdownReason, TaskGuard, TaskTracker,
};
pub use logging::{BoundedLogBuffer, LogEntry, LogLevel};
pub use metrics::{AppMetrics, ConnectionGuard, MetricsSnapshot};
pub use persistence::{ConfigManager, ConfigPersistenceError};
pub use server::HttpServer;
pub use shutdown::{wait_for_os_shutdown_signal, PlatformSignalSupport};
