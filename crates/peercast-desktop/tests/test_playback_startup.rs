use peercast_app::ConfigManager;
use peercast_desktop::backend::{DesktopBackend, YellowPageEntry};
use peercast_desktop::player::PlayerSettings;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CHANNEL_ID: &str = "11111111111111111111111111111111";

fn atom_leaf(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

fn atom_parent(tag: &[u8; 4], children: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(0x8000_0000u32 | children.len() as u32).to_le_bytes());
    for child in children {
        out.extend_from_slice(&child);
    }
    out
}

fn ready_fixture() -> Vec<u8> {
    let id = (0..16)
        .map(|i| u8::from_str_radix(&CHANNEL_ID[i * 2..i * 2 + 2], 16).unwrap())
        .collect::<Vec<_>>();
    [
        atom_parent(b"oleh", vec![atom_leaf(b"sid\0", &[7; 16])]),
        atom_leaf(b"ok\0\0", &1u32.to_le_bytes()),
        atom_parent(
            b"chan",
            vec![
                atom_leaf(b"id\0\0", &id),
                atom_parent(
                    b"pkt\0",
                    vec![
                        atom_leaf(b"type", b"head"),
                        atom_leaf(b"pos\0", &0u32.to_le_bytes()),
                        atom_leaf(b"data", b"RAW-HEADER"),
                    ],
                ),
            ],
        ),
    ]
    .concat()
}

fn yp(url: &str) -> YellowPageEntry {
    YellowPageEntry {
        name: "Fixture".into(),
        channels_url: url.into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playback_waits_for_delayed_initial_source_header() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("desktop.json");
    let config = peercast_app::AppConfig::default();
    ConfigManager::new(&config_path).save(&config).unwrap();
    let tracker = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tracker_addr = tracker.local_addr().unwrap().to_string();
    let directory = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let directory_url = format!("http://{}/index.txt", directory.local_addr().unwrap());
    let body = format!(
        "Delayed Source<>{CHANNEL_ID}<>{tracker_addr}<>http://example.com<>test<>fixture<>-1<>-1<>128<>RAW<>Artist<>Album<>Title<>http://example.com<>test<>00:01<>click<>comment<>free<>\n"
    );
    let directory_task = tokio::spawn(async move {
        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(3), directory.accept())
            .await
            .expect("directory request should arrive")
            .unwrap();
        let mut request = [0u8; 2048];
        tokio::time::timeout(Duration::from_secs(2), stream.read(&mut request))
            .await
            .expect("directory request should be read")
            .unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let backend = DesktopBackend::open(config_path.clone()).await.unwrap();
    let initial = backend.snapshot().await.unwrap();
    let mut desktop = initial.desktop;
    desktop.player = PlayerSettings {
        executable: Some(std::env::current_exe().unwrap().display().to_string()),
        arguments: "--list".into(),
    };
    backend
        .apply_desktop_preferences(initial.limits, initial.yellow_pages, desktop)
        .await
        .unwrap();
    backend
        .apply_yellow_pages(vec![yp(&directory_url)])
        .await
        .unwrap();
    backend.get_yp_channels(true).await.unwrap();
    directory_task.await.unwrap();

    let tracker_task = tokio::spawn(async move {
        let (mut upstream, _) = tokio::time::timeout(Duration::from_secs(3), tracker.accept())
            .await
            .expect("playback relay should connect to the local tracker")
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 1024];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let size = upstream.read(&mut buffer).await.unwrap();
            assert!(size > 0);
            request.extend_from_slice(&buffer[..size]);
        }
        upstream
            .write_all(b"HTTP/1.0 200 OK\r\nContent-Type: application/x-peercast-pcp\r\n\r\n")
            .await
            .unwrap();
        let mut helo = [0u8; 4096];
        tokio::time::timeout(Duration::from_secs(3), upstream.read(&mut helo))
            .await
            .expect("relay HELO should arrive")
            .unwrap();
        tokio::time::sleep(Duration::from_secs(11)).await;
        upstream.write_all(&ready_fixture()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    let result = backend.play_yp_channel(CHANNEL_ID, "Fixture").await;
    tracker_task.await.unwrap();
    backend.close().await.unwrap();
    assert!(
        result.is_ok(),
        "playback should launch after delayed source header: {}",
        result.err().unwrap_or_default()
    );
    assert!(ConfigManager::new(&config_path).load().is_ok());
}
