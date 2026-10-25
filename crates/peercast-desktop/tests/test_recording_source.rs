use peercast_app::ConfigManager;
use peercast_desktop::backend::{DesktopBackend, YellowPageEntry};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CHANNEL_ID: &str = "11111111111111111111111111111111";

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

    let (mut upstream, _) = tokio::time::timeout(Duration::from_secs(2), tracker.accept())
        .await
        .expect("dedicated recording relay should dial the tracker")
        .unwrap();
    let mut request = [0u8; 2048];
    let _ = tokio::time::timeout(Duration::from_secs(2), upstream.read(&mut request))
        .await
        .expect("dedicated relay should send its PCP request")
        .unwrap();
    let main_during = backend.snapshot().await.unwrap();
    assert_eq!(main_during.endpoint, main_before.endpoint);
    assert_eq!(main_during.channels.len(), main_before.channels.len());
    assert_eq!(
        main_during.channels[0].connection_attempts,
        main_before.channels[0].connection_attempts
    );
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
    drop(upstream);
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
