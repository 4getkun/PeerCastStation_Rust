use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use peercast_app::config::AppConfig;
use peercast_app::lifecycle::TaskTracker;
use peercast_app::logging::BoundedLogBuffer;
use peercast_app::runtime::AppRuntime;
use peercast_app::runtime_config::{ChannelConfig, ChannelSource};
use peercast_rpc::server::RpcState;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

fn make_runtime() -> (Arc<AppRuntime>, TaskTracker) {
    let tracker = TaskTracker::new();
    let runtime = AppRuntime::new(
        &AppConfig::default(),
        tracker.clone(),
        Arc::new(BoundedLogBuffer::default()),
        Arc::new(RwLock::new(RpcState::default())),
    )
    .unwrap();
    (runtime, tracker)
}

fn relay(id: &str) -> ChannelConfig {
    ChannelConfig {
        channel_id: id.to_owned(),
        name: "dynamic relay".to_owned(),
        source: ChannelSource::Relay {
            endpoint: "127.0.0.1:1".parse::<SocketAddr>().unwrap(),
        },
        ..ChannelConfig::default()
    }
}

async fn drain(runtime: &AppRuntime, tracker: &TaskTracker) {
    runtime.shutdown().await;
    tracker
        .wait_for_drain(Duration::from_secs(2))
        .await
        .expect("tracked workers should drain");
}

#[tokio::test]
async fn rejects_add_before_start_and_after_shutdown() {
    let (runtime, tracker) = make_runtime();
    let error = runtime
        .add_relay_channel(relay("0123456789abcdef0123456789abcdef"))
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

    runtime.start().await.unwrap();
    runtime.shutdown().await;
    let error = runtime
        .add_relay_channel(relay("1123456789abcdef0123456789abcdef"))
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    tracker
        .wait_for_drain(Duration::from_secs(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn rejects_invalid_ids_and_non_relay_sources() {
    let (runtime, tracker) = make_runtime();
    runtime.start().await.unwrap();
    assert!(runtime
        .add_relay_channel(relay("00000000000000000000000000000000"))
        .await
        .is_err());
    assert!(runtime
        .add_relay_channel(ChannelConfig {
            channel_id: "2123456789abcdef0123456789abcdef".to_owned(),
            source: ChannelSource::Relay {
                endpoint: "127.0.0.1:0".parse().unwrap(),
            },
            ..ChannelConfig::default()
        })
        .await
        .is_err());
    assert!(runtime
        .add_relay_channel(ChannelConfig {
            channel_id: "2123456789abcdef0123456789abcdef".to_owned(),
            source: ChannelSource::HttpPush {
                bind_address: "127.0.0.1:7146".parse().unwrap(),
                auth_token: None,
            },
            ..ChannelConfig::default()
        })
        .await
        .is_err());
    drain(&runtime, &tracker).await;
}

#[tokio::test]
async fn rejects_the_33rd_dynamic_relay() {
    let (runtime, tracker) = make_runtime();
    runtime.start().await.unwrap();
    for index in 1..=32 {
        let id = format!("{index:032x}");
        runtime.add_relay_channel(relay(&id)).await.unwrap();
    }
    let error = runtime
        .add_relay_channel(relay("00000000000000000000000000000021"))
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert_eq!(runtime.channel_snapshots().await.len(), 32);
    drain(&runtime, &tracker).await;
}

#[tokio::test]
async fn concurrent_duplicate_adds_share_one_channel_and_worker() {
    let (runtime, tracker) = make_runtime();
    runtime.start().await.unwrap();
    let mut adds = Vec::new();
    for _ in 0..16 {
        let runtime = Arc::clone(&runtime);
        adds.push(tokio::spawn(async move {
            runtime
                .add_relay_channel(relay("3123456789abcdef0123456789abcdef"))
                .await
                .unwrap()
        }));
    }
    let mut channels = Vec::new();
    for add in adds {
        channels.push(add.await.unwrap());
    }
    assert!(channels
        .iter()
        .all(|channel| Arc::ptr_eq(&channels[0], channel)));
    tokio::task::yield_now().await;
    assert_eq!(channels[0].snapshot().await.connection_attempts, 1);
    assert_eq!(runtime.channel_snapshots().await.len(), 1);
    drain(&runtime, &tracker).await;
}

#[tokio::test]
async fn shutdown_racing_add_leaves_no_worker_or_live_channel() {
    let (runtime, tracker) = make_runtime();
    runtime.start().await.unwrap();
    let adding_runtime = Arc::clone(&runtime);
    let add = tokio::spawn(async move {
        adding_runtime
            .add_relay_channel(relay("4123456789abcdef0123456789abcdef"))
            .await
    });
    let shutdown_runtime = Arc::clone(&runtime);
    let shutdown = tokio::spawn(async move { shutdown_runtime.shutdown().await });
    let _ = add.await.unwrap();
    shutdown.await.unwrap();
    tracker
        .wait_for_drain(Duration::from_secs(2))
        .await
        .expect("workers should drain after shutdown");
    let channel = runtime.channel("4123456789abcdef0123456789abcdef").unwrap();
    assert!(!channel.snapshot().await.available);
}

#[tokio::test]
async fn selecting_same_dynamic_relay_after_terminal_404_restarts_worker() {
    const ID: &str = "5123456789abcdef0123456789abcdef";
    let tracker = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = tracker.local_addr().unwrap();
    let (first_done_tx, first_done_rx) = tokio::sync::oneshot::channel();
    let tracker_task = tokio::spawn(async move {
        let (mut first, _) = tokio::time::timeout(Duration::from_secs(3), tracker.accept())
            .await
            .expect("first relay connection should arrive")
            .unwrap();
        let mut request = [0u8; 1024];
        tokio::time::timeout(Duration::from_secs(2), first.read(&mut request))
            .await
            .expect("first relay request should arrive")
            .unwrap();
        first
            .write_all(b"HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        drop(first);
        let _ = first_done_tx.send(());

        tokio::time::timeout(Duration::from_secs(2), tracker.accept())
            .await
            .is_ok()
    });

    let task_tracker = TaskTracker::new();
    let logs = Arc::new(BoundedLogBuffer::default());
    let runtime = AppRuntime::new(
        &AppConfig::default(),
        task_tracker.clone(),
        Arc::clone(&logs),
        Arc::new(RwLock::new(RpcState::default())),
    )
    .unwrap();
    runtime.start().await.unwrap();
    let mut config = relay(ID);
    config.source = ChannelSource::Relay { endpoint };
    let channel = runtime.add_relay_channel(config.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), first_done_rx)
        .await
        .expect("fixture should respond to the first connection")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if logs.snapshot().iter().any(|entry| {
                entry.target == "relay"
                    && entry.message.contains("source ended")
                    && entry.message.contains("terminal=true")
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first 404 should end the relay worker terminally");
    assert_eq!(channel.snapshot().await.connection_attempts, 1);

    runtime.add_relay_channel(config).await.unwrap();
    let second_connection = tracker_task.await.unwrap();
    drain(&runtime, &task_tracker).await;
    assert!(
        second_connection,
        "reselecting a channel after terminal 404 should restart its relay worker"
    );
}

#[tokio::test]
async fn dynamic_relay_forwards_http_push_data_to_stream_reader() {
    use peercast_app::lifecycle::ShutdownReason;
    use peercast_app::runtime::AppRuntime;
    use peercast_app::runtime_config::{ChannelConfig, ChannelSource};
    use peercast_app::server::HttpServer;
    use peercast_http::router::HttpRequest;
    use tokio::sync::broadcast;

    const SOURCE_ID: &str = "11111111111111111111111111111111";
    const RELAY_ID: &str = SOURCE_ID;
    let mut source_config = AppConfig::default();
    source_config.runtime.channels.push(ChannelConfig {
        channel_id: SOURCE_ID.to_owned(),
        name: "dynamic relay source".to_owned(),
        container_type: "RAW".to_owned(),
        source: ChannelSource::HttpPush {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            auth_token: None,
        },
        ..ChannelConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    source_config.server.port = addr.port();
    source_config.server.bind_address = "127.0.0.1".to_owned();
    let source_tracker = TaskTracker::new();
    let source_server = HttpServer::new(
        &source_config,
        RpcState::default(),
        Arc::new(peercast_app::metrics::AppMetrics::new()),
        Arc::new(BoundedLogBuffer::default()),
        source_tracker.clone(),
    )
    .unwrap();
    let source_runtime = source_server.runtime();
    source_runtime.start().await.unwrap();
    let (source_shutdown_tx, source_shutdown_rx) = broadcast::channel(2);
    let source_server_task = tokio::spawn(async move {
        source_server
            .run_on_listener(listener, source_shutdown_rx)
            .await
            .map_err(|error| error.to_string())
    });

    let mut push = TcpStream::connect(source_runtime.ingest_addr(SOURCE_ID).await.unwrap())
        .await
        .unwrap();
    push.write_all(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
        .await
        .unwrap();
    let feeder = tokio::spawn(async move {
        loop {
            if push.write_all(b"5\r\nHELLO\r\n").await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    let receiver_tracker = TaskTracker::new();
    let receiver = AppRuntime::new(
        &AppConfig::default(),
        receiver_tracker.clone(),
        Arc::new(BoundedLogBuffer::default()),
        Arc::new(RwLock::new(RpcState::default())),
    )
    .unwrap();
    receiver.start().await.unwrap();
    let channel = receiver
        .add_relay_channel(ChannelConfig {
            channel_id: RELAY_ID.to_owned(),
            name: "dynamic relay receiver".to_owned(),
            container_type: "RAW".to_owned(),
            source: ChannelSource::Relay { endpoint: addr },
            ..ChannelConfig::default()
        })
        .await
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = channel.snapshot().await;
            if state.available && state.buffered_bytes > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(received.is_ok(), "relay should receive source bytes");

    let request =
        HttpRequest::parse(format!("GET /stream/{RELAY_ID} HTTP/1.1\r\n\r\n").as_bytes()).unwrap();
    let (mut stream_writer, mut stream_reader) = tokio::io::duplex(4096);
    let stream_runtime = Arc::clone(&receiver);
    let stream_task = tokio::spawn(async move {
        stream_runtime
            .serve_stream(&mut stream_writer, &request, RELAY_ID)
            .await
    });
    let output = tokio::time::timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 128];
        loop {
            let count = stream_reader.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.windows(5).any(|window| window == b"HELLO") {
                break;
            }
        }
        io::Result::Ok(bytes)
    })
    .await
    .expect("stream should deliver relayed bytes")
    .unwrap();
    assert!(output.windows(5).any(|window| window == b"HELLO"));

    receiver.shutdown().await;
    receiver_tracker
        .wait_for_drain(Duration::from_secs(2))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), stream_task)
        .await
        .expect("stream task should stop")
        .unwrap()
        .unwrap();
    feeder.abort();
    source_runtime.shutdown().await;
    let _ = source_shutdown_tx.send(ShutdownReason::AdminRpc);
    tokio::time::timeout(Duration::from_secs(2), source_server_task)
        .await
        .expect("source HTTP server should stop")
        .unwrap()
        .unwrap();
    source_tracker
        .wait_for_drain(Duration::from_secs(2))
        .await
        .unwrap();
}
