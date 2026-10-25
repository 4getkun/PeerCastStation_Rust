use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use peercast_app::{AppLifecycleManager, AppMetrics, ShutdownReason};
use peercast_core::cache::{BoundedContentCache, FakeClock, HeaderSnapshot};
use peercast_core::content::{Content, ContinuationFlags};
use peercast_core::subscriber::{
    create_subscriber_pair, SubscriberDropReason, SubscriberId, SubscriberLimits,
};
use peercast_pcp::yellow_page::{YpBackoffTracker, INITIAL_RETRY_DELAY, MAX_RETRY_DELAY};

/// Acceptance criterion 1:
/// Predeclared soak thresholds and resource boundaries (REQ-003, REQ-070).
/// Verifies that peer churn does not cause task leaks or unbounded memory growth.
#[tokio::test]
async fn test_p43_peer_churn_and_zero_task_leak() {
    let lifecycle = AppLifecycleManager::new();
    lifecycle.mark_running().await;

    let tracker = lifecycle.task_tracker();
    let metrics = AppMetrics::new();

    let churn_cycles = 30;
    let successful_cycles = Arc::new(AtomicUsize::new(0));

    // Execute 30 rapid connection churn cycles
    for _cycle_idx in 0..churn_cycles {
        let conn_guard = metrics.track_connection();
        let succ = Arc::clone(&successful_cycles);

        // Spawn short-lived connection task
        let handle = tracker.spawn(async move {
            let _conn = conn_guard;
            // Simulate brief burst of traffic
            tokio::time::sleep(Duration::from_millis(2)).await;
            succ.fetch_add(1, Ordering::SeqCst);
        });

        // Await task completion
        handle.await.expect("churn task should complete");

        // Verify task and connection counters decrement immediately
        assert_eq!(metrics.snapshot(0).active_connections, 0);
        assert_eq!(tracker.active_tasks(), 0);
    }

    assert_eq!(
        successful_cycles.load(Ordering::SeqCst),
        churn_cycles,
        "all churn cycles must complete cleanly"
    );

    // Stop/block guard: Zero task leak after churn stops
    assert_eq!(
        tracker.active_tasks(),
        0,
        "active tasks must be strictly 0 after churn"
    );
}

/// Acceptance criterion 2:
/// Slow peer backpressure and lag-based eviction under continuous traffic (REQ-013, REQ-003).
/// Verifies slow reader does not block fast reader or cause unbounded memory queues.
#[tokio::test]
async fn test_p43_slow_peer_backpressure_and_lag_eviction() {
    let limits = SubscriberLimits {
        max_bytes: 4096,
        max_count: 5,
        max_lag: Duration::from_secs(5),
    };

    let (fast_sink, mut fast_source) = create_subscriber_pair(SubscriberId(1), limits);
    let (slow_sink, slow_source) = create_subscriber_pair(SubscriberId(2), limits);

    let mut slow_evicted = false;

    // Feed chunks into both subscribers
    for i in 0..20 {
        let content = Content {
            serial: i as i64,
            stream_id: 1,
            timestamp: Duration::from_millis(i * 100),
            pos: (i * 256) as i64,
            cont_flags: ContinuationFlags::NONE,
            data: Bytes::from(vec![0xAA; 256]),
        };

        // Fast subscriber: admit and immediately drain
        let fast_admit = fast_sink.try_admit(content.clone(), Duration::from_millis(i * 100));
        assert!(
            fast_admit.is_ok(),
            "fast subscriber must always be admitted"
        );
        let popped = fast_source.recv().await;
        assert!(popped.is_some(), "fast subscriber should drain popped item");

        // Slow subscriber: admit without draining until queue overflows
        let slow_admit = slow_sink.try_admit(content, Duration::from_millis(i * 100));
        if let Err(reason) = slow_admit {
            assert!(matches!(
                reason,
                SubscriberDropReason::CountLimitExceeded { .. }
                    | SubscriberDropReason::ByteLimitExceeded { .. }
            ));
            slow_evicted = true;
            break;
        }
    }

    assert!(
        slow_evicted,
        "slow subscriber must be evicted when bounds are exceeded"
    );
    // Slow subscriber queue must be strictly bounded
    assert!(slow_source.queued_bytes() <= 4096);
}

/// Acceptance criterion 3:
/// Source restart, continuity gap, and epoch transition soak (REQ-026, REQ-050).
/// Verifies content cache transitions cleanly when source restarts with new header.
#[test]
fn test_p43_source_restart_and_epoch_rollover() {
    let clock = FakeClock::new(1000);
    let mut cache = BoundedContentCache::with_clock(
        100_000, // 100 KB max body
        Duration::from_secs(30),
        50_000,
        clock,
    );

    // Stream Epoch 1
    let header1 = HeaderSnapshot::new(1, 0, "FLV", Bytes::from_static(b"FLV-HEADER-EPOCH-1"));
    cache.set_header(header1).expect("header 1 should succeed");

    for i in 0..20 {
        let chunk = Content {
            serial: i as i64,
            stream_id: 1,
            timestamp: Duration::from_millis(i * 50),
            pos: (i * 1000) as i64,
            cont_flags: ContinuationFlags::NONE,
            data: Bytes::from(vec![0x11; 1000]),
        };
        cache.push_body(chunk, None).expect("append epoch 1");
    }

    assert_eq!(cache.total_body_bytes(), 20_000);

    // Source restarts: Stream Epoch 2 arrives with new header
    let header2 = HeaderSnapshot::new(2, 0, "FLV", Bytes::from_static(b"FLV-HEADER-EPOCH-2"));
    cache.set_header(header2).expect("header 2 should succeed");

    // Old epoch body data is cleared/isolated per REQ-051; new body packets begin
    for i in 0..10 {
        let chunk = Content {
            serial: (20 + i) as i64,
            stream_id: 2,
            timestamp: Duration::from_millis(i * 50),
            pos: (i * 1000) as i64,
            cont_flags: ContinuationFlags::NONE,
            data: Bytes::from(vec![0x22; 1000]),
        };
        cache.push_body(chunk, None).expect("append epoch 2");
    }

    assert_eq!(cache.total_body_bytes(), 10_000);
    assert_eq!(cache.current_epoch(), Some(2));
    assert_eq!(
        cache.header().unwrap().data,
        Bytes::from_static(b"FLV-HEADER-EPOCH-2")
    );
}

/// Acceptance criterion 4:
/// Yellow Pages intermittent failure, exponential backoff, and recovery (REQ-062, REQ-070).
#[test]
fn test_p43_yellow_page_backoff_and_recovery() {
    let mut backoff = YpBackoffTracker::new(INITIAL_RETRY_DELAY, MAX_RETRY_DELAY);

    // Initial state
    assert_eq!(backoff.current_delay(), INITIAL_RETRY_DELAY);
    assert_eq!(backoff.attempt_count(), 0);

    // Consecutive failures induce exponential backoff up to cap
    backoff.record_failure();
    assert!(backoff.current_delay() >= INITIAL_RETRY_DELAY);
    assert_eq!(backoff.attempt_count(), 1);

    for _ in 0..10 {
        backoff.record_failure();
    }
    assert_eq!(backoff.current_delay(), MAX_RETRY_DELAY);
    assert_eq!(backoff.attempt_count(), 11);

    // Recovery resets backoff immediately to initial delay
    backoff.reset_success();
    assert_eq!(backoff.current_delay(), INITIAL_RETRY_DELAY);
    assert_eq!(backoff.attempt_count(), 0);
}

/// Acceptance criterion 5:
/// Full graceful shutdown during active high-throughput fan-out (REQ-014, REQ-067, stop/block guard).
/// Verifies zero residual background tasks or active connections post-shutdown.
#[tokio::test]
async fn test_p43_graceful_shutdown_high_traffic_zero_residual() {
    let lifecycle = AppLifecycleManager::new();
    lifecycle.mark_running().await;

    let tracker = lifecycle.task_tracker();
    let metrics = AppMetrics::new();
    let shutdown_rx = lifecycle.subscribe_shutdown();

    let worker_count = 10;
    let stop_acknowledged = Arc::new(AtomicUsize::new(0));

    // Spawn 10 concurrent active streaming workers
    for _worker_id in 0..worker_count {
        let mut rx = shutdown_rx.resubscribe();
        let conn = metrics.track_connection();
        let ack = Arc::clone(&stop_acknowledged);

        tracker.spawn(async move {
            let _conn = conn;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(5)) => {
                        // Ongoing streaming traffic
                    }
                    _ = rx.recv() => {
                        // Received shutdown broadcast
                        ack.fetch_add(1, Ordering::SeqCst);
                        break;
                    }
                }
            }
        });
    }

    assert_eq!(tracker.active_tasks(), worker_count);
    assert_eq!(metrics.snapshot(0).active_connections, worker_count);

    // Trigger administrative shutdown during active streaming
    lifecycle.request_shutdown(ShutdownReason::AdminRpc).await;

    // Await complete drain with timeout
    let drain_res = lifecycle.complete_shutdown(Duration::from_secs(3)).await;
    assert!(
        drain_res.is_ok(),
        "all workers must drain cleanly within timeout"
    );

    // Stop/block guard: Zero active tasks, zero active connections, zero zombie threads
    assert_eq!(
        tracker.active_tasks(),
        0,
        "residual active tasks must be strictly 0"
    );
    assert_eq!(
        metrics.snapshot(0).active_connections,
        0,
        "residual active connections must be strictly 0"
    );
    assert_eq!(
        stop_acknowledged.load(Ordering::SeqCst),
        worker_count,
        "all workers must have gracefully acknowledged shutdown"
    );
}
