use std::{net::SocketAddr, sync::Arc, time::Duration};

use peercast_app::BoundedLogBuffer;
use peercast_desktop::{
    backend::Limits,
    broadcast::{BroadcastManager, BroadcastRequest},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

fn limits() -> Limits {
    Limits {
        max_directs: 8,
        max_relays: 8,
        max_upstream_rate: 20_000,
    }
}

async fn free_loopback_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn request(ingest_port: u16) -> BroadcastRequest {
    BroadcastRequest {
        name: "manager loopback test".into(),
        genre: "test".into(),
        description: "local fixture".into(),
        comment: "no external announcement".into(),
        contact_url: String::new(),
        content_type: "FLV".into(),
        bitrate: 128,
        source_mode: "push".into(),
        source_uri: String::new(),
        ingest_port,
        relay_port: 0,
        announce_sp: false,
    }
}

fn stream_port(url: &str) -> u16 {
    reqwest::Url::parse(url)
        .unwrap()
        .port_or_known_default()
        .unwrap()
}

async fn assert_port_reusable(port: u16) {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(address)
        .await
        .unwrap_or_else(|error| panic!("port {port} should be reusable: {error}"));
    drop(listener);
}

fn flv_tag(timestamp: u32, body: &[u8]) -> Vec<u8> {
    let mut tag = Vec::with_capacity(15 + body.len());
    tag.extend_from_slice(&[
        9,
        (body.len() >> 16) as u8,
        (body.len() >> 8) as u8,
        body.len() as u8,
        (timestamp >> 16) as u8,
        (timestamp >> 8) as u8,
        timestamp as u8,
        (timestamp >> 24) as u8,
        0,
        0,
        0,
    ]);
    tag.extend_from_slice(body);
    tag.extend_from_slice(&((11 + body.len()) as u32).to_be_bytes());
    tag
}

fn flv_fixture() -> (Vec<u8>, Vec<u8>) {
    let mut bytes = b"FLV\x01\x01\x00\x00\x00\x09\x00\x00\x00\x00".to_vec();
    bytes.extend_from_slice(&flv_tag(0, &[0x17, 0, 0, 0, 0]));
    let keyframe = flv_tag(100, &[0x17, 1, 0, 0, 0, 0x65, 0x88, 0x84]);
    bytes.extend_from_slice(&keyframe);
    (bytes, keyframe)
}

fn pcp_leaf(name: &[u8; 4], value: &[u8]) -> Vec<u8> {
    let mut atom = Vec::with_capacity(8 + value.len());
    atom.extend_from_slice(name);
    atom.extend_from_slice(&(value.len() as u32).to_le_bytes());
    atom.extend_from_slice(value);
    atom
}

fn pcp_helo() -> Vec<u8> {
    let mut children = Vec::new();
    children.extend_from_slice(&pcp_leaf(b"agnt", b"loopback test\0"));
    children.extend_from_slice(&pcp_leaf(b"sid\0", &[0x71; 16]));
    children.extend_from_slice(&pcp_leaf(b"ver\0", &1218u32.to_le_bytes()));
    let mut atom = Vec::with_capacity(8 + children.len());
    atom.extend_from_slice(b"helo");
    atom.extend_from_slice(&(0x8000_0000u32 | 3).to_le_bytes());
    atom.extend_from_slice(&children);
    atom
}

async fn post_flv(addr: SocketAddr, bytes: &[u8]) -> TcpStream {
    let mut socket = TcpStream::connect(addr).await.unwrap();
    socket
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: video/x-flv\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
        )
        .await
        .unwrap();
    write_chunk(&mut socket, bytes).await;
    let mut response = [0u8; 256];
    let count = timeout(Duration::from_secs(2), socket.read(&mut response))
        .await
        .expect("HTTP Push should acknowledge the local request")
        .unwrap();
    assert!(response[..count].starts_with(b"HTTP/1.1 200 OK"));
    socket
}

async fn write_chunk(socket: &mut TcpStream, bytes: &[u8]) {
    socket
        .write_all(format!("{:X}\r\n", bytes.len()).as_bytes())
        .await
        .unwrap();
    socket.write_all(bytes).await.unwrap();
    socket.write_all(b"\r\n").await.unwrap();
}

async fn read_until(socket: &mut TcpStream, expected: &[u8], duration: Duration) -> Vec<u8> {
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    timeout(duration, async {
        loop {
            let count = socket.read(&mut chunk).await.unwrap();
            assert_ne!(count, 0, "connection closed before expected bytes arrived");
            response.extend_from_slice(&chunk[..count]);
            if response
                .windows(expected.len())
                .any(|window| window == expected)
            {
                break;
            }
        }
    })
    .await
    .expect("expected bytes should arrive before timeout");
    response
}

async fn read_http_header(socket: &mut TcpStream) -> Vec<u8> {
    let mut response = Vec::new();
    let mut chunk = [0u8; 1024];
    timeout(Duration::from_secs(3), async {
        while !response.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = socket.read(&mut chunk).await.unwrap();
            assert_ne!(count, 0, "HTTP listener closed before its response");
            response.extend_from_slice(&chunk[..count]);
        }
    })
    .await
    .expect("HTTP response header should arrive");
    response
}

#[tokio::test]
async fn push_broadcast_snapshot_stop_and_port_release() {
    let manager = BroadcastManager::new(None);
    let ingest_port = free_loopback_port().await;
    let id = manager
        .start(
            request(ingest_port),
            &limits(),
            Arc::new(BoundedLogBuffer::default()),
        )
        .await
        .unwrap();

    let (views, channels) = manager.snapshot().await.unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].channel_id, id);
    assert_eq!(views[0].name, "manager loopback test");
    assert!(views[0].input_url.starts_with("http://127.0.0.1:"));
    assert!(views[0].stream_url.starts_with("http://127.0.0.1:"));
    assert!(views[0].stream_url.ends_with(&format!("/{id}.flv")));
    assert_eq!(views[0].announcement_status, "掲載しない");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].id, id);
    assert_eq!(channels[0].name, "manager loopback test");
    assert!(channels[0].is_broadcasting);

    let http_port = stream_port(&views[0].stream_url);
    manager.stop(&id).await.unwrap();
    let (views, channels) = manager.snapshot().await.unwrap();
    assert!(views.is_empty());
    assert!(channels.is_empty());
    assert_port_reusable(ingest_port).await;
    assert_port_reusable(http_port).await;
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_racing_start_drains_and_rejects_later_starts() {
    let manager = BroadcastManager::new(None);
    let ingest_port = free_loopback_port().await;
    let starting_manager = Arc::clone(&manager);
    let starting = tokio::spawn(async move {
        starting_manager
            .start(
                request(ingest_port),
                &limits(),
                Arc::new(BoundedLogBuffer::default()),
            )
            .await
    });
    let shutting_manager = Arc::clone(&manager);
    let shutting_down = tokio::spawn(async move { shutting_manager.shutdown().await });

    if let Err(error) = starting.await.unwrap() {
        assert_eq!(error, "配信マネージャーは終了しています");
    }
    shutting_down.await.unwrap().unwrap();
    let (views, channels) = manager.snapshot().await.unwrap();
    assert!(views.is_empty());
    assert!(channels.is_empty());
    assert!(manager
        .start(
            request(free_loopback_port().await),
            &limits(),
            Arc::new(BoundedLogBuffer::default()),
        )
        .await
        .is_err());
    assert!(manager.snapshot().await.unwrap().0.is_empty());
    assert_port_reusable(ingest_port).await;
}

#[tokio::test]
async fn invalid_broadcast_requests_are_rejected_before_start() {
    let manager = BroadcastManager::new(None);
    let ingest_port = free_loopback_port().await;
    let valid = request(ingest_port);
    let mut invalid = Vec::new();

    let mut empty_name = valid.clone();
    empty_name.name = "  ".into();
    invalid.push(empty_name);

    let mut file_url = valid.clone();
    file_url.source_mode = "pull".into();
    file_url.source_uri = "file:///tmp/input.flv".into();
    invalid.push(file_url);

    let mut unsupported_mode = valid.clone();
    unsupported_mode.source_mode = "relay".into();
    invalid.push(unsupported_mode);

    let mut unsupported_format = valid;
    unsupported_format.content_type = "MP4".into();
    invalid.push(unsupported_format);

    for request in invalid {
        assert!(manager
            .start(request, &limits(), Arc::new(BoundedLogBuffer::default()),)
            .await
            .is_err());
    }
    let (views, channels) = manager.snapshot().await.unwrap();
    assert!(views.is_empty());
    assert!(channels.is_empty());
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn public_pcp_relay_keeps_streaming_after_five_seconds() {
    let manager = BroadcastManager::new(None);
    let ingest_port = free_loopback_port().await;
    let relay_port = free_loopback_port().await;
    let mut request = request(ingest_port);
    request.relay_port = relay_port;
    let id = manager
        .start(request, &limits(), Arc::new(BoundedLogBuffer::default()))
        .await
        .unwrap();

    let ingest_addr = SocketAddr::from(([127, 0, 0, 1], ingest_port));
    let (fixture, first_keyframe) = flv_fixture();
    let mut input = post_flv(ingest_addr, &fixture).await;

    let relay_addr = SocketAddr::from(([127, 0, 0, 1], relay_port));
    let mut relay = TcpStream::connect(relay_addr).await.unwrap();
    relay
        .write_all(
            format!(
                "GET /channel/{id} HTTP/1.0\r\nHost: localhost\r\nUser-Agent: loopback-test\r\nx-peercast-pcp: 1\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let prelude = read_http_header(&mut relay).await;
    assert!(prelude.starts_with(b"HTTP/1.0 200 OK"));
    relay.write_all(&pcp_helo()).await.unwrap();
    let initial = read_until(&mut relay, &first_keyframe, Duration::from_secs(3)).await;
    assert!(initial
        .windows(first_keyframe.len())
        .any(|bytes| bytes == first_keyframe));

    tokio::time::sleep(Duration::from_millis(5_300)).await;
    let late_keyframe = flv_tag(5_400, &[0x17, 1, 0, 0, 0, 0x66, 0x77, 0x88]);
    write_chunk(&mut input, &late_keyframe).await;
    let after_idle = read_until(&mut relay, &late_keyframe, Duration::from_secs(3)).await;
    assert!(after_idle
        .windows(late_keyframe.len())
        .any(|bytes| bytes == late_keyframe));

    drop(relay);
    drop(input);
    manager.stop(&id).await.unwrap();
    assert_port_reusable(relay_port).await;
    assert_port_reusable(ingest_port).await;
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn public_relay_port_rejects_http_management_and_api_paths() {
    let manager = BroadcastManager::new(None);
    let ingest_port = free_loopback_port().await;
    let relay_port = free_loopback_port().await;
    let mut request = request(ingest_port);
    request.relay_port = relay_port;
    let id = manager
        .start(request, &limits(), Arc::new(BoundedLogBuffer::default()))
        .await
        .unwrap();
    let relay_addr = SocketAddr::from(([127, 0, 0, 1], relay_port));

    for request in [
        b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n".as_slice(),
        b"POST /rpc HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .as_slice(),
    ] {
        let mut socket = TcpStream::connect(relay_addr).await.unwrap();
        socket.write_all(request).await.unwrap();
        let response = read_http_header(&mut socket).await;
        assert!(
            response.starts_with(b"HTTP/1.1 403 Forbidden"),
            "public PCP port must reject management HTTP: {}",
            String::from_utf8_lossy(&response)
        );
    }

    manager.stop(&id).await.unwrap();
    assert_port_reusable(relay_port).await;
    assert_port_reusable(ingest_port).await;
    manager.shutdown().await.unwrap();
}
