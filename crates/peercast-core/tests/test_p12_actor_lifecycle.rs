use peercast_core::actor::{
    create_actor_pair, ActorEnvelope, ActorError, GenerationToken, WorkerPhase,
};
use std::time::Duration;
use tokio::time::sleep;

#[derive(Debug, PartialEq, Eq)]
enum TestCmd {
    Ping(u32),
    Work(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TestStatus {
    processed_count: u32,
    last_ping: u32,
}

#[tokio::test]
async fn test_post_while_worker_runs_without_lock_across_await() {
    let gen = GenerationToken(1);
    let (handle, mut ctx) = create_actor_pair::<TestCmd, TestStatus>(16, gen);

    // Spawn the worker task with single-ownership of ctx
    let worker_handle = tokio::spawn(async move {
        ctx.set_phase(WorkerPhase::Running);
        while let Some(envelope) = ctx.recv().await {
            match envelope {
                ActorEnvelope::Custom { cmd, .. } => match cmd {
                    TestCmd::Ping(val) => {
                        ctx.update_custom(|s| {
                            s.processed_count += 1;
                            s.last_ping = val;
                        });
                    }
                    TestCmd::Work(_) => {
                        ctx.update_custom(|s| s.processed_count += 1);
                    }
                },
                ActorEnvelope::Stop { .. } => {
                    ctx.set_phase(WorkerPhase::Stopping);
                    break;
                }
            }
        }
        ctx.set_phase(WorkerPhase::Stopped);
    });

    // Multiple cloned handles post commands concurrently
    let h1 = handle.clone();
    let h2 = handle.clone();

    let t1 = tokio::spawn(async move {
        for i in 0..10 {
            h1.post(TestCmd::Ping(i)).await.unwrap();
        }
    });

    let t2 = tokio::spawn(async move {
        for _ in 0..10 {
            h2.post(TestCmd::Work("data".into())).await.unwrap();
        }
    });

    t1.await.unwrap();
    t2.await.unwrap();

    // Verify worker processed all 20 commands
    // Wait briefly for worker queue to drain
    for _ in 0..20 {
        if handle.status().custom.processed_count == 20 {
            break;
        }
        sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(handle.status().custom.processed_count, 20);
    assert_eq!(handle.phase(), WorkerPhase::Running);

    // Stop worker and join
    handle.stop().await.unwrap();
    worker_handle.await.unwrap();
    assert_eq!(handle.phase(), WorkerPhase::Stopped);
}

#[tokio::test]
async fn test_stale_generation_events_ignored() {
    let active_gen = GenerationToken(2);
    let stale_gen = GenerationToken(1);

    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let (st_tx, _st_rx) = tokio::sync::watch::channel(peercast_core::actor::WorkerStatus::<TestStatus>::new_initial(active_gen));
    let mut ctx = peercast_core::actor::WorkerContext::new(rx, st_tx, active_gen);

    // 1. Send stale envelope (Gen 1)
    tx.send(ActorEnvelope::Custom {
        cmd: TestCmd::Ping(999),
        generation: stale_gen,
    }).await.unwrap();

    // 2. Send active envelope (Gen 2)
    tx.send(ActorEnvelope::Custom {
        cmd: TestCmd::Ping(42),
        generation: active_gen,
    }).await.unwrap();

    // 3. Send another stale envelope (Gen 1)
    tx.send(ActorEnvelope::Custom {
        cmd: TestCmd::Work("obsolete".into()),
        generation: stale_gen,
    }).await.unwrap();

    // 4. Send active stop envelope (Gen 2)
    tx.send(ActorEnvelope::Stop {
        generation: active_gen,
    }).await.unwrap();

    // Context MUST ignore both Gen 1 envelopes and only return Gen 2 envelopes!
    let first = ctx.recv().await.unwrap();
    assert_eq!(first.generation(), active_gen);
    assert!(matches!(first, ActorEnvelope::Custom { cmd: TestCmd::Ping(42), .. }));

    let second = ctx.recv().await.unwrap();
    assert_eq!(second.generation(), active_gen);
    assert!(matches!(second, ActorEnvelope::Stop { .. }));
}

#[tokio::test]
async fn test_idempotent_double_stop() {
    let gen = GenerationToken(1);
    let (handle, mut ctx) = create_actor_pair::<TestCmd, TestStatus>(16, gen);

    let worker = tokio::spawn(async move {
        ctx.set_phase(WorkerPhase::Running);
        while let Some(envelope) = ctx.recv().await {
            if matches!(envelope, ActorEnvelope::Stop { .. }) {
                ctx.set_phase(WorkerPhase::Stopped);
                break;
            }
        }
    });

    // First stop
    assert!(handle.stop().await.is_ok());

    // Second stop (must be idempotent and return Ok)
    assert!(handle.stop().await.is_ok());

    // Non-blocking stop_nowait must also return Ok
    assert!(handle.stop_nowait().is_ok());

    worker.await.unwrap();
    assert_eq!(handle.phase(), WorkerPhase::Stopped);
}

#[tokio::test]
async fn test_shutdown_during_startup() {
    let gen = GenerationToken(1);
    let (handle, mut ctx) = create_actor_pair::<TestCmd, TestStatus>(16, gen);

    // Initial phase is Idle
    assert_eq!(handle.phase(), WorkerPhase::Idle);

    // Stop while in Starting phase
    ctx.set_phase(WorkerPhase::Starting);
    assert_eq!(handle.phase(), WorkerPhase::Starting);

    // Handle issues stop during startup
    handle.stop().await.unwrap();

    // Worker receives stop during its initialization
    let received = ctx.recv().await.unwrap();
    assert!(matches!(received, ActorEnvelope::Stop { .. }));

    ctx.set_phase(WorkerPhase::Stopped);
    assert_eq!(handle.phase(), WorkerPhase::Stopped);
}

#[tokio::test]
async fn test_bounded_queue_backpressure() {
    let gen = GenerationToken(1);
    // Channel with capacity 2
    let (handle, _ctx) = create_actor_pair::<TestCmd, TestStatus>(2, gen);

    assert_eq!(handle.try_post(TestCmd::Ping(1)), Ok(()));
    assert_eq!(handle.try_post(TestCmd::Ping(2)), Ok(()));

    // Third try_post must fail with QueueFull
    assert_eq!(handle.try_post(TestCmd::Ping(3)), Err(ActorError::QueueFull));
}
