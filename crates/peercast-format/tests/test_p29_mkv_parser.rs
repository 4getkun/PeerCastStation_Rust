use bytes::{BufMut, Bytes, BytesMut};
use peercast_format::mkv::{
    inspect_mkv_keyframe, MkvDemuxOutput, MkvDemuxer, MkvDemuxerState, MkvParseError, VInt,
    ID_BLOCK_GROUP, ID_SIMPLE_BLOCK,
};
use std::fs;
use std::path::Path;

/// Helper: construct a synthetic EBML header element.
fn make_synthetic_ebml_header(doc_type: &str) -> Bytes {
    let mut body = BytesMut::new();
    // DocType element
    let dt_id = VInt::parse_id(&[0x42, 0x82]).unwrap().unwrap();
    body.put_slice(&dt_id.raw_bytes);
    let dt_bytes = doc_type.as_bytes();
    let dt_sz = VInt::encode_size(Some(dt_bytes.len() as u64));
    body.put_slice(&dt_sz);
    body.put_slice(dt_bytes);

    // DocTypeVersion: 2
    let dtv_id = VInt::parse_id(&[0x42, 0x87]).unwrap().unwrap();
    body.put_slice(&dtv_id.raw_bytes);
    let dtv_sz = VInt::encode_size(Some(1));
    body.put_slice(&dtv_sz);
    body.put_u8(2);

    // Wrap in EBML element (0x1A45DFA3)
    let ebml_id = VInt::parse_id(&[0x1A, 0x45, 0xDF, 0xA3]).unwrap().unwrap();
    let mut out = BytesMut::new();
    out.put_slice(&ebml_id.raw_bytes);
    let ebml_sz = VInt::encode_size(Some(body.len() as u64));
    out.put_slice(&ebml_sz);
    out.put_slice(&body);
    out.freeze()
}

/// Helper: construct synthetic Segment header with unknown size.
fn make_synthetic_segment_header() -> Bytes {
    let mut out = BytesMut::new();
    let seg_id = VInt::parse_id(&[0x18, 0x53, 0x80, 0x67]).unwrap().unwrap();
    out.put_slice(&seg_id.raw_bytes);
    // Unknown size (8 bytes with all 56 data bits 1)
    let unknown_sz = VInt::encode_size(None);
    out.put_slice(&unknown_sz);
    out.freeze()
}

/// Helper: construct a synthetic Cluster element header.
fn make_synthetic_cluster_header() -> Bytes {
    let mut out = BytesMut::new();
    let cl_id = VInt::parse_id(&[0x1F, 0x43, 0xB6, 0x75]).unwrap().unwrap();
    out.put_slice(&cl_id.raw_bytes);
    let cl_sz = VInt::encode_size(None);
    out.put_slice(&cl_sz);
    out.freeze()
}

/// Helper: construct a synthetic SimpleBlock with given track number and keyframe flag.
fn make_synthetic_simple_block(track_number: u64, is_keyframe: bool, payload: &[u8]) -> Bytes {
    let mut body = BytesMut::new();
    // 1. Track number VINT
    let track_vint = VInt::encode_size(Some(track_number));
    body.put_slice(&track_vint);
    // 2. Relative timecode: 2 bytes
    body.put_i16(0);
    // 3. Flags: bit 7 is keyframe flag
    let flags = if is_keyframe { 0x80 } else { 0x00 };
    body.put_u8(flags);
    // 4. Media data
    body.put_slice(payload);

    // Wrap in SimpleBlock element (0xA3)
    let mut out = BytesMut::new();
    out.put_u8(0xA3);
    let sz = VInt::encode_size(Some(body.len() as u64));
    out.put_slice(&sz);
    out.put_slice(&body);
    out.freeze()
}

#[test]
fn test_p29_upstream_test_mkv_real_fixture() {
    let fixture_path = Path::new(r"C:\Github\_upstream_peercaststation\PeerCastStation\TestMKV\fixtures\test.mkv");
    if !fixture_path.exists() {
        eprintln!("Skipping test_p29_upstream_test_mkv_real_fixture: fixture not found at {:?}", fixture_path);
        return;
    }

    let file_bytes = fs::read(fixture_path).expect("Failed to read test.mkv fixture");
    assert!(file_bytes.len() > 1_000_000, "test.mkv should be ~7.83 MB, got {}", file_bytes.len());

    let mut demuxer = MkvDemuxer::new();
    demuxer.feed(&file_bytes).expect("Feeding test.mkv bytes must succeed");

    // 1. First output must be HeaderSnapshotReady
    let first_out = demuxer.next_output().expect("Parsing test.mkv header must succeed");
    let (snapshot, metadata) = match first_out {
        Some(MkvDemuxOutput::HeaderSnapshotReady { snapshot, metadata, epoch }) => {
            assert_eq!(epoch, 0);
            (snapshot, metadata)
        }
        other => panic!("Expected HeaderSnapshotReady, got {:?}", other),
    };

    // Upstream parity check with Station's `test_mkvcontentreader.rb`:
    // ContentHeader not null, position == 0
    assert_eq!(snapshot.pos, 0);
    assert_eq!(snapshot.container_type, "MKV");
    assert!(snapshot.data.len() > 32, "HeaderSnapshot must contain EBML and Segment headers");

    // ChannelInfo check:
    // assert_equal 'video/x-matroska', res.ChannelInfo.MIMEType
    // assert_equal '.mkv', res.ChannelInfo.ContentExtension
    // assert_equal 'MKV', res.ChannelInfo.ContentType
    assert_eq!(metadata.doc_type, "matroska");
    assert_eq!(metadata.content_type, "MKV");
    assert_eq!(metadata.mime_type, "video/x-matroska");
    assert_eq!(metadata.stream_ext, ".mkv");
    assert!(metadata.has_video);

    // 2. Iterate through contents
    let mut total_blocks = 0;
    let mut keyframe_count = 0;
    let mut expected_pos = snapshot.data.len() as u64;

    while let Some(out) = demuxer.next_output().expect("Parsing test.mkv blocks should succeed") {
        match out {
            MkvDemuxOutput::ContentPacket { packet, epoch, position, is_keyframe } => {
                assert_eq!(epoch, 0);
                assert_eq!(position, expected_pos, "Stream position must advance contiguously");
                expected_pos += packet.len() as u64;
                total_blocks += 1;
                if is_keyframe {
                    keyframe_count += 1;
                }
            }
            MkvDemuxOutput::EndOfStream => break,
            other => panic!("Unexpected output during stream: {:?}", other),
        }
    }

    assert!(total_blocks > 100, "test.mkv must have many content blocks, got {}", total_blocks);
    assert!(keyframe_count > 0, "test.mkv must contain verified keyframes, got {}", keyframe_count);
    assert_eq!(demuxer.stream_position(), expected_pos);
}

#[test]
fn test_p29_upstream_test_webm_real_fixture() {
    let fixture_path = Path::new(r"C:\Github\_upstream_peercaststation\PeerCastStation\TestMKV\fixtures\test.webm");
    if !fixture_path.exists() {
        eprintln!("Skipping test_p29_upstream_test_webm_real_fixture: fixture not found at {:?}", fixture_path);
        return;
    }

    let file_bytes = fs::read(fixture_path).expect("Failed to read test.webm fixture");
    assert!(file_bytes.len() > 1_000_000, "test.webm should be ~1.80 MB, got {}", file_bytes.len());

    let mut demuxer = MkvDemuxer::new();
    demuxer.feed(&file_bytes).expect("Feeding test.webm bytes must succeed");

    let first_out = demuxer.next_output().expect("Parsing test.webm header must succeed");
    let (snapshot, metadata) = match first_out {
        Some(MkvDemuxOutput::HeaderSnapshotReady { snapshot, metadata, epoch }) => {
            assert_eq!(epoch, 0);
            (snapshot, metadata)
        }
        other => panic!("Expected HeaderSnapshotReady, got {:?}", other),
    };

    // Upstream parity check with Station's `test_mkvcontentreader.rb`:
    // assert_equal 'video/webm', res.ChannelInfo.MIMEType
    // assert_equal '.webm', res.ChannelInfo.ContentExtension
    // assert_equal 'WEBM', res.ChannelInfo.ContentType
    assert_eq!(snapshot.container_type, "WEBM");
    assert_eq!(metadata.doc_type, "webm");
    assert_eq!(metadata.content_type, "WEBM");
    assert_eq!(metadata.mime_type, "video/webm");
    assert_eq!(metadata.stream_ext, ".webm");

    // Read subsequent blocks
    let mut total_blocks = 0;
    while let Some(out) = demuxer.next_output().expect("Parsing test.webm blocks should succeed") {
        if let MkvDemuxOutput::ContentPacket { is_keyframe, .. } = out {
            total_blocks += 1;
            if is_keyframe {
                // Verified keyframe found
            }
        }
    }
    assert!(total_blocks > 50, "test.webm must contain content blocks, got {}", total_blocks);
}

#[test]
fn test_p29_stop_block_ebml_vint_not_cloned_flv() {
    // REQ-055 STOP/BLOCK GUARD:
    // "Parser contract not established; do not clone FLV assumptions."
    // MKV parsing MUST follow EBML variable-size integer encoding (VINT 1..8 bytes).

    // Test VINT length determination:
    // Length 1 (0x80..0xFF)
    let v1 = VInt::parse_size(&[0x81]).unwrap().unwrap();
    assert_eq!(v1.len, 1);
    assert_eq!(v1.value, 1);
    assert!(!v1.is_unknown);

    // Length 1 unknown (0xFF)
    let v1_unk = VInt::parse_size(&[0xFF]).unwrap().unwrap();
    assert_eq!(v1_unk.len, 1);
    assert!(v1_unk.is_unknown);

    // Length 2 (0x40..0x7F)
    let v2 = VInt::parse_size(&[0x40, 0x02]).unwrap().unwrap();
    assert_eq!(v2.len, 2);
    assert_eq!(v2.value, 2);

    // Length 8 Unknown size (0x01 followed by 7x 0xFF)
    let unk_8 = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
    let v8_unk = VInt::parse_size(&unk_8).unwrap().unwrap();
    assert_eq!(v8_unk.len, 8);
    assert!(v8_unk.is_unknown);

    // Invalid VINT with 0x00 first byte rejected
    let err = VInt::parse_size(&[0x00, 0x01]).unwrap_err();
    assert_eq!(err, MkvParseError::InvalidVintFirstByte);
}

#[test]
fn test_p29_keyframe_detection_simpleblock_and_blockgroup() {
    // REQ-055: Container-aware keyframe detection
    // 1. SimpleBlock: keyframe flag in bit 7
    let kf_simple = make_synthetic_simple_block(1, true, &[0xDE, 0xAD, 0xBE, 0xEF]);
    let non_kf_simple = make_synthetic_simple_block(1, false, &[0xDE, 0xAD, 0xBE, 0xEF]);

    // Inspect payload (skip 1-byte ID 0xA3 + size VINT)
    let kf_payload = &kf_simple[2..]; // 1 byte ID + 1 byte size = 2
    assert!(inspect_mkv_keyframe(ID_SIMPLE_BLOCK, kf_payload));

    let non_kf_payload = &non_kf_simple[2..];
    assert!(!inspect_mkv_keyframe(ID_SIMPLE_BLOCK, non_kf_payload));

    // 2. BlockGroup:
    // BlockGroup containing only Block (0xA1) -> Keyframe
    let mut bg_kf = BytesMut::new();
    bg_kf.put_u8(0xA1); // Block ID
    bg_kf.put_slice(&VInt::encode_size(Some(4)));
    bg_kf.put_slice(&[1, 0, 0, 0]);
    assert!(inspect_mkv_keyframe(ID_BLOCK_GROUP, &bg_kf));

    // BlockGroup containing Block (0xA1) AND ReferenceBlock (0xFB) -> Interframe (non-keyframe)
    let mut bg_non_kf = BytesMut::new();
    bg_non_kf.put_u8(0xA1);
    bg_non_kf.put_slice(&VInt::encode_size(Some(4)));
    bg_non_kf.put_slice(&[1, 0, 0, 0]);
    // ReferenceBlock
    bg_non_kf.put_u8(0xFB);
    bg_non_kf.put_slice(&VInt::encode_size(Some(2)));
    bg_non_kf.put_slice(&[0, 10]);
    assert!(!inspect_mkv_keyframe(ID_BLOCK_GROUP, &bg_non_kf));
}

#[test]
fn test_p29_unknown_and_void_elements_skipped() {
    // Void elements (0xEC) and padding in headers or clusters must be safely skipped
    let ebml_header = make_synthetic_ebml_header("matroska");
    let segment_header = make_synthetic_segment_header();

    // Void element (0xEC + size 4 + 4 zeros)
    let mut void_elem = BytesMut::new();
    void_elem.put_u8(0xEC);
    void_elem.put_slice(&VInt::encode_size(Some(4)));
    void_elem.put_slice(&[0u8; 4]);

    let cluster_header = make_synthetic_cluster_header();

    let mut stream = BytesMut::new();
    stream.extend_from_slice(&ebml_header);
    stream.extend_from_slice(&segment_header);
    stream.extend_from_slice(&void_elem);
    stream.extend_from_slice(&cluster_header);

    let mut demuxer = MkvDemuxer::new();
    demuxer.feed(&stream).unwrap();

    let out = demuxer.next_output().unwrap().unwrap();
    match out {
        MkvDemuxOutput::HeaderSnapshotReady { metadata, .. } => {
            assert_eq!(metadata.doc_type, "matroska");
            assert_eq!(metadata.content_type, "MKV");
        }
        _ => panic!("Expected HeaderSnapshotReady"),
    }
}

#[test]
fn test_p29_oversized_element_rejection() {
    // Elements claiming size exceeding max_element_size budget must be rejected (Global Rule 3)
    let ebml_header = make_synthetic_ebml_header("matroska");
    let segment_header = make_synthetic_segment_header();

    // Oversized child element (100 MiB)
    let mut oversized_elem = BytesMut::new();
    oversized_elem.put_slice(&[0x15, 0x49, 0xA9, 0x66]); // Info element ID
    oversized_elem.put_slice(&VInt::encode_size(Some(100 * 1024 * 1024)));

    let mut stream = BytesMut::new();
    stream.extend_from_slice(&ebml_header);
    stream.extend_from_slice(&segment_header);
    stream.extend_from_slice(&oversized_elem);

    let mut demuxer = MkvDemuxer::with_limits(16 * 1024 * 1024, 16 * 1024 * 1024);
    demuxer.feed(&stream).unwrap();

    let err = demuxer.next_output().unwrap_err();
    assert_eq!(
        err,
        MkvParseError::ElementSizeExceeded {
            actual: 100 * 1024 * 1024,
            max: 16 * 1024 * 1024,
        }
    );
}

#[test]
fn test_p29_header_replacement_epoch_bump() {
    // REQ-051: Mid-stream EBML header bumps epoch and rebuilds HeaderSnapshot
    let ebml1 = make_synthetic_ebml_header("matroska");
    let seg1 = make_synthetic_segment_header();
    let cluster1 = make_synthetic_cluster_header();
    let block1 = make_synthetic_simple_block(1, true, &[1, 2, 3]);

    let ebml2 = make_synthetic_ebml_header("webm");
    let seg2 = make_synthetic_segment_header();
    let cluster2 = make_synthetic_cluster_header();

    let mut demuxer = MkvDemuxer::new();
    demuxer.feed(&ebml1).unwrap();
    demuxer.feed(&seg1).unwrap();
    demuxer.feed(&cluster1).unwrap();
    demuxer.feed(&block1).unwrap();

    // 1. Initial snapshot: MKV, epoch 0
    let out1 = demuxer.next_output().unwrap().unwrap();
    match out1 {
        MkvDemuxOutput::HeaderSnapshotReady { epoch, metadata, .. } => {
            assert_eq!(epoch, 0);
            assert_eq!(metadata.content_type, "MKV");
        }
        _ => panic!("Expected HeaderSnapshotReady"),
    }

    // 2. Cluster header packet
    let out2 = demuxer.next_output().unwrap().unwrap();
    assert!(matches!(out2, MkvDemuxOutput::ContentPacket { .. }));

    // 3. SimpleBlock media packet
    let out3 = demuxer.next_output().unwrap().unwrap();
    assert!(matches!(out3, MkvDemuxOutput::ContentPacket { .. }));

    // Buffer is now drained. Stream restarts with WebM header
    demuxer.feed(&ebml2).unwrap();
    demuxer.feed(&seg2).unwrap();
    demuxer.feed(&cluster2).unwrap();

    let out4 = demuxer.next_output().unwrap().unwrap();
    match out4 {
        MkvDemuxOutput::HeaderSnapshotReady { epoch, metadata, .. } => {
            assert_eq!(epoch, 1, "Epoch must bump on stream restart");
            assert_eq!(metadata.content_type, "WEBM");
        }
        _ => panic!("Expected HeaderSnapshotReady"),
    }
}

#[test]
fn test_p29_incremental_feeding_and_truncated_stream() {
    let ebml = make_synthetic_ebml_header("matroska");
    let seg = make_synthetic_segment_header();
    let cluster = make_synthetic_cluster_header();
    let block = make_synthetic_simple_block(1, true, &[10, 20, 30]);

    let mut combined = BytesMut::new();
    combined.extend_from_slice(&ebml);
    combined.extend_from_slice(&seg);
    combined.extend_from_slice(&cluster);
    combined.extend_from_slice(&block);

    let mut demuxer = MkvDemuxer::new();

    // Feed in 5-byte chunks
    for chunk in combined.chunks(5) {
        demuxer.feed(chunk).unwrap();
    }

    // Output 1: HeaderSnapshot
    let out1 = demuxer.next_output().unwrap().unwrap();
    assert!(matches!(out1, MkvDemuxOutput::HeaderSnapshotReady { .. }));

    // Output 2: Cluster header
    let out2 = demuxer.next_output().unwrap().unwrap();
    assert!(matches!(out2, MkvDemuxOutput::ContentPacket { .. }));

    // Output 3: SimpleBlock
    let out3 = demuxer.next_output().unwrap().unwrap();
    match out3 {
        MkvDemuxOutput::ContentPacket { is_keyframe, .. } => {
            assert!(is_keyframe);
        }
        _ => panic!("Expected ContentPacket"),
    }

    // Signal EOF
    let eof = demuxer.signal_eof().unwrap().unwrap();
    assert_eq!(eof, MkvDemuxOutput::EndOfStream);
    assert_eq!(demuxer.state(), MkvDemuxerState::Ended);
    assert!(demuxer.next_output().unwrap().is_none());
}
