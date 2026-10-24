use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

use peercast_http::{
    build_asset_http_response, get_embedded_asset, HttpMethod, HttpRequest, RouteTarget,
};
use peercast_rpc::server::{AccessLevel, RpcDispatcher, RpcState};

use crate::config::AppConfig;
use crate::lifecycle::{ShutdownReason, TaskTracker};
use crate::logging::{BoundedLogBuffer, LogLevel};
use crate::metrics::AppMetrics;

/// Embedded HTTP and JSON-RPC management server (REQ-060, REQ-064, REQ-066).
pub struct HttpServer {
    bind_addr: SocketAddr,
    rpc_dispatcher: Arc<RpcDispatcher>,
    metrics: Arc<AppMetrics>,
    log_buffer: Arc<BoundedLogBuffer>,
    task_tracker: TaskTracker,
}

impl HttpServer {
    pub fn new(
        config: &AppConfig,
        rpc_state: RpcState,
        metrics: Arc<AppMetrics>,
        log_buffer: Arc<BoundedLogBuffer>,
        task_tracker: TaskTracker,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let addr_str = format!("{}:{}", config.server.bind_address, config.server.port);
        let bind_addr: SocketAddr = addr_str.parse()?;
        let rpc_dispatcher = Arc::new(RpcDispatcher::new(rpc_state));

        Ok(Self {
            bind_addr,
            rpc_dispatcher,
            metrics,
            log_buffer,
            task_tracker,
        })
    }

    pub fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    /// Runs the HTTP server loop until a shutdown signal is received.
    pub async fn run(
        &self,
        mut shutdown_rx: broadcast::Receiver<ShutdownReason>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(self.bind_addr).await?;
        self.log_buffer.log(
            LogLevel::Info,
            "http",
            &format!("HTTP/RPC server listening on http://{}/", self.bind_addr),
        );

        loop {
            tokio::select! {
                accept_res = listener.accept() => {
                    match accept_res {
                        Ok((stream, peer_addr)) => {
                            let dispatcher = Arc::clone(&self.rpc_dispatcher);
                            let metrics = Arc::clone(&self.metrics);
                            let log = Arc::clone(&self.log_buffer);
                            let conn_guard = metrics.track_connection();

                            self.task_tracker.spawn(async move {
                                let _guard = conn_guard;
                                if let Err(e) = handle_connection(stream, peer_addr, dispatcher, log).await {
                                    // Connection closed or client disconnected early
                                    let _ = e;
                                }
                            });
                        }
                        Err(err) => {
                            self.log_buffer.log(
                                LogLevel::Warn,
                                "http",
                                &format!("TCP accept error: {}", err),
                            );
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    self.log_buffer.log(
                        LogLevel::Info,
                        "http",
                        "Stopping HTTP listener for graceful drain...",
                    );
                    break;
                }
            }
        }

        Ok(())
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    dispatcher: Arc<RpcDispatcher>,
    log: Arc<BoundedLogBuffer>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }

    let raw_request = &buf[..n];
    let req = match HttpRequest::parse(raw_request) {
        Ok(r) => r,
        Err(_) => {
            let bad_req = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 15\r\nConnection: close\r\n\r\n400 Bad Request";
            stream.write_all(bad_req).await?;
            return Ok(());
        }
    };

    // Extract If-None-Match if present
    let if_none_match = extract_header(raw_request, "if-none-match");

    match &req.target {
        RouteTarget::StaticAsset { path } => {
            let clean_path = path.strip_prefix('/').unwrap_or(path.as_str());
            let lookup = if clean_path.is_empty() {
                "index.html"
            } else {
                clean_path
            };

            if let Some(asset) = get_embedded_asset(lookup) {
                let (status, headers, body) =
                    build_asset_http_response(&asset, if_none_match.as_deref());
                let status_line = match status {
                    200 => "200 OK",
                    304 => "304 Not Modified",
                    _ => "200 OK",
                };

                let mut resp_head = format!("HTTP/1.1 {}\r\n", status_line);
                for (k, v) in headers {
                    resp_head.push_str(&format!("{}: {}\r\n", k, v));
                }
                resp_head.push_str("Connection: close\r\n\r\n");

                stream.write_all(resp_head.as_bytes()).await?;
                if let Some(body_bytes) = body {
                    stream.write_all(body_bytes).await?;
                }
            } else {
                let not_found = b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found";
                stream.write_all(not_found).await?;
            }
        }
        RouteTarget::RpcApi { subpath } => {
            if subpath == "peercaststation.js" || req.raw_path.contains("peercaststation.js") {
                if let Some(asset) = get_embedded_asset("api/1/peercaststation.js") {
                    let (status, headers, body) =
                        build_asset_http_response(&asset, if_none_match.as_deref());
                    let mut resp_head = format!("HTTP/1.1 {}\r\n", if status == 304 { "304 Not Modified" } else { "200 OK" });
                    for (k, v) in headers {
                        resp_head.push_str(&format!("{}: {}\r\n", k, v));
                    }
                    resp_head.push_str("Connection: close\r\n\r\n");
                    stream.write_all(resp_head.as_bytes()).await?;
                    if let Some(body_bytes) = body {
                        stream.write_all(body_bytes).await?;
                    }
                }
                return Ok(());
            }

            match req.method {
                HttpMethod::Get => {
                    // Station APIHost parity: GET /api/1 returns agent metadata
                    let version_json = r#"{"agentName":"PeerCastStation (Rust Edition)","version":"0.1.0","apiVersion":1}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        version_json.len(),
                        version_json
                    );
                    stream.write_all(resp.as_bytes()).await?;
                }
                HttpMethod::Post => {
                    // Extract POST body
                    let body_str = if let Some(pos) = raw_request.windows(4).position(|w| w == b"\r\n\r\n") {
                        std::str::from_utf8(&raw_request[pos + 4..]).unwrap_or("{}")
                    } else {
                        "{}"
                    };

                    let access_level = if peer_addr.ip().is_loopback() {
                        AccessLevel::Admin
                    } else {
                        AccessLevel::Playback
                    };

                    let rpc_result = dispatcher.handle_request_str(body_str, access_level).await;
                    if let Some(json_payload) = rpc_result {
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            json_payload.len(),
                            json_payload
                        );
                        stream.write_all(resp.as_bytes()).await?;
                    } else {
                        // All notifications -> HTTP 204 No Content
                        let resp = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        stream.write_all(resp).await?;
                    }
                }
                _ => {
                    let method_not_allowed = b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    stream.write_all(method_not_allowed).await?;
                }
            }
        }
        RouteTarget::Stream { .. } => {
            let not_found = b"HTTP/1.1 404 Channel Not Found\r\nContent-Length: 17\r\nConnection: close\r\n\r\nChannel Not Found";
            stream.write_all(not_found).await?;
        }
        RouteTarget::Playlist { .. } => {
            let not_found = b"HTTP/1.1 404 Playlist Not Found\r\nContent-Length: 18\r\nConnection: close\r\n\r\nPlaylist Not Found";
            stream.write_all(not_found).await?;
        }
    }

    let _ = log;
    Ok(())
}

fn extract_header(raw_http: &[u8], target_header_lower: &str) -> Option<String> {
    let text = std::str::from_utf8(raw_http).ok()?;
    for line in text.lines().skip(1) {
        if line.is_empty() || line == "\r" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(target_header_lower) {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}
