use bytes::{BufMut, Bytes, BytesMut};
use peercast_format::flv::{
    FlvDemuxOutput, FlvDemuxer, FlvParseError, FlvParserEvent, FlvParserPhase, FlvStreamParser,
    FlvTagType,
};
use std::fs;
use std::path::Path;

/// Helper: construct standard AMF0 `onMetaData` payload.
fn make_amf0_metadata_payload(
    videodatarate: f64,
    audiodatarate: f64,
    width: u32,
    height: u32,
) -> Bytes {
    let mut out = BytesMut::new();
    // AMF0 String: "onMetaData"
    out.put_u8(2); // String marker
    let name = b"onMetaData";
    out.put_u16(name.len() as u16);
    out.put_slice(name);

    // AMF0 ECMA Array (marker 8)
    out.put_u8(8);
    out.put_u32(4); // approx property count

    // "videodatarate" -> Number
    let k_v = b"videodatarate";
    out.put_u16(k_v.len() as u16);
    out.put_slice(k_v);
    out.put_u8(0);
    out.put_f64(videodatarate);

    // "audiodatarate" -> Number
    let k_a = b"audiodatarate";
    out.put_u16(k_a.len() as u16);
    out.put_slice(k_a);
    out.put_u8(0);
    out.put_f64(audiodatarate);

    // "width" -> Number
    let k_w = b"width";
    out.put_u16(k_w.len() as u16);
    out.put_slice(k_w);
    out.put_u8(0);
    out.put_f64(width as f64);

    // "height" -> Number
    let k_h = b"height";
    out.put_u16(k_h.len() as u16);
    out.put_slice(k_h);
    out.put_u8(0);
    out.put_f64(height as f64);

    // ObjectEnd: length 0 string + 0x09
    out.put_u16(0);
    out.put_u8(9);

    out.freeze()
}

/// Helper: construct a complete wire FLV tag (11 bytes header + body + 4 bytes previous tag size).
fn make_wire_tag(tag_type: FlvTagType, timestamp: u32, body: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(11 + body.len() + 4);
    out.put_u8(tag_type as u8);
    // 3 bytes data size
    out.put_u8(((body.len() >> 16) & 0xFF) as u8);
    out.put_u8(((body.len() >> 8) & 0xFF) as u8);
    out.put_u8((body.len() & 0xFF) as u8);
    // timestamp 3 bytes low + 1 byte high
    out.put_u8(((timestamp >> 16) & 0xFF) as u8);
    out.put_u8(((timestamp >> 8) & 0xFF) as u8);
    out.put_u8((timestamp & 0xFF) as u8);
    out.put_u8(((timestamp >> 24) & 0xFF) as u8);
    // stream ID (3 bytes)
    out.put_u8(0);
    out.put_u8(0);
    out.put_u8(0);
    // body
    out.put_slice(body);
    // PreviousTagSize (4 bytes BE)
    out.put_u32(11 + body.len() as u32);
    out.freeze()
}

/// Helper: construct 13-byte FLV file header prelude.
fn make_flv_prelude(has_audio: bool, has_video: bool, prev_tag_size_0: u32) -> Bytes {
    let mut out = BytesMut::with_capacity(13);
    out.put_slice(b"FLV");
    out.put_u8(1); // Version 1
    let flags = (if has_audio { 0x04 } else { 0 }) | (if has_video { 0x01 } else { 0 });
    out.put_u8(flags);
    out.put_u32(9); // DataOffset = 9
    out.put_u32(prev_tag_size_0); // PreviousTagSize0 (must be 0)
    out.freeze()
}

#[test]
fn test_p23_standard_flv_stream_and_header_reconstruction_ordering() {
    // 1. Build standard stream
    let prelude = make_flv_prelude(true, true, 0);
    let meta_body = make_amf0_metadata_payload(500.0, 128.0, 1280, 720);
    let meta_tag = make_wire_tag(FlvTagType::ScriptData, 0, &meta_body);

    // AAC sequence header: 0xAF (AAC stereo 44kHz), 0x00 (AAC sequence header), AudioSpecificConfig
    let aac_seq_body = vec![0xAF, 0x00, 0x12, 0x10];
    let aac_seq_tag = make_wire_tag(FlvTagType::Audio, 0, &aac_seq_body);

    // AVC sequence header: 0x17 (Keyframe AVC), 0x00 (AVC sequence header), CompositionTime 0x000000, AVCDecoderConfigurationRecord
    let avc_seq_body = vec![
        0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x1F, 0xFF, 0xE1, 0x00, 0x10,
    ];
    let avc_seq_tag = make_wire_tag(FlvTagType::Video, 0, &avc_seq_body);

    // Media video keyframe tag
    let video_frame_body = vec![0x17, 0x01, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x05, 0x65];
    let video_frame_tag = make_wire_tag(FlvTagType::Video, 100, &video_frame_body);

    // Media audio frame tag
    let audio_frame_body = vec![0xAF, 0x01, 0x21, 0x22, 0x23, 0x24];
    let audio_frame_tag = make_wire_tag(FlvTagType::Audio, 105, &audio_frame_body);

    let mut stream_bytes = BytesMut::new();
    stream_bytes.extend_from_slice(&prelude);
    stream_bytes.extend_from_slice(&meta_tag);
    stream_bytes.extend_from_slice(&aac_seq_tag);
    stream_bytes.extend_from_slice(&avc_seq_tag);
    stream_bytes.extend_from_slice(&video_frame_tag);
    stream_bytes.extend_from_slice(&audio_frame_tag);

    // 2. Demux stream
    let mut demuxer = FlvDemuxer::default();
    demuxer.feed(&stream_bytes);

    let mut outputs = Vec::new();
    while let Some(out) = demuxer.next_output().expect("demuxer should succeed") {
        outputs.push(out);
    }

    // Must have emitted HeaderChanged events during config gathering and then media tags
    let mut snapshots = Vec::new();
    let mut content_tags = Vec::new();
    for out in outputs {
        match out {
            FlvDemuxOutput::HeaderChanged(s) => snapshots.push(s),
            FlvDemuxOutput::ContentTag {
                epoch,
                tag,
                is_keyframe,
            } => content_tags.push((epoch, tag, is_keyframe)),
        }
    }

    assert!(
        !snapshots.is_empty(),
        "Must emit at least one HeaderChanged"
    );
    let final_snapshot = snapshots.last().unwrap();

    // 3. Verify HeaderSnapshot structure (REQ-051, REQ-052)
    assert_eq!(final_snapshot.container_type, "FLV");
    assert!(final_snapshot.data.len() >= 13);
    assert_eq!(&final_snapshot.data[..3], b"FLV");
    assert_eq!(final_snapshot.data[3], 1); // version 1
    assert_eq!(final_snapshot.data[4], 0x05); // audio (0x04) | video (0x01)
    assert_eq!(&final_snapshot.data[5..9], &[0, 0, 0, 9]); // DataOffset 9
    assert_eq!(&final_snapshot.data[9..13], &[0, 0, 0, 0]); // PreviousTagSize0 == 0!

    // Verify metadata was extracted
    let meta = demuxer.metadata().expect("Metadata should be parsed");
    assert_eq!(meta.videodatarate, Some(500.0));
    assert_eq!(meta.audiodatarate, Some(128.0));
    assert_eq!(meta.bitrate_kbps, Some(628)); // 500 + 128
    assert_eq!(meta.width, Some(1280));
    assert_eq!(meta.height, Some(720));

    // 4. Verify reconstructed HeaderSnapshot bytes parse cleanly in an independent reader
    let mut verifier = FlvStreamParser::default();
    verifier.feed(&final_snapshot.data);

    let mut verified_tags = Vec::new();
    let file_hdr = match verifier
        .next_event()
        .expect("Reconstructed snapshot must parse")
    {
        Some(FlvParserEvent::FileHeader(h)) => h,
        other => panic!("Expected FileHeader, got {:?}", other),
    };
    assert!(file_hdr.has_audio);
    assert!(file_hdr.has_video);

    while let Some(ev) = verifier.next_event().expect("Tags must parse cleanly") {
        if let FlvParserEvent::Tag(t) = ev {
            assert!(t.is_valid_footer());
            verified_tags.push(t);
        }
    }

    // Must have 3 header tags: Metadata, AAC Sequence Header, AVC Sequence Header
    assert_eq!(verified_tags.len(), 3);
    assert_eq!(verified_tags[0].header.tag_type, FlvTagType::ScriptData);
    assert_eq!(verified_tags[1].header.tag_type, FlvTagType::Audio);
    assert!(verified_tags[1].is_aac_sequence_header());
    assert_eq!(verified_tags[2].header.tag_type, FlvTagType::Video);
    assert!(verified_tags[2].is_avc_sequence_header());

    // Sequence headers in the reconstructed snapshot must have timestamp forced to 0
    assert_eq!(verified_tags[0].header.timestamp, 0);
    assert_eq!(verified_tags[1].header.timestamp, 0);
    assert_eq!(verified_tags[2].header.timestamp, 0);

    // 5. Verify ContentTags
    assert_eq!(content_tags.len(), 2);
    // First is video keyframe
    assert!(content_tags[0].2); // is_keyframe == true
    assert_eq!(content_tags[0].1.header.tag_type, FlvTagType::Video);
    // Second is audio frame
    assert!(!content_tags[1].2);
    assert_eq!(content_tags[1].1.header.tag_type, FlvTagType::Audio);
}

#[test]
fn test_p23_audio_only_stream() {
    let prelude = make_flv_prelude(true, false, 0);
    let meta_body = make_amf0_metadata_payload(0.0, 192.0, 0, 0);
    let meta_tag = make_wire_tag(FlvTagType::ScriptData, 0, &meta_body);

    let aac_seq_body = vec![0xAF, 0x00, 0x11, 0x90];
    let aac_seq_tag = make_wire_tag(FlvTagType::Audio, 0, &aac_seq_body);

    let audio_frame_body = vec![0xAF, 0x01, 0xAA, 0xBB, 0xCC];
    let audio_frame_tag = make_wire_tag(FlvTagType::Audio, 50, &audio_frame_body);

    let mut stream = BytesMut::new();
    stream.extend_from_slice(&prelude);
    stream.extend_from_slice(&meta_tag);
    stream.extend_from_slice(&aac_seq_tag);
    stream.extend_from_slice(&audio_frame_tag);

    let mut demuxer = FlvDemuxer::default();
    demuxer.feed(&stream);

    let mut last_snapshot = None;
    let mut media_tags = Vec::new();
    while let Some(out) = demuxer.next_output().unwrap() {
        match out {
            FlvDemuxOutput::HeaderChanged(s) => last_snapshot = Some(s),
            FlvDemuxOutput::ContentTag { tag, .. } => media_tags.push(tag),
        }
    }

    let snapshot = last_snapshot.expect("Header snapshot must be emitted");
    assert_eq!(snapshot.data[4], 0x04); // audio only flag

    // Verify snapshot does NOT contain AVC video sequence header
    let mut parser = FlvStreamParser::default();
    parser.feed(&snapshot.data);

    let mut parsed_tags = Vec::new();
    while let Some(ev) = parser.next_event().unwrap() {
        if let FlvParserEvent::Tag(t) = ev {
            parsed_tags.push(t);
        }
    }
    assert_eq!(parsed_tags.len(), 2);
    assert_eq!(parsed_tags[0].header.tag_type, FlvTagType::ScriptData);
    assert_eq!(parsed_tags[1].header.tag_type, FlvTagType::Audio);
    assert!(parsed_tags[1].is_aac_sequence_header());

    assert_eq!(media_tags.len(), 1);
    assert_eq!(media_tags[0].header.tag_type, FlvTagType::Audio);
}

#[test]
fn test_p23_changed_config_epoch_invalidation() {
    let prelude = make_flv_prelude(true, true, 0);

    // Initial AVC sequence header (e.g. 720p)
    let avc_seq_1 = vec![0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x1F];
    let avc_seq_tag_1 = make_wire_tag(FlvTagType::Video, 0, &avc_seq_1);

    // Video frame on initial config
    let frame_1 = vec![0x17, 0x01, 0x00, 0x00, 0x00, 0xAA];
    let frame_tag_1 = make_wire_tag(FlvTagType::Video, 100, &frame_1);

    // Changed AVC sequence header (e.g. dynamic resolution switch to 1080p)
    let avc_seq_2 = vec![
        0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x28, 0xFF, 0xE1,
    ];
    let avc_seq_tag_2 = make_wire_tag(FlvTagType::Video, 200, &avc_seq_2);

    // Video frame on new config
    let frame_2 = vec![0x17, 0x01, 0x00, 0x00, 0x00, 0xBB];
    let frame_tag_2 = make_wire_tag(FlvTagType::Video, 300, &frame_2);

    let mut demuxer = FlvDemuxer::default();

    // Feed initial header and frame
    demuxer.feed(&prelude);
    demuxer.feed(&avc_seq_tag_1);
    demuxer.feed(&frame_tag_1);

    let ev1 = demuxer.next_output().unwrap().unwrap();
    let initial_epoch = match ev1 {
        FlvDemuxOutput::HeaderChanged(s) => s.epoch,
        _ => panic!("Expected HeaderChanged"),
    };

    let ev2 = demuxer.next_output().unwrap().unwrap();
    match ev2 {
        FlvDemuxOutput::ContentTag { epoch, .. } => assert_eq!(epoch, initial_epoch),
        _ => panic!("Expected ContentTag"),
    }

    // Feed new sequence header and subsequent frame
    demuxer.feed(&avc_seq_tag_2);
    demuxer.feed(&frame_tag_2);

    let ev3 = demuxer.next_output().unwrap().unwrap();
    let updated_epoch = match ev3 {
        FlvDemuxOutput::HeaderChanged(s) => {
            assert!(
                s.epoch > initial_epoch,
                "Epoch must increment on config change (REQ-051)"
            );
            // Verify new snapshot contains avc_seq_2 bytes
            assert!(s
                .data
                .windows(avc_seq_2.len())
                .any(|w| w == avc_seq_2.as_slice()));
            s.epoch
        }
        _ => panic!("Expected HeaderChanged for changed sequence header"),
    };

    let ev4 = demuxer.next_output().unwrap().unwrap();
    match ev4 {
        FlvDemuxOutput::ContentTag { epoch, .. } => {
            assert_eq!(
                epoch, updated_epoch,
                "Subsequent tag must use updated epoch"
            );
        }
        _ => panic!("Expected ContentTag"),
    }
}

#[test]
fn test_p23_truncated_tag_fragmentation_resilience() {
    let prelude = make_flv_prelude(false, true, 0);
    let frame_body = vec![0x17, 0x01, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03];
    let wire_tag = make_wire_tag(FlvTagType::Video, 1000, &frame_body);

    let mut full_bytes = Vec::new();
    full_bytes.extend_from_slice(&prelude);
    full_bytes.extend_from_slice(&wire_tag);

    // Feed single byte at a time through FlvStreamParser
    let mut parser = FlvStreamParser::default();
    let mut file_header_found = false;
    let mut tag_found = false;

    for &b in &full_bytes {
        parser.feed(&[b]);
        while let Some(event) = parser.next_event().unwrap() {
            match event {
                FlvParserEvent::FileHeader(h) => {
                    assert!(h.has_video);
                    file_header_found = true;
                }
                FlvParserEvent::Tag(t) => {
                    assert_eq!(t.header.timestamp, 1000);
                    assert_eq!(t.body.as_ref(), frame_body.as_slice());
                    assert!(t.is_valid_footer());
                    tag_found = true;
                }
            }
        }
    }

    assert!(file_header_found);
    assert!(tag_found);
}

#[test]
fn test_p23_oversized_tag_rejection() {
    let prelude = make_flv_prelude(false, true, 0);
    let mut parser = FlvStreamParser::new(1024); // 1 KiB maximum budget
    parser.feed(&prelude);

    let _ = parser.next_event().unwrap(); // consumes header

    // Tag header declaring 5000 bytes data size (> 1024 limit)
    let mut oversized_hdr = BytesMut::new();
    oversized_hdr.put_u8(9); // Video
    oversized_hdr.put_u8(0);
    oversized_hdr.put_u8(0x13);
    oversized_hdr.put_u8(0x88); // 5000 bytes
    oversized_hdr.put_u32(0); // timestamp
    oversized_hdr.put_u8(0);
    oversized_hdr.put_u8(0);
    oversized_hdr.put_u8(0); // stream id

    parser.feed(&oversized_hdr);
    let res = parser.next_event();
    assert!(
        matches!(res, Err(FlvParseError::TagTooLarge { actual: 5000, max: 1024 })),
        "Oversized tag must be rejected with TagTooLarge error"
    );

    // Re-entrancy guard test: subsequent call must fail with FailedState
    let res2 = parser.next_event();
    assert_eq!(res2, Err(FlvParseError::FailedState));
}

#[test]
fn test_p23_invalid_previoustagsize_footer() {
    let prelude = make_flv_prelude(false, true, 0);
    let mut parser = FlvStreamParser::default();
    parser.feed(&prelude);
    let _ = parser.next_event().unwrap();

    let body = vec![1, 2, 3, 4];
    let mut tag_bytes = BytesMut::new();
    tag_bytes.put_u8(9); // Video
    tag_bytes.put_u8(0);
    tag_bytes.put_u8(0);
    tag_bytes.put_u8(4); // 4 bytes body
    tag_bytes.put_u32(0); // timestamp
    tag_bytes.put_u8(0);
    tag_bytes.put_u8(0);
    tag_bytes.put_u8(0); // stream id
    tag_bytes.put_slice(&body);
    tag_bytes.put_u32(999); // Corrupted footer: expected 15 (4 + 11)

    parser.feed(&tag_bytes);
    let res = parser.next_event();
    assert_eq!(
        res,
        Err(FlvParseError::InvalidPreviousTagSize {
            expected: 15,
            actual: 999
        })
    );
    assert_eq!(parser.phase(), FlvParserPhase::Failed);
}

#[test]
fn test_p23_previoustagsize0_nonzero_rejected() {
    let prelude = make_flv_prelude(true, true, 42); // PreviousTagSize0 = 42 (must be 0)
    let mut parser = FlvStreamParser::default();
    parser.feed(&prelude);
    let res = parser.next_event();
    assert_eq!(res, Err(FlvParseError::InvalidPreviousTagSize0(42)));
}

#[test]
fn test_p23_stop_block_extended_tags_rejected_not_silently_accepted() {
    let prelude = make_flv_prelude(true, true, 0);

    // ExVideo tag has bit 7 set in body[0]
    let ex_video_body = vec![0x80, 0x01, 0x00];
    let ex_video_tag = make_wire_tag(FlvTagType::Video, 0, &ex_video_body);

    let mut demuxer = FlvDemuxer::default();
    demuxer.feed(&prelude);
    demuxer.feed(&ex_video_tag);

    let res = demuxer.next_output();
    assert!(
        matches!(res, Err(FlvParseError::UnsupportedExtendedCodec(_))),
        "Extended video tag must be rejected, never silently treated as standard codec"
    );

    // ExAudio tag has format 9 in body[0]
    let ex_audio_body = vec![0x90, 0x01, 0x00];
    let ex_audio_tag = make_wire_tag(FlvTagType::Audio, 0, &ex_audio_body);

    let mut demuxer2 = FlvDemuxer::default();
    demuxer2.feed(&prelude);
    demuxer2.feed(&ex_audio_tag);

    let res2 = demuxer2.next_output();
    assert!(
        matches!(res2, Err(FlvParseError::UnsupportedExtendedCodec(_))),
        "Extended audio tag must be rejected, never silently treated as standard codec"
    );
}

#[test]
fn test_p23_upstream_test_flv_real_file_parse() {
    let path = Path::new(r"C:\Github\_upstream_peercaststation\PeerCastStation\TestFLV\fixtures\test.flv");
    if !path.exists() {
        eprintln!("Skipping test_flv real file test: fixture not found at {:?}", path);
        return;
    }

    let file_bytes = fs::read(path).expect("Failed to read test.flv fixture");
    assert!(file_bytes.len() > 10000);

    let mut demuxer = FlvDemuxer::default();
    demuxer.feed(&file_bytes);

    let mut parser = FlvStreamParser::default();
    parser.feed(&file_bytes);
    let mut first_video_printed = false;
    let mut first_audio_printed = false;
    while let Some(ev) = parser.next_event().unwrap() {
        if let FlvParserEvent::Tag(t) = ev {
            if t.header.tag_type == FlvTagType::Video && !first_video_printed {
                first_video_printed = true;
                eprintln!("test.flv first video tag: body[0]=0x{:02x}, body[1]=0x{:02x}, len={}", t.body[0], t.body.get(1).copied().unwrap_or(0), t.body.len());
            }
            if t.header.tag_type == FlvTagType::Audio && !first_audio_printed {
                first_audio_printed = true;
                eprintln!("test.flv first audio tag: body[0]=0x{:02x}, body[1]=0x{:02x}, len={}", t.body[0], t.body.get(1).copied().unwrap_or(0), t.body.len());
            }
        }
    }

    let mut snapshot_count = 0;
    let mut total_content_tags = 0;
    let mut last_snapshot = None;

    while let Some(out) = demuxer.next_output().expect("Parsing test.flv should succeed") {
        match out {
            FlvDemuxOutput::HeaderChanged(s) => {
                snapshot_count += 1;
                last_snapshot = Some(s);
            }
            FlvDemuxOutput::ContentTag { tag, .. } => {
                total_content_tags += 1;
                assert!(tag.is_valid_footer());
            }
        }
    }

    assert!(snapshot_count >= 1, "Must generate at least one header snapshot");
    assert!(total_content_tags > 50, "test.flv has many media tags, got {}", total_content_tags);

    let meta = demuxer.metadata().expect("test.flv must have metadata");
    assert!(meta.videodatarate.is_some() || meta.audiodatarate.is_some() || meta.bitrate_kbps.is_some());

    // Verify final snapshot parses cleanly in a fresh parser
    let final_snapshot = last_snapshot.unwrap();
    assert_eq!(&final_snapshot.data[..3], b"FLV");
    assert_eq!(final_snapshot.data[3], 1);
    assert_eq!(&final_snapshot.data[9..13], &[0, 0, 0, 0]); // PreviousTagSize0

    let mut verifier = FlvStreamParser::default();
    verifier.feed(&final_snapshot.data);
    let mut verified_tags = 0;
    while let Some(ev) = verifier.next_event().expect("Snapshot from test.flv must verify") {
        if let FlvParserEvent::Tag(t) = ev {
            eprintln!("test.flv snapshot tag: type={:?}, len={}", t.header.tag_type, t.body.len());
            assert!(t.is_valid_footer());
            verified_tags += 1;
        }
    }
    eprintln!("Total verified tags in snapshot: {}", verified_tags);
    assert!(verified_tags >= 1, "Snapshot should contain at least metadata or sequence headers");
}
