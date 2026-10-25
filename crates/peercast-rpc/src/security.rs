use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use thiserror::Error;

use crate::protocol::{rpc_code, RpcErrorObject, RpcResponse};
use crate::server::{AccessLevel, RpcDispatcher, DEFAULT_MAX_RPC_REQUEST_BYTES};

/// Security and policy validation errors (REQ-064, REQ-065).
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum SecurityError {
    #[error("Missing Content-Length header (HTTP 411)")]
    LengthRequired,

    #[error("Request entity exceeds limit of {limit} bytes (HTTP 413)")]
    PayloadTooLarge { limit: usize, actual: usize },

    #[error("Missing or invalid X-Requested-With header (HTTP 400 CSRF protection)")]
    CsrfMissingHeader,

    #[error("Disallowed origin '{origin}' (HTTP 403 CSRF protection)")]
    CsrfInvalidOrigin { origin: String },

    #[error("Remote administrative access is disabled on this listener")]
    RemoteAdminDisabled,

    #[error("Unauthorized: invalid or missing authentication token")]
    Unauthorized,

    #[error("Destination '{destination}' blocked by SSRF protection policy: {reason}")]
    SsrfBlocked {
        destination: String,
        reason: &'static str,
    },
}

/// Endpoint listener binding classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerKind {
    /// Local loopback listener (127.0.0.1 or ::1) - administrative by default.
    Loopback,
    /// Public relay listener (0.0.0.0, [::], or public IP) - strictly Playback by default.
    PublicRelay,
}

/// Configurable security profile for an RPC listener endpoint (REQ-065).
#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    pub listener_kind: ListenerKind,
    pub allow_remote_admin: bool,
    pub require_csrf_header: bool,
    pub allowed_origins: Vec<String>,
    pub max_request_bytes: usize,
    pub auth_token: Option<String>,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            listener_kind: ListenerKind::Loopback,
            allow_remote_admin: false,
            require_csrf_header: true,
            allowed_origins: vec![
                "http://localhost".to_string(),
                "http://127.0.0.1".to_string(),
                "http://[::1]".to_string(),
            ],
            max_request_bytes: DEFAULT_MAX_RPC_REQUEST_BYTES,
            auth_token: None,
        }
    }
}

impl SecurityPolicy {
    /// Creates a default secure loopback policy.
    pub fn loopback() -> Self {
        Self::default()
    }

    /// Creates a public relay listener policy (strictly non-administrative by default).
    pub fn public_relay() -> Self {
        Self {
            listener_kind: ListenerKind::PublicRelay,
            allow_remote_admin: false,
            ..Default::default()
        }
    }

    /// Explicitly enables authenticated remote admin access with a mandatory secret token.
    pub fn enable_authenticated_remote_admin(&mut self, token: impl Into<String>) {
        self.allow_remote_admin = true;
        self.auth_token = Some(token.into());
    }
}

/// Incoming HTTP request context for the RPC security filter.
#[derive(Debug, Clone)]
pub struct HttpRequestContext {
    pub remote_addr: SocketAddr,
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub query_token: Option<String>,
    pub body: Vec<u8>,
}

impl HttpRequestContext {
    pub fn new(
        remote_addr: SocketAddr,
        method: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            remote_addr,
            method: method.into(),
            path: path.into(),
            headers: HashMap::new(),
            query_token: None,
            body: Vec::new(),
        }
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers
            .insert(key.into().to_ascii_lowercase(), value.into());
        self
    }

    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        let b = body.into();
        self.headers
            .insert("content-length".to_string(), b.len().to_string());
        self.body = b;
        self
    }

    pub fn with_query_token(mut self, token: impl Into<String>) -> Self {
        self.query_token = Some(token.into());
        self
    }

    pub fn get_header(&self, key: &str) -> Option<&str> {
        self.headers
            .get(&key.to_ascii_lowercase())
            .map(|s| s.as_str())
    }
}

/// HTTP response emitted by the security handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRpcResponse {
    pub status_code: u16,
    pub content_type: &'static str,
    pub body: Option<String>,
}

impl HttpRpcResponse {
    pub fn ok_json(body: String) -> Self {
        Self {
            status_code: 200,
            content_type: "application/json; charset=utf-8",
            body: Some(body),
        }
    }

    pub fn no_content() -> Self {
        Self {
            status_code: 204,
            content_type: "application/json; charset=utf-8",
            body: None,
        }
    }

    pub fn bad_request(msg: &str) -> Self {
        Self {
            status_code: 400,
            content_type: "text/plain; charset=utf-8",
            body: Some(msg.to_string()),
        }
    }

    pub fn forbidden(msg: &str) -> Self {
        Self {
            status_code: 403,
            content_type: "text/plain; charset=utf-8",
            body: Some(msg.to_string()),
        }
    }

    pub fn length_required() -> Self {
        Self {
            status_code: 411,
            content_type: "text/plain; charset=utf-8",
            body: Some("Content-Length header required".to_string()),
        }
    }

    pub fn payload_too_large() -> Self {
        Self {
            status_code: 413,
            content_type: "text/plain; charset=utf-8",
            body: Some("Request payload exceeds limit".to_string()),
        }
    }
}

/// Secure HTTP RPC handler coordinating request framing, CSRF checks, access control, and dispatch (REQ-064, REQ-065).
pub struct SecureRpcHandler {
    dispatcher: Arc<RpcDispatcher>,
    policy: SecurityPolicy,
}

impl SecureRpcHandler {
    pub fn new(dispatcher: Arc<RpcDispatcher>, policy: SecurityPolicy) -> Self {
        Self { dispatcher, policy }
    }

    /// Evaluates access level for an incoming request context.
    pub fn evaluate_access_level(
        &self,
        ctx: &HttpRequestContext,
    ) -> Result<AccessLevel, SecurityError> {
        let is_loopback = ctx.remote_addr.ip().is_loopback();

        // Extract credentials: from query param, X-PeerCast-Auth-Token header, or Authorization header
        let provided_token = ctx
            .query_token
            .as_deref()
            .or_else(|| ctx.get_header("x-peercast-auth-token"))
            .or_else(|| {
                ctx.get_header("authorization")
                    .and_then(|auth| auth.strip_prefix("Bearer ").map(|token| token.trim()))
            });

        // 1. Loopback listener
        if is_loopback && self.policy.listener_kind == ListenerKind::Loopback {
            // If an auth_token is configured on loopback, verify it; otherwise grant Admin
            if let Some(required) = &self.policy.auth_token {
                if provided_token == Some(required.as_str()) {
                    return Ok(AccessLevel::Admin);
                } else {
                    return Ok(AccessLevel::Playback);
                }
            }
            return Ok(AccessLevel::Admin);
        }

        // 2. Remote / Public client
        if !self.policy.allow_remote_admin {
            // Strictly clamp public clients to Playback (no Admin access allowed)
            return Ok(AccessLevel::Playback);
        }

        // 3. Explicitly allowed remote admin
        if let Some(required) = &self.policy.auth_token {
            if provided_token == Some(required.as_str()) {
                Ok(AccessLevel::Admin)
            } else {
                // Wrong or missing token on remote client requesting access
                Ok(AccessLevel::Playback)
            }
        } else {
            // Remote admin allowed without token is prohibited for security
            Ok(AccessLevel::Playback)
        }
    }

    /// Validates framing, CSRF, and origin constraints.
    pub fn validate_request_security(&self, ctx: &HttpRequestContext) -> Result<(), SecurityError> {
        // Enforce Content-Length on POST
        if ctx.method.eq_ignore_ascii_case("POST") {
            let cl_str = ctx
                .get_header("content-length")
                .ok_or(SecurityError::LengthRequired)?;
            let cl: usize = cl_str.parse().map_err(|_| SecurityError::LengthRequired)?;

            if cl > self.policy.max_request_bytes || ctx.body.len() > self.policy.max_request_bytes
            {
                return Err(SecurityError::PayloadTooLarge {
                    limit: self.policy.max_request_bytes,
                    actual: cl.max(ctx.body.len()),
                });
            }

            // CSRF protection: X-Requested-With header check (Station APIHost.cs:1440)
            if self.policy.require_csrf_header && ctx.get_header("x-requested-with").is_none() {
                return Err(SecurityError::CsrfMissingHeader);
            }

            // Origin / Referer check
            if let Some(origin) = ctx.get_header("origin") {
                let matches_allowed = self
                    .policy
                    .allowed_origins
                    .iter()
                    .any(|allowed| origin.starts_with(allowed));
                if !matches_allowed {
                    return Err(SecurityError::CsrfInvalidOrigin {
                        origin: origin.to_string(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Dispatches an incoming HTTP request through the security barrier and RPC engine.
    pub async fn handle_request(&self, ctx: HttpRequestContext) -> HttpRpcResponse {
        // 1. Validate security constraints
        if let Err(err) = self.validate_request_security(&ctx) {
            return match err {
                SecurityError::LengthRequired => HttpRpcResponse::length_required(),
                SecurityError::PayloadTooLarge { .. } => HttpRpcResponse::payload_too_large(),
                SecurityError::CsrfMissingHeader => HttpRpcResponse::bad_request(
                    "X-Requested-With header is required for CSRF protection",
                ),
                SecurityError::CsrfInvalidOrigin { origin } => {
                    HttpRpcResponse::forbidden(&format!("Disallowed origin '{}'", origin))
                }
                SecurityError::Unauthorized => HttpRpcResponse::forbidden("Unauthorized"),
                _ => HttpRpcResponse::bad_request(&err.to_string()),
            };
        }

        // 2. Evaluate access level
        let access = match self.evaluate_access_level(&ctx) {
            Ok(acc) => acc,
            Err(_) => AccessLevel::Playback,
        };

        // 3. Process method
        if ctx.method.eq_ignore_ascii_case("POST") {
            let body_str = match std::str::from_utf8(&ctx.body) {
                Ok(s) => s,
                Err(_) => {
                    let err_resp = RpcResponse::error(
                        None,
                        RpcErrorObject::new(rpc_code::PARSE_ERROR, "Invalid UTF-8 request payload"),
                    );
                    return HttpRpcResponse::ok_json(serde_json::to_string(&err_resp).unwrap());
                }
            };

            let response_opt = self.dispatcher.handle_request_str(body_str, access).await;
            match response_opt {
                Some(resp_json) => HttpRpcResponse::ok_json(resp_json),
                None => HttpRpcResponse::no_content(), // Notification -> HTTP 204
            }
        } else if ctx.method.eq_ignore_ascii_case("GET") {
            // GET /api/1 returns getVersionInfo (Station APIHost.cs:1478)
            let ver = self.dispatcher.state().read().await.agent_name.clone();
            let api_ver = self.dispatcher.state().read().await.api_version.clone();
            let val = serde_json::json!({
                "agentName": ver,
                "apiVersion": api_ver,
                "jsonrpc": "2.0"
            });
            HttpRpcResponse::ok_json(serde_json::to_string(&val).unwrap())
        } else {
            HttpRpcResponse::bad_request("Method not allowed")
        }
    }
}

/// Redacts sensitive secrets (tokens, credentials) from strings before writing to logs (REQ-065).
pub fn redact_secrets(input: &str, secrets: &[&str]) -> String {
    let mut result = input.to_string();

    // Redact explicit secret strings
    for secret in secrets {
        if !secret.is_empty() && secret.len() >= 4 {
            result = result.replace(secret, "[REDACTED]");
        }
    }

    // Redact token parameters in URLs: ?token=... or &token=...
    for prefix in ["?token=", "&token=", "token="] {
        let mut search_from = 0;
        while let Some(relative_idx) = result[search_from..].find(prefix) {
            let start_idx = search_from + relative_idx;
            let val_start = start_idx + prefix.len();
            if result[val_start..].starts_with("[REDACTED]") {
                search_from = val_start + "[REDACTED]".len();
                continue;
            }
            let end_idx = result[val_start..]
                .find(['&', ' ', '"', '\''])
                .map(|i| val_start + i)
                .unwrap_or(result.len());

            if val_start < end_idx {
                result.replace_range(val_start..end_idx, "[REDACTED]");
                search_from = val_start + "[REDACTED]".len();
            } else {
                search_from = val_start;
            }
        }
    }

    // Redact Bearer authorization headers
    let mut search_from = 0;
    while let Some(relative_idx) = result[search_from..].find("Bearer ") {
        let start_idx = search_from + relative_idx;
        let val_start = start_idx + 7;
        if result[val_start..].starts_with("[REDACTED]") {
            search_from = val_start + "[REDACTED]".len();
            continue;
        }
        let end_idx = result[val_start..]
            .find([' ', '"', '\'', '\r', '\n'])
            .map(|i| val_start + i)
            .unwrap_or(result.len());
        if val_start < end_idx {
            result.replace_range(val_start..end_idx, "[REDACTED]");
            search_from = val_start + "[REDACTED]".len();
        } else {
            search_from = val_start;
        }
    }

    result
}

/// Validates outbound relay destination IP addresses against SSRF attacks (REQ-065).
///
/// Rules:
/// - LAN private IP addresses (192.168.0.0/16, 10.0.0.0/8, 172.16.0.0/12) are explicitly PERMITTED
///   for user-configured relays per REQ-065.
/// - Loopback (127.0.0.1, ::1) is PERMITTED for local relay chaining.
/// - Public internet addresses are PERMITTED.
/// - Cloud instance metadata endpoints (169.254.169.254, fd00:ec2::254) are STRICTLY BLOCKED.
/// - Multicast, broadcast, and unspecified addresses are STRICTLY BLOCKED.
pub fn validate_relay_destination(ip: IpAddr) -> Result<(), SecurityError> {
    match ip {
        IpAddr::V4(ipv4) => {
            // Block 169.254.0.0/16 (Link Local & Cloud Metadata)
            if ipv4.is_link_local() {
                return Err(SecurityError::SsrfBlocked {
                    destination: ipv4.to_string(),
                    reason: "Link-local and cloud metadata addresses are blocked",
                });
            }
            // Block broadcast
            if ipv4.is_broadcast() {
                return Err(SecurityError::SsrfBlocked {
                    destination: ipv4.to_string(),
                    reason: "Broadcast address is blocked",
                });
            }
            // Block multicast
            if ipv4.is_multicast() {
                return Err(SecurityError::SsrfBlocked {
                    destination: ipv4.to_string(),
                    reason: "Multicast address is blocked",
                });
            }
            // Block unspecified (0.0.0.0)
            if ipv4.is_unspecified() {
                return Err(SecurityError::SsrfBlocked {
                    destination: ipv4.to_string(),
                    reason: "Unspecified address is blocked",
                });
            }
            Ok(())
        }
        IpAddr::V6(ipv6) => {
            // Block IPv6 multicast
            if ipv6.is_multicast() {
                return Err(SecurityError::SsrfBlocked {
                    destination: ipv6.to_string(),
                    reason: "Multicast IPv6 address is blocked",
                });
            }
            // Block unspecified (::)
            if ipv6.is_unspecified() {
                return Err(SecurityError::SsrfBlocked {
                    destination: ipv6.to_string(),
                    reason: "Unspecified IPv6 address is blocked",
                });
            }
            // Block cloud metadata e.g. fd00:ec2::254
            let octets = ipv6.octets();
            if octets[0] == 0xfd && octets[1] == 0x00 && octets[2] == 0x0e && octets[3] == 0xc2 {
                return Err(SecurityError::SsrfBlocked {
                    destination: ipv6.to_string(),
                    reason: "AWS/cloud IPv6 metadata endpoint is blocked",
                });
            }
            Ok(())
        }
    }
}
