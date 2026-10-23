use peercast_core::content::{Content, ContinuationFlags};
use peercast_core::errors::QuitReason;
use peercast_core::ringbuffer::ContentRingBuffer;
use std::time::Duration;

#[test]
fn test_quit_reason_preserves_raw_codes() {
    assert_eq!(QuitReason::NORMAL.raw(), 1000);
    assert_eq!(QuitReason::SKIP.raw(), 1001);
    assert_eq!(QuitReason::ALREADY_CONNECTED.raw(), 1002);
    assert_eq!(QuitReason::UNAVAILABLE.raw(), 1003);
    assert_eq!(QuitReason::READ_ERROR.raw(), 3000);
    assert_eq!(QuitReason::WRITE_ERROR.raw(), 4000);

    // Custom or unknown code
    let custom = QuitReason::from_raw(4042);
    assert_eq!(custom.raw(), 4042);
    assert_eq!(custom.description(), "Unknown quit code");
}

#[test]
fn test_content_ringbuffer_sliding_window_eviction() {
    // 1000 bytes max, 3 seconds max
    let mut rb = ContentRingBuffer::new(1000, Duration::from_secs(3));

    // Push chunk at t=0s, 300 bytes
    let c1 = Content::new(1, Duration::from_secs(0), 0, vec![0u8; 300], ContinuationFlags::NONE);
    rb.push(c1);
    assert_eq!(rb.len(), 1);
    assert_eq!(rb.total_bytes(), 300);

    // Push chunk at t=2s, 300 bytes
    let c2 = Content::new(1, Duration::from_secs(2), 300, vec![0u8; 300], ContinuationFlags::NONE);
    rb.push(c2);
    assert_eq!(rb.len(), 2);
    assert_eq!(rb.total_bytes(), 600);

    // Push chunk at t=4s (> 3s from t=0), 300 bytes
    // t=4s minus t=0s is 4s (> 3s window) -> c1 must be evicted!
    let c3 = Content::new(1, Duration::from_secs(4), 600, vec![0u8; 300], ContinuationFlags::NONE);
    rb.push(c3);
    assert_eq!(rb.len(), 2); // c1 evicted, c2 and c3 remain
    assert_eq!(rb.oldest().unwrap().pos, 300);
    assert_eq!(rb.total_bytes(), 600);

    // Push chunk exceeding byte budget: 600 bytes
    // Total would be 600 + 600 = 1200 (> 1000 budget) -> c2 must be evicted!
    let c4 = Content::new(1, Duration::from_secs(4), 900, vec![0u8; 600], ContinuationFlags::NONE);
    rb.push(c4);
    assert_eq!(rb.len(), 2); // c3 (300) + c4 (600) = 900 <= 1000
    assert_eq!(rb.oldest().unwrap().pos, 600);
    assert_eq!(rb.total_bytes(), 900);

    // Invalidation on stream epoch change: stream_id changes from 1 to 2
    let c_new_epoch = Content::new(2, Duration::from_secs(0), 0, vec![0u8; 100], ContinuationFlags::NONE);
    rb.push(c_new_epoch);
    assert_eq!(rb.len(), 1); // all stream 1 chunks evicted
    assert_eq!(rb.oldest().unwrap().stream_id, 2);
    assert_eq!(rb.total_bytes(), 100);
}
