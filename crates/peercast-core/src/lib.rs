//! Core primitives, binary Atom serialization, bounded buffers, and types for PeerCast.
//!
//! This crate contains pure data models and codecs with zero network or Tokio I/O dependencies.

pub mod actor;
pub mod atom;
pub mod cache;
pub mod channel_id;
pub mod content;
pub mod errors;
pub mod gossip;
pub mod host;
pub mod host_registry;
pub mod id4;
pub mod index_txt;
pub mod parent_selection;
pub mod ringbuffer;
pub mod scalar;
pub mod subscriber;

pub use actor::{
    create_actor_pair, ActorEnvelope, ActorError, GenerationToken, WorkerContext, WorkerHandle,
    WorkerPhase, WorkerStatus,
};
pub use atom::{Atom, AtomBudget, AtomError};
pub use cache::{
    BoundedContentCache, CacheClock, CacheError, FakeClock, HeaderSnapshot, PositionContinuity,
    SystemCacheClock,
};
pub use channel_id::{derive_broadcast_channel_id, ChannelId, ChannelIdError, ChannelNetworkType};
pub use content::{Content, ContinuationFlags};
pub use errors::QuitReason;
pub use gossip::{
    select_forwarding_recipients, BroadcastGroup, GossipDedupCache, GossipDropReason,
    GossipForwardDecision, GossipForwarder, PeerBroadcastTarget, DEFAULT_MAX_DEDUP_ENTRIES,
    DEFAULT_MIN_FORWARDING_TTL, DEFAULT_STORM_SUPPRESSION_WINDOW,
};
pub use host::{Host, HostError, HostFlags1};
pub use host_registry::{
    is_candidate_eligible, CooldownEntry, EndpointCooldownTable, EndpointIdentity, HostRegistry,
    RegisteredHost, UpsertAction, DEFAULT_COOLDOWN_DURATION, DEFAULT_FRESHNESS_THRESHOLD,
    DEFAULT_MAX_HOSTS, DEFAULT_MAX_IGNORED_ENDPOINTS,
};
pub use id4::{Id4Error, ID4};
pub use index_txt::{
    html_decode, parse_index_txt, parse_uptime_str, DirectoryChannelEntry, IndexParseError,
    IndexParserLimits, DEFAULT_MAX_FIELD_LEN, DEFAULT_MAX_INDEX_BYTES, DEFAULT_MAX_INDEX_LINES,
    INDEX_DELIMITER,
};
pub use parent_selection::{
    DeterministicRng, FixedSequenceRng, LocalityDetector, ParentSelectionPolicy, ParentSelector,
    PeerCastYtGreedyHopPolicy, ReconnectBudget, RetryDecision, RngSource,
    StationWeightedRandomPolicy, SystemRng,
};
pub use ringbuffer::ContentRingBuffer;
pub use scalar::{Scalar, ScalarError};
pub use subscriber::{
    create_subscriber_pair, SubscriberDropReason, SubscriberId, SubscriberLimits, SubscriberPacket,
    SubscriberRegistry, SubscriberSink, SubscriberSource,
};
