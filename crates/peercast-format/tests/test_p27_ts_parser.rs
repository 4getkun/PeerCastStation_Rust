use peercast_format::ts::{
    PatTable, PmtTable, TsDemuxOutput, TsDemuxer, TsPacketHeader, TsParseError, TsStreamType,
    TS_PACKET_SIZE, TS_SYNC_BYTE,
};

/// Helper: construct a standard 188-byte PAT packet (PID 0) pointing program 1 to `pmt_pid`.
fn make_pat_packet(cc: u8, pmt_pid: u16) -> [u8; TS_PACKET_SIZE] {
    let mut pkt = [0xFFu8; TS_PACKET_SIZE];
    pkt[0] = TS_SYNC_BYTE;
    pkt[1] = 0x40; // PUSI = 1, PID = 0
    pkt[2] = 0x00;
    pkt[3] = 0x10 | (cc & 0x0F); // No adaptation, payload only, CC

    // Payload: pointer_field = 0
    pkt[4] = 0x00;
    let sec = 5;
    pkt[sec] = 0x00; // table_id = 0 (PAT)
    // section_syntax_indicator = 1, '0', reserved, section_length = 13 (9 + 4)
    pkt[sec + 1] = 0xB0;
    pkt[sec + 2] = 13;
    pkt[sec + 3] = 0x00; // transport_stream_id = 1
    pkt[sec + 4] = 0x01;
    pkt[sec + 5] = 0xC1; // version 0, current_next = 1
    pkt[sec + 6] = 0x00; // section_number = 0
    pkt[sec + 7] = 0x00; // last_section_number = 0

    // Program 1 -> pmt_pid
    pkt[sec + 8] = 0x00;
    pkt[sec + 9] = 0x01; // program_number = 1
    pkt[sec + 10] = 0xE0 | (((pmt_pid >> 8) & 0x1F) as u8);
    pkt[sec + 11] = (pmt_pid & 0xFF) as u8;

    // Dummy 4-byte CRC32
    pkt[sec + 12] = 0xAA;
    pkt[sec + 13] = 0xBB;
    pkt[sec + 14] = 0xCC;
    pkt[sec + 15] = 0xDD;

    pkt
}

/// Helper: construct a standard 188-byte PMT packet describing H.264 video and AAC audio.
fn make_pmt_packet(
    cc: u8,
    pmt_pid: u16,
    video_pid: Option<u16>,
    audio_pid: Option<u16>,
) -> [u8; TS_PACKET_SIZE] {
    let mut pkt = [0xFFu8; TS_PACKET_SIZE];
    pkt[0] = TS_SYNC_BYTE;
    pkt[1] = 0x40 | (((pmt_pid >> 8) & 0x1F) as u8); // PUSI = 1
    pkt[2] = (pmt_pid & 0xFF) as u8;
    pkt[3] = 0x10 | (cc & 0x0F); // Payload only

    pkt[4] = 0x00; // pointer_field = 0
    let sec = 5;
    pkt[sec] = 0x02; // table_id = 2 (PMT)

    let mut stream_bytes = Vec::new();
    if let Some(vpid) = video_pid {
        stream_bytes.push(0x1B); // H.264
        stream_bytes.push(0xE0 | (((vpid >> 8) & 0x1F) as u8));
        stream_bytes.push((vpid & 0xFF) as u8);
        stream_bytes.push(0xF0); // ES_info_length = 0
        stream_bytes.push(0x00);
    }
    if let Some(apid) = audio_pid {
        stream_bytes.push(0x0F); // AAC ADTS
        stream_bytes.push(0xE0 | (((apid >> 8) & 0x1F) as u8));
        stream_bytes.push((apid & 0xFF) as u8);
        stream_bytes.push(0xF0); // ES_info_length = 0
        stream_bytes.push(0x00);
    }

    let section_length = 9 + stream_bytes.len() + 4;
    pkt[sec + 1] = 0xB0 | (((section_length >> 8) & 0x0F) as u8);
    pkt[sec + 2] = (section_length & 0xFF) as u8;
    pkt[sec + 3] = 0x00; // program_number = 1
    pkt[sec + 4] = 0x01;
    pkt[sec + 5] = 0xC1; // version 0, current_next = 1
    pkt[sec + 6] = 0x00;
    pkt[sec + 7] = 0x00;

    let pcr_pid = video_pid.unwrap_or(audio_pid.unwrap_or(0x100));
    pkt[sec + 8] = 0xE0 | (((pcr_pid >> 8) & 0x1F) as u8);
    pkt[sec + 9] = (pcr_pid & 0xFF) as u8;
    pkt[sec + 10] = 0xF0; // program_info_length = 0
    pkt[sec + 11] = 0x00;

    for (i, &b) in stream_bytes.iter().enumerate() {
        pkt[sec + 12 + i] = b;
    }

    let crc_pos = sec + 12 + stream_bytes.len();
    pkt[crc_pos] = 0x11;
    pkt[crc_pos + 1] = 0x22;
    pkt[crc_pos + 2] = 0x33;
    pkt[crc_pos + 3] = 0x44;

    pkt
}

/// Helper: construct a 188-byte media TS packet with optional adaptation field / keyframe indicator.
fn make_media_packet(
    pid: u16,
    cc: u8,
    is_keyframe: bool,
    pusi: bool,
    payload: &[u8],
) -> [u8; TS_PACKET_SIZE] {
    let mut pkt = [0xFFu8; TS_PACKET_SIZE];
    pkt[0] = TS_SYNC_BYTE;
    let pusi_bit = if pusi { 0x40 } else { 0x00 };
    pkt[1] = pusi_bit | (((pid >> 8) & 0x1F) as u8);
    pkt[2] = (pid & 0xFF) as u8;

    if is_keyframe {
        // Adaptation field + payload: 0x30
        pkt[3] = 0x30 | (cc & 0x0F);
        pkt[4] = 7; // adaptation_field_length = 7
        pkt[5] = 0x50; // RAI (0x40) + PCR flag (0x10)
        // 6 bytes PCR
        pkt[6] = 0x00;
        pkt[7] = 0x00;
        pkt[8] = 0x00;
        pkt[9] = 0x02;
        pkt[10] = 0x00;
        pkt[11] = 0x00;
        let payload_start = 12;
        let copy_len = payload.len().min(TS_PACKET_SIZE - payload_start);
        pkt[payload_start..payload_start + copy_len].copy_from_slice(&payload[..copy_len]);
    } else {
        // Payload only: 0x10
        pkt[3] = 0x10 | (cc & 0x0F);
        let payload_start = 4;
        let copy_len = payload.len().min(TS_PACKET_SIZE - payload_start);
        pkt[payload_start..payload_start + copy_len].copy_from_slice(&payload[..copy_len]);
    }

    pkt
}

#[test]
fn test_p27_pat_pmt_parsing_and_header_reconstruction() {
    let pat_pkt = make_pat_packet(0, 256);
    let pmt_pkt = make_pmt_packet(0, 256, Some(257), Some(258));
    let video_kf = make_media_packet(257, 1, true, true, &[0x00, 0x00, 0x01, 0xE0, 0x01, 0x02]);

    let mut demuxer = TsDemuxer::default();

    // Feed PAT and PMT
    demuxer.feed(&pat_pkt);
    demuxer.feed(&pmt_pkt);
    demuxer.feed(&video_kf);

    let mut outputs = Vec::new();
    while let Some(out) = demuxer.next_output().unwrap() {
        outputs.push(out);
    }

    // Must emit HeaderChanged with 376-byte HeaderSnapshot
    assert_eq!(outputs.len(), 2);
    let snapshot = match &outputs[0] {
        TsDemuxOutput::HeaderChanged(s) => s,
        _ => panic!("Expected HeaderChanged first"),
    };

    assert_eq!(snapshot.container_type, "TS");
    assert_eq!(snapshot.data.len(), 376); // PAT (188) + PMT (188)
    assert_eq!(snapshot.data[0], TS_SYNC_BYTE);
    assert_eq!(snapshot.data[188], TS_SYNC_BYTE);

    // Verify normalized CC bits in snapshot (must have lower 4 bits masked to 0)
    assert_eq!(snapshot.data[3] & 0x0F, 0x00);
    assert_eq!(snapshot.data[191] & 0x0F, 0x00);

    // Verify media packet
    match &outputs[1] {
        TsDemuxOutput::MediaPacket {
            pid,
            is_keyframe,
            header,
            ..
        } => {
            assert_eq!(*pid, 257);
            assert!(is_keyframe);
            assert!(header.random_access_indicator);
            assert!(header.pcr.is_some());
        }
        _ => panic!("Expected MediaPacket"),
    }

    // Independent parse test: feed reconstructed HeaderSnapshot bytes into fresh PAT/PMT parsers
    let pat_header = TsPacketHeader::parse(&snapshot.data[..188]).unwrap();
    let pat_table = PatTable::parse(&snapshot.data[..188], pat_header.payload_offset).unwrap();
    assert_eq!(pat_table.programs, vec![(1, 256)]);

    let pmt_header = TsPacketHeader::parse(&snapshot.data[188..376]).unwrap();
    let pmt_table = PmtTable::parse(&snapshot.data[188..376], pmt_header.payload_offset).unwrap();
    assert_eq!(pmt_table.program_number, 1);
    assert_eq!(pmt_table.streams.len(), 2);
    assert_eq!(pmt_table.streams[0].stream_type, TsStreamType::H264);
    assert_eq!(pmt_table.streams[0].pid, 257);
    assert_eq!(pmt_table.streams[1].stream_type, TsStreamType::AacAdts);
    assert_eq!(pmt_table.streams[1].pid, 258);
}

#[test]
fn test_p27_continuity_counter_and_adaptation_field() {
    let pat_pkt = make_pat_packet(0, 256);
    let pmt_pkt = make_pmt_packet(0, 256, Some(257), None);

    let mut demuxer = TsDemuxer::default();
    demuxer.feed(&pat_pkt);
    demuxer.feed(&pmt_pkt);
    let _ = demuxer.next_output().unwrap(); // consumes header

    // Feed 3 video packets with sequential CC (1, 2, 3)
    let pkt1 = make_media_packet(257, 1, false, false, &[0xAA; 100]);
    let pkt2 = make_media_packet(257, 2, false, false, &[0xBB; 100]);
    let pkt3 = make_media_packet(257, 3, true, true, &[0xCC; 100]); // Keyframe with RAI

    demuxer.feed(&pkt1);
    demuxer.feed(&pkt2);
    demuxer.feed(&pkt3);

    let out1 = demuxer.next_output().unwrap().unwrap();
    let out2 = demuxer.next_output().unwrap().unwrap();
    let out3 = demuxer.next_output().unwrap().unwrap();

    match (out1, out2, out3) {
        (
            TsDemuxOutput::MediaPacket { header: h1, is_keyframe: k1, .. },
            TsDemuxOutput::MediaPacket { header: h2, is_keyframe: k2, .. },
            TsDemuxOutput::MediaPacket { header: h3, is_keyframe: k3, .. },
        ) => {
            assert_eq!(h1.continuity_counter, 1);
            assert!(!k1);
            assert_eq!(h2.continuity_counter, 2);
            assert!(!k2);
            assert_eq!(h3.continuity_counter, 3);
            assert!(k3); // RAI set on keyframe!
            assert!(h3.random_access_indicator);
        }
        _ => panic!("Expected 3 MediaPackets"),
    }
}

#[test]
fn test_p27_audio_only_ts_demuxing() {
    // PMT with audio only (no video PID)
    let pat_pkt = make_pat_packet(0, 256);
    let pmt_pkt = make_pmt_packet(0, 256, None, Some(300));
    let audio_pkt = make_media_packet(300, 1, false, true, &[0xFF, 0xF1, 0x50, 0x80]);

    let mut demuxer = TsDemuxer::default();
    demuxer.feed(&pat_pkt);
    demuxer.feed(&pmt_pkt);
    demuxer.feed(&audio_pkt);

    let ev1 = demuxer.next_output().unwrap().unwrap();
    assert!(matches!(ev1, TsDemuxOutput::HeaderChanged(_)));

    let ev2 = demuxer.next_output().unwrap().unwrap();
    match ev2 {
        TsDemuxOutput::MediaPacket { pid, is_keyframe, .. } => {
            assert_eq!(pid, 300);
            assert!(!is_keyframe);
        }
        _ => panic!("Expected audio MediaPacket"),
    }
}

#[test]
fn test_p27_changed_pmt_pid_epoch_invalidation() {
    let mut demuxer = TsDemuxer::default();

    // Initial PAT/PMT on PMT PID 256 (epoch 1)
    let pat_1 = make_pat_packet(0, 256);
    let pmt_1 = make_pmt_packet(0, 256, Some(257), None);
    demuxer.feed(&pat_1);
    demuxer.feed(&pmt_1);

    let ev1 = demuxer.next_output().unwrap().unwrap();
    let initial_epoch = match ev1 {
        TsDemuxOutput::HeaderChanged(s) => s.epoch,
        _ => panic!("Expected HeaderChanged"),
    };

    // Video packet on epoch 1
    let v1 = make_media_packet(257, 1, true, true, &[1, 2, 3]);
    demuxer.feed(&v1);
    let ev2 = demuxer.next_output().unwrap().unwrap();
    match ev2 {
        TsDemuxOutput::MediaPacket { epoch, .. } => assert_eq!(epoch, initial_epoch),
        _ => panic!("Expected MediaPacket"),
    }

    // Source updates PAT: PMT PID changes to 512, new video PID 513
    let pat_2 = make_pat_packet(1, 512);
    let pmt_2 = make_pmt_packet(1, 512, Some(513), None);
    demuxer.feed(&pat_2);
    demuxer.feed(&pmt_2);

    let ev3 = demuxer.next_output().unwrap().unwrap();
    let updated_epoch = match ev3 {
        TsDemuxOutput::HeaderChanged(s) => {
            assert!(s.epoch > initial_epoch, "Epoch must increment on PID change (REQ-051)");
            s.epoch
        }
        _ => panic!("Expected HeaderChanged on PID change"),
    };

    // Subsequent media packet uses updated epoch
    let v2 = make_media_packet(513, 1, true, true, &[4, 5, 6]);
    demuxer.feed(&v2);
    let ev4 = demuxer.next_output().unwrap().unwrap();
    match ev4 {
        TsDemuxOutput::MediaPacket { epoch, pid, .. } => {
            assert_eq!(epoch, updated_epoch);
            assert_eq!(pid, 513);
        }
        _ => panic!("Expected MediaPacket on new PID"),
    }
}

#[test]
fn test_p27_malformed_sync_and_unsupported_framing_rejection() {
    let mut demuxer = TsDemuxer::default();

    // 1. Noise prefix before valid TS sync byte 0x47
    let mut noisy_stream = Vec::new();
    noisy_stream.extend_from_slice(&[0x00, 0x11, 0x22, 0x33]); // 4 bytes garbage
    noisy_stream.extend_from_slice(&make_pat_packet(0, 256));

    demuxer.feed(&noisy_stream);
    // Demuxer should scan forward to 0x47 and parse PAT cleanly
    let _ = demuxer.next_output().unwrap(); // consumes PAT

    // 2. Stop/block rule: 192-byte and 204-byte framing must fail parse
    let invalid_192 = [0x47u8; 192];
    let res_192 = TsPacketHeader::parse(&invalid_192);
    assert!(
        matches!(res_192, Err(TsParseError::InvalidPacketSize(192))),
        "192-byte framing must be rejected without configuration"
    );

    let invalid_204 = [0x47u8; 204];
    let res_204 = TsPacketHeader::parse(&invalid_204);
    assert!(
        matches!(res_204, Err(TsParseError::InvalidPacketSize(204))),
        "204-byte framing must be rejected without configuration"
    );

    // 3. Corrupted sync byte at index 0
    let mut corrupted_sync = [0x47u8; TS_PACKET_SIZE];
    corrupted_sync[0] = 0x99;
    let res_sync = TsPacketHeader::parse(&corrupted_sync);
    assert_eq!(res_sync, Err(TsParseError::MalformedSyncByte(0x99)));

    // 4. Adaptation field length exceeding 183 bytes
    let mut invalid_af = [0x47u8; TS_PACKET_SIZE];
    invalid_af[3] = 0x20; // Adaptation field only
    invalid_af[4] = 184; // 184 > 183 limit
    let res_af = TsPacketHeader::parse(&invalid_af);
    assert_eq!(res_af, Err(TsParseError::AdaptationFieldTooLong(184)));
}
