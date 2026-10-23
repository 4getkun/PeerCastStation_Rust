use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use serde_json::json;

use peercast_rpc::{
    redact_secrets, validate_relay_destination, HttpRequestContext,
    RpcDispatcher, RpcState, SecureRpcHandler, SecurityError,
    SecurityPolicy, ServentSettings,
};

fn loopback_client() -> SocketAddr {
    "127.0.0.1:52134".parse().unwrap()
}

fn remote_client() -> SocketAddr {
    "203.0.113.195:48912".parse().unwrap()
}

fn create_test_handler(policy: SecurityPolicy) -> SecureRpcHandler {
    let state = RpcState {
        settings: ServentSettings {
            max_relays: 16,
            ..Default::default()
        },
        ..Default::default()
    };
    let dispatcher = Arc::new(RpcDispatcher::new(state));
    SecureRpcHandler::new(dispatcher, policy)
}

/// Acceptance criterion 1:
/// Loopback default permits administrative operations when originated from localhost (REQ-065).
#[tokio::test]
async fn test_p38_loopback_default_access() {
    let handler = create_test_handler(SecurityPolicy::loopback());

    // 1. Read query over loopback succeeds
    let get_req = HttpRequestContext::new(loopback_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "method": "getStatus",
                "id": 1
            })
            .to_string(),
        );

    let resp = handler.handle_request(get_req).await;
    assert_eq!(resp.status_code, 200);
    assert!(resp.body.unwrap().contains("\"result\""));

    // 2. Mutation over loopback succeeds without auth token by default
    let set_req = HttpRequestContext::new(loopback_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "method": "setSettings",
                "params": { "maxRelays": 24 },
                "id": 2
            })
            .to_string(),
        );

    let resp = handler.handle_request(set_req).await;
    assert_eq!(resp.status_code, 200);
    assert!(resp.body.unwrap().contains("\"result\":true"));
}

/// Acceptance criterion 2:
/// Public relay listener strictly blocks administrative mutations, preventing unauthorized exposure (REQ-065).
#[tokio::test]
async fn test_p38_public_relay_listener_blocks_admin_mutations() {
    let handler = create_test_handler(SecurityPolicy::public_relay());

    // 1. Public client calling read-only query succeeds
    let get_req = HttpRequestContext::new(remote_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "method": "getStatus",
                "id": 10
            })
            .to_string(),
        );

    let resp = handler.handle_request(get_req).await;
    assert_eq!(resp.status_code, 200);
    assert!(resp.body.unwrap().contains("\"result\""));

    // 2. Public client calling administrative mutation is REJECTED with -32001 (UNAUTHORIZED)
    let set_req = HttpRequestContext::new(remote_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "method": "setSettings",
                "params": { "maxRelays": 99 },
                "id": 11
            })
            .to_string(),
        );

    let resp = handler.handle_request(set_req).await;
    assert_eq!(resp.status_code, 200);
    let body = resp.body.unwrap();
    assert!(body.contains("\"error\""));
    assert!(body.contains("-32001"), "Must return UNAUTHORIZED (-32001) error code");
    assert!(body.contains("requires administrative authorization"));
}

/// Acceptance criterion 3:
/// Deliberately enabled remote administrative exposure requires valid authentication token (REQ-065).
#[tokio::test]
async fn test_p38_explicit_authenticated_remote_admin() {
    let mut policy = SecurityPolicy::public_relay();
    policy.enable_authenticated_remote_admin("secret_admin_token_987");
    let handler = create_test_handler(policy);

    // 1. Remote client with NO token is rejected for mutation
    let unauth_req = HttpRequestContext::new(remote_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "method": "setSettings",
                "params": { "maxRelays": 50 },
                "id": 20
            })
            .to_string(),
        );
    let resp = handler.handle_request(unauth_req).await;
    assert!(resp.body.unwrap().contains("-32001"));

    // 2. Remote client with WRONG token is rejected
    let wrong_req = HttpRequestContext::new(remote_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_header("X-PeerCast-Auth-Token", "wrong_token")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "method": "setSettings",
                "params": { "maxRelays": 50 },
                "id": 21
            })
            .to_string(),
        );
    let resp = handler.handle_request(wrong_req).await;
    assert!(resp.body.unwrap().contains("-32001"));

    // 3. Remote client with VALID token via header succeeds
    let auth_header_req = HttpRequestContext::new(remote_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_header("X-PeerCast-Auth-Token", "secret_admin_token_987")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "method": "setSettings",
                "params": { "maxRelays": 50 },
                "id": 22
            })
            .to_string(),
        );
    let resp = handler.handle_request(auth_header_req).await;
    assert_eq!(resp.status_code, 200);
    assert!(resp.body.unwrap().contains("\"result\":true"));

    // 4. Remote client with VALID token via query parameter succeeds (Station URL token parity)
    let auth_query_req = HttpRequestContext::new(remote_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_query_token("secret_admin_token_987")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "method": "setSettings",
                "params": { "maxRelays": 60 },
                "id": 23
            })
            .to_string(),
        );
    let resp = handler.handle_request(auth_query_req).await;
    assert_eq!(resp.status_code, 200);
    assert!(resp.body.unwrap().contains("\"result\":true"));
}

/// Acceptance criterion 4:
/// CSRF mitigations enforce X-Requested-With and reject unauthorized cross-origin requests (REQ-065).
#[tokio::test]
async fn test_p38_csrf_and_origin_protection() {
    let handler = create_test_handler(SecurityPolicy::loopback());

    // 1. Missing X-Requested-With header on POST -> HTTP 400 Bad Request
    let no_csrf_req = HttpRequestContext::new(loopback_client(), "POST", "/api/1").with_body(
        json!({
            "jsonrpc": "2.0",
            "method": "getStatus",
            "id": 30
        })
        .to_string(),
    );
    let resp = handler.handle_request(no_csrf_req).await;
    assert_eq!(resp.status_code, 400);
    assert!(resp.body.unwrap().contains("X-Requested-With"));

    // 2. Disallowed external origin -> HTTP 403 Forbidden
    let evil_origin_req = HttpRequestContext::new(loopback_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_header("Origin", "http://evil-tracker-attack.com")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "method": "getStatus",
                "id": 31
            })
            .to_string(),
        );
    let resp = handler.handle_request(evil_origin_req).await;
    assert_eq!(resp.status_code, 403);
    assert!(resp.body.unwrap().contains("Disallowed origin"));

    // 3. Allowed localhost origin -> HTTP 200 OK
    let good_origin_req = HttpRequestContext::new(loopback_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_header("Origin", "http://localhost:7144")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "method": "getStatus",
                "id": 32
            })
            .to_string(),
        );
    let resp = handler.handle_request(good_origin_req).await;
    assert_eq!(resp.status_code, 200);
}

/// Acceptance criterion 5:
/// Request framing enforcement: Content-Length required, payload size capped at 64 KiB (REQ-064).
#[tokio::test]
async fn test_p38_request_limits_and_framing() {
    let handler = create_test_handler(SecurityPolicy::loopback());

    // 1. Missing Content-Length -> HTTP 411 Length Required
    let mut no_len_req = HttpRequestContext::new(loopback_client(), "POST", "/api/1");
    no_len_req.body = b"{}".to_vec();
    // Headers do not contain content-length
    let resp = handler.handle_request(no_len_req).await;
    assert_eq!(resp.status_code, 411);

    // 2. Oversized payload exceeding 64 KiB -> HTTP 413 Payload Too Large
    let big_body = vec![b'a'; 70 * 1024];
    let big_req = HttpRequestContext::new(loopback_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_body(big_body);
    let resp = handler.handle_request(big_req).await;
    assert_eq!(resp.status_code, 413);

    // 3. Non-UTF8 payload -> JSON-RPC Parse Error (-32700)
    let bad_utf8_req = HttpRequestContext::new(loopback_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_body(vec![0xFF, 0xFE, 0xFD]);
    let resp = handler.handle_request(bad_utf8_req).await;
    assert_eq!(resp.status_code, 200);
    assert!(resp.body.unwrap().contains("-32700"));
}

/// Acceptance criterion 6:
/// JSON-RPC notifications (omitting id) return HTTP 204 No Content (REQ-064, Station parity).
#[tokio::test]
async fn test_p38_notification_returns_204_no_content() {
    let handler = create_test_handler(SecurityPolicy::loopback());

    // Notification request (no "id" field)
    let notify_req = HttpRequestContext::new(loopback_client(), "POST", "/api/1")
        .with_header("X-Requested-With", "XMLHttpRequest")
        .with_body(
            json!({
                "jsonrpc": "2.0",
                "method": "getStatus"
            })
            .to_string(),
        );

    let resp = handler.handle_request(notify_req).await;
    assert_eq!(resp.status_code, 204);
    assert_eq!(resp.body, None);
}

/// Acceptance criterion 7:
/// GET /api/1 returns version info payload (Station APIHost.cs:1478 parity).
#[tokio::test]
async fn test_p38_get_version_info_endpoint() {
    let handler = create_test_handler(SecurityPolicy::loopback());

    let get_req = HttpRequestContext::new(loopback_client(), "GET", "/api/1");
    let resp = handler.handle_request(get_req).await;
    assert_eq!(resp.status_code, 200);
    let body = resp.body.unwrap();
    assert!(body.contains("agentName"));
    assert!(body.contains("apiVersion"));
    assert!(body.contains("2.0"));
}

/// Acceptance criterion 8:
/// Secrets (auth tokens, bearer keys, URL query credentials) are reliably redacted from log messages (REQ-065).
#[test]
fn test_p38_secret_redaction() {
    let secret_token = "SuperSecretAuthKey12345";

    // 1. Exact token in message is redacted
    let log1 = format!("User authenticated with token: {}", secret_token);
    let clean1 = redact_secrets(&log1, &[secret_token]);
    assert!(!clean1.contains(secret_token));
    assert!(clean1.contains("[REDACTED]"));

    // 2. URL query string token (?token=...) is redacted
    let log2 = "GET /api/1?token=SuperSecretAuthKey12345&foo=bar HTTP/1.1";
    let clean2 = redact_secrets(log2, &[]);
    assert!(!clean2.contains(secret_token));
    assert_eq!(clean2, "GET /api/1?token=[REDACTED]&foo=bar HTTP/1.1");

    // 3. Authorization Bearer header is redacted
    let log3 = "Header: Authorization: Bearer SuperSecretAuthKey12345";
    let clean3 = redact_secrets(log3, &[]);
    assert!(!clean3.contains(secret_token));
    assert_eq!(clean3, "Header: Authorization: Bearer [REDACTED]");
}

/// Acceptance criterion 9:
/// SSRF protections block cloud metadata, broadcast, and multicast while permitting valid LAN relays (REQ-065).
#[test]
fn test_p38_ssrf_destination_validation() {
    // 1. Valid LAN IP addresses MUST be permitted (REQ-065: "SSRF protections must not indiscriminately block valid user-selected LAN relays")
    assert!(validate_relay_destination("192.168.1.50".parse().unwrap()).is_ok());
    assert!(validate_relay_destination("10.0.0.1".parse().unwrap()).is_ok());
    assert!(validate_relay_destination("172.16.20.1".parse().unwrap()).is_ok());

    // 2. Loopback and public Internet IPs are permitted
    assert!(validate_relay_destination("127.0.0.1".parse().unwrap()).is_ok());
    assert!(validate_relay_destination("::1".parse().unwrap()).is_ok());
    assert!(validate_relay_destination("93.184.216.34".parse().unwrap()).is_ok());

    // 3. Cloud instance metadata service (169.254.169.254) is STRICTLY BLOCKED
    let aws_meta: IpAddr = "169.254.169.254".parse().unwrap();
    let err = validate_relay_destination(aws_meta).unwrap_err();
    assert!(matches!(err, SecurityError::SsrfBlocked { .. }));

    // 4. IPv6 cloud metadata (fd00:ec2::254) is STRICTLY BLOCKED
    let aws_v6_meta: IpAddr = "fd00:ec2::254".parse().unwrap();
    assert!(validate_relay_destination(aws_v6_meta).is_err());

    // 5. Broadcast (255.255.255.255) is BLOCKED
    let bcast: IpAddr = "255.255.255.255".parse().unwrap();
    assert!(validate_relay_destination(bcast).is_err());

    // 6. Multicast (224.0.0.1 / ff02::1) is BLOCKED
    let mcast_v4: IpAddr = "224.0.0.1".parse().unwrap();
    let mcast_v6: IpAddr = "ff02::1".parse().unwrap();
    assert!(validate_relay_destination(mcast_v4).is_err());
    assert!(validate_relay_destination(mcast_v6).is_err());

    // 7. Unspecified (0.0.0.0 / ::) is BLOCKED
    let unspec_v4: IpAddr = "0.0.0.0".parse().unwrap();
    let unspec_v6: IpAddr = "::".parse().unwrap();
    assert!(validate_relay_destination(unspec_v4).is_err());
    assert!(validate_relay_destination(unspec_v6).is_err());
}
