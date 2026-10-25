use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use peercast_app::runtime_config::{ChannelConfig, ChannelMetadata, ChannelSource};
use peercast_app::{AppConfig, ConfigManager, ConfigValidationError};
use peercast_core::content::{Content, ContinuationFlags};
use peercast_core::ringbuffer::{DEFAULT_RINGBUFFER_MAX_BYTES, DEFAULT_RINGBUFFER_TIME_WINDOW};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(1);

fn test_config_path() -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "peercast_runtime_config_{}_{}",
        std::process::id(),
        TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("create temporary config directory");
    (dir.join("peercast.json"), dir)
}

#[test]
fn missing_new_fields_load_with_legacy_defaults_and_saved_values_round_trip() {
    let (path, dir) = test_config_path();
    let manager = ConfigManager::new(&path);

    let mut old_v2 = serde_json::to_value(AppConfig::default()).unwrap();
    let server = old_v2["server"].as_object_mut().unwrap();
    server.remove("maxRequestBufferBytes");
    server.remove("requestTimeoutMs");
    server.remove("maxRpcRequestBytes");
    server.remove("ringbufferMaxBytes");
    server.remove("ringbufferWindowSecs");
    old_v2["logging"]
        .as_object_mut()
        .unwrap()
        .remove("maxInMemoryEntries");
    old_v2.as_object_mut().unwrap().remove("runtime");
    fs::write(&path, serde_json::to_vec_pretty(&old_v2).unwrap()).unwrap();

    let loaded = manager.load().unwrap();
    assert_eq!(loaded.server.max_request_buffer_bytes, 8192);
    assert_eq!(loaded.server.request_timeout_ms, 5000);
    assert_eq!(
        loaded.server.max_rpc_request_bytes,
        peercast_rpc::server::DEFAULT_MAX_RPC_REQUEST_BYTES
    );
    assert_eq!(
        loaded.server.ringbuffer_max_bytes,
        DEFAULT_RINGBUFFER_MAX_BYTES
    );
    assert_eq!(
        loaded.server.ringbuffer_window_secs,
        DEFAULT_RINGBUFFER_TIME_WINDOW.as_secs()
    );
    assert_eq!(loaded.logging.max_in_memory_entries, 1000);
    assert_eq!(loaded.runtime, Default::default());

    let mut changed = loaded;
    changed.server.max_request_buffer_bytes = 16_384;
    changed.server.request_timeout_ms = 12_500;
    changed.server.max_rpc_request_bytes = 131_072;
    changed.server.ringbuffer_max_bytes = 2 * 1024 * 1024;
    changed.server.ringbuffer_window_secs = 7;
    changed.logging.max_in_memory_entries = 250;
    manager.save(&changed).unwrap();
    let restored = manager.load().unwrap();
    assert_eq!(restored.server.max_request_buffer_bytes, 16_384);
    assert_eq!(restored.server.request_timeout_ms, 12_500);
    assert_eq!(restored.server.max_rpc_request_bytes, 131_072);
    assert_eq!(restored.server.ringbuffer_max_bytes, 2 * 1024 * 1024);
    assert_eq!(restored.server.ringbuffer_window_secs, 7);
    assert_eq!(restored.logging.max_in_memory_entries, 250);

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn every_new_capacity_or_window_setting_rejects_zero_with_its_field_name() {
    type ZeroValueCase = (&'static str, fn(&mut AppConfig));
    let cases: [ZeroValueCase; 6] = [
        ("server.maxRequestBufferBytes", |c| {
            c.server.max_request_buffer_bytes = 0
        }),
        ("server.requestTimeoutMs", |c| {
            c.server.request_timeout_ms = 0
        }),
        ("server.maxRpcRequestBytes", |c| {
            c.server.max_rpc_request_bytes = 0
        }),
        ("server.ringbufferMaxBytes", |c| {
            c.server.ringbuffer_max_bytes = 0
        }),
        ("server.ringbufferWindowSecs", |c| {
            c.server.ringbuffer_window_secs = 0
        }),
        ("logging.maxInMemoryEntries", |c| {
            c.logging.max_in_memory_entries = 0
        }),
    ];

    for (field, invalidate) in cases {
        let mut config = AppConfig::default();
        invalidate(&mut config);
        assert_eq!(
            config.validate(),
            Err(ConfigValidationError::ZeroConfigValue(field))
        );
    }
}

#[test]
fn configured_content_ring_buffer_applies_byte_and_time_eviction() {
    let mut config = AppConfig::default();
    config.server.ringbuffer_max_bytes = 5;
    config.server.ringbuffer_window_secs = 10;
    let mut by_bytes = config.create_content_ring_buffer();
    by_bytes.push(Content::new(
        0,
        Duration::ZERO,
        0,
        b"abc".to_vec(),
        ContinuationFlags::NONE,
    ));
    by_bytes.push(Content::new(
        0,
        Duration::from_secs(1),
        3,
        b"def".to_vec(),
        ContinuationFlags::NONE,
    ));
    assert_eq!(by_bytes.len(), 1);
    assert_eq!(by_bytes.oldest().unwrap().pos, 3);

    config.server.ringbuffer_max_bytes = 100;
    config.server.ringbuffer_window_secs = 1;
    let mut by_time = config.create_content_ring_buffer();
    by_time.push(Content::new(
        0,
        Duration::ZERO,
        0,
        b"old".to_vec(),
        ContinuationFlags::NONE,
    ));
    by_time.push(Content::new(
        0,
        Duration::from_secs(2),
        3,
        b"new".to_vec(),
        ContinuationFlags::NONE,
    ));
    assert_eq!(by_time.len(), 1);
    assert_eq!(by_time.oldest().unwrap().pos, 3);
}

#[test]
fn runtime_settings_round_trip_with_camel_case_and_validate() {
    let mut config = AppConfig::default();
    config.runtime.channels.push(ChannelConfig {
        channel_id: "0123456789abcdef0123456789abcdef".to_string(),
        name: "test".to_string(),
        source: ChannelSource::HttpPush {
            bind_address: "127.0.0.1:7145".parse().unwrap(),
            auth_token: Some("token".to_string()),
        },
        ..ChannelConfig::default()
    });
    config.runtime.streaming.safe_start_timeout_ms = 4200;

    config.validate().unwrap();
    let value = serde_json::to_value(&config).unwrap();
    assert_eq!(value["runtime"]["streaming"]["safeStartTimeoutMs"], 4200);
    assert_eq!(
        value["runtime"]["channels"][0]["source"]["type"],
        "httpPush"
    );
    assert_eq!(
        value["runtime"]["channels"][0]["source"]["bindAddress"],
        "127.0.0.1:7145"
    );
    assert_eq!(serde_json::from_value::<AppConfig>(value).unwrap(), config);
}

#[test]
fn channel_metadata_defaults_when_loading_legacy_channel_config() {
    let mut config = AppConfig::default();
    config.runtime.channels.push(ChannelConfig {
        channel_id: "0123456789abcdef0123456789abcdef".to_string(),
        name: "legacy channel".to_string(),
        ..ChannelConfig::default()
    });
    let mut value = serde_json::to_value(config).unwrap();
    value["runtime"]["channels"][0]
        .as_object_mut()
        .unwrap()
        .remove("metadata");

    let loaded: AppConfig = serde_json::from_value(value).unwrap();
    assert_eq!(
        loaded.runtime.channels[0].metadata,
        ChannelMetadata::default()
    );
}

#[test]
fn runtime_settings_reject_invalid_ids_and_zero_limits() {
    let mut config = AppConfig::default();
    config.runtime.channels.push(ChannelConfig {
        channel_id: "00000000000000000000000000000000".to_string(),
        ..ChannelConfig::default()
    });
    assert!(matches!(
        config.validate(),
        Err(ConfigValidationError::InvalidRuntimeConfig(_))
    ));

    let mut config = AppConfig::default();
    config.runtime.ingest.max_header_bytes = 0;
    assert!(matches!(
        config.validate(),
        Err(ConfigValidationError::InvalidRuntimeConfig(_))
    ));
}

#[test]
fn port_mapping_requires_a_lan_listener_and_a_finite_renewable_lease() {
    let mut config = AppConfig::default();
    assert!(!config.runtime.port_mapping.enabled);
    config.runtime.port_mapping.enabled = true;
    assert!(matches!(
        config.validate(),
        Err(ConfigValidationError::InvalidRuntimeConfig(_))
    ));
    config.server.bind_address = "0.0.0.0".into();
    config.validate().unwrap();
    config.runtime.port_mapping.lease_duration_secs = 0;
    assert!(matches!(
        config.validate(),
        Err(ConfigValidationError::InvalidRuntimeConfig(_))
    ));
    config.runtime.port_mapping.lease_duration_secs =
        config.runtime.port_mapping.renewal_margin_secs;
    assert!(matches!(
        config.validate(),
        Err(ConfigValidationError::InvalidRuntimeConfig(_))
    ));
    config.runtime.port_mapping = Default::default();
    config.runtime.port_mapping.enabled = true;
    config.server.bind_address = "::".into();
    assert!(matches!(
        config.validate(),
        Err(ConfigValidationError::InvalidRuntimeConfig(_))
    ));
}
