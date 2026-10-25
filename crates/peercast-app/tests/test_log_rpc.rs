use peercast_app::config::AppConfig;
use peercast_app::lifecycle::{ShutdownReason, TaskTracker};
use peercast_app::logging::{BoundedLogBuffer, LogLevel};
use peercast_app::metrics::AppMetrics;
use peercast_app::server::HttpServer;
use peercast_rpc::server::RpcState;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

async fn rpc_call(addr: std::net::SocketAddr, method: &str, params: Value) -> Value {
    rpc_call_with_id(addr, method, params, json!(1), false).await
}

async fn rpc_call_with_id(
    addr: std::net::SocketAddr,
    method: &str,
    params: Value,
    id: Value,
    fragment_utf8_body: bool,
) -> Value {
    let payload = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": id,
    })
    .to_string();
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "POST /api/1 HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    if fragment_utf8_body {
        let split = payload
            .find('分')
            .expect("test id includes a multibyte character")
            + 1;
        stream
            .write_all(&payload.as_bytes()[..split])
            .await
            .unwrap();
        tokio::task::yield_now().await;
        stream
            .write_all(&payload.as_bytes()[split..])
            .await
            .unwrap();
    } else {
        stream.write_all(payload.as_bytes()).await.unwrap();
    }
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response))
        .await
        .expect("HTTP RPC response should finish")
        .unwrap();
    let body_start = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP response should contain headers")
        + 4;
    let body = &response[body_start..];
    let content_length = String::from_utf8_lossy(&response[..body_start])
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .expect("HTTP response should declare Content-Length");
    assert_eq!(body.len(), content_length);
    serde_json::from_slice(body).expect("RPC response should be JSON")
}

#[tokio::test]
async fn logging_rpc_supports_station_params_redaction_levels_and_clear_over_http() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut config = AppConfig::default();
    config.server.port = addr.port();
    config.server.bind_address = "127.0.0.1".to_owned();
    config.logging.level = "debug".to_owned();

    let secret = "LogRpcPrivateKey-456";
    let log_buffer = Arc::new(
        BoundedLogBuffer::from_config(&config.logging, &[secret]).expect("logger config valid"),
    );
    let tracker = TaskTracker::new();
    let server = HttpServer::new(
        &config,
        RpcState::default(),
        Arc::new(AppMetrics::new()),
        Arc::clone(&log_buffer),
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
    let readiness = TcpStream::connect(addr).await.unwrap();
    drop(readiness);
    log_buffer.clear();
    log_buffer.log(
        LogLevel::Debug,
        "seed-debug",
        &format!("debug marker {secret}"),
    );
    log_buffer.log(
        LogLevel::Warn,
        "seed-warn",
        &format!("warn marker {secret}"),
    );

    let test_log = Arc::clone(&log_buffer);
    let exercise = tokio::spawn(async move {
        let settings = rpc_call(addr, "getLogSettings", Value::Null).await;
        assert_eq!(settings["result"]["level"], 5);
        assert_eq!(settings["result"]["levelName"], "DEBUG");

        let named = rpc_call_with_id(
            addr,
            "getLog",
            json!({"from": 0, "maxLines": 1}),
            json!("分割ID"),
            true,
        )
        .await;
        assert_eq!(named["id"], "分割ID");
        assert_eq!(named["result"]["from"], 0);
        assert_eq!(named["result"]["lines"], 1);
        assert!(named["result"]["log"]
            .as_str()
            .unwrap()
            .contains("DEBUG seed-debug"));
        assert_eq!(named["result"]["entries"][0]["level"], "DEBUG");
        assert!(!named.to_string().contains(secret));

        let positional = rpc_call(addr, "getLog", json!([0, 2])).await;
        assert_eq!(positional["result"]["lines"], 2);
        assert!(positional["result"]["log"]
            .as_str()
            .unwrap()
            .contains("WARN seed-warn"));
        assert_eq!(positional["result"]["entries"][1]["level"], "WARN");
        assert!(!positional.to_string().contains(secret));

        let from_one = rpc_call(addr, "getLog", json!({"from": 1, "maxLines": 1})).await;
        assert_eq!(from_one["result"]["from"], 1);
        assert_eq!(from_one["result"]["lines"], 1);
        assert!(from_one["result"]["log"]
            .as_str()
            .unwrap()
            .contains("warn marker"));

        let named_level = rpc_call(addr, "setLogSettings", json!({"settings": {"level": 5}})).await;
        assert!(named_level.get("error").is_none());
        let positional_level = rpc_call(addr, "setLogSettings", json!([{"level": 2}])).await;
        assert!(positional_level.get("error").is_none());
        let settings = rpc_call(addr, "getLogSettings", Value::Null).await;
        assert_eq!(settings["result"]["level"], 2);
        assert_eq!(settings["result"]["levelName"], "ERROR");
        test_log.log(LogLevel::Debug, "filtered", "must not be stored");
        assert_eq!(test_log.len(), 2);

        let invalid_from = rpc_call(addr, "getLog", json!({"from": -1})).await;
        assert_eq!(invalid_from["error"]["code"], -32602);
        let invalid_level =
            rpc_call(addr, "setLogSettings", json!({"settings": {"level": 6}})).await;
        assert_eq!(invalid_level["error"]["code"], -32602);

        let off_level = rpc_call(addr, "setLogSettings", json!({"settings": {"level": 0}})).await;
        assert!(off_level.get("error").is_none());
        let settings = rpc_call(addr, "getLogSettings", Value::Null).await;
        assert_eq!(settings["result"]["level"], 0);
        assert_eq!(settings["result"]["levelName"], "OFF");
        test_log.log(LogLevel::Error, "off", "must not be stored while off");
        assert_eq!(test_log.len(), 2);

        let cleared = rpc_call(addr, "clearLog", Value::Null).await;
        assert!(cleared["result"].is_null());
        let empty = rpc_call(addr, "getLog", json!([0, 2])).await;
        assert_eq!(empty["result"]["lines"], 0);
        assert_eq!(empty["result"]["log"], "");
        assert_eq!(empty["result"]["entries"], json!([]));
    });

    let exercise_result = exercise.await;
    runtime.shutdown().await;
    let _ = shutdown_tx.send(ShutdownReason::AdminRpc);
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP server should stop")
        .unwrap()
        .unwrap();
    tracker
        .wait_for_drain(Duration::from_secs(2))
        .await
        .expect("runtime tasks should drain");
    exercise_result.expect("logging RPC checks should pass");
}
