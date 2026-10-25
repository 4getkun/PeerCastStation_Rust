use std::{sync::Arc, time::Duration};

use peercast_app::BoundedLogBuffer;
use peercast_desktop::{
    backend::Limits,
    broadcast::{BroadcastManager, BroadcastRequest, SP_ANNOUNCE_URI},
    csharp_backend::CSharpBackend,
};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

const CHANNEL_ID: &str = "11111111111111111111111111111111";
const OWNED_YP_ID: i64 = 71;
const EXISTING_YP_ID: i64 = 12;

#[derive(Default)]
struct ApiState {
    methods: Vec<String>,
    broadcast_params: Option<Value>,
    pages: Vec<Value>,
    channel_active: bool,
}

async fn mock_api(
    pages: Vec<Value>,
    stop_delay: Duration,
) -> (String, Arc<Mutex<ApiState>>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(ApiState {
        pages,
        ..ApiState::default()
    }));
    let server_state = Arc::clone(&state);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let state = Arc::clone(&server_state);
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0; 2048];
                loop {
                    let count = socket.read(&mut chunk).await.unwrap_or(0);
                    if count == 0 {
                        return;
                    }
                    request.extend_from_slice(&chunk[..count]);
                    let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n")
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + content_length {
                        break;
                    }
                }
                let end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap();
                let call: Value = serde_json::from_slice(&request[end + 4..]).unwrap();
                let method = call["method"].as_str().unwrap().to_owned();
                let params = call.get("params").cloned().unwrap_or_else(|| json!({}));
                let result = {
                    let mut guard = state.lock().await;
                    guard.methods.push(method.clone());
                    match method.as_str() {
                        "getVersionInfo" => {
                            json!({"agentName":"PeerCastStation/6.0.0.0","apiVersion":"1.0.0","jsonrpc":"2.0"})
                        }
                        "getChannels" => {
                            if guard.channel_active {
                                json!([{
                                    "channelId":CHANNEL_ID,
                                    "info":{"name":"Metadata test broadcast"},
                                    "status":{"status":"Receiving","isReceiving":true,"isBroadcasting":true,"totalDirects":0,"totalRelays":0}
                                }])
                            } else {
                                json!([])
                            }
                        }
                        "getYellowPages" => json!(guard.pages),
                        "addYellowPage" => {
                            guard.pages.push(json!({
                                "yellowPageId":OWNED_YP_ID,
                                "name":"SP (Peercast RT)",
                                "protocol":"pcp",
                                "uri":SP_ANNOUNCE_URI,
                                "announceUri":SP_ANNOUNCE_URI,
                                "channelsUri":"http://bayonet.ddo.jp/sp/index.txt"
                            }));
                            json!({"yellowPageId":OWNED_YP_ID,"announceUri":SP_ANNOUNCE_URI})
                        }
                        "broadcastChannel" => {
                            guard.broadcast_params = Some(params);
                            guard.channel_active = true;
                            json!(CHANNEL_ID)
                        }
                        "getChannelInfo" => json!({
                            "yellowPages":[{"yellowPageId":OWNED_YP_ID,"name":"SP (Peercast RT)","status":"Idle"}]
                        }),
                        "stopChannel" => {
                            drop(guard);
                            tokio::time::sleep(stop_delay).await;
                            let mut guard = state.lock().await;
                            guard.channel_active = false;
                            Value::Null
                        }
                        "removeYellowPage" => {
                            let id = params["yellowPageId"].as_i64().unwrap();
                            guard.pages.retain(|page| page["yellowPageId"] != id);
                            Value::Null
                        }
                        other => panic!("unexpected C# API call {other}"),
                    }
                };
                let body =
                    serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"result":result})).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            });
        }
    });
    (origin, state, task)
}

fn request(announce_sp: bool) -> BroadcastRequest {
    BroadcastRequest {
        name: "Metadata test broadcast".into(),
        genre: "Test genre".into(),
        description: "Test description".into(),
        comment: "Test comment".into(),
        contact_url: "https://example.invalid/contact".into(),
        content_type: "FLV".into(),
        bitrate: 192,
        source_mode: "push".into(),
        source_uri: String::new(),
        ingest_port: 8123,
        relay_port: 7144,
        announce_sp,
    }
}

fn limits() -> Limits {
    Limits {
        max_directs: 8,
        max_relays: 8,
        max_upstream_rate: 20_000,
    }
}

#[tokio::test]
async fn csharp_broadcast_preserves_metadata_handles_slow_stop_and_removes_only_owned_sp() {
    let (origin, state, server) = mock_api(Vec::new(), Duration::from_millis(3_200)).await;
    let backend = CSharpBackend::connect(&origin).await.unwrap();
    let manager = BroadcastManager::new(Some(backend));
    let id = tokio::time::timeout(
        Duration::from_secs(5),
        manager.start(
            request(true),
            &limits(),
            Arc::new(BoundedLogBuffer::default()),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(id, CHANNEL_ID);

    let (views, channels) = manager.snapshot().await.unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].channel_id, CHANNEL_ID);
    assert_eq!(views[0].status, "受信中");
    assert_eq!(views[0].announcement_status, "Idle");
    assert_eq!(views[0].stream_url, format!("{origin}/stream/{CHANNEL_ID}"));
    assert!(channels.is_empty());

    let params = state.lock().await.broadcast_params.clone().unwrap();
    assert_eq!(params["yellowPageId"], OWNED_YP_ID);
    assert_eq!(params["networkType"], "ipv4");
    assert_eq!(params["sourceUri"], "http://127.0.0.1:8123/");
    assert_eq!(params["sourceStream"], "HTTP Push Source");
    assert_eq!(params["contentReader"], "Flash Video (FLV)");
    assert_eq!(params["info"]["name"], "Metadata test broadcast");
    assert_eq!(params["info"]["genre"], "Test genre");
    assert_eq!(params["info"]["desc"], "Test description");
    assert_eq!(params["info"]["comment"], "Test comment");
    assert_eq!(params["info"]["url"], "https://example.invalid/contact");
    assert_eq!(params["info"]["bitrate"], 192);

    tokio::time::timeout(Duration::from_secs(8), manager.shutdown())
        .await
        .expect("stopChannel response should tolerate a short delay")
        .unwrap();
    let state = state.lock().await;
    assert!(
        state.pages.is_empty(),
        "manager-owned SP registration is removed"
    );
    assert!(state.methods.iter().any(|method| method == "addYellowPage"));
    assert!(state.methods.iter().any(|method| method == "stopChannel"));
    assert!(state
        .methods
        .iter()
        .any(|method| method == "removeYellowPage"));
    drop(state);
    server.abort();

    let existing = json!({
        "yellowPageId":EXISTING_YP_ID,
        "name":"User SP config",
        "protocol":"pcp",
        "uri":SP_ANNOUNCE_URI,
        "announceUri":SP_ANNOUNCE_URI,
        "channelsUri":"http://bayonet.ddo.jp/sp/index.txt"
    });
    let (origin, state, server) = mock_api(vec![existing], Duration::ZERO).await;
    let manager = BroadcastManager::new(Some(CSharpBackend::connect(&origin).await.unwrap()));
    let id = manager
        .start(
            request(true),
            &limits(),
            Arc::new(BoundedLogBuffer::default()),
        )
        .await
        .unwrap();
    assert_eq!(id, CHANNEL_ID);
    manager.shutdown().await.unwrap();
    let state = state.lock().await;
    assert_eq!(state.pages.len(), 1);
    assert_eq!(state.pages[0]["yellowPageId"], EXISTING_YP_ID);
    assert!(!state.methods.iter().any(|method| method == "addYellowPage"));
    assert!(!state
        .methods
        .iter()
        .any(|method| method == "removeYellowPage"));
    drop(state);
    server.abort();
}
