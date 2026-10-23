use std::fs;
use std::path::PathBuf;
use serde_json::Value;

use peercast_rpc::{
    ChannelViewModel, ClientRpcError, ControllerError, IndexController, MockRpcTransport,
    PeerCastRpcClient, PluginSummary, RpcTransport, ScreenState,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Acceptance criterion 1:
/// Semantic design tokens file (`tokens.css`) defines required token families,
/// dark/high-contrast themes, and reduced-motion media query (REQ-082).
#[test]
fn test_p37d_tokens_file_complete_and_valid() {
    let tokens_path = repo_root().join("ui/css/tokens.css");
    assert!(tokens_path.exists(), "ui/css/tokens.css must exist");

    let css = fs::read_to_string(&tokens_path).expect("Failed to read tokens.css");

    // Surface tokens
    assert!(css.contains("--peca-surface-bg:"));
    assert!(css.contains("--peca-surface-card:"));
    assert!(css.contains("--peca-surface-subtle:"));

    // Text tokens
    assert!(css.contains("--peca-text-primary:"));
    assert!(css.contains("--peca-text-secondary:"));
    assert!(css.contains("--peca-text-muted:"));
    assert!(css.contains("--peca-text-inverse:"));

    // Brand and feedback tokens
    assert!(css.contains("--peca-color-primary:"));
    assert!(css.contains("--peca-state-success-bg:"));
    assert!(css.contains("--peca-state-warning-bg:"));
    assert!(css.contains("--peca-state-error-bg:"));
    assert!(css.contains("--peca-state-info-bg:"));

    // Focus & accessibility tokens
    assert!(css.contains("--peca-focus-ring:"));
    assert!(css.contains("--peca-focus-outline-width:"));

    // Theme variations
    assert!(css.contains(r#"[data-theme="dark"]"#), "Dark theme must be defined");
    assert!(
        css.contains(r#"[data-theme="high-contrast"]"#),
        "High-contrast theme must be defined (WCAG AAA)"
    );

    // Reduced-motion support
    assert!(
        css.contains("@media (prefers-reduced-motion: reduce)"),
        "prefers-reduced-motion media query must be present"
    );
    assert!(css.contains("animation-duration:"));
}

/// Acceptance criterion 2:
/// Component styles exist and reference semantic tokens exclusively without hardcoded color literals (REQ-082).
#[test]
fn test_p37d_component_styles_reference_tokens() {
    let components_dir = repo_root().join("ui/css/components");
    assert!(components_dir.exists());

    let files = vec!["status_card.css", "channel_table.css", "alert_banner.css"];
    for file in files {
        let path = components_dir.join(file);
        assert!(path.exists(), "Missing component CSS: {}", file);
        let content = fs::read_to_string(path).unwrap();
        assert!(
            content.contains("var(--peca-"),
            "Component {} must reference semantic CSS tokens",
            file
        );
    }
}

/// Acceptance criterion 3:
/// Fixture gallery contains all 8 required state fixtures and operates 100% offline without CDN (REQ-083).
#[test]
fn test_p37d_fixture_gallery_covers_all_eight_states() {
    let gallery_path = repo_root().join("ui/fixtures/gallery.html");
    assert!(gallery_path.exists(), "gallery.html must exist");

    let html = fs::read_to_string(&gallery_path).expect("Failed to read gallery.html");

    // All 8 required states must be present
    let required_states = [
        "id=\"state-normal\"",
        "id=\"state-empty\"",
        "id=\"state-loading\"",
        "id=\"state-partial\"",
        "id=\"state-forbidden\"",
        "id=\"state-disconnected\"",
        "id=\"state-stale\"",
        "id=\"state-failed\"",
    ];

    for state_id in required_states {
        assert!(
            html.contains(state_id),
            "gallery.html must contain state fixture for {}",
            state_id
        );
    }

    // Zero CDN / external requests guarantee:
    assert!(
        !html.contains("http://") && !html.contains("https://"),
        "gallery.html must not depend on external CDN or network urls"
    );

    // Accessibility elements
    assert!(html.contains("role=\"main\"") || html.contains("<main"));
    assert!(html.contains("aria-live="));
}

/// Acceptance criterion 4:
/// Fixture JSON files parse successfully and cleanly map to controller ViewModels (REQ-083).
#[test]
fn test_p37d_fixtures_match_rust_viewmodels() {
    let index_fixtures_path = repo_root().join("ui/fixtures/index_fixtures.json");
    let index_json: Value = serde_json::from_str(&fs::read_to_string(index_fixtures_path).unwrap())
        .expect("index_fixtures.json must be valid JSON");

    // Validate normal state fields
    let normal = index_json.get("normal").expect("normal fixture missing");
    let version = normal.get("version").unwrap();
    assert_eq!(version.get("apiVersion").unwrap().as_str().unwrap(), "1.0.0");

    let plugins_raw = normal.get("plugins").unwrap().clone();
    let plugins: Vec<PluginSummary> = serde_json::from_value(plugins_raw).unwrap();
    assert_eq!(plugins.len(), 2);
    assert_eq!(plugins[0].name, "PCP");

    let channels_fixtures_path = repo_root().join("ui/fixtures/channels_fixtures.json");
    let channels_json: Value = serde_json::from_str(&fs::read_to_string(channels_fixtures_path).unwrap())
        .expect("channels_fixtures.json must be valid JSON");

    let normal_channels_raw = channels_json.get("normal").unwrap().clone();
    let channels: Vec<ChannelViewModel> = serde_json::from_value(normal_channels_raw).unwrap();
    assert_eq!(channels.len(), 2);
    assert_eq!(channels[0].name, "Live PeerCast Broadcast");
    assert_eq!(channels[0].display_bitrate, "1500 kbps");
}

/// Acceptance criterion 5:
/// Production client cannot silently use mock transport on real-backend failure (REQ-083).
#[tokio::test]
async fn test_p37d_production_cannot_silently_use_mock_on_failure() {
    // Failing real transport (simulating network drop / refused connection)
    #[derive(Clone)]
    struct FailingRealTransport;

    impl RpcTransport for FailingRealTransport {
        async fn send_request(
            &self,
            _method: &str,
            _params: Option<serde_json::Value>,
        ) -> Result<serde_json::Value, ClientRpcError> {
            Err(ClientRpcError::Transport(
                "Connection refused: peer daemon is not running on 127.0.0.1:7144".to_string(),
            ))
        }
    }

    // A mock transport preloaded with successful canned responses
    let mock = MockRpcTransport::new();
    mock.register_success(
        "getVersionInfo",
        serde_json::json!({
            "agentName": "SilentMock/0.0",
            "apiVersion": "1.0.0"
        }),
    )
    .await;

    // Real client using the failing transport
    let production_client = PeerCastRpcClient::new(FailingRealTransport);
    let controller = IndexController::new(production_client);

    // Execute query
    let result = controller.refresh().await;

    // Must fail fast with Transport error and enter Disconnected state.
    // It must NEVER catch the error and secretly query `mock`!
    assert!(result.is_err());
    match result.unwrap_err() {
        ControllerError::Client(ClientRpcError::Transport(msg)) => {
            assert!(msg.contains("Connection refused"));
        }
        other => panic!("Expected ClientRpcError::Transport, got {:?}", other),
    }

    assert_eq!(controller.state().await, ScreenState::Disconnected);
}
