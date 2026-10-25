use peercast_core::cache::{
    BoundedContentCache, CacheError, FakeClock, HeaderSnapshot, PositionContinuity,
};
use peercast_core::content::{Content, ContinuationFlags};
use std::time::Duration;

#[test]
fn test_missing_and_oversized_header_rejection() {
    let fake_clock = FakeClock::new(0);
    let mut cache = BoundedContentCache::with_clock(
        8 * 1024 * 1024,
        Duration::from_secs(3),
        1024 * 1024, // 1 MiB max header
        fake_clock,
    );

    // 1. Pushing body without an active header must fail with MissingHeader
    let chunk = Content::new(
        1,
        Duration::from_secs(0),
        0,
        vec![0u8; 100],
        ContinuationFlags::NONE,
    );
    assert_eq!(cache.push_body(chunk, None), Err(CacheError::MissingHeader));

    // 2. Setting an oversized header must fail with HeaderTooLarge and never truncate silently
    let oversized_header = HeaderSnapshot::new(1, 0, "FLV", vec![0u8; 2 * 1024 * 1024]); // 2 MiB
    assert!(matches!(
        cache.set_header(oversized_header),
        Err(CacheError::HeaderTooLarge { actual, max }) if actual == 2 * 1024 * 1024 && max == 1024 * 1024
    ));
    assert!(cache.header().is_none());
}

#[test]
fn test_no_old_epoch_body_follows_new_header() {
    let fake_clock = FakeClock::new(0);
    let mut cache = BoundedContentCache::with_clock(
        8 * 1024 * 1024,
        Duration::from_secs(3),
        1024 * 1024,
        fake_clock,
    );

    // Set epoch 1 header
    let h1 = HeaderSnapshot::new(1, 0, "FLV", b"flv-header-v1".to_vec());
    cache.set_header(h1).unwrap();
    assert_eq!(cache.current_epoch(), Some(1));

    // Push epoch 1 body chunks
    let c1 = Content::new(
        1,
        Duration::from_secs(0),
        100,
        vec![0u8; 500],
        ContinuationFlags::NONE,
    );
    cache.push_body(c1, None).unwrap();
    assert_eq!(cache.body_len(), 1);

    // Stream generation restarts: set epoch 2 header
    let h2 = HeaderSnapshot::new(2, 0, "FLV", b"flv-header-v2".to_vec());
    cache.set_header(h2).unwrap();
    assert_eq!(cache.current_epoch(), Some(2));

    // REQ-051: All prior body chunks from epoch 1 must be purged immediately
    assert_eq!(cache.body_len(), 0, "No old-epoch body chunks may remain");
    assert_eq!(cache.total_body_bytes(), 0);

    // Attempting to push a stale epoch 1 chunk must be rejected
    let c_stale = Content::new(
        1,
        Duration::from_secs(1),
        600,
        vec![0u8; 500],
        ContinuationFlags::NONE,
    );
    assert!(matches!(
        cache.push_body(c_stale, None),
        Err(CacheError::StaleEpoch {
            chunk_epoch: 1,
            current_epoch: 2
        })
    ));
    assert_eq!(cache.body_len(), 0);

    // Pushing a valid epoch 2 chunk succeeds
    let c2 = Content::new(
        2,
        Duration::from_secs(0),
        100,
        vec![0u8; 500],
        ContinuationFlags::NONE,
    );
    assert!(cache.push_body(c2, None).is_ok());
    assert_eq!(cache.body_len(), 1);
}

#[test]
fn test_time_only_eviction_with_fake_clock() {
    let fake_clock = FakeClock::new(1000); // start at t = 1.0s
    let mut cache = BoundedContentCache::with_clock(
        10 * 1024 * 1024,       // huge byte budget
        Duration::from_secs(3), // 3s time budget
        1024 * 1024,
        fake_clock.clone(),
    );

    let h = HeaderSnapshot::new(1, 0, "RAW", b"hdr".to_vec());
    cache.set_header(h).unwrap();

    // t = 1.0s: push c1 (pos 0, 100 bytes)
    let c1 = Content::new(
        1,
        Duration::from_secs(1),
        0,
        vec![0u8; 100],
        ContinuationFlags::NONE,
    );
    cache.push_body(c1, None).unwrap();
    assert_eq!(cache.body_len(), 1);

    // Advance clock to t = 2.5s: push c2 (pos 100, 100 bytes)
    fake_clock.advance(Duration::from_millis(1500));
    let c2 = Content::new(
        1,
        Duration::from_secs(2),
        100,
        vec![0u8; 100],
        ContinuationFlags::NONE,
    );
    cache.push_body(c2, None).unwrap();
    assert_eq!(cache.body_len(), 2);

    // Advance clock to t = 4.5s (c1 age = 3.5s > 3.0s window, c2 age = 2.0s <= 3.0s)
    fake_clock.advance(Duration::from_millis(2000));
    let c3 = Content::new(
        1,
        Duration::from_secs(4),
        200,
        vec![0u8; 100],
        ContinuationFlags::NONE,
    );
    cache.push_body(c3, None).unwrap();

    // c1 must be evicted due to time expiration; c2 and c3 remain
    assert_eq!(cache.body_len(), 2);
    let remaining_pos: Vec<i64> = cache.iter_body().map(|c| c.pos).collect();
    assert_eq!(remaining_pos, vec![100, 200]);
}

#[test]
fn test_byte_only_eviction_with_fake_clock() {
    let fake_clock = FakeClock::new(0);
    let mut cache = BoundedContentCache::with_clock(
        1000,                    // 1000 bytes budget
        Duration::from_secs(60), // huge time window
        1024 * 1024,
        fake_clock,
    );

    let h = HeaderSnapshot::new(1, 0, "RAW", b"hdr".to_vec());
    cache.set_header(h).unwrap();

    // Push 400 bytes
    let c1 = Content::new(
        1,
        Duration::from_secs(0),
        0,
        vec![0u8; 400],
        ContinuationFlags::NONE,
    );
    cache.push_body(c1, None).unwrap();

    // Push 400 bytes (total: 800 <= 1000)
    let c2 = Content::new(
        1,
        Duration::from_secs(1),
        400,
        vec![0u8; 400],
        ContinuationFlags::NONE,
    );
    cache.push_body(c2, None).unwrap();
    assert_eq!(cache.body_len(), 2);
    assert_eq!(cache.total_body_bytes(), 800);

    // Push 400 bytes (total would be 1200 > 1000 -> c1 evicted, total becomes 800)
    let c3 = Content::new(
        1,
        Duration::from_secs(2),
        800,
        vec![0u8; 400],
        ContinuationFlags::NONE,
    );
    cache.push_body(c3, None).unwrap();
    assert_eq!(cache.body_len(), 2);
    assert_eq!(cache.total_body_bytes(), 800);
    let remaining_pos: Vec<i64> = cache.iter_body().map(|c| c.pos).collect();
    assert_eq!(remaining_pos, vec![400, 800]);
}

#[test]
fn test_simultaneous_eviction_and_retained_allocation_accounting() {
    let fake_clock = FakeClock::new(0);
    let mut cache = BoundedContentCache::with_clock(
        2000,                   // 2000 bytes budget
        Duration::from_secs(3), // 3s time budget
        1024 * 1024,
        fake_clock.clone(),
    );

    let h = HeaderSnapshot::new(1, 0, "RAW", b"hdr".to_vec());
    cache.set_header(h).unwrap();

    // Push c1: 200 byte slice, but retains 1200 bytes of backing buffer (REQ-027)
    let c1 = Content::new(
        1,
        Duration::from_secs(0),
        0,
        vec![0u8; 200],
        ContinuationFlags::NONE,
    );
    cache.push_body(c1, Some(1200)).unwrap();
    assert_eq!(cache.total_body_bytes(), 1200);

    // Push c2: 200 byte slice, retains 500 bytes (total: 1700 <= 2000)
    let c2 = Content::new(
        1,
        Duration::from_secs(1),
        200,
        vec![0u8; 200],
        ContinuationFlags::NONE,
    );
    cache.push_body(c2, Some(500)).unwrap();
    assert_eq!(cache.total_body_bytes(), 1700);

    // Push c3: 200 byte slice, retains 500 bytes (total would be 2200 > 2000 -> c1 evicted!)
    let c3 = Content::new(
        1,
        Duration::from_secs(2),
        400,
        vec![0u8; 200],
        ContinuationFlags::NONE,
    );
    cache.push_body(c3, Some(500)).unwrap();
    assert_eq!(cache.total_body_bytes(), 1000); // 500 + 500
    assert_eq!(cache.body_len(), 2);
}

#[test]
fn test_continuity_gap_and_overlap_detection() {
    let fake_clock = FakeClock::new(0);
    let mut cache = BoundedContentCache::with_clock(
        8 * 1024 * 1024,
        Duration::from_secs(3),
        1024 * 1024,
        fake_clock,
    );

    let h = HeaderSnapshot::new(1, 0, "RAW", b"hdr".to_vec());
    cache.set_header(h).unwrap();

    // 1. First chunk at pos 0, length 100: contiguous
    let c1 = Content::new(
        1,
        Duration::from_secs(0),
        0,
        vec![0u8; 100],
        ContinuationFlags::NONE,
    );
    assert_eq!(
        cache.push_body(c1, None).unwrap(),
        PositionContinuity::Contiguous
    );

    // 2. Second chunk expected at pos 100, but arrives at pos 150 (Gap of 50 bytes)
    let c2_gap = Content::new(
        1,
        Duration::from_secs(1),
        150,
        vec![0u8; 100],
        ContinuationFlags::NONE,
    );
    assert_eq!(
        cache.push_body(c2_gap, None).unwrap(),
        PositionContinuity::Gap {
            expected: 100,
            actual: 150
        }
    );

    // 3. Third chunk expected at pos 250, but arrives at pos 200 (Overlap / retransmission of 50 bytes)
    let c3_overlap = Content::new(
        1,
        Duration::from_secs(2),
        200,
        vec![0u8; 100],
        ContinuationFlags::NONE,
    );
    assert_eq!(
        cache.push_body(c3_overlap, None).unwrap(),
        PositionContinuity::Overlap {
            expected: 250,
            actual: 200
        }
    );
}
