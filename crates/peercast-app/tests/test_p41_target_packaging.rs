use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use peercast_http::{get_embedded_asset, EmbeddedAsset};
use sha2::{Digest, Sha256};

static CLEAN_DIR_COUNTER: AtomicU64 = AtomicU64::new(500);

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn locate_app_binary() -> PathBuf {
    let root = repo_root();
    let release_bin = root.join("target").join("release").join(if cfg!(windows) {
        "peercast-app.exe"
    } else {
        "peercast-app"
    });
    if release_bin.exists() {
        return release_bin;
    }

    let debug_bin = root.join("target").join("debug").join(if cfg!(windows) {
        "peercast-app.exe"
    } else {
        "peercast-app"
    });
    if debug_bin.exists() {
        return debug_bin;
    }

    panic!("peercast-app executable not found in target/release or target/debug");
}

fn create_clean_dir(name: &str) -> PathBuf {
    let pid = std::process::id();
    let counter = CLEAN_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("peercast_clean_{}_{}_{}", name, pid, counter));
    fs::create_dir_all(&dir).expect("failed to create clean test temp dir");
    dir
}

/// Acceptance criterion 1:
/// Target executable exists, size is self-contained and bounded, and SHA-256 hash is computed (REQ-066, REQ-070).
#[test]
fn test_p41_target_binary_exists_and_hash_computed() {
    let bin_path = locate_app_binary();
    assert!(bin_path.exists(), "Binary must exist: {}", bin_path.display());

    let bytes = fs::read(&bin_path).expect("failed to read binary bytes");
    assert!(!bytes.is_empty(), "Binary bytes must not be empty");

    // Single self-contained binary size verification (REQ-066: bounded, compact)
    assert!(
        bytes.len() < 10 * 1024 * 1024,
        "Executable size must be under 10 MiB, actual size: {} bytes",
        bytes.len()
    );

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let hex_hash = format!("{:02x}", digest);
    assert_eq!(hex_hash.len(), 64, "SHA-256 hex digest must be 64 characters");
}

/// Acceptance criterion 2:
/// Clean-machine isolated directory startup and default configuration generation (REQ-067, REQ-070).
#[test]
fn test_p41_clean_machine_isolated_startup() {
    let bin_path = locate_app_binary();
    let clean_dir = create_clean_dir("startup");
    let target_cfg = clean_dir.join("peercast.json");

    assert!(!target_cfg.exists(), "Config file must not exist before clean startup");

    // Run peercast-app --kill in clean directory (performs initialization and clean exit)
    let output = Command::new(&bin_path)
        .arg("--config")
        .arg(&target_cfg)
        .arg("--port")
        .arg("7197")
        .arg("--headless")
        .arg("--kill")
        .current_dir(&clean_dir)
        .output()
        .expect("failed to execute peercast-app");

    assert!(output.status.success(), "Clean startup with --kill should exit with 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Kill requested") || stdout.contains("signaling"));

    // Verify config file was automatically generated with valid versioned schema
    assert!(target_cfg.exists(), "Default config must be automatically generated");
    let cfg_text = fs::read_to_string(&target_cfg).expect("failed to read generated config");
    assert!(cfg_text.contains("\"schemaVersion\": 2"), "Generated config must be schema v2");
    assert!(cfg_text.contains("\"port\": 7197"), "CLI port override must be persisted in config");

    let _ = fs::remove_dir_all(clean_dir);
}

/// Acceptance criterion 3:
/// Standalone CLI invocation checks: --help and --version (REQ-067).
#[test]
fn test_p41_cli_flags_invocation() {
    let bin_path = locate_app_binary();

    // 1. --version
    let ver_out = Command::new(&bin_path)
        .arg("--version")
        .output()
        .expect("failed to execute --version");
    assert!(ver_out.status.success());
    let ver_str = String::from_utf8_lossy(&ver_out.stdout);
    assert!(ver_str.contains("PeerCastStation (Rust Edition) v0.1.0"));

    // 2. --help
    let help_out = Command::new(&bin_path)
        .arg("--help")
        .output()
        .expect("failed to execute --help");
    assert!(help_out.status.success());
    let help_str = String::from_utf8_lossy(&help_out.stdout);
    assert!(help_str.contains("Usage: peercast-station"));
    assert!(help_str.contains("--port"));
    assert!(help_str.contains("--headless"));
    assert!(help_str.contains("--kill"));
}

/// Acceptance criterion 4:
/// Embedded UI assets are compiled into the binary with zero remote dependencies (REQ-066).
#[test]
fn test_p41_embedded_ui_self_containment() {
    let required_assets = [
        "/index.html",
        "/channels.html",
        "/settings.html",
        "/gallery.html",
        "/api/1/peercaststation.js",
        "/css/tokens.css",
        "/css/themes/nord.css",
        "/css/components/status_card.css",
        "/css/components/channel_table.css",
        "/css/components/alert_banner.css",
        "/css/components/compact_layout.css",
    ];

    for asset_path in required_assets {
        let asset: Option<EmbeddedAsset> = get_embedded_asset(asset_path);
        assert!(asset.is_some(), "Asset {} must be embedded in binary", asset_path);
        let a = asset.unwrap();
        assert!(!a.content.is_empty(), "Asset {} must have non-empty payload", asset_path);

        // Verify zero external CDN references
        let content_str = String::from_utf8_lossy(a.content);
        assert!(
            !content_str.contains("https://cdn.") && !content_str.contains("http://cdn."),
            "Embedded asset {} must not contain remote CDN URLs",
            asset_path
        );
    }
}

/// Acceptance criterion 5:
/// License notices, copyright manifest, and packaging documentation complete (REQ-066, REQ-070).
#[test]
fn test_p41_license_and_packaging_documentation() {
    let root = repo_root();

    let license_audit = root.join("docs").join("LICENSE_AUDIT.md");
    assert!(license_audit.exists(), "LICENSE_AUDIT.md must exist");
    let audit_text = fs::read_to_string(&license_audit).unwrap();
    assert!(audit_text.contains("GPL-3.0-or-later"));
    assert!(audit_text.contains("kumaryu"));
    assert!(audit_text.contains("plonk"));

    let target_packaging = root.join("docs").join("TARGET_PACKAGING.md");
    assert!(target_packaging.exists(), "TARGET_PACKAGING.md must exist");
    let pkg_text = fs::read_to_string(&target_packaging).unwrap();
    assert!(pkg_text.contains("x86_64-pc-windows-msvc"));
    assert!(pkg_text.contains("x86_64-unknown-linux-gnu"));
    assert!(pkg_text.contains("Dynamic Runtime Requirements"));
}
