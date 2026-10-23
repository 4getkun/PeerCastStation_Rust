use bytes::{BufMut, Bytes, BytesMut};
use peercast_format::asf::{
    parse_asf_top_level_header, AsfDemuxOutput, AsfDemuxer, AsfDemuxerState, AsfGuid,
    AsfParseError, AsfTransportProfile, ASF_STREAM_BITRATE_GUID,
    ASF_STREAM_PROPERTIES_GUID, ASF_STREAM_TYPE_AUDIO_GUID,
    ASF_TOP_LEVEL_HEADER_GUID,
};
use std::fs;
use std::path::Path;

/// Helper to build a synthetic generic ASF object header (24 bytes).
fn make_generic_asf_header(guid: AsfGuid, size: u64) -> BytesMut {
    let mut b = BytesMut::with_capacity(24);
    b.put_slice(guid.as_bytes());
    b.put_u64_le(size);
    b
}

/// Helper to build a synthetic minimal Top-Level Header (30 bytes, 0 child objects).
fn make_minimal_top_level_header() -> Bytes {
    let mut b = make_generic_asf_header(ASF_TOP_LEVEL_HEADER_GUID, 30);
    b.put_u32_le(0); // child count = 0
    b.put_u8(1); // reserved 1
    b.put_u8(2); // reserved 2
    b.freeze()
}

/// Helper to build a synthetic WMSP frame with 4-byte header and optional MMS data packet.
fn make_wmsp_frame(subtype: u8, mms_payload: &[u8]) -> Bytes {
    let mut b = BytesMut::new();
    let length = if mms_payload.is_empty() {
        0u16
    } else {
        (8 + mms_payload.len()) as u16
    };

    // 4-byte WMSP header: '$', subtype, length (LE)
    b.put_u8(b'$');
    b.put_u8(subtype);
    b.put_u16_le(length);

    if !mms_payload.is_empty() {
        // MMS Data Packet header: location_id (4), incarnation (1), af_flags (1), packet_len (2)
        b.put_u32_le(0); // location_id
        b.put_u8(0); // incarnation
        b.put_u8(0x0C); // af_flags
        b.put_u16_le(length); // packet_len
        b.put_slice(mms_payload);
    }

    b.freeze()
}

#[test]
fn test_p28_upstream_test_asf_real_fixture() {
    let fixture_path = Path::new(r"C:\Github\_upstream_peercaststation\PeerCastStation\TestASF\fixtures\test.asf");
    if !fixture_path.exists() {
        eprintln!("Skipping test_p28_upstream_test_asf_real_fixture: fixture not found at {:?}", fixture_path);
        return;
    }

    let file_bytes = fs::read(fixture_path).expect("Failed to read test.asf fixture");
    assert!(file_bytes.len() > 1_000_000, "test.asf should be ~1.96 MB, got {}", file_bytes.len());

    let mut demuxer = AsfDemuxer::new(AsfTransportProfile::MmsFramed);
    demuxer.feed(&file_bytes).expect("Feeding test.asf bytes must succeed");

    // 1. Initial output must be HeaderSnapshotReady
    let first_out = demuxer.next_output().expect("Parsing test.asf header must succeed");
    let (snapshot, metadata) = match first_out {
        Some(AsfDemuxOutput::HeaderSnapshotReady { snapshot, metadata, epoch }) => {
            assert_eq!(epoch, 0);
            (snapshot, metadata)
        }
        other => panic!("Expected HeaderSnapshotReady, got {:?}", other),
    };

    // Upstream parity check with Station's `test_asfcontentreader.rb`:
    // `assert_equal(5271, content.content_header.data.length)`
    assert_eq!(snapshot.data.len(), 5271, "Header snapshot length must match Station's 5271 bytes");
    assert_eq!(snapshot.pos, 0);
    assert_eq!(snapshot.container_type, "WMV");

    // Metadata checks
    assert_eq!(metadata.content_type, "WMV");
    assert_eq!(metadata.mime_type, "video/x-ms-wmv");
    assert_eq!(metadata.stream_ext, ".wmv");
    assert!(
        metadata.bitrate_kbps == 438 || metadata.bitrate_kbps == 439,
        "Bitrate must match Station's 438/439 kbps, got {}",
        metadata.bitrate_kbps
    );
    assert!(metadata.has_video);
    assert!(metadata.has_audio);

    // File properties check
    let fp = metadata.file_properties.as_ref().expect("FileProperties must be present");
    assert_eq!(fp.min_packet_size, fp.max_packet_size);

    // Stream bitrate records: Stream 1 (Audio: 33,742 bps) and Stream 2 (Video: 404,368 bps)
    assert_eq!(metadata.bitrate_records.len(), 2);
    let total_br: u32 = metadata.bitrate_records.iter().map(|r| r.average_bitrate_bps).sum();
    assert_eq!(total_br, 438_110);
    assert_eq!(total_br / 1000, 438);

    // 2. Iterate through media packets and count them
    // Upstream Ruby test verified: initial chunk has 7 packets, total file has 361 content packets!
    let mut total_packets = 0;
    let mut expected_pos = snapshot.data.len() as u64;

    while let Some(out) = demuxer.next_output().expect("Parsing test.asf content should succeed") {
        match out {
            AsfDemuxOutput::ContentPacket { packet, epoch, position } => {
                assert_eq!(epoch, 0);
                assert_eq!(position, expected_pos, "Stream position must advance contiguously");
                expected_pos += packet.len() as u64;
                total_packets += 1;
            }
            AsfDemuxOutput::EndOfStream => {
                break;
            }
            other => panic!("Unexpected output during stream: {:?}", other),
        }
    }

    assert_eq!(
        total_packets, 361,
        "Total content packets in test.asf must match Station's 361, got {}",
        total_packets
    );
    assert_eq!(demuxer.stream_position(), expected_pos);
}

#[test]
fn test_p28_stop_block_no_assumed_50_byte_minimum() {
    // REQ-054 STOP/BLOCK GUARD:
    // "Do not use a guessed '50-byte minimum.' Distinguish ASF objects from MMS-framed transport."
    // An implementation that guesses a "50-byte minimum" would erroneously reject:
    // 1. Generic ASF objects of 24 bytes.
    // 2. Top-level headers of 30 bytes (24 bytes generic + 6 bytes prelude).
    // 3. Child objects smaller than 50 bytes (e.g. StreamBitrateProperties with 1 stream is 24 + 2 + 6 = 32 bytes!).

    // Test 1: 30-byte minimal top-level header is parsed successfully without 50-byte rejection
    let minimal_header = make_minimal_top_level_header();
    assert_eq!(minimal_header.len(), 30);
    assert!(minimal_header.len() < 50, "Header is strictly under 50 bytes");

    let meta = parse_asf_top_level_header(&minimal_header, 1024 * 1024)
        .expect("30-byte Top-Level Header must NOT be rejected by a false 50-byte minimum!");
    assert_eq!(meta.content_type, "ASF");
    assert_eq!(meta.mime_type, "video/x-ms-asf");
    assert_eq!(meta.stream_ext, ".asf");
    assert_eq!(meta.bitrate_kbps, 0);

    // Test 2: Child object of 38 bytes (StreamBitrateProperties with 2 streams: 24 + 2 + 12 = 38 bytes)
    // 38 bytes is strictly under 50 bytes.
    let mut top_level = BytesMut::new();
    let total_size = 30 + 38;
    top_level.put_slice(ASF_TOP_LEVEL_HEADER_GUID.as_bytes());
    top_level.put_u64_le(total_size as u64);
    top_level.put_u32_le(1); // 1 child
    top_level.put_u8(1);
    top_level.put_u8(2);

    // Child: Stream Bitrate Properties Object (38 bytes)
    top_level.put_slice(ASF_STREAM_BITRATE_GUID.as_bytes());
    top_level.put_u64_le(38);
    top_level.put_u16_le(2); // 2 records
    // Record 1: stream 1, 128,000 bps
    top_level.put_u16_le(1);
    top_level.put_u32_le(128_000);
    // Record 2: stream 2, 384,000 bps
    top_level.put_u16_le(2);
    top_level.put_u32_le(384_000);

    assert_eq!(top_level.len(), total_size);
    let meta2 = parse_asf_top_level_header(&top_level, 1024 * 1024)
        .expect("Child object under 50 bytes must parse successfully");
    assert_eq!(meta2.bitrate_kbps, 512); // (128000 + 384000) / 1000 = 512 kbps
}

#[test]
fn test_p28_malformed_object_size_rejection() {
    // 1. Generic object size < 24 rejected
    let mut bad_size_under_24 = BytesMut::new();
    bad_size_under_24.put_slice(ASF_TOP_LEVEL_HEADER_GUID.as_bytes());
    bad_size_under_24.put_u64_le(23); // invalid: < 24
    let err = parse_asf_top_level_header(&bad_size_under_24, 1024 * 1024).unwrap_err();
    assert_eq!(err, AsfParseError::InvalidObjectSize(23));

    // 2. Object size exceeding max allowed budget rejected
    let mut oversized = BytesMut::new();
    oversized.put_slice(ASF_TOP_LEVEL_HEADER_GUID.as_bytes());
    oversized.put_u64_le(100 * 1024 * 1024); // 100 MiB
    let err2 = parse_asf_top_level_header(&oversized, 16 * 1024 * 1024).unwrap_err();
    assert_eq!(
        err2,
        AsfParseError::ObjectSizeExceeded {
            actual: 100 * 1024 * 1024,
            max: 16 * 1024 * 1024,
        }
    );

    // 3. Top-level header with size between 24 and 29 (missing 6-byte prelude) rejected
    let mut truncated_prelude = BytesMut::new();
    truncated_prelude.put_slice(ASF_TOP_LEVEL_HEADER_GUID.as_bytes());
    truncated_prelude.put_u64_le(28);
    let err3 = parse_asf_top_level_header(&truncated_prelude, 1024 * 1024).unwrap_err();
    assert_eq!(err3, AsfParseError::InvalidTopLevelHeaderSize(28));
}

#[test]
fn test_p28_mms_versus_raw_asf_distinction() {
    // REQ-054: "Distinguish ASF objects from MMS-framed transport. The audited Station reader
    // forwards complete MMS header frames; a raw ASF-only implementation is not automatically equivalent."

    let raw_header = make_minimal_top_level_header();

    // 1. Feeding raw un-framed ASF bytes into an MMS-configured demuxer must reject invalid WMSP frame header
    let mut mms_demuxer = AsfDemuxer::new(AsfTransportProfile::MmsFramed);
    mms_demuxer.feed(&raw_header).expect("Feed succeeds");
    let err = mms_demuxer.next_output().unwrap_err();
    assert_eq!(err, AsfParseError::InvalidWmspFrameHeader);

    // 2. Feeding raw ASF into a RawAsf-configured demuxer succeeds
    let mut raw_demuxer = AsfDemuxer::new(AsfTransportProfile::RawAsf);
    raw_demuxer.feed(&raw_header).expect("Feed succeeds");
    let out = raw_demuxer.next_output().expect("Parse succeeds");
    match out {
        Some(AsfDemuxOutput::HeaderSnapshotReady { snapshot, .. }) => {
            // Snapshot contains exact raw ASF header bytes (not wrapped in WMSP)
            assert_eq!(snapshot.data, raw_header);
        }
        other => panic!("Expected HeaderSnapshotReady, got {:?}", other),
    }

    // 3. MMS transport wraps the header in a WMSP "$H" frame and forwards complete MMS frame
    let wmsp_header = make_wmsp_frame(b'H', &raw_header);
    let mut mms_demuxer2 = AsfDemuxer::new(AsfTransportProfile::MmsFramed);
    mms_demuxer2.feed(&wmsp_header).expect("Feed succeeds");
    let out2 = mms_demuxer2.next_output().expect("Parse succeeds");
    match out2 {
        Some(AsfDemuxOutput::HeaderSnapshotReady { snapshot, .. }) => {
            // Snapshot contains the COMPLETE MMS WMSP frame, NOT stripped raw ASF (REQ-054)
            assert_eq!(snapshot.data, wmsp_header);
        }
        other => panic!("Expected HeaderSnapshotReady, got {:?}", other),
    }
}

#[test]
fn test_p28_header_replacement_epoch_bump() {
    // REQ-051: Header replacement mid-stream bumps epoch and rebuilds header snapshot.
    let raw_header1 = make_minimal_top_level_header();
    let wmsp_header1 = make_wmsp_frame(b'H', &raw_header1);

    let raw_header2 = make_minimal_top_level_header();
    let wmsp_header2 = make_wmsp_frame(b'H', &raw_header2);

    let mut demuxer = AsfDemuxer::new(AsfTransportProfile::MmsFramed);
    demuxer.feed(&wmsp_header1).unwrap();

    let out1 = demuxer.next_output().unwrap().unwrap();
    match out1 {
        AsfDemuxOutput::HeaderSnapshotReady { epoch, .. } => assert_eq!(epoch, 0),
        _ => panic!("Expected HeaderSnapshotReady"),
    }

    // Send a data packet
    let data_packet = make_wmsp_frame(b'D', &[0xAA; 32]);
    demuxer.feed(&data_packet).unwrap();
    let out2 = demuxer.next_output().unwrap().unwrap();
    match out2 {
        AsfDemuxOutput::ContentPacket { epoch, .. } => assert_eq!(epoch, 0),
        _ => panic!("Expected ContentPacket"),
    }

    // Replace header mid-stream
    demuxer.feed(&wmsp_header2).unwrap();
    let out3 = demuxer.next_output().unwrap().unwrap();
    match out3 {
        AsfDemuxOutput::HeaderSnapshotReady { epoch, .. } => assert_eq!(epoch, 1, "Epoch must increment on replacement"),
        _ => panic!("Expected HeaderSnapshotReady"),
    }
    assert_eq!(demuxer.current_epoch(), 1);
}

#[test]
fn test_p28_truncated_frame_and_incremental_feeding() {
    // Feeding stream 1 byte at a time or in arbitrary small chunks must not corrupt framing
    let raw_header = make_minimal_top_level_header();
    let wmsp_header = make_wmsp_frame(b'H', &raw_header);
    let data_frame = make_wmsp_frame(b'D', &[1, 2, 3, 4, 5, 6, 7, 8]);

    let mut combined = BytesMut::new();
    combined.extend_from_slice(&wmsp_header);
    combined.extend_from_slice(&data_frame);

    let mut demuxer = AsfDemuxer::new(AsfTransportProfile::MmsFramed);

    // Feed in 3-byte chunks
    for chunk in combined.chunks(3) {
        demuxer.feed(chunk).unwrap();
    }

    // First output: HeaderSnapshot
    let out1 = demuxer.next_output().unwrap().unwrap();
    assert!(matches!(out1, AsfDemuxOutput::HeaderSnapshotReady { .. }));

    // Second output: ContentPacket
    let out2 = demuxer.next_output().unwrap().unwrap();
    assert!(matches!(out2, AsfDemuxOutput::ContentPacket { .. }));

    // No more output
    let out3 = demuxer.next_output().unwrap();
    assert!(out3.is_none());
}

#[test]
fn test_p28_clean_eof_and_end_frame() {
    // Test '$E' End of stream frame
    let raw_header = make_minimal_top_level_header();
    let wmsp_header = make_wmsp_frame(b'H', &raw_header);
    let end_frame = make_wmsp_frame(b'E', &[]);

    let mut demuxer = AsfDemuxer::new(AsfTransportProfile::MmsFramed);
    demuxer.feed(&wmsp_header).unwrap();
    demuxer.feed(&end_frame).unwrap();

    let out1 = demuxer.next_output().unwrap().unwrap();
    assert!(matches!(out1, AsfDemuxOutput::HeaderSnapshotReady { .. }));

    let out2 = demuxer.next_output().unwrap().unwrap();
    assert_eq!(out2, AsfDemuxOutput::EndOfStream);
    assert_eq!(demuxer.state(), AsfDemuxerState::Ended);

    // Re-entrancy: subsequent calls return None
    assert!(demuxer.next_output().unwrap().is_none());
}

#[test]
fn test_p28_audio_only_wma_classification() {
    // Build Top-Level Header containing ONLY an audio stream property object
    let mut top_level = BytesMut::new();
    let child_size = 24 + 52; // generic 24 + stream properties payload 52 = 76 bytes
    let total_size = 30 + child_size; // 30 + 76 = 106 bytes

    top_level.put_slice(ASF_TOP_LEVEL_HEADER_GUID.as_bytes());
    top_level.put_u64_le(total_size as u64);
    top_level.put_u32_le(1); // 1 child
    top_level.put_u8(1);
    top_level.put_u8(2);

    // Child: Stream Properties Object
    top_level.put_slice(ASF_STREAM_PROPERTIES_GUID.as_bytes());
    top_level.put_u64_le(child_size as u64);
    // Stream type: Audio
    top_level.put_slice(ASF_STREAM_TYPE_AUDIO_GUID.as_bytes());
    // Error correction type: all zero GUID
    top_level.put_slice(&[0u8; 16]);
    top_level.put_u64_le(0); // time offset
    top_level.put_u32_le(0); // type specific data len
    top_level.put_u32_le(0); // error corr data len
    top_level.put_u16_le(1); // flags (stream number 1)
    top_level.put_u16_le(0); // reserved

    assert_eq!(top_level.len(), total_size);

    let meta = parse_asf_top_level_header(&top_level, 1024 * 1024).expect("Parsing audio-only ASF header");
    assert_eq!(meta.content_type, "WMA");
    assert_eq!(meta.mime_type, "audio/x-ms-wma");
    assert_eq!(meta.stream_ext, ".wma");
    assert!(!meta.has_video);
    assert!(meta.has_audio);
}
