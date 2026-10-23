use crate::router::{HttpMethod, HttpRequest, HttpVersion};
use bytes::{Bytes, BytesMut};
use peercast_core::cache::HeaderSnapshot;
use peercast_core::content::Content;
use peercast_format::safestart::{SafeStartConfig, SafeStartDecision, SafeStartMode, SafeStartSelector};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

/// Streaming errors.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum StreamerError {
    #[error("Channel not found: {0}")]
    ChannelNotFound(String),

    #[error("I/O error during HTTP streaming: {0}")]
    IoError(String),

    #[error("Client disconnected")]
    ClientDisconnected,

    #[error("Streaming timed out or stalled")]
    Stalled,
}

/// HTTP framing choice (REQ-060).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpFraming {
    /// Standard HTTP/1.1 chunked transfer encoding (`Transfer-Encoding: chunked`).
    Chunked,

    /// Raw continuous binary body (`Connection: close`).
    /// Mandatory for HTTP/1.0 and MMS/ASF streams.
    RawContinuous,
}

/// Determines the media MIME type from container/extension hints.
pub fn resolve_mime_type(container: &str, ext: Option<&str>) -> &'static str {
    if let Some(e) = ext {
        match e.to_ascii_lowercase().as_str() {
            "flv" => return "video/x-flv",
            "ts" | "m2ts" => return "video/mp2t",
            "mp3" => return "audio/mpeg",
            "ogg" | "oga" | "ogv" => return "application/ogg",
            "mkv" => return "video/x-matroska",
            "webm" => return "video/webm",
            "wmv" | "asf" => return "video/x-ms-asf",
            _ => {}
        }
    }

    match container.to_ascii_uppercase().as_str() {
        "FLV" => "video/x-flv",
        "TS" => "video/mp2t",
        "MP3" => "audio/mpeg",
        "OGG" => "application/ogg",
        "MKV" => "video/x-matroska",
        "WEBM" => "video/webm",
        "WMV" | "ASF" => "video/x-ms-asf",
        _ => "application/octet-stream",
    }
}

/// Determines whether HTTP/1.1 chunked encoding or raw continuous streaming must be used.
///
/// REQ-060: Stop/block when chunking is applied blindly to HTTP/1.0 or MMS streams!
pub fn resolve_framing(version: HttpVersion, mime_type: &str) -> HttpFraming {
    if version == HttpVersion::Http10 {
        return HttpFraming::RawContinuous;
    }

    if mime_type == "application/x-mms-framed" || mime_type == "video/x-ms-asf" {
        return HttpFraming::RawContinuous;
    }

    HttpFraming::Chunked
}

/// Formats standard HTTP 200 response headers based on chosen framing and MIME type.
pub fn build_http_200_headers(
    version: HttpVersion,
    mime_type: &str,
    framing: HttpFraming,
) -> Bytes {
    let mut out = String::new();
    let ver_str = match version {
        HttpVersion::Http11 => "HTTP/1.1",
        HttpVersion::Http10 => "HTTP/1.0",
    };

    out.push_str(&format!("{ver_str} 200 OK\r\n"));
    out.push_str("Server: PeerCastStation-Rust\r\n");
    out.push_str("Access-Control-Allow-Origin: *\r\n");
    out.push_str("Cache-Control: no-cache\r\n");
    out.push_str(&format!("Content-Type: {mime_type}\r\n"));

    match framing {
        HttpFraming::Chunked => {
            out.push_str("Transfer-Encoding: chunked\r\n");
        }
        HttpFraming::RawContinuous => {
            out.push_str("Connection: close\r\n");
        }
    }

    out.push_str("\r\n");
    Bytes::from(out)
}

/// Formats standard HTTP error response headers and body.
pub fn build_http_error(
    version: HttpVersion,
    status_code: u16,
    status_text: &str,
    body_text: &str,
) -> Bytes {
    let ver_str = match version {
        HttpVersion::Http11 => "HTTP/1.1",
        HttpVersion::Http10 => "HTTP/1.0",
    };

    let mut out = String::new();
    out.push_str(&format!("{ver_str} {status_code} {status_text}\r\n"));
    out.push_str("Server: PeerCastStation-Rust\r\n");
    out.push_str("Connection: close\r\n");
    out.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    out.push_str(&format!("Content-Length: {}\r\n", body_text.len()));
    out.push_str("\r\n");
    out.push_str(body_text);
    Bytes::from(out)
}

/// Encodes a data payload with HTTP/1.1 chunked transfer framing (`hex_len\r\ndata\r\n`).
pub fn encode_chunked(data: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(data.len() + 16);
    let hex_len = format!("{:x}\r\n", data.len());
    out.extend_from_slice(hex_len.as_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
    out.freeze()
}

/// Encodes the HTTP/1.1 chunked stream terminator (`0\r\n\r\n`).
pub fn encode_chunked_terminator() -> Bytes {
    Bytes::from_static(b"0\r\n\r\n")
}

/// Runs the HTTP streaming loop to completion for a single client connection (REQ-060).
pub async fn stream_media_to_client<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &HttpRequest,
    header_snapshot: &HeaderSnapshot,
    mut content_receiver: mpsc::Receiver<Content>,
    safe_start_mode: SafeStartMode,
) -> Result<(), StreamerError> {
    let ext = match &request.target {
        crate::router::RouteTarget::Stream { extension, .. } => extension.as_deref(),
        _ => None,
    };

    let mime_type = resolve_mime_type(&header_snapshot.container_type, ext);
    let framing = resolve_framing(request.version, mime_type);

    // 1. Send HTTP 200 Status & Headers
    let header_bytes = build_http_200_headers(request.version, mime_type, framing);
    writer
        .write_all(&header_bytes)
        .await
        .map_err(|e| StreamerError::IoError(e.to_string()))?;

    // If HEAD request, terminate cleanly without body
    if request.method == HttpMethod::Head {
        writer
            .flush()
            .await
            .map_err(|e| StreamerError::IoError(e.to_string()))?;
        return Ok(());
    }

    // 2. Send initial HeaderSnapshot (P23, REQ-051, REQ-052)
    if !header_snapshot.data.is_empty() {
        let wire_snapshot = match framing {
            HttpFraming::Chunked => encode_chunked(&header_snapshot.data),
            HttpFraming::RawContinuous => header_snapshot.data.clone(),
        };
        writer
            .write_all(&wire_snapshot)
            .await
            .map_err(|e| StreamerError::IoError(e.to_string()))?;
    }

    // 3. Initialize format-aware safe start selector (P24, REQ-050, REQ-055)
    let mut safe_selector = SafeStartSelector::new(SafeStartConfig {
        mode: safe_start_mode,
        max_wait: Duration::from_secs(3),
    });
    safe_selector.on_header(header_snapshot);

    // 4. Stream media chunks continuously
    while let Some(content) = content_receiver.recv().await {
        let decision = safe_selector.evaluate(&content, None);
        match decision {
            SafeStartDecision::Skip { .. } => {
                // Skip packet waiting for safe start boundary
                continue;
            }
            SafeStartDecision::EmitHeader(new_hdr) => {
                let wire_bytes = match framing {
                    HttpFraming::Chunked => encode_chunked(&new_hdr.data),
                    HttpFraming::RawContinuous => new_hdr.data,
                };
                if let Err(e) = writer.write_all(&wire_bytes).await {
                    return Err(StreamerError::IoError(e.to_string()));
                }
            }
            SafeStartDecision::Admit | SafeStartDecision::FallbackAdmit { .. } => {
                let wire_bytes = match framing {
                    HttpFraming::Chunked => encode_chunked(&content.data),
                    HttpFraming::RawContinuous => content.data,
                };
                if let Err(e) = writer.write_all(&wire_bytes).await {
                    return Err(StreamerError::IoError(e.to_string()));
                }
            }
        }

        // Flush writer to maintain low streaming latency
        if let Err(e) = writer.flush().await {
            return Err(StreamerError::IoError(e.to_string()));
        }
    }

    // 5. If chunked framing, send terminator
    if framing == HttpFraming::Chunked {
        let term = encode_chunked_terminator();
        let _ = writer.write_all(&term).await;
        let _ = writer.flush().await;
    }

    Ok(())
}
