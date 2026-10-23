//! PeerCast container format framing, sniffing, and demuxing engine.
//!
//! Enforces:
//! - REQ-050: Separation of transport packetization, container framing, codec initialization, and safe-start points.
//! - REQ-056: Bounded container sniffing for FLV, TS, MKV/WebM, ASF, and Ogg.
//! - Global Rule 3: Bounded memory buffers and non-consuming replay guarantees.

pub mod asf;
pub mod flv;
pub mod mkv;
pub mod safestart;
pub mod sniffer;
pub mod ts;

pub use asf::{
    parse_asf_top_level_header, AsfBitrateRecord, AsfContentDescription, AsfDemuxOutput,
    AsfDemuxer, AsfDemuxerState, AsfFileProperties, AsfGuid, AsfMetadata, AsfObjectHeader,
    AsfParseError, AsfStreamProperties, AsfTransportProfile, WmspFrame, WmspFrameType,
    ASF_CONTENT_DESCRIPTION_GUID, ASF_DATA_OBJECT_GUID, ASF_FILE_PROPERTIES_GUID,
    ASF_OBJECT_HEADER_LEN, ASF_STREAM_BITRATE_GUID, ASF_STREAM_PROPERTIES_GUID,
    ASF_STREAM_TYPE_AUDIO_GUID, ASF_STREAM_TYPE_VIDEO_GUID, ASF_TOP_LEVEL_HEADER_GUID,
    ASF_TOP_LEVEL_HEADER_MIN_LEN, ASF_TOP_LEVEL_HEADER_PRELUDE_LEN,
    DEFAULT_MAX_ASF_BUFFER_CAPACITY, DEFAULT_MAX_ASF_OBJECT_SIZE,
};

pub use flv::{
    FlvDemuxOutput, FlvDemuxer, FlvFileHeader, FlvHeaderSnapshotBuilder, FlvMetadata,
    FlvParseError, FlvParserEvent, FlvParserPhase, FlvStreamParser, FlvTag, FlvTagHeader,
    FlvTagType, DEFAULT_MAX_FLV_TAG_SIZE, FLV_HEADER_PRELUDE_LEN,
};
pub use mkv::{
    inspect_mkv_keyframe, parse_ebml_element, MkvDemuxOutput, MkvDemuxer, MkvDemuxerState,
    MkvElement, MkvElementHeader, MkvMetadata, MkvParseError, VInt, DEFAULT_MAX_MKV_BUFFER_CAPACITY,
    DEFAULT_MAX_MKV_ELEMENT_SIZE, ID_BLOCK, ID_BLOCK_GROUP, ID_CLUSTER, ID_DOC_TYPE,
    ID_EBML, ID_INFO, ID_REFERENCE_BLOCK, ID_SEGMENT, ID_SIMPLE_BLOCK, ID_TIMECODE,
    ID_TIMECODE_SCALE, ID_TRACKS,
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
