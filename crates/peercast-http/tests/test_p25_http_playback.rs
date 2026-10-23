use bytes::{BufMut, BytesMut};
use peercast_core::cache::HeaderSnapshot;
use peercast_core::content::{Content, ContinuationFlags};
use peercast_format::flv::{FlvParserEvent, FlvStreamParser, FlvTagType};
use peercast_format::safestart::SafeStartMode;
use peercast_http::playlist::{PlaylistBuilder, PlaylistEntry, PlaylistFormat};
use peercast_http::router::{
    HttpMethod, HttpRequest, HttpVersion, RouteTarget, RoutingError,
};
use peercast_http::streamer::{
    build_http_200_headers, resolve_framing, stream_media_to_client, HttpFraming,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

#[test]
fn test_p25_http_framing_parity_and_mms_guards() {
    // 1. HTTP/1.1 with FLV: must be chunked
    let framing1 = resolve_framing(HttpVersion::Http11, "video/x-flv");
    assert_eq!(framing1, HttpFraming::Chunked);
    let hdr1 = String::from_utf8(build_http_200_headers(HttpVersion::Http11, "video/x-flv", framing1).to_vec()).unwrap();
    assert!(hdr1.contains("HTTP/1.1 200 OK"));
    assert!(hdr1.contains("Transfer-Encoding: chunked"));

    // 2. HTTP/1.0 with FLV: MUST NOT be chunked (REQ-060 stop/block rule)
    let framing2 = resolve_framing(HttpVersion::Http10, "video/x-flv");
    assert_eq!(framing2, HttpFraming::RawContinuous);
    let hdr2 = String::from_utf8(build_http_200_headers(HttpVersion::Http10, "video/x-flv", framing2).to_vec()).unwrap();
    assert!(hdr2.contains("HTTP/1.0 200 OK"));
    assert!(!hdr2.contains("Transfer-Encoding"));
    assert!(hdr2.contains("Connection: close"));

    // 3. HTTP/1.1 with MMS / ASF: MUST NOT be chunked (REQ-060 stop/block rule)
    let framing3 = resolve_framing(HttpVersion::Http11, "video/x-ms-asf");
    assert_eq!(framing3, HttpFraming::RawContinuous);
    let hdr3 = String::from_utf8(build_http_200_headers(HttpVersion::Http11, "video/x-ms-asf", framing3).to_vec()).unwrap();
    assert!(!hdr3.contains("Transfer-Encoding"));
    assert!(hdr3.contains("Connection: close"));

    let framing4 = resolve_framing(HttpVersion::Http11, "application/x-mms-framed");
    assert_eq!(framing4, HttpFraming::RawContinuous);
}

#[test]
fn test_p25_channel_id_validation_and_negative_routing() {
    // 1. Valid channel ID with extension
    let req1 = HttpRequest::parse(b"GET /stream/9778E62BDC59DF56F9216D0387F80BF2.flv HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
    assert_eq!(req1.method, HttpMethod::Get);
    assert_eq!(req1.version, HttpVersion::Http11);
    assert_eq!(
        req1.target,
        RouteTarget::Stream {
            channel_id_hex: "9778E62BDC59DF56F9216D0387F80BF2".to_string(),
            extension: Some("flv".to_string()),
        }
    );

    // 2. Valid playlist request
    let req2 = HttpRequest::parse(b"GET /pls/9778E62BDC59DF56F9216D0387F80BF2.m3u?tip=127.0.0.1 HTTP/1.0\r\n\r\n").unwrap();
    assert_eq!(req2.version, HttpVersion::Http10);
    assert_eq!(
        req2.target,
        RouteTarget::Playlist {
            channel_id_hex: "9778E62BDC59DF56F9216D0387F80BF2".to_string(),
            format: PlaylistFormat::M3U,
            extension: Some("m3u".to_string()),
        }
    );
    assert_eq!(req2.query_params, vec![("tip".to_string(), "127.0.0.1".to_string())]);

    // 3. Negative test: Missing channel ID
    let err1 = HttpRequest::parse(b"GET /stream/ HTTP/1.1\r\n\r\n").unwrap_err();
    assert!(matches!(err1, RoutingError::InvalidChannelId(_)));

    // 4. Negative test: Invalid non-hex characters
    let err2 = HttpRequest::parse(b"GET /stream/9778E62BDC59DF56F9216D0387F80ZZZ.flv HTTP/1.1\r\n\r\n").unwrap_err();
    assert!(matches!(err2, RoutingError::InvalidChannelId(_)));

    // 5. Negative test: Invalid length (too short)
    let err3 = HttpRequest::parse(b"GET /stream/9778E62B.flv HTTP/1.1\r\n\r\n").unwrap_err();
    assert!(matches!(err3, RoutingError::InvalidChannelId(_)));

    // 6. Negative test: Unsupported method
    let err4 = HttpRequest::parse(b"POST /stream/9778E62BDC59DF56F9216D0387F80BF2.flv HTTP/1.1\r\n\r\n").unwrap_err();
    assert!(matches!(err4, RoutingError::MethodNotAllowed(_)));

    // 7. Negative test: Unknown path
    let err5 = HttpRequest::parse(b"GET /unknown/path HTTP/1.1\r\n\r\n").unwrap_err();
    assert!(matches!(err5, RoutingError::NotFound(_)));
}

#[test]
fn test_p25_playlist_escaping_and_negative_injection() {
    // 1. M3U with CRLF injection attack attempt
    let builder_m3u = PlaylistBuilder::new(PlaylistFormat::M3U, "localhost:7144")
        .add_query_param("tip", "127.0.0.1")
        .add_entry(PlaylistEntry {
            channel_id_hex: "9778E62BDC59DF56F9216D0387F80BF2".to_string(),
            title: "Safe Title\r\n#EXTINF:-1, Injected Title\r\nhttp://evil.com/".to_string(),
            extension: "flv".to_string(),
            contact_url: None,
        });

    let m3u_bytes = builder_m3u.build();
    let m3u_str = String::from_utf8(m3u_bytes.to_vec()).unwrap();

    // Verify newline injection was neutralized into a single line title; no injected URL line exists
    let lines: Vec<&str> = m3u_str.lines().collect();
    assert_eq!(lines[0], "#EXTM3U");
    assert!(lines[1].starts_with("#EXTINF:-1, Safe Title"));
    assert_eq!(lines.len(), 3, "Must only have header, title, and one stream URL");
    assert!(lines[2].starts_with("http://localhost:7144/stream/"));

    // 2. ASX with XML injection attack attempt
    let builder_asx = PlaylistBuilder::new(PlaylistFormat::ASX, "localhost:7144")
        .add_entry(PlaylistEntry {
            channel_id_hex: "9778E62BDC59DF56F9216D0387F80BF2".to_string(),
            title: "Title with <script>evil()</script> & 'quotes' \"double\"".to_string(),
            extension: "wmv".to_string(),
            contact_url: Some("http://contact.example.com/?a=1&b=2".to_string()),
        });

    let asx_bytes = builder_asx.build();
    let asx_str = String::from_utf8(asx_bytes.to_vec()).unwrap();

    // Verify XML escaping of special characters
    assert!(!asx_str.contains("<script>"));
    assert!(asx_str.contains("&lt;script&gt;evil()&lt;/script&gt;"));
    assert!(asx_str.contains("&amp;"));
    assert!(asx_str.contains("&quot;"));
    assert!(asx_str.contains("&apos;"));
    assert!(asx_str.contains("<Ref href=\"mms://localhost:7144/stream/9778E62BDC59DF56F9216D0387F80BF2.wmv\" />"));

    // 3. PLS generation
    let builder_pls = PlaylistBuilder::new(PlaylistFormat::PLS, "localhost:7144")
        .add_entry(PlaylistEntry {
            channel_id_hex: "9778E62BDC59DF56F9216D0387F80BF2".to_string(),
            title: "Channel One".to_string(),
            extension: "mp3".to_string(),
            contact_url: None,
        });

    let pls_bytes = builder_pls.build();
    let pls_str = String::from_utf8(pls_bytes.to_vec()).unwrap();
    assert!(pls_str.contains("[playlist]"));
    assert!(pls_str.contains("NumberOfEntries=1"));
    assert!(pls_str.contains("File1=http://localhost:7144/stream/9778E62BDC59DF56F9216D0387F80BF2.mp3"));
    assert!(pls_str.contains("Title1=Channel One"));
}

/// Helper: construct standard FLV prelude (13 bytes) + minimal metadata tag.
fn make_flv_header_snapshot() -> HeaderSnapshot {
    let mut out = BytesMut::new();
    // 13-byte prelude
    out.put_slice(b"FLV");
    out.put_u8(1); // Version 1
    out.put_u8(0x05); // audio | video
    out.put_u32(9); // DataOffset
    out.put_u32(0); // PreviousTagSize0 = 0

    // Minimal metadata tag (AMF0 ScriptData)
    let meta_body = vec![2, 0, 10, b'o', b'n', b'M', b'e', b't', b'a', b'D', b'a', b't', b'a', 8, 0, 0, 0, 0, 0, 0, 9];
    out.put_u8(18); // ScriptData
    out.put_u8(0);
    out.put_u8(0);
    out.put_u8(meta_body.len() as u8);
    out.put_u32(0); // timestamp
    out.put_u8(0);
    out.put_u8(0);
    out.put_u8(0); // stream id
    out.put_slice(&meta_body);
    out.put_u32(11 + meta_body.len() as u32); // PreviousTagSize

    HeaderSnapshot::new(1, 0, "FLV", out.freeze())
}

/// Helper: construct an FLV video keyframe tag chunk.
fn make_flv_keyframe_chunk(epoch: i32, pos: i64) -> Content {
    let mut data = BytesMut::new();
    let body = vec![0x17, 0x01, 0x00, 0x00, 0x00, 0x65, 0x88, 0x99]; // AVC keyframe
    data.put_u8(9); // Video tag
    data.put_u8(0);
    data.put_u8(0);
    data.put_u8(body.len() as u8);
    data.put_u32(100); // timestamp
    data.put_u8(0);
    data.put_u8(0);
    data.put_u8(0); // stream id
    data.put_slice(&body);
    data.put_u32(11 + body.len() as u32); // PreviousTagSize

    Content::new(epoch, Duration::from_millis(100), pos, data.freeze(), ContinuationFlags::NONE)
}

#[tokio::test]
async fn test_p25_loopback_http_streaming_flv_and_safe_start() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (tx, rx) = mpsc::channel(16);
    let header_snapshot = make_flv_header_snapshot();

    // Server background task
    let server_handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        let req = HttpRequest::parse(&buf[..n]).unwrap();

        stream_media_to_client(
            &mut socket,
            &req,
            &header_snapshot,
            rx,
            SafeStartMode::VideoKeyframe,
        )
        .await
        .unwrap();
    });

    // Client task connecting over real loopback TCP socket
    let mut client = TcpStream::connect(addr).await.unwrap();
    let req_bytes = b"GET /stream/9778E62BDC59DF56F9216D0387F80BF2.flv HTTP/1.1\r\nHost: localhost\r\n\r\n";
    client.write_all(req_bytes).await.unwrap();

    // Send a media keyframe chunk through the channel
    tx.send(make_flv_keyframe_chunk(1, 1000)).await.unwrap();
    drop(tx); // Close channel to terminate stream

    // Client reads response
    let mut response_bytes = Vec::new();
    client.read_to_end(&mut response_bytes).await.unwrap();

    // Verify HTTP 200 chunked headers
    let resp_str = String::from_utf8_lossy(&response_bytes);
    assert!(resp_str.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(resp_str.contains("Content-Type: video/x-flv\r\n"));
    assert!(resp_str.contains("Transfer-Encoding: chunked\r\n"));

    // Extract body and decode HTTP chunked framing
    let header_end = response_bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let mut chunked_body = &response_bytes[header_end + 4..];

    let mut raw_flv = Vec::new();
    while !chunked_body.is_empty() {
        let crlf = chunked_body.windows(2).position(|w| w == b"\r\n").unwrap();
        let len_hex = std::str::from_utf8(&chunked_body[..crlf]).unwrap();
        let chunk_len = usize::from_str_radix(len_hex.trim(), 16).unwrap();
        chunked_body = &chunked_body[crlf + 2..];

        if chunk_len == 0 {
            break;
        }

        raw_flv.extend_from_slice(&chunked_body[..chunk_len]);
        chunked_body = &chunked_body[chunk_len + 2..]; // skip data + \r\n
    }

    // Pass the decoded FLV bytes to an independent FlvStreamParser (P23)
    let mut parser = FlvStreamParser::default();
    parser.feed(&raw_flv);

    let mut event_count = 0;
    let mut file_header_found = false;
    let mut keyframe_found = false;

    while let Some(ev) = parser.next_event().expect("De-chunked FLV stream must parse cleanly in player") {
        event_count += 1;
        match ev {
            FlvParserEvent::FileHeader(h) => {
                assert!(h.has_audio);
                assert!(h.has_video);
                file_header_found = true;
            }
            FlvParserEvent::Tag(t) => {
                assert!(t.is_valid_footer());
                if t.header.tag_type == FlvTagType::Video && t.is_keyframe() {
                    keyframe_found = true;
                }
            }
        }
    }

    assert!(file_header_found, "FLV file header must be parsed");
    assert!(keyframe_found, "Video keyframe must be parsed");
    assert!(event_count >= 2);

    server_handle.await.unwrap();
}

#[tokio::test]
async fn test_p25_http_10_raw_streaming_loopback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (tx, rx) = mpsc::channel(16);
    let header_snapshot = make_flv_header_snapshot();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        let req = HttpRequest::parse(&buf[..n]).unwrap();

        // Must stream raw continuous body for HTTP/1.0
        stream_media_to_client(
            &mut socket,
            &req,
            &header_snapshot,
            rx,
            SafeStartMode::VideoKeyframe,
        )
        .await
        .unwrap();
    });

    let mut client = TcpStream::connect(addr).await.unwrap();
    // HTTP/1.0 request
    let req_bytes = b"GET /stream/9778E62BDC59DF56F9216D0387F80BF2.flv HTTP/1.0\r\n\r\n";
    client.write_all(req_bytes).await.unwrap();

    tx.send(make_flv_keyframe_chunk(1, 1000)).await.unwrap();
    drop(tx);

    let mut response_bytes = Vec::new();
    client.read_to_end(&mut response_bytes).await.unwrap();

    let resp_str = String::from_utf8_lossy(&response_bytes);
    assert!(resp_str.starts_with("HTTP/1.0 200 OK\r\n"));
    assert!(resp_str.contains("Connection: close\r\n"));
    assert!(!resp_str.contains("Transfer-Encoding: chunked")); // REQ-060 guard!

    let header_end = response_bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let raw_flv = &response_bytes[header_end + 4..];

    // Raw body directly parses in FlvStreamParser without chunked decoding
    let mut parser = FlvStreamParser::default();
    parser.feed(raw_flv);
    let ev1 = parser.next_event().unwrap().unwrap();
    assert!(matches!(ev1, FlvParserEvent::FileHeader(_)));
}

#[tokio::test]
async fn test_p25_client_disconnect_and_resource_cleanup() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (tx, rx) = mpsc::channel(16);
    let header_snapshot = make_flv_header_snapshot();

    let server_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        let req = HttpRequest::parse(&buf[..n]).unwrap();

        let result = stream_media_to_client(
            &mut socket,
            &req,
            &header_snapshot,
            rx,
            SafeStartMode::VideoKeyframe,
        )
        .await;

        // Must return I/O error / ClientDisconnected upon client abrupt close
        assert!(result.is_err(), "Server must detect client disconnect");
    });

    let mut client = TcpStream::connect(addr).await.unwrap();
    client.write_all(b"GET /stream/9778E62BDC59DF56F9216D0387F80BF2.flv HTTP/1.1\r\n\r\n").await.unwrap();

    // Read only first few bytes of response headers, then abruptly drop socket
    let mut buf = [0u8; 32];
    let n = client.read(&mut buf).await.unwrap();
    assert!(n > 0);
    drop(client); // Abrupt client disconnection

    // Send chunks to force server write to broken socket
    for i in 0..10 {
        let _ = tx.send(make_flv_keyframe_chunk(1, 1000 + i * 100)).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Server must finish promptly and release resources
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("Server task must not hang indefinitely upon client disconnection")
        .unwrap();
}
