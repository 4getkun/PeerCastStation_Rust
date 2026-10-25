use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::content::Content;
use crate::errors::QuitReason;

/// Unique identifier for an active channel subscriber connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriberId(pub u64);

/// Resource and lag limits for a subscriber connection (REQ-013, REQ-028).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriberLimits {
    /// Maximum queued unconsumed payload bytes (default: 512 KiB = 524,288 bytes).
    pub max_bytes: usize,
    /// Maximum queued chunk count (default: 256).
    pub max_count: usize,
    /// Maximum time lag between oldest queued packet and newest packet (default: 5 seconds).
    pub max_lag: Duration,
}

impl Default for SubscriberLimits {
    fn default() -> Self {
        Self {
            max_bytes: 512 * 1024,
            max_count: 256,
            max_lag: Duration::from_secs(5),
        }
    }
}

/// Reason why a slow or stalled subscriber was evicted (REQ-028).
#[derive(Error, Debug, PartialEq, Eq, Clone, Copy)]
pub enum SubscriberDropReason {
    #[error("Byte queue limit exceeded ({current} > {max} bytes)")]
    ByteLimitExceeded { current: usize, max: usize },

    #[error("Message queue count limit exceeded ({current} > {max})")]
    CountLimitExceeded { current: usize, max: usize },

    #[error("Queue lag exceeded ({lag:?} > {max:?})")]
    LagExceeded { lag: Duration, max: Duration },

    #[error("Subscriber writer channel closed / disconnected")]
    ChannelClosed,
}

impl SubscriberDropReason {
    /// REQ-028: PCP output overflow maps to wire skip reason (QuitReason::SKIP = 1001).
    pub fn to_quit_reason(&self) -> QuitReason {
        QuitReason::SKIP
    }
}

/// Internal item transmitted through subscriber queue.
#[derive(Debug, Clone)]
pub struct SubscriberPacket {
    pub content: Content,
    pub enqueued_at: Duration,
    pub byte_size: usize,
}

/// Channel-side non-blocking admission handle for one subscriber (REQ-013).
pub struct SubscriberSink {
    id: SubscriberId,
    sender: mpsc::Sender<SubscriberPacket>,
    queued_bytes: Arc<AtomicUsize>,
    limits: SubscriberLimits,
    oldest_enqueued: Arc<std::sync::Mutex<Option<Duration>>>,
}

impl SubscriberSink {
    pub fn id(&self) -> SubscriberId {
        self.id
    }

    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes.load(Ordering::SeqCst)
    }

    /// Non-blocking admission check and dispatch (REQ-013).
    ///
    /// Never awaits the subscriber writer; fails immediately with `SubscriberDropReason`
    /// if byte budget, packet count, or lag window is exceeded.
    pub fn try_admit(&self, content: Content, now: Duration) -> Result<(), SubscriberDropReason> {
        let chunk_size = content.len();
        let current_bytes = self.queued_bytes.load(Ordering::SeqCst);

        // 1. Check byte budget (REQ-028)
        if current_bytes + chunk_size > self.limits.max_bytes {
            return Err(SubscriberDropReason::ByteLimitExceeded {
                current: current_bytes + chunk_size,
                max: self.limits.max_bytes,
            });
        }

        // 2. Check lag limit (REQ-025 table: 5s lag guard)
        if let Ok(mut oldest_lock) = self.oldest_enqueued.lock() {
            if let Some(oldest) = *oldest_lock {
                let lag = now.saturating_sub(oldest);
                if lag > self.limits.max_lag {
                    return Err(SubscriberDropReason::LagExceeded {
                        lag,
                        max: self.limits.max_lag,
                    });
                }
            } else {
                *oldest_lock = Some(now);
            }
        }

        // 3. Attempt nonblocking send
        let packet = SubscriberPacket {
            content,
            enqueued_at: now,
            byte_size: chunk_size,
        };

        match self.sender.try_send(packet) {
            Ok(()) => {
                self.queued_bytes.fetch_add(chunk_size, Ordering::SeqCst);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                Err(SubscriberDropReason::CountLimitExceeded {
                    current: self.limits.max_count,
                    max: self.limits.max_count,
                })
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(SubscriberDropReason::ChannelClosed),
        }
    }
}

/// The consumer end owned by a dedicated per-connection writer task (REQ-012, REQ-013).
///
/// Owns network awaits independently from the channel broadcast actor.
pub struct SubscriberSource {
    id: SubscriberId,
    receiver: mpsc::Receiver<SubscriberPacket>,
    queued_bytes: Arc<AtomicUsize>,
    oldest_enqueued: Arc<std::sync::Mutex<Option<Duration>>>,
}

impl SubscriberSource {
    pub fn id(&self) -> SubscriberId {
        self.id
    }

    /// Asynchronously waits for the next packet to write to network socket.
    ///
    /// Decrements atomic byte accounting as soon as packet is popped.
    pub async fn recv(&mut self) -> Option<Content> {
        let packet = self.receiver.recv().await?;
        self.queued_bytes
            .fetch_sub(packet.byte_size, Ordering::SeqCst);

        // Update oldest enqueued timestamp for remaining items
        // Note: For exact FIFO, parking_lot mutex is only locked during pops
        if let Ok(mut lock) = self.oldest_enqueued.lock() {
            if self.receiver.is_empty() {
                *lock = None;
            }
        }

        Some(packet.content)
    }

    /// Current unconsumed payload bytes in queue.
    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes.load(Ordering::SeqCst)
    }
}

impl Drop for SubscriberSource {
    fn drop(&mut self) {
        // Clear all remaining queued packets to release memory immediately
        while let Ok(pkt) = self.receiver.try_recv() {
            self.queued_bytes.fetch_sub(pkt.byte_size, Ordering::SeqCst);
        }
        if let Ok(mut lock) = self.oldest_enqueued.lock() {
            *lock = None;
        }
    }
}

/// Creates a new paired subscriber sink (for broadcaster) and source (for connection writer).
pub fn create_subscriber_pair(
    id: SubscriberId,
    limits: SubscriberLimits,
) -> (SubscriberSink, SubscriberSource) {
    let (tx, rx) = mpsc::channel(limits.max_count);
    let queued_bytes = Arc::new(AtomicUsize::new(0));
    let oldest_enqueued = Arc::new(std::sync::Mutex::new(None));

    let sink = SubscriberSink {
        id,
        sender: tx,
        queued_bytes: queued_bytes.clone(),
        limits,
        oldest_enqueued: oldest_enqueued.clone(),
    };

    let source = SubscriberSource {
        id,
        receiver: rx,
        queued_bytes,
        oldest_enqueued,
    };

    (sink, source)
}

/// Registry managing active subscribers and non-blocking O(N) fan-out (REQ-013).
#[derive(Default)]
pub struct SubscriberRegistry {
    subscribers: HashMap<SubscriberId, SubscriberSink>,
}

impl SubscriberRegistry {
    pub fn new() -> Self {
        Self {
            subscribers: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.subscribers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.subscribers.is_empty()
    }

    pub fn register(&mut self, sink: SubscriberSink) {
        self.subscribers.insert(sink.id(), sink);
    }

    pub fn unregister(&mut self, id: SubscriberId) -> Option<SubscriberSink> {
        self.subscribers.remove(&id)
    }

    /// Broadcasts a content chunk to all registered subscribers in O(N) time (REQ-013).
    ///
    /// Uses nonblocking `try_admit`. If a subscriber stalls or exceeds limits:
    /// - It does NOT stall healthy subscribers.
    /// - The failed subscriber is evicted from the registry.
    /// - The returned list contains `(SubscriberId, SubscriberDropReason)` for logging/cleanup.
    pub fn fanout(
        &mut self,
        content: &Content,
        now: Duration,
    ) -> Vec<(SubscriberId, SubscriberDropReason)> {
        let mut failed = Vec::new();

        self.subscribers.retain(|&id, sink| {
            match sink.try_admit(content.clone(), now) {
                Ok(()) => true,
                Err(reason) => {
                    failed.push((id, reason));
                    false // Remove slow/failed subscriber immediately
                }
            }
        });

        failed
    }
}
