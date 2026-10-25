use bytes::{BufMut, Bytes, BytesMut};
use peercast_format::flv::{FlvDemuxOutput, FlvDemuxer, FlvTagType};
use peercast_http::ingest::{
    parse_ingest_handshake, read_chunked_stream, HttpPushConfig, HttpPushIngestListener,
    IngestError,
};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

#[test]
fn test_p26_authorized_and_unauthorized_post() {
    // 1. Unauthorized: missing auth token
    let raw_no_auth = b"POST / HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let res1 = parse_ingest_handshake(raw_no_auth, Some("secret_key"));
    assert_eq!(res1, Err(IngestError::Unauthorized));

    // 2. Unauthorized: wrong bearer token
    let raw_wrong_auth = b"POST / HTTP/1.1\r\nAuthorization: Bearer wrong_token\r\n\r\n";
    let res2 = parse_ingest_handshake(raw_wrong_auth, Some("secret_key"));
    assert_eq!(res2, Err(IngestError::Unauthorized));

    // 3. Authorized: correct bearer token in header
    let raw_valid_header = b"POST / HTTP/1.1\r\nAuthorization: Bearer secret_key\r\nTransfer-Encoding: chunked\r\n\r\n";
    let res3 = parse_ingest_handshake(raw_valid_header, Some("secret_key")).unwrap();
    assert!(res3.is_chunked);

    // 4. Authorized: correct token in query param (?auth=secret_key)
    let raw_valid_query = b"POST /?auth=secret_key HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
    let res4 = parse_ingest_handshake(raw_valid_query, Some("secret_key")).unwrap();
    assert!(res4.is_chunked);

    // 5. Method not allowed for non-POST
    let raw_get = b"GET / HTTP/1.1\r\n\r\n";
    let res5 = parse_ingest_handshake(raw_get, None);
    assert_eq!(res5, Err(IngestError::MethodNotAllowed("GET".to_string())));

    let raw_content_length = b"POST / HTTP/1.1\r\nContent-Length: 4\r\n\r\n";
    assert!(
        !parse_ingest_handshake(raw_content_length, None)
            .unwrap()
            .is_chunked
    );
    let raw_unsupported = b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n";
    assert_eq!(
        parse_ingest_handshake(raw_unsupported, None),
        Err(IngestError::UnsupportedTransferEncoding)
    );
    let raw_unsupported_chain =
        b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip, gzip, chunked\r\n\r\n";
    assert_eq!(
        parse_ingest_handshake(raw_unsupported_chain, None),
        Err(IngestError::UnsupportedTransferEncoding)
    );
    for malformed in [
        &b"POST / HTTP/1.1\r\nTransfer-Encoding:\r\n\r\n"[..],
        &b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked, chunked\r\n\r\n"[..],
        &b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n"[..],
        &b"POST / HTTP/1.1\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n"[..],
    ] {
        assert!(matches!(
            parse_ingest_handshake(malformed, None),
            Err(IngestError::MalformedRequest(_))
        ));
    }
}

#[tokio::test]
async fn test_p26_conflicting_bind_error() {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut listener1 = HttpPushIngestListener::new(HttpPushConfig::new(addr));
    let bound_addr = listener1.bind().await.unwrap();

    // Second listener attempting to bind the exact same port must return BindError
    let mut listener2 = HttpPushIngestListener::new(HttpPushConfig::new(bound_addr));
    let res2 = listener2.bind().await;
    assert!(
        matches!(res2, Err(IngestError::BindError(_))),
        "Conflicting port bind must return BindError"
    );
}

#[tokio::test]
async fn test_p26_configured_header_limit_is_enforced_during_read() {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut listener =
        HttpPushIngestListener::new(HttpPushConfig::new(addr).with_max_header_size(20));
    let bound_addr = listener.bind().await.unwrap();

    let server_task = tokio::spawn(async move { listener.accept_connection().await });
    let mut client = TcpStream::connect(bound_addr).await.unwrap();
    let oversized_request = b"POST / HTTP/1.1\r\nHost: localhost\r\n\r\n";
    client.write_all(&oversized_request[..20]).await.unwrap();

    let result = server_task.await.unwrap();
    assert_eq!(
        result.unwrap_err(),
        IngestError::HeaderTooLarge {
            actual: 20,
            max: 20
        }
    );
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 431"));
}

#[tokio::test]
async fn test_p26_transfer_encoding_errors_have_status_and_listener_recovers() {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut listener = HttpPushIngestListener::new(HttpPushConfig::new(addr));
    let bound_addr = listener.bind().await.unwrap();
    let (errors_tx, mut errors_rx) = mpsc::channel(2);

    let server_task = tokio::spawn(async move {
        for _ in 0..2 {
            let error = listener.accept_connection().await.unwrap_err();
            errors_tx.send(error).await.unwrap();
        }
        let (_, handshake, _) = listener.accept_connection().await.unwrap();
        assert!(handshake.is_chunked);
    });

    for (request, expected_status, expected_error) in [
        (
            &b"POST / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n"[..],
            "HTTP/1.1 501 Not Implemented",
            IngestError::UnsupportedTransferEncoding,
        ),
        (
            &b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n"
                [..],
            "HTTP/1.1 400 Bad Request",
            IngestError::MalformedRequest("Multiple Transfer-Encoding fields"),
        ),
    ] {
        let mut client = TcpStream::connect(bound_addr).await.unwrap();
        client.write_all(request).await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).starts_with(expected_status));
        assert_eq!(errors_rx.recv().await.unwrap(), expected_error);
    }

    let mut valid_client = TcpStream::connect(bound_addr).await.unwrap();
    valid_client
        .write_all(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
        .await
        .unwrap();
    let mut response = [0u8; 128];
    let n = valid_client.read(&mut response).await.unwrap();
    assert!(String::from_utf8_lossy(&response[..n]).starts_with("HTTP/1.1 200 OK"));
    server_task.await.unwrap();
}

#[tokio::test]
async fn test_p26_header_timeout_is_total_and_listener_accepts_next_request() {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut listener = HttpPushIngestListener::new(
        HttpPushConfig::new(addr).with_header_timeout(Duration::from_millis(180)),
    );
    let bound_addr = listener.bind().await.unwrap();
    let (timed_out_tx, timed_out_rx) = tokio::sync::oneshot::channel();

    let server_task = tokio::spawn(async move {
        let first = listener.accept_connection().await;
        timed_out_tx
            .send(first.as_ref().unwrap_err().clone())
            .unwrap();
        let (_, handshake, _) = listener.accept_connection().await.unwrap();
        assert!(handshake.is_chunked);
    });

    let mut slow_client = TcpStream::connect(bound_addr).await.unwrap();
    slow_client.write_all(b"P").await.unwrap();
    let trickle_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(60)).await;
            if slow_client.write_all(b"x").await.is_err() {
                break;
            }
        }
    });

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), timed_out_rx)
            .await
            .expect("header deadline should release the connection")
            .unwrap(),
        IngestError::HeaderTimedOut
    );
    trickle_task.abort();
    let _ = trickle_task.await;

    let mut next_client = TcpStream::connect(bound_addr).await.unwrap();
    next_client
        .write_all(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
        .await
        .unwrap();
    let mut response = [0u8; 128];
    let n = next_client.read(&mut response).await.unwrap();
    assert!(String::from_utf8_lossy(&response[..n]).starts_with("HTTP/1.1 200 OK"));
    server_task.await.unwrap();
}

#[tokio::test]
async fn test_p26_chunked_stream_dechunking_and_malformed_rejection() {
    // 1. Valid chunked stream
    let (tx1, mut rx1) = mpsc::channel(16);
    let mut valid_chunked = &b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"[..];

    read_chunked_stream(&mut valid_chunked, Bytes::new(), tx1, 1024 * 1024)
        .await
        .unwrap();

    let chunk1 = rx1.recv().await.unwrap();
    assert_eq!(chunk1.as_ref(), b"hello");
    let chunk2 = rx1.recv().await.unwrap();
    assert_eq!(chunk2.as_ref(), b" world");
    assert!(rx1.recv().await.is_none());

    // 2. Malformed chunk size (invalid hex characters)
    let (tx2, _rx2) = mpsc::channel(16);
    let mut malformed_hex = &b"ZZ\r\ninvalid\r\n"[..];
    let res2 = read_chunked_stream(&mut malformed_hex, Bytes::new(), tx2, 1024 * 1024).await;
    assert!(
        matches!(res2, Err(IngestError::MalformedChunking(_))),
        "Malformed hex must return MalformedChunking"
    );

    // 3. Chunk exceeding maximum budget (REQ-027, Global Rule 3)
    let (tx3, _rx3) = mpsc::channel(16);
    let mut oversized = &b"1000\r\n"[..]; // declares 4096 bytes with budget of 1024
    let res3 = read_chunked_stream(&mut oversized, Bytes::new(), tx3, 1024).await;
    assert!(
        matches!(
            res3,
            Err(IngestError::ChunkTooLarge {
                actual: 4096,
                max: 1024
            })
        ),
        "Oversized chunk must be rejected"
    );
}

#[tokio::test]
async fn test_p26_chunked_stream_validates_data_terminator_and_trailers() {
    let (mut writer, mut reader) = tokio::io::duplex(8);
    let (tx, mut rx) = mpsc::channel(4);
    let writer_task = tokio::spawn(async move {
        for piece in [
            &b"\nab"[..],
            &b"\r"[..],
            &b"\n0\r\nX-Test: yes\r\n\r"[..],
            &b"\n"[..],
        ] {
            writer.write_all(piece).await.unwrap();
            tokio::task::yield_now().await;
        }
    });
    read_chunked_stream(&mut reader, Bytes::from_static(b"2\r"), tx, 1024)
        .await
        .unwrap();
    writer_task.await.unwrap();
    assert_eq!(rx.recv().await.unwrap().as_ref(), b"ab");
    assert!(rx.recv().await.is_none());
}

#[tokio::test]
async fn test_p26_chunked_stream_rejects_incomplete_or_invalid_framing() {
    let (tx, mut rx) = mpsc::channel(4);
    let mut bad_data_terminator = &b"2\r\nabXX0\r\n\r\n"[..];
    assert!(matches!(
        read_chunked_stream(&mut bad_data_terminator, Bytes::new(), tx, 1024).await,
        Err(IngestError::MalformedChunking(
            "Missing chunk data terminator"
        ))
    ));
    assert!(rx.try_recv().is_err(), "invalid chunk must not be emitted");

    let (tx, _rx) = mpsc::channel(4);
    let mut incomplete_body = &b"5\r\nab"[..];
    assert_eq!(
        read_chunked_stream(&mut incomplete_body, Bytes::new(), tx, 1024).await,
        Err(IngestError::ConnectionClosed)
    );

    let (tx, _rx) = mpsc::channel(4);
    let mut incomplete_zero_chunk = &b"0\r\n"[..];
    assert_eq!(
        read_chunked_stream(&mut incomplete_zero_chunk, Bytes::new(), tx, 1024).await,
        Err(IngestError::ConnectionClosed)
    );

    let (tx, _rx) = mpsc::channel(4);
    let mut invalid_trailer = &b"0\r\nBad Field: value\r\n\r\n"[..];
    assert!(matches!(
        read_chunked_stream(&mut invalid_trailer, Bytes::new(), tx, 1024).await,
        Err(IngestError::MalformedChunking("Invalid trailer field name"))
    ));

    let mut oversized_trailer = b"0\r\nX: ".to_vec();
    oversized_trailer.extend(std::iter::repeat_n(b'x', 8192));
    oversized_trailer.extend_from_slice(b"\r\n\r\n");
    let (tx, _rx) = mpsc::channel(4);
    assert!(matches!(
        read_chunked_stream(&mut oversized_trailer.as_slice(), Bytes::new(), tx, 1024).await,
        Err(IngestError::MalformedChunking(
            "Chunk line or trailers too large"
        ))
    ));

    let (tx, _rx) = mpsc::channel(4);
    let mut signed_hex = &b"-1\r\n"[..];
    assert!(matches!(
        read_chunked_stream(&mut signed_hex, Bytes::new(), tx, 1024).await,
        Err(IngestError::MalformedChunking("Invalid chunk size hex"))
    ));

    let (tx, _rx) = mpsc::channel(4);
    let mut plus_hex = &b"+1\r\na\r\n0\r\n\r\n"[..];
    assert!(matches!(
        read_chunked_stream(&mut plus_hex, Bytes::new(), tx, 1024).await,
        Err(IngestError::MalformedChunking("Invalid chunk size hex"))
    ));

    for control_trailer in [
        &b"0\r\nX-Test: a\0b\r\n\r\n"[..],
        &b"0\r\nX-Test: a\nb\r\n\r\n"[..],
        &b"0\r\nX-Test: a\x7fb\r\n\r\n"[..],
    ] {
        let (tx, _rx) = mpsc::channel(4);
        let mut control_in_trailer = control_trailer;
        assert!(matches!(
            read_chunked_stream(&mut control_in_trailer, Bytes::new(), tx, 1024).await,
            Err(IngestError::MalformedChunking(
                "Invalid trailer field value"
            ))
        ));
    }
}

#[tokio::test]
async fn test_p26_client_cancellation_during_ingest() {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut listener = HttpPushIngestListener::new(HttpPushConfig::new(addr));
    let bound_addr = listener.bind().await.unwrap();

    let server_task = tokio::spawn(async move {
        let (mut socket, handshake, prefix) = listener.accept_connection().await.unwrap();
        assert!(handshake.is_chunked);

        let (tx, mut rx) = mpsc::channel(16);
        let ingest_task =
            tokio::spawn(
                async move { read_chunked_stream(&mut socket, prefix, tx, 1024 * 1024).await },
            );

        // Read one chunk from client
        let first = rx.recv().await;
        assert!(first.is_some());

        // Ingest task should return ConnectionClosed upon client abort
        let res = ingest_task.await.unwrap();
        assert_eq!(res, Err(IngestError::ConnectionClosed));
    });

    let mut client = TcpStream::connect(bound_addr).await.unwrap();
    client
        .write_all(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
        .await
        .unwrap();

    // Read 200 OK handshake response from server
    let mut resp = [0u8; 64];
    let _ = client.read(&mut resp).await.unwrap();

    // Write one valid chunk
    client.write_all(b"4\r\ntest\r\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Abruptly close socket (cancellation)
    drop(client);

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("Server must finish promptly upon client cancellation")
        .unwrap();
}

#[tokio::test]
async fn test_p26_real_ffmpeg_flv_ingest_pipeline() {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut listener =
        HttpPushIngestListener::new(HttpPushConfig::new(addr).with_auth("obs_token"));
    let bound_addr = listener.bind().await.unwrap();

    let (tx_demux, mut rx_demux) = mpsc::channel(16);

    // Ingest server task feeding into FlvDemuxer pipeline
    let server_task = tokio::spawn(async move {
        let (mut socket, handshake, prefix) = listener.accept_connection().await.unwrap();
        assert_eq!(handshake.content_type.as_deref(), Some("video/x-flv"));
        assert!(handshake.is_chunked);

        let (tx_raw, mut rx_raw) = mpsc::channel(16);
        let dechunker = tokio::spawn(async move {
            read_chunked_stream(&mut socket, prefix, tx_raw, 1024 * 1024).await
        });

        let mut demuxer = FlvDemuxer::default();
        while let Some(chunk) = rx_raw.recv().await {
            demuxer.feed(&chunk);
            while let Some(out) = demuxer.next_output().unwrap() {
                tx_demux.send(out).await.unwrap();
            }
        }

        dechunker.await.unwrap().unwrap();
    });

    // Client acting as ffmpeg / OBS pushing FLV over HTTP POST
    let mut client = TcpStream::connect(bound_addr).await.unwrap();
    let post_req = b"POST /live?auth=obs_token HTTP/1.1\r\n\
                     Host: 127.0.0.1\r\n\
                     User-Agent: Lavf/58.76.100 (ffmpeg)\r\n\
                     Content-Type: video/x-flv\r\n\
                     Transfer-Encoding: chunked\r\n\r\n";
    client.write_all(post_req).await.unwrap();

    let mut resp = [0u8; 64];
    let _ = client.read(&mut resp).await.unwrap();

    // 1. Build FLV stream chunks to simulate encoder output
    let mut flv_prelude = BytesMut::new();
    flv_prelude.put_slice(b"FLV");
    flv_prelude.put_u8(1);
    flv_prelude.put_u8(0x05); // audio | video
    flv_prelude.put_u32(9);
    flv_prelude.put_u32(0); // PreviousTagSize0

    // Metadata tag
    let meta_body = vec![
        2, 0, 10, b'o', b'n', b'M', b'e', b't', b'a', b'D', b'a', b't', b'a', 8, 0, 0, 0, 0, 0, 0,
        9,
    ];
    flv_prelude.put_u8(18); // ScriptData
    flv_prelude.put_u8(0);
    flv_prelude.put_u8(0);
    flv_prelude.put_u8(meta_body.len() as u8);
    flv_prelude.put_u32(0);
    flv_prelude.put_u8(0);
    flv_prelude.put_u8(0);
    flv_prelude.put_u8(0);
    flv_prelude.put_slice(&meta_body);
    flv_prelude.put_u32(11 + meta_body.len() as u32);

    // Send first chunked slice
    let chunk1 = format!("{:x}\r\n", flv_prelude.len());
    client.write_all(chunk1.as_bytes()).await.unwrap();
    client.write_all(&flv_prelude).await.unwrap();
    client.write_all(b"\r\n").await.unwrap();

    // Video keyframe tag
    let mut keyframe_tag = BytesMut::new();
    let body = vec![0x17, 0x01, 0x00, 0x00, 0x00, 0x65, 0x01, 0x02];
    keyframe_tag.put_u8(9); // Video tag
    keyframe_tag.put_u8(0);
    keyframe_tag.put_u8(0);
    keyframe_tag.put_u8(body.len() as u8);
    keyframe_tag.put_u32(50); // timestamp
    keyframe_tag.put_u8(0);
    keyframe_tag.put_u8(0);
    keyframe_tag.put_u8(0);
    keyframe_tag.put_slice(&body);
    keyframe_tag.put_u32(11 + body.len() as u32);

    // Send second chunked slice
    let chunk2 = format!("{:x}\r\n", keyframe_tag.len());
    client.write_all(chunk2.as_bytes()).await.unwrap();
    client.write_all(&keyframe_tag).await.unwrap();
    client.write_all(b"\r\n").await.unwrap();

    // Send zero terminator (0\r\n\r\n)
    client.write_all(b"0\r\n\r\n").await.unwrap();
    client.flush().await.unwrap();

    // Verify pipeline outputs
    let ev1 = rx_demux.recv().await.unwrap();
    match ev1 {
        FlvDemuxOutput::HeaderChanged(s) => {
            assert_eq!(s.container_type, "FLV");
            assert!(s.data.len() >= 13);
            assert_eq!(&s.data[..3], b"FLV");
        }
        _ => panic!("Expected HeaderChanged first"),
    }

    let ev2 = rx_demux.recv().await.unwrap();
    match ev2 {
        FlvDemuxOutput::ContentTag {
            tag, is_keyframe, ..
        } => {
            assert_eq!(tag.header.tag_type, FlvTagType::Video);
            assert!(is_keyframe);
        }
        _ => panic!("Expected ContentTag keyframe"),
    }

    server_task.await.unwrap();
}
