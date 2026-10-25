use bytes::{BufMut, Bytes, BytesMut};
use peercast_format::mp3::{Mp3DemuxOutput, Mp3Demuxer};
use peercast_format::ogg::{
    compute_ogg_crc, OggCodec, OggDemuxOutput, OggDemuxer, OggPageHeader, OggParseError,
    OGG_CAPTURE_PATTERN, OGG_FLAG_BOS, OGG_FLAG_EOS,
};

/// Helper to build a valid Ogg page with exact RFC 3533 CRC-32 checksum.
fn make_ogg_page(
    header_type: u8,
    granule_pos: u64,
    serial_no: u32,
    seq_no: u32,
    packets: &[&[u8]],
) -> Bytes {
    let mut segment_table = Vec::new();
    let mut body = BytesMut::new();

    for &pkt in packets {
        let mut remaining = pkt.len();
        let mut offset = 0;
        while remaining >= 255 {
            segment_table.push(255u8);
            body.put_slice(&pkt[offset..offset + 255]);
            offset += 255;
            remaining -= 255;
        }
        segment_table.push(remaining as u8);
        body.put_slice(&pkt[offset..offset + remaining]);
    }

    assert!(
        segment_table.len() <= 255,
        "Ogg page cannot have more than 255 segments"
    );

    let mut header = BytesMut::new();
    header.put_slice(&OGG_CAPTURE_PATTERN); // 'OggS'
    header.put_u8(0); // Version 0
    header.put_u8(header_type);
    header.put_u64_le(granule_pos);
    header.put_u32_le(serial_no);
    header.put_u32_le(seq_no);
    header.put_u32_le(0); // Checksum placeholder
    header.put_u8(segment_table.len() as u8);
    header.put_slice(&segment_table);

    // Compute RFC 3533 CRC-32 over header + body with zero checksum field
    let mut page_data = BytesMut::new();
    page_data.extend_from_slice(&header);
    page_data.extend_from_slice(&body);

    let crc = compute_ogg_crc(&page_data);
    // Write computed CRC into bytes 22..26 (little-endian)
    page_data[22..26].copy_from_slice(&crc.to_le_bytes());

    page_data.freeze()
}

/// Helper: construct a standard Vorbis Identification packet (RFC 5215).
fn make_vorbis_id_packet(channels: u8, sample_rate: u32, bitrate_nominal: u32) -> Bytes {
    let mut b = BytesMut::new();
    b.put_u8(0x01); // Header type 1: Identification
    b.put_slice(b"vorbis");
    b.put_u32_le(0); // Vorbis version 0
    b.put_u8(channels);
    b.put_u32_le(sample_rate);
    b.put_u32_le(0); // Bitrate maximum
    b.put_u32_le(bitrate_nominal);
    b.put_u32_le(0); // Bitrate minimum
    b.put_u8(0xB8); // Blocksize 0 & 1
    b.put_u8(0x01); // Framing flag
    b.freeze()
}

/// Helper: construct a standard Vorbis Comment packet.
fn make_vorbis_comment_packet(vendor: &str) -> Bytes {
    let mut b = BytesMut::new();
    b.put_u8(0x03); // Header type 3: Comment
    b.put_slice(b"vorbis");
    b.put_u32_le(vendor.len() as u32);
    b.put_slice(vendor.as_bytes());
    b.put_u32_le(0); // 0 user comments
    b.put_u8(0x01); // Framing flag
    b.freeze()
}

/// Helper: construct a standard Vorbis Setup packet.
fn make_vorbis_setup_packet() -> Bytes {
    let mut b = BytesMut::new();
    b.put_u8(0x05); // Header type 5: Setup
    b.put_slice(b"vorbis");
    b.put_slice(&[0x42; 64]); // Dummy codebook data
    b.freeze()
}

/// Helper: construct a standard Opus Identification packet (RFC 7845).
fn make_opus_id_packet(channels: u8, sample_rate: u32) -> Bytes {
    let mut b = BytesMut::new();
    b.put_slice(b"OpusHead");
    b.put_u8(1); // Version 1
    b.put_u8(channels);
    b.put_u16_le(384); // Pre-skip
    b.put_u32_le(sample_rate);
    b.put_i16_le(0); // Output gain
    b.put_u8(0); // Channel mapping family
    b.freeze()
}

/// Helper: construct a standard Opus Comment packet.
fn make_opus_comment_packet(vendor: &str) -> Bytes {
    let mut b = BytesMut::new();
    b.put_slice(b"OpusTags");
    b.put_u32_le(vendor.len() as u32);
    b.put_slice(vendor.as_bytes());
    b.put_u32_le(0); // 0 user comments
    b.freeze()
}

/// Helper: construct a standard MPEG-1 Layer III audio frame (128 kbps, 44.1 kHz, stereo).
fn make_mp3_frame(bitrate_idx: u8, sr_idx: u8, padding: bool) -> Bytes {
    let mut b = BytesMut::new();
    b.put_u8(0xFF);
    b.put_u8(0xFB); // MPEG-1, Layer III, no CRC
    let b2 = (bitrate_idx << 4) | (sr_idx << 2) | if padding { 0x02 } else { 0x00 };
    b.put_u8(b2);
    b.put_u8(0x00); // Stereo, no emphasis

    // MPEG-1 Layer III frame length = (144 * bitrate_bps) / sample_rate + padding
    let br = peercast_format::mp3::MPEG1_L3_BITRATES_KBPS[bitrate_idx as usize];
    let sr = peercast_format::mp3::MPEG1_SAMPLING_RATES_HZ[sr_idx as usize];
    let pad = if padding { 1 } else { 0 };
    let frame_len = ((144 * br * 1000) / sr) as usize + pad;

    // Pad remaining frame bytes
    let payload_len = frame_len - 4;
    b.put_slice(&vec![0xAA; payload_len]);
    b.freeze()
}

#[test]
fn test_p30_vorbis_headers_and_audio_only_startup() {
    let serial = 0x12345678;

    // 1. Page 1: BOS with Vorbis ID packet
    let id_pkt = make_vorbis_id_packet(2, 44100, 128_000);
    let page1 = make_ogg_page(OGG_FLAG_BOS, 0, serial, 0, &[&id_pkt]);

    // 2. Page 2: Comment and Setup packets
    let comment_pkt = make_vorbis_comment_packet("PeerCast-Rust-Vorbis");
    let setup_pkt = make_vorbis_setup_packet();
    let page2 = make_ogg_page(0, 0, serial, 1, &[&comment_pkt, &setup_pkt]);

    // 3. Page 3: First audio packet
    let audio_pkt = vec![0x55; 200];
    let page3 = make_ogg_page(0, 1024, serial, 2, &[&audio_pkt]);

    let mut demuxer = OggDemuxer::new();
    demuxer.feed(&page1).unwrap();
    demuxer.feed(&page2).unwrap();

    // Verify HeaderSnapshot established
    let out1 = demuxer.next_output().unwrap().unwrap();
    match out1 {
        OggDemuxOutput::HeaderSnapshotReady {
            snapshot,
            metadata,
            epoch,
        } => {
            assert_eq!(epoch, 0);
            assert_eq!(snapshot.container_type, "OGG");
            assert_eq!(metadata.codec, OggCodec::Vorbis);
            assert_eq!(metadata.content_type, "OGG");
            assert_eq!(metadata.mime_type, "audio/ogg");
            assert_eq!(metadata.stream_ext, ".ogg");
            assert!(metadata.has_audio);
            assert!(!metadata.has_video);
            // Snapshot contains exactly page 1 and page 2 bytes
            assert_eq!(snapshot.data.len(), page1.len() + page2.len());
        }
        other => panic!("Expected HeaderSnapshotReady, got {:?}", other),
    }

    // Feed media page
    demuxer.feed(&page3).unwrap();
    let out2 = demuxer.next_output().unwrap().unwrap();
    match out2 {
        OggDemuxOutput::ContentPage {
            page,
            is_keyframe,
            epoch,
            position,
        } => {
            assert_eq!(epoch, 0);
            assert_eq!(position, (page1.len() + page2.len()) as u64);
            assert_eq!(page, page3);
            // Audio-only admits packets immediately (REQ-055 safe start)
            assert!(is_keyframe);
        }
        other => panic!("Expected ContentPage, got {:?}", other),
    }
}

#[test]
fn test_p30_opus_headers_and_startup() {
    let serial = 0x87654321;

    // Opus requires 2 packets: OpusHead (page 1) and OpusTags (page 2)
    let opus_head = make_opus_id_packet(2, 48000);
    let page1 = make_ogg_page(OGG_FLAG_BOS, 0, serial, 0, &[&opus_head]);

    let opus_tags = make_opus_comment_packet("PeerCast-Rust-Opus");
    let page2 = make_ogg_page(0, 0, serial, 1, &[&opus_tags]);

    let mut demuxer = OggDemuxer::new();
    demuxer.feed(&page1).unwrap();
    demuxer.feed(&page2).unwrap();

    let out = demuxer.next_output().unwrap().unwrap();
    match out {
        OggDemuxOutput::HeaderSnapshotReady {
            snapshot,
            metadata,
            epoch,
        } => {
            assert_eq!(epoch, 0);
            assert_eq!(snapshot.container_type, "OPUS");
            assert_eq!(metadata.codec, OggCodec::Opus);
            assert_eq!(metadata.content_type, "OPUS");
            assert_eq!(metadata.mime_type, "audio/ogg");
            assert_eq!(metadata.stream_ext, ".opus");
        }
        other => panic!("Expected HeaderSnapshotReady, got {:?}", other),
    }
}

#[test]
fn test_p30_theora_video_headers_and_keyframe_detection() {
    let serial = 0x11223344;

    // Theora headers: ID header (0x80theora), comment (0x81theora), setup (0x82theora)
    let mut theora_id = BytesMut::new();
    theora_id.put_u8(0x80);
    theora_id.put_slice(b"theora");
    theora_id.put_slice(&[3, 2, 1]); // version 3.2.1
    theora_id.put_u16(20); // width
    theora_id.put_u16(15); // height

    let page1 = make_ogg_page(OGG_FLAG_BOS, 0, serial, 0, &[&theora_id]);

    let mut theora_comment = BytesMut::new();
    theora_comment.put_u8(0x81);
    theora_comment.put_slice(b"theora");
    theora_comment.put_u32_le(0);

    let mut theora_setup = BytesMut::new();
    theora_setup.put_u8(0x82);
    theora_setup.put_slice(b"theora");
    theora_setup.put_slice(&[0; 32]);

    let page2 = make_ogg_page(0, 0, serial, 1, &[&theora_comment, &theora_setup]);

    let mut demuxer = OggDemuxer::new();
    demuxer.feed(&page1).unwrap();
    demuxer.feed(&page2).unwrap();

    let out1 = demuxer.next_output().unwrap().unwrap();
    match out1 {
        OggDemuxOutput::HeaderSnapshotReady { metadata, .. } => {
            assert_eq!(metadata.codec, OggCodec::Theora);
            assert_eq!(metadata.content_type, "OGM");
            assert_eq!(metadata.mime_type, "video/ogg");
            assert_eq!(metadata.stream_ext, ".ogm");
            assert!(metadata.has_video);
        }
        _ => panic!("Expected HeaderSnapshotReady"),
    }

    // Media pages:
    // Granulepos with interframe count == 0 -> Keyframe
    let kf_page = make_ogg_page(0, 0x00010000_00000000, serial, 2, &[&[0xAA; 100]]);
    demuxer.feed(&kf_page).unwrap();
    let out2 = demuxer.next_output().unwrap().unwrap();
    match out2 {
        OggDemuxOutput::ContentPage { is_keyframe, .. } => {
            assert!(
                is_keyframe,
                "Theora granulepos with 0 interframe offset must be keyframe"
            );
        }
        _ => panic!("Expected ContentPage"),
    }

    // Granulepos with interframe count > 0 -> Interframe
    let inter_page = make_ogg_page(0, 0x00010000_00000001, serial, 3, &[&[0xBB; 100]]);
    demuxer.feed(&inter_page).unwrap();
    let out3 = demuxer.next_output().unwrap().unwrap();
    match out3 {
        OggDemuxOutput::ContentPage { is_keyframe, .. } => {
            assert!(
                !is_keyframe,
                "Theora granulepos with non-zero interframe offset is interframe"
            );
        }
        _ => panic!("Expected ContentPage"),
    }
}

#[test]
fn test_p30_stop_block_unsupported_ogg_codec_rejection() {
    // REQ-056 STOP/BLOCK GUARD:
    // "Stop / block when: Ogg container support is equated with every codec it can contain."
    // An arbitrary codec (e.g. Ogg Skeleton, Speex, FLAC, or unknown binary) must NOT be silently accepted!

    let serial = 0x99999999;
    // Dummy unknown codec header packet: "fishead\0" (Ogg Skeleton)
    let unknown_packet = b"fishead\0\0\0\0\0\0\0";
    let bad_page = make_ogg_page(OGG_FLAG_BOS, 0, serial, 0, &[unknown_packet]);

    let mut demuxer = OggDemuxer::new();
    demuxer.feed(&bad_page).unwrap();

    let err = demuxer.next_output().unwrap_err();
    assert_eq!(err, OggParseError::UnsupportedCodec);
}

#[test]
fn test_p30_chained_stream_serial_change_epoch_bump() {
    // REQ-051: Chained Ogg stream with EOS followed by new BOS bumps epoch
    let serial1 = 0x11111111;
    let id1 = make_vorbis_id_packet(2, 44100, 128_000);
    let p1 = make_ogg_page(OGG_FLAG_BOS, 0, serial1, 0, &[&id1]);
    let c1 = make_vorbis_comment_packet("Stream 1");
    let s1 = make_vorbis_setup_packet();
    let p2 = make_ogg_page(0, 0, serial1, 1, &[&c1, &s1]);
    let eos1 = make_ogg_page(OGG_FLAG_EOS, 1000, serial1, 2, &[&[0x11; 50]]);

    let serial2 = 0x22222222;
    let id2 = make_vorbis_id_packet(2, 48000, 192_000);
    let p3 = make_ogg_page(OGG_FLAG_BOS, 0, serial2, 0, &[&id2]);
    let c2 = make_vorbis_comment_packet("Stream 2");
    let s2 = make_vorbis_setup_packet();
    let p4 = make_ogg_page(0, 0, serial2, 1, &[&c2, &s2]);

    let mut demuxer = OggDemuxer::new();
    demuxer.feed(&p1).unwrap();
    demuxer.feed(&p2).unwrap();

    // First epoch 0
    let out1 = demuxer.next_output().unwrap().unwrap();
    match out1 {
        OggDemuxOutput::HeaderSnapshotReady {
            epoch, metadata, ..
        } => {
            assert_eq!(epoch, 0);
            assert_eq!(metadata.serial_number, serial1);
        }
        _ => panic!("Expected HeaderSnapshotReady"),
    }

    // Media and EOS page
    demuxer.feed(&eos1).unwrap();
    let out_eos = demuxer.next_output().unwrap().unwrap();
    assert!(matches!(out_eos, OggDemuxOutput::ContentPage { .. }));

    // Feed chained stream (BOS with serial2)
    demuxer.feed(&p3).unwrap();
    demuxer.feed(&p4).unwrap();

    let out2 = demuxer.next_output().unwrap().unwrap();
    match out2 {
        OggDemuxOutput::HeaderSnapshotReady {
            epoch, metadata, ..
        } => {
            assert_eq!(epoch, 1, "Chained stream must bump epoch");
            assert_eq!(metadata.serial_number, serial2);
        }
        _ => panic!("Expected HeaderSnapshotReady"),
    }
}

#[test]
fn test_p30_truncated_pages_and_lacing_reassembly() {
    let serial = 0x33333333;
    // Create a 600-byte packet that laces across three 255/255/90 segments
    let large_packet = vec![0x77; 600];
    let page = make_ogg_page(0, 5000, serial, 5, &[&large_packet]);

    let mut demuxer = OggDemuxer::new();

    // Feed byte-by-byte
    for chunk in page.chunks(7) {
        demuxer.feed(chunk).unwrap();
    }

    // Page must be parsed without error when full bytes arrive
    let parsed = OggPageHeader::parse(&page).unwrap().unwrap();
    assert_eq!(parsed.body_len, 600);
    assert_eq!(parsed.segment_table, vec![255, 255, 90]);
}

#[test]
fn test_p30_explicit_mp3_audio_scope_decision_and_parser() {
    // P30 Step 3: Document whether MP3 is supported or deferred with visible capability output.
    // Decision: SUPPORTED for direct playback (audio/mpeg, .mp3) without transcoding.

    // 1. MPEG-1 Layer III frame: bitrate index 9 (128 kbps), sample rate index 0 (44.1 kHz), no padding
    // Frame length = (144 * 128,000) / 44,100 = 417 bytes
    let frame1 = make_mp3_frame(9, 0, false);
    assert_eq!(frame1.len(), 417);

    // Frame 2 with padding: frame length = 418 bytes
    let frame2 = make_mp3_frame(9, 0, true);
    assert_eq!(frame2.len(), 418);

    // Build ID3v2 tag prefix (10 bytes: "ID3" + version + flags + 20 bytes payload = 30 bytes total)
    let mut id3 = BytesMut::new();
    id3.put_slice(b"ID3");
    id3.put_slice(&[3, 0, 0]); // version 2.3.0, flags 0
    id3.put_slice(&[0, 0, 0, 20]); // synchsafe size 20 bytes
    id3.put_slice(&[0u8; 20]); // tag data

    let mut stream = BytesMut::new();
    stream.extend_from_slice(&id3);
    stream.extend_from_slice(&frame1);
    stream.extend_from_slice(&frame2);

    let mut demuxer = Mp3Demuxer::new();
    demuxer.feed(&stream).unwrap();

    // 1. First output: HeaderSnapshotReady (skips ID3 tag and establishes snapshot on frame 1)
    let out1 = demuxer.next_output().unwrap().unwrap();
    match out1 {
        Mp3DemuxOutput::HeaderSnapshotReady {
            snapshot,
            bitrate_kbps,
            sample_rate_hz,
            epoch,
        } => {
            assert_eq!(epoch, 0);
            assert_eq!(snapshot.container_type, "MP3");
            assert_eq!(snapshot.pos, 0);
            assert_eq!(bitrate_kbps, 128);
            assert_eq!(sample_rate_hz, 44100);
            assert_eq!(snapshot.data.len(), 417);
        }
        _ => panic!("Expected HeaderSnapshotReady"),
    }

    // 2. Second output: ContentFrame (frame 2)
    let out2 = demuxer.next_output().unwrap().unwrap();
    match out2 {
        Mp3DemuxOutput::ContentFrame {
            frame,
            epoch,
            position,
        } => {
            assert_eq!(epoch, 0);
            assert_eq!(position, 417);
            assert_eq!(frame.len(), 418);
        }
        _ => panic!("Expected ContentFrame"),
    }

    // Signal EOF
    let eof = demuxer.signal_eof().unwrap().unwrap();
    assert_eq!(eof, Mp3DemuxOutput::EndOfStream);
    assert_eq!(
        demuxer.state(),
        peercast_format::mp3::Mp3DemuxerState::Ended
    );
}
