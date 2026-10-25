use peercast_app::ConfigManager;
use peercast_desktop::backend::{DesktopBackend, YellowPageEntry};
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

fn listing_line(id: &str, tracker: &str) -> String {
    format!(
        "Recording Source<>{id}<>{tracker}<>http://example.com<>test<>fixture<>-1<>-1<>128<>RAW<>Artist<>Album<>Title<>http://example.com<>test<>00:01<>click<>comment<>free<>\n"
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recording_source_has_an_isolated_runtime_and_close_drains_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let tracker = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tracker_addr = tracker.local_addr().unwrap().to_string();
    let directory = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let directory_url = format!("http://{}/index.txt", directory.local_addr().unwrap());
    let body = listing_line(CHANNEL_ID, &tracker_addr);
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
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    backend
        .apply_yellow_pages(vec![yp(&directory_url)])
        .await
        .unwrap();
    backend.get_yp_channels(true).await.unwrap();
    directory_task.await.unwrap();

    let before_config = ConfigManager::new(&path).load().unwrap();
    let main_before = backend.snapshot().await.unwrap();
    assert!(main_before.recordings.is_empty());
    let tracker_task = tokio::spawn(async move {
        let (mut upstream, _) = tokio::time::timeout(Duration::from_secs(2), tracker.accept())
            .await
            .expect("dedicated recording relay should dial the tracker")
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
        upstream.write_all(&ready_fixture()).await.unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let mut source = backend
        .open_recording_source(CHANNEL_ID, "Fixture")
        .await
        .unwrap();
    assert_eq!(source.name, "Recording Source");
    assert_eq!(source.content_type, "RAW");
    assert_eq!(source.title, "fixture");
    assert!(source
        .stream_url
        .ends_with(&format!("/stream/{CHANNEL_ID}")));

    let main_during = backend.snapshot().await.unwrap();
    assert_eq!(main_during.endpoint, main_before.endpoint);
    assert!(main_before.channels.is_empty());
    assert!(main_during.channels.is_empty());
    assert_eq!(ConfigManager::new(&path).load().unwrap(), before_config);

    let source_port: u16 = reqwest::Url::parse(&source.stream_url)
        .unwrap()
        .port()
        .unwrap();
    source.close().await.unwrap();
    let rebound = TcpListener::bind(("127.0.0.1", source_port))
        .await
        .expect("recording listener should be released after close");
    drop(rebound);
    tracker_task.await.unwrap();
    let main_after = backend.snapshot().await.unwrap();
    assert!(main_after.running);
    assert_eq!(main_after.endpoint, main_before.endpoint);
    close_backend(&backend).await;
}

async fn close_backend(backend: &DesktopBackend) {
    tokio::time::timeout(Duration::from_secs(8), backend.close())
        .await
        .expect("main backend close should be bounded")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recording_source_rejects_invalid_cached_tracker_without_persisting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop.json");
    let directory = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let directory_url = format!("http://{}/index.txt", directory.local_addr().unwrap());
    let body = listing_line(CHANNEL_ID, "0.0.0.0:7144");
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
    let backend = DesktopBackend::open(path.clone()).await.unwrap();
    backend
        .apply_yellow_pages(vec![yp(&directory_url)])
        .await
        .unwrap();
    backend.get_yp_channels(true).await.unwrap();
    directory_task.await.unwrap();
    let before = ConfigManager::new(&path).load().unwrap();
    let endpoint = backend.snapshot().await.unwrap().endpoint;
    assert!(backend
        .open_recording_source(CHANNEL_ID, "Fixture")
        .await
        .is_err());
    assert_eq!(ConfigManager::new(&path).load().unwrap(), before);
    assert_eq!(backend.snapshot().await.unwrap().endpoint, endpoint);
    close_backend(&backend).await;
}
