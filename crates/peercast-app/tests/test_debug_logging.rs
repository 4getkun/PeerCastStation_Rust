use peercast_app::config::LogConfig;
use peercast_app::logging::{BoundedLogBuffer, LogLevel};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_base(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join(format!(
            "peercast-debug-logging-{}-{nonce}",
            std::process::id()
        ))
        .join(label)
}

fn config(path: PathBuf, max_log_bytes: usize, max_backups: usize) -> LogConfig {
    LogConfig {
        level: "trace".to_owned(),
        max_log_bytes,
        max_backups,
        max_in_memory_entries: 2048,
        log_file: Some(path.to_string_lossy().into_owned()),
    }
}

fn json_lines(path: &PathBuf) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn runtime_file_failure_is_reported_and_memory_logging_continues() {
    let path = temp_base("file-failure.log");
    let settings = config(path.clone(), 1024, 1);
    let logger = BoundedLogBuffer::from_config(&settings, &["PrivateValue"]).unwrap();
    let backup = PathBuf::from(format!("{}.1", path.display()));
    fs::create_dir(&backup).unwrap();
    for _ in 0..4 {
        logger.log(
            LogLevel::Info,
            "fixture",
            &"PrivateValue content ".repeat(40),
        );
    }
    logger.log(LogLevel::Warn, "fixture", "memory still available");
    let entries = logger.snapshot();
    assert!(entries.iter().any(|entry| entry.target == "logging"
        && entry
            .message
            .contains("File logging disabled after write failure")));
    assert!(entries
        .iter()
        .any(|entry| entry.message == "memory still available"));
    assert!(entries
        .iter()
        .all(|entry| !entry.message.contains("PrivateValue")));
    drop(logger);
    fs::remove_dir(backup).unwrap();
    fs::remove_file(&path).unwrap();
    fs::remove_dir(path.parent().unwrap()).unwrap();
}

#[test]
fn configured_level_filters_memory_and_file_and_redacts_all_fields() {
    let path = temp_base("nested/app.log");
    let mut settings = config(path.clone(), 4096, 1);
    settings.level = "WARN".to_owned();
    let logger = BoundedLogBuffer::from_config(&settings, &["KnownSecret"])
        .expect("configured logger should open its file");
    assert_eq!(logger.minimum_level(), LogLevel::Warn);

    logger.log(LogLevel::Info, "filtered", "filtered message");
    logger.log(LogLevel::Warn, "KnownSecret-target", "KnownSecret message");
    logger.log_with_secrets(
        LogLevel::Error,
        "explicit-secret-target",
        "explicit-secret-message",
        &["explicit-secret"],
    );

    let entries = logger.snapshot();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| entry.level >= LogLevel::Warn));
    assert!(!entries.iter().any(|entry| {
        entry.target.contains("KnownSecret") || entry.message.contains("KnownSecret")
    }));
    assert!(!entries.iter().any(|entry| {
        entry.target.contains("explicit-secret") || entry.message.contains("explicit-secret")
    }));
    let file = fs::read_to_string(&path).unwrap();
    assert!(!file.contains("filtered message"));
    assert!(!file.contains("KnownSecret"));
    assert!(!file.contains("explicit-secret"));
    assert!(file.contains("[REDACTED]"));
    assert_eq!(json_lines(&path).len(), 2);
    assert_eq!(serde_json::to_value(&entries[0]).unwrap()["level"], "WARN");
    logger.set_minimum_level(LogLevel::Error);
    assert_eq!(logger.minimum_level(), LogLevel::Error);
    logger.log(LogLevel::Warn, "test", "filtered after level update");
    assert_eq!(logger.len(), 2);

    let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
}

#[test]
fn stderr_sink_obeys_configured_level() {
    const CHILD_ENV: &str = "PEERCAST_DEBUG_LOG_STDERR_CHILD";
    if std::env::var_os(CHILD_ENV).is_some() {
        let settings = LogConfig {
            level: "error".to_owned(),
            log_file: None,
            ..LogConfig::default()
        };
        let logger = BoundedLogBuffer::from_config(&settings, &["stderr-known-secret"]).unwrap();
        logger.log(LogLevel::Warn, "test", "stderr-filtered-marker");
        logger.log(
            LogLevel::Error,
            "stderr-known-secret-target",
            "stderr-allowed-marker stderr-known-secret",
        );
        return;
    }

    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "stderr_sink_obeys_configured_level",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .output()
        .expect("test subprocess should launch");
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("stderr-filtered-marker"));
    assert!(!stderr.contains("stderr-known-secret"));
    assert!(stderr.contains("[REDACTED]"));
    assert!(stderr.contains("stderr-allowed-marker"));
}

#[test]
fn rotating_files_respect_size_and_backup_count_including_zero() {
    let directory = temp_base("rotation");
    let path = directory.join("app.log");
    let logger = BoundedLogBuffer::from_config(&config(path.clone(), 1024, 2), &[]).unwrap();
    for index in 0..12 {
        logger.log(
            LogLevel::Info,
            "rotate",
            &format!("{index}:{}", "x".repeat(500)),
        );
    }
    for candidate in [
        &path,
        &path.with_extension("log.1"),
        &path.with_extension("log.2"),
    ] {
        assert!(candidate.exists(), "expected rotation file {candidate:?}");
        let bytes = fs::read(candidate).unwrap();
        assert!(bytes.len() <= 1024);
        for line in bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            serde_json::from_slice::<Value>(line).expect("each rotated line remains valid JSON");
        }
    }
    assert!(!path.with_extension("log.3").exists());

    let zero_backup_path = directory.join("zero.log");
    let zero_logger =
        BoundedLogBuffer::from_config(&config(zero_backup_path.clone(), 1024, 0), &[]).unwrap();
    for index in 0..5 {
        zero_logger.log(
            LogLevel::Info,
            "rotate",
            &format!("{index}:{}", "y".repeat(500)),
        );
    }
    assert!(fs::metadata(&zero_backup_path).unwrap().len() <= 1024);
    assert!(!zero_backup_path.with_extension("log.1").exists());
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn oversized_utf8_message_is_truncated_to_valid_bounded_json() {
    let path = temp_base("oversized.log");
    let logger = BoundedLogBuffer::from_config(&config(path.clone(), 1024, 1), &[]).unwrap();
    logger.log(LogLevel::Error, "oversized", &"配信😀".repeat(10_000));
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.len() <= 1024);
    let entry: Value =
        serde_json::from_slice(&bytes).expect("truncated record should be valid JSON");
    let message = entry["message"].as_str().unwrap();
    assert!(message.ends_with("…[truncated]"));
    assert!(message.is_char_boundary(message.len()));
    let _ = fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn parallel_writers_emit_complete_json_lines() {
    let path = temp_base("parallel.log");
    let logger = std::sync::Arc::new(
        BoundedLogBuffer::from_config(&config(path.clone(), 1024 * 1024, 1), &[]).unwrap(),
    );
    let workers: Vec<_> = (0..8)
        .map(|worker| {
            let logger = std::sync::Arc::clone(&logger);
            std::thread::spawn(move || {
                for item in 0..100 {
                    logger.log(LogLevel::Debug, "parallel", &format!("{worker}:{item}"));
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let records = json_lines(&path);
    assert_eq!(records.len(), 800);
    assert_eq!(logger.snapshot().len(), 800);
    let _ = fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn legacy_constructor_remains_memory_only_and_accepts_trace() {
    let logger = BoundedLogBuffer::new(2);
    assert_eq!(logger.minimum_level(), LogLevel::Trace);
    logger.log(LogLevel::Trace, "legacy", "trace message");
    assert_eq!(logger.snapshot()[0].message, "trace message");
}

#[test]
fn off_and_none_levels_disable_logging_and_off_records_are_discarded() {
    assert_eq!(LogLevel::from_config("off").unwrap(), LogLevel::Off);
    assert_eq!(LogLevel::from_config("none").unwrap().as_str(), "OFF");
    let settings = LogConfig {
        level: "none".to_owned(),
        log_file: None,
        ..LogConfig::default()
    };
    let logger = BoundedLogBuffer::from_config(&settings, &[]).unwrap();
    logger.log(LogLevel::Error, "disabled", "should be filtered");
    assert!(logger.is_empty());

    let legacy = BoundedLogBuffer::new(2);
    legacy.log(LogLevel::Off, "disabled", "off record is never stored");
    assert!(legacy.is_empty());
}

#[test]
fn file_creation_errors_are_returned_without_panicking() {
    let blocked_path = temp_base("file-blocker");
    fs::create_dir_all(blocked_path.parent().unwrap()).unwrap();
    fs::write(&blocked_path, b"not a directory").unwrap();
    let invalid = blocked_path.join("app.log");
    let result = BoundedLogBuffer::from_config(&config(invalid, 1024, 1), &[]);
    assert!(result.is_err(), "file sink setup errors should be returned");
    let _ = fs::remove_dir_all(blocked_path.parent().unwrap());
}
