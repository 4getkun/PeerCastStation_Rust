use std::collections::VecDeque;
use std::time::Duration;

use crate::content::Content;

/// Bounded sliding-window buffer for channel media content (REQ-025, REQ-027).
///
/// Enforces dual simultaneous constraints:
/// - Maximum byte budget (default: 8 MiB = 8,388,608 bytes)
/// - Maximum time window (default: 3 seconds)
/// - Stream generation invalidation: when a newer `stream_id` arrives,
///   stale packets from prior generations are immediately evicted.
#[derive(Debug, Clone)]
pub struct ContentRingBuffer {
    items: VecDeque<Content>,
    total_bytes: usize,
    max_bytes: usize,
    max_time_window: Duration,
    next_serial: i64,
}

impl Default for ContentRingBuffer {
    fn default() -> Self {
        Self::new(8 * 1024 * 1024, Duration::from_secs(3))
    }
}

impl ContentRingBuffer {
    /// Creates a new buffer with specified byte and duration caps.
    pub fn new(max_bytes: usize, max_time_window: Duration) -> Self {
        Self {
            items: VecDeque::new(),
            total_bytes: 0,
            max_bytes,
            max_time_window,
            next_serial: 0,
        }
    }

    /// Pushes a new content chunk, assigning it a monotonic serial and evicting
    /// entries exceeding the time window, byte budget, or from prior stream epochs.
    pub fn push(&mut self, mut item: Content) {
        if let Some(newest) = self.items.back() {
            if item.stream_id > newest.stream_id {
                // Strictly newer stream generation: immediately purge prior epoch contents
                self.clear();
            } else if item.stream_id < newest.stream_id {
                // Stale chunk from older stream generation: ignore and do not store
                return;
            }
        }

        self.next_serial += 1;
        item.serial = self.next_serial;

        self.total_bytes += item.len();
        self.items.push_back(item);

        self.evict();
    }

    /// Evicts expired or oversized items according to simultaneous constraints.
    fn evict(&mut self) {
        let newest = match self.items.back() {
            Some(c) => (c.stream_id, c.timestamp),
            None => return,
        };

        while let Some(front) = self.items.front() {
            let stale_stream = front.stream_id < newest.0;
            let expired_time = (front.stream_id == newest.0)
                && newest.1.saturating_sub(front.timestamp) > self.max_time_window;
            let over_budget = self.total_bytes > self.max_bytes;

            if stale_stream || expired_time || over_budget {
                if let Some(removed) = self.items.pop_front() {
                    self.total_bytes = self.total_bytes.saturating_sub(removed.len());
                }
            } else {
                break;
            }
        }
    }

    /// Returns the newest content chunk, if any.
    pub fn newest(&self) -> Option<&Content> {
        self.items.back()
    }

    /// Returns the oldest content chunk, if any.
    pub fn oldest(&self) -> Option<&Content> {
        self.items.front()
    }

    /// Returns an iterator over all buffered contents.
    pub fn iter(&self) -> impl Iterator<Item = &Content> {
        self.items.iter()
    }

    /// Finds content starting from a requested position or timestamp.
    ///
    /// Selects the first chunk matching or immediately succeeding `(stream_id, timestamp, pos)`.
    pub fn find_from(&self, stream_id: i32, timestamp: Duration, pos: i64) -> impl Iterator<Item = &Content> {
        self.items.iter().filter(move |c| {
            if c.stream_id != stream_id {
                c.stream_id > stream_id
            } else if c.timestamp != timestamp {
                c.timestamp >= timestamp
            } else {
                c.pos >= pos
            }
        })
    }

    /// Number of chunks currently stored.
    #[inline]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns true if the buffer has no contents.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Total payload bytes currently held in the buffer.
    #[inline]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Clears all buffered chunks and resets byte accounting.
    pub fn clear(&mut self) {
        self.items.clear();
        self.total_bytes = 0;
    }
}
