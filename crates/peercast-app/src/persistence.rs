use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use thiserror::Error;

use crate::config::{
    AppConfig, ConfigValidationError, CURRENT_SCHEMA_VERSION,
};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Errors occurring during configuration persistence, loading, or migration.
#[derive(Debug, Error)]
pub enum ConfigPersistenceError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization/deserialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("configuration validation failed: {0}")]
    Validation(#[from] ConfigValidationError),
    #[error("migration backup failed: {0}")]
    MigrationBackupFailed(String),
    #[error("unsupported schema version: {0}")]
    UnsupportedSchemaVersion(u32),
    #[error("configuration file not found: {0}")]
    NotFound(PathBuf),
}

/// Manages persistence, atomic saves, and version migrations for application configurations (REQ-067).
pub struct ConfigManager {
    config_path: PathBuf,
}

impl ConfigManager {
    pub fn new<P: AsRef<Path>>(config_path: P) -> Self {
        Self {
            config_path: config_path.as_ref().to_path_buf(),
        }
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Loads the configuration from disk, performing safe migration if older schema is detected.
    ///
    /// Preserves a backup before upgrading (REQ-067).
    pub fn load(&self) -> Result<AppConfig, ConfigPersistenceError> {
        if !self.config_path.exists() {
            return Err(ConfigPersistenceError::NotFound(self.config_path.clone()));
        }

        let mut file = File::open(&self.config_path)?;
        let mut raw_bytes = Vec::new();
        file.read_to_end(&mut raw_bytes)?;

        // Parse as generic JSON Value to inspect schema version
        let val: Value = serde_json::from_slice(&raw_bytes)?;

        let version = val
            .get("schemaVersion")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as u32;

        if version == CURRENT_SCHEMA_VERSION {
            let config: AppConfig = serde_json::from_value(val)?;
            config.validate()?;
            Ok(config)
        } else if version < CURRENT_SCHEMA_VERSION {
            // Older schema: perform safe migration with verified backup
            self.migrate_and_persist(&raw_bytes, version, &val)
        } else {
            Err(ConfigPersistenceError::UnsupportedSchemaVersion(version))
        }
    }

    /// Loads configuration if it exists, or creates and atomically persists a default configuration.
    pub fn load_or_create_default(&self) -> Result<AppConfig, ConfigPersistenceError> {
        match self.load() {
            Ok(cfg) => Ok(cfg),
            Err(ConfigPersistenceError::NotFound(_)) => {
                let default_cfg = AppConfig::default();
                default_cfg.validate()?;
                self.save(&default_cfg)?;
                Ok(default_cfg)
            }
            Err(e) => Err(e),
        }
    }

    /// Atomically persists configuration to disk using write-to-temp + fsync + rename (REQ-067).
    pub fn save(&self, config: &AppConfig) -> Result<(), ConfigPersistenceError> {
        config.validate()?;

        let serialized = serde_json::to_vec_pretty(config)?;

        // Ensure parent directory exists
        if let Some(parent) = self.config_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let pid = std::process::id();
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_filename = format!(
            "{}.tmp.{}_{}",
            self.config_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("config"),
            pid,
            counter
        );
        let temp_path = match self.config_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join(temp_filename),
            _ => PathBuf::from(temp_filename),
        };

        // Write to temporary file with explicit sync
        let write_res = (|| -> Result<(), std::io::Error> {
            let mut file = File::create(&temp_path)?;
            file.write_all(&serialized)?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp_path, &self.config_path)?;
            Ok(())
        })();

        if let Err(e) = write_res {
            // Clean up temporary file if rename was not reached or failed
            let _ = fs::remove_file(&temp_path);
            return Err(ConfigPersistenceError::Io(e));
        }

        Ok(())
    }

    /// Migrates an older configuration JSON to the current schema with verified backup preservation.
    ///
    /// Stop/block guard: Migration NEVER overwrites user data without a verified backup (REQ-067).
    fn migrate_and_persist(
        &self,
        raw_bytes: &[u8],
        old_version: u32,
        val: &Value,
    ) -> Result<AppConfig, ConfigPersistenceError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let backup_filename = format!(
            "{}.bak.{}_v{}",
            self.config_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("config"),
            timestamp,
            old_version
        );

        let backup_path = match self.config_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join(backup_filename),
            _ => PathBuf::from(backup_filename),
        };

        // 1. Create and verify backup BEFORE any modification
        let backup_res = (|| -> Result<(), std::io::Error> {
            let mut bf = File::create(&backup_path)?;
            bf.write_all(raw_bytes)?;
            bf.flush()?;
            bf.sync_all()?;
            Ok(())
        })();

        if let Err(e) = backup_res {
            return Err(ConfigPersistenceError::MigrationBackupFailed(format!(
                "Failed to write backup {}: {}",
                backup_path.display(),
                e
            )));
        }

        // Verify backup exists and non-empty
        let meta = fs::metadata(&backup_path).map_err(|e| {
            ConfigPersistenceError::MigrationBackupFailed(format!(
                "Failed to verify backup {}: {}",
                backup_path.display(),
                e
            ))
        })?;
        if meta.len() == 0 {
            return Err(ConfigPersistenceError::MigrationBackupFailed(
                "Backup file created was empty; aborting migration to protect user data".to_string(),
            ));
        }

        // 2. Perform migration logic
        let migrated_config = match old_version {
            1 => migrate_v1_to_v2(val)?,
            _ => return Err(ConfigPersistenceError::UnsupportedSchemaVersion(old_version)),
        };

        // 3. Validate migrated config
        migrated_config.validate()?;

        // 4. Atomically persist upgraded configuration
        self.save(&migrated_config)?;

        Ok(migrated_config)
    }
}

/// Migrates schema v1 (unversioned or v1) to schema v2.
fn migrate_v1_to_v2(v1: &Value) -> Result<AppConfig, ConfigPersistenceError> {
    let mut config = AppConfig::default();

    // Map server fields if present
    if let Some(srv) = v1.get("server") {
        if let Some(port) = srv.get("port").and_then(|p| p.as_u64()) {
            config.server.port = port as u16;
        }
        if let Some(bind) = srv.get("bindAddress").and_then(|b| b.as_str()) {
            config.server.bind_address = bind.to_string();
        }
        if let Some(mr) = srv.get("maxRelays").and_then(|m| m.as_u64()) {
            config.server.max_relays = mr as u32;
        }
        if let Some(mr_pc) = srv.get("maxRelaysPerChannel").and_then(|m| m.as_u64()) {
            config.server.max_relays_per_channel = mr_pc as u32;
        }
        if let Some(md) = srv.get("maxDirects").and_then(|m| m.as_u64()) {
            config.server.max_directs = md as u32;
        }
        if let Some(md_pc) = srv.get("maxDirectsPerChannel").and_then(|m| m.as_u64()) {
            config.server.max_directs_per_channel = md_pc as u32;
        }
        if let Some(mu) = srv.get("maxUpstreamRate").and_then(|m| m.as_u64()) {
            config.server.max_upstream_rate = mu as u32;
        }
        if let Some(mu6) = srv.get("maxUpstreamRateIPv6").and_then(|m| m.as_u64()) {
            config.server.max_upstream_rate_ipv6 = mu6 as u32;
        }
        if let Some(ra) = srv.get("allowRemoteAdmin").and_then(|a| a.as_bool()) {
            config.server.allow_remote_admin = ra;
        }
        if let Some(token) = srv.get("authToken").and_then(|t| t.as_str()) {
            config.server.auth_token = Some(token.to_string());
        }
    } else {
        // Flat legacy fields (Station AppSettingsReader compatibility)
        if let Some(port) = v1.get("port").and_then(|p| p.as_u64()) {
            config.server.port = port as u16;
        }
        if let Some(bind) = v1.get("bindAddress").and_then(|b| b.as_str()) {
            config.server.bind_address = bind.to_string();
        }
    }

    // Map logging fields if present
    if let Some(log) = v1.get("logging") {
        if let Some(lvl) = log.get("level").and_then(|l| l.as_str()) {
            config.logging.level = lvl.to_string();
        }
        if let Some(mb) = log.get("maxLogBytes").and_then(|m| m.as_u64()) {
            config.logging.max_log_bytes = mb as usize;
        }
    }

    // Map lifecycle fields if present
    if let Some(life) = v1.get("lifecycle") {
        if let Some(hl) = life.get("headless").and_then(|h| h.as_bool()) {
            config.lifecycle.headless = hl;
        }
        if let Some(to) = life.get("shutdownTimeoutSecs").and_then(|t| t.as_u64()) {
            config.lifecycle.shutdown_timeout_secs = to;
        }
    }

    config.schema_version = CURRENT_SCHEMA_VERSION;
    Ok(config)
}
