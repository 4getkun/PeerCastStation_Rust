use peercast_app::runtime_config::YellowPageSettings;
use peercast_app::{AppConfig, ConfigManager};
use peercast_desktop::backend::{DesktopBackend, Limits};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn limits(max_directs: u32, max_relays: u32, max_upstream_rate: u32) -> Limits {
    Limits {
        max_directs,
        max_relays,
        max_upstream_rate,
    }
}

async fn close_backend(backend: &DesktopBackend) {
    tokio::time::timeout(Duration::from_secs(8), backend.close())
        .await
        .expect("backend close timed out")
        .expect("backend close failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_and_limits_survive_backend_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backend = tokio::time::timeout(Duration::from_secs(5), DesktopBackend::open(path.clone()))
        .await
        .unwrap()
        .unwrap();

    let initial = backend.snapshot().await.unwrap();
    assert!(initial.running);
    assert!(initial.endpoint.starts_with("http://127.0.0.1:"));
    let probe = tokio::time::timeout(Duration::from_secs(10), backend.run_probe())
        .await
        .unwrap()
        .unwrap();
    let result = probe.last_probe.expect("probe result missing");
    assert!(result.passed);
    assert_eq!(result.bytes, 20 * 8192);

    let requested = limits(12, 10, 32100);
    let updated = tokio::time::timeout(
        Duration::from_secs(8),
        backend.apply_limits(requested.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(updated.limits.max_directs, requested.max_directs);
    assert_eq!(updated.limits.max_relays, requested.max_relays);
    assert_eq!(
        updated.limits.max_upstream_rate,
        requested.max_upstream_rate
    );
    let persisted = ConfigManager::new(&path).load().unwrap();
    assert_eq!(persisted.server.max_directs, requested.max_directs);
    assert_eq!(persisted.server.max_relays, requested.max_relays);
    assert_eq!(
        persisted.server.max_upstream_rate,
        requested.max_upstream_rate
    );

    let restarted = tokio::time::timeout(Duration::from_secs(8), backend.restart())
        .await
        .unwrap()
        .unwrap();
    assert!(restarted.running);
    assert_eq!(restarted.limits.max_directs, requested.max_directs);
    assert_eq!(restarted.limits.max_relays, requested.max_relays);
    assert_eq!(
        restarted.limits.max_upstream_rate,
        requested.max_upstream_rate
    );
    let persisted_after_restart = ConfigManager::new(&path).load().unwrap();
    assert_eq!(
        persisted_after_restart.server.max_directs,
        requested.max_directs
    );
    assert_eq!(
        persisted_after_restart.server.max_relays,
        requested.max_relays
    );
    assert_eq!(
        persisted_after_restart.server.max_upstream_rate,
        requested.max_upstream_rate
    );

    close_backend(&backend).await;
    let reopened = DesktopBackend::open(path).await.unwrap();
    assert_eq!(reopened.snapshot().await.unwrap().limits, requested);
    close_backend(&reopened).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_limits_leave_running_backend_and_saved_config_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    backend.apply_limits(limits(9, 7, 24000)).await.unwrap();
    let before = backend.snapshot().await.unwrap();
    let saved_before = std::fs::read(&path).unwrap();

    for invalid in [limits(0, 7, 24000), limits(9, 257, 24000), limits(9, 7, 0)] {
        assert!(backend.apply_limits(invalid).await.is_err());
        let current = backend.snapshot().await.unwrap();
        assert!(current.running);
        assert_eq!(current.endpoint, before.endpoint);
        assert_eq!(current.limits.max_directs, before.limits.max_directs);
        assert_eq!(std::fs::read(&path).unwrap(), saved_before);
        let address = before.endpoint.strip_prefix("http://").unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(address))
                .await
                .unwrap()
                .is_ok()
        );
    }

    close_backend(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_rejects_queued_restart_and_releases_listener_during_probe() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backend = DesktopBackend::open(path).await.unwrap();
    let endpoint = backend.snapshot().await.unwrap().endpoint;
    let probe_backend = backend.clone();
    let probe_task = tokio::spawn(async move { probe_backend.run_probe().await });

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!probe_task.is_finished(), "probe should still be active");
    assert!(backend.begin_close());
    let restart_backend = backend.clone();
    let restart_task = tokio::spawn(async move { restart_backend.restart().await });
    let close_backend_ref = backend.clone();
    let close_task = tokio::spawn(async move { close_backend_ref.close().await });

    let restart_result = tokio::time::timeout(Duration::from_secs(10), restart_task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        restart_result.is_err(),
        "restart must be rejected after begin_close"
    );
    let _probe_result = tokio::time::timeout(Duration::from_secs(10), probe_task)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), close_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let stopped = backend.snapshot().await.unwrap();
    assert!(!stopped.running);
    assert_eq!(stopped.active_tasks, 0);
    let address = endpoint.strip_prefix("http://").unwrap();
    let rebound = TcpListener::bind(address)
        .await
        .expect("the closed listener address must be available for a new server");
    drop(rebound);
}

fn yp(name: &str, channels_url: &str) -> peercast_desktop::backend::YellowPageEntry {
    peercast_desktop::backend::YellowPageEntry {
        name: name.into(),
        channels_url: channels_url.into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn yellow_pages_default_crud_empty_and_limits_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    assert_eq!(
        backend.snapshot().await.unwrap().yellow_pages,
        vec![
            yp("SP", "http://bayonet.ddo.jp/sp/index.txt"),
            yp("P@", "https://p-at.net/index.txt"),
        ]
    );
    let entries = vec![
        yp("SP", "http://bayonet.ddo.jp/sp/"),
        yp(" Custom ", "https://example.com/yp/"),
    ];
    let expected = vec![
        yp("SP", "http://bayonet.ddo.jp/sp/index.txt"),
        yp("Custom", "https://example.com/yp/index.txt"),
    ];
    assert_eq!(
        backend
            .apply_yellow_pages(entries)
            .await
            .unwrap()
            .yellow_pages,
        expected
    );
    let requested = limits(11, 9, 31000);
    assert_eq!(
        backend
            .apply_limits(requested.clone())
            .await
            .unwrap()
            .yellow_pages,
        expected
    );
    let config = ConfigManager::new(&path).load().unwrap();
    assert_eq!(config.runtime.yellow_pages.len(), 2);
    assert!(config
        .runtime
        .yellow_pages
        .iter()
        .all(|yp| !yp.announcement_enabled));
    close_backend(&backend).await;
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    assert_eq!(backend.snapshot().await.unwrap().yellow_pages, expected);
    assert_eq!(backend.snapshot().await.unwrap().limits, requested);
    let edited = vec![yp(
        "Custom updated",
        "http://127.0.0.1:12345/index.txt?source=1",
    )];
    assert_eq!(
        backend
            .apply_yellow_pages(edited.clone())
            .await
            .unwrap()
            .yellow_pages,
        edited
    );
    assert_eq!(backend.restart().await.unwrap().yellow_pages, edited);
    assert!(backend
        .apply_yellow_pages(vec![])
        .await
        .unwrap()
        .yellow_pages
        .is_empty());
    close_backend(&backend).await;
    let backend = DesktopBackend::open(path).await.unwrap();
    assert!(backend.snapshot().await.unwrap().yellow_pages.is_empty());
    assert_eq!(backend.snapshot().await.unwrap().limits, requested);
    close_backend(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_single_sp_default_migrates_to_sp_and_p_at() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let mut config = AppConfig::default();
    config.runtime.yellow_pages.push(YellowPageSettings {
        name: "SP".into(),
        channels_uri: Some("http://bayonet.ddo.jp/sp/index.txt".into()),
        announcement_enabled: false,
        ..Default::default()
    });
    ConfigManager::new(&path).save(&config).unwrap();

    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    let expected = vec![
        yp("SP", "http://bayonet.ddo.jp/sp/index.txt"),
        yp("P@", "https://p-at.net/index.txt"),
    ];
    assert_eq!(backend.snapshot().await.unwrap().yellow_pages, expected);
    let saved = ConfigManager::new(&path).load().unwrap();
    let saved_pages: Vec<_> = saved
        .runtime
        .yellow_pages
        .iter()
        .filter_map(|page| {
            page.channels_uri
                .as_ref()
                .map(|url| (page.name.as_str(), url.as_str()))
        })
        .collect();
    assert_eq!(
        saved_pages,
        vec![
            ("SP", "http://bayonet.ddo.jp/sp/index.txt"),
            ("P@", "https://p-at.net/index.txt"),
        ]
    );
    close_backend(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_and_empty_yellow_page_configs_are_not_migrated() {
    let dir = tempfile::tempdir().unwrap();
    for (filename, pages) in [
        (
            "custom.json",
            vec![
                yp("SP", "http://bayonet.ddo.jp/sp/index.txt"),
                yp("Custom", "https://example.org/yp/index.txt"),
            ],
        ),
        ("empty.json", Vec::new()),
    ] {
        let path = dir.path().join(filename);
        let mut config = AppConfig::default();
        config.runtime.yellow_pages = pages
            .iter()
            .map(|entry| YellowPageSettings {
                name: entry.name.clone(),
                channels_uri: Some(entry.channels_url.clone()),
                announcement_enabled: false,
                ..Default::default()
            })
            .collect();
        ConfigManager::new(&path).save(&config).unwrap();

        let backend = DesktopBackend::open(path).await.unwrap();
        assert_eq!(backend.snapshot().await.unwrap().yellow_pages, pages);
        close_backend(&backend).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_yellow_pages_preserve_backend_and_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    let before = backend.snapshot().await.unwrap();
    let saved = std::fs::read(&path).unwrap();
    let mut invalid = vec![
        vec![yp("", "https://example.com/")],
        vec![yp("Test", "file:///C:/test")],
        vec![yp("Test", "javascript:alert(1)")],
        vec![yp("Test", "https://user:secret@example.com/")],
        vec![yp("Test", "https://example.com/#fragment")],
        vec![
            yp("SP", "https://example.com/"),
            yp("SP", "https://example.net/"),
        ],
        vec![
            yp("One", "http://example.com/"),
            yp("Two", "http://example.com/index.txt"),
        ],
    ];
    invalid.push(
        (0..33)
            .map(|i| yp(&format!("YP{i}"), &format!("https://example.com/{i}.txt")))
            .collect(),
    );
    for entries in invalid {
        assert!(backend.apply_yellow_pages(entries).await.is_err());
        let current = backend.snapshot().await.unwrap();
        assert_eq!(current.endpoint, before.endpoint);
        assert_eq!(current.yellow_pages, before.yellow_pages);
        assert_eq!(std::fs::read(&path).unwrap(), saved);
    }
    close_backend(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn combined_preferences_save_atomically_and_invalid_input_preserves_both() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    let requested = limits(13, 12, 42000);
    let pages = vec![yp("Custom", "https://example.com/yp/")];
    let updated = backend
        .apply_preferences(requested.clone(), pages)
        .await
        .unwrap();
    assert_eq!(updated.limits, requested);
    assert_eq!(
        updated.yellow_pages,
        vec![yp("Custom", "https://example.com/yp/index.txt")]
    );
    let persisted = ConfigManager::new(&path).load().unwrap();
    assert_eq!(persisted.server.max_directs, 13);
    assert_eq!(persisted.runtime.yellow_pages[0].name, "Custom");
    let saved = std::fs::read(&path).unwrap();
    for (bad_limits, bad_pages) in [
        (
            limits(0, 12, 42000),
            vec![yp("Changed", "https://example.org/")],
        ),
        (limits(15, 14, 43000), vec![yp("Invalid", "file:///test")]),
    ] {
        assert!(backend
            .apply_preferences(bad_limits, bad_pages)
            .await
            .is_err());
        let actual = backend.snapshot().await.unwrap();
        assert_eq!(actual.endpoint, updated.endpoint);
        assert_eq!(actual.limits, updated.limits);
        assert_eq!(actual.yellow_pages, updated.yellow_pages);
        assert_eq!(std::fs::read(&path).unwrap(), saved);
    }
    close_backend(&backend).await;
    let backend = DesktopBackend::open(path).await.unwrap();
    assert_eq!(backend.snapshot().await.unwrap().limits, requested);
    assert_eq!(
        backend.snapshot().await.unwrap().yellow_pages,
        updated.yellow_pages
    );
    close_backend(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_preferences_write_keeps_current_backend_and_preferences() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let backup = dir.path().join("saved.json");
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    let before = backend.snapshot().await.unwrap();
    std::fs::rename(&path, &backup).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(backend
        .apply_preferences(limits(15, 14, 43000), vec![])
        .await
        .is_err());
    let after = backend.snapshot().await.unwrap();
    assert_eq!(after.endpoint, before.endpoint);
    assert_eq!(after.limits, before.limits);
    assert_eq!(after.yellow_pages, before.yellow_pages);
    assert!(after.running);
    close_backend(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn internal_probe_channel_is_not_exposed_as_a_user_connection() {
    let dir = tempfile::tempdir().unwrap();
    let backend = DesktopBackend::open(dir.path().join("desktop.json"))
        .await
        .unwrap();
    let snapshot = backend.snapshot().await.unwrap();
    assert!(snapshot.channels.is_empty());
    close_backend(&backend).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn directory_selection_creates_one_dynamic_relay_and_a_local_playlist() {
    const ID: &str = "11111111111111111111111111111111";
    let tracker = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let directory = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let directory_url = format!("http://{}/index.txt", directory.local_addr().unwrap());
    let body = format!("Test Channel<>{ID}<>{}<>http://example.com<>test<>fixture<>-1<>-1<>128<>RAW<>Artist<>Album<>Title<>http://example.com<>test<>00:01<>click<>comment<>free<>\n", tracker.local_addr().unwrap());
    let invalid = body
        .replace(ID, "22222222222222222222222222222222")
        .replace(&tracker.local_addr().unwrap().to_string(), "0.0.0.0:1234");
    let body = body + &invalid;
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
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let backend = DesktopBackend::open(dir.path().join("desktop.json"))
        .await
        .unwrap();
    backend
        .apply_yellow_pages(vec![yp("Fixture YP", &directory_url)])
        .await
        .unwrap();
    assert!(backend.prepare_yp_channel(ID, "Fixture YP").await.is_err());
    let rows = backend.get_yp_channels(true).await.unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);
    assert_eq!(rows[0]["name"], "Test Channel");
    assert!(backend.prepare_yp_channel(ID, "Wrong YP").await.is_err());
    assert!(backend
        .prepare_yp_channel("22222222222222222222222222222222", "Fixture YP")
        .await
        .is_err());
    let playlist = backend.prepare_yp_channel(ID, "Fixture YP").await.unwrap();
    let (_upstream, _) = tokio::time::timeout(Duration::from_secs(2), tracker.accept())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        backend.prepare_yp_channel(ID, "Fixture YP").await.unwrap(),
        playlist
    );
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client.get(&playlist).send().await.unwrap();
    assert!(response.status().is_success());
    let text = response.text().await.unwrap();
    assert!(text.starts_with("#EXTM3U"));
    assert!(text.contains("Test Channel"));
    let snapshot = backend.snapshot().await.unwrap();
    assert_eq!(snapshot.channels.len(), 1);
    assert!(text
        .lines()
        .any(|line| line == format!("{}/stream/{ID}", snapshot.endpoint)));
    assert_eq!(
        snapshot
            .channels
            .iter()
            .find(|c| c.id == ID)
            .unwrap()
            .connection_attempts,
        1
    );
    backend.apply_limits(limits(9, 8, 20000)).await.unwrap();
    assert!(
        backend.prepare_yp_channel(ID, "Fixture YP").await.is_err(),
        "replaced configuration must invalidate cached selections"
    );
    close_backend(&backend).await;
    directory_task.await.unwrap();
}
