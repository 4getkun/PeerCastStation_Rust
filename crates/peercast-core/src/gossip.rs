//! Bounded gossip broadcast forwarding, loop prevention, and storm suppression (REQ-044).
//!
//! # Architectural Invariants (REQ-044)
//! 1. **TTL Decrement & Hop Increment**:
//!    When forwarding eligible `bcst` atoms:
//!    - Decrement TTL: `new_ttl = ttl.saturating_sub(1)`.
//!    - INCREMENT Hops: `new_hops = hops.saturating_add(1)` with `u8` overflow protection (caps at 255).
//!    - Never reverse TTL and hop semantics (do NOT decrement hops).
//! 2. **Forwarding Eligibility**:
//!    - In the Station path, forwarding requires `incoming_ttl > 1` (or `new_ttl >= 1`).
//!      Packets with `ttl == 0` or `ttl == 1` are expired and not forwarded.
//!    - If `dest == self_session_id`, the broadcast is addressed to this local servent; it must be
//!      processed locally and NOT forwarded further.
//!    - If `from == self_session_id`, this is an echo/reflection loop of our own broadcast and must be dropped.
//!    - Forwarding requires a valid, known broadcast group (`Root = 1`, `Trackers = 2`, `Relays = 4`).
//! 3. **Source Peer Exclusion**:
//!    Broadcast packets must NEVER be forwarded back to the peer from which they were received.
//! 4. **Bounded Loop & Storm Suppression (Global Rule 3)**:
//!    To prevent broadcast storms in cyclic P2P overlay topologies, incoming gossip packets are
//!    deduplicated via a bounded cache ([`GossipDedupCache`]) keyed by origin session ID and payload hash.
//!    The cache is strictly capped (`max_entries`) and uses monotonic time to expire old entries,
//!    ensuring zero unbounded heap growth.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use md5::{Digest, Md5};

use crate::atom::Atom;
use crate::cache::{CacheClock, SystemCacheClock};
use crate::channel_id::ChannelId;
use crate::id4::ID4;

/// Default minimum incoming TTL required to forward a `bcst` packet (Station requires `ttl > 1`).
pub const DEFAULT_MIN_FORWARDING_TTL: u8 = 2;

/// Default storm suppression window duration.
pub const DEFAULT_STORM_SUPPRESSION_WINDOW: Duration = Duration::from_secs(60);

/// Default maximum number of entries in the storm suppression deduplication cache.
pub const DEFAULT_MAX_DEDUP_ENTRIES: usize = 1024;

/// Bitflags representing targeted broadcast distribution groups (REQ-044).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct BroadcastGroup(pub u8);

impl BroadcastGroup {
    /// YellowPage / Directory distribution (value 1).
    pub const ROOT: BroadcastGroup = BroadcastGroup(1);
    /// Upstream source stream / tracker distribution (value 2).
    pub const TRACKERS: BroadcastGroup = BroadcastGroup(2);
    /// Downstream relay stream distribution (value 4).
    pub const RELAYS: BroadcastGroup = BroadcastGroup(4);
    /// Universal broadcast targeting all connected peers (value 7).
    pub const ALL: BroadcastGroup = BroadcastGroup(1 | 2 | 4);

    /// Checks if this group includes the YellowPage root.
    #[inline]
    pub fn contains_root(&self) -> bool {
        (self.0 & Self::ROOT.0) != 0
    }

    /// Checks if this group includes upstream trackers / sources.
    #[inline]
    pub fn contains_trackers(&self) -> bool {
        (self.0 & Self::TRACKERS.0) != 0
    }

    /// Checks if this group includes downstream relay peers.
    #[inline]
    pub fn contains_relays(&self) -> bool {
        (self.0 & Self::RELAYS.0) != 0
    }

    /// Validates that the group mask contains at least one known flag and no undefined bits.
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.0 != 0 && (self.0 & !Self::ALL.0) == 0
    }

    /// Raw byte representation.
    #[inline]
    pub fn raw(&self) -> u8 {
        self.0
    }
}

impl std::ops::BitOr for BroadcastGroup {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        BroadcastGroup(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for BroadcastGroup {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        BroadcastGroup(self.0 & rhs.0)
    }
}

/// Reason why a `bcst` packet was not forwarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GossipDropReason {
    /// TTL was too low to permit forwarding (e.g. TTL <= 1 in Station).
    ExpiredTtl { incoming_ttl: u8 },
    /// Destination was explicitly targeted to this local servent; processed locally only.
    LocalDestination,
    /// Origin session ID matched local servent; reflection loop avoided.
    OwnReflectionLoop,
    /// Packet was identified as a duplicate and dropped to prevent broadcast storms.
    DuplicateStormSuppressed,
    /// Broadcast group was unknown or missing.
    UnknownGroup(u8),
    /// Missing mandatory metadata atoms (from, ttl, hops, or group).
    MalformedAtom(String),
}

/// Outcome of inspecting and evaluating an incoming `bcst` atom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GossipForwardDecision {
    /// Whether the packet payload should be processed locally by this node.
    pub process_locally: bool,
    /// If forwardable, the modified `bcst` Atom with decremented TTL and incremented hops.
    pub forward_packet: Option<Atom>,
    /// Target broadcast group for outgoing delivery.
    pub target_groups: BroadcastGroup,
    /// If forwarding was rejected or suppressed, the reason why.
    pub drop_reason: Option<GossipDropReason>,
}

/// Unique deduplication key for storm suppression.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GossipDedupKey {
    origin_sid: ChannelId,
    channel_id: Option<ChannelId>,
    payload_hash: [u8; 16],
}

/// Bounded LRU/FIFO deduplication cache for gossip storm suppression (REQ-044, Global Rule 3).
pub struct GossipDedupCache<C: CacheClock = SystemCacheClock> {
    max_entries: usize,
    window: Duration,
    entries: HashMap<GossipDedupKey, Duration>,
    clock: Arc<C>,
}

impl GossipDedupCache<SystemCacheClock> {
    /// Creates a new deduplication cache with default capacity and window.
    pub fn new(max_entries: usize, window: Duration) -> Self {
        Self::with_clock(max_entries, window, Arc::new(SystemCacheClock::new()))
    }
}

impl<C: CacheClock> GossipDedupCache<C> {
    /// Creates a new deduplication cache with a specific clock provider.
    pub fn with_clock(max_entries: usize, window: Duration, clock: Arc<C>) -> Self {
        Self {
            max_entries: max_entries.max(1),
            window,
            entries: HashMap::new(),
            clock,
        }
    }

    /// Checks if a packet has already been seen within the suppression window.
    ///
    /// If unseen, records the packet with the current monotonic timestamp.
    /// Enforces `max_entries` bounded capacity through expired pruning and FIFO eviction.
    pub fn check_and_record(
        &mut self,
        origin_sid: ChannelId,
        channel_id: Option<ChannelId>,
        payload_hash: [u8; 16],
    ) -> bool {
        let now = self.clock.now();
        let key = GossipDedupKey {
            origin_sid,
            channel_id,
            payload_hash,
        };

        if let Some(&seen_at) = self.entries.get(&key) {
            if now.saturating_sub(seen_at) <= self.window {
                return false; // Duplicate within active window
            }
        }

        // Bounded capacity check (Global Rule 3)
        if self.entries.len() >= self.max_entries {
            self.prune_expired();
            if self.entries.len() >= self.max_entries {
                // Evict the entry with the oldest timestamp
                if let Some(oldest_key) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, &ts)| ts)
                    .map(|(k, _)| k.clone())
                {
                    self.entries.remove(&oldest_key);
                }
            }
        }

        self.entries.insert(key, now);
        true // First time seen
    }

    /// Physically removes expired entries from memory (REQ-044, Global Rule 3).
    pub fn prune_expired(&mut self) -> usize {
        let now = self.clock.now();
        let window = self.window;
        let before = self.entries.len();
        self.entries.retain(|_, &mut seen_at| now.saturating_sub(seen_at) <= window);
        before.saturating_sub(self.entries.len())
    }

    /// Current entry count.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clears all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Gossip broadcast forwarder and routing engine (REQ-044).
pub struct GossipForwarder<C: CacheClock = SystemCacheClock> {
    self_session_id: ChannelId,
    min_forwarding_ttl: u8,
    dedup: GossipDedupCache<C>,
}

impl GossipForwarder<SystemCacheClock> {
    /// Constructs a new gossip forwarder with system monotonic clock.
    pub fn new(self_session_id: ChannelId) -> Self {
        Self::with_clock(
            self_session_id,
            DEFAULT_MIN_FORWARDING_TTL,
            DEFAULT_MAX_DEDUP_ENTRIES,
            DEFAULT_STORM_SUPPRESSION_WINDOW,
            Arc::new(SystemCacheClock::new()),
        )
    }
}

impl<C: CacheClock> GossipForwarder<C> {
    /// Constructs a new gossip forwarder with a specific clock and configuration.
    pub fn with_clock(
        self_session_id: ChannelId,
        min_forwarding_ttl: u8,
        max_dedup_entries: usize,
        storm_window: Duration,
        clock: Arc<C>,
    ) -> Self {
        Self {
            self_session_id,
            min_forwarding_ttl,
            dedup: GossipDedupCache::with_clock(max_dedup_entries, storm_window, clock),
        }
    }

    /// Evaluates an incoming `bcst` Atom and determines local processing and forwarding actions.
    pub fn evaluate_incoming_bcst(&mut self, atom: &Atom) -> GossipForwardDecision {
        if atom.name() != ID4::PCP_BCST {
            return GossipForwardDecision {
                process_locally: false,
                forward_packet: None,
                target_groups: BroadcastGroup(0),
                drop_reason: Some(GossipDropReason::MalformedAtom(format!(
                    "Expected 'bcst' atom, found {:?}",
                    atom.name()
                ))),
            };
        }

        let children = match atom.children() {
            Some(c) => c,
            None => {
                return GossipForwardDecision {
                    process_locally: false,
                    forward_packet: None,
                    target_groups: BroadcastGroup(0),
                    drop_reason: Some(GossipDropReason::MalformedAtom(
                        "Atom has no children".to_string(),
                    )),
                };
            }
        };

        // Extract metadata fields from bcst children
        let mut ttl = None;
        let mut hops = None;
        let mut from = None;
        let mut dest = None;
        let mut group = None;
        let mut channel_id = None;

        let mut payload_hasher = Md5::new();

        for child in children {
            match child.name() {
                ID4::BCST_TTL => {
                    if let Ok(v) = child.get_u8() {
                        ttl = Some(v);
                    } else if let Ok(v) = child.get_u32_le() {
                        ttl = Some((v & 0xFF) as u8);
                    }
                }
                ID4::BCST_HOPS => {
                    if let Ok(v) = child.get_u8() {
                        hops = Some(v);
                    } else if let Ok(v) = child.get_u32_le() {
                        hops = Some((v & 0xFF) as u8);
                    }
                }
                ID4::BCST_FROM => {
                    if let Some(bytes) = child.leaf_bytes() {
                        if bytes.len() == 16 {
                            let mut arr = [0u8; 16];
                            arr.copy_from_slice(bytes);
                            from = Some(ChannelId::from_bytes(arr));
                        }
                    }
                }
                ID4::BCST_DEST => {
                    if let Some(bytes) = child.leaf_bytes() {
                        if bytes.len() == 16 {
                            let mut arr = [0u8; 16];
                            arr.copy_from_slice(bytes);
                            dest = Some(ChannelId::from_bytes(arr));
                        }
                    }
                }
                ID4::BCST_GRP => {
                    if let Ok(v) = child.get_u8() {
                        group = Some(BroadcastGroup(v));
                    }
                }
                ID4::BCST_CID => {
                    if let Some(bytes) = child.leaf_bytes() {
                        if bytes.len() == 16 {
                            let mut arr = [0u8; 16];
                            arr.copy_from_slice(bytes);
                            channel_id = Some(ChannelId::from_bytes(arr));
                        }
                    }
                }
                _ => {
                    // Non-header payload atoms are hashed to create a unique content fingerprint
                    payload_hasher.update(child.name().0);
                    if let Some(bytes) = child.leaf_bytes() {
                        payload_hasher.update(bytes);
                    }
                }
            }
        }

        // Validate mandatory header fields
        let ttl_val = match ttl {
            Some(t) => t,
            None => {
                return GossipForwardDecision {
                    process_locally: false,
                    forward_packet: None,
                    target_groups: BroadcastGroup(0),
                    drop_reason: Some(GossipDropReason::MalformedAtom("Missing 'ttl'".to_string())),
                };
            }
        };

        let hops_val = match hops {
            Some(h) => h,
            None => {
                return GossipForwardDecision {
                    process_locally: false,
                    forward_packet: None,
                    target_groups: BroadcastGroup(0),
                    drop_reason: Some(GossipDropReason::MalformedAtom("Missing 'hops'".to_string())),
                };
            }
        };

        let from_sid = match from {
            Some(f) => f,
            None => {
                return GossipForwardDecision {
                    process_locally: false,
                    forward_packet: None,
                    target_groups: BroadcastGroup(0),
                    drop_reason: Some(GossipDropReason::MalformedAtom("Missing 'from'".to_string())),
                };
            }
        };

        let b_group = match group {
            Some(g) if g.is_valid() => g,
            Some(g) => {
                return GossipForwardDecision {
                    process_locally: false,
                    forward_packet: None,
                    target_groups: g,
                    drop_reason: Some(GossipDropReason::UnknownGroup(g.0)),
                };
            }
            None => {
                return GossipForwardDecision {
                    process_locally: false,
                    forward_packet: None,
                    target_groups: BroadcastGroup(0),
                    drop_reason: Some(GossipDropReason::MalformedAtom("Missing 'grp'".to_string())),
                };
            }
        };

        // 1. Origin Check: Reflection loop prevention
        if from_sid == self.self_session_id && !self.self_session_id.is_zero() {
            return GossipForwardDecision {
                process_locally: false,
                forward_packet: None,
                target_groups: b_group,
                drop_reason: Some(GossipDropReason::OwnReflectionLoop),
            };
        }

        // 2. Destination Check: Local delivery vs forward
        let is_for_me = match dest {
            Some(d) => d == self.self_session_id || d.is_zero(),
            None => true, // Broadcast with no dest is for everyone
        };

        let is_targeted_strictly_to_me = match dest {
            Some(d) => d == self.self_session_id && !self.self_session_id.is_zero(),
            None => false,
        };

        if is_targeted_strictly_to_me {
            // Targeted to this local servent specifically; do not forward further
            return GossipForwardDecision {
                process_locally: true,
                forward_packet: None,
                target_groups: b_group,
                drop_reason: Some(GossipDropReason::LocalDestination),
            };
        }

        // 3. Storm Suppression: Deduplication check
        let payload_digest: [u8; 16] = payload_hasher.finalize().into();
        let is_new = self
            .dedup
            .check_and_record(from_sid, channel_id, payload_digest);
        if !is_new {
            return GossipForwardDecision {
                process_locally: false,
                forward_packet: None,
                target_groups: b_group,
                drop_reason: Some(GossipDropReason::DuplicateStormSuppressed),
            };
        }

        // 4. TTL Check (Station requires incoming TTL > 1)
        if ttl_val < self.min_forwarding_ttl {
            return GossipForwardDecision {
                process_locally: is_for_me,
                forward_packet: None,
                target_groups: b_group,
                drop_reason: Some(GossipDropReason::ExpiredTtl {
                    incoming_ttl: ttl_val,
                }),
            };
        }

        // 5. Decrement TTL and INCREMENT Hops (with u8 overflow protection)
        let new_ttl = ttl_val.saturating_sub(1);
        let new_hops = hops_val.saturating_add(1);

        // Reconstruct modified child list
        let mut new_children = Vec::with_capacity(children.len());
        for child in children {
            match child.name() {
                ID4::BCST_TTL => {
                    new_children.push(Atom::new_u8(ID4::BCST_TTL, new_ttl));
                }
                ID4::BCST_HOPS => {
                    new_children.push(Atom::new_u8(ID4::BCST_HOPS, new_hops));
                }
                _ => {
                    new_children.push(child.clone());
                }
            }
        }

        let forwarded_atom = Atom::new_parent(ID4::PCP_BCST, new_children);

        GossipForwardDecision {
            process_locally: is_for_me,
            forward_packet: Some(forwarded_atom),
            target_groups: b_group,
            drop_reason: None,
        }
    }

    /// Access the internal deduplication cache.
    #[inline]
    pub fn dedup_cache(&self) -> &GossipDedupCache<C> {
        &self.dedup
    }

    /// Mutable access to the internal deduplication cache.
    #[inline]
    pub fn dedup_cache_mut(&mut self) -> &mut GossipDedupCache<C> {
        &mut self.dedup
    }
}

/// Helper identifying an active peer for broadcast routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerBroadcastTarget {
    /// Peer session ID (if established).
    pub session_id: Option<ChannelId>,
    /// Peer network contact address.
    pub address: SocketAddr,
    /// Whether this peer is an upstream source or downstream relay sink.
    pub is_upstream: bool,
}

/// Filters candidate peers to receive a forwarded broadcast, strictly excluding the incoming source.
pub fn select_forwarding_recipients(
    decision: &GossipForwardDecision,
    source_session_id: Option<&ChannelId>,
    source_addr: Option<&SocketAddr>,
    peers: &[PeerBroadcastTarget],
) -> Vec<PeerBroadcastTarget> {
    if decision.forward_packet.is_none() {
        return Vec::new();
    }

    let group = decision.target_groups;
    let mut recipients = Vec::new();

    for peer in peers {
        // Source exclusion: NEVER forward back to the incoming sender (REQ-044)
        if let Some(src_sid) = source_session_id {
            if let Some(peer_sid) = peer.session_id {
                if peer_sid == *src_sid && !src_sid.is_zero() {
                    continue;
                }
            }
        }
        if let Some(src_addr) = source_addr {
            if &peer.address == src_addr {
                continue;
            }
        }

        // Group routing
        if peer.is_upstream {
            if group.contains_trackers() || group.contains_root() {
                recipients.push(peer.clone());
            }
        } else if group.contains_relays() {
            recipients.push(peer.clone());
        }
    }

    recipients
}
