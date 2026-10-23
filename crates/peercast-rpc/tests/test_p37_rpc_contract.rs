use serde_json::Value;

use peercast_core::channel_id::ChannelId;
use peercast_rpc::{rpc_code, AccessLevel, ChannelSummary, RpcDispatcher, RpcState};

fn sample_cid() -> ChannelId {
    ChannelId::from_hex("0123456789ABCDEF0123456789ABCDEF").unwrap()
}

/// Acceptance criterion 1:
/// Core JSON-RPC 2.0 query methods: `getVersionInfo`, `getStatus`, `getChannels`, `getAuthToken` (REQ-064).
#[tokio::test]
async fn test_p37_core_query_methods() {
    let mut state = RpcState::default();
    state.channels.push(ChannelSummary::new(
        sample_cid(),
        "TestStream",
        "Receiving",
        512,
        "video/x-flv",
        3,
        7,
    ));
    state.auth_token = Some("secret-token-xyz".to_string());
    state.status.uptime = 3600;

    let dispatcher = RpcDispatcher::new(state);

    // 1. getVersionInfo
    let req1 = r#"{"jsonrpc": "2.0", "method": "getVersionInfo", "id": 1}"#;
    let res1: Value = serde_json::from_str(&dispatcher.handle_request_str(req1, AccessLevel::Playback).await.unwrap()).unwrap();
    assert_eq!(res1["id"], 1);
    assert_eq!(res1["result"]["agentName"], "PeerCastStation/1.0");
    assert_eq!(res1["result"]["apiVersion"], "1.0.0");
    assert_eq!(res1["result"]["jsonrpc"], "2.0");

    // 2. getStatus
    let req2 = r#"{"jsonrpc": "2.0", "method": "getStatus", "id": 2}"#;
    let res2: Value = serde_json::from_str(&dispatcher.handle_request_str(req2, AccessLevel::Playback).await.unwrap()).unwrap();
    assert_eq!(res2["id"], 2);
    assert_eq!(res2["result"]["uptime"], 3600);
    assert_eq!(res2["result"]["localRelayEndPoint"][0], "127.0.0.1");
    assert_eq!(res2["result"]["localRelayEndPoint"][1], 7144);

    // 3. getChannels
    let req3 = r#"{"jsonrpc": "2.0", "method": "getChannels", "id": 3}"#;
    let res3: Value = serde_json::from_str(&dispatcher.handle_request_str(req3, AccessLevel::Playback).await.unwrap()).unwrap();
    assert_eq!(res3["id"], 3);
    let channels = res3["result"].as_array().unwrap();
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0]["name"], "TestStream");
    assert_eq!(channels[0]["totalRelays"], 3);

    // 4. getAuthToken
    let req4 = r#"{"jsonrpc": "2.0", "method": "getAuthToken", "id": 4}"#;
    let res4: Value = serde_json::from_str(&dispatcher.handle_request_str(req4, AccessLevel::Playback).await.unwrap()).unwrap();
    assert_eq!(res4["result"], "secret-token-xyz");
}

/// Acceptance criterion 2:
/// Administrative settings read and mutation: `getSettings`, `setSettings` (REQ-064, REQ-065).
#[tokio::test]
async fn test_p37_settings_read_and_mutation() {
    let dispatcher = RpcDispatcher::default();

    // 1. Initial settings read under Admin access
    let req1 = r#"{"jsonrpc": "2.0", "method": "getSettings", "id": 10}"#;
    let res1: Value = serde_json::from_str(&dispatcher.handle_request_str(req1, AccessLevel::Admin).await.unwrap()).unwrap();
    assert_eq!(res1["id"], 10);
    assert_eq!(res1["result"]["maxRelays"], 16);
    assert_eq!(res1["result"]["maxDirects"], 8);

    // 2. Mutation under Admin access
    let req2 = r#"{"jsonrpc": "2.0", "method": "setSettings", "params": {"maxRelays": 32, "maxDirects": 16}, "id": 11}"#;
    let res2: Value = serde_json::from_str(&dispatcher.handle_request_str(req2, AccessLevel::Admin).await.unwrap()).unwrap();
    assert_eq!(res2["id"], 11);
    assert_eq!(res2["result"], true);

    // 3. Verify settings updated in dispatcher state
    let req3 = r#"{"jsonrpc": "2.0", "method": "getSettings", "id": 12}"#;
    let res3: Value = serde_json::from_str(&dispatcher.handle_request_str(req3, AccessLevel::Admin).await.unwrap()).unwrap();
    assert_eq!(res3["result"]["maxRelays"], 32);
    assert_eq!(res3["result"]["maxDirects"], 16);
}

/// Acceptance criterion 3:
/// Access control enforcement: Playback access cannot call Admin-only methods (REQ-065).
#[tokio::test]
async fn test_p37_access_control_enforcement() {
    let dispatcher = RpcDispatcher::default();

    // 1. Playback caller attempts to read settings -> REJECTED (Unauthorized)
    let req1 = r#"{"jsonrpc": "2.0", "method": "getSettings", "id": 1}"#;
    let res1: Value = serde_json::from_str(&dispatcher.handle_request_str(req1, AccessLevel::Playback).await.unwrap()).unwrap();
    assert_eq!(res1["id"], 1);
    assert_eq!(res1["error"]["code"], rpc_code::UNAUTHORIZED);
    assert!(res1["result"].is_null());

    // 2. Playback caller attempts to modify settings -> REJECTED (Unauthorized)
    let req2 = r#"{"jsonrpc": "2.0", "method": "setSettings", "params": {"maxRelays": 100}, "id": 2}"#;
    let res2: Value = serde_json::from_str(&dispatcher.handle_request_str(req2, AccessLevel::Playback).await.unwrap()).unwrap();
    assert_eq!(res2["id"], 2);
    assert_eq!(res2["error"]["code"], rpc_code::UNAUTHORIZED);

    // 3. Playback caller calls Playback-permitted method -> ACCEPTED
    let req3 = r#"{"jsonrpc": "2.0", "method": "getVersionInfo", "id": 3}"#;
    let res3: Value = serde_json::from_str(&dispatcher.handle_request_str(req3, AccessLevel::Playback).await.unwrap()).unwrap();
    assert_eq!(res3["id"], 3);
    assert!(res3["error"].is_null());
    assert_eq!(res3["result"]["apiVersion"], "1.0.0");
}

/// Acceptance criterion 4:
/// Explicit capability reporting for deferred methods; NO silent success stubs (REQ-064).
#[tokio::test]
async fn test_p37_deferred_methods_report_unsupported_capability() {
    let dispatcher = RpcDispatcher::default();

    let deferred_calls = vec![
        ("broadcastChannel", "broadcast_ingest"),
        ("checkBandwidth", "diagnostics"),
        ("checkPorts", "diagnostics"),
        ("checkUpdate", "updater"),
        ("updateAndRestart", "updater"),
        ("getUpdateStatus", "updater"),
        ("getNewVersions", "updater"),
        ("getNotificationMessages", "notifications"),
        ("stopChannel", "channel_lifecycle"),
        ("bumpChannel", "channel_lifecycle"),
        ("getYellowPages", "yp_client"),
        ("addListener", "listener_manager"),
    ];

    for (i, (method, capability)) in deferred_calls.iter().enumerate() {
        let req = format!(r#"{{"jsonrpc": "2.0", "method": "{}", "id": {}}}"#, method, i + 1);
        let raw_res = dispatcher.handle_request_str(&req, AccessLevel::Admin).await.unwrap();
        let res: Value = serde_json::from_str(&raw_res).unwrap();

        assert_eq!(res["id"], i + 1);
        assert_eq!(
            res["error"]["code"],
            rpc_code::UNSUPPORTED_METHOD,
            "Method {} must return UNSUPPORTED_METHOD, never a success stub!",
            method
        );
        assert_eq!(res["error"]["data"]["method"], *method);
        assert_eq!(res["error"]["data"]["status"], "deferred");
        assert_eq!(res["error"]["data"]["capability"], *capability);
        assert!(res["result"].is_null());
    }
}

/// Acceptance criterion 5:
/// Unknown methods return METHOD_NOT_FOUND (-32601) (REQ-064).
#[tokio::test]
async fn test_p37_unknown_method_returns_method_not_found() {
    let dispatcher = RpcDispatcher::default();

    let req = r#"{"jsonrpc": "2.0", "method": "nonExistentMethodXYZ", "id": 42}"#;
    let res: Value = serde_json::from_str(&dispatcher.handle_request_str(req, AccessLevel::Admin).await.unwrap()).unwrap();

    assert_eq!(res["id"], 42);
    assert_eq!(res["error"]["code"], rpc_code::METHOD_NOT_FOUND);
    assert!(res["error"]["message"].as_str().unwrap().contains("nonExistentMethodXYZ"));
}

/// Acceptance criterion 6:
/// Batch request execution and error isolation (REQ-064).
#[tokio::test]
async fn test_p37_batch_requests() {
    let dispatcher = RpcDispatcher::default();

    // Mixed batch: 1 success, 1 deferred, 1 method not found
    let batch = r#"[
        {"jsonrpc": "2.0", "method": "getVersionInfo", "id": 101},
        {"jsonrpc": "2.0", "method": "checkUpdate", "id": 102},
        {"jsonrpc": "2.0", "method": "unknownBar", "id": 103}
    ]"#;

    let raw_res = dispatcher.handle_request_str(batch, AccessLevel::Admin).await.unwrap();
    let res: Value = serde_json::from_str(&raw_res).unwrap();
    let arr = res.as_array().unwrap();
    assert_eq!(arr.len(), 3);

    // Item 1: getVersionInfo succeeded
    assert_eq!(arr[0]["id"], 101);
    assert_eq!(arr[0]["result"]["apiVersion"], "1.0.0");

    // Item 2: checkUpdate reported deferred unsupported
    assert_eq!(arr[1]["id"], 102);
    assert_eq!(arr[1]["error"]["code"], rpc_code::UNSUPPORTED_METHOD);

    // Item 3: unknownBar reported method not found
    assert_eq!(arr[2]["id"], 103);
    assert_eq!(arr[2]["error"]["code"], rpc_code::METHOD_NOT_FOUND);

    // Empty batch array returns INVALID_REQUEST
    let empty_batch = "[]";
    let empty_res: Value = serde_json::from_str(&dispatcher.handle_request_str(empty_batch, AccessLevel::Admin).await.unwrap()).unwrap();
    assert_eq!(empty_res["error"]["code"], rpc_code::INVALID_REQUEST);
}

/// Acceptance criterion 7:
/// Notification semantics: omitting `id` executes without returning a response (REQ-064).
#[tokio::test]
async fn test_p37_notification_semantics() {
    let dispatcher = RpcDispatcher::default();

    // 1. Single notification (setSettings without id)
    let notif = r#"{"jsonrpc": "2.0", "method": "setSettings", "params": {"maxRelays": 48}}"#;
    let res = dispatcher.handle_request_str(notif, AccessLevel::Admin).await;
    assert!(res.is_none(), "Single notification must return None (HTTP 204 No Content)");

    // Verify settings mutation actually took effect
    let st = dispatcher.state().read().await;
    assert_eq!(st.settings.max_relays, 48);
    drop(st);

    // 2. Batch containing only notifications
    let batch_notifs = r#"[
        {"jsonrpc": "2.0", "method": "setSettings", "params": {"maxDirects": 24}},
        {"jsonrpc": "2.0", "method": "setSettings", "params": {"maxUpstreamRate": 20000}}
    ]"#;
    let batch_res = dispatcher.handle_request_str(batch_notifs, AccessLevel::Admin).await;
    assert!(batch_res.is_none(), "Batch of pure notifications must return None (HTTP 204)");

    let st2 = dispatcher.state().read().await;
    assert_eq!(st2.settings.max_directs, 24);
    assert_eq!(st2.settings.max_upstream_rate, 20000);
}

/// Acceptance criterion 8:
/// Malformed JSON and payload size limit rejection (REQ-064, Global Rule 3).
#[tokio::test]
async fn test_p37_malformed_json_and_size_limits() {
    let dispatcher = RpcDispatcher::default().with_max_request_bytes(100);

    // 1. Invalid JSON syntax returns PARSE_ERROR (-32700)
    let bad_json = r#"{"jsonrpc": "2.0", "method": "getVersionInfo", "id": 1, Broken"#;
    let res1: Value = serde_json::from_str(&dispatcher.handle_request_str(bad_json, AccessLevel::Admin).await.unwrap()).unwrap();
    assert_eq!(res1["error"]["code"], rpc_code::PARSE_ERROR);

    // 2. Request body exceeding max_request_bytes returns INVALID_REQUEST (-32600)
    let huge_req = format!(
        r#"{{"jsonrpc": "2.0", "method": "getVersionInfo", "id": 1, "padding": "{}"}}"#,
        "X".repeat(200)
    );
    let res2: Value = serde_json::from_str(&dispatcher.handle_request_str(&huge_req, AccessLevel::Admin).await.unwrap()).unwrap();
    assert_eq!(res2["error"]["code"], rpc_code::INVALID_REQUEST);
}
