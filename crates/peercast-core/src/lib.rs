//! Core primitives, binary Atom serialization, bounded buffers, and types for PeerCast.
//!
//! This crate contains pure data models and codecs with zero network or Tokio I/O dependencies.

pub mod id4;
pub mod scalar;
pub mod atom;
pub mod content;
pub mod ringbuffer;
pub mod host;
pub mod channel_id;
pub mod errors;
pub mod actor;
pub mod cache;
pub mod subscriber;
pub mod host_registry;
pub mod parent_selection;
pub mod gossip;

pub use id4::{ID4, Id4Error};
pub use scalar::{Scalar, ScalarError};
pub use atom::{Atom, AtomBudget, AtomError};
pub use channel_id::{ChannelId, ChannelIdError, ChannelNetworkType, derive_broadcast_channel_id};
pub use content::{Content, ContinuationFlags};
pub use ringbuffer::ContentRingBuffer;
pub use host::{Host, HostError, HostFlags1};
pub use errors::QuitReason;
pub use actor::{
    ActorEnvelope, ActorError, GenerationToken, WorkerContext, WorkerHandle, WorkerPhase,
    WorkerStatus, create_actor_pair,
};
pub use cache::{
    BoundedContentCache, CacheClock, CacheError, FakeClock, HeaderSnapshot, PositionContinuity,
    SystemCacheClock,
};
pub use subscriber::{
    create_subscriber_pair, SubscriberDropReason, SubscriberId, SubscriberLimits,
    SubscriberPacket, SubscriberRegistry, SubscriberSink, SubscriberSource,
};
pub use host_registry::{
    CooldownEntry, EndpointCooldownTable, EndpointIdentity, HostRegistry, RegisteredHost,
    UpsertAction, is_candidate_eligible, DEFAULT_COOLDOWN_DURATION, DEFAULT_FRESHNESS_THRESHOLD,
    DEFAULT_MAX_HOSTS, DEFAULT_MAX_IGNORED_ENDPOINTS,
};
pub use parent_selection::{
    DeterministicRng, FixedSequenceRng, LocalityDetector, ParentSelectionPolicy, ParentSelector,
    PeerCastYtGreedyHopPolicy, ReconnectBudget, RetryDecision, RngSource, StationWeightedRandomPolicy,
    SystemRng,
};
pub use gossip::{
    select_forwarding_recipients, BroadcastGroup, GossipDedupCache, GossipDropReason,
    GossipForwardDecision, GossipForwarder, PeerBroadcastTarget, DEFAULT_MAX_DEDUP_ENTRIES,
    DEFAULT_MIN_FORWARDING_TTL, DEFAULT_STORM_SUPPRESSION_WINDOW,
};
