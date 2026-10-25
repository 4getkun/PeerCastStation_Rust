use std::time::Duration;

use peercast_app::{AppConfig, ConfigManager};
use peercast_desktop::{
    backend::DesktopBackend, broadcast::BroadcastRequest, preferences::DesktopPreferences,
};
use tokio::net::TcpListener;

async fn free_loopback_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn broadcast_request(ingest_port: u16) -> BroadcastRequest {
    BroadcastRequest {
        name: "Archive stream 2026".into(),
        genre: "Talk / Research".into(),
        description: "Saved broadcast form metadata".into(),
        comment: "Local test only".into(),
        contact_url: "https://example.invalid/contact".into(),
        content_type: "WMV".into(),
        bitrate: 384,
        source_mode: "push".into(),
        source_uri: "http://127.0.0.1/unused-source".into(),
        ingest_port,
        relay_port: 0,
        announce_sp: false,
    }
}

fn assert_request_eq(actual: &BroadcastRequest, expected: &BroadcastRequest) {
    assert_eq!(actual.name, expected.name);
    assert_eq!(actual.genre, expected.genre);
    assert_eq!(actual.description, expected.description);
    assert_eq!(actual.comment, expected.comment);
    assert_eq!(actual.contact_url, expected.contact_url);
    assert_eq!(actual.content_type, expected.content_type);
    assert_eq!(actual.bitrate, expected.bitrate);
    assert_eq!(actual.source_mode, expected.source_mode);
    assert_eq!(actual.source_uri, expected.source_uri);
    assert_eq!(actual.ingest_port, expected.ingest_port);
    assert_eq!(actual.relay_port, expected.relay_port);
    assert_eq!(actual.announce_sp, expected.announce_sp);
}

async fn close(backend: &DesktopBackend) {
    tokio::time::timeout(Duration::from_secs(8), backend.close())
        .await
        .expect("backend close timed out")
        .expect("backend close failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_broadcast_persists_without_auto_start_and_failed_start_keeps_saved_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let ingest_port = free_loopback_port().await;
    let expected = broadcast_request(ingest_port);

    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    let started = backend.start_broadcast(expected.clone()).await.unwrap();
    assert!(started
        .broadcasts
        .iter()
        .any(|broadcast| broadcast.name == expected.name));
    assert_request_eq(
        backend
            .snapshot()
            .await
            .unwrap()
            .desktop
            .last_broadcast
            .as_ref()
            .unwrap(),
        &expected,
    );
    close(&backend).await;

    let reopened = DesktopBackend::open(path.clone()).await.unwrap();
    let snapshot = reopened.snapshot().await.unwrap();
    assert_request_eq(snapshot.desktop.last_broadcast.as_ref().unwrap(), &expected);
    assert!(
        snapshot.broadcasts.is_empty(),
        "saved settings must not auto-start"
    );

    let occupied_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let occupied_port = occupied_listener.local_addr().unwrap().port();
    let failed_request = broadcast_request(occupied_port);
    assert!(reopened.start_broadcast(failed_request).await.is_err());
    let after_failure = reopened.snapshot().await.unwrap();
    assert_request_eq(
        after_failure.desktop.last_broadcast.as_ref().unwrap(),
        &expected,
    );
    assert!(after_failure.broadcasts.is_empty());
    drop(occupied_listener);
    close(&reopened).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_desktop_preferences_without_last_broadcast_load_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy-desktop.json");
    let mut config = AppConfig::default();
    let mut desktop = serde_json::to_value(DesktopPreferences::default()).unwrap();
    desktop.as_object_mut().unwrap().remove("lastBroadcast");
    config.desktop = desktop;
    ConfigManager::new(&path).save(&config).unwrap();

    let backend = DesktopBackend::open(path).await.unwrap();
    let snapshot = backend.snapshot().await.unwrap();
    assert!(snapshot.desktop.last_broadcast.is_none());
    assert!(snapshot.broadcasts.is_empty());
    close(&backend).await;
}
