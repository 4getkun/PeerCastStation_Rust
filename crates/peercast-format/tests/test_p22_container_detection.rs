use bytes::Bytes;
use peercast_format::sniffer::{
    detect_container, ContainerType, SniffBuffer, SniffError, SniffResult,
};

#[test]
fn test_p22_flv_detection_and_flags() {
    // 1. Valid FLV header with audio (0x04) and video (0x01): TypeFlags = 0x05
    let flv_av_header: Vec<u8> = vec![
        b'F', b'L', b'V', 0x01, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00,
    ];
    let res = detect_container(&flv_av_header);
    match res {
        SniffResult::Definitive(info) => {
            assert_eq!(info.container_type, ContainerType::FLV);
            assert_eq!(info.content_type_str, "FLV");
            assert_eq!(info.mime_type, "video/x-flv");
            assert!(info.has_audio);
            assert!(info.has_video);
        }
        other => panic!("Expected Definitive FLV, got {:?}", other),
    }

    // 2. Audio-only FLV: TypeFlags = 0x04
    let flv_audio_header: Vec<u8> = vec![
        b'F', b'L', b'V', 0x01, 0x04, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00,
    ];
    if let SniffResult::Definitive(info) = detect_container(&flv_audio_header) {
        assert!(info.has_audio);
        assert!(!info.has_video);
    } else {
        panic!("Expected Definitive FLV");
    }

    // 3. Video-only FLV: TypeFlags = 0x01
    let flv_video_header: Vec<u8> = vec![
        b'F', b'L', b'V', 0x01, 0x01, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00,
    ];
    if let SniffResult::Definitive(info) = detect_container(&flv_video_header) {
        assert!(!info.has_audio);
        assert!(info.has_video);
    } else {
        panic!("Expected Definitive FLV");
    }
}

#[test]
fn test_p22_ts_detection_and_sync_verification() {
    // 1. Valid TS with 3 consecutive 188-byte packets starting with 0x47
    let mut ts_stream = vec![0u8; 188 * 3];
    ts_stream[0] = 0x47;
    ts_stream[188] = 0x47;
    ts_stream[376] = 0x47;

    let res = detect_container(&ts_stream);
    match res {
        SniffResult::Definitive(info) => {
            assert_eq!(info.container_type, ContainerType::TS);
            assert_eq!(info.content_type_str, "TS");
            assert_eq!(info.mime_type, "video/mp2t");
            assert!(info.has_audio);
            assert!(info.has_video);
        }
        other => panic!("Expected Definitive TS, got {:?}", other),
    }

    // 2. False-positive avoidance: First byte is 0x47 (e.g. ASCII 'G'), but byte 188 is NOT 0x47
    let mut fake_ts = vec![0x20u8; 376];
    fake_ts[0] = 0x47; // 'G'
    fake_ts[188] = 0x99; // Not 0x47!

    let res_fake = detect_container(&fake_ts);
    assert_eq!(
        res_fake,
        SniffResult::Unsupported,
        "Single 0x47 without consistent TS packet framing must not trigger false positive"
    );
}

#[test]
fn test_p22_ebml_mkv_and_webm_detection() {
    // 1. WebM EBML header (>= 32 bytes)
    let mut webm_header = vec![0x1A, 0x45, 0xDF, 0xA3];
    webm_header.extend_from_slice(b"....DocType....webm............");
    let res = detect_container(&webm_header);
    match res {
        SniffResult::Definitive(info) => {
            assert_eq!(info.container_type, ContainerType::WebM);
            assert_eq!(info.content_type_str, "WEBM");
            assert_eq!(info.mime_type, "video/webm");
        }
        other => panic!("Expected Definitive WebM, got {:?}", other),
    }

    // 2. Matroska MKV EBML header (>= 32 bytes)
    let mut mkv_header = vec![0x1A, 0x45, 0xDF, 0xA3];
    mkv_header.extend_from_slice(b"....DocType....matroska........");
    let res_mkv = detect_container(&mkv_header);
    match res_mkv {
        SniffResult::Definitive(info) => {
            assert_eq!(info.container_type, ContainerType::MKV);
            assert_eq!(info.content_type_str, "MKV");
            assert_eq!(info.mime_type, "video/x-matroska");
        }
        other => panic!("Expected Definitive MKV, got {:?}", other),
    }
}

#[test]
fn test_p22_asf_ogg_mp3_detection() {
    // 1. ASF Header Object GUID
    let asf_guid = [
        0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
    ];
    let mut asf_data = asf_guid.to_vec();
    asf_data.extend_from_slice(&[0u8; 32]);
    let res_asf = detect_container(&asf_data);
    match res_asf {
        SniffResult::Definitive(info) => {
            assert_eq!(info.container_type, ContainerType::ASF);
            assert_eq!(info.content_type_str, "WMV");
            assert_eq!(info.mime_type, "video/x-ms-asf");
        }
        other => panic!("Expected Definitive ASF, got {:?}", other),
    }

    // 2. OggS
    let ogg_data = b"OggS\x00\x02\x00\x00\x00\x00\x00\x00\x00\x00";
    let res_ogg = detect_container(ogg_data);
    match res_ogg {
        SniffResult::Definitive(info) => {
            assert_eq!(info.container_type, ContainerType::Ogg);
            assert_eq!(info.content_type_str, "OGG");
            assert_eq!(info.mime_type, "application/ogg");
        }
        other => panic!("Expected Definitive Ogg, got {:?}", other),
    }

    // 3. MP3 ID3
    let id3_data = b"ID3\x04\x00\x00\x00\x00\x00\x00";
    let res_id3 = detect_container(id3_data);
    match res_id3 {
        SniffResult::Definitive(info) => {
            assert_eq!(info.container_type, ContainerType::MP3);
            assert_eq!(info.content_type_str, "MP3");
            assert_eq!(info.mime_type, "audio/mpeg");
        }
        other => panic!("Expected Definitive MP3, got {:?}", other),
    }
}

#[test]
fn test_p22_short_reads_and_ambiguous_prefixes() {
    // Empty input -> NeedsMoreData
    assert_eq!(detect_container(&[]), SniffResult::NeedsMoreData { needed_bytes: 4 });

    // 1-2 bytes FLV prefix
    assert_eq!(detect_container(b"F"), SniffResult::NeedsMoreData { needed_bytes: 13 });
    assert_eq!(detect_container(b"FL"), SniffResult::NeedsMoreData { needed_bytes: 13 });

    // 3 bytes "FLV" without 13-byte header -> NeedsMoreData 13
    assert_eq!(detect_container(b"FLV"), SniffResult::NeedsMoreData { needed_bytes: 13 });

    // Partial EBML prefix
    assert_eq!(detect_container(&[0x1A]), SniffResult::NeedsMoreData { needed_bytes: 32 });
    assert_eq!(detect_container(&[0x1A, 0x45]), SniffResult::NeedsMoreData { needed_bytes: 32 });

    // Partial Ogg prefix
    assert_eq!(detect_container(b"Og"), SniffResult::NeedsMoreData { needed_bytes: 4 });

    // Partial ASF GUID
    let partial_asf = &[0x30, 0x26, 0xB2, 0x75];
    assert_eq!(detect_container(partial_asf), SniffResult::NeedsMoreData { needed_bytes: 16 });
}

#[test]
fn test_p22_misleading_signatures_rejected() {
    // 1. Starts with "FLV", but invalid version (e.g. 0x02 instead of 0x01)
    let bad_flv_version: Vec<u8> = vec![
        b'F', b'L', b'V', 0x02, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00,
    ];
    assert_eq!(detect_container(&bad_flv_version), SniffResult::Unsupported);

    // 2. Starts with "FLV", but non-zero PreviousTagSize0
    let bad_prev_tag_size: Vec<u8> = vec![
        b'F', b'L', b'V', 0x01, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x42,
    ];
    assert_eq!(detect_container(&bad_prev_tag_size), SniffResult::Unsupported);

    // 3. Random binary garbage
    let garbage = b"SOME_RANDOM_TEXT_NOT_MEDIA_STREAM";
    assert_eq!(detect_container(garbage), SniffResult::Unsupported);
}

#[test]
fn test_p22_sniff_buffer_capacity_and_replay_fidelity() {
    let mut sniff_buf = SniffBuffer::new(256);

    // 1. Initial state
    assert!(sniff_buf.is_empty());
    assert_eq!(sniff_buf.len(), 0);

    // 2. Feed fragmented chunks incrementally
    let chunk1 = b"FL";
    sniff_buf.feed(chunk1).unwrap();
    assert_eq!(sniff_buf.detect(), SniffResult::NeedsMoreData { needed_bytes: 13 });

    let chunk2 = b"V\x01\x05\x00\x00\x00\x09\x00\x00\x00\x00";
    sniff_buf.feed(chunk2).unwrap();

    // Now complete 13-byte FLV header is present
    match sniff_buf.detect() {
        SniffResult::Definitive(info) => {
            assert_eq!(info.container_type, ContainerType::FLV);
        }
        other => panic!("Expected Definitive FLV, got {:?}", other),
    }

    // 3. Replay fidelity: Verify that the accumulated buffer can be replayed
    // to the downstream format reader with 100% bit-for-bit parity
    let replayed_bytes: Bytes = sniff_buf.replay();
    let expected_bytes = b"FLV\x01\x05\x00\x00\x00\x09\x00\x00\x00\x00";
    assert_eq!(replayed_bytes.as_ref(), expected_bytes);

    // 4. Bounded capacity enforcement (Global Rule 3)
    let oversized_chunk = vec![0xcc; 300];
    let err = sniff_buf.feed(&oversized_chunk).unwrap_err();
    assert!(matches!(err, SniffError::BufferOverflow { actual: 313, max: 256 }));
}
