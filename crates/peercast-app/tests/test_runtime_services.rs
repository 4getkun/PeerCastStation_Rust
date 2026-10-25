use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use peercast_app::config::AppConfig;
use peercast_app::lifecycle::TaskTracker;
use peercast_app::logging::BoundedLogBuffer;
use peercast_app::runtime::AppRuntime;
use peercast_app::runtime_config::{ChannelConfig, ChannelSource, YellowPageSettings};
use peercast_core::atom::Atom;
use peercast_core::channel_id::ChannelId;
use peercast_core::gossip::{BroadcastGroup, GossipDropReason};
use peercast_core::id4::ID4;
use peercast_rpc::server::RpcState;
use peercast_rpc::{AccessLevel, RpcDispatcher};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

const CHANNEL_ID: &str = "0123456789abcdef0123456789abcdef";
const INDEX_LINE: &str = "My Channel<>0123456789ABCDEF0123456789ABCDEF<>127.0.0.1:7144<>http://example.com<>J-Pop<>24/7 Music<>12<>3<>128<>FLV<>Artist Name<>Album Name<>Song Title<>http://track.url<>my_channel<>01:30<>click<>Test comment<>free<>\n";

fn make_runtime(config: &AppConfig) -> (Arc<AppRuntime>, TaskTracker) {
    let tracker = TaskTracker::new();
    let runtime = AppRuntime::new(
        config,
        tracker.clone(),
        Arc::new(BoundedLogBuffer::default()),
        Arc::new(RwLock::new(RpcState::default())),
    )
    .unwrap();
    (runtime, tracker)
}

async fn stop(runtime: Arc<AppRuntime>, tracker: TaskTracker) {
    runtime.shutdown().await;
    tokio::time::timeout(
        Duration::from_secs(2),
        tracker.wait_for_drain(Duration::from_secs(2)),
    )
    .await
    .expect("drain must be bounded")
    .expect("runtime tasks should drain");
}

fn runtime_with_directory(
    body_limit: usize,
    line_limit: usize,
    field_limit: usize,
    timeout_ms: u64,
    uri: String,
) -> (Arc<AppRuntime>, TaskTracker, Arc<BoundedLogBuffer>) {
    let mut config = AppConfig::default();
    config.runtime.directory.max_total_bytes = body_limit;
    config.runtime.directory.max_lines = line_limit;
    config.runtime.directory.max_field_len = field_limit;
    config.runtime.directory.request_timeout_ms = timeout_ms;
    config.runtime.yellow_pages.push(YellowPageSettings {
        name: "loopback directory".to_string(),
        channels_uri: Some(uri),
        ..YellowPageSettings::default()
    });
    let tracker = TaskTracker::new();
    let log = Arc::new(BoundedLogBuffer::default());
    let runtime = AppRuntime::new(
        &config,
        tracker.clone(),
        Arc::clone(&log),
        Arc::new(RwLock::new(RpcState::default())),
    )
    .unwrap();
    (runtime, tracker, log)
}

async fn fake_directory(responses: Vec<Vec<u8>>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let uri = format!("http://{}/index.txt", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .expect("bounded fake server accept")
                .unwrap();
            let mut request = Vec::new();
            tokio::time::timeout(Duration::from_secs(2), async {
                let mut buf = [0; 1024];
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let n = socket.read(&mut buf).await?;
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                io::Result::Ok(())
            })
            .await
            .expect("bounded fake server request read")
            .unwrap();
            tokio::time::timeout(Duration::from_secs(2), socket.write_all(&response))
                .await
                .expect("bounded fake server response")
                .unwrap();
        }
    });
    (uri, task)
}

fn chunked_http_response(body: &[u8]) -> Vec<u8> {
    let mut response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    response.extend_from_slice(format!("{:X}\r\n", body.len()).as_bytes());
    response.extend_from_slice(body);
    response.extend_from_slice(b"\r\n0\r\n\r\n");
    response
}

async fn call_directory_rpc(dispatcher: &RpcDispatcher, method: &str) -> serde_json::Value {
    let request = format!(r#"{{"jsonrpc":"2.0","method":"{method}","id":1}}"#);
    serde_json::from_str(
        &dispatcher
            .handle_request_str(&request, AccessLevel::Admin)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn directory_runtime_limits_change_parser_results() {
    let mut config = AppConfig::default();
    config.runtime.directory.max_total_bytes = INDEX_LINE.len() + 80;
    config.runtime.directory.max_lines = 1;
    config.runtime.directory.max_field_len = 64;
    let (runtime, tracker) = make_runtime(&config);
    assert_eq!(runtime.parse_directory(INDEX_LINE).unwrap().len(), 1);
    assert!(runtime
        .parse_directory(&format!("{INDEX_LINE}{INDEX_LINE}"))
        .is_err());
    stop(runtime, tracker).await;

    let mut config = AppConfig::default();
    config.runtime.directory.max_lines = 1;
    let (runtime, tracker) = make_runtime(&config);
    assert_eq!(
        runtime
            .parse_directory(&format!("{INDEX_LINE}{INDEX_LINE}"))
            .unwrap()
            .len(),
        1
    );
    stop(runtime, tracker).await;

    let mut config = AppConfig::default();
    config.runtime.directory.max_field_len = 3;
    let (runtime, tracker) = make_runtime(&config);
    assert!(runtime.parse_directory(INDEX_LINE).is_err());
    stop(runtime, tracker).await;
}

fn bcst(payload: &'static [u8]) -> Atom {
    let from = ChannelId::from_bytes([0x11; 16]);
    let cid = ChannelId::from_bytes([0x22; 16]);
    Atom::new_parent(
        ID4::PCP_BCST,
        vec![
            Atom::new_u8(ID4::BCST_TTL, 8),
            Atom::new_u8(ID4::BCST_HOPS, 0),
            Atom::new_leaf(ID4::BCST_FROM, Bytes::copy_from_slice(from.as_bytes())),
            Atom::new_u8(ID4::BCST_GRP, BroadcastGroup::ALL.raw()),
            Atom::new_leaf(ID4::BCST_CID, Bytes::copy_from_slice(cid.as_bytes())),
            Atom::new_leaf(ID4::PCP_CHAN, Bytes::from_static(payload)),
        ],
    )
}

#[tokio::test]
async fn gossip_runtime_capacity_evicts_and_allows_repeat_after_window() {
    let mut config = AppConfig::default();
    config.runtime.gossip.max_entries = 1;
    config.runtime.gossip.window_secs = 1;
    let (runtime, tracker) = make_runtime(&config);
    let first = bcst(b"first");
    let second = bcst(b"second");
    assert!(runtime
        .process_broadcast(&first)
        .await
        .drop_reason
        .is_none());
    assert_eq!(
        runtime.process_broadcast(&first).await.drop_reason,
        Some(GossipDropReason::DuplicateStormSuppressed)
    );
    assert!(runtime
        .process_broadcast(&second)
        .await
        .drop_reason
        .is_none());
    assert!(
        runtime
            .process_broadcast(&first)
            .await
            .drop_reason
            .is_none(),
        "capacity eviction should permit the old entry again"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        runtime
            .process_broadcast(&first)
            .await
            .drop_reason
            .is_none(),
        "expired entry should be admitted again"
    );
    stop(runtime, tracker).await;
}

#[tokio::test]
async fn probe_runtime_applies_exchange_deadline_to_stalled_loopback_peer() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let mut config = AppConfig::default();
    config.runtime.probe.connect_timeout_ms = 300;
    config.runtime.probe.exchange_timeout_ms = 80;
    let (runtime, tracker) = make_runtime(&config);
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        runtime.probe(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            addr.port(),
            ChannelId::from_bytes([0x33; 16]),
        ),
    )
    .await
    .expect("probe call must be bounded")
    .unwrap();
    assert!(matches!(
        result,
        peercast_pcp::probe::ProbeResult::Unreachable { .. }
    ));
    assert!(started.elapsed() < Duration::from_millis(500));
    stop(runtime, tracker).await;
    peer.abort();
    let _ = tokio::time::timeout(Duration::from_secs(1), peer).await;
}

fn push_config(header_limit: usize, chunk_limit: usize) -> AppConfig {
    let mut config = AppConfig::default();
    config.runtime.ingest.max_header_bytes = header_limit;
    config.runtime.ingest.max_chunk_bytes = chunk_limit;
    config.runtime.channels.push(ChannelConfig {
        channel_id: CHANNEL_ID.into(),
        name: "push test".into(),
        source: ChannelSource::HttpPush {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            auth_token: None,
        },
        ..ChannelConfig::default()
    });
    config
}

#[tokio::test]
async fn ingest_runtime_enforces_configured_header_and_chunk_limits() {
    let (runtime, tracker) = make_runtime(&push_config(32, 8));
    runtime.start().await.unwrap();
    let addr = runtime.ingest_addr(CHANNEL_ID).await.unwrap();
    let mut socket = TcpStream::connect(addr).await.unwrap();
    let header = b"POST / HTTP/1.1\r\nX-Long: 123456789012345678901234567890\r\n\r\n";
    tokio::time::timeout(Duration::from_secs(1), socket.write_all(header))
        .await
        .unwrap()
        .unwrap();
    let mut response = [0; 128];
    let read = tokio::time::timeout(Duration::from_secs(1), socket.read(&mut response))
        .await
        .unwrap();
    match read {
        Ok(n) => assert!(String::from_utf8_lossy(&response[..n]).contains("431")),
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset),
    }
    drop(socket);
    stop(runtime, tracker).await;

    let (runtime, tracker) = make_runtime(&push_config(1024, 2));
    runtime.start().await.unwrap();
    let addr = runtime.ingest_addr(CHANNEL_ID).await.unwrap();
    let mut socket = TcpStream::connect(addr).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        socket.write_all(
            b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let mut response = [0; 128];
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.read(&mut response))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !runtime
                .channel(CHANNEL_ID)
                .unwrap()
                .snapshot()
                .await
                .available
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("oversized chunk should take source offline");
    stop(runtime, tracker).await;
}

#[tokio::test]
async fn directory_http_fetch_applies_limits_and_cache_refresh_through_rpc() {
    let doubled = format!("{INDEX_LINE}{INDEX_LINE}").into_bytes();
    let (uri, server) = fake_directory(vec![
        chunked_http_response(&doubled),
        chunked_http_response(INDEX_LINE.as_bytes()),
    ])
    .await;
    let (runtime, tracker, log) = runtime_with_directory(4096, 1, 128, 1000, uri);
    let dispatcher =
        RpcDispatcher::default().with_yellow_page_directory(runtime.yellow_page_directory());
    assert_eq!(
        call_directory_rpc(&dispatcher, "getYPChannels").await["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        call_directory_rpc(&dispatcher, "getYPChannels").await["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        call_directory_rpc(&dispatcher, "updateYPChannels").await["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    assert!(log.is_empty());
    stop(runtime, tracker).await;

    // A chunked response has no Content-Length, so the streamed byte budget must stop it.
    let (uri, server) = fake_directory(vec![chunked_http_response(INDEX_LINE.as_bytes())]).await;
    let (runtime, tracker, log) = runtime_with_directory(INDEX_LINE.len() - 1, 10, 128, 1000, uri);
    let dispatcher =
        RpcDispatcher::default().with_yellow_page_directory(runtime.yellow_page_directory());
    assert_eq!(
        call_directory_rpc(&dispatcher, "updateYPChannels").await["result"],
        serde_json::json!([])
    );
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    assert!(log
        .snapshot()
        .iter()
        .any(|entry| entry.target == "directory" && entry.message.contains("body exceeds")));
    stop(runtime, tracker).await;

    let (uri, server) = fake_directory(vec![chunked_http_response(INDEX_LINE.as_bytes())]).await;
    let (runtime, tracker, log) = runtime_with_directory(4096, 10, 3, 1000, uri);
    let dispatcher =
        RpcDispatcher::default().with_yellow_page_directory(runtime.yellow_page_directory());
    assert_eq!(
        call_directory_rpc(&dispatcher, "getYPChannels").await["result"],
        serde_json::json!([])
    );
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
    assert!(log
        .snapshot()
        .iter()
        .any(|entry| entry.target == "directory"));
    stop(runtime, tracker).await;
}

#[tokio::test]
async fn directory_http_stall_times_out_and_shutdown_unblocks_provider() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let uri = format!("http://{}/index.txt", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut request = [0; 1024];
        let _ = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut request)).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let (runtime, tracker, _) = runtime_with_directory(4096, 10, 128, 100, uri);
    let dispatcher =
        RpcDispatcher::default().with_yellow_page_directory(runtime.yellow_page_directory());
    let started = Instant::now();
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        call_directory_rpc(&dispatcher, "updateYPChannels"),
    )
    .await
    .expect("RPC must be bounded");
    assert_eq!(
        response["error"]["code"],
        peercast_rpc::rpc_code::INTERNAL_ERROR
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    runtime.shutdown().await;
    let stopped = call_directory_rpc(&dispatcher, "getYPChannels").await;
    assert_eq!(
        stopped["error"]["code"],
        peercast_rpc::rpc_code::INTERNAL_ERROR
    );
    server.abort();
    let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
    tokio::time::timeout(
        Duration::from_secs(2),
        tracker.wait_for_drain(Duration::from_secs(2)),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn listing_only_yellow_page_fetches_directory_without_announcing() {
    let (uri, directory_server) =
        fake_directory(vec![chunked_http_response(INDEX_LINE.as_bytes())]).await;
    let announce_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let announce_endpoint = announce_listener.local_addr().unwrap();

    let mut config = AppConfig::default();
    config.runtime.directory.request_timeout_ms = 1000;
    config.runtime.yellow_pages.push(YellowPageSettings {
        name: "listing only".into(),
        channels_uri: Some(uri),
        endpoint: announce_endpoint,
        announcement_enabled: false,
        ..YellowPageSettings::default()
    });
    let (runtime, tracker) = make_runtime(&config);
    runtime.start().await.unwrap();
    let dispatcher =
        RpcDispatcher::default().with_yellow_page_directory(runtime.yellow_page_directory());

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        call_directory_rpc(&dispatcher, "updateYPChannels"),
    )
    .await
    .expect("directory listing request should be bounded");
    assert_eq!(response["result"].as_array().unwrap().len(), 1);
    tokio::time::timeout(Duration::from_secs(2), directory_server)
        .await
        .unwrap()
        .unwrap();

    assert!(
        tokio::time::timeout(Duration::from_millis(200), announce_listener.accept())
            .await
            .is_err(),
        "listing-only YP must not connect to its PCP endpoint"
    );
    stop(runtime, tracker).await;
}

#[test]
fn yellow_page_announcement_setting_defaults_true_and_roundtrips_false() {
    let legacy: YellowPageSettings = serde_json::from_value(serde_json::json!({
        "name": "legacy",
        "endpoint": "127.0.0.1:7144"
    }))
    .unwrap();
    assert!(legacy.announcement_enabled);

    let disabled = YellowPageSettings {
        announcement_enabled: false,
        ..YellowPageSettings::default()
    };
    let serialized = serde_json::to_value(&disabled).unwrap();
    assert_eq!(serialized["announcementEnabled"], false);
    let decoded: YellowPageSettings = serde_json::from_value(serialized).unwrap();
    assert!(!decoded.announcement_enabled);
}
