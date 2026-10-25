use bytes::Bytes;
use peercast_core::cache::{BoundedContentCache, FakeClock, HeaderSnapshot};
use peercast_core::content::{Content, ContinuationFlags};
use peercast_format::safestart::{
    SafeStartConfig, SafeStartDecision, SafeStartMode, SafeStartPhase, SafeStartSelector,
};
use std::sync::Arc;
use std::time::Duration;

/// Helper: construct a mock FLV video packet (either keyframe 0x17 or interframe 0x27).
fn make_flv_video_packet(epoch: i32, pos: i64, is_keyframe: bool, is_fragment: bool) -> Content {
    let mut data = Vec::with_capacity(16);
    data.push(9); // FLV Tag Type = Video
    data.extend_from_slice(&[0, 0, 5]); // data_size = 5
    data.extend_from_slice(&[0, 0, 0, 0]); // timestamp = 0
    data.extend_from_slice(&[0, 0, 0]); // stream_id = 0
                                        // Tag body byte 0: frame_type (1 = keyframe, 2 = interframe), codec 7 (AVC)
    let first_byte = if is_keyframe { 0x17 } else { 0x27 };
    data.push(first_byte);
    data.extend_from_slice(&[1, 0, 0, 0]); // AVCPacketType = 1 (NALU)

    let cont = if is_fragment {
        ContinuationFlags::FRAGMENT
    } else {
        ContinuationFlags::NONE
    };

    Content::new(epoch, Duration::from_millis(pos as u64), pos, data, cont)
}

/// Helper: construct mock audio packet.
fn make_audio_packet(epoch: i32, pos: i64, is_fragment: bool) -> Content {
    let mut data = Vec::with_capacity(16);
    data.push(8); // FLV Tag Type = Audio
    data.extend_from_slice(&[0, 0, 4]); // data_size = 4
    data.extend_from_slice(&[0, 0, 0, 0]); // timestamp = 0
    data.extend_from_slice(&[0, 0, 0]); // stream_id = 0
    data.extend_from_slice(&[0xAF, 1, 0xAA, 0xBB]); // AAC raw frame

    let cont = if is_fragment {
        ContinuationFlags::FRAGMENT
    } else {
        ContinuationFlags::NONE
    };

    Content::new(epoch, Duration::from_millis(pos as u64), pos, data, cont)
}

#[test]
fn test_p24_interframe_first_skipped_despite_cont_zero() {
    let mut selector = SafeStartSelector::new(SafeStartConfig {
        mode: SafeStartMode::VideoKeyframe,
        max_wait: Duration::from_secs(3),
    });

    let header = HeaderSnapshot::new(1, 0, "FLV", Bytes::from_static(b"FLV_HEADER"));
    let decision = selector.on_header(&header);
    assert_eq!(decision, SafeStartDecision::EmitHeader(header));

    // 1. Interframe with cont == NONE arrives first
    let interframe1 = make_flv_video_packet(1, 100, false, false);
    assert_eq!(interframe1.cont_flags, ContinuationFlags::NONE);

    // CRITICAL (REQ-050): cont == 0 MUST NOT be treated as proof of IDR!
    let d1 = selector.evaluate(&interframe1, None);
    assert!(
        matches!(d1, SafeStartDecision::Skip { .. }),
        "Interframe must be skipped despite cont==0 (REQ-050)"
    );

    // 2. Second interframe arrives
    let interframe2 = make_flv_video_packet(1, 200, false, false);
    let d2 = selector.evaluate(&interframe2, None);
    assert!(matches!(d2, SafeStartDecision::Skip { .. }));

    // 3. True video keyframe arrives
    let keyframe = make_flv_video_packet(1, 300, true, false);
    let d3 = selector.evaluate(&keyframe, None);
    assert_eq!(d3, SafeStartDecision::Admit);
    assert_eq!(
        selector.phase(),
        SafeStartPhase::Streaming {
            epoch: 1,
            started_at_pos: 300,
        }
    );

    // 4. Subsequent interframe is now admitted as part of active stream
    let interframe3 = make_flv_video_packet(1, 400, false, false);
    let d4 = selector.evaluate(&interframe3, None);
    assert_eq!(d4, SafeStartDecision::Admit);
}

#[test]
fn test_p24_fragmented_keyframe_boundary_guards() {
    let mut selector = SafeStartSelector::new(SafeStartConfig {
        mode: SafeStartMode::VideoKeyframe,
        max_wait: Duration::from_secs(3),
    });

    let header = HeaderSnapshot::new(1, 0, "FLV", Bytes::from_static(b"FLV_HEADER"));
    selector.on_header(&header);

    // 1. Fragmented keyframe slice arriving without start of frame
    let keyframe_fragment = make_flv_video_packet(1, 500, true, true);
    let d1 = selector.evaluate(&keyframe_fragment, None);
    assert!(
        matches!(d1, SafeStartDecision::Skip { reason } if reason.contains("continuation fragment")),
        "Cannot start playback on a continuation fragment"
    );

    // 2. First unfragmented keyframe slice arrives
    let keyframe_start = make_flv_video_packet(1, 1000, true, false);
    let d2 = selector.evaluate(&keyframe_start, None);
    assert_eq!(d2, SafeStartDecision::Admit);

    // 3. Subsequent fragment slice of the ongoing frame is admitted
    let keyframe_continuation = make_flv_video_packet(1, 1100, true, true);
    let d3 = selector.evaluate(&keyframe_continuation, None);
    assert_eq!(d3, SafeStartDecision::Admit);
}

#[test]
fn test_p24_audio_only_never_waits_for_keyframe() {
    let mut selector = SafeStartSelector::new(SafeStartConfig {
        mode: SafeStartMode::AudioOnly,
        max_wait: Duration::from_secs(3),
    });

    let header = HeaderSnapshot::new(1, 0, "FLV", Bytes::from_static(b"FLV_HEADER"));
    selector.on_header(&header);

    // First audio packet arrives (not a video keyframe)
    let audio_pkt = make_audio_packet(1, 100, false);
    let d1 = selector.evaluate(&audio_pkt, None);

    assert_eq!(
        d1,
        SafeStartDecision::Admit,
        "Audio-only format must admit first unfragmented packet immediately (REQ-055)"
    );
    assert_eq!(
        selector.phase(),
        SafeStartPhase::Streaming {
            epoch: 1,
            started_at_pos: 100,
        }
    );
}

#[test]
fn test_p24_no_keyframe_timeout_bounded_fallback() {
    let fake_clock = Arc::new(FakeClock::new(1000));
    let mut selector = SafeStartSelector::with_clock(
        SafeStartConfig {
            mode: SafeStartMode::VideoKeyframe,
            max_wait: Duration::from_secs(3),
        },
        fake_clock.clone(),
    );

    let header = HeaderSnapshot::new(1, 0, "FLV", Bytes::from_static(b"FLV_HEADER"));
    selector.on_header(&header);

    // At t = 1000ms: Interframe skipped
    let if1 = make_flv_video_packet(1, 100, false, false);
    assert!(matches!(
        selector.evaluate(&if1, None),
        SafeStartDecision::Skip { .. }
    ));

    // Advance clock to 3000ms (elapsed 2000ms < 3000ms max_wait)
    fake_clock.set(Duration::from_millis(3000));
    let if2 = make_flv_video_packet(1, 200, false, false);
    assert!(matches!(
        selector.evaluate(&if2, None),
        SafeStartDecision::Skip { .. }
    ));

    // Advance clock to 4500ms (elapsed 3500ms >= 3000ms max_wait)
    fake_clock.set(Duration::from_millis(4500));
    let if3 = make_flv_video_packet(1, 300, false, false);
    let d3 = selector.evaluate(&if3, None);

    assert!(
        matches!(d3, SafeStartDecision::FallbackAdmit { .. }),
        "Must fallback-admit after timeout without indefinite waiting (REQ-055)"
    );
    assert!(matches!(selector.phase(), SafeStartPhase::Streaming { .. }));
}

#[test]
fn test_p24_epoch_restart_resets_safe_start() {
    let mut selector = SafeStartSelector::new(SafeStartConfig {
        mode: SafeStartMode::VideoKeyframe,
        max_wait: Duration::from_secs(3),
    });

    let header1 = HeaderSnapshot::new(1, 0, "FLV", Bytes::from_static(b"FLV_HEADER_1"));
    selector.on_header(&header1);

    // Start streaming epoch 1
    let kf1 = make_flv_video_packet(1, 100, true, false);
    assert_eq!(selector.evaluate(&kf1, None), SafeStartDecision::Admit);

    // New epoch arrives (e.g. resolution change or source restart)
    let header2 = HeaderSnapshot::new(2, 500, "FLV", Bytes::from_static(b"FLV_HEADER_2"));
    let d_hdr = selector.on_header(&header2);
    assert_eq!(d_hdr, SafeStartDecision::EmitHeader(header2));

    // Stale epoch 1 packet is rejected
    let stale_pkt = make_flv_video_packet(1, 550, true, false);
    assert!(matches!(
        selector.evaluate(&stale_pkt, None),
        SafeStartDecision::Skip { .. }
    ));

    // Epoch 2 interframe is skipped
    let if2 = make_flv_video_packet(2, 600, false, false);
    assert!(matches!(
        selector.evaluate(&if2, None),
        SafeStartDecision::Skip { .. }
    ));

    // Epoch 2 keyframe is admitted
    let kf2 = make_flv_video_packet(2, 700, true, false);
    assert_eq!(selector.evaluate(&kf2, None), SafeStartDecision::Admit);
    assert_eq!(
        selector.phase(),
        SafeStartPhase::Streaming {
            epoch: 2,
            started_at_pos: 700,
        }
    );
}

#[test]
fn test_p24_select_cache_start_point_historical() {
    let fake_clock = FakeClock::new(1000);
    let mut cache = BoundedContentCache::with_clock(
        1024 * 1024,
        Duration::from_secs(10),
        64 * 1024,
        fake_clock,
    );

    let header = HeaderSnapshot::new(1, 0, "FLV", Bytes::from_static(b"FLV_HEADER"));
    cache.set_header(header).unwrap();

    // Push: Keyframe 1 (pos 0), Interframe 1 (pos 1000), Keyframe 2 (pos 2000), Interframe 2 (pos 3000)
    cache
        .push_body(make_flv_video_packet(1, 0, true, false), None)
        .unwrap();
    cache
        .push_body(make_flv_video_packet(1, 1000, false, false), None)
        .unwrap();
    cache
        .push_body(make_flv_video_packet(1, 2000, true, false), None)
        .unwrap();
    cache
        .push_body(make_flv_video_packet(1, 3000, false, false), None)
        .unwrap();

    let video_selector = SafeStartSelector::new(SafeStartConfig {
        mode: SafeStartMode::VideoKeyframe,
        max_wait: Duration::from_secs(3),
    });

    // In video mode, must select the most recent keyframe (pos 2000), NOT the interframe at pos 3000!
    let chosen_video = video_selector
        .select_cache_start_point(&cache, 1)
        .expect("Must find keyframe start point");
    assert_eq!(chosen_video.pos, 2000);

    // In audio mode, selects the newest unfragmented packet (pos 3000)
    let audio_selector = SafeStartSelector::new(SafeStartConfig {
        mode: SafeStartMode::AudioOnly,
        max_wait: Duration::from_secs(3),
    });
    let chosen_audio = audio_selector
        .select_cache_start_point(&cache, 1)
        .expect("Must find unfragmented start point");
    assert_eq!(chosen_audio.pos, 3000);
}
