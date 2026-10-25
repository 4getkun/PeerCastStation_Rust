use peercast_core::content::{Content, ContinuationFlags};
use peercast_core::errors::QuitReason;
use peercast_core::subscriber::{
    create_subscriber_pair, SubscriberDropReason, SubscriberId, SubscriberLimits,
    SubscriberRegistry,
};
use std::time::Duration;

#[tokio::test]
async fn test_stalled_receiver_cannot_stop_healthy_receiver() {
    let mut registry = SubscriberRegistry::new();

    // 1. Healthy subscriber with small queue limits (max 500 bytes, max 10 packets)
    let limits_healthy = SubscriberLimits {
        max_bytes: 1000,
        max_count: 10,
        max_lag: Duration::from_secs(5),
    };
    let (sink_healthy, mut source_healthy) =
        create_subscriber_pair(SubscriberId(1), limits_healthy);
    registry.register(sink_healthy);

    // 2. Stalled subscriber with tiny byte budget (max 250 bytes)
    let limits_stalled = SubscriberLimits {
        max_bytes: 250,
        max_count: 5,
        max_lag: Duration::from_secs(5),
    };
    let (sink_stalled, source_stalled) = create_subscriber_pair(SubscriberId(2), limits_stalled);
    registry.register(sink_stalled);

    // Healthy reader task drains packets concurrently
    let drain_healthy = tokio::spawn(async move {
        let mut count = 0;
        while let Some(_pkt) = source_healthy.recv().await {
            count += 1;
            if count == 5 {
                break;
            }
        }
        count
    });

    // Stalled reader does NOT read from source_stalled!

    // Broadcast 5 packets (100 bytes each) at t=0s
    let mut all_failed = Vec::new();
    for i in 0..5 {
        let content = Content::new(
            1,
            Duration::from_millis(i * 100),
            (i * 100) as i64,
            vec![0u8; 100],
            ContinuationFlags::NONE,
        );
        let failed = registry.fanout(&content, Duration::from_secs(0));
        all_failed.extend(failed);
    }

    // Healthy reader must receive all 5 packets
    let received_healthy = drain_healthy.await.unwrap();
    assert_eq!(received_healthy, 5);

    // Stalled subscriber must have been evicted due to byte overflow
    assert_eq!(all_failed.len(), 1);
    let (failed_id, reason) = &all_failed[0];
    assert_eq!(*failed_id, SubscriberId(2));
    assert!(matches!(
        reason,
        SubscriberDropReason::ByteLimitExceeded { current, max } if *current == 300 && *max == 250
    ));
    assert_eq!(reason.to_quit_reason(), QuitReason::SKIP);

    // Stalled subscriber is now removed from registry; only healthy subscriber remains
    assert_eq!(registry.len(), 1);

    // Verify memory cleanup when source_stalled is dropped
    assert_eq!(source_stalled.queued_bytes(), 200); // had 2 packets before failing 3rd
    drop(source_stalled); // Dropping releases remaining buffered memory
}

#[tokio::test]
async fn test_lag_based_eviction() {
    let mut registry = SubscriberRegistry::new();

    let limits = SubscriberLimits {
        max_bytes: 10 * 1024,
        max_count: 100,
        max_lag: Duration::from_secs(5), // 5s lag limit
    };
    let (sink, _source) = create_subscriber_pair(SubscriberId(10), limits);
    registry.register(sink);

    // Packet 1 admitted at t = 1.0s
    let c1 = Content::new(
        1,
        Duration::from_secs(0),
        0,
        vec![0u8; 50],
        ContinuationFlags::NONE,
    );
    let failed1 = registry.fanout(&c1, Duration::from_secs(1));
    assert!(failed1.is_empty());

    // Packet 2 at t = 7.0s (> 5.0s lag from Packet 1)
    let c2 = Content::new(
        1,
        Duration::from_secs(1),
        50,
        vec![0u8; 50],
        ContinuationFlags::NONE,
    );
    let failed2 = registry.fanout(&c2, Duration::from_secs(7));

    assert_eq!(failed2.len(), 1);
    let (failed_id, reason) = &failed2[0];
    assert_eq!(*failed_id, SubscriberId(10));
    assert!(matches!(
        reason,
        SubscriberDropReason::LagExceeded { lag, max } if *lag == Duration::from_secs(6) && *max == Duration::from_secs(5)
    ));
    assert_eq!(reason.to_quit_reason(), QuitReason::SKIP);
    assert_eq!(registry.len(), 0);
}
