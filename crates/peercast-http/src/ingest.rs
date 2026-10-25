use bytes::{Buf, Bytes, BytesMut};
use std::net::SocketAddr;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;

/// Maximum allowed HTTP request line and headers budget (8 KiB) for ingest (REQ-027).
pub const MAX_INGEST_HEADER_SIZE: usize = 8192;

/// Maximum allowed individual chunk size (16 MiB) for chunked ingest (REQ-027, Global Rule 3).
pub const MAX_INGEST_CHUNK_SIZE: usize = 16 * 1024 * 1024;

/// Ingest errors.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum IngestError {
    #[error("Port binding failed: {0}")]
    BindError(String),

    #[error("HTTP request line/headers exceed budget ({actual} > {max} bytes)")]
    HeaderTooLarge { actual: usize, max: usize },

    #[error("HTTP request line/headers timed out")]
    HeaderTimedOut,

    #[error("Malformed HTTP request: {0}")]
    MalformedRequest(&'static str),

    #[error("Unsupported HTTP transfer encoding")]
    UnsupportedTransferEncoding,

    #[error("Method not allowed: {0} (only POST is supported for push ingest)")]
    MethodNotAllowed(String),

    #[error("Unauthorized ingest request: authentication token required or mismatched")]
    Unauthorized,

    #[error("Malformed chunked transfer encoding: {0}")]
    MalformedChunking(&'static str),

    #[error("Individual chunk size exceeds allowed budget ({actual} > {max} bytes)")]
    ChunkTooLarge { actual: usize, max: usize },

    #[error("Ingest connection closed by client")]
    ConnectionClosed,

    #[error("Ingest I/O error: {0}")]
    IoError(String),
}

/// Configuration for dedicated HTTP push ingest listener (REQ-061).
#[derive(Debug, Clone)]
pub struct HttpPushConfig {
    pub bind_addr: SocketAddr,
    pub auth_token: Option<String>,
    pub max_header_size: usize,
    pub max_chunk_size: usize,
    pub header_timeout: Duration,
}

impl HttpPushConfig {
    pub fn new(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            auth_token: None,
            max_header_size: MAX_INGEST_HEADER_SIZE,
            max_chunk_size: MAX_INGEST_CHUNK_SIZE,
            header_timeout: Duration::from_secs(5),
        }
    }

    pub fn with_auth(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    pub fn with_max_header_size(mut self, max_header_size: usize) -> Self {
        self.max_header_size = max_header_size;
        self
    }

    pub fn with_max_chunk_size(mut self, max_chunk_size: usize) -> Self {
        self.max_chunk_size = max_chunk_size;
        self
    }

    pub fn with_header_timeout(mut self, header_timeout: Duration) -> Self {
        self.header_timeout = header_timeout;
        self
    }
}

/// Metadata extracted during HTTP POST ingest handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestHandshake {
    pub path: String,
    pub is_chunked: bool,
    pub content_type: Option<String>,
    pub user_agent: Option<String>,
}

/// Dedicated HTTP Push Ingest Listener (REQ-061, REQ-065).
///
/// Station parity note:
/// Upstream PeerCastStation's `HTTPPushSourceStream` binds a dedicated listener to its configured
/// source URI and accepts HTTP POST. It does not provide a universal `/push/{channel_id}` endpoint.
pub struct HttpPushIngestListener {
    config: HttpPushConfig,
    listener: Option<TcpListener>,
}

impl HttpPushIngestListener {
    pub fn new(config: HttpPushConfig) -> Self {
        Self {
            config,
            listener: None,
        }
    }

    /// Binds to the configured address. Fails with `IngestError::BindError` if the port is in use.
    pub async fn bind(&mut self) -> Result<SocketAddr, IngestError> {
        let listener = TcpListener::bind(self.config.bind_addr)
            .await
            .map_err(|e| IngestError::BindError(e.to_string()))?;
        let local_addr = listener
            .local_addr()
            .map_err(|e| IngestError::BindError(e.to_string()))?;
        self.listener = Some(listener);
        Ok(local_addr)
    }

    /// Accepts an incoming ingest connection and performs the HTTP handshake.
    /// Returns the socket, handshake metadata, and any initial body bytes that were read with the headers.
    pub async fn accept_connection(
        &self,
    ) -> Result<(TcpStream, IngestHandshake, Bytes), IngestError> {
        let listener = self
            .listener
            .as_ref()
            .ok_or_else(|| IngestError::BindError("Listener not bound".to_string()))?;

        let (mut socket, _) = listener
            .accept()
            .await
            .map_err(|e| IngestError::IoError(e.to_string()))?;

        // Read headers up to \r\n\r\n within budget
        let max_header_size = self.config.max_header_size;
        let mut header_buf = Vec::with_capacity(max_header_size.min(1024));
        let mut temp = [0u8; 1024];
        let header_read = timeout(self.config.header_timeout, async {
            loop {
                if header_buf.len() >= max_header_size {
                    return Ok(None);
                }
                let read_size = (max_header_size - header_buf.len()).min(temp.len());
                let n = socket
                    .read(&mut temp[..read_size])
                    .await
                    .map_err(|e| IngestError::IoError(e.to_string()))?;
                if n == 0 {
                    return Err(IngestError::ConnectionClosed);
                }
                header_buf.extend_from_slice(&temp[..n]);

                if let Some(pos) = memchr::memmem::find(&header_buf, b"\r\n\r\n") {
                    if pos + 4 <= max_header_size {
                        return Ok(Some(pos));
                    }
                    return Ok(None);
                }
            }
        })
        .await;

        let header_end = match header_read {
            Err(_) => {
                let _ = timeout(
                    Duration::from_millis(100),
                    socket.write_all(b"HTTP/1.1 408 Request Timeout\r\nConnection: close\r\n\r\n"),
                )
                .await;
                return Err(IngestError::HeaderTimedOut);
            }
            Ok(Err(error)) => return Err(error),
            Ok(Ok(Some(pos))) => pos,
            Ok(Ok(None)) => {
                let _ = socket
                    .write_all(b"HTTP/1.1 431 Request Header Fields Too Large\r\nConnection: close\r\n\r\n")
                    .await;
                return Err(IngestError::HeaderTooLarge {
                    actual: header_buf.len(),
                    max: max_header_size,
                });
            }
        };

        let header_bytes = &header_buf[..header_end];
        let remaining_body_prefix = Bytes::copy_from_slice(&header_buf[header_end + 4..]);

        let handshake = match parse_ingest_handshake(
            header_bytes,
            self.config.auth_token.as_deref(),
        ) {
            Ok(h) => h,
            Err(err) => {
                match err {
                    IngestError::MethodNotAllowed(_) => {
                        let _ = socket
                            .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nAllow: POST\r\nConnection: close\r\n\r\n")
                            .await;
                    }
                    IngestError::Unauthorized => {
                        let _ = socket
                            .write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer realm=\"PeerCast Ingest\"\r\nConnection: close\r\n\r\n")
                            .await;
                    }
                    IngestError::UnsupportedTransferEncoding => {
                        let _ = socket
                            .write_all(b"HTTP/1.1 501 Not Implemented\r\nConnection: close\r\n\r\n")
                            .await;
                    }
                    _ => {
                        let _ = socket
                            .write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n")
                            .await;
                    }
                }
                return Err(err);
            }
        };

        // Respond with HTTP 200 OK / 100 Continue if needed
        let response = format!(
            "HTTP/1.1 200 OK\r\n{}Connection: close\r\n\r\n",
            crate::HTTP_SERVER_HEADER
        );
        let _ = socket.write_all(response.as_bytes()).await;

        Ok((socket, handshake, remaining_body_prefix))
    }
}

/// Parses the ingest handshake from raw header bytes, verifying POST method and authorization.
pub fn parse_ingest_handshake(
    header_bytes: &[u8],
    required_auth_token: Option<&str>,
) -> Result<IngestHandshake, IngestError> {
    let text = std::str::from_utf8(header_bytes)
        .map_err(|_| IngestError::MalformedRequest("Non-UTF8 headers"))?;

    let mut lines = text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or(IngestError::MalformedRequest("Empty request line"))?;

    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or(IngestError::MalformedRequest("Missing method"))?;
    let path = parts
        .next()
        .ok_or(IngestError::MalformedRequest("Missing path"))?;

    if method != "POST" {
        return Err(IngestError::MethodNotAllowed(method.to_string()));
    }

    let mut transfer_encoding = None;
    let mut has_content_length = false;
    let mut content_type = None;
    let mut user_agent = None;
    let mut authorization = None;

    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            match key.as_str() {
                "transfer-encoding" => {
                    if transfer_encoding.replace(val.to_string()).is_some() {
                        return Err(IngestError::MalformedRequest(
                            "Multiple Transfer-Encoding fields",
                        ));
                    }
                }
                "content-length" => has_content_length = true,
                "content-type" => content_type = Some(val.to_string()),
                "user-agent" => user_agent = Some(val.to_string()),
                "authorization" => authorization = Some(val.to_string()),
                _ => {}
            }
        }
    }

    let is_chunked = if let Some(encoding) = transfer_encoding {
        if has_content_length {
            return Err(IngestError::MalformedRequest(
                "Content-Length with Transfer-Encoding",
            ));
        }
        if encoding.is_empty() || encoding.split(',').any(|coding| coding.trim().is_empty()) {
            return Err(IngestError::MalformedRequest("Empty Transfer-Encoding"));
        }
        let codings = encoding.split(',').map(str::trim).collect::<Vec<_>>();
        if codings.len() > 1
            && codings
                .iter()
                .all(|coding| coding.eq_ignore_ascii_case("chunked"))
        {
            return Err(IngestError::MalformedRequest("Duplicate chunked encoding"));
        }
        if codings.len() != 1 || !codings[0].eq_ignore_ascii_case("chunked") {
            return Err(IngestError::UnsupportedTransferEncoding);
        }
        true
    } else {
        false
    };

    // Authorization verification (REQ-065)
    if let Some(expected_token) = required_auth_token {
        let authorized = match authorization {
            Some(auth_header) => {
                if let Some(token) = auth_header.strip_prefix("Bearer ") {
                    token.trim() == expected_token
                } else {
                    false
                }
            }
            None => {
                // Check query param ?auth=...
                if let Some((_, query)) = path.split_once('?') {
                    query.split('&').any(|pair| {
                        if let Some((k, v)) = pair.split_once('=') {
                            k == "auth" && v == expected_token
                        } else {
                            false
                        }
                    })
                } else {
                    false
                }
            }
        };

        if !authorized {
            return Err(IngestError::Unauthorized);
        }
    }

    Ok(IngestHandshake {
        path: path.to_string(),
        is_chunked,
        content_type,
        user_agent,
    })
}

/// Incrementally reads and de-chunks an HTTP chunked payload stream from a reader (REQ-060, REQ-061).
pub async fn read_chunked_stream<R: AsyncRead + Unpin>(
    reader: &mut R,
    prefix_bytes: Bytes,
    data_sender: mpsc::Sender<Bytes>,
    max_chunk_size: usize,
) -> Result<(), IngestError> {
    let mut buffer = BytesMut::with_capacity(8192);
    buffer.extend_from_slice(&prefix_bytes);

    let mut temp = [0u8; 8192];

    loop {
        // Read chunk size line: {hex}\r\n
        let line_end =
            read_chunk_line(reader, &mut buffer, &mut temp, MAX_INGEST_HEADER_SIZE).await?;
        let hex_str = std::str::from_utf8(&buffer[..line_end - 2])
            .map_err(|_| IngestError::MalformedChunking("Non-UTF8 chunk size hex"))?;
        let clean_hex = hex_str.split(';').next().unwrap_or("");
        if clean_hex.is_empty() || !clean_hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(IngestError::MalformedChunking("Invalid chunk size hex"));
        }
        let chunk_len = usize::from_str_radix(clean_hex, 16)
            .map_err(|_| IngestError::MalformedChunking("Invalid chunk size hex"))?;

        buffer.advance(line_end);

        if chunk_len > max_chunk_size {
            return Err(IngestError::ChunkTooLarge {
                actual: chunk_len,
                max: max_chunk_size,
            });
        }

        // Chunk length 0 indicates end of chunked stream (0\r\n\r\n)
        if chunk_len == 0 {
            let mut trailer_bytes = 0usize;
            loop {
                let remaining = MAX_INGEST_HEADER_SIZE.saturating_sub(trailer_bytes);
                let trailer_end =
                    read_chunk_line(reader, &mut buffer, &mut temp, remaining).await?;
                trailer_bytes += trailer_end;
                if trailer_end == 2 {
                    return Ok(());
                }

                let line = &buffer[..trailer_end - 2];
                let Some(colon) = line.iter().position(|byte| *byte == b':') else {
                    return Err(IngestError::MalformedChunking("Invalid trailer field"));
                };
                if colon == 0 || !line[..colon].iter().copied().all(is_http_token_byte) {
                    return Err(IngestError::MalformedChunking("Invalid trailer field name"));
                }
                if !line[colon + 1..]
                    .iter()
                    .copied()
                    .all(is_http_field_value_byte)
                {
                    return Err(IngestError::MalformedChunking(
                        "Invalid trailer field value",
                    ));
                }
                buffer.advance(trailer_end);
            }
        }

        // Read chunk data + trailing \r\n
        let needed_len = chunk_len
            .checked_add(2)
            .ok_or(IngestError::MalformedChunking("Chunk length overflow"))?;
        while buffer.len() < needed_len {
            let n = match reader.read(&mut temp).await {
                Ok(0) => return Err(IngestError::ConnectionClosed),
                Ok(n) => n,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::UnexpectedEof
                    ) =>
                {
                    return Err(IngestError::ConnectionClosed);
                }
                Err(e) => return Err(IngestError::IoError(e.to_string())),
            };
            buffer.extend_from_slice(&temp[..n]);
        }

        if &buffer[chunk_len..needed_len] != b"\r\n" {
            return Err(IngestError::MalformedChunking(
                "Missing chunk data terminator",
            ));
        }
        let chunk_data = buffer.split_to(chunk_len).freeze();
        buffer.advance(2);

        if data_sender.send(chunk_data).await.is_err() {
            // Receiver dropped; terminate ingest
            return Ok(());
        }
    }
}

async fn read_chunk_line<R: AsyncRead + Unpin>(
    reader: &mut R,
    buffer: &mut BytesMut,
    temp: &mut [u8],
    max_line_bytes: usize,
) -> Result<usize, IngestError> {
    loop {
        if let Some(pos) = find_chunk_line_end(buffer) {
            let line_end = pos + 2;
            if line_end > max_line_bytes {
                return Err(IngestError::MalformedChunking(
                    "Chunk line or trailers too large",
                ));
            }
            return Ok(line_end);
        }
        let remaining = max_line_bytes.saturating_sub(buffer.len());
        if remaining == 0 {
            return Err(IngestError::MalformedChunking(
                "Chunk line or trailers too large",
            ));
        }
        let read_len = remaining.min(temp.len());
        let n = match reader.read(&mut temp[..read_len]).await {
            Ok(0) => return Err(IngestError::ConnectionClosed),
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                return Err(IngestError::ConnectionClosed);
            }
            Err(e) => return Err(IngestError::IoError(e.to_string())),
        };
        buffer.extend_from_slice(&temp[..n]);
    }
}

#[inline]
fn find_chunk_line_end(buffer: &[u8]) -> Option<usize> {
    for end in memchr::memchr_iter(b'\n', buffer) {
        if end > 0 && buffer[end - 1] == b'\r' {
            return Some(end - 1);
        }
    }
    None
}

fn is_http_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn is_http_field_value_byte(byte: u8) -> bool {
    byte == b'\t' || (0x20..=0x7e).contains(&byte) || byte >= 0x80
}

#[cfg(test)]
mod delimiter_tests {
    use super::find_chunk_line_end;

    #[test]
    fn delimiter_search_matches_scalar_at_every_prefix_boundary() {
        for length in 0..80 {
            for position in 0..length {
                let mut bytes = vec![b'x'; length];
                bytes[position] = b'\r';
                if position + 1 < length {
                    bytes[position + 1] = b'\n';
                }
                assert_eq!(
                    find_chunk_line_end(&bytes),
                    bytes.windows(2).position(|pair| pair == b"\r\n")
                );
            }
        }
        assert_eq!(find_chunk_line_end(b""), None);
        for bytes in [b"\n\r\n".as_slice(), b"\rX\n\r\n", b"\r\n\r\n", b"\n\n\r"] {
            assert_eq!(
                find_chunk_line_end(bytes),
                bytes.windows(2).position(|pair| pair == b"\r\n")
            );
        }
    }
}
