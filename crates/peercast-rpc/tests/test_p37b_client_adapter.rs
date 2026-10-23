use serde_json::json;

use peercast_core::channel_id::ChannelId;
use peercast_rpc::{
    rpc_code, AccessLevel, ChannelSummary, ClientRpcError, MockRpcTransport, PeerCastRpcClient,
    RpcDispatcher, RpcErrorObject, RpcState, RpcTransport,
};

fn sample_cid() -> ChannelId {
    ChannelId::from_hex("0123456789ABCDEF0123456789ABCDEF").unwrap()
}

/// Acceptance criterion 1:
/// Typed client adapter runs against mock transport without a real daemon (REQ-081).
#[tokio::test]
async fn test_p37b_mock_transport_offline_queries() {
    let mock = MockRpcTransport::new();

    // Register canned responses
    mock.register_success(
        "getVersionInfo",
        json!({
            "agentName": "PeerCastStation/1.0",
            "apiVersion": "1.0.0",
            "jsonrpc": "2.0"
        }),
    )
    .await;

    mock.register_success(
        "getStatus",
        json!({
            "uptime": 7325,
            "isFirewalled": false,
            "localRelayEndPoint": ["127.0.0.1", 7144],
            "globalRelayEndPoint": ["203.0.113.10", 7144]
        }),
    )
    .await;

    let client = PeerCastRpcClient::new(mock.clone());

    // 1. Version Info
    let ver = client.get_version_info().await.unwrap();
    assert_eq!(ver.agent_name, "PeerCastStation/1.0");
    assert_eq!(ver.api_version, "1.0.0");

    // 2. Status Snapshot to ViewModel transformation (REQ-081)
    let status = client.get_status().await.unwrap();
    assert_eq!(status.uptime_seconds, 7325);
    assert_eq!(status.uptime_formatted, "02:02:05");
    assert_eq!(status.port_status_label, "ポート開放中 (Open)");
    assert_eq!(status.local_endpoint, Some("127.0.0.1:7144".to_string()));
    assert_eq!(status.global_endpoint, Some("203.0.113.10:7144".to_string()));

    // Verify call log recorded
    let calls = mock.calls().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "getVersionInfo");
    assert_eq!(calls[1].0, "getStatus");
}

/// Acceptance criterion 2:
/// Channel summary to ViewModel conversion with formatted bitrate (REQ-081).
#[tokio::test]
async fn test_p37b_channel_viewmodel_conversion() {
    let mock = MockRpcTransport::new();
    let cid = sample_cid();

    let channel_summary = ChannelSummary::new(
        cid,
        "StreamOne",
        "Receiving",
        1500,
        "video/x-flv",
        10,
        25,
    );

    mock.register_success("getChannels", json!([channel_summary])).await;

    let client = PeerCastRpcClient::new(mock);
    let channels = client.get_channels().await.unwrap();

    assert_eq!(channels.len(), 1);
    let ch = &channels[0];
    assert_eq!(ch.name, "StreamOne");
    assert_eq!(ch.channel_id, cid.to_hex_upper());
    assert_eq!(ch.display_bitrate, "1500 kbps");
    assert_eq!(ch.raw_bitrate, 1500);
    assert_eq!(ch.total_relays, 10);
    assert_eq!(ch.total_directs, 25);
}

/// Acceptance criterion 3:
/// Error normalization and unauthorized mapping (REQ-064, REQ-081).
#[tokio::test]
async fn test_p37b_error_normalization() {
    let mock = MockRpcTransport::new();

    // Register unauthorized error (-32001)
    mock.register_error(
        "getSettings",
        RpcErrorObject::new(rpc_code::UNAUTHORIZED, "Admin access required"),
    )
    .await;

    let client = PeerCastRpcClient::new(mock);
    let err = client.get_settings().await.unwrap_err();

    assert_eq!(err, ClientRpcError::Unauthorized);
}

/// Acceptance criterion 4:
/// Explicit capability reporting for deferred methods; mock success is strictly forbidden from hiding unsupported status (REQ-064, Stop condition).
#[tokio::test]
async fn test_p37b_deferred_method_reports_unsupported_capability() {
    let mock = MockRpcTransport::new();

    // Deferred method broadcastChannel returns UNSUPPORTED_METHOD
    mock.register_error(
        "broadcastChannel",
        RpcErrorObject::unsupported_method("broadcastChannel", "broadcast_ingest"),
    )
    .await;

    let client = PeerCastRpcClient::new(mock);
    let err = client.broadcast_channel(json!({"name": "Test"})).await.unwrap_err();

    match err {
        ClientRpcError::UnsupportedMethod { method, capability } => {
            assert_eq!(method, "broadcastChannel");
            assert_eq!(capability, "broadcast_ingest");
        }
        other => panic!("Expected ClientRpcError::UnsupportedMethod, got {:?}", other),
    }
}

/// Acceptance criterion 5:
/// Mutation classification prevents blind automatic retries of uncertain mutations (REQ-084).
#[test]
fn test_p37b_mutation_retry_classification() {
    // Queries are safe / idempotent
    assert!(!PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("getVersionInfo"));
    assert!(!PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("getStatus"));
    assert!(!PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("getChannels"));
    assert!(!PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("getSettings"));

    // Mutations are non-idempotent or state-altering; must never be blindly retried on uncertain drop
    assert!(PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("setSettings"));
    assert!(PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("stopChannel"));
    assert!(PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("bumpChannel"));
    assert!(PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("setChannelInfo"));
    assert!(PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("addYellowPage"));
    assert!(PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("removeYellowPage"));
    assert!(PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("addListener"));
    assert!(PeerCastRpcClient::<MockRpcTransport>::is_mutating_method("broadcastChannel"));
}

/// Direct in-memory transport bridging PeerCastRpcClient directly to RpcDispatcher.
struct InprocessDispatcherTransport {
    dispatcher: RpcDispatcher,
    access: AccessLevel,
}

impl RpcTransport for InprocessDispatcherTransport {
    async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, ClientRpcError> {
        let req_json = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        });
        let req_str = serde_json::to_string(&req_json).unwrap();
        let res_str = self
            .dispatcher
            .handle_request_str(&req_str, self.access)
            .await
            .ok_or_else(|| ClientRpcError::Transport("No response returned".into()))?;

        let res_val: serde_json::Value = serde_json::from_str(&res_str)
            .map_err(|e| ClientRpcError::Deserialization(e.to_string()))?;

        if let Some(err_obj) = res_val.get("error") {
            let rpc_err: RpcErrorObject = serde_json::from_value(err_obj.clone())
                .map_err(|e| ClientRpcError::Deserialization(e.to_string()))?;
            Err(ClientRpcError::from(rpc_err))
        } else if let Some(res) = res_val.get("result") {
            Ok(res.clone())
        } else {
            Err(ClientRpcError::Deserialization("Invalid response envelope".into()))
        }
    }
}

/// Acceptance criterion 6:
/// End-to-end integration between PeerCastRpcClient and RpcDispatcher in-memory (REQ-064, REQ-081).
#[tokio::test]
async fn test_p37b_inprocess_client_dispatcher_roundtrip() {
    let mut state = RpcState::default();
    state.settings.max_relays = 25;
    state.settings.max_directs = 15;
    let dispatcher = RpcDispatcher::new(state);

    let transport = InprocessDispatcherTransport {
        dispatcher: dispatcher.clone(),
        access: AccessLevel::Admin,
    };
    let client = PeerCastRpcClient::new(transport);

    // 1. Read settings via typed client
    let initial_settings = client.get_settings().await.unwrap();
    assert_eq!(initial_settings.max_relays, 25);
    assert_eq!(initial_settings.max_directs, 15);

    // 2. Mutate settings via typed client
    let mut updated = initial_settings.clone();
    updated.max_relays = 50;
    let ok = client.set_settings(&updated).await.unwrap();
    assert!(ok);

    // 3. Verify updated in dispatcher state
    let read_back = client.get_settings().await.unwrap();
    assert_eq!(read_back.max_relays, 50);

    // 4. Verify access control: Playback client gets ClientRpcError::Unauthorized
    let playback_transport = InprocessDispatcherTransport {
        dispatcher: dispatcher.clone(),
        access: AccessLevel::Playback,
    };
    let playback_client = PeerCastRpcClient::new(playback_transport);
    let auth_err = playback_client.get_settings().await.unwrap_err();
    assert_eq!(auth_err, ClientRpcError::Unauthorized);
}
