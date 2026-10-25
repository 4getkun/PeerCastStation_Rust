use crate::router::{HttpMethod, HttpRequest, HttpVersion};
use bytes::{Bytes, BytesMut};
use peercast_core::cache::HeaderSnapshot;
use peercast_core::content::Content;
use peercast_format::safestart::{
    SafeStartConfig, SafeStartDecision, SafeStartMode, SafeStartSelector,
};
use std::future::Future;
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
    out.push_str(crate::HTTP_SERVER_HEADER);
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
    out.push_str(crate::HTTP_SERVER_HEADER);
    out.push_str("Connection: close\r\n");
    out.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    out.push_str(&format!("Content-Length: {}\r\n", body_text.len()));
    out.push_str("\r\n");
    out.push_str(body_text);
    Bytes::from(out)
}

/// Encodes a data payload with HTTP/1.1 chunked transfer framing (`hex_len\r\ndata\r\n`).
pub fn encode_chunked(data: &[u8]) -> Bytes {
    let mut prefix = [0; CHUNK_PREFIX_CAPACITY];
    let prefix = chunk_prefix(data.len(), &mut prefix);
    let mut out = BytesMut::with_capacity(data.len() + prefix.len() + 2);
    out.extend_from_slice(prefix);
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
    out.freeze()
}

const CHUNK_PREFIX_CAPACITY: usize = 2 * std::mem::size_of::<usize>() + 2;

#[inline]
fn chunk_prefix(mut length: usize, buffer: &mut [u8; CHUNK_PREFIX_CAPACITY]) -> &[u8] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut cursor = buffer.len() - 2;
    buffer[cursor..].copy_from_slice(b"\r\n");
    loop {
        cursor -= 1;
        buffer[cursor] = HEX[length & 15];
        length >>= 4;
        if length == 0 {
            return &buffer[cursor..];
        }
    }
}

/// Encodes the HTTP/1.1 chunked stream terminator (`0\r\n\r\n`).
pub fn encode_chunked_terminator() -> Bytes {
    Bytes::from_static(b"0\r\n\r\n")
}

/// Keep the shared payload intact; Windows TCP uses WSASend for these slices.
async fn write_payload<W: AsyncWrite + Unpin>(
    writer: &mut W,
    data: &[u8],
    framing: HttpFraming,
) -> std::io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if framing == HttpFraming::RawContinuous {
        return writer.write_all(data).await;
    }
    let mut prefix = [0; CHUNK_PREFIX_CAPACITY];
    let prefix = chunk_prefix(data.len(), &mut prefix);
    let mut buffers = [
        std::io::IoSlice::new(prefix),
        std::io::IoSlice::new(data),
        std::io::IoSlice::new(b"\r\n"),
    ];
    let mut remaining = &mut buffers[..];
    while !remaining.is_empty() {
        let count = writer.write_vectored(remaining).await?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "stream writer made no progress",
            ));
        }
        std::io::IoSlice::advance_slices(&mut remaining, count);
    }
    Ok(())
}

/// Runs the HTTP streaming loop to completion for a single client connection (REQ-060).
pub async fn stream_media_to_client<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &HttpRequest,
    header_snapshot: &HeaderSnapshot,
    content_receiver: mpsc::Receiver<Content>,
    safe_start_mode: SafeStartMode,
) -> Result<(), StreamerError> {
    stream_media_to_client_with_config(
        writer,
        request,
        header_snapshot,
        content_receiver,
        SafeStartConfig {
            mode: safe_start_mode,
            ..SafeStartConfig::default()
        },
    )
    .await
}

/// Runs the HTTP streaming loop with an explicit safe-start configuration.
pub async fn stream_media_to_client_with_config<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &HttpRequest,
    header_snapshot: &HeaderSnapshot,
    content_receiver: mpsc::Receiver<Content>,
    safe_start_config: SafeStartConfig,
) -> Result<(), StreamerError> {
    stream_media_to_client_with_write_timeout(
        writer,
        request,
        header_snapshot,
        content_receiver,
        safe_start_config,
        Duration::from_secs(5),
    )
    .await
}

async fn write_step(
    timeout: Duration,
    operation: impl Future<Output = std::io::Result<()>>,
) -> Result<(), StreamerError> {
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|_| StreamerError::Stalled)?
        .map_err(|error| StreamerError::IoError(error.to_string()))
}

/// Bounds each complete write/flush operation without limiting the stream's lifetime.
pub async fn stream_media_to_client_with_write_timeout<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &HttpRequest,
    header_snapshot: &HeaderSnapshot,
    mut content_receiver: mpsc::Receiver<Content>,
    safe_start_config: SafeStartConfig,
    write_timeout: Duration,
) -> Result<(), StreamerError> {
    let ext = match &request.target {
        crate::router::RouteTarget::Stream { extension, .. } => extension.as_deref(),
        _ => None,
    };

    let mime_type = resolve_mime_type(&header_snapshot.container_type, ext);
    let framing = resolve_framing(request.version, mime_type);

    // 1. Send HTTP 200 Status & Headers
    let header_bytes = build_http_200_headers(request.version, mime_type, framing);
    write_step(write_timeout, writer.write_all(&header_bytes)).await?;

    // If HEAD request, terminate cleanly without body
    if request.method == HttpMethod::Head {
        write_step(write_timeout, writer.flush()).await?;
        return Ok(());
    }

    // 2. Send initial HeaderSnapshot (P23, REQ-051, REQ-052)
    if !header_snapshot.data.is_empty() {
        write_step(
            write_timeout,
            write_payload(writer, &header_snapshot.data, framing),
        )
        .await?;
    }

    // 3. Initialize format-aware safe start selector (P24, REQ-050, REQ-055)
    let mut safe_selector = SafeStartSelector::new(safe_start_config);
    safe_selector.on_header(header_snapshot);

    // 4. Stream media chunks continuously
    while let Some(content) = content_receiver.recv().await {
        let decision = safe_selector.evaluate(&content, None);
        let payload = match decision {
            SafeStartDecision::Skip { .. } => continue,
            SafeStartDecision::EmitHeader(new_hdr) => new_hdr.data,
            SafeStartDecision::Admit | SafeStartDecision::FallbackAdmit { .. } => content.data,
        };
        write_step(write_timeout, async {
            write_payload(writer, &payload, framing).await?;
            writer.flush().await
        })
        .await?;
    }

    // 5. If chunked framing, send terminator
    if framing == HttpFraming::Chunked {
        let term = encode_chunked_terminator();
        write_step(write_timeout, async {
            writer.write_all(&term).await?;
            writer.flush().await
        })
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod write_tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn stack_chunk_prefix_matches_wire_format_at_boundaries() {
        for length in [0, 1, 15, 16, 255, 256, 4095, 4096, 1024 * 1024, usize::MAX] {
            let mut prefix = [0; CHUNK_PREFIX_CAPACITY];
            assert_eq!(
                chunk_prefix(length, &mut prefix),
                format!("{length:x}\r\n").as_bytes()
            );
        }
        assert_eq!(encode_chunked(b""), Bytes::from_static(b"0\r\n\r\n"));
    }

    struct StallingWriter {
        allowed_writes: usize,
        stall_flush: bool,
    }

    impl AsyncWrite for StallingWriter {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buffer: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            if self.allowed_writes == 0 {
                std::task::Poll::Pending
            } else {
                self.allowed_writes -= 1;
                std::task::Poll::Ready(Ok(buffer.len()))
            }
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.stall_flush {
                std::task::Poll::Pending
            } else {
                std::task::Poll::Ready(Ok(()))
            }
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn stalled_headers_payload_flush_and_terminator_have_deadlines() {
        for (phase, method, allowed_writes, stall_flush, initial_header, body) in [
            ("response headers", "GET", 0, false, false, false),
            ("initial header", "GET", 1, false, true, false),
            ("media payload", "GET", 1, false, false, true),
            ("media flush", "GET", usize::MAX, true, false, true),
            ("HEAD flush", "HEAD", usize::MAX, true, false, false),
            ("terminator", "GET", 1, false, false, false),
        ] {
            let request = HttpRequest::parse(
                format!("{method} /stream/0123456789abcdef0123456789abcdef HTTP/1.1\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
            let header = HeaderSnapshot::new(
                1,
                0,
                "RAW",
                if initial_header {
                    Bytes::from_static(b"HEAD")
                } else {
                    Bytes::new()
                },
            );
            let (tx, rx) = mpsc::channel(1);
            if body {
                tx.send(Content::new(
                    1,
                    Duration::ZERO,
                    0,
                    Bytes::from_static(b"DATA"),
                    peercast_core::content::ContinuationFlags::NONE,
                ))
                .await
                .unwrap();
            }
            drop(tx);
            let mut writer = StallingWriter {
                allowed_writes,
                stall_flush,
            };
            let outcome = tokio::time::timeout(
                Duration::from_secs(2),
                stream_media_to_client_with_write_timeout(
                    &mut writer,
                    &request,
                    &header,
                    rx,
                    SafeStartConfig {
                        mode: SafeStartMode::Raw,
                        ..SafeStartConfig::default()
                    },
                    Duration::from_millis(20),
                ),
            )
            .await
            .expect("write deadline must release the stream");
            assert_eq!(outcome, Err(StreamerError::Stalled), "phase: {phase}");
        }
    }

    #[tokio::test]
    async fn write_deadline_does_not_limit_stream_lifetime() {
        let request =
            HttpRequest::parse(b"GET /stream/0123456789abcdef0123456789abcdef HTTP/1.1\r\n\r\n")
                .unwrap();
        let header = HeaderSnapshot::new(1, 0, "RAW", Bytes::new());
        let (tx, rx) = mpsc::channel(1);
        let producer = tokio::spawn(async move {
            for pos in 0..3 {
                tokio::time::sleep(Duration::from_millis(40)).await;
                tx.send(Content::new(
                    1,
                    Duration::ZERO,
                    pos,
                    Bytes::from_static(b"A"),
                    peercast_core::content::ContinuationFlags::NONE,
                ))
                .await
                .unwrap();
            }
        });
        let mut output = Vec::new();
        stream_media_to_client_with_write_timeout(
            &mut output,
            &request,
            &header,
            rx,
            SafeStartConfig {
                mode: SafeStartMode::Raw,
                ..SafeStartConfig::default()
            },
            Duration::from_millis(20),
        )
        .await
        .unwrap();
        producer.await.unwrap();
        assert!(output.ends_with(b"1\r\nA\r\n1\r\nA\r\n1\r\nA\r\n0\r\n\r\n"));
    }

    #[tokio::test]
    async fn payload_writer_preserves_framing_across_short_writes() {
        for framing in [HttpFraming::Chunked, HttpFraming::RawContinuous] {
            let (mut writer, mut reader) = tokio::io::duplex(2);
            let output = tokio::spawn(async move {
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes).await.unwrap();
                bytes
            });
            write_payload(&mut writer, b"abcdefgh", framing)
                .await
                .unwrap();
            writer.shutdown().await.unwrap();
            let expected: &[u8] = if framing == HttpFraming::Chunked {
                b"8\r\nabcdefgh\r\n"
            } else {
                b"abcdefgh"
            };
            assert_eq!(output.await.unwrap(), expected);
        }
    }
}
