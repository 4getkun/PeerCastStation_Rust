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
