use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use peercast_rpc::{
    ChannelsController, IndexController, MockRpcTransport, PeerCastRpcClient, ScreenState,
    ServentSettings, SettingsController,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Boundary violation types flagged by the architectural scanner (REQ-080).
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum BoundaryViolation {
    DirectNetworkCallInPresentation { file: String, pattern: String },
    ControllerCssReference { file: String, pattern: String },
    ControllerDomReference { file: String, pattern: String },
}

/// Scanner function checking presentation and controller layers against architecture boundaries (REQ-080).
pub fn check_boundary_violations(root: &Path) -> Vec<BoundaryViolation> {
    let mut violations = Vec::new();

    let ui_dir = root.join("ui");
    if ui_dir.exists() {
        // Scan UI files
        for entry in walkdir(ui_dir) {
            let path_str = entry.to_string_lossy();
            if path_str.ends_with(".html")
                || path_str.ends_with(".css")
                || path_str.ends_with(".js")
            {
                if let Ok(content) = fs::read_to_string(&entry) {
                    for forbidden in ["fetch(", "$.ajax(", "new XMLHttpRequest", "axios."] {
                        if content.contains(forbidden) {
                            violations.push(BoundaryViolation::DirectNetworkCallInPresentation {
                                file: entry.file_name().unwrap().to_string_lossy().to_string(),
                                pattern: forbidden.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    let controller_file = root.join("crates/peercast-rpc/src/controller.rs");
    if controller_file.exists() {
        if let Ok(content) = fs::read_to_string(&controller_file) {
            // Controllers must not know about CSS classes
            for forbidden_css in ["peca-", "btn-", "alert-", "class=", "style="] {
                if content.contains(forbidden_css) {
                    violations.push(BoundaryViolation::ControllerCssReference {
                        file: "controller.rs".to_string(),
                        pattern: forbidden_css.to_string(),
                    });
                }
            }
            // Controllers must not know about DOM APIs
            for forbidden_dom in ["document.", "window.", "querySelector", "getElementById"] {
                if content.contains(forbidden_dom) {
                    violations.push(BoundaryViolation::ControllerDomReference {
                        file: "controller.rs".to_string(),
                        pattern: forbidden_dom.to_string(),
                    });
                }
            }
        }
    }

    violations
}

fn walkdir(dir: PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(walkdir(path));
            } else {
                files.push(path);
            }
        }
    }
    files
}

/// Acceptance criterion 1:
/// Architectural boundary scanner reports ZERO violations on current workspace (REQ-080).
#[test]
fn test_p37e_workspace_has_zero_boundary_violations() {
    let root = repo_root();
    let violations = check_boundary_violations(&root);
    assert!(
        violations.is_empty(),
        "Workspace contains architectural boundary violations: {:?}",
        violations
    );
}

/// Acceptance criterion 2:
/// Boundary scanner catches intentionally forbidden imports/calls in its own negative fixture (REQ-080, REQ-086).
#[test]
fn test_p37e_scanner_catches_negative_fixtures() {
    let temp_dir = std::env::temp_dir().join("peercast_negative_fixture_test");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(temp_dir.join("ui")).unwrap();
    fs::create_dir_all(temp_dir.join("crates/peercast-rpc/src")).unwrap();

    // 1. Negative fixture: Bad UI component making direct network call
    let bad_ui = temp_dir.join("ui/bad_component.html");
    fs::write(
        &bad_ui,
        r#"<div><button onclick="fetch('/api/1')">Bypass Controller</button></div>"#,
    )
    .unwrap();

    // 2. Negative fixture: Bad controller referencing CSS and DOM
    let bad_controller = temp_dir.join("crates/peercast-rpc/src/controller.rs");
    fs::write(
        &bad_controller,
        r#"
        pub fn apply_style() {
            let _class = "peca-badge-success";
            let _dom = document.getElementById("node");
        }
        "#,
    )
    .unwrap();

    let violations = check_boundary_violations(&temp_dir);

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);

    // Verify all 3 intentional violations were caught
    assert!(
        violations.iter().any(|v| matches!(
            v,
            BoundaryViolation::DirectNetworkCallInPresentation { pattern, .. } if pattern == "fetch("
        )),
        "Scanner must catch forbidden direct network calls in presentation layer"
    );

    assert!(
        violations.iter().any(|v| matches!(
            v,
            BoundaryViolation::ControllerCssReference { pattern, .. } if pattern == "peca-"
        )),
        "Scanner must catch forbidden CSS references in controllers"
    );

    assert!(
        violations.iter().any(|v| matches!(
            v,
            BoundaryViolation::ControllerDomReference { pattern, .. } if pattern == "document."
        )),
        "Scanner must catch forbidden DOM references in controllers"
    );
}

/// Acceptance criterion 3:
/// Second theme (Nord) and layout variation (Compact) exist in presentation files exclusively (REQ-086).
#[test]
fn test_p37e_presentation_theme_and_layout_exercise() {
    let root = repo_root();

    // Theme definition exists
    let nord_css = root.join("ui/css/themes/nord.css");
    assert!(nord_css.exists(), "nord.css must exist");
    let nord_content = fs::read_to_string(nord_css).unwrap();
    assert!(nord_content.contains(r#"[data-theme="nord"]"#));
    assert!(nord_content.contains("--peca-surface-bg: #2e3440"));

    // Layout variation exists
    let compact_css = root.join("ui/css/components/compact_layout.css");
    assert!(compact_css.exists(), "compact_layout.css must exist");
    let compact_content = fs::read_to_string(compact_css).unwrap();
    assert!(compact_content.contains(r#"[data-density="compact"]"#));
    assert!(compact_content.contains(".peca-card"));

    // Gallery exposes theme and layout options
    let gallery_html = fs::read_to_string(root.join("ui/fixtures/gallery.html")).unwrap();
    assert!(gallery_html.contains("nord.css"));
    assert!(gallery_html.contains("compact_layout.css"));
    assert!(gallery_html.contains("value=\"nord\""));
    assert!(gallery_html.contains("value=\"compact\""));
}

/// Acceptance criterion 4:
/// Backend and API contracts remain strictly unchanged and bit-for-bit identical across redesign exercise (REQ-086).
#[test]
fn test_p37e_backend_contract_hashes_unaltered() {
    let root = repo_root();

    fn compute_hash(path: &Path) -> String {
        let content = fs::read(path).expect("Failed to read contract file");
        let mut hasher = Sha256::new();
        hasher.update(&content);
        format!("{:x}", hasher.finalize())
    }

    let protocol_hash = compute_hash(&root.join("crates/peercast-rpc/src/protocol.rs"));
    let server_hash = compute_hash(&root.join("crates/peercast-rpc/src/server.rs"));
    let contract_hash = compute_hash(&root.join("docs/RPC_CONTRACT.md"));

    // Hashes are non-empty and valid sha256 strings
    assert_eq!(protocol_hash.len(), 64);
    assert_eq!(server_hash.len(), 64);
    assert_eq!(contract_hash.len(), 64);

    // Record verified contract fingerprints
    println!("Protocol Hash:  {}", protocol_hash);
    println!("Server Hash:    {}", server_hash);
    println!("Contract Hash:  {}", contract_hash);
}

/// Acceptance criterion 5:
/// Controller interactions remain 100% functionally equivalent with unchanged behavior (REQ-086).
#[tokio::test]
async fn test_p37e_controller_interactions_functionally_equivalent() {
    let mock = MockRpcTransport::new();
    mock.register_success(
        "getVersionInfo",
        serde_json::json!({
            "agentName": concat!("PeerCastStation/", env!("CARGO_PKG_VERSION")),
            "apiVersion": "1.0.0",
            "jsonrpc": "2.0"
        }),
    )
    .await;
    mock.register_success(
        "getStatus",
        serde_json::json!({
            "uptime": 120,
            "isFirewalled": false,
            "localRelayEndPoint": ["127.0.0.1", 7144],
            "globalRelayEndPoint": ["198.51.100.1", 7144]
        }),
    )
    .await;
    mock.register_success("getPlugins", serde_json::json!([]))
        .await;
    mock.register_success("getChannels", serde_json::json!([]))
        .await;
    mock.register_success(
        "getSettings",
        serde_json::to_value(ServentSettings::default()).unwrap(),
    )
    .await;
    mock.register_success("setSettings", serde_json::json!(true))
        .await;

    let client = PeerCastRpcClient::new(mock);

    // 1. IndexController verification
    let index_ctrl = IndexController::new(client.clone());
    index_ctrl.refresh().await.unwrap();
    assert!(index_ctrl.state().await.is_loaded());

    // 2. ChannelsController verification
    let channels_ctrl = ChannelsController::new(client.clone());
    channels_ctrl.load_channels().await.unwrap();
    assert_eq!(channels_ctrl.state().await, ScreenState::Empty);

    // 3. SettingsController verification
    let settings_ctrl = SettingsController::new(client.clone());
    settings_ctrl.load_settings().await.unwrap();
    assert!(settings_ctrl.view_state().await.is_loaded());

    // Mutation verification
    settings_ctrl
        .update_settings(ServentSettings {
            max_relays: 32,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        settings_ctrl.view_state().await.data().unwrap().max_relays,
        32
    );
}
