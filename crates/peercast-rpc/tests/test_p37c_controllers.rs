use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

use peercast_rpc::{
    ChannelsController, ClientRpcError, ControllerError, IndexController, MockRpcTransport,
    MutationState, PeerCastRpcClient, RpcTransport, ScreenState, ServentSettings,
    SettingsController,
};

/// Acceptance criterion 1:
/// IndexController loads aggregated screen data and transitions between Initial -> Loaded (REQ-081).
#[tokio::test]
async fn test_p37c_index_controller_lifecycle() {
    let mock = MockRpcTransport::new();
    mock.register_success(
        "getVersionInfo",
        json!({
            "agentName": concat!("PeerCastStation/", env!("CARGO_PKG_VERSION")),
            "apiVersion": "1.0.0",
            "jsonrpc": "2.0"
        }),
    )
    .await;
    mock.register_success(
        "getStatus",
        json!({
            "uptime": 120,
            "isFirewalled": false,
            "localRelayEndPoint": ["127.0.0.1", 7144],
            "globalRelayEndPoint": ["198.51.100.1", 7144]
        }),
    )
    .await;
    mock.register_success(
        "getPlugins",
        json!([
            {
                "name": "PCP",
                "isUsable": true,
                "assembly": {
                    "name": "pcp-plugin",
                    "version": "1.0.0",
                    "copyright": "PeerCastStation"
                }
            }
        ]),
    )
    .await;

    let client = PeerCastRpcClient::new(mock);
    let controller = IndexController::new(client);

    // Initial state
    assert_eq!(controller.state().await, ScreenState::Initial);

    // Refresh and transition to Loaded
    controller.refresh().await.unwrap();

    let state = controller.state().await;
    assert!(state.is_loaded());
    let data = state.data().expect("screen data must be present");
    assert_eq!(
        data.version.agent_name,
        concat!("PeerCastStation/", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(data.status.uptime_formatted, "00:02:00");
    assert_eq!(data.plugins.len(), 1);
    assert_eq!(data.plugins[0].name, "PCP");
}

/// Acceptance criterion 2:
/// IndexController correctly maps Unauthorized to ScreenState::Forbidden (REQ-081).
#[tokio::test]
async fn test_p37c_index_controller_unauthorized() {
    let mock = MockRpcTransport::new();
    mock.register_success(
        "getVersionInfo",
        json!({
            "agentName": concat!("PeerCastStation/", env!("CARGO_PKG_VERSION")),
            "apiVersion": "1.0.0",
            "jsonrpc": "2.0"
        }),
    )
    .await;
    // Status returns unauthorized
    mock.register_client_error("getStatus", ClientRpcError::Unauthorized)
        .await;
    mock.register_success("getPlugins", json!([])).await;

    let client = PeerCastRpcClient::new(mock);
    let controller = IndexController::new(client);

    controller.refresh().await.unwrap();

    match controller.state().await {
        ScreenState::Forbidden { reason } => {
            assert!(reason.contains("Admin authorization required"));
        }
        other => panic!("Expected ScreenState::Forbidden, got {:?}", other),
    }
}

/// Acceptance criterion 3:
/// IndexController maps transport drop to ScreenState::Disconnected (REQ-084).
#[tokio::test]
async fn test_p37c_index_controller_disconnected() {
    let mock = MockRpcTransport::new();
    mock.register_client_error(
        "getVersionInfo",
        ClientRpcError::Transport("Connection refused (os error 10061)".to_string()),
    )
    .await;
    mock.register_success("getStatus", json!({})).await;
    mock.register_success("getPlugins", json!([])).await;

    let client = PeerCastRpcClient::new(mock);
    let controller = IndexController::new(client);

    let err = controller.refresh().await.unwrap_err();
    assert!(matches!(
        err,
        ControllerError::Client(ClientRpcError::Transport(_))
    ));
    assert_eq!(controller.state().await, ScreenState::Disconnected);
}

/// Acceptance criterion 4:
/// Delayed / superseded response is rejected with StaleResponseIgnored and does not overwrite state (REQ-084).
#[tokio::test]
async fn test_p37c_index_controller_stale_response_ignored() {
    // Custom transport that pauses on first call to simulate network delay
    #[derive(Clone)]
    struct DelayedTestTransport {
        pause: Arc<Notify>,
        paused_once: Arc<AtomicBool>,
    }

    impl RpcTransport for DelayedTestTransport {
        async fn send_request(
            &self,
            method: &str,
            _params: Option<serde_json::Value>,
        ) -> Result<serde_json::Value, ClientRpcError> {
            if method == "getVersionInfo" && !self.paused_once.swap(true, Ordering::SeqCst) {
                // First call waits on notify
                self.pause.notified().await;
                Ok(json!({
                    "agentName": "StalePeerCast/0.9",
                    "apiVersion": "0.9.0",
                    "jsonrpc": "2.0"
                }))
            } else if method == "getVersionInfo" {
                // Second call returns immediately
                Ok(json!({
                    "agentName": "FreshPeerCast/1.0",
                    "apiVersion": "1.0.0",
                    "jsonrpc": "2.0"
                }))
            } else if method == "getStatus" {
                Ok(json!({
                    "uptime": 500,
                    "isFirewalled": false
                }))
            } else {
                Ok(json!([]))
            }
        }
    }

    let pause = Arc::new(Notify::new());
    let transport = DelayedTestTransport {
        pause: pause.clone(),
        paused_once: Arc::new(AtomicBool::new(false)),
    };

    let client = PeerCastRpcClient::new(transport);
    let controller = Arc::new(IndexController::new(client));

    // Spawn first refresh (will pause on getVersionInfo)
    let c1 = controller.clone();
    let handle1 = tokio::spawn(async move { c1.refresh().await });

    // Allow task 1 to start and hit the pause point
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Run second refresh to completion
    let res2 = controller.refresh().await;
    assert!(res2.is_ok());

    let state = controller.state().await;
    let data = state.data().expect("state should be loaded");
    assert_eq!(data.version.agent_name, "FreshPeerCast/1.0");

    // Unpause first refresh
    pause.notify_one();
    let res1 = handle1.await.unwrap();

    // Verify first refresh was rejected due to staleness
    match res1 {
        Err(ControllerError::StaleResponseIgnored { expected, received }) => {
            assert!(expected > received);
        }
        other => panic!("Expected StaleResponseIgnored, got {:?}", other),
    }

    // State remains FreshPeerCast/1.0 (not overwritten by stale response)
    let state_after = controller.state().await;
    let data_after = state_after.data().expect("state should remain loaded");
    assert_eq!(data_after.version.agent_name, "FreshPeerCast/1.0");
}

/// Acceptance criterion 5:
/// SettingsController mutation re-entrancy guard blocks concurrent modifications (REQ-084, Global Rule 2).
#[tokio::test]
async fn test_p37c_settings_controller_reentrancy_guard() {
    #[derive(Clone)]
    struct SlowMutationTransport {
        pause: Arc<Notify>,
    }

    impl RpcTransport for SlowMutationTransport {
        async fn send_request(
            &self,
            method: &str,
            _params: Option<serde_json::Value>,
        ) -> Result<serde_json::Value, ClientRpcError> {
            if method == "setSettings" {
                self.pause.notified().await;
                Ok(json!(true))
            } else {
                Ok(json!({}))
            }
        }
    }

    let pause = Arc::new(Notify::new());
    let transport = SlowMutationTransport {
        pause: pause.clone(),
    };
    let client = PeerCastRpcClient::new(transport);
    let controller = Arc::new(SettingsController::new(client));

    // Start first mutation
    let c1 = controller.clone();
    let handle1 = tokio::spawn(async move {
        c1.update_settings(ServentSettings {
            max_relays: 42,
            ..Default::default()
        })
        .await
    });

    // Wait until mutation enters pending state
    tokio::time::sleep(Duration::from_millis(20)).await;
    let mutation_state = controller.mutation_state().await;
    assert!(mutation_state.is_pending());

    // Attempt second mutation while first is pending: must be rejected by re-entrancy guard
    let dup_err = controller
        .update_settings(ServentSettings {
            max_relays: 43,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(dup_err, ControllerError::DuplicatePendingMutation);

    // Let first mutation finish
    pause.notify_one();
    let res1 = handle1.await.unwrap();
    assert!(res1.is_ok());

    assert_eq!(controller.mutation_state().await, MutationState::Succeeded);
    let current_view = controller.view_state().await;
    let loaded = current_view.data().unwrap();
    assert_eq!(loaded.max_relays, 42);
}

/// Acceptance criterion 6:
/// SettingsController prohibits blind retries on transport drop mid-mutation (REQ-084).
#[tokio::test]
async fn test_p37c_settings_controller_uncertain_retry_forbidden() {
    let mock = MockRpcTransport::new();
    mock.register_client_error(
        "setSettings",
        ClientRpcError::Transport("Broken pipe (os error 32)".to_string()),
    )
    .await;

    let client = PeerCastRpcClient::new(mock);
    let controller = SettingsController::new(client);

    let err = controller
        .update_settings(ServentSettings::default())
        .await
        .unwrap_err();

    assert_eq!(err, ControllerError::UncertainMutationRetryForbidden);
    assert_eq!(controller.mutation_state().await, MutationState::Failed);
}

/// Acceptance criterion 7:
/// ChannelsController differentiates Loaded versus Empty states and handles authorization (REQ-081).
#[tokio::test]
async fn test_p37c_channels_controller_loaded_vs_empty() {
    let mock = MockRpcTransport::new();
    // 1. Empty channel list
    mock.register_success("getChannels", json!([])).await;
    let client = PeerCastRpcClient::new(mock.clone());
    let controller = ChannelsController::new(client);

    controller.load_channels().await.unwrap();
    assert_eq!(controller.state().await, ScreenState::Empty);

    // 2. Non-empty channel list
    mock.register_success(
        "getChannels",
        json!([
            {
                "channelId": "0123456789ABCDEF0123456789ABCDEF",
                "name": "Live Broadcast",
                "tracker": "http://yp.example.com",
                "totalDirects": 10,
                "totalRelays": 5
            }
        ]),
    )
    .await;

    controller.load_channels().await.unwrap();
    let state = controller.state().await;
    assert!(state.is_loaded());
    let channels = state.data().unwrap();
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].name, "Live Broadcast");
    assert_eq!(channels[0].total_directs, 10);
    assert_eq!(channels[0].total_relays, 5);

    // 3. Unauthorized access
    mock.register_client_error("getChannels", ClientRpcError::Unauthorized)
        .await;
    controller.load_channels().await.unwrap();
    match controller.state().await {
        ScreenState::Forbidden { reason } => {
            assert!(reason.contains("Channels view forbidden"));
        }
        other => panic!("Expected ScreenState::Forbidden, got {:?}", other),
    }
}
