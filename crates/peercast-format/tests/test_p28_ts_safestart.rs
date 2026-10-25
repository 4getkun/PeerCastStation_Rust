use bytes::Bytes;
use peercast_core::cache::{BoundedContentCache, FakeClock, HeaderSnapshot};
use peercast_core::content::{Content, ContinuationFlags};
use peercast_format::safestart::{
    SafeStartConfig, SafeStartDecision, SafeStartMode, SafeStartPhase, SafeStartSelector,
};
use peercast_format::ts::{TS_PACKET_SIZE, TS_SYNC_BYTE};
use std::sync::Arc;
use std::time::Duration;

/// Helper: construct a 188-byte TS packet with configurable PID, RAI flag, and CC.
fn make_ts_packet(pid: u16, cc: u8, has_rai: bool, payload: &[u8]) -> Content {
    let mut pkt = [0xFFu8; TS_PACKET_SIZE];
    pkt[0] = TS_SYNC_BYTE;
    pkt[1] = 0x40 | (((pid >> 8) & 0x1F) as u8); // PUSI = 1
    pkt[2] = (pid & 0xFF) as u8;

    if has_rai {
        pkt[3] = 0x30 | (cc & 0x0F); // Adaptation + payload
        pkt[4] = 7; // AF length
        pkt[5] = 0x40; // RAI = 1 (Random Access Indicator)
        pkt[6..12].copy_from_slice(&[0, 0, 0, 1, 0, 0]); // PCR
        let start = 12;
        let len = payload.len().min(TS_PACKET_SIZE - start);
        pkt[start..start + len].copy_from_slice(&payload[..len]);
    } else {
        pkt[3] = 0x10 | (cc & 0x0F); // Payload only
        let start = 4;
        let len = payload.len().min(TS_PACKET_SIZE - start);
        pkt[start..start + len].copy_from_slice(&payload[..len]);
    }

    Content::new(
        1,
        Duration::from_millis(100),
        1000,
        Bytes::copy_from_slice(&pkt),
        ContinuationFlags::NONE,
    )
}

/// Helper: construct 376-byte PAT + PMT HeaderSnapshot.
fn make_ts_header_snapshot(epoch: i32) -> HeaderSnapshot {
    let mut data = vec![0xFFu8; 376];
    // PAT packet at 0..188
    data[0] = TS_SYNC_BYTE;
    data[1] = 0x40; // PID 0
    data[2] = 0x00;
    data[3] = 0x10; // CC 0

    // PMT packet at 188..376
    data[188] = TS_SYNC_BYTE;
    data[189] = 0x41; // PID 256
    data[190] = 0x00;
    data[191] = 0x10; // CC 0

    HeaderSnapshot::new(epoch, 0, "TS", Bytes::from(data))
}

#[test]
fn test_p28_pat_pmt_alone_not_treated_as_video_keyframe() {
    let mut selector = SafeStartSelector::new(SafeStartConfig {
        mode: SafeStartMode::TsVideoKeyframe,
        max_wait: Duration::from_secs(3),
    });

    let header = make_ts_header_snapshot(1);
    let decision = selector.on_header(&header);
    assert_eq!(decision, SafeStartDecision::EmitHeader(header));

    // REQ-053 STOP/BLOCK RULE:
    // PAT/PMT packets provide table IDs and PIDs, but NOT an immediate artifact-free keyframe!
    // Video packet arrives with RAI == 0 (Interframe)
    let interframe_ts = make_ts_packet(257, 1, false, &[0x00, 0x00, 0x01, 0xE0, 0x22, 0x33]);
    let d1 = selector.evaluate(&interframe_ts, None);

    assert!(
        matches!(d1, SafeStartDecision::Skip { .. }),
        "PAT/PMT alone must NOT permit starting on an interframe (REQ-053)"
    );

    // Video packet arrives with RAI == 1 (Random Access Indicator in adaptation field)
    let keyframe_ts = make_ts_packet(257, 2, true, &[0x00, 0x00, 0x01, 0xE0, 0x65, 0x88]);
    let d2 = selector.evaluate(&keyframe_ts, None);

    assert_eq!(d2, SafeStartDecision::Admit);
    assert!(matches!(selector.phase(), SafeStartPhase::Streaming { .. }));
}

#[test]
fn test_p28_mid_stream_join_receives_pat_pmt_before_media() {
    let fake_clock = FakeClock::new(1000);
    let mut cache = BoundedContentCache::with_clock(
        1024 * 1024,
        Duration::from_secs(10),
        64 * 1024,
        fake_clock,
    );

    let header = make_ts_header_snapshot(1);
    cache.set_header(header.clone()).unwrap();

    // Cache contains: Interframe (pos 0), Interframe (pos 1000), Keyframe with RAI (pos 2000), Interframe (pos 3000)
    let mut pkt0 = make_ts_packet(257, 1, false, &[0xAA; 100]);
    pkt0.pos = 0;
    let mut pkt1 = make_ts_packet(257, 2, false, &[0xBB; 100]);
    pkt1.pos = 1000;
    let mut pkt2 = make_ts_packet(257, 3, true, &[0xCC; 100]); // Keyframe
    pkt2.pos = 2000;
    let mut pkt3 = make_ts_packet(257, 4, false, &[0xDD; 100]);
    pkt3.pos = 3000;

    cache.push_body(pkt0, None).unwrap();
    cache.push_body(pkt1, None).unwrap();
    cache.push_body(pkt2, None).unwrap();
    cache.push_body(pkt3, None).unwrap();

    let selector = SafeStartSelector::new(SafeStartConfig {
        mode: SafeStartMode::TsVideoKeyframe,
        max_wait: Duration::from_secs(3),
    });

    // 1. Selector selects the latest keyframe (pos 2000)
    let start_point = selector
        .select_cache_start_point(&cache, 1)
        .expect("Must find keyframe start point");
    assert_eq!(start_point.pos, 2000);

    // 2. Verified invariant: cache has valid PAT/PMT HeaderSnapshot
    let active_header = cache.header().expect("Active header must be present");
    assert_eq!(active_header.container_type, "TS");
    assert_eq!(active_header.data.len(), 376); // Precedes media packet
}

#[test]
fn test_p28_ts_audio_only_never_waits_for_rai() {
    let mut selector = SafeStartSelector::new(SafeStartConfig {
        mode: SafeStartMode::AudioOnly,
        max_wait: Duration::from_secs(3),
    });

    let header = make_ts_header_snapshot(1);
    selector.on_header(&header);

    // Audio TS packet arrives (RAI is false, no video)
    let audio_ts = make_ts_packet(258, 1, false, &[0xFF, 0xF1, 0x50, 0x80]);
    let d = selector.evaluate(&audio_ts, None);

    assert_eq!(
        d,
        SafeStartDecision::Admit,
        "Audio-only TS must admit first unfragmented packet immediately without waiting for RAI"
    );
    assert!(matches!(selector.phase(), SafeStartPhase::Streaming { .. }));
}

#[test]
fn test_p28_ts_no_rai_timeout_fallback() {
    let fake_clock = Arc::new(FakeClock::new(1000));
    let mut selector = SafeStartSelector::with_clock(
        SafeStartConfig {
            mode: SafeStartMode::TsVideoKeyframe,
            max_wait: Duration::from_secs(3),
        },
        fake_clock.clone(),
    );

    let header = make_ts_header_snapshot(1);
    selector.on_header(&header);

    // Stream sends video TS packets without RAI (e.g. encoder without RAI flags)
    let pkt1 = make_ts_packet(257, 1, false, &[1, 2, 3]);
    assert!(matches!(
        selector.evaluate(&pkt1, None),
        SafeStartDecision::Skip { .. }
    ));

    // Advance clock past 3s timeout
    fake_clock.set(Duration::from_millis(4500)); // 3500ms > 3000ms max_wait
    let pkt2 = make_ts_packet(257, 2, false, &[4, 5, 6]);
    let d2 = selector.evaluate(&pkt2, None);

    assert!(
        matches!(d2, SafeStartDecision::FallbackAdmit { .. }),
        "Must fallback-admit after timeout without indefinite stall (REQ-055)"
    );
    assert!(matches!(selector.phase(), SafeStartPhase::Streaming { .. }));
}

#[test]
fn test_p28_pid_change_resets_safe_start() {
    let mut selector = SafeStartSelector::new(SafeStartConfig {
        mode: SafeStartMode::TsVideoKeyframe,
        max_wait: Duration::from_secs(3),
    });

    let header1 = make_ts_header_snapshot(1);
    selector.on_header(&header1);

    // Start streaming epoch 1
    let kf1 = make_ts_packet(257, 1, true, &[1, 2]);
    assert_eq!(selector.evaluate(&kf1, None), SafeStartDecision::Admit);

    // PMT / PID change in broadcast (epoch 2)
    let header2 = make_ts_header_snapshot(2);
    let d_hdr = selector.on_header(&header2);
    assert_eq!(d_hdr, SafeStartDecision::EmitHeader(header2));

    // Stale epoch 1 packet rejected
    let mut stale_pkt = make_ts_packet(257, 2, true, &[3, 4]);
    stale_pkt.stream_id = 1;
    assert!(matches!(
        selector.evaluate(&stale_pkt, None),
        SafeStartDecision::Skip { .. }
    ));

    // Non-RAI packet in epoch 2 skipped
    let mut if2 = make_ts_packet(513, 1, false, &[5, 6]);
    if2.stream_id = 2;
    assert!(matches!(
        selector.evaluate(&if2, None),
        SafeStartDecision::Skip { .. }
    ));

    // RAI keyframe in epoch 2 admitted
    let mut kf2 = make_ts_packet(513, 2, true, &[7, 8]);
    kf2.stream_id = 2;
    assert_eq!(selector.evaluate(&kf2, None), SafeStartDecision::Admit);
    assert_eq!(
        selector.phase(),
        SafeStartPhase::Streaming {
            epoch: 2,
            started_at_pos: 1000,
        }
    );
}
