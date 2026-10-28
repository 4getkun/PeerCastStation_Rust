use crate::youtube::{ProbeData, ProbeRequest};
use futures_util::{stream, StreamExt};
use std::future::Future;

// Each provider has its own worker, limiting the total to eight helper processes.
const MAX_CONCURRENT_PROBES: usize = 4;

pub async fn run_probe_batch<P, F, C>(requests: Vec<ProbeRequest>, probe: P, complete: C)
where
    P: Fn(ProbeRequest) -> F,
    F: Future<Output = Result<ProbeData, String>>,
    C: Fn(String, Result<ProbeData, String>),
{
    let complete = &complete;
    stream::iter(requests)
        .for_each_concurrent(MAX_CONCURRENT_PROBES, |request| {
            let id = request.id.clone();
            let result = probe(request);
            async move { complete(id, result.await) }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Duration};
    use tokio::sync::{mpsc, Semaphore};

    fn request(id: &str) -> ProbeRequest {
        ProbeRequest {
            id: id.into(),
            url: String::new(),
        }
    }

    #[tokio::test]
    async fn fast_live_result_is_reported_before_slow_probe() {
        let gate = Arc::new(Semaphore::new(0));
        let (events, mut received) = mpsc::unbounded_channel();
        let worker_gate = gate.clone();
        let worker = tokio::spawn(async move {
            run_probe_batch(
                vec![request("slow"), request("live")],
                |request| {
                    let gate = worker_gate.clone();
                    async move {
                        if request.id == "slow" {
                            let _permit = gate.acquire().await.unwrap();
                        }
                        Err("fixture".into())
                    }
                },
                |id, _| {
                    events.send(id).unwrap();
                },
            )
            .await;
        });
        let first = tokio::time::timeout(Duration::from_millis(500), received.recv()).await;
        gate.add_permits(1);
        worker.await.unwrap();
        assert_eq!(
            first
                .expect("A slow probe must not delay a ready live result")
                .as_deref(),
            Some("live")
        );
        assert_eq!(received.recv().await.as_deref(), Some("slow"));
    }

    #[tokio::test]
    async fn probes_are_bounded_and_every_result_is_completed_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let gate = Arc::new(Semaphore::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (started, mut starts) = mpsc::unbounded_channel();
        let (completed, mut results) = mpsc::unbounded_channel();
        let worker_gate = gate.clone();
        let worker_peak = peak.clone();
        let worker = tokio::spawn(async move {
            run_probe_batch(
                (0..8).map(|n| request(&n.to_string())).collect(),
                |request| {
                    let (gate, active, peak, started) = (
                        worker_gate.clone(),
                        active.clone(),
                        worker_peak.clone(),
                        started.clone(),
                    );
                    async move {
                        let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(count, Ordering::SeqCst);
                        started.send(request.id).unwrap();
                        let _permit = gate.acquire().await.unwrap();
                        active.fetch_sub(1, Ordering::SeqCst);
                        Err("fixture".into())
                    }
                },
                |id, _| {
                    completed.send(id).unwrap();
                },
            )
            .await;
        });
        for _ in 0..MAX_CONCURRENT_PROBES {
            tokio::time::timeout(Duration::from_secs(2), starts.recv())
                .await
                .unwrap()
                .unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), starts.recv())
                .await
                .is_err()
        );
        gate.add_permits(8);
        worker.await.unwrap();
        assert_eq!(peak.load(Ordering::SeqCst), MAX_CONCURRENT_PROBES);
        let mut ids = Vec::new();
        while let Ok(id) = results.try_recv() {
            ids.push(id);
        }
        ids.sort();
        assert_eq!(ids, (0..8).map(|n| n.to_string()).collect::<Vec<_>>());
    }
}
