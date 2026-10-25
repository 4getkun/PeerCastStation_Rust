use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

use peercast_http::embedded_assets::{API_CLIENT_SCRIPT_NAME, API_CLIENT_SCRIPT_PATH};
use peercast_http::{
    build_asset_http_response, get_embedded_asset, EmbeddedAsset, HttpMethod, HttpRequest,
    RouteTarget,
};
use peercast_rpc::server::{AccessLevel, RpcDispatcher, RpcState};

use crate::config::AppConfig;
use crate::lifecycle::{ShutdownReason, TaskTracker};
use crate::logging::{BoundedLogBuffer, LogLevel};
use crate::metrics::AppMetrics;
use crate::runtime::AppRuntime;

const REQUEST_READ_CHUNK_BYTES: usize = 4096;
const REQUEST_REJECTION_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// Embedded HTTP and JSON-RPC management server (REQ-060, REQ-064, REQ-066).
pub struct HttpServer {
    bind_addr: SocketAddr,
    rpc_dispatcher: Arc<RpcDispatcher>,
    metrics: Arc<AppMetrics>,
    log_buffer: Arc<BoundedLogBuffer>,
    task_tracker: TaskTracker,
    max_request_buffer_bytes: usize,
    request_timeout: Duration,
    runtime: Arc<AppRuntime>,
}

impl HttpServer {
    pub fn new(
        config: &AppConfig,
        rpc_state: RpcState,
        metrics: Arc<AppMetrics>,
        log_buffer: Arc<BoundedLogBuffer>,
        task_tracker: TaskTracker,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let bind_addr = SocketAddr::new(config.server.bind_address.parse()?, config.server.port);
        let rpc_dispatcher = RpcDispatcher::new(rpc_state)
            .with_max_request_bytes(config.server.max_rpc_request_bytes)
            .with_logging(Arc::new(crate::logging_rpc::RuntimeLogs(Arc::clone(
                &log_buffer,
            ))));
        let runtime = AppRuntime::new(
            config,
            task_tracker.clone(),
            Arc::clone(&log_buffer),
            Arc::clone(rpc_dispatcher.state()),
        )?;
        let rpc_dispatcher =
            Arc::new(rpc_dispatcher.with_yellow_page_directory(runtime.yellow_page_directory()));

        Ok(Self {
            bind_addr,
            rpc_dispatcher,
            metrics,
            log_buffer,
            task_tracker,
            max_request_buffer_bytes: config.server.max_request_buffer_bytes,
            request_timeout: Duration::from_millis(config.server.request_timeout_ms),
            runtime,
        })
    }

    pub fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub fn runtime(&self) -> Arc<AppRuntime> {
        Arc::clone(&self.runtime)
    }

    /// Runs the HTTP server loop until a shutdown signal is received.
    pub async fn run(
        &self,
        shutdown_rx: broadcast::Receiver<ShutdownReason>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(self.bind_addr).await?;
        self.run_on_listener(listener, shutdown_rx).await
    }

    pub async fn run_on_listener(
        &self,
        listener: TcpListener,
        mut shutdown_rx: broadcast::Receiver<ShutdownReason>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.runtime.start().await?;
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
                            #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
                            if let Err(error) = stream.set_nodelay(true) {
                                self.log_buffer.log(LogLevel::Warn, "http", &format!("TCP_NODELAY failed: {error}"));
                                continue;
                            }
                            let dispatcher = Arc::clone(&self.rpc_dispatcher);
                            let metrics = Arc::clone(&self.metrics);
                            let conn_guard = metrics.track_connection();
                            let max_request_buffer_bytes = self.max_request_buffer_bytes;
                            let request_timeout = self.request_timeout;
                            let runtime = Arc::clone(&self.runtime);
                            let log = Arc::clone(&self.log_buffer);
                            let mut connection_shutdown = runtime.shutdown_signal();

                            self.task_tracker.spawn(async move {
                                let _guard = conn_guard;
                                tokio::select! {
                                    result = handle_connection(stream, peer_addr, dispatcher, max_request_buffer_bytes, request_timeout, runtime) => {
                                        if let Err(error) = result {
                                            log.log(LogLevel::Debug, "connection", &format!("peer={peer_addr} connection ended: {error}"));
                                        }
                                    },
                                    _ = connection_shutdown.changed() => {},
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
                    self.runtime.shutdown().await;
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
    max_request_buffer_bytes: usize,
    request_timeout: Duration,
    runtime: Arc<AppRuntime>,
) -> Result<(), Box<dyn std::error::Error>> {
    let read = match tokio::time::timeout(
        request_timeout,
        read_request(&mut stream, max_request_buffer_bytes),
    )
    .await
    {
        Ok(read) => read,
        Err(_) => Err(RequestReadError::TimedOut),
    };
    let request = match read {
        Ok(Some(request)) => request,
        Ok(None) => return Ok(()),
        Err(RequestReadError::TooLarge) => {
            reject_request(
                &mut stream,
                b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await?;
            return Ok(());
        }
        Err(RequestReadError::Malformed) => {
            reject_request(
                &mut stream,
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await?;
            return Ok(());
        }
        Err(RequestReadError::TimedOut) => {
            reject_request(
                &mut stream,
                b"HTTP/1.1 408 Request Timeout\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await?;
            return Ok(());
        }
        Err(RequestReadError::LengthRequired) => {
            reject_request(
                &mut stream,
                b"HTTP/1.1 411 Length Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await?;
            return Ok(());
        }
        Err(RequestReadError::Io(error)) => return Err(error.into()),
    };

    let raw_request = request.as_slice();
    if raw_request.starts_with(b"pcp\n") {
        runtime.serve_pong(&mut stream, raw_request).await?;
        return Ok(());
    }
    let is_relay = std::str::from_utf8(raw_request)
        .ok()
        .and_then(|text| text.lines().next())
        .and_then(|line| line.split_whitespace().nth(1))
        .is_some_and(|path| path.starts_with("/channel/"));
    if is_relay {
        runtime
            .serve_relay(&mut stream, peer_addr, raw_request)
            .await?;
        return Ok(());
    }
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
            if let Some(asset) = get_embedded_asset(path) {
                write_asset_response(
                    &mut stream,
                    &asset,
                    if_none_match.as_deref(),
                    req.method != HttpMethod::Head,
                )
                .await?;
            } else {
                let not_found = b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found";
                stream.write_all(not_found).await?;
            }
        }
        RouteTarget::RpcApi { subpath } => {
            if subpath == API_CLIENT_SCRIPT_NAME || req.raw_path.contains(API_CLIENT_SCRIPT_NAME) {
                if !matches!(req.method, HttpMethod::Get | HttpMethod::Head) {
                    stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
                    return Ok(());
                }
                if let Some(asset) = get_embedded_asset(API_CLIENT_SCRIPT_PATH) {
                    write_asset_response(
                        &mut stream,
                        &asset,
                        if_none_match.as_deref(),
                        req.method != HttpMethod::Head,
                    )
                    .await?;
                }
                return Ok(());
            }

            match req.method {
                HttpMethod::Get => {
                    // Station APIHost parity: GET /api/1 returns agent metadata
                    let version_json = dispatcher.version_info().await.to_string();
                    write_json_response(&mut stream, &version_json).await?;
                }
                HttpMethod::Post => {
                    // Extract POST body
                    let body_str =
                        if let Some(pos) = raw_request.windows(4).position(|w| w == b"\r\n\r\n") {
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
                        write_json_response(&mut stream, &json_payload).await?;
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
        RouteTarget::Stream { channel_id_hex, .. } => {
            runtime
                .serve_stream_from(&mut stream, &req, channel_id_hex, Some(peer_addr))
                .await?;
        }
        RouteTarget::Playlist {
            channel_id_hex,
            format,
            extension,
        } => {
            let host = req.host.as_deref().unwrap_or("localhost");
            if let Some(body) =
                runtime.playlist(channel_id_hex, *format, extension.as_deref(), host)
            {
                let head = format!("HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", format.mime_type(), body.len());
                stream.write_all(head.as_bytes()).await?;
                if req.method != HttpMethod::Head {
                    stream.write_all(&body).await?;
                }
            } else {
                stream.write_all(b"HTTP/1.1 404 Playlist Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
            }
        }
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum RequestReadError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("HTTP request exceeds the configured buffer limit")]
    TooLarge,
    #[error("Malformed or incomplete HTTP request")]
    Malformed,
    #[error("HTTP request read timed out")]
    TimedOut,
    #[error("POST requests require Content-Length")]
    LengthRequired,
}

async fn reject_request(stream: &mut TcpStream, response: &[u8]) -> std::io::Result<()> {
    stream.write_all(response).await?;
    stream.shutdown().await?;
    // Drain unread input briefly so Windows does not discard the response with a TCP reset.
    let _ = tokio::time::timeout(
        REQUEST_REJECTION_DRAIN_TIMEOUT,
        tokio::io::copy(stream, &mut tokio::io::sink()),
    )
    .await;
    Ok(())
}

/// Buffers one complete request, including a possibly fragmented Content-Length body.
async fn read_request<R: AsyncRead + Unpin>(
    stream: &mut R,
    limit: usize,
) -> Result<Option<Vec<u8>>, RequestReadError> {
    let mut request = Vec::with_capacity(limit.min(REQUEST_READ_CHUNK_BYTES));
    let mut chunk = [0u8; REQUEST_READ_CHUNK_BYTES];
    let mut expected_len = None;
    loop {
        if let Some(expected) = expected_len {
            if request.len() >= expected {
                request.truncate(expected);
                return Ok(Some(request));
            }
        }
        let remaining = limit.saturating_sub(request.len());
        if remaining == 0 {
            return Err(RequestReadError::TooLarge);
        }
        let to_read = remaining.min(chunk.len());
        let n = stream.read(&mut chunk[..to_read]).await?;
        if n == 0 {
            return if request.is_empty() {
                Ok(None)
            } else {
                Err(RequestReadError::Malformed)
            };
        }
        let previous_len = request.len();
        request.extend_from_slice(&chunk[..n]);
        if request.starts_with(b"pcp\n") {
            return Ok(Some(request));
        }
        if expected_len.is_none() {
            let search_start = previous_len.saturating_sub(3);
            if let Some(offset) = request[search_start..]
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
            {
                let header_end = search_start + offset;
                let headers = std::str::from_utf8(&request[..header_end])
                    .map_err(|_| RequestReadError::Malformed)?;
                let mut content_length = None;
                for line in headers.split("\r\n").skip(1) {
                    let (name, value) = line.split_once(':').ok_or(RequestReadError::Malformed)?;
                    if name.trim().eq_ignore_ascii_case("transfer-encoding") {
                        // The management endpoint accepts Content-Length framing only.
                        return Err(RequestReadError::Malformed);
                    }
                    if name.trim().eq_ignore_ascii_case("content-length") {
                        let value = value.trim();
                        if content_length.is_some()
                            || value.is_empty()
                            || !value.bytes().all(|b| b.is_ascii_digit())
                        {
                            return Err(RequestReadError::Malformed);
                        }
                        content_length = Some(
                            value
                                .parse::<usize>()
                                .map_err(|_| RequestReadError::TooLarge)?,
                        );
                    }
                }
                if headers.split_whitespace().next() == Some("POST") && content_length.is_none() {
                    return Err(RequestReadError::LengthRequired);
                }
                let total = (header_end + 4)
                    .checked_add(content_length.unwrap_or(0))
                    .ok_or(RequestReadError::TooLarge)?;
                if total > limit {
                    return Err(RequestReadError::TooLarge);
                }
                expected_len = Some(total);
            }
        }
    }
}

async fn write_asset_response(
    stream: &mut TcpStream,
    asset: &EmbeddedAsset,
    if_none_match: Option<&str>,
    include_body: bool,
) -> std::io::Result<()> {
    let (status, headers, body) = build_asset_http_response(asset, if_none_match);
    let status_line = if status == 304 {
        "304 Not Modified"
    } else {
        "200 OK"
    };
    let mut head = format!("HTTP/1.1 {}\r\n", status_line);
    for (name, value) in headers {
        head.push_str(&format!("{}: {}\r\n", name, value));
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    if let Some(body) = body.filter(|_| include_body) {
        stream.write_all(body).await?;
    }
    Ok(())
}

async fn write_json_response(stream: &mut TcpStream, payload: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        payload.len(), payload
    );
    stream.write_all(response.as_bytes()).await
}

fn extract_header(raw_http: &[u8], target_header_lower: &str) -> Option<String> {
    let text = std::str::from_utf8(raw_http).ok()?;
    let mut values = Vec::new();
    for line in text.lines().skip(1) {
        if line.is_empty() || line == "\r" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(target_header_lower) {
                values.push(v.trim());
            }
        }
    }
    (!values.is_empty()).then(|| values.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn exchange(request: &[u8]) -> Vec<u8> {
        exchange_with_config(request, &AppConfig::default()).await
    }

    async fn exchange_with_config(request: &[u8], config: &AppConfig) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let http = HttpServer::new(
            config,
            RpcState::default(),
            Arc::new(AppMetrics::new()),
            Arc::new(BoundedLogBuffer::new(config.logging.max_in_memory_entries)),
            crate::AppLifecycleManager::new().task_tracker().clone(),
        )
        .unwrap();
        let server = async {
            let (stream, peer) = listener.accept().await.unwrap();
            handle_connection(
                stream,
                peer,
                http.rpc_dispatcher,
                http.max_request_buffer_bytes,
                http.request_timeout,
                http.runtime,
            )
            .await
            .unwrap();
        };
        let client = async {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            stream.write_all(request).await.unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).await.unwrap();
            response
        };
        let (_, response) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(server, client)
        })
        .await
        .expect("HTTP exchange timed out");
        response
    }

    fn response_parts(response: &[u8]) -> (&str, &[u8]) {
        let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        (
            std::str::from_utf8(&response[..split]).unwrap(),
            &response[split + 4..],
        )
    }

    #[tokio::test]
    async fn embedded_asset_routes_preserve_headers_body_and_conditional_get() {
        for (path, asset_path) in [
            ("/", "index.html"),
            ("/api/1/peercaststation.js", "api/1/peercaststation.js"),
            ("/html/index.html", "index.html"),
            ("/css/tokens.css", "css/tokens.css"),
        ] {
            let asset = get_embedded_asset(asset_path).unwrap();
            let response =
                exchange(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
                    .await;
            let (head, body) = response_parts(&response);
            assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{path}: {head}");
            assert!(head.contains(&format!("\r\nContent-Type: {}\r\n", asset.mime_type)));
            assert!(head.contains(&format!("\r\nContent-Length: {}\r\n", body.len())));
            assert!(head.contains("\r\nX-Content-Type-Options: nosniff\r\n"));
            assert!(head.contains("\r\nX-Frame-Options: SAMEORIGIN\r\n"));
            assert!(head.contains("\r\nContent-Security-Policy: "));
            assert!(head.ends_with("\r\nConnection: close"));
            assert_eq!(body, asset.content);

            let cached = exchange(
                format!(
                    "GET {path} HTTP/1.1\r\nHost: localhost\r\nIf-None-Match: {}\r\n\r\n",
                    asset.etag
                )
                .as_bytes(),
            )
            .await;
            let (head, body) = response_parts(&cached);
            assert!(head.starts_with("HTTP/1.1 304 Not Modified\r\n"));
            assert!(head.contains(&format!("\r\nETag: {}\r\n", asset.etag)));
            assert!(head.contains(&format!("\r\nContent-Length: {}\r\n", asset.content.len())));
            assert!(body.is_empty());
        }
    }

    #[tokio::test]
    async fn api_preserves_version_rpc_and_notification_responses() {
        let response = exchange(b"GET /api/1 HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        let (head, body) = response_parts(&response);
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(head.contains("\r\nContent-Type: application/json; charset=utf-8\r\n"));
        assert!(head.contains(&format!("\r\nContent-Length: {}\r\n", body.len())));
        let version: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(
            version,
            serde_json::json!({
                "agentName": concat!("PeerCastStation/", env!("CARGO_PKG_VERSION")),
                "apiVersion": "1.0.0",
                "jsonrpc": "2.0",
            })
        );

        let payload = r#"{"jsonrpc":"2.0","method":"getVersionInfo","id":1}"#;
        let response = exchange(
            format!(
                "POST /api/1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{payload}",
                payload.len()
            )
            .as_bytes(),
        )
        .await;
        let (head, body) = response_parts(&response);
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(head.contains(&format!("\r\nContent-Length: {}\r\n", body.len())));
        let rpc: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(rpc["result"], version);
        assert_eq!(rpc["result"]["apiVersion"], "1.0.0");
        assert_eq!(rpc["id"], 1);

        let notification = r#"{"jsonrpc":"2.0","method":"getVersionInfo"}"#;
        let response = exchange(format!("POST /api/1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{notification}", notification.len()).as_bytes()).await;
        assert_eq!(
            response,
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
    }

    #[tokio::test]
    async fn configured_http_and_rpc_limits_reach_the_running_handler() {
        let request = format!(
            "GET /api/1 HTTP/1.1\r\nHost: localhost\r\nX-Padding: {}\r\n\r\n",
            "x".repeat(9000)
        );
        let response = exchange(request.as_bytes()).await;
        assert_eq!(
            response,
            b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );

        let mut config = AppConfig::default();
        config.server.max_request_buffer_bytes = 16_384;
        let response = exchange_with_config(request.as_bytes(), &config).await;
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));

        let payload = r#"{"jsonrpc":"2.0","method":"getVersionInfo","id":1}"#;
        let request = format!(
            "POST /api/1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{payload}",
            payload.len()
        );
        config.server.max_rpc_request_bytes = payload.len() - 1;
        let response = exchange_with_config(request.as_bytes(), &config).await;
        let (_, body) = response_parts(&response);
        let rejected: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(rejected["error"]["code"], -32600);

        config.server.max_rpc_request_bytes = payload.len();
        let response = exchange_with_config(request.as_bytes(), &config).await;
        let (_, body) = response_parts(&response);
        let accepted: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(accepted["result"]["apiVersion"], "1.0.0");

        let response = exchange(b"POST /api/1 HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        assert_eq!(
            response,
            b"HTTP/1.1 411 Length Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );

        config.server.request_timeout_ms = 10;
        let response = exchange_with_config(b"GET /api/1 HTTP/1.1\r\n", &config).await;
        assert_eq!(
            response,
            b"HTTP/1.1 408 Request Timeout\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
    }

    #[tokio::test]
    async fn fragmented_requests_are_complete_and_limits_include_headers_and_body() {
        let request = b"POST /api/1 HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello";
        let (mut writer, mut reader) = tokio::io::duplex(8);
        let send = async {
            for piece in request.chunks(3) {
                writer.write_all(piece).await.unwrap();
                tokio::task::yield_now().await;
            }
            writer.shutdown().await.unwrap();
        };
        let receive = read_request(&mut reader, request.len());
        let (_, received) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(send, receive)
        })
        .await
        .unwrap();
        assert_eq!(received.unwrap().unwrap(), request);

        assert!(matches!(
            read_request(&mut request.as_slice(), request.len() - 1).await,
            Err(RequestReadError::TooLarge)
        ));
        let incomplete = b"POST /api/1 HTTP/1.1\r\nContent-Length: 5\r\n\r\nhi";
        assert!(matches!(
            read_request(&mut incomplete.as_slice(), 8192).await,
            Err(RequestReadError::Malformed)
        ));
        for mut malformed in [
            b"POST /api/1 HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\n".as_slice(),
            b"POST /api/1 HTTP/1.1\r\nContent-Length: +1\r\n\r\nx".as_slice(),
            b"POST /api/1 HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n".as_slice(),
        ] {
            assert!(matches!(
                read_request(&mut malformed, 8192).await,
                Err(RequestReadError::Malformed)
            ));
        }
    }

    #[tokio::test]
    async fn conditional_head_and_repeated_etag_headers_have_no_body() {
        let asset = get_embedded_asset("/index.html").unwrap();
        let response = exchange(b"HEAD /index.html HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        let (head, body) = response_parts(&response);
        assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(head.contains(&format!("\r\nContent-Length: {}\r\n", asset.content.len())));
        assert!(body.is_empty());

        for method in ["GET", "HEAD"] {
            let response = exchange(format!("{method} /index.html HTTP/1.1\r\nHost: localhost\r\nIf-None-Match: \"stale\"\r\nIf-None-Match: W/{}\r\n\r\n", asset.etag).as_bytes()).await;
            let (head, body) = response_parts(&response);
            assert!(head.starts_with("HTTP/1.1 304 Not Modified\r\n"));
            assert!(body.is_empty());
        }
    }
}
