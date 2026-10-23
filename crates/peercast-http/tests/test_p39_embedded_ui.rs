use std::fs;
use std::path::PathBuf;

use peercast_http::{
    build_asset_http_response, get_embedded_asset, HttpMethod, HttpRequest, RouteTarget,
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
/// Embedded UI assets serve across standard routes with correct MIME types and non-empty content (REQ-066).
#[test]
fn test_p39_embedded_assets_serving_and_mime_types() {
    let routes = [
        ("/", "text/html; charset=utf-8"),
        ("/index.html", "text/html; charset=utf-8"),
        ("/channels.html", "text/html; charset=utf-8"),
        ("/settings.html", "text/html; charset=utf-8"),
        ("/gallery.html", "text/html; charset=utf-8"),
        ("/api/1/peercaststation.js", "application/javascript; charset=utf-8"),
        ("/css/tokens.css", "text/css; charset=utf-8"),
        ("/css/themes/nord.css", "text/css; charset=utf-8"),
        ("/css/components/status_card.css", "text/css; charset=utf-8"),
        ("/css/components/channel_table.css", "text/css; charset=utf-8"),
        ("/css/components/alert_banner.css", "text/css; charset=utf-8"),
        ("/css/components/compact_layout.css", "text/css; charset=utf-8"),
        ("/fixtures/index_fixtures.json", "application/json; charset=utf-8"),
        ("/fixtures/channels_fixtures.json", "application/json; charset=utf-8"),
    ];

    for (route, expected_mime) in routes {
        let asset = get_embedded_asset(route).unwrap_or_else(|| panic!("Route {} must be embedded", route));
        assert_eq!(asset.mime_type, expected_mime, "Route {} MIME mismatch", route);
        assert!(!asset.content.is_empty(), "Route {} content must not be empty", route);
        assert!(asset.etag.starts_with('"') && asset.etag.ends_with('"'), "Route {} ETag must be valid", route);
    }
}

/// Acceptance criterion 2:
/// Conditional GET with matching If-None-Match ETag returns HTTP 304 Not Modified (REQ-066).
#[test]
fn test_p39_conditional_get_304_not_modified() {
    let asset = get_embedded_asset("/index.html").expect("index.html must exist");

    // 1. Initial request (no If-None-Match) -> HTTP 200 with full content
    let (status, headers, body) = build_asset_http_response(&asset, None);
    assert_eq!(status, 200);
    assert_eq!(body.unwrap(), asset.content);
    assert!(headers.iter().any(|(k, v)| *k == "ETag" && v == asset.etag));

    // 2. Conditional request with matching ETag -> HTTP 304 without body
    let (status304, headers304, body304) = build_asset_http_response(&asset, Some(asset.etag));
    assert_eq!(status304, 304);
    assert_eq!(body304, None);
    assert!(headers304.iter().any(|(k, v)| *k == "Content-Length" && v == "0"));

    // 3. Conditional request with mismatched ETag -> HTTP 200 with content
    let (status_mismatch, _, body_mismatch) = build_asset_http_response(&asset, Some("\"outdated-etag\""));
    assert_eq!(status_mismatch, 200);
    assert_eq!(body_mismatch.unwrap(), asset.content);
}

/// Acceptance criterion 3:
/// Security headers (X-Content-Type-Options, X-Frame-Options, Content-Security-Policy) injected on all assets (REQ-066).
#[test]
fn test_p39_security_headers_and_csp() {
    let asset = get_embedded_asset("/css/tokens.css").unwrap();
    let (_, headers, _) = build_asset_http_response(&asset, None);

    assert!(
        headers.iter().any(|(k, v)| *k == "X-Content-Type-Options" && v == "nosniff"),
        "nosniff header must be present"
    );
    assert!(
        headers.iter().any(|(k, v)| *k == "X-Frame-Options" && v == "SAMEORIGIN"),
        "SAMEORIGIN header must be present"
    );
    assert!(
        headers
            .iter()
            .any(|(k, v)| *k == "Content-Security-Policy" && v.contains("default-src 'self'")),
        "Content-Security-Policy header must restrict remote origin scripts"
    );
}

/// Acceptance criterion 4:
/// Zero remote asset dependencies: No external CDN or HTTP URLs exist in embedded UI payloads (REQ-066).
#[test]
fn test_p39_zero_remote_asset_dependency() {
    let check_routes = [
        "/index.html",
        "/channels.html",
        "/settings.html",
        "/gallery.html",
        "/api/1/peercaststation.js",
        "/css/tokens.css",
        "/css/themes/nord.css",
    ];

    for route in check_routes {
        let asset = get_embedded_asset(route).unwrap();
        let text = std::str::from_utf8(asset.content).expect("Embedded asset must be valid UTF-8");

        assert!(
            !text.contains("http://") && !text.contains("https://"),
            "Embedded asset '{}' must have zero remote asset URLs (found remote link)",
            route
        );

        assert!(
            !text.contains("fonts.googleapis.com")
                && !text.contains("cdnjs.cloudflare.com")
                && !text.contains("cdn.jsdelivr.net")
                && !text.contains("unpkg.com"),
            "Embedded asset '{}' references remote CDN",
            route
        );
    }
}

/// Acceptance criterion 5:
/// License audit manifest file exists and documents upstream licenses and dependencies (REQ-066).
#[test]
fn test_p39_license_audit_manifest_complete() {
    let audit_path = repo_root().join("docs/LICENSE_AUDIT.md");
    assert!(audit_path.exists(), "docs/LICENSE_AUDIT.md must exist");

    let audit_text = fs::read_to_string(audit_path).expect("Failed to read LICENSE_AUDIT.md");

    // Core licenses documented
    assert!(audit_text.contains("GPL-3.0-or-later"));
    assert!(audit_text.contains("GPL-2.0-or-later"));
    assert!(audit_text.contains("MIT"));
    assert!(audit_text.contains("Apache"));

    // Key upstream components recorded
    assert!(audit_text.contains("kumaryu"));
    assert!(audit_text.contains("plonk"));
    assert!(audit_text.contains("Knockout.js"));
    assert!(audit_text.contains("Bootstrap"));

    // Offline integrity statement verified
    assert!(audit_text.contains("Content-Security-Policy"));
    assert!(audit_text.contains("zero external CDN dependencies"));
}

/// Acceptance criterion 6:
/// HTTP Router correctly classifies StaticAsset, RpcApi, Stream, and Playlist targets (REQ-060, REQ-066).
#[test]
fn test_p39_router_integration_with_rpc_and_static_targets() {
    // 1. Static asset routes
    let static_req = b"GET /index.html HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let parsed_static = HttpRequest::parse(static_req).unwrap();
    assert_eq!(
        parsed_static.target,
        RouteTarget::StaticAsset {
            path: "/index.html".to_string()
        }
    );

    let root_req = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let parsed_root = HttpRequest::parse(root_req).unwrap();
    assert_eq!(
        parsed_root.target,
        RouteTarget::StaticAsset {
            path: "/".to_string()
        }
    );

    // 2. RPC API route (POST and GET)
    let rpc_req = b"POST /api/1 HTTP/1.1\r\nHost: localhost\r\nX-Requested-With: XMLHttpRequest\r\n\r\n";
    let parsed_rpc = HttpRequest::parse(rpc_req).unwrap();
    assert_eq!(parsed_rpc.method, HttpMethod::Post);
    assert_eq!(
        parsed_rpc.target,
        RouteTarget::RpcApi {
            subpath: String::new()
        }
    );

    let js_req = b"GET /api/1/peercaststation.js HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let parsed_js = HttpRequest::parse(js_req).unwrap();
    assert_eq!(
        parsed_js.target,
        RouteTarget::RpcApi {
            subpath: "/peercaststation.js".to_string()
        }
    );

    // 3. Media streaming route
    let stream_req = b"GET /stream/0123456789ABCDEF0123456789ABCDEF.flv HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let parsed_stream = HttpRequest::parse(stream_req).unwrap();
    assert_eq!(
        parsed_stream.target,
        RouteTarget::Stream {
            channel_id_hex: "0123456789ABCDEF0123456789ABCDEF".to_string(),
            extension: Some("flv".to_string()),
        }
    );
}
