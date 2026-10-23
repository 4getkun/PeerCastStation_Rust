//! PeerCast container format framing, sniffing, and demuxing engine.
//!
//! Enforces:
//! - REQ-050: Separation of transport packetization, container framing, codec initialization, and safe-start points.
//! - REQ-056: Bounded container sniffing for FLV, TS, MKV/WebM, ASF, and Ogg.
//! - Global Rule 3: Bounded memory buffers and non-consuming replay guarantees.

pub mod sniffer;

pub use sniffer::{
    detect_container, ContainerInfo, ContainerType, SniffBuffer, SniffError, SniffResult,
};
