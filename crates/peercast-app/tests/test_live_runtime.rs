use bytes::{Buf, Bytes, BytesMut};
use peercast_app::config::AppConfig;
use peercast_app::lifecycle::{ShutdownReason, TaskTracker};
use peercast_app::logging::BoundedLogBuffer;
use peercast_app::metrics::AppMetrics;
use peercast_app::runtime_config::{
    ChannelConfig, ChannelMetadata, ChannelSource, YellowPageSettings,
};
use peercast_app::server::HttpServer;
use peercast_core::atom::Atom;
use peercast_core::channel_id::ChannelId;
use peercast_core::content::ContinuationFlags;
use peercast_core::id4::ID4;
use peercast_core::AtomBudget;
use peercast_pcp::packet::{ChannelPacket, PacketPayload};
use peercast_pcp::relay_client::{RelayClientStateMachine, RelayEvent, RelayNetworkProfile};
use peercast_pcp::relay_server::{RelayServerEvent, RelayServerStateMachine};
use peercast_rpc::server::RpcState;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, oneshot};

const CHANNEL_ID: &str = "0123456789abcdef0123456789abcdef";
const BROADCAST_ID: &str = "abcdef0123456789abcdef0123456789";

struct StalledHttpWriter;

impl tokio::io::AsyncWrite for StalledHttpWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::task::Poll::Pending
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Pending
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Pending
    }
}

#[tokio::test]
async fn stalled_http_writer_times_out_and_releases_viewer_slot() {
    let mut config = AppConfig::default();
    config.server.request_timeout_ms = 50;
    config.server.max_directs = 1;
    config.server.max_directs_per_channel = 1;
    config.runtime.channels.push(ChannelConfig {
        channel_id: CHANNEL_ID.into(),
        container_type: "RAW".into(),
        source: ChannelSource::HttpPush {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            auth_token: None,
        },
        ..ChannelConfig::default()
    });
    let server = start_server(config).await;
    let mut source = TcpStream::connect(server.runtime.ingest_addr(CHANNEL_ID).await.unwrap())
        .await
        .unwrap();
    source
        .write_all(b"POST / HTTP/1.1\r\n\r\nDATA")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !server
            .runtime
            .channel(CHANNEL_ID)
            .unwrap()
            .snapshot()
            .await
            .available
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let request = peercast_http::router::HttpRequest::parse(
        b"GET /stream/0123456789abcdef0123456789abcdef HTTP/1.1\r\n\r\n",
    )
    .unwrap();
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        server
            .runtime
            .serve_stream(&mut StalledHttpWriter, &request, CHANNEL_ID),
    )
    .await
    .expect("stalled client should release its slot")
    .unwrap_err();
    assert!(error.to_string().contains("timed out or stalled"));

    let mut next_viewer = TcpStream::connect(server.addr).await.unwrap();
    next_viewer
        .write_all(b"GET /stream/0123456789abcdef0123456789abcdef HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    let mut response = [0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(1), next_viewer.read(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(
        response[..n].starts_with(b"HTTP/1.1 200 OK"),
        "viewer slot was not released"
    );
    stop_server(server).await;
}

#[tokio::test]
async fn live_http_push_and_direct_connections_are_snapshotted_and_removed() {
    let mut config = AppConfig::default();
    config.runtime.channels.push(ChannelConfig {
        channel_id: CHANNEL_ID.into(),
        container_type: "RAW".into(),
        source: ChannelSource::HttpPush {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            auth_token: None,
        },
        ..ChannelConfig::default()
    });
    let server = start_server(config).await;
    let runtime_channel = server.runtime.channel(CHANNEL_ID).unwrap();

    let mut source = TcpStream::connect(server.runtime.ingest_addr(CHANNEL_ID).await.unwrap())
        .await
        .unwrap();
    let source_endpoint = source.local_addr().unwrap().to_string();
    source
        .write_all(b"POST / HTTP/1.1\r\n\r\nDATA")
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = runtime_channel.snapshot().await;
            if snapshot.available
                && snapshot.connections.iter().any(|connection| {
                    connection.kind == "ingest"
                        && connection.endpoint == source_endpoint
                        && connection.status == "connected"
                        && connection.received_bytes > 0
                })
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("ingest connection should appear with received bytes");

    let mut viewer = TcpStream::connect(server.addr).await.unwrap();
    let viewer_endpoint = viewer.local_addr().unwrap().to_string();
    viewer
        .write_all(b"GET /stream/0123456789abcdef0123456789abcdef HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    let mut response = [0u8; 2048];
    let n = tokio::time::timeout(Duration::from_secs(2), viewer.read(&mut response))
        .await
        .expect("direct viewer should receive data")
        .unwrap();
    assert!(response[..n].starts_with(b"HTTP/1.1 200 OK"));

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = runtime_channel.snapshot().await;
            if snapshot.connections.iter().any(|connection| {
                connection.kind == "direct"
                    && connection.endpoint == viewer_endpoint
                    && connection.status == "connected"
                    && connection.sent_bytes > 0
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("direct connection should appear with sent bytes");

    drop(viewer);
    drop(source);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if runtime_channel.snapshot().await.connections.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("closed source and viewer connections should be removed");
    stop_server(server).await;
}

struct RunningServer {
    addr: SocketAddr,
    runtime: Arc<peercast_app::runtime::AppRuntime>,
    shutdown_tx: broadcast::Sender<ShutdownReason>,
    server_task: tokio::task::JoinHandle<Result<(), String>>,
    tracker: TaskTracker,
}

async fn start_server(mut config: AppConfig) -> RunningServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    config.server.port = addr.port();
    config.server.bind_address = "127.0.0.1".to_string();

    let tracker = TaskTracker::new();
    let server = HttpServer::new(
        &config,
        RpcState::default(),
        Arc::new(AppMetrics::new()),
        Arc::new(BoundedLogBuffer::default()),
        tracker.clone(),
    )
    .unwrap();
    let runtime = server.runtime();
    runtime.start().await.unwrap();

    let (shutdown_tx, shutdown_rx) = broadcast::channel(4);
    let server_task = tokio::spawn(async move {
        server
            .run_on_listener(listener, shutdown_rx)
            .await
            .map_err(|error| error.to_string())
    });
    RunningServer {
        addr,
        runtime,
        shutdown_tx,
        server_task,
        tracker,
    }
}

async fn stop_server(server: RunningServer) {
    server.runtime.shutdown().await;
    let _ = server.shutdown_tx.send(ShutdownReason::AdminRpc);
    tokio::time::timeout(Duration::from_secs(2), server.server_task)
        .await
        .expect("HTTP server should stop")
        .unwrap()
        .unwrap();
    server
        .tracker
        .wait_for_drain(Duration::from_secs(2))
        .await
        .expect("runtime tasks should drain");
}

fn relay_channel(endpoint: SocketAddr) -> ChannelConfig {
    ChannelConfig {
        channel_id: CHANNEL_ID.to_string(),
        name: "Runtime Replay Test".to_string(),
        container_type: "RAW".to_string(),
        source: ChannelSource::Relay { endpoint },
        ..ChannelConfig::default()
    }
}

async fn http_response(addr: SocketAddr, request: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response))
        .await
        .expect("HTTP response should finish")
        .unwrap();
    response
}

async fn next_relay_event(
    state: &mut RelayServerStateMachine,
    socket: &mut TcpStream,
    buffer: &mut BytesMut,
) -> io::Result<RelayServerEvent> {
    let mut temp = [0u8; 4096];
    loop {
        if !buffer.is_empty() {
            let mut frozen = buffer.clone().freeze();
            let original_len = frozen.len();
            let event = state
                .feed_bytes(&mut frozen)
                .map_err(|error| io::Error::other(error.to_string()))?;
            let consumed = original_len - frozen.len();
            if consumed > 0 {
                buffer.advance(consumed);
            }
            if let Some(event) = event {
                return Ok(event);
            }
        }
        let n = socket.read(&mut temp).await?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "relay closed"));
        }
        buffer.extend_from_slice(&temp[..n]);
    }
}

async fn write_channel_packet(
    socket: &mut TcpStream,
    channel_id: ChannelId,
    stream_type: peercast_core::id4::ID4,
    pos: i64,
    payload: PacketPayload,
) {
    let packet = ChannelPacket {
        channel_id: Some(channel_id),
        stream_type,
        pos,
        cont: ContinuationFlags::NONE,
        payload,
        unknown_extensions: Vec::new(),
    };
    for atom in packet.to_atoms().unwrap() {
        let mut encoded = Vec::new();
        atom.encode(&mut encoded).unwrap();
        socket.write_all(&encoded).await.unwrap();
    }
}

async fn relay_handshake(socket: &mut TcpStream, peer_addr: SocketAddr) -> RelayServerStateMachine {
    let server_sid = ChannelId::from_bytes([0x51; 16]);
    let mut state =
        RelayServerStateMachine::new(server_sid, RelayNetworkProfile::IPv4, Some(peer_addr.ip()));
    let mut buffer = BytesMut::with_capacity(8192);
    match next_relay_event(&mut state, socket, &mut buffer)
        .await
        .unwrap()
    {
        RelayServerEvent::RequestReceived(request) => {
            assert_eq!(request.channel_id.to_hex_lower(), CHANNEL_ID);
        }
        other => panic!("expected relay request, got {other:?}"),
    }
    socket.write_all(&state.accept().unwrap()).await.unwrap();
    match next_relay_event(&mut state, socket, &mut buffer)
        .await
        .unwrap()
    {
        RelayServerEvent::HandshakeCompleted { outbound_bytes, .. } => {
            socket.write_all(&outbound_bytes).await.unwrap();
        }
        other => panic!("expected relay handshake, got {other:?}"),
    }
    state
}

#[tokio::test]
async fn relay_live_stream_replays_byte_and_time_bounded_content_over_http() {
    let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay_listener.local_addr().unwrap();
    let mut config = AppConfig::default();
    config.server.ringbuffer_max_bytes = 16;
    config.server.ringbuffer_window_secs = 1;
    config.runtime.channels.push(relay_channel(relay_addr));
    let running = start_server(config).await;

    let (initial_packets_tx, initial_packets_rx) = oneshot::channel();
    let (send_final_tx, send_final_rx) = oneshot::channel();
    let (final_packet_tx, final_packet_rx) = oneshot::channel();
    let (close_relay_tx, close_relay_rx) = oneshot::channel();
    let relay_task = tokio::spawn(async move {
        let (mut socket, peer_addr) = relay_listener.accept().await.unwrap();
        let _state = relay_handshake(&mut socket, peer_addr).await;
        let channel_id = ChannelId::from_hex(CHANNEL_ID).unwrap();
        write_channel_packet(
            &mut socket,
            channel_id,
            ID4::PKT_TYPE_HEAD,
            0,
            PacketPayload::Header(Bytes::from_static(b"runtime-header")),
        )
        .await;
        for (pos, payload) in [(0, b"AAAAAAAA"), (8, b"BBBBBBBB"), (16, b"CCCCCCCC")] {
            write_channel_packet(
                &mut socket,
                channel_id,
                ID4::PKT_TYPE_DATA,
                pos,
                PacketPayload::Data(Bytes::from_static(payload)),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        let _ = initial_packets_tx.send(());
        let _ = send_final_rx.await;
        write_channel_packet(
            &mut socket,
            channel_id,
            ID4::PKT_TYPE_DATA,
            24,
            PacketPayload::Data(Bytes::from_static(b"DDDDDDDD")),
        )
        .await;
        let _ = final_packet_tx.send(());
        let _ = close_relay_rx.await;
    });

    tokio::time::timeout(Duration::from_secs(2), initial_packets_rx)
        .await
        .expect("relay should send initial packets")
        .unwrap();
    let runtime_channel = running.runtime.channel(CHANNEL_ID).unwrap();
    let initial_snapshot = runtime_channel.snapshot().await;
    assert_eq!(initial_snapshot.buffered_bytes, 16);
    assert_eq!(initial_snapshot.buffered_packets, 2);
    assert_eq!(initial_snapshot.oldest_pos, Some(8));
    assert_eq!(initial_snapshot.newest_pos, Some(16));

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let _ = send_final_tx.send(());
    tokio::time::timeout(Duration::from_secs(2), final_packet_rx)
        .await
        .expect("relay should send the final packet")
        .unwrap();
    let time_limited_snapshot = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = runtime_channel.snapshot().await;
            if snapshot.newest_pos == Some(24) {
                break snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("final relay packet should reach the ring buffer");
    assert_eq!(time_limited_snapshot.buffered_bytes, 8);
    assert_eq!(time_limited_snapshot.oldest_pos, Some(24));

    let id_upper = CHANNEL_ID.to_ascii_uppercase();
    let head = http_response(
        running.addr,
        format!("HEAD /stream/{id_upper}.bin HTTP/1.0\r\n\r\n").as_bytes(),
    )
    .await;
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.0 200 OK"));
    let playlist = http_response(
        running.addr,
        format!("GET /pls/{id_upper}.pls HTTP/1.0\r\n\r\n").as_bytes(),
    )
    .await;
    assert!(String::from_utf8_lossy(&playlist).starts_with("HTTP/1.1 200 OK"));
    assert!(String::from_utf8_lossy(&playlist).contains("Runtime Replay Test"));

    let mut playback = TcpStream::connect(running.addr).await.unwrap();
    playback
        .write_all(format!("GET /stream/{id_upper}.bin HTTP/1.0\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut response = [0u8; 256];
    let n = tokio::time::timeout(Duration::from_secs(2), playback.read(&mut response))
        .await
        .expect("late subscriber should receive replay")
        .unwrap();
    assert!(String::from_utf8_lossy(&response[..n]).starts_with("HTTP/1.0 200 OK"));
    let _ = close_relay_tx.send(());
    let mut tail = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), playback.read_to_end(&mut tail))
        .await
        .expect("stream should finish after relay closes")
        .unwrap();
    let mut whole_response = response[..n].to_vec();
    whole_response.extend_from_slice(&tail);
    let response_text = String::from_utf8_lossy(&whole_response);
    let body_start = whole_response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap()
        + 4;
    let body = &whole_response[body_start..];
    assert!(body
        .windows(b"runtime-header".len())
        .any(|w| w == b"runtime-header"));
    assert!(body.windows(8).any(|w| w == b"DDDDDDDD"));
    assert!(!body.windows(8).any(|w| w == b"AAAAAAAA"));
    assert!(!body.windows(8).any(|w| w == b"BBBBBBBB"));
    assert!(!body.windows(8).any(|w| w == b"CCCCCCCC"));
    assert!(response_text.contains("HTTP/1.0 200 OK"));

    relay_task.await.unwrap();
    stop_server(running).await;
}

#[tokio::test]
async fn relay_retries_use_configured_short_read_timeout_and_backoff() {
    let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay_listener.local_addr().unwrap();
    let mut config = AppConfig::default();
    config.runtime.reconnect.max_attempts_per_cycle = 2;
    config.runtime.reconnect.base_backoff_ms = 20;
    config.runtime.reconnect.max_backoff_ms = 40;
    config.runtime.reconnect.connect_timeout_ms = 200;
    config.runtime.reconnect.read_timeout_ms = 60;
    config.runtime.channels.push(relay_channel(relay_addr));
    let running = start_server(config).await;

    let (retry_tx, retry_rx) = oneshot::channel();
    let relay_task = tokio::spawn(async move {
        let first_at = Instant::now();
        let (_first, _) = relay_listener.accept().await.unwrap();
        let (second, _) = tokio::time::timeout(Duration::from_millis(700), relay_listener.accept())
            .await
            .expect("configured read timeout and retry should create a second connection")
            .unwrap();
        let _ = retry_tx.send(first_at.elapsed());
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(second);
    });

    let elapsed = tokio::time::timeout(Duration::from_secs(1), retry_rx)
        .await
        .expect("relay should retry")
        .unwrap();
    assert!(
        elapsed >= Duration::from_millis(50),
        "first attempt should honor the configured read timeout: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(700),
        "custom short retry budget should beat default timeout: {elapsed:?}"
    );
    assert!(
        running
            .runtime
            .channel(CHANNEL_ID)
            .unwrap()
            .snapshot()
            .await
            .connection_attempts
            >= 2
    );

    relay_task.await.unwrap();
    stop_server(running).await;
}

async fn read_atom(socket: &mut TcpStream, buffer: &mut BytesMut) -> io::Result<Atom> {
    let mut temp = [0u8; 4096];
    loop {
        if !buffer.is_empty() {
            let mut bytes = buffer.clone().freeze();
            let original_len = bytes.len();
            if let Ok(Some(atom)) = Atom::decode(&mut bytes, &AtomBudget::default()) {
                buffer.advance(original_len - bytes.len());
                return Ok(atom);
            }
        }
        let n = socket.read(&mut temp).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "YP peer closed",
            ));
        }
        buffer.extend_from_slice(&temp[..n]);
    }
}

async fn write_atom(socket: &mut TcpStream, atom: Atom) {
    let mut bytes = BytesMut::new();
    atom.encode(&mut bytes).unwrap();
    socket.write_all(&bytes).await.unwrap();
}

fn helo_port(atom: &Atom) -> u16 {
    atom.children()
        .unwrap()
        .iter()
        .find(|child| child.name() == ID4::HELO_PORT)
        .unwrap()
        .get_u16_le()
        .unwrap()
}

#[tokio::test]
async fn loopback_yp_runtime_retries_announces_periodically_and_advertises_bound_port() {
    let yp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let yp_addr = yp_listener.local_addr().unwrap();
    let push_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    let (retry_elapsed_tx, retry_elapsed_rx) = oneshot::channel();
    let (announcements_tx, announcements_rx) = oneshot::channel();
    let yp_task = tokio::spawn(async move {
        let started = Instant::now();
        let (mut first, _) = yp_listener.accept().await.unwrap();
        let mut first_buf = BytesMut::new();
        assert_eq!(
            read_atom(&mut first, &mut first_buf).await.unwrap().name(),
            ID4::PCP_CONNECT
        );
        let first_helo = read_atom(&mut first, &mut first_buf).await.unwrap();
        let first_port = helo_port(&first_helo);
        drop(first);

        let (mut second, _) =
            tokio::time::timeout(Duration::from_millis(700), yp_listener.accept())
                .await
                .expect("runtime should retry the failed YP connection")
                .unwrap();
        let elapsed = started.elapsed();
        let mut second_buf = BytesMut::new();
        assert_eq!(
            read_atom(&mut second, &mut second_buf)
                .await
                .unwrap()
                .name(),
            ID4::PCP_CONNECT
        );
        let second_helo = read_atom(&mut second, &mut second_buf).await.unwrap();
        let second_port = helo_port(&second_helo);

        write_atom(
            &mut second,
            Atom::new_parent(
                ID4::PCP_OLEH,
                vec![
                    Atom::new_leaf(ID4::HELO_SESSIONID, Bytes::from(vec![0x77; 16])),
                    Atom::new_u8(ID4::HELO_DISABLE, 0),
                    Atom::new_u16_le(ID4::HELO_PORT, 7144),
                ],
            ),
        )
        .await;

        let first_announcement = read_atom(&mut second, &mut second_buf).await.unwrap();
        let first_at = Instant::now();
        let second_announcement = tokio::time::timeout(
            Duration::from_millis(2200),
            read_atom(&mut second, &mut second_buf),
        )
        .await
        .expect("runtime should send a periodic YP announcement")
        .unwrap();
        let periodic_elapsed = first_at.elapsed();
        let _ = retry_elapsed_tx.send((elapsed, first_port, second_port, periodic_elapsed));
        let _ = announcements_tx.send((first_announcement, second_announcement));

        loop {
            let atom = match read_atom(&mut second, &mut second_buf).await {
                Ok(atom) => atom,
                Err(_) => break,
            };
            if atom.name() == ID4::PCP_QUIT {
                break;
            }
        }
    });

    let mut config = AppConfig::default();
    let channel = ChannelConfig {
        channel_id: CHANNEL_ID.to_string(),
        name: "YP Runtime Test Radio".to_string(),
        container_type: "RAW".to_string(),
        broadcast_id: Some(BROADCAST_ID.to_string()),
        metadata: ChannelMetadata {
            genre: "Test Genre".into(),
            description: "Test Description".into(),
            comment: "Test Comment".into(),
            contact_url: "https://example.invalid/contact".into(),
            bitrate: 192,
        },
        source: ChannelSource::HttpPush {
            bind_address: push_addr,
            auth_token: Some("local-test-token".to_string()),
        },
    };
    config.runtime.channels.push(channel);
    config.runtime.yellow_pages.push(YellowPageSettings {
        endpoint: yp_addr,
        update_interval_secs: 1,
        retry_base_ms: 20,
        retry_max_ms: 40,
        io_timeout_ms: 500,
        allow_public_announcements: false,
        ..YellowPageSettings::default()
    });
    let running = start_server(config).await;

    let (elapsed, first_port, second_port, periodic_elapsed) =
        tokio::time::timeout(Duration::from_secs(4), retry_elapsed_rx)
            .await
            .expect("YP retry and periodic update should complete")
            .unwrap();
    assert!(
        elapsed < Duration::from_millis(700),
        "custom YP retry delay should be used: {elapsed:?}"
    );
    assert_eq!(first_port, running.addr.port());
    assert_eq!(second_port, running.addr.port());
    assert!(
        periodic_elapsed >= Duration::from_millis(800),
        "custom one-second period should be honored: {periodic_elapsed:?}"
    );

    let (first_announcement, second_announcement) = announcements_rx.await.unwrap();
    assert_eq!(first_announcement.name(), ID4::PCP_BCST);
    assert_eq!(second_announcement.name(), ID4::PCP_BCST);
    for announcement in [first_announcement, second_announcement] {
        let channel_id = announcement
            .children()
            .unwrap()
            .iter()
            .find(|child| child.name() == ID4::BCST_CID)
            .unwrap()
            .leaf_bytes()
            .unwrap();
        assert_eq!(
            &channel_id[..],
            &ChannelId::from_hex(CHANNEL_ID).unwrap().as_bytes()[..]
        );
        let chan = announcement
            .children()
            .unwrap()
            .iter()
            .find(|child| child.name() == ID4::PCP_CHAN)
            .unwrap();
        let info = chan
            .children()
            .unwrap()
            .iter()
            .find(|child| child.name() == ID4::CHAN_INFO)
            .unwrap();
        let info = info.children().unwrap();
        for (id, expected) in [
            (ID4::INFO_NAME, "YP Runtime Test Radio"),
            (ID4::INFO_GENRE, "Test Genre"),
            (ID4::INFO_DESC, "Test Description"),
            (ID4::INFO_COMMENT, "Test Comment"),
            (ID4::INFO_URL, "https://example.invalid/contact"),
        ] {
            assert_eq!(
                info.iter()
                    .find(|child| child.name() == id)
                    .unwrap()
                    .get_string()
                    .unwrap(),
                expected
            );
        }
        assert_eq!(
            info.iter()
                .find(|child| child.name() == ID4::INFO_BITR)
                .unwrap()
                .get_u32_le()
                .unwrap(),
            192
        );
    }

    stop_server(running).await;
    tokio::time::timeout(Duration::from_secs(2), yp_task)
        .await
        .expect("fake YP peer should observe graceful shutdown")
        .unwrap();
}

fn announcement_positions(atom: &Atom) -> (Option<u32>, Option<u32>) {
    let host = atom
        .children()
        .unwrap()
        .iter()
        .find(|child| child.name() == ID4::PCP_HOST)
        .unwrap();
    let children = host.children().unwrap();
    (
        children
            .iter()
            .find(|child| child.name() == ID4::HOST_OLDPOS)
            .and_then(|child| child.get_u32_le().ok()),
        children
            .iter()
            .find(|child| child.name() == ID4::HOST_NEWPOS)
            .and_then(|child| child.get_u32_le().ok()),
    )
}

#[tokio::test]
async fn yp_periodic_announcement_refreshes_ring_positions_after_more_push_data() {
    let yp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let yp_addr = yp_listener.local_addr().unwrap();
    let (push_ready_tx, push_ready_rx) = oneshot::channel();
    let (first_announcement_tx, first_announcement_rx) = oneshot::channel();
    let yp_task = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), yp_listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut buffered = BytesMut::new();
        assert_eq!(
            read_atom(&mut socket, &mut buffered).await.unwrap().name(),
            ID4::PCP_CONNECT
        );
        assert_eq!(
            read_atom(&mut socket, &mut buffered).await.unwrap().name(),
            ID4::PCP_HELO
        );
        push_ready_rx.await.unwrap();
        write_atom(
            &mut socket,
            Atom::new_parent(
                ID4::PCP_OLEH,
                vec![
                    Atom::new_leaf(ID4::HELO_SESSIONID, Bytes::from(vec![0x55; 16])),
                    Atom::new_u8(ID4::HELO_DISABLE, 0),
                    Atom::new_u16_le(ID4::HELO_PORT, 7144),
                ],
            ),
        )
        .await;
        let initial = read_atom(&mut socket, &mut buffered).await.unwrap();
        let initial_positions = announcement_positions(&initial);
        first_announcement_tx.send(initial_positions).unwrap();
        let periodic = tokio::time::timeout(
            Duration::from_millis(2200),
            read_atom(&mut socket, &mut buffered),
        )
        .await
        .expect("periodic YP update should arrive")
        .unwrap();
        (initial_positions, announcement_positions(&periodic))
    });

    let mut config = AppConfig::default();
    config.server.ringbuffer_max_bytes = 4;
    config.runtime.channels.push(ChannelConfig {
        channel_id: CHANNEL_ID.into(),
        broadcast_id: Some(BROADCAST_ID.into()),
        name: "YP buffer position test".into(),
        container_type: "RAW".into(),
        metadata: ChannelMetadata::default(),
        source: ChannelSource::HttpPush {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            auth_token: None,
        },
    });
    config.runtime.yellow_pages.push(YellowPageSettings {
        endpoint: yp_addr,
        update_interval_secs: 1,
        retry_base_ms: 20,
        retry_max_ms: 40,
        io_timeout_ms: 500,
        allow_public_announcements: false,
        ..YellowPageSettings::default()
    });
    let server = start_server(config).await;
    let mut push = TcpStream::connect(server.runtime.ingest_addr(CHANNEL_ID).await.unwrap())
        .await
        .unwrap();
    push.write_all(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nAAAA\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = server.runtime.channel(CHANNEL_ID).unwrap().snapshot().await;
            if snapshot.newest_pos == Some(0) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first push chunk should reach the ring");
    push_ready_tx.send(()).unwrap();
    let initial_positions = tokio::time::timeout(Duration::from_secs(2), first_announcement_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial_positions, (Some(0), Some(0)));

    push.write_all(b"4\r\nBBBB\r\n").await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = server.runtime.channel(CHANNEL_ID).unwrap().snapshot().await;
            if snapshot.oldest_pos == Some(4) && snapshot.newest_pos == Some(4) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("second push chunk should replace old ring content");

    let (initial, periodic) = tokio::time::timeout(Duration::from_secs(3), yp_task)
        .await
        .expect("YP updates should finish")
        .unwrap();
    assert_eq!(initial, (Some(0), Some(0)));
    assert_eq!(periodic, (Some(4), Some(4)));
    drop(push);
    stop_server(server).await;
}

async fn next_client_event(
    socket: &mut TcpStream,
    client: &mut RelayClientStateMachine,
    pending: &mut BytesMut,
) -> RelayEvent {
    let mut temp = [0u8; 4096];
    loop {
        if !pending.is_empty() {
            let mut bytes = pending.clone().freeze();
            let before = bytes.len();
            let event = client.feed_bytes(&mut bytes).unwrap();
            let consumed = before - bytes.len();
            if consumed > 0 {
                pending.advance(consumed);
            }
            if let Some(event) = event {
                return event;
            }
            if consumed > 0 {
                continue;
            }
        }
        let n = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut temp))
            .await
            .expect("PCP response should arrive")
            .unwrap();
        assert!(n > 0, "PCP peer closed before expected event");
        pending.extend_from_slice(&temp[..n]);
    }
}

async fn connect_relay_client(addr: SocketAddr) -> (TcpStream, RelayClientStateMachine, BytesMut) {
    let mut socket = TcpStream::connect(addr).await.unwrap();
    let channel_id = ChannelId::from_hex(CHANNEL_ID).unwrap();
    let client_sid = ChannelId::from_bytes([0x33; 16]);
    let mut client =
        RelayClientStateMachine::new(channel_id, client_sid, RelayNetworkProfile::IPv4, 0);
    let request = client.build_relay_request(&addr.to_string(), "runtime-test");
    socket.write_all(&request).await.unwrap();
    let pending = BytesMut::new();
    (socket, client, pending)
}

#[tokio::test]
async fn http_push_is_streamed_as_pcp_and_relay_capacity_rejects_second_client() {
    use peercast_pcp::relay_client::RelayClientPhase;

    let mut config = AppConfig::default();
    config.server.max_relays_per_channel = 1;
    config.server.max_relays = 1;
    config.runtime.channels.push(ChannelConfig {
        channel_id: CHANNEL_ID.into(),
        broadcast_id: Some(BROADCAST_ID.into()),
        name: "live push PCP test".into(),
        container_type: "RAW".into(),
        metadata: ChannelMetadata {
            genre: "Live Genre".into(),
            description: "Live Description".into(),
            comment: "Live Comment".into(),
            contact_url: "https://example.invalid/live".into(),
            bitrate: 256,
        },
        source: ChannelSource::HttpPush {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            auth_token: None,
        },
    });
    let server = start_server(config).await;
    let ingest_addr = server.runtime.ingest_addr(CHANNEL_ID).await.unwrap();
    let mut push = TcpStream::connect(ingest_addr).await.unwrap();
    push.write_all(
        b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Type: application/octet-stream\r\n\r\n4\r\nDATA\r\n",
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if server
                .runtime
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
    .expect("push stream should become available");

    let (mut first, mut first_client, mut first_pending) = connect_relay_client(server.addr).await;
    assert_eq!(
        next_client_event(&mut first, &mut first_client, &mut first_pending).await,
        RelayEvent::Connected200
    );
    first
        .write_all(&first_client.build_helo("runtime-test", None).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        next_client_event(&mut first, &mut first_client, &mut first_pending).await,
        RelayEvent::HandshakeCompleted { .. }
    ));
    let channel_info =
        match next_client_event(&mut first, &mut first_client, &mut first_pending).await {
            RelayEvent::ChannelInfo(atom) => atom,
            event => panic!("expected channel metadata, got {event:?}"),
        };
    let announced_id = channel_info
        .find_child(ID4::CHAN_ID)
        .expect("channel metadata should include its channel ID");
    assert_eq!(
        announced_id.leaf_bytes().unwrap().as_ref(),
        ChannelId::from_hex(CHANNEL_ID).unwrap().as_bytes()
    );
    let info = channel_info
        .find_child(ID4::CHAN_INFO)
        .expect("channel metadata should include channel info");
    assert_eq!(
        info.find_child(ID4::INFO_NAME)
            .unwrap()
            .get_string()
            .unwrap(),
        "live push PCP test"
    );
    for (id, expected) in [
        (ID4::INFO_GENRE, "Live Genre"),
        (ID4::INFO_DESC, "Live Description"),
        (ID4::INFO_COMMENT, "Live Comment"),
        (ID4::INFO_URL, "https://example.invalid/live"),
    ] {
        assert_eq!(info.find_child(id).unwrap().get_string().unwrap(), expected);
    }
    assert_eq!(
        info.find_child(ID4::INFO_BITR)
            .unwrap()
            .get_u32_le()
            .unwrap(),
        256
    );
    assert_eq!(
        info.find_child(ID4::INFO_TYPE)
            .unwrap()
            .get_string()
            .unwrap(),
        "RAW"
    );
    assert!(
        matches!(next_client_event(&mut first, &mut first_client, &mut first_pending).await, RelayEvent::HeaderChunk(header) if header.data.is_empty())
    );
    assert!(
        matches!(next_client_event(&mut first, &mut first_client, &mut first_pending).await, RelayEvent::MediaChunk(chunk) if chunk.data.as_ref() == b"DATA")
    );

    let (mut second, mut second_client, mut second_pending) =
        connect_relay_client(server.addr).await;
    assert_eq!(
        next_client_event(&mut second, &mut second_client, &mut second_pending).await,
        RelayEvent::Unavailable503
    );
    second
        .write_all(&second_client.build_helo("runtime-test", None).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        next_client_event(&mut second, &mut second_client, &mut second_pending).await,
        RelayEvent::HandshakeCompleted { .. }
    ));
    assert_eq!(
        next_client_event(&mut second, &mut second_client, &mut second_pending).await,
        RelayEvent::Quit(peercast_core::errors::QuitReason::UNAVAILABLE)
    );
    assert_eq!(
        second_client.phase(),
        RelayClientPhase::Terminal(peercast_core::errors::QuitReason::UNAVAILABLE)
    );

    drop(second);
    drop(first);
    drop(push);
    stop_server(server).await;
}

#[tokio::test]
async fn unknown_relay_channel_returns_404_without_atom_decoding() {
    let server = start_server(AppConfig::default()).await;
    let missing_id = "fedcba9876543210fedcba9876543210";
    let mut socket = TcpStream::connect(server.addr).await.unwrap();
    let request = format!(
        "GET /channel/{missing_id} HTTP/1.0\r\nHost: {}\r\nx-peercast-pcp: 1\r\n\r\n",
        server.addr
    );
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.0 404 Not Found\r\n"));
    assert!(
        response.ends_with(b"\r\n\r\n"),
        "404 must contain no PCP Atom bytes"
    );
    stop_server(server).await;
}

async fn verify_reverse_probe_port(session_matches: bool) {
    use peercast_core::errors::QuitReason;

    let probe_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let probe_port = probe_listener.local_addr().unwrap().port();
    let expected_sid = ChannelId::from_bytes([0x33; 16]);
    let actual_sid = if session_matches {
        expected_sid
    } else {
        ChannelId::from_bytes([0x44; 16])
    };
    let probe_task = tokio::spawn(async move {
        use peercast_core::atom::AtomBudget;
        let (mut socket, _) = probe_listener.accept().await.unwrap();
        let mut buffered = BytesMut::new();
        let mut temp = [0u8; 1024];
        let mut atoms = Vec::new();
        let budget = AtomBudget::default();
        while atoms.len() < 2 {
            let n = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut temp))
                .await
                .expect("probe request should arrive")
                .unwrap();
            assert!(n > 0);
            buffered.extend_from_slice(&temp[..n]);
            let mut bytes = buffered.clone().freeze();
            while let Some(atom) = Atom::decode(&mut bytes, &budget).unwrap() {
                atoms.push(atom);
            }
            let consumed = buffered.len() - bytes.len();
            if consumed > 0 {
                buffered.advance(consumed);
            }
        }
        assert_eq!(atoms[0].name(), ID4::PCP_CONNECT);
        assert_eq!(atoms[1].name(), ID4::PCP_HELO);
        let oleh = Atom::new_parent(
            ID4::PCP_OLEH,
            vec![
                Atom::new_string(ID4::HELO_AGENT, "probe-test"),
                Atom::new_leaf(
                    ID4::HELO_SESSIONID,
                    Bytes::copy_from_slice(actual_sid.as_bytes()),
                ),
            ],
        );
        socket.write_all(&oleh.to_bytes().unwrap()).await.unwrap();
        let mut quit_bytes = BytesMut::new();
        loop {
            let n = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut temp))
                .await
                .expect("probe quit should arrive")
                .unwrap();
            assert!(n > 0);
            quit_bytes.extend_from_slice(&temp[..n]);
            let mut bytes = quit_bytes.clone().freeze();
            if let Some(quit) = Atom::decode(&mut bytes, &budget).unwrap() {
                assert_eq!(quit.name(), ID4::PCP_QUIT);
                assert_eq!(quit.get_u32_le().unwrap(), QuitReason::NORMAL.raw());
                break;
            }
        }
    });

    let mut config = AppConfig::default();
    config.runtime.channels.push(ChannelConfig {
        channel_id: CHANNEL_ID.into(),
        broadcast_id: Some(BROADCAST_ID.into()),
        name: "reverse probe test".into(),
        container_type: "RAW".into(),
        metadata: ChannelMetadata::default(),
        source: ChannelSource::HttpPush {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            auth_token: None,
        },
    });
    let server = start_server(config).await;
    let mut push = TcpStream::connect(server.runtime.ingest_addr(CHANNEL_ID).await.unwrap())
        .await
        .unwrap();
    push.write_all(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nDATA\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !server
            .runtime
            .channel(CHANNEL_ID)
            .unwrap()
            .snapshot()
            .await
            .available
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let (mut relay, mut client, mut pending) = connect_relay_client(server.addr).await;
    assert_eq!(
        next_client_event(&mut relay, &mut client, &mut pending).await,
        RelayEvent::Connected200
    );
    relay
        .write_all(&client.build_helo("probe-test", Some(probe_port)).unwrap())
        .await
        .unwrap();
    let event = next_client_event(&mut relay, &mut client, &mut pending).await;
    match event {
        RelayEvent::HandshakeCompleted { remote_port, .. } => {
            assert_eq!(
                remote_port,
                Some(if session_matches { probe_port } else { 0 })
            );
        }
        other => panic!("expected verified OLEH handshake, received {other:?}"),
    }

    let snapshot = server.runtime.channel(CHANNEL_ID).unwrap().snapshot().await;
    let child = snapshot
        .connections
        .iter()
        .find(|entry| entry.kind == "relay")
        .expect("accepted relay must be visible");
    assert_eq!(child.status, "connected");
    assert_eq!(child.port_open, Some(session_matches));
    assert!(child.session_id.is_some());

    drop(relay);
    drop(push);
    stop_server(server).await;
    tokio::time::timeout(Duration::from_secs(2), probe_task)
        .await
        .expect("probe task should finish")
        .unwrap();
}

#[tokio::test]
async fn reverse_probe_only_advertises_port_when_session_id_matches() {
    verify_reverse_probe_port(true).await;
    verify_reverse_probe_port(false).await;
}

#[tokio::test]
async fn reverse_probe_on_shared_listener_survives_short_relay_read_timeout() {
    let mut parent_config = AppConfig::default();
    parent_config.server.max_relays_per_channel = 1;
    parent_config.runtime.channels.push(ChannelConfig {
        channel_id: CHANNEL_ID.into(),
        name: "reverse probe relay parent".into(),
        container_type: "RAW".into(),
        source: ChannelSource::HttpPush {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            auth_token: None,
        },
        ..ChannelConfig::default()
    });
    let parent = start_server(parent_config).await;
    let mut push = TcpStream::connect(parent.runtime.ingest_addr(CHANNEL_ID).await.unwrap())
        .await
        .unwrap();
    push.write_all(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nDATA\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !parent
            .runtime
            .channel(CHANNEL_ID)
            .unwrap()
            .snapshot()
            .await
            .available
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("parent push should become available");

    let mut child_config = AppConfig::default();
    child_config.runtime.reconnect.read_timeout_ms = 300;
    child_config.runtime.channels.push(ChannelConfig {
        channel_id: CHANNEL_ID.into(),
        name: "reverse probe relay child".into(),
        container_type: "RAW".into(),
        source: ChannelSource::Relay {
            endpoint: parent.addr,
        },
        ..ChannelConfig::default()
    });
    let child = start_server(child_config).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if child
                .runtime
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
    .expect("child should receive data despite its short relay read timeout");
    let snapshot = child.runtime.channel(CHANNEL_ID).unwrap().snapshot().await;
    let upstream = snapshot
        .connections
        .iter()
        .find(|entry| entry.kind == "source")
        .expect("connected upstream must be visible");
    assert_eq!(upstream.endpoint, parent.addr.to_string());
    assert_eq!(upstream.status, "connected");
    assert!(upstream.received_bytes > 0);
    assert_eq!(child.runtime.network_snapshot().ipv4, "closed");
    assert_eq!(
        child.runtime.network_snapshot().observed_ipv4.as_deref(),
        Some("127.0.0.1")
    );

    let mut playback = TcpStream::connect(child.addr).await.unwrap();
    playback
        .write_all(format!("GET /stream/{CHANNEL_ID} HTTP/1.0\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !response.windows(4).any(|window| window == b"DATA") {
            let mut chunk = [0u8; 128];
            let n = playback.read(&mut chunk).await.unwrap();
            assert!(n > 0, "playback ended before pushed data arrived");
            response.extend_from_slice(&chunk[..n]);
        }
    })
    .await
    .expect("child HTTP playback should include pushed data within two seconds");
    assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.0 200 OK"));

    drop(playback);
    drop(push);
    stop_server(child).await;
    stop_server(parent).await;
}

#[tokio::test]
async fn shared_listener_handles_fragmented_probe_and_rejects_invalid_probe_atoms() {
    use peercast_core::atom::AtomBudget;
    use peercast_pcp::probe::ProbeCodec;

    let server = start_server(AppConfig::default()).await;
    let session_id = ChannelId::from_bytes([0x33; 16]);
    let init = ProbeCodec::build_probe_init(session_id, "fragmented-probe-test").unwrap();
    let mut probe = TcpStream::connect(server.addr).await.unwrap();
    for fragment in init.chunks(2) {
        probe.write_all(fragment).await.unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let mut response = BytesMut::new();
    let mut chunk = [0u8; 64];
    let pong = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let n = probe.read(&mut chunk).await.unwrap();
            assert!(n > 0, "probe listener closed before OLEH");
            response.extend_from_slice(&chunk[..n]);
            let mut bytes = response.clone().freeze();
            if let Some(atom) = Atom::decode(&mut bytes, &AtomBudget::default()).unwrap() {
                assert_eq!(atom.name(), ID4::PCP_OLEH);
                let response_sid = atom
                    .find_child(ID4::HELO_SESSIONID)
                    .and_then(Atom::leaf_bytes)
                    .expect("OLEH should contain a session ID");
                assert_eq!(response_sid.len(), 16);
                break atom;
            }
        }
    })
    .await
    .expect("fragmented probe should receive OLEH");
    assert_eq!(pong.name(), ID4::PCP_OLEH);
    drop(probe);

    let mut invalid = Vec::new();
    invalid.push(
        ProbeCodec::build_probe_init_with_version(2, session_id, "unsupported-version").unwrap(),
    );

    let mut missing_sid = Atom::new_u32_le(ID4::PCP_CONNECT, 1)
        .to_bytes()
        .unwrap()
        .to_vec();
    missing_sid.extend_from_slice(
        &Atom::new_parent(
            ID4::PCP_HELO,
            vec![Atom::new_string(ID4::HELO_AGENT, "missing-sid")],
        )
        .to_bytes()
        .unwrap(),
    );
    invalid.push(Bytes::from(missing_sid));

    let mut oversized = Atom::new_u32_le(ID4::PCP_CONNECT, 1)
        .to_bytes()
        .unwrap()
        .to_vec();
    oversized.extend_from_slice(
        &Atom::new_parent(
            ID4::PCP_HELO,
            vec![
                Atom::new_leaf(
                    ID4::HELO_SESSIONID,
                    Bytes::copy_from_slice(session_id.as_bytes()),
                ),
                Atom::new_leaf(ID4::HELO_AGENT, Bytes::from(vec![b'x'; 10_000])),
            ],
        )
        .to_bytes()
        .unwrap(),
    );
    invalid.push(Bytes::from(oversized));

    for request in invalid {
        let mut socket = TcpStream::connect(server.addr).await.unwrap();
        socket.write_all(&request).await.unwrap();
        let mut byte = [0u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(1), socket.read(&mut byte))
            .await
            .expect("invalid probe connection should close promptly");
        match closed {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof
                ) => {}
            Ok(n) => panic!("invalid probe received {n} response bytes"),
            Err(error) => panic!("invalid probe did not close cleanly: {error}"),
        }
        drop(socket);

        let http = http_response(
            server.addr,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(String::from_utf8_lossy(&http).starts_with("HTTP/1.1 200 OK"));
    }

    stop_server(server).await;
}
