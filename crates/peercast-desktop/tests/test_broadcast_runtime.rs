use std::{net::SocketAddr, sync::Arc, time::Duration};

use peercast_app::BoundedLogBuffer;
use peercast_desktop::{
    backend::Limits, broadcast::BroadcastRequest, broadcast_runtime::BroadcastRuntime,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::timeout,
};

const CHANNEL_ID: &str = "1123456789abcdef0123456789abcdef";
const BROADCAST_ID: &str = "2123456789abcdef0123456789abcdef";

fn request() -> BroadcastRequest {
    BroadcastRequest {
        name: "loopback broadcast test".into(),
        genre: "test".into(),
        description: "local fixture only".into(),
        comment: "synthetic media".into(),
        contact_url: String::new(),
        content_type: "FLV".into(),
        bitrate: 128,
        source_mode: "push".into(),
        source_uri: String::new(),
        ingest_port: 0,
        relay_port: 0,
        announce_sp: false,
    }
}

fn limits() -> Limits {
    Limits {
        max_directs: 8,
        max_relays: 8,
        max_upstream_rate: 1000,
    }
}

fn flv_tag(tag_type: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(15 + body.len());
    out.push(tag_type);
    out.extend_from_slice(&[
        (body.len() >> 16) as u8,
        (body.len() >> 8) as u8,
        body.len() as u8,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ]);
    out.extend_from_slice(body);
    out.extend_from_slice(&((11 + body.len()) as u32).to_be_bytes());
    out
}

fn flv_fixture() -> (Vec<u8>, Vec<u8>) {
    let mut fixture = b"FLV\x01\x01\x00\x00\x00\x09\x00\x00\x00\x00".to_vec();
    let sequence = flv_tag(9, &[0x17, 0, 0, 0, 0]);
    let keyframe = flv_tag(9, &[0x17, 1, 0, 0, 0, 0x65, 0x88, 0x84]);
    fixture.extend_from_slice(&sequence);
    fixture.extend_from_slice(&keyframe);
    (fixture, keyframe)
}

async fn write_chunked_body(socket: &mut TcpStream, bytes: &[u8]) {
    socket
        .write_all(format!("{:X}\r\n", bytes.len()).as_bytes())
        .await
        .unwrap();
    socket.write_all(bytes).await.unwrap();
    socket.write_all(b"\r\n").await.unwrap();
}

async fn read_stream_when_ready(addr: SocketAddr, expected: &[u8]) -> Vec<u8> {
    timeout(Duration::from_secs(5), async {
        loop {
            let mut socket = TcpStream::connect(addr).await.unwrap();
            socket
                .write_all(
                    format!(
                        "GET /stream/{CHANNEL_ID}.flv HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let mut response = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let count = socket.read(&mut chunk).await.unwrap();
                if count == 0 {
                    break;
                }
                response.extend_from_slice(&chunk[..count]);
                if response.starts_with(b"HTTP/1.1 503") {
                    break;
                }
                if response.starts_with(b"HTTP/1.1 200")
                    && response.windows(expected.len()).any(|window| window == expected)
                {
                    return response;
                }
            }
            assert!(
                response.starts_with(b"HTTP/1.1 503")
                    || response.starts_with(b"HTTP/1.1 200"),
                "unexpected stream response: {}",
                String::from_utf8_lossy(&response)
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("fixture must reach the HTTP stream")
}

async fn post_flv(addr: SocketAddr, fixture: &[u8]) -> TcpStream {
    let mut input = TcpStream::connect(addr).await.unwrap();
    input
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: video/x-flv\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n",
        )
        .await
        .unwrap();
    write_chunked_body(&mut input, fixture).await;
    let mut response = vec![0; 256];
    let count = timeout(Duration::from_secs(2), input.read(&mut response))
        .await
        .expect("ingest handshake should reply")
        .unwrap();
    assert!(response[..count].starts_with(b"HTTP/1.1 200 OK"));
    input
}

#[tokio::test]
async fn local_http_push_flv_reaches_stream_and_shutdown_releases_ports() {
    let mut broadcast = BroadcastRuntime::start(
        &request(),
        CHANNEL_ID,
        BROADCAST_ID,
        &limits(),
        Arc::new(BoundedLogBuffer::default()),
        None,
    )
    .await
    .unwrap();
    let ingest_addr = broadcast.ingest_addr();
    let http_addr = broadcast.http_addr();
    assert_eq!(ingest_addr.ip().to_string(), "127.0.0.1");
    assert_eq!(http_addr.ip().to_string(), "127.0.0.1");

    let (fixture, expected_keyframe) = flv_fixture();
    let input = post_flv(ingest_addr, &fixture).await;
    let response = read_stream_when_ready(http_addr, &expected_keyframe).await;
    assert!(response.starts_with(b"HTTP/1.1 200 OK"));

    drop(input);
    broadcast.shutdown().await.unwrap();
    timeout(Duration::from_secs(2), TcpListener::bind(ingest_addr))
        .await
        .expect("ingest port release should be prompt")
        .expect("ingest port should be released");
    timeout(Duration::from_secs(2), TcpListener::bind(http_addr))
        .await
        .expect("HTTP port release should be prompt")
        .expect("HTTP port should be released");
}

#[tokio::test]
async fn pull_source_is_forwarded_incrementally_and_cancelled_on_shutdown() {
    let source_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let (fixture, expected_keyframe) = flv_fixture();
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let source_task = tokio::spawn(async move {
        let (mut socket, _) = timeout(Duration::from_secs(2), source_listener.accept())
            .await
            .expect("pull source should be requested")
            .unwrap();
        let mut headers = Vec::new();
        let mut chunk = [0u8; 512];
        timeout(Duration::from_secs(2), async {
            while !headers.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = socket.read(&mut chunk).await.unwrap();
                assert_ne!(count, 0);
                headers.extend_from_slice(&chunk[..count]);
            }
        })
        .await
        .expect("GET request headers should arrive");
        assert!(headers.starts_with(b"GET /source HTTP/1.1"));
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n{:X}\r\n",
                    fixture.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        socket.write_all(&fixture).await.unwrap();
        socket.write_all(b"\r\n").await.unwrap();
        let _ = release_rx.await;
    });

    let mut request = request();
    request.source_mode = "pull".into();
    request.source_uri = format!("http://{source_addr}/source");
    let mut broadcast = BroadcastRuntime::start(
        &request,
        CHANNEL_ID,
        BROADCAST_ID,
        &limits(),
        Arc::new(BoundedLogBuffer::default()),
        None,
    )
    .await
    .unwrap();
    let http_addr = broadcast.http_addr();
    let response = read_stream_when_ready(http_addr, &expected_keyframe).await;
    assert!(response.starts_with(b"HTTP/1.1 200 OK"));
    broadcast.shutdown().await.unwrap();
    drop(release_tx);
    timeout(Duration::from_secs(2), source_task)
        .await
        .expect("loopback source task should stop")
        .unwrap();
}
