use bytes::Bytes;
use peercast_app::config::AppConfig;
use peercast_app::lifecycle::TaskTracker;
use peercast_app::logging::BoundedLogBuffer;
use peercast_app::runtime::AppRuntime;
use peercast_app::runtime_config::{ChannelConfig, ChannelSource};
use peercast_core::atom::Atom;
use peercast_core::channel_id::ChannelId;
use peercast_core::content::ContinuationFlags;
use peercast_core::errors::QuitReason;
use peercast_core::id4::ID4;
use peercast_pcp::packet::{ChannelPacket, PacketPayload};
use peercast_rpc::server::RpcState;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

const CHANNEL_ID: &str = "0123456789abcdef0123456789abcdef";

async fn read_atom(socket: &mut TcpStream, bytes: &mut Vec<u8>) -> io::Result<Atom> {
    let mut input = [0u8; 2048];
    loop {
        let mut decoded = Bytes::copy_from_slice(bytes);
        if let Some(atom) =
            Atom::decode(&mut decoded, &Default::default()).map_err(io::Error::other)?
        {
            let consumed = bytes.len() - decoded.len();
            bytes.drain(..consumed);
            return Ok(atom);
        }
        let count = socket.read(&mut input).await?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "client closed",
            ));
        }
        bytes.extend_from_slice(&input[..count]);
    }
}

async fn read_request(socket: &mut TcpStream) -> io::Result<()> {
    let mut request = Vec::new();
    let mut byte = [0u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        if socket.read(&mut byte).await? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request ended",
            ));
        }
        request.push(byte[0]);
        if request.len() > 8192 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request too long",
            ));
        }
    }
    Ok(())
}

async fn serve_attempt(socket: &mut TcpStream, status: u16, send_header: bool) -> io::Result<()> {
    read_request(socket).await?;
    let response = match status {
        200 => b"HTTP/1.0 200 OK\r\n\r\n".as_slice(),
        503 => b"HTTP/1.0 503 Service Unavailable\r\n\r\n".as_slice(),
        _ => unreachable!(),
    };
    socket.write_all(response).await?;
    let mut buffered = Vec::new();
    let helo = tokio::time::timeout(Duration::from_secs(1), read_atom(socket, &mut buffered))
        .await
        .map_err(io::Error::other)??;
    assert_eq!(helo.name(), ID4::PCP_HELO);

    let remote_sid = ChannelId::from_bytes([0x51; 16]);
    let oleh = Atom::new_parent(
        ID4::PCP_OLEH,
        vec![Atom::new_leaf(
            ID4::HELO_SESSIONID,
            Bytes::copy_from_slice(remote_sid.as_bytes()),
        )],
    );
    socket
        .write_all(&oleh.to_bytes().map_err(io::Error::other)?)
        .await?;
    if status == 200 {
        let ok = Atom::new_u32_le(ID4::PCP_OK, 1);
        socket
            .write_all(&ok.to_bytes().map_err(io::Error::other)?)
            .await?;
        if send_header {
            let packet = ChannelPacket {
                channel_id: Some(ChannelId::from_hex(CHANNEL_ID).unwrap()),
                stream_type: ID4::PKT_TYPE_HEAD,
                pos: 0,
                cont: ContinuationFlags::NONE,
                payload: PacketPayload::Header(Bytes::from_static(b"non-empty-header")),
                unknown_extensions: Vec::new(),
            };
            for atom in packet.to_atoms().map_err(io::Error::other)? {
                socket
                    .write_all(&atom.to_bytes().map_err(io::Error::other)?)
                    .await?;
            }
        }
    }
    let quit = Atom::new_u32_le(ID4::PCP_QUIT, QuitReason::UNAVAILABLE.raw());
    socket
        .write_all(&quit.to_bytes().map_err(io::Error::other)?)
        .await?;
    Ok(())
}

async fn backoff_before_third_attempt(status: u16, send_header: bool) -> u64 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint: SocketAddr = listener.local_addr().unwrap();
    let mut config = AppConfig::default();
    config.runtime.reconnect.max_attempts_per_cycle = 2;
    config.runtime.reconnect.base_backoff_ms = 100;
    config.runtime.reconnect.max_backoff_ms = 200;
    config.runtime.reconnect.connect_timeout_ms = 500;
    config.runtime.reconnect.read_timeout_ms = 500;
    config.runtime.channels.push(ChannelConfig {
        channel_id: CHANNEL_ID.to_owned(),
        name: "retry backoff regression".to_owned(),
        container_type: "RAW".to_owned(),
        source: ChannelSource::Relay { endpoint },
        ..ChannelConfig::default()
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
    runtime.start().await.unwrap();

    let third_attempt = tokio::spawn(async move {
        for attempt in 1..=3 {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .expect("relay attempt should arrive")
                .unwrap();
            if attempt == 3 {
                serve_attempt(&mut socket, status, send_header)
                    .await
                    .unwrap();
                return;
            }
            serve_attempt(&mut socket, status, send_header)
                .await
                .unwrap();
        }
        unreachable!()
    });
    tokio::time::timeout(Duration::from_secs(3), third_attempt)
        .await
        .expect("third attempt should arrive after bounded retry")
        .unwrap();

    let backoff_ms = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(entry) = log.snapshot().into_iter().find(|entry| {
                entry.message.contains("next connection")
                    && entry
                        .message
                        .split_whitespace()
                        .any(|field| field == "attempt=3")
            }) {
                break entry
                    .message
                    .split_once("backoff_ms=")
                    .and_then(|(_, value)| value.parse().ok())
                    .expect("retry log should include backoff_ms");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("retry decision for attempt 3 should be logged");

    runtime.shutdown().await;
    tracker
        .wait_for_drain(Duration::from_secs(2))
        .await
        .expect("relay worker should drain");
    backoff_ms
}

#[tokio::test]
async fn http_503_or_200_helo_without_media_does_not_reset_retry_backoff() {
    for status in [503, 200] {
        let backoff_ms = backoff_before_third_attempt(status, false).await;
        assert_eq!(
            backoff_ms, 200,
            "HTTP {status} without media should exhaust the two-attempt budget"
        );
    }
}

#[tokio::test]
async fn http_200_nonempty_header_resets_retry_budget() {
    let backoff_ms = backoff_before_third_attempt(200, true).await;
    assert_eq!(
        backoff_ms, 0,
        "non-empty header should reset the retry budget"
    );
}
