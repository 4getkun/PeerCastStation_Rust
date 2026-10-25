use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use peercast_app::{
    AppCliArgs, AppConfig, AppLifecycleManager, AppMetrics, BoundedLogBuffer, ConfigManager,
    ConfigPersistenceError, ConfigValidationError, LifecyclePhase, LogLevel, PlatformSignalSupport,
    ShutdownReason, CURRENT_SCHEMA_VERSION, DEFAULT_BIND_ADDRESS, DEFAULT_PORT,
};
use peercast_rpc::ServentSettings;

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(100);

fn create_test_dir(name: &str) -> PathBuf {
    let pid = std::process::id();
    let counter = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("peercast_test_{}_{}_{}", name, pid, counter));
    fs::create_dir_all(&dir).expect("failed to create test temp dir");
    dir
}

/// Acceptance criterion 1:
/// Configuration validation and invariants (REQ-067, REQ-065).
#[test]
fn test_p40_config_validation_and_invariants() {
    // 1. Valid default config
    let mut config = AppConfig::default();
    assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(config.server.port, DEFAULT_PORT);
    assert_eq!(config.server.bind_address, DEFAULT_BIND_ADDRESS);
    assert!(config.validate().is_ok());

    // 2. Port cannot be zero
    config.server.port = 0;
    assert_eq!(config.validate(), Err(ConfigValidationError::InvalidPort));
    config.server.port = DEFAULT_PORT;

    // 3. Invalid IP address
    config.server.bind_address = "not-an-ip".to_string();
    assert_eq!(
        config.validate(),
        Err(ConfigValidationError::InvalidBindAddress(
            "not-an-ip".to_string()
        ))
    );
    config.server.bind_address = DEFAULT_BIND_ADDRESS.to_string();

    // 4. max_relays < max_relays_per_channel
    config.server.max_relays = 2;
    config.server.max_relays_per_channel = 5;
    assert_eq!(
        config.validate(),
        Err(ConfigValidationError::MaxRelaysLessThanPerChannel(2, 5))
    );
    config.server.max_relays = 16;
    config.server.max_relays_per_channel = 4;

    // 5. max_directs < max_directs_per_channel
    config.server.max_directs = 1;
    config.server.max_directs_per_channel = 3;
    assert_eq!(
        config.validate(),
        Err(ConfigValidationError::MaxDirectsLessThanPerChannel(1, 3))
    );
    config.server.max_directs = 8;
    config.server.max_directs_per_channel = 2;

    // 6. Zero upstream rate
    config.server.max_upstream_rate = 0;
    assert_eq!(
        config.validate(),
        Err(ConfigValidationError::ZeroUpstreamRate)
    );
    config.server.max_upstream_rate = 10000;

    // 7. Insecure remote admin (enabled without token or token < 8 chars)
    config.server.allow_remote_admin = true;
    config.server.auth_token = None;
    assert_eq!(
        config.validate(),
        Err(ConfigValidationError::InsecureRemoteAdminToken)
    );

    config.server.auth_token = Some("short".to_string());
    assert_eq!(
        config.validate(),
        Err(ConfigValidationError::InsecureRemoteAdminToken)
    );

    config.server.auth_token = Some("secure_secret_token_123".to_string());
    assert!(config.validate().is_ok());
    config.server.allow_remote_admin = false;
    config.server.auth_token = None;

    // 8. Invalid log level
    config.logging.level = "invalid_level".to_string();
    assert_eq!(
        config.validate(),
        Err(ConfigValidationError::InvalidLogLevel(
            "invalid_level".to_string()
        ))
    );
    config.logging.level = "info".to_string();

    // 9. ServentSettings conversion parity (REQ-064)
    let servent = config.to_servent_settings();
    assert_eq!(servent.max_relays, 16);
    assert_eq!(servent.max_directs, 8);

    let updated = ServentSettings {
        max_relays: 32,
        max_relays_per_channel: 8,
        max_directs: 16,
        max_directs_per_channel: 4,
        max_upstream_rate: 20000,
        max_upstream_rate_ipv6: 20000,
    };
    config.apply_servent_settings(&updated);
    assert_eq!(config.server.max_relays, 32);
    assert_eq!(config.server.max_upstream_rate, 20000);
}

/// Acceptance criterion 2:
/// Atomic persistence and resilience against interrupted writes (REQ-067).
#[test]
fn test_p40_atomic_write_and_interrupted_write_resilience() {
    let dir = create_test_dir("atomic");
    let config_path = dir.join("peercast.json");
    let mgr = ConfigManager::new(&config_path);

    // Initial save
    let mut initial_cfg = AppConfig::default();
    initial_cfg.server.port = 7150;
    mgr.save(&initial_cfg).expect("save should succeed");

    // Verify file exists and content matches
    let loaded = mgr.load().expect("load should succeed");
    assert_eq!(loaded.server.port, 7150);

    // Simulate an interrupted write: create a stray .tmp file left behind by interrupted process
    let stray_tmp = dir.join("peercast.json.tmp.999_1");
    let mut f = File::create(&stray_tmp).unwrap();
    f.write_all(b"partial-data").unwrap();
    drop(f);

    // Verify main configuration file is completely unaffected by stray tmp file
    let loaded_after_interrupted = mgr.load().expect("load must still succeed");
    assert_eq!(loaded_after_interrupted.server.port, 7150);

    // Clean up
    let _ = fs::remove_file(stray_tmp);
    let _ = fs::remove_dir_all(dir);
}

/// Acceptance criterion 3:
/// Safe migration and backup preservation (REQ-067, stop/block guard).
#[test]
fn test_p40_safe_migration_and_backup_preservation() {
    let dir = create_test_dir("migrate");
    let config_path = dir.join("peercast.json");

    // Write a legacy v1 configuration file
    let legacy_v1_json = r#"{
        "schemaVersion": 1,
        "server": {
            "port": 7145,
            "bindAddress": "127.0.0.1",
            "maxRelays": 24,
            "maxRelaysPerChannel": 6,
            "maxDirects": 12,
            "maxDirectsPerChannel": 3,
            "maxUpstreamRate": 15000,
            "maxUpstreamRateIPv6": 15000
        },
        "logging": {
            "level": "warn",
            "maxLogBytes": 2097152
        }
    }"#;
    fs::write(&config_path, legacy_v1_json).expect("failed to write legacy config");

    let mgr = ConfigManager::new(&config_path);
    let migrated = mgr.load().expect("migration should succeed");

    // Verify migrated data
    assert_eq!(migrated.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(migrated.server.port, 7145);
    assert_eq!(migrated.server.max_relays, 24);
    assert_eq!(migrated.logging.level, "warn");
    assert_eq!(migrated.logging.max_log_bytes, 2097152);
    // Newly introduced schema fields have sensible defaults
    assert_eq!(migrated.lifecycle.shutdown_timeout_secs, 5);

    // Stop/block guard: Verify backup file was created and is non-empty
    let mut backup_found = false;
    for entry in fs::read_dir(&dir).unwrap() {
        let entry = entry.unwrap();
        let fname = entry.file_name().to_string_lossy().to_string();
        if fname.starts_with("peercast.json.bak.") && fname.ends_with("_v1") {
            backup_found = true;
            let backup_content = fs::read_to_string(entry.path()).unwrap();
            assert_eq!(
                backup_content, legacy_v1_json,
                "backup must preserve exact original bytes"
            );
        }
    }
    assert!(backup_found, "migration backup file must exist on disk");

    let _ = fs::remove_dir_all(dir);
}

/// Acceptance criterion 4:
/// Corrupted configuration file rejection (REQ-067).
#[test]
fn test_p40_corrupt_config_rejection() {
    let dir = create_test_dir("corrupt");
    let config_path = dir.join("peercast.json");

    // Truncated/corrupt JSON
    fs::write(&config_path, b"{\"server\": { \"port\": ").unwrap();

    let mgr = ConfigManager::new(&config_path);
    let res = mgr.load();
    assert!(
        matches!(res, Err(ConfigPersistenceError::Json(_))),
        "corrupt JSON must return Json error"
    );

    // Verify file content was not overwritten with default config
    let raw = fs::read(&config_path).unwrap();
    assert_eq!(raw, b"{\"server\": { \"port\": ");

    let _ = fs::remove_dir_all(dir);
}

/// Acceptance criterion 5:
/// CLI arguments parsing, short flags, and configuration merging (REQ-067).
#[test]
fn test_p40_cli_flags_parsing_and_precedence() {
    // 1. Long flags
    let args = [
        "peercast-station",
        "--port",
        "7155",
        "--bind",
        "0.0.0.0",
        "--config",
        "/custom/path.json",
        "--headless",
        "--multi",
    ];
    let cli = AppCliArgs::parse_from(args).expect("parse long flags should succeed");
    assert_eq!(cli.port, Some(7155));
    assert_eq!(cli.bind_address.as_deref(), Some("0.0.0.0"));
    assert_eq!(cli.config_path, Some(PathBuf::from("/custom/path.json")));
    assert!(cli.headless);
    assert!(cli.multi);
    assert!(!cli.kill);

    // 2. Short flags & Station aliases
    let short_args = [
        "peercast-station",
        "-p",
        "7160",
        "-b",
        "192.168.1.10",
        "-s",
        "/etc/peercast.json",
        "-kill",
    ];
    let short_cli = AppCliArgs::parse_from(short_args).expect("parse short flags should succeed");
    assert_eq!(short_cli.port, Some(7160));
    assert_eq!(short_cli.bind_address.as_deref(), Some("192.168.1.10"));
    assert_eq!(
        short_cli.config_path,
        Some(PathBuf::from("/etc/peercast.json"))
    );
    assert!(short_cli.kill);

    // 3. CLI takes precedence over configuration file
    let mut config = AppConfig::default();
    assert_eq!(config.server.port, 7144);
    assert!(!config.lifecycle.headless);

    cli.apply_to_config(&mut config);
    assert_eq!(config.server.port, 7155);
    assert_eq!(config.server.bind_address, "0.0.0.0");
    assert!(config.lifecycle.headless);

    // 4. Invalid arguments
    assert!(AppCliArgs::parse_from(["bin", "--port"]).is_err());
    assert!(AppCliArgs::parse_from(["bin", "--port", "not_a_num"]).is_err());
    assert!(AppCliArgs::parse_from(["bin", "--unknown-flag"]).is_err());
}

/// Acceptance criterion 6:
/// Explicit lifecycle state machine and early-return re-entrancy guard (Global Rule 2).
#[tokio::test]
async fn test_p40_lifecycle_state_machine_and_reentrancy_guard() {
    let lifecycle = AppLifecycleManager::new();

    // 1. Initial state
    assert_eq!(lifecycle.phase().await, LifecyclePhase::Initializing);

    // 2. Transition to Running
    assert!(lifecycle.mark_running().await);
    assert_eq!(lifecycle.phase().await, LifecyclePhase::Running);
    // Double mark_running is idempotent / ignored
    assert!(!lifecycle.mark_running().await);

    // 3. First shutdown request initiates transition
    let first = lifecycle.request_shutdown(ShutdownReason::AdminRpc).await;
    assert!(first, "first shutdown call must return true");
    assert_eq!(lifecycle.phase().await, LifecyclePhase::ShuttingDown);

    // 4. Re-entrancy guard: subsequent calls return false immediately
    let duplicate = lifecycle.request_shutdown(ShutdownReason::IpcKill).await;
    assert!(
        !duplicate,
        "duplicate shutdown call must be rejected by re-entrancy guard"
    );

    // 5. Complete shutdown transitions to Stopped
    let res = lifecycle
        .complete_shutdown(Duration::from_millis(100))
        .await;
    assert!(res.is_ok());
    assert_eq!(lifecycle.phase().await, LifecyclePhase::Stopped);

    // 6. Shutdown call while Stopped returns false
    let after_stopped = lifecycle.request_shutdown(ShutdownReason::AdminRpc).await;
    assert!(
        !after_stopped,
        "shutdown call on Stopped state must return false"
    );
}

/// Acceptance criterion 7:
/// Graceful shutdown during active traffic and task drain verification (REQ-014, REQ-067).
/// Stop/block guard: Tasks must NOT survive shutdown.
#[tokio::test]
async fn test_p40_graceful_shutdown_during_active_traffic() {
    let lifecycle = AppLifecycleManager::new();
    lifecycle.mark_running().await;

    let tracker = lifecycle.task_tracker();
    let shutdown_rx = lifecycle.subscribe_shutdown();

    let task_count = 5;
    let completed_tasks = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    // Spawn 5 active worker tasks simulating network connections
    for _ in 0..task_count {
        let mut rx = shutdown_rx.resubscribe();
        let completed = Arc::clone(&completed_tasks);
        tracker.spawn(async move {
            // Task loops until shutdown signal arrives
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                _ = rx.recv() => {
                    // Clean cancellation drain
                    completed.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
    }

    // Verify tasks are running and tracked
    assert_eq!(tracker.active_tasks(), task_count);

    // Trigger shutdown
    lifecycle
        .request_shutdown(ShutdownReason::OsSignal("SIGINT".to_string()))
        .await;

    // Drain tasks with timeout
    let drain_result = lifecycle.complete_shutdown(Duration::from_secs(2)).await;
    assert!(drain_result.is_ok(), "all tasks must drain within timeout");

    // Stop/block guard: Verify zero tasks survived shutdown
    assert_eq!(
        tracker.active_tasks(),
        0,
        "active task count must be strictly 0 after shutdown"
    );
    assert_eq!(
        completed_tasks.load(Ordering::SeqCst),
        task_count,
        "all tasks must have acknowledged shutdown"
    );
}

/// Acceptance criterion 8:
/// Bounded logging buffer with FIFO eviction and secret redaction (Global Rule 3, REQ-065, REQ-067).
#[test]
fn test_p40_bounded_logging_and_secret_redaction() {
    // 1. Capacity limit and FIFO sliding window eviction
    let buffer = BoundedLogBuffer::new(4);
    assert_eq!(buffer.capacity(), 4);
    assert!(buffer.is_empty());

    for i in 1..=6 {
        buffer.log(LogLevel::Info, "test", &format!("msg-{}", i));
    }

    assert_eq!(buffer.len(), 4, "buffer must strictly obey capacity limit");
    let snapshot = buffer.snapshot();
    let messages: Vec<&str> = snapshot.iter().map(|e| e.message.as_str()).collect();
    // Oldest messages (msg-1, msg-2) were evicted; msg-3..msg-6 retained
    assert_eq!(messages, vec!["msg-3", "msg-4", "msg-5", "msg-6"]);

    // 2. Secret redaction in logs (REQ-065)
    let secret_buffer = BoundedLogBuffer::new(10);
    secret_buffer.log(
        LogLevel::Warn,
        "security",
        "Connection from client with http://127.0.0.1:7144/stream?token=MySecretAuthToken123&foo=bar",
    );
    secret_buffer.log(
        LogLevel::Info,
        "auth",
        "Incoming request with header Authorization: Bearer SuperSecretBearerXYZ",
    );
    secret_buffer.log_with_secrets(
        LogLevel::Info,
        "admin",
        "Admin password CustomMasterKey entered",
        &["CustomMasterKey"],
    );

    let entries = secret_buffer.snapshot();
    assert_eq!(entries.len(), 3);
    assert!(entries[0].message.contains("?token=[REDACTED]"));
    assert!(!entries[0].message.contains("MySecretAuthToken123"));

    assert!(entries[1].message.contains("Bearer [REDACTED]"));
    assert!(!entries[1].message.contains("SuperSecretBearerXYZ"));

    assert!(entries[2].message.contains("[REDACTED]"));
    assert!(!entries[2].message.contains("CustomMasterKey"));
}

/// Acceptance criterion 9:
/// Operational metrics tracking and connection lifetime management (REQ-067, REQ-014).
#[test]
fn test_p40_operational_metrics_tracking() {
    let metrics = AppMetrics::new();

    // 1. Connection guards decrement on drop
    {
        let conn1 = metrics.track_connection();
        let _conn2 = metrics.track_connection();
        let snap = metrics.snapshot(2);
        assert_eq!(snap.active_connections, 2);
        assert_eq!(snap.active_tasks, 2);
        drop(conn1);
    }
    let snap_after = metrics.snapshot(0);
    assert_eq!(snap_after.active_connections, 0);

    // 2. Channels and byte counters
    metrics.inc_channels();
    metrics.inc_channels();
    metrics.dec_channels();
    metrics.add_bytes_sent(1024);
    metrics.add_bytes_received(2048);

    let final_snap = metrics.snapshot(1);
    assert_eq!(final_snap.active_channels, 1);
    assert_eq!(final_snap.bytes_sent, 1024);
    assert_eq!(final_snap.bytes_received, 2048);
}

/// Acceptance criterion 10:
/// OS-specific shutdown contracts (Windows and Linux recorded separately) (REQ-067).
#[test]
fn test_p40_os_specific_shutdown_contracts() {
    let support = PlatformSignalSupport::current();

    if cfg!(windows) {
        assert_eq!(support.os, "windows");
        assert!(support.supported_signals.contains(&"Ctrl+C"));
        assert!(support.supported_signals.contains(&"Ctrl+Break"));
        assert!(support.supported_signals.contains(&"Close"));
    } else if cfg!(unix) {
        assert_eq!(support.os, "unix");
        assert!(support.supported_signals.contains(&"SIGINT"));
        assert!(support.supported_signals.contains(&"SIGTERM"));
    }
}
