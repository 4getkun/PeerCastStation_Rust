//! PeerCast container format framing, sniffing, and demuxing engine.
//!
//! Enforces:
//! - REQ-050: Separation of transport packetization, container framing, codec initialization, and safe-start points.
//! - REQ-056: Bounded container sniffing for FLV, TS, MKV/WebM, ASF, and Ogg.
//! - Global Rule 3: Bounded memory buffers and non-consuming replay guarantees.

pub mod flv;
pub mod safestart;
pub mod sniffer;
pub mod ts;

pub use flv::{
    FlvDemuxOutput, FlvDemuxer, FlvFileHeader, FlvHeaderSnapshotBuilder, FlvMetadata,
    FlvParseError, FlvParserEvent, FlvParserPhase, FlvStreamParser, FlvTag, FlvTagHeader,
    FlvTagType, DEFAULT_MAX_FLV_TAG_SIZE, FLV_HEADER_PRELUDE_LEN,
};
pub use safestart::{
    inspect_flv_keyframe, inspect_ts_keyframe, SafeStartConfig, SafeStartDecision, SafeStartMode,
    SafeStartPhase, SafeStartSelector, DEFAULT_SAFE_START_TIMEOUT,
};
pub use sniffer::{
    detect_container, ContainerInfo, ContainerType, SniffBuffer, SniffError, SniffResult,
};
pub use ts::{
    PatTable, PmtTable, TsDemuxOutput, TsDemuxer, TsPacketHeader, TsParseError, TsStreamInfo,
    TsStreamType, NULL_PID, PAT_PID, TS_PACKET_SIZE, TS_SYNC_BYTE,
};
