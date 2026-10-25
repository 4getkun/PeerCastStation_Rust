use peercast_desktop::{
    backend::{DesktopBackend, YellowPageEntry},
    csharp_backend::CSharpBackend,
    player::PlayerContext,
    preferences::DesktopPreferences,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

const ID: &str = "11111111111111111111111111111111";

async fn fixture(agent: &str) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let tracker = listener.local_addr().unwrap().to_string();
    let agent = agent.to_owned();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let saved = Arc::clone(&calls);
    let task = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let agent = agent.clone();
            let calls = Arc::clone(&saved);
            let tracker = tracker.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0; 2048];
                loop {
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                    if let Some(end) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&request[..end]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let end = request.windows(4).position(|v| v == b"\r\n\r\n").unwrap();
                let header = String::from_utf8_lossy(&request[..end]);
                let body = if header.starts_with("POST /api/1 ") {
                    assert!(header
                        .to_ascii_lowercase()
                        .contains("x-requested-with: xmlhttprequest"));
                    let call: Value = serde_json::from_slice(&request[end + 4..]).unwrap();
                    let method = call["method"].as_str().unwrap().to_owned();
                    calls.lock().await.push(method.clone());
                    let result = match method.as_str() {
                        "getVersionInfo" => {
                            json!({"agentName":agent,"apiVersion":"1.0.0","jsonrpc":"2.0"})
                        }
                        "getChannels" => {
                            json!([{"channelId":ID,"info":{"name":"C# fixture broadcast"},"status":{"isReceiving":true,"isBroadcasting":true,"totalDirects":3,"totalRelays":2}}])
                        }
                        "getStatus" => {
                            json!({"uptime":321,"localRelayEndPoint":["0.0.0.0",7144],"globalRelayEndPoint":["203.0.113.8",7144]})
                        }
                        "getListeners" => {
                            json!([{"address":"0.0.0.0","port":7144,"portStatus":2,"authToken":"must-not-escape","authenticationPassword":"secret"}])
                        }
                        "getChannelInfo" => {
                            json!({"info":{"name":"Fixture metadata","url":"https://example.test","genre":"game","desc":"description","comment":"comment","bitrate":256,"contentType":"video/x-flv"}})
                        }
                        "getChannelConnections" => {
                            json!([{"connectionId":7,"type":"relay","status":"Connected","remoteEndPoint":"192.0.2.3:7144","remoteSessionId":"ABC123","agentName":"PeerCastStation","remoteHostStatus":["firewalled"]}])
                        }
                        _ => panic!("unexpected mutation: {method}"),
                    };
                    serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"result":result})).unwrap()
                } else if header.starts_with("GET /index.txt ") {
                    format!("Fixture<>{ID}<>{tracker}<>http://example.com<>game<>Title<>3<>2<>128<>FLV<><><><><>test<>00:01<>click<>comment<>free<>\n").into_bytes()
                } else if header.starts_with("GET /stream/") {
                    b"FLV-fixture-media".to_vec()
                } else {
                    panic!("unexpected request: {header}")
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            });
        }
    });
    (origin, calls, task)
}

#[tokio::test]
async fn external_csharp_routes_media_and_preserves_the_original_process_and_settings() {
    let (origin, calls, server) = fixture("PeerCastStation/6.0.0.0").await;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let backend = DesktopBackend::open_external(path, &origin).await.unwrap();
    let initial = backend.snapshot().await.unwrap();
    assert_eq!(initial.backend_kind, "csharp");
    assert_eq!(initial.endpoint, origin);
    assert_eq!(initial.channels.len(), 1);
    assert!(initial.channels[0].is_broadcasting);
    assert_eq!(initial.channels[0].listeners, Some(3));
    assert!(initial.channels[0].diagnostics.available);
    assert_eq!(initial.channels[0].diagnostics.metadata.genre, "game");
    assert_eq!(initial.channels[0].diagnostics.content_type, "video/x-flv");
    assert_eq!(initial.channels[0].diagnostics.uptime_secs, 321);
    assert_eq!(
        initial.channels[0].diagnostics.connections[0].port_open,
        Some(false)
    );
    assert!(!initial.channels[0].diagnostics.connections[0].measurements_available);
    assert_eq!(
        initial.channels[0]
            .diagnostics
            .network
            .as_ref()
            .unwrap()
            .ipv4,
        "closed"
    );
    assert!(!serde_json::to_string(&initial)
        .unwrap()
        .contains("must-not-escape"));
    assert!(!serde_json::to_string(&initial).unwrap().contains("secret"));
    backend
        .apply_yellow_pages(vec![YellowPageEntry {
            name: "Fixture".into(),
            channels_url: format!("{origin}/index.txt"),
        }])
        .await
        .unwrap();
    backend.get_yp_channels(true).await.unwrap();
    let url = backend.prepare_yp_channel(ID, "Fixture").await.unwrap();
    assert!(url.starts_with(&format!("{origin}/stream/{ID}?tip=")));
    let context = PlayerContext::from_yp(url.clone(), &json!({"name":"Fixture"})).unwrap();
    assert_eq!(context.channel_id, ID);
    let mut source = backend.open_recording_source(ID, "Fixture").await.unwrap();
    assert_eq!(source.stream_url, url);
    assert_eq!(
        reqwest::get(&source.stream_url)
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .as_ref(),
        b"FLV-fixture-media"
    );
    source.close().await.unwrap();
    assert_eq!(backend.snapshot().await.unwrap().channels.len(), 1);
    assert!(backend.run_probe().await.is_err());
    let saved = backend.snapshot().await.unwrap();
    let mut desktop = saved.desktop.clone();
    desktop.directory.auto_refresh = true;
    desktop.window.width = 1280;
    desktop.window.height = 800;
    let changed = backend
        .apply_desktop_preferences(saved.limits.clone(), saved.yellow_pages.clone(), desktop)
        .await
        .unwrap();
    assert_eq!(changed.desktop.window.width, 1280);
    let mut limits = changed.limits.clone();
    limits.max_relays += 1;
    assert!(backend.apply_limits(limits).await.is_err());
    assert!(!backend.stop().await.unwrap().running);
    assert!(backend.restart().await.unwrap().running);
    tokio::time::timeout(Duration::from_secs(8), backend.close())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        CSharpBackend::connect(&origin)
            .await
            .unwrap()
            .channels()
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(calls.lock().await.iter().all(|m| matches!(
        m.as_str(),
        "getVersionInfo"
            | "getChannels"
            | "getStatus"
            | "getListeners"
            | "getChannelInfo"
            | "getChannelConnections"
    )));
    server.abort();
}

#[tokio::test]
async fn discovery_rejects_other_products_and_remote_origins() {
    let (origin, _, server) = fixture("OtherProgram/1.0").await;
    assert!(CSharpBackend::connect(&origin).await.is_err());
    server.abort();
    for origin in [
        "https://127.0.0.1:7144",
        "http://example.com:7144",
        "http://user:pass@127.0.0.1:7144",
        "http://127.0.0.1:7144/api/1",
    ] {
        assert!(CSharpBackend::connect(origin).await.is_err());
    }
    for url in [
        format!("http://127.0.0.1:7144/stream/{ID}?auth=secret"),
        format!("http://127.0.0.1:7144/stream/{ID}?tip=example.com:7144"),
        format!("http://127.0.0.1:7144/pls/{ID}?tip=1.2.3.4:7144"),
    ] {
        assert!(PlayerContext::from_yp(url, &json!({})).is_err());
    }
}

#[test]
fn legacy_preferences_default_to_manual_refresh_and_opt_in_windows_features() {
    let prefs: DesktopPreferences = serde_json::from_value(json!({})).unwrap();
    assert!(!prefs.directory.auto_refresh);
    assert_eq!(prefs.directory.refresh_interval_secs, 300);
    assert!(!prefs.window.minimize_to_tray);
    assert!(!prefs.window.launch_at_startup);
    assert_eq!((prefs.window.width, prefs.window.height), (1600, 900));
    let mut legacy = prefs.clone();
    legacy.directory.refresh_interval_secs = 15;
    assert_eq!(
        legacy.normalize().unwrap().directory.refresh_interval_secs,
        60
    );
    let mut invalid = prefs.clone();
    invalid.directory.refresh_interval_secs = 0;
    assert!(invalid.normalize().is_err());
    let mut invalid = prefs;
    invalid.window.height = 0;
    assert!(invalid.normalize().is_err());
}
