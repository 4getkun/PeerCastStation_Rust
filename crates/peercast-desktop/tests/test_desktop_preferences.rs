use peercast_app::ConfigManager;
use peercast_desktop::backend::{DesktopBackend, Limits, YellowPageEntry};
use peercast_desktop::player::PlayerSettings;
use peercast_desktop::preferences::{DesktopPreferences, WindowSettings};
use peercast_desktop::rules::ChannelRule;
use std::fs;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[test]
fn logical_window_size_defaults_to_physical_and_round_trips() {
    let legacy: WindowSettings = serde_json::from_value(serde_json::json!({
        "width": 1486,
        "height": 1059
    }))
    .unwrap();
    assert_eq!(legacy.width, 1486);
    assert_eq!(legacy.height, 1059);
    assert!(!legacy.logical_size);

    let logical = WindowSettings {
        logical_size: true,
        ..WindowSettings::default()
    };
    let round_trip: WindowSettings =
        serde_json::from_value(serde_json::to_value(&logical).unwrap()).unwrap();
    assert_eq!(round_trip, logical);
}

fn limits(directs: u32, relays: u32, upstream: u32) -> Limits {
    Limits {
        max_directs: directs,
        max_relays: relays,
        max_upstream_rate: upstream,
    }
}

fn yp(name: &str, url: &str) -> YellowPageEntry {
    YellowPageEntry {
        name: name.into(),
        channels_url: url.into(),
    }
}

async fn close(backend: &DesktopBackend) {
    tokio::time::timeout(Duration::from_secs(8), backend.close())
        .await
        .expect("backend close timed out")
        .expect("backend close failed");
}

fn rule(id: &str, name: &str, pattern: &str) -> ChannelRule {
    ChannelRule {
        id: id.into(),
        name: name.into(),
        pattern: pattern.into(),
        ..ChannelRule::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_config_without_desktop_preferences_opens_with_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let mut legacy = serde_json::to_value(peercast_app::AppConfig::default()).unwrap();
    legacy.as_object_mut().unwrap().remove("desktop");
    fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    assert_eq!(
        backend.snapshot().await.unwrap().desktop,
        DesktopPreferences::default()
    );
    close(&backend).await;

    let reopened = DesktopBackend::open(path.clone()).await.unwrap();
    assert_eq!(
        reopened.snapshot().await.unwrap().desktop,
        DesktopPreferences::default()
    );
    let persisted = ConfigManager::new(&path).load().unwrap();
    assert_eq!(
        serde_json::from_value::<DesktopPreferences>(persisted.desktop).unwrap(),
        DesktopPreferences::default()
    );
    close(&reopened).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_player_and_rules_survive_limits_and_yp_saves_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let custom_player = dir.path().join("custom-player.exe");
    fs::write(&custom_player, b"fixture").unwrap();
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    let initial = backend.snapshot().await.unwrap();
    let desktop = DesktopPreferences {
        player: PlayerSettings {
            executable: Some(custom_player.display().to_string()),
            arguments: "\"<stream/>\" \"<channelname/>\" \"<direct/>\"".into(),
        },
        channel_rules: vec![rule("favorite", "Favorite", "Rock")],
        ..DesktopPreferences::default()
    };
    let saved = backend
        .apply_desktop_preferences(
            initial.limits.clone(),
            initial.yellow_pages.clone(),
            desktop.clone(),
        )
        .await
        .unwrap();
    assert_eq!(saved.desktop, desktop);
    assert_eq!(saved.player_path, custom_player.display().to_string());
    assert!(saved.player_available);

    backend.apply_limits(limits(10, 9, 24000)).await.unwrap();
    backend
        .apply_yellow_pages(vec![yp("Fixture", "https://example.com/index.txt")])
        .await
        .unwrap();
    let persisted = ConfigManager::new(&path).load().unwrap();
    assert_eq!(
        serde_json::from_value::<DesktopPreferences>(persisted.desktop).unwrap(),
        desktop
    );
    close(&backend).await;

    let reopened = DesktopBackend::open(path).await.unwrap();
    let snapshot = reopened.snapshot().await.unwrap();
    assert_eq!(snapshot.desktop, desktop);
    assert_eq!(snapshot.player_path, custom_player.display().to_string());
    assert!(snapshot.player_available);
    close(&reopened).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_only_save_keeps_the_active_runtime_and_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backend = DesktopBackend::open(path).await.unwrap();
    let initial = backend.snapshot().await.unwrap();
    assert!(
        backend
            .run_probe()
            .await
            .unwrap()
            .last_probe
            .unwrap()
            .passed
    );
    let mut desktop = initial.desktop.clone();
    desktop.channel_rules = vec![rule("rock", "Rock", "Rock")];
    let updated = backend
        .apply_desktop_preferences(initial.limits, initial.yellow_pages, desktop.clone())
        .await
        .unwrap();
    assert!(updated.running);
    assert_eq!(updated.endpoint, initial.endpoint);
    assert_eq!(updated.desktop, desktop);
    assert!(updated.channels.is_empty());
    assert!(
        backend
            .run_probe()
            .await
            .unwrap()
            .last_probe
            .unwrap()
            .passed
    );
    close(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_failure_rolls_back_desktop_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    let before = backend.snapshot().await.unwrap();
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let mut desktop = before.desktop.clone();
    desktop.channel_rules = vec![rule("rock", "Rock", "Rock")];
    assert!(backend
        .apply_desktop_preferences(before.limits.clone(), before.yellow_pages.clone(), desktop,)
        .await
        .is_err());
    let after = backend.snapshot().await.unwrap();
    assert_eq!(after.desktop, before.desktop);
    assert_eq!(after.endpoint, before.endpoint);
    assert_eq!(after.limits, before.limits);
    close(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_rules_are_rejected_without_changing_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    let before = backend.snapshot().await.unwrap();
    let mut desktop = before.desktop.clone();
    let mut invalid = rule("invalid", "Invalid", "[");
    invalid.use_regex = true;
    desktop.channel_rules = vec![invalid];
    let response = backend
        .apply_desktop_preferences(before.limits.clone(), before.yellow_pages.clone(), desktop)
        .await;
    assert!(response.is_err());
    assert_eq!(backend.snapshot().await.unwrap().desktop, before.desktop);
    close(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_player_arguments_are_rejected_and_muted_rules_have_no_sound() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backend = DesktopBackend::open(path).await.unwrap();
    let before = backend.snapshot().await.unwrap();
    let mut notification = rule("notify", "Notification", "Rock");
    notification.sound_path = Some("C:/fixture/alert.wav".into());
    let mut desktop = before.desktop.clone();
    desktop.channel_rules = vec![notification.clone()];
    backend
        .apply_desktop_preferences(
            before.limits.clone(),
            before.yellow_pages.clone(),
            desktop.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        backend.rule_sound_path("notify").await.unwrap().as_deref(),
        Some("C:/fixture/alert.wav")
    );

    notification.mute = true;
    desktop.channel_rules = vec![notification];
    let updated = backend
        .apply_desktop_preferences(
            before.limits.clone(),
            before.yellow_pages.clone(),
            desktop.clone(),
        )
        .await
        .unwrap();
    assert_eq!(backend.rule_sound_path("notify").await.unwrap(), None);

    desktop.player.arguments = "\"unterminated".into();
    assert!(backend
        .apply_desktop_preferences(before.limits, before.yellow_pages, desktop)
        .await
        .is_err());
    assert_eq!(backend.snapshot().await.unwrap().desktop, updated.desktop);
    close(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_preview_uses_cached_raw_rows_and_resolves_priority_flags() {
    const ROCK: &str = "11111111111111111111111111111111";
    const NG: &str = "22222222222222222222222222222222";
    const JAZZ: &str = "33333333333333333333333333333333";
    let directory = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/index.txt", directory.local_addr().unwrap());
    let body = format!(
        "Rock Player<>{ROCK}<>127.0.0.1:7144<>http://example.com<>test<>fixture<>-1<>-1<>128<>RAW<>Artist<>Album<>Title<>http://example.com<>test<>00:01<>click<>comment<>free<>\nNG Player<>{NG}<>127.0.0.1:7144<>http://example.com<>test<>fixture<>-1<>-1<>128<>RAW<>Artist<>Album<>Title<>http://example.com<>test<>00:01<>click<>comment<>free<>\nJazz Channel<>{JAZZ}<>127.0.0.1:7144<>http://example.com<>test<>fixture<>-1<>-1<>128<>RAW<>Artist<>Album<>Title<>http://example.com<>test<>00:01<>click<>comment<>free<>\n"
    );
    let directory_task = tokio::spawn(async move {
        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(3), directory.accept())
            .await
            .unwrap()
            .unwrap();
        let mut request = [0u8; 2048];
        tokio::time::timeout(Duration::from_secs(2), stream.read(&mut request))
            .await
            .unwrap()
            .unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let backend = DesktopBackend::open(dir.path().join("desktop.json"))
        .await
        .unwrap();
    backend
        .apply_yellow_pages(vec![yp("Fixture", &url)])
        .await
        .unwrap();
    let decorated = backend.get_yp_channels(true).await.unwrap();
    assert_eq!(decorated.as_array().unwrap().len(), 3);
    directory_task.await.unwrap();

    let favorite = rule("favorite", "Favorite", "Player");
    let mut hidden = rule("hidden", "Hide NG", "NG");
    hidden.favorite = false;
    hidden.hidden = true;
    let preview = backend
        .preview_channel_rules(vec![favorite, hidden])
        .await
        .unwrap();
    assert_eq!(preview.total, 3);
    assert_eq!(preview.matched, 2);
    assert_eq!(preview.favorites, 1);
    assert_eq!(preview.hidden, 1);
    assert_eq!(preview.channels.len(), 2);
    assert!(preview
        .channels
        .iter()
        .any(|item| item.channel_id == ROCK && item.is_favorite));
    assert!(preview
        .channels
        .iter()
        .any(|item| item.channel_id == NG && item.is_hidden));
    close(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_only_preferences_can_be_saved_while_backend_is_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    let initial = backend.snapshot().await.unwrap();
    backend.stop().await.unwrap();
    let mut desktop = initial.desktop.clone();
    desktop.channel_rules = vec![rule("rock", "Rock", "Rock")];
    let saved = backend
        .apply_desktop_preferences(initial.limits, initial.yellow_pages, desktop.clone())
        .await
        .unwrap();
    assert!(!saved.running);
    assert_eq!(saved.desktop, desktop);
    let persisted = ConfigManager::new(&path).load().unwrap();
    assert_eq!(
        serde_json::from_value::<DesktopPreferences>(persisted.desktop).unwrap(),
        desktop
    );
    close(&backend).await;
}
