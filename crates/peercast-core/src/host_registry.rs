//! Servent host registry, freshness tracking, and failure cooldown tables (REQ-042).
//!
//! # Architectural Invariants (REQ-042)
//! 1. **Separation of Concepts**: Host freshness tracking and endpoint failure cooldown are
//!    fundamentally distinct concerns with independent monotonic timers and thresholds.
//!    Station checks node age (default 180s) in `Channel`, while it tracks ignored failed
//!    sources (default 180s) in `PCPSourceStream`.
//! 2. **No Accidental Reset**: Freshness updates (e.g. from gossip or discovery) MUST NEVER
//!    clear or shorten an active failure cooldown. Conversely, an endpoint failure cooldown
//!    MUST NEVER modify or refresh host freshness or node age.
//! 3. **Bounded Resources (Global Rule 3)**: Both host registries and cooldown tables enforce
//!    strict upper bounds (`max_hosts`, `max_entries`). Memory is kept bounded through
//!    insertion pruning, FIFO/LRU eviction at capacity, and scheduled physical cleanup.
//! 4. **Deterministic Testing**: All time queries go through a monotonic clock abstraction
//!    ([`CacheClock`]), enabling zero-sleep, fully deterministic verification with [`FakeClock`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::cache::{CacheClock, SystemCacheClock};
use crate::channel_id::ChannelId;
use crate::host::Host;

/// Default node freshness threshold (Station's `NodeLimit` = 180,000 ms).
pub const DEFAULT_FRESHNESS_THRESHOLD: Duration = Duration::from_secs(180);

/// Default failed endpoint cooldown duration (Station's `ignoredTime` = 180,000 ms).
pub const DEFAULT_COOLDOWN_DURATION: Duration = Duration::from_secs(180);

/// Default maximum number of hosts tracked in a registry.
pub const DEFAULT_MAX_HOSTS: usize = 512;

/// Default maximum number of ignored endpoints tracked in a cooldown table.
pub const DEFAULT_MAX_IGNORED_ENDPOINTS: usize = 512;

/// Result of upserting a host into the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertAction {
    /// Newly inserted host entry.
    Inserted,
    /// Existing host updated in place with fresh metadata.
    Updated,
    /// Host replaced a previous entry that occupied the same socket address.
    ReplacedEndpoint,
}

/// A host entry stored in the [`HostRegistry`] with its monotonic registration timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredHost {
    /// The host descriptor.
    pub host: Host,
    /// Clock monotonic duration when this host was registered or last refreshed.
    pub last_seen_offset: Duration,
}

impl RegisteredHost {
    /// Constructs a new registered host entry with the given clock offset.
    pub fn new(host: Host, now: Duration) -> Self {
        Self {
            host,
            last_seen_offset: now,
        }
    }

    /// Monotonic age since registration/refresh relative to `now`.
    #[inline]
    pub fn age_at(&self, now: Duration) -> Duration {
        now.saturating_sub(self.last_seen_offset)
    }

    /// Determines if this host entry is considered fresh at `now` given a threshold.
    #[inline]
    pub fn is_fresh_at(&self, now: Duration, threshold: Duration) -> bool {
        self.age_at(now) <= threshold
    }
}

/// Identifies an endpoint for connection attempts, failure logging, and cooldown.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointIdentity {
    /// Associated channel ID (if failure is channel-specific, e.g. 404/503), or `None` if host-wide (e.g. ECONNREFUSED).
    pub channel_id: Option<ChannelId>,
    /// Target network address.
    pub address: SocketAddr,
}

impl EndpointIdentity {
    /// Creates an endpoint identity bound to a specific channel.
    pub fn for_channel(channel_id: ChannelId, address: SocketAddr) -> Self {
        Self {
            channel_id: Some(channel_id),
            address,
        }
    }

    /// Creates a global endpoint identity not tied to any specific channel.
    pub fn global(address: SocketAddr) -> Self {
        Self {
            channel_id: None,
            address,
        }
    }
}

impl std::fmt::Display for EndpointIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.channel_id {
            Some(cid) => write!(f, "pcp://{}/channel/{}", self.address, cid),
            None => write!(f, "pcp://{}", self.address),
        }
    }
}

/// Represents an entry in the failed-endpoint cooldown table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CooldownEntry {
    /// Endpoint identity.
    pub identity: EndpointIdentity,
    /// Clock monotonic duration when the failure was recorded.
    pub failed_at: Duration,
    /// Cumulative count of recorded failures for this endpoint.
    pub failure_count: u32,
    /// Optional failure reason or diagnostic description.
    pub reason: Option<String>,
}

impl CooldownEntry {
    /// Determines whether the cooldown is actively blocking connection attempts at `now`.
    #[inline]
    pub fn is_active_at(&self, now: Duration, cooldown_duration: Duration) -> bool {
        now.saturating_sub(self.failed_at) <= cooldown_duration
    }

    /// Remaining duration of the cooldown period, or `None` if expired.
    #[inline]
    pub fn remaining_at(&self, now: Duration, cooldown_duration: Duration) -> Option<Duration> {
        let elapsed = now.saturating_sub(self.failed_at);
        if elapsed <= cooldown_duration {
            Some(cooldown_duration - elapsed)
        } else {
            None
        }
    }
}

/// Bounded failure cooldown table for temporarily ignoring problematic endpoints (REQ-042).
pub struct EndpointCooldownTable<C: CacheClock = SystemCacheClock> {
    cooldown_duration: Duration,
    max_entries: usize,
    entries: HashMap<EndpointIdentity, CooldownEntry>,
    clock: Arc<C>,
}

impl EndpointCooldownTable<SystemCacheClock> {
    /// Creates a new cooldown table with default duration and system monotonic clock.
    pub fn new(cooldown_duration: Duration, max_entries: usize) -> Self {
        Self::with_clock(cooldown_duration, max_entries, Arc::new(SystemCacheClock::new()))
    }
}

impl<C: CacheClock> EndpointCooldownTable<C> {
    /// Creates a new cooldown table with a specific clock provider.
    pub fn with_clock(cooldown_duration: Duration, max_entries: usize, clock: Arc<C>) -> Self {
        Self {
            cooldown_duration,
            max_entries: max_entries.max(1),
            entries: HashMap::new(),
            clock,
        }
    }

    /// Configured cooldown duration.
    #[inline]
    pub fn cooldown_duration(&self) -> Duration {
        self.cooldown_duration
    }

    /// Sets a new cooldown duration without clearing existing entries.
    pub fn set_cooldown_duration(&mut self, duration: Duration) {
        self.cooldown_duration = duration;
    }

    /// Records a failure for the specified endpoint.
    ///
    /// If the endpoint is already in cooldown, updates the timestamp to `now` and increments
    /// `failure_count`. If capacity is reached, expired entries are cleaned up first; if
    /// still at capacity, the oldest entry is evicted.
    pub fn record_failure(&mut self, identity: EndpointIdentity, reason: Option<String>) {
        let now = self.clock.now();

        if let Some(entry) = self.entries.get_mut(&identity) {
            entry.failed_at = now;
            entry.failure_count = entry.failure_count.saturating_add(1);
            if reason.is_some() {
                entry.reason = reason;
            }
            return;
        }

        // Bounded capacity enforcement (Global Rule 3)
        if self.entries.len() >= self.max_entries {
            self.prune_expired();
            if self.entries.len() >= self.max_entries {
                // Evict the entry with the oldest failure timestamp
                if let Some(oldest_key) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, e)| e.failed_at)
                    .map(|(k, _)| k.clone())
                {
                    self.entries.remove(&oldest_key);
                }
            }
        }

        self.entries.insert(
            identity.clone(),
            CooldownEntry {
                identity,
                failed_at: now,
                failure_count: 1,
                reason,
            },
        );
    }

    /// Checks if the specified endpoint is actively in cooldown.
    ///
    /// If checking a channel-specific identity, callers may also want to verify if
    /// the host address has a global failure cooldown.
    pub fn is_cooled_down(&self, identity: &EndpointIdentity) -> bool {
        let now = self.clock.now();
        if let Some(entry) = self.entries.get(identity) {
            if entry.is_active_at(now, self.cooldown_duration) {
                return true;
            }
        }
        // Also check if there's a global cooldown on this address
        if identity.channel_id.is_some() {
            let global = EndpointIdentity::global(identity.address);
            if let Some(entry) = self.entries.get(&global) {
                if entry.is_active_at(now, self.cooldown_duration) {
                    return true;
                }
            }
        }
        false
    }

    /// Returns remaining cooldown duration for an endpoint, if actively cooled down.
    pub fn cooldown_remaining(&self, identity: &EndpointIdentity) -> Option<Duration> {
        let now = self.clock.now();
        if let Some(entry) = self.entries.get(identity) {
            if let Some(rem) = entry.remaining_at(now, self.cooldown_duration) {
                return Some(rem);
            }
        }
        if identity.channel_id.is_some() {
            let global = EndpointIdentity::global(identity.address);
            if let Some(entry) = self.entries.get(&global) {
                return entry.remaining_at(now, self.cooldown_duration);
            }
        }
        None
    }

    /// Explicitly removes an endpoint from cooldown (e.g. manual retry or operator override).
    pub fn clear_endpoint(&mut self, identity: &EndpointIdentity) -> bool {
        self.entries.remove(identity).is_some()
    }

    /// Clears all entries from the cooldown table.
    pub fn clear_all(&mut self) {
        self.entries.clear();
    }

    /// Physically removes all expired entries from memory (REQ-042, Global Rule 3).
    ///
    /// Returns the number of removed entries.
    pub fn prune_expired(&mut self) -> usize {
        let now = self.clock.now();
        let cooldown = self.cooldown_duration;
        let before = self.entries.len();
        self.entries.retain(|_, entry| entry.is_active_at(now, cooldown));
        before.saturating_sub(self.entries.len())
    }

    /// Current count of entries in the cooldown table (including unpruned expired ones).
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Bounded Servent Host Registry for tracking active peer distribution nodes (REQ-042).
pub struct HostRegistry<C: CacheClock = SystemCacheClock> {
    freshness_threshold: Duration,
    max_hosts: usize,
    /// Primary storage keyed by `(channel_id, session_id)`.
    hosts: HashMap<(ChannelId, ChannelId), RegisteredHost>,
    /// Index mapping `(channel_id, socket_addr)` to `session_id` for address deduplication.
    endpoint_index: HashMap<(ChannelId, SocketAddr), ChannelId>,
    clock: Arc<C>,
}

impl HostRegistry<SystemCacheClock> {
    /// Creates a new host registry with default freshness threshold and system monotonic clock.
    pub fn new(freshness_threshold: Duration, max_hosts: usize) -> Self {
        Self::with_clock(freshness_threshold, max_hosts, Arc::new(SystemCacheClock::new()))
    }
}

impl<C: CacheClock> HostRegistry<C> {
    /// Creates a new host registry with a specific clock provider.
    pub fn with_clock(freshness_threshold: Duration, max_hosts: usize, clock: Arc<C>) -> Self {
        Self {
            freshness_threshold,
            max_hosts: max_hosts.max(1),
            hosts: HashMap::new(),
            endpoint_index: HashMap::new(),
            clock,
        }
    }

    /// Configured freshness threshold.
    #[inline]
    pub fn freshness_threshold(&self) -> Duration {
        self.freshness_threshold
    }

    /// Sets a new freshness threshold without clearing existing entries.
    pub fn set_freshness_threshold(&mut self, threshold: Duration) {
        self.freshness_threshold = threshold;
    }

    /// Inserts or updates a host entry in the registry.
    ///
    /// # Deduplication (REQ-042)
    /// - If a host with the same `(channel_id, session_id)` exists, it is updated in place,
    ///   refreshing `last_seen_offset` and updating metadata.
    /// - If a host with the same `(channel_id, socket_addr)` exists under an older session ID
    ///   (e.g. peer restarted and obtained a new session ID), the old session entry is evicted.
    ///
    /// # Bounded Capacity (Global Rule 3)
    /// - If capacity is reached, expired hosts are pruned first; if still at capacity, the oldest
    ///   host by registration time is evicted.
    pub fn upsert(&mut self, host: Host) -> UpsertAction {
        let now = self.clock.now();
        let channel_id = host.channel_id;
        let session_id = host.session_id;
        let contact_addr = host.contact_address();

        let mut action = UpsertAction::Inserted;

        // 1. Endpoint deduplication: check if another session previously occupied this address
        if let Some(addr) = contact_addr {
            let ep_key = (channel_id, addr);
            if let Some(&existing_sid) = self.endpoint_index.get(&ep_key) {
                if existing_sid != session_id {
                    // Evict the stale session entry for this socket address
                    self.hosts.remove(&(channel_id, existing_sid));
                    action = UpsertAction::ReplacedEndpoint;
                }
            }
            self.endpoint_index.insert(ep_key, session_id);
        }

        let primary_key = (channel_id, session_id);

        // 2. Session deduplication: if entry exists, update in place
        if let Some(entry) = self.hosts.get_mut(&primary_key) {
            entry.host = host;
            entry.last_seen_offset = now;
            return if action == UpsertAction::ReplacedEndpoint {
                action
            } else {
                UpsertAction::Updated
            };
        }

        // 3. Bounded capacity enforcement
        if self.hosts.len() >= self.max_hosts {
            self.prune_expired();
            if self.hosts.len() >= self.max_hosts {
                // Evict the host with the oldest last_seen_offset
                if let Some(oldest_key) = self
                    .hosts
                    .iter()
                    .min_by_key(|(_, e)| e.last_seen_offset)
                    .map(|(k, _)| *k)
                {
                    if let Some(old_host) = self.hosts.remove(&oldest_key) {
                        if let Some(addr) = old_host.host.contact_address() {
                            self.endpoint_index.remove(&(oldest_key.0, addr));
                        }
                    }
                }
            }
        }

        // 4. Insert new registered host
        self.hosts.insert(primary_key, RegisteredHost::new(host, now));
        action
    }

    /// Checks if a host entry exists and is fresh.
    pub fn is_fresh(&self, channel_id: &ChannelId, session_id: &ChannelId) -> bool {
        let now = self.clock.now();
        self.hosts
            .get(&(*channel_id, *session_id))
            .map(|e| e.is_fresh_at(now, self.freshness_threshold))
            .unwrap_or(false)
    }

    /// Retrieves a fresh host descriptor, or `None` if absent or expired.
    pub fn get_fresh(&self, channel_id: &ChannelId, session_id: &ChannelId) -> Option<&Host> {
        let now = self.clock.now();
        self.hosts
            .get(&(*channel_id, *session_id))
            .filter(|e| e.is_fresh_at(now, self.freshness_threshold))
            .map(|e| &e.host)
    }

    /// Retrieves the raw registered host entry regardless of freshness.
    pub fn get_raw(
        &self,
        channel_id: &ChannelId,
        session_id: &ChannelId,
    ) -> Option<&RegisteredHost> {
        self.hosts.get(&(*channel_id, *session_id))
    }

    /// Returns a list of all currently fresh hosts for a given channel.
    pub fn fresh_hosts_for_channel(&self, channel_id: &ChannelId) -> Vec<Host> {
        let now = self.clock.now();
        let threshold = self.freshness_threshold;
        self.hosts
            .iter()
            .filter(|((cid, _), entry)| cid == channel_id && entry.is_fresh_at(now, threshold))
            .map(|(_, entry)| entry.host.clone())
            .collect()
    }

    /// Returns a list of all fresh hosts across all channels in the registry.
    pub fn all_fresh_hosts(&self) -> Vec<Host> {
        let now = self.clock.now();
        let threshold = self.freshness_threshold;
        self.hosts
            .values()
            .filter(|entry| entry.is_fresh_at(now, threshold))
            .map(|entry| entry.host.clone())
            .collect()
    }

    /// Removes a host entry by channel ID and session ID.
    pub fn remove(&mut self, channel_id: &ChannelId, session_id: &ChannelId) -> Option<Host> {
        if let Some(entry) = self.hosts.remove(&(*channel_id, *session_id)) {
            if let Some(addr) = entry.host.contact_address() {
                self.endpoint_index.remove(&(*channel_id, addr));
            }
            Some(entry.host)
        } else {
            None
        }
    }

    /// Clears all entries from the registry.
    pub fn clear(&mut self) {
        self.hosts.clear();
        self.endpoint_index.clear();
    }

    /// Physically removes all expired host entries from memory (REQ-042, Global Rule 3).
    ///
    /// Returns the number of removed entries.
    pub fn prune_expired(&mut self) -> usize {
        let now = self.clock.now();
        let threshold = self.freshness_threshold;
        let before = self.hosts.len();

        let mut expired_keys = Vec::new();
        for (key, entry) in &self.hosts {
            if !entry.is_fresh_at(now, threshold) {
                expired_keys.push(*key);
            }
        }

        for key in expired_keys {
            if let Some(removed) = self.hosts.remove(&key) {
                if let Some(addr) = removed.host.contact_address() {
                    self.endpoint_index.remove(&(key.0, addr));
                }
            }
        }

        before.saturating_sub(self.hosts.len())
    }

    /// Current count of hosts in the registry (including unpruned expired ones).
    #[inline]
    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    /// Whether the registry is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }
}

/// Evaluates candidate peer eligibility combining structural validation, freshness,
/// and failure cooldown (REQ-042, REQ-043).
///
/// An eligible candidate peer must:
/// 1. Satisfy [`Host::is_valid_candidate`] (valid IP/port, non-unspecified, non-loop).
/// 2. Be registered and actively fresh in the [`HostRegistry`].
/// 3. NOT be in failure cooldown in the [`EndpointCooldownTable`].
pub fn is_candidate_eligible<C1: CacheClock, C2: CacheClock>(
    host: &Host,
    registry: &HostRegistry<C1>,
    cooldown: &EndpointCooldownTable<C2>,
    self_session_id: &ChannelId,
    self_addr: Option<&SocketAddr>,
) -> bool {
    // 1. Structural validity & self-loop prevention
    if !host.is_valid_candidate(self_session_id, self_addr) {
        return false;
    }

    // 2. Freshness check
    if !registry.is_fresh(&host.channel_id, &host.session_id) {
        return false;
    }

    // 3. Failure cooldown check
    if let Some(addr) = host.contact_address() {
        let channel_id = EndpointIdentity::for_channel(host.channel_id, addr);
        if cooldown.is_cooled_down(&channel_id) {
            return false;
        }
        let global_id = EndpointIdentity::global(addr);
        if cooldown.is_cooled_down(&global_id) {
            return false;
        }
    }

    true
}
