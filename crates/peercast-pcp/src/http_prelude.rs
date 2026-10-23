use bytes::Bytes;
use std::fmt;
use thiserror::Error;

use peercast_core::channel_id::{ChannelId, ChannelIdError};

/// Resource boundaries for HTTP prelude headers (REQ-031, REQ-037).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcpHttpLimits {
    /// Maximum total bytes allowed for HTTP request/response headers (default: 8 KiB).
    pub max_header_bytes: usize,
    /// Maximum header lines allowed (default: 100).
    pub max_header_lines: usize,
}

impl Default for PcpHttpLimits {
    fn default() -> Self {
        Self {
            max_header_bytes: 8192,
            max_header_lines: 100,
        }
    }
}

/// HTTP status classification for PCP relay responses (REQ-032).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcpHttpStatus {
    /// 200 OK: proceed through helo/oleh exchange, then media/metadata stream.
    Ok200,
    /// 503 Service Unavailable: proceed through helo/oleh exchange + host candidates + quit.
    Unavailable503,
    /// 404 Not Found: channel off-air; MUST NOT enter Atom decoding state (REQ-032).
    NotFound404,
    /// Other unhandled HTTP status.
    Other(u16),
}

impl PcpHttpStatus {
    #[inline]
    pub fn is_atom_eligible(&self) -> bool {
        matches!(self, PcpHttpStatus::Ok200 | PcpHttpStatus::Unavailable503)
    }
}

impl fmt::Display for PcpHttpStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PcpHttpStatus::Ok200 => write!(f, "200 OK"),
            PcpHttpStatus::Unavailable503 => write!(f, "503 Service Unavailable"),
            PcpHttpStatus::NotFound404 => write!(f, "404 Not Found"),
            PcpHttpStatus::Other(code) => write!(f, "{}", code),
        }
    }
}

/// Errors produced during HTTP prelude parsing.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum PcpHttpError {
    #[error("HTTP header exceeds maximum allowed size ({actual} > {max} bytes)")]
    HeaderTooLarge { actual: usize, max: usize },

    #[error("HTTP header line count exceeded maximum limit {0}")]
    LineLimitExceeded(usize),

    #[error("Malformed HTTP request line: \"{0}\"")]
    MalformedRequestLine(String),

    #[error("Malformed HTTP status line: \"{0}\"")]
    MalformedStatusLine(String),

    #[error("Invalid ChannelId in request path: {0}")]
    InvalidChannelId(#[from] ChannelIdError),

    #[error("Invalid x-peercast-pos numeric value: \"{0}\"")]
    InvalidPos(String),

    #[error("Channel is off-air (404 Not Found); Atom decoding forbidden (REQ-032)")]
    NotFoundOffAir,

    #[error("HTTP handshake deadline / read timeout exceeded (REQ-037)")]
    Timeout,
}

/// Parsed HTTP relay client request (`GET /channel/{id} HTTP/1.0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcpRelayRequest {
    pub channel_id: ChannelId,
    pub http_version: String,
    pub pcp_version: Option<u32>,
    pub pos: Option<i64>,
    pub headers: Vec<(String, String)>,
}

/// Parsed HTTP relay server response (`HTTP/1.0 200 OK` or `503 Service Unavailable`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcpRelayResponse {
    pub status: PcpHttpStatus,
    pub raw_status_code: u16,
    pub pos: Option<i64>,
    pub headers: Vec<(String, String)>,
}

/// Finds the end of HTTP headers in `buf` (`\r\n\r\n` or `\n\n`).
/// Returns `Some(offset_to_end_of_delimiter)` or `None` if incomplete.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    for i in 0..buf.len() {
        if buf[i] == b'\n' {
            if i >= 1 && buf[i - 1] == b'\n' {
                return Some(i + 1);
            }
            if i >= 3 && &buf[i - 3..=i] == b"\r\n\r\n" {
                return Some(i + 1);
            }
        }
    }
    None
}

/// Parses an incoming HTTP PCP relay client request from a `Bytes` buffer.
///
/// If headers are complete:
/// - Splits off the header bytes.
/// - **Preserves any bytes read beyond CRLF CRLF in `buf` as the binary tail (REQ-031).**
/// - Returns `Ok(Some(PcpRelayRequest))`.
///
/// If headers are incomplete and within limits, returns `Ok(None)`.
pub fn parse_relay_request(
    buf: &mut Bytes,
    limits: &PcpHttpLimits,
) -> Result<Option<PcpRelayRequest>, PcpHttpError> {
    let header_len = match find_header_end(buf.as_ref()) {
        Some(len) => len,
        None => {
            if buf.len() > limits.max_header_bytes {
                return Err(PcpHttpError::HeaderTooLarge {
                    actual: buf.len(),
                    max: limits.max_header_bytes,
                });
            }
            return Ok(None);
        }
    };

    if header_len > limits.max_header_bytes {
        return Err(PcpHttpError::HeaderTooLarge {
            actual: header_len,
            max: limits.max_header_bytes,
        });
    }

    // Split off header bytes; buf retains the binary tail!
    let header_bytes = buf.split_to(header_len);
    let header_str = String::from_utf8_lossy(&header_bytes);
    let lines: Vec<&str> = header_str.lines().collect();

    if lines.len() > limits.max_header_lines {
        return Err(PcpHttpError::LineLimitExceeded(lines.len()));
    }
    if lines.is_empty() {
        return Err(PcpHttpError::MalformedRequestLine("Empty request".into()));
    }

    // Parse request line: "GET /channel/{id} HTTP/1.0"
    let req_line = lines[0].trim();
    let parts: Vec<&str> = req_line.split_whitespace().collect();
    if parts.len() < 3 || parts[0] != "GET" {
        return Err(PcpHttpError::MalformedRequestLine(req_line.into()));
    }

    let path = parts[1];
    let http_version = parts[2].to_string();

    let channel_hex = if let Some(hex) = path.strip_prefix("/channel/") {
        hex
    } else {
        return Err(PcpHttpError::MalformedRequestLine(req_line.into()));
    };

    // Strip optional query params if present (e.g. /channel/HEX?...)
    let channel_hex = channel_hex.split('?').next().unwrap_or(channel_hex);
    let channel_id = ChannelId::from_hex(channel_hex)?;

    // Parse headers
    let mut pcp_version = None;
    let mut pos = None;
    let mut headers = Vec::new();

    for line in &lines[1..] {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();

            if key == "x-peercast-pcp" {
                pcp_version = val.parse::<u32>().ok();
            } else if key == "x-peercast-pos" {
                match val.parse::<i64>() {
                    Ok(p) => pos = Some(p),
                    Err(_) => return Err(PcpHttpError::InvalidPos(val.into())),
                }
            }
            headers.push((k.trim().to_string(), val.to_string()));
        }
    }

    Ok(Some(PcpRelayRequest {
        channel_id,
        http_version,
        pcp_version,
        pos,
        headers,
    }))
}

/// Parses an incoming HTTP PCP relay server response from a `Bytes` buffer.
///
/// If status is 404, stops immediately with `Err(PcpHttpError::NotFoundOffAir)` (REQ-032).
/// If status is 200 or 503:
/// - Splits off the header bytes.
/// - **Preserves any bytes read beyond CRLF CRLF in `buf` as the binary tail for Atom decoding.**
/// - Returns `Ok(Some(PcpRelayResponse))`.
pub fn parse_relay_response(
    buf: &mut Bytes,
    limits: &PcpHttpLimits,
) -> Result<Option<PcpRelayResponse>, PcpHttpError> {
    let header_len = match find_header_end(buf.as_ref()) {
        Some(len) => len,
        None => {
            if buf.len() > limits.max_header_bytes {
                return Err(PcpHttpError::HeaderTooLarge {
                    actual: buf.len(),
                    max: limits.max_header_bytes,
                });
            }
            return Ok(None);
        }
    };

    if header_len > limits.max_header_bytes {
        return Err(PcpHttpError::HeaderTooLarge {
            actual: header_len,
            max: limits.max_header_bytes,
        });
    }

    // Split off header bytes; buf retains the binary tail!
    let header_bytes = buf.split_to(header_len);
    let header_str = String::from_utf8_lossy(&header_bytes);
    let lines: Vec<&str> = header_str.lines().collect();

    if lines.len() > limits.max_header_lines {
        return Err(PcpHttpError::LineLimitExceeded(lines.len()));
    }
    if lines.is_empty() {
        return Err(PcpHttpError::MalformedStatusLine("Empty status".into()));
    }

    // Parse status line: "HTTP/1.0 200 OK"
    let status_line = lines[0].trim();
    let parts: Vec<&str> = status_line.split_whitespace().collect();
    if parts.len() < 2 || !parts[0].starts_with("HTTP/") {
        return Err(PcpHttpError::MalformedStatusLine(status_line.into()));
    }

    let raw_status_code: u16 = parts[1]
        .parse()
        .map_err(|_| PcpHttpError::MalformedStatusLine(status_line.into()))?;

    let status = match raw_status_code {
        200 => PcpHttpStatus::Ok200,
        503 => PcpHttpStatus::Unavailable503,
        404 => return Err(PcpHttpError::NotFoundOffAir), // REQ-032: Stop, never enter Atom state!
        other => PcpHttpStatus::Other(other),
    };

    // Parse headers
    let mut pos = None;
    let mut headers = Vec::new();

    for line in &lines[1..] {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();

            if key == "x-peercast-pos" {
                match val.parse::<i64>() {
                    Ok(p) => pos = Some(p),
                    Err(_) => return Err(PcpHttpError::InvalidPos(val.into())),
                }
            }
            headers.push((k.trim().to_string(), val.to_string()));
        }
    }

    Ok(Some(PcpRelayResponse {
        status,
        raw_status_code,
        pos,
        headers,
    }))
}
