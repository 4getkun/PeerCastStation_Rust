use bytes::Bytes;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

use crate::content::Content;

/// Errors produced by cache bounds or epoch invalidation (REQ-027, REQ-051).
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum CacheError {
    #[error("Header exceeds maximum allowed budget ({actual} > {max} bytes)")]
    HeaderTooLarge { actual: usize, max: usize },

    #[error("Stale body chunk from epoch {chunk_epoch} rejected (current epoch is {current_epoch})")]
    StaleEpoch {
        chunk_epoch: i32,
        current_epoch: i32,
    },

    #[error("No active header initialized before pushing body data")]
    MissingHeader,
}

/// Abstract clock for cache time eviction, allowing deterministic fake clocks in tests.
pub trait CacheClock: Send + Sync {
    fn now(&self) -> Duration;
}

/// Monotonic wall-time clock implementation.
#[derive(Debug, Clone)]
pub struct SystemCacheClock {
    start: std::time::Instant,
}

impl Default for SystemCacheClock {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemCacheClock {
    pub fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }
}

impl CacheClock for SystemCacheClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Deterministic fake clock controllable in tests (P13 acceptance criteria).
#[derive(Debug, Clone)]
pub struct FakeClock {
    millis: Arc<AtomicU64>,
}

impl FakeClock {
    pub fn new(initial_millis: u64) -> Self {
        Self {
            millis: Arc::new(AtomicU64::new(initial_millis)),
        }
    }

    pub fn advance(&self, duration: Duration) {
        self.millis
            .fetch_add(duration.as_millis() as u64, Ordering::SeqCst);
    }

    pub fn set(&self, duration: Duration) {
        self.millis
            .store(duration.as_millis() as u64, Ordering::SeqCst);
    }
}

impl CacheClock for FakeClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.millis.load(Ordering::SeqCst))
    }
}

/// A bounded header snapshot with epoch, position and container identity (REQ-051).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderSnapshot {
    /// Stream generation/epoch number.
    pub epoch: i32,
    /// Stream byte position when this header was established.
    pub pos: i64,
    /// Container/codec identity string (e.g. "FLV", "TS", "ASF", "RAW").
    pub container_type: String,
    /// Raw header bytes.
    pub data: Bytes,
    /// Accounted allocation cost (accounts for retained backing allocation, REQ-027).
    pub retained_bytes: usize,
}

impl HeaderSnapshot {
    pub fn new(
        epoch: i32,
        pos: i64,
        container_type: impl Into<String>,
        data: impl Into<Bytes>,
    ) -> Self {
        let b = data.into();
        let len = b.len();
        Self {
            epoch,
            pos,
            container_type: container_type.into(),
            data: b,
            retained_bytes: len,
        }
    }

    pub fn with_retained_bytes(mut self, retained: usize) -> Self {
        self.retained_bytes = retained.max(self.data.len());
        self
    }
}

/// Metadata tracked for each cached body chunk.
#[derive(Debug, Clone)]
struct CachedItem {
    content: Content,
    enqueued_at: Duration,
    retained_bytes: usize,
}

/// Position continuity tracking between consecutive packets (REQ-051).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionContinuity {
    Contiguous,
    Gap { expected: i64, actual: i64 },
    Overlap { expected: i64, actual: i64 },
}

/// Bounded channel content cache enforcing epochs, separate header budget,
/// and simultaneous byte and time eviction (REQ-027, REQ-050, REQ-051).
pub struct BoundedContentCache<C: CacheClock = SystemCacheClock> {
    header: Option<HeaderSnapshot>,
    body_items: VecDeque<CachedItem>,
    total_body_retained: usize,
    max_body_bytes: usize,
    max_body_duration: Duration,
    max_header_bytes: usize,
    clock: C,
    last_end_pos: Option<i64>,
}

impl BoundedContentCache<SystemCacheClock> {
    pub fn new(max_body_bytes: usize, max_body_duration: Duration, max_header_bytes: usize) -> Self {
        Self::with_clock(
            max_body_bytes,
            max_body_duration,
            max_header_bytes,
            SystemCacheClock::new(),
        )
    }
}

impl<C: CacheClock> BoundedContentCache<C> {
    pub fn with_clock(
        max_body_bytes: usize,
        max_body_duration: Duration,
        max_header_bytes: usize,
        clock: C,
    ) -> Self {
        Self {
            header: None,
            body_items: VecDeque::new(),
            total_body_retained: 0,
            max_body_bytes,
            max_body_duration,
            max_header_bytes,
            clock,
            last_end_pos: None,
        }
    }

    /// Sets or updates the active header snapshot (REQ-051).
    ///
    /// If the header exceeds `max_header_bytes`, returns `Err(CacheError::HeaderTooLarge)`.
    /// On restart or changed epoch/initialization, immediately invalidates and purges
    /// all cached body data from prior epochs.
    pub fn set_header(&mut self, header: HeaderSnapshot) -> Result<(), CacheError> {
        let actual_size = header.retained_bytes;
        if actual_size > self.max_header_bytes {
            return Err(CacheError::HeaderTooLarge {
                actual: actual_size,
                max: self.max_header_bytes,
            });
        }

        let is_new_epoch = self
            .header
            .as_ref()
            .map(|h| h.epoch != header.epoch || h.data != header.data)
            .unwrap_or(true);

        if is_new_epoch {
            // REQ-051: Invalidate incompatible cached body data
            self.clear_body();
            self.last_end_pos = Some(header.pos);
        }

        self.header = Some(header);
        Ok(())
    }

    /// Returns current active header snapshot, if any.
    pub fn header(&self) -> Option<&HeaderSnapshot> {
        self.header.as_ref()
    }

    /// Returns current stream epoch, if an active header is present.
    pub fn current_epoch(&self) -> Option<i32> {
        self.header.as_ref().map(|h| h.epoch)
    }

    /// Appends a new body chunk, verifying stream epoch and tracking continuity.
    ///
    /// Also allows explicit `retained_bytes` accounting for larger backing allocations.
    pub fn push_body(
        &mut self,
        content: Content,
        retained_bytes: Option<usize>,
    ) -> Result<PositionContinuity, CacheError> {
        let current_epoch = match self.header.as_ref() {
            Some(h) => h.epoch,
            None => return Err(CacheError::MissingHeader),
        };

        if content.stream_id != current_epoch {
            return Err(CacheError::StaleEpoch {
                chunk_epoch: content.stream_id,
                current_epoch,
            });
        }

        // Track continuity
        let continuity = match self.last_end_pos {
            Some(expected) => {
                if content.pos == expected {
                    PositionContinuity::Contiguous
                } else if content.pos > expected {
                    PositionContinuity::Gap {
                        expected,
                        actual: content.pos,
                    }
                } else {
                    PositionContinuity::Overlap {
                        expected,
                        actual: content.pos,
                    }
                }
            }
            None => PositionContinuity::Contiguous,
        };

        let chunk_len = content.len() as i64;
        self.last_end_pos = Some(content.pos.saturating_add(chunk_len));

        let retained = retained_bytes
            .unwrap_or_else(|| content.len())
            .max(content.len());
        let now = self.clock.now();

        self.total_body_retained += retained;
        self.body_items.push_back(CachedItem {
            content,
            enqueued_at: now,
            retained_bytes: retained,
        });

        self.evict(now);
        Ok(continuity)
    }

    /// Evicts body items under simultaneous time and byte limits (REQ-027).
    fn evict(&mut self, now: Duration) {
        while let Some(front) = self.body_items.front() {
            let expired = now.saturating_sub(front.enqueued_at) > self.max_body_duration;
            let over_budget = self.total_body_retained > self.max_body_bytes;

            if expired || over_budget {
                if let Some(removed) = self.body_items.pop_front() {
                    self.total_body_retained = self
                        .total_body_retained
                        .saturating_sub(removed.retained_bytes);
                }
            } else {
                break;
            }
        }
    }

    /// Clears all body chunks and resets position tracking.
    pub fn clear_body(&mut self) {
        self.body_items.clear();
        self.total_body_retained = 0;
        self.last_end_pos = None;
    }

    /// Number of body chunks currently retained.
    #[inline]
    pub fn body_len(&self) -> usize {
        self.body_items.len()
    }

    /// Total retained allocation bytes of body chunks.
    #[inline]
    pub fn total_body_bytes(&self) -> usize {
        self.total_body_retained
    }

    /// Iterates over all cached body chunks in FIFO order.
    pub fn iter_body(&self) -> impl Iterator<Item = &Content> {
        self.body_items.iter().map(|item| &item.content)
    }

    /// Finds first body chunk matching or following `pos`.
    pub fn find_from_pos(&self, pos: i64) -> impl Iterator<Item = &Content> {
        self.body_items
            .iter()
            .map(|item| &item.content)
            .filter(move |c| c.pos >= pos)
    }
}
