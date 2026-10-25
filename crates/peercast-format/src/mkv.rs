//! Matroska (MKV) and WebM (EBML) stream parser and demuxer (REQ-051, REQ-055, REQ-056).
//!
//! Enforces:
//! - REQ-055: Container-aware safe-start selection and keyframe detection.
//!   Keyframes are identified unambiguously via SimpleBlock flags (`flags & 0x80 != 0`)
//!   or BlockGroup absence of ReferenceBlock elements. Never clones FLV assumptions!
//! - REQ-051: Header snapshot retention, content epoch bumping on header replacement,
//!   and invalidation of stale stream state.
//! - REQ-056: Full support for MKV and WebM containers with valid, truncated, oversized,
//!   and change-of-header verification.
//! - Global Rule 2: Explicit state machine with entry re-entrancy guards.
//! - Global Rule 3: Bounded memory buffers and zero unbounded allocations.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use peercast_core::cache::HeaderSnapshot;
use thiserror::Error;

/// Default maximum allowed MKV element size (16 MiB) to prevent unbounded memory allocation (Global Rule 3).
pub const DEFAULT_MAX_MKV_ELEMENT_SIZE: u64 = 16 * 1024 * 1024;

/// Default maximum parser buffer capacity (16 MiB).
pub const DEFAULT_MAX_MKV_BUFFER_CAPACITY: usize = 16 * 1024 * 1024;

// Standard EBML and Matroska Element IDs
pub const ID_EBML: u32 = 0x1A45DFA3;
pub const ID_EBML_VERSION: u32 = 0x4286;
pub const ID_EBML_READ_VERSION: u32 = 0x42F7;
pub const ID_EBML_MAX_ID_LENGTH: u32 = 0x42F2;
pub const ID_EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;
pub const ID_DOC_TYPE: u32 = 0x4282;
pub const ID_DOC_TYPE_VERSION: u32 = 0x4287;
pub const ID_DOC_TYPE_READ_VERSION: u32 = 0x4285;

pub const ID_SEGMENT: u32 = 0x18538067;
pub const ID_SEEK_HEAD: u32 = 0x114D9B74;
pub const ID_INFO: u32 = 0x1549A966;
pub const ID_CLUSTER: u32 = 0x1F43B675;
pub const ID_TRACKS: u32 = 0x1654AE6B;
pub const ID_TRACK_ENTRY: u32 = 0xAE;
pub const ID_TRACK_NUMBER: u32 = 0xD7;
pub const ID_TRACK_TYPE: u32 = 0x83;
pub const ID_CODEC_ID: u32 = 0x86;
pub const ID_TIMECODE_SCALE: u32 = 0x2AD7B1;
pub const ID_TIMECODE: u32 = 0xE7;
pub const ID_SIMPLE_BLOCK: u32 = 0xA3;
pub const ID_BLOCK_GROUP: u32 = 0xA0;
pub const ID_BLOCK: u32 = 0xA1;
pub const ID_REFERENCE_BLOCK: u32 = 0xFB;
pub const ID_VOID: u32 = 0xEC;
pub const ID_CRC32: u32 = 0xBF;
pub const ID_CUES: u32 = 0x1C53BB6B;
pub const ID_TAGS: u32 = 0x1254C367;

/// Errors encountered during MKV/WebM demuxing and parsing.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum MkvParseError {
    #[error("Invalid EBML VINT: first byte was 0 (no marker bit set)")]
    InvalidVintFirstByte,

    #[error("EBML VINT length {0} exceeds maximum supported limit of 8 bytes")]
    VintLengthExceeded(usize),

    #[error("EBML Element ID length {0} exceeds declared max ID length {1}")]
    ElementIdLengthExceeded(usize, usize),

    #[error("EBML Element Size length {0} exceeds declared max size length {1}")]
    ElementSizeLengthExceeded(usize, usize),

    #[error("Element size {actual} exceeded maximum allowed limit {max} (Global Rule 3)")]
    ElementSizeExceeded { actual: u64, max: u64 },

    #[error("Element size {0} exceeds addressable memory on this architecture")]
    ElementSizeOverflow(u64),

    #[error("Truncated MKV element: needed {needed} bytes, but only {available} available")]
    TruncatedElement { needed: usize, available: usize },

    #[error("Invalid EBML header: missing or malformed EBML element")]
    InvalidEbmlHeader,

    #[error("Buffer capacity {actual} exceeded configured maximum {max} (Global Rule 3)")]
    BufferCapacityExceeded { actual: usize, max: usize },

    #[error("Unexpected EOF during stream parsing")]
    UnexpectedEof,
}

/// Variable-Length Integer (EBML VINT).
/// Format: 1 to 8 bytes where leading zero count indicates length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VInt {
    /// Interpreted value (with length descriptor bit cleared for sizes, or raw u32/u64 for IDs).
    pub value: u64,
    /// Total encoded length in bytes (1..=8).
    pub len: usize,
    /// Raw wire bytes.
    pub raw_bytes: Bytes,
    /// True if all data bits are set to 1 (representing Unknown / Undefined size in EBML).
    pub is_unknown: bool,
}

impl VInt {
    /// Attempts to parse a VINT size from the prefix of a slice.
    /// The length descriptor bit is stripped from `value`.
    pub fn parse_size(slice: &[u8]) -> Result<Option<Self>, MkvParseError> {
        if slice.is_empty() {
            return Ok(None);
        }
        let b0 = slice[0];
        if b0 == 0 {
            return Err(MkvParseError::InvalidVintFirstByte);
        }
        let len = (b0.leading_zeros() + 1) as usize;
        if len > 8 {
            return Err(MkvParseError::VintLengthExceeded(len));
        }
        if slice.len() < len {
            return Ok(None);
        }

        let raw_bytes = Bytes::copy_from_slice(&slice[..len]);
        // Mask out the length descriptor bit
        let data_mask = (1u64 << (8 - len)) - 1;
        let mut value = (b0 as u64) & data_mask;
        for &b in &slice[1..len] {
            value = (value << 8) | (b as u64);
        }

        // Unknown size: all (7 * len) data bits are 1
        let unknown_marker = (1u64 << (7 * len)) - 1;
        let is_unknown = value == unknown_marker;

        Ok(Some(Self {
            value,
            len,
            raw_bytes,
            is_unknown,
        }))
    }

    /// Attempts to parse an Element ID from the prefix of a slice.
    /// The length descriptor bit is PRESERVED in `value` and mapped to a standard u32 ID.
    pub fn parse_id(slice: &[u8]) -> Result<Option<Self>, MkvParseError> {
        if slice.is_empty() {
            return Ok(None);
        }
        let b0 = slice[0];
        if b0 == 0 {
            return Err(MkvParseError::InvalidVintFirstByte);
        }
        let len = (b0.leading_zeros() + 1) as usize;
        if len > 4 {
            // Standard EBML element IDs are at most 4 bytes
            return Err(MkvParseError::ElementIdLengthExceeded(len, 4));
        }
        if slice.len() < len {
            return Ok(None);
        }

        let raw_bytes = Bytes::copy_from_slice(&slice[..len]);
        let mut value = 0u64;
        for &b in &slice[..len] {
            value = (value << 8) | (b as u64);
        }

        Ok(Some(Self {
            value,
            len,
            raw_bytes,
            is_unknown: false,
        }))
    }

    /// Encodes a u64 size as an EBML VINT. If `size` is None, encodes as unknown size.
    pub fn encode_size(size: Option<u64>) -> Bytes {
        let mut b = BytesMut::new();
        match size {
            None => {
                // Unknown size (8 bytes with all 56 data bits 1)
                b.put_u8(0x01);
                b.put_slice(&[0xFF; 7]);
            }
            Some(v) => {
                let len = if v < 127 {
                    1
                } else if v < 16383 {
                    2
                } else if v < 2097151 {
                    3
                } else if v < 268435455 {
                    4
                } else if v < 34359738367 {
                    5
                } else if v < 4398046511103 {
                    6
                } else if v < 562949953421311 {
                    7
                } else {
                    8
                };

                let marker = 1u64 << (7 * len);
                let encoded = marker | v;
                for i in (0..len).rev() {
                    b.put_u8(((encoded >> (i * 8)) & 0xFF) as u8);
                }
            }
        }
        b.freeze()
    }
}

/// Parsed EBML Element Header (ID + Size).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkvElementHeader {
    pub id: VInt,
    pub size: VInt,
}

impl MkvElementHeader {
    /// Total byte length of the header on wire (id length + size length).
    pub fn header_len(&self) -> usize {
        self.id.len + self.size.len
    }

    /// Attempts to parse an element header from a slice.
    pub fn parse(slice: &[u8]) -> Result<Option<Self>, MkvParseError> {
        let id = match VInt::parse_id(slice)? {
            Some(id) => id,
            None => return Ok(None),
        };
        let size_slice = &slice[id.len..];
        let size = match VInt::parse_size(size_slice)? {
            Some(sz) => sz,
            None => return Ok(None),
        };
        Ok(Some(Self { id, size }))
    }
}

/// Parsed EBML Element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkvElement {
    pub header: MkvElementHeader,
    pub data: Bytes,
}

impl MkvElement {
    /// Total serialized size of the element on wire.
    pub fn byte_size(&self) -> usize {
        self.header.header_len() + self.data.len()
    }

    /// Serializes the complete element to a `Bytes` buffer.
    pub fn to_bytes(&self) -> Bytes {
        let mut b = BytesMut::with_capacity(self.byte_size());
        b.put_slice(&self.header.id.raw_bytes);
        b.put_slice(&self.header.size.raw_bytes);
        b.put_slice(&self.data);
        b.freeze()
    }
}

/// Stream metadata and format profile (Parity with upstream Station `MKVContentReader`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkvMetadata {
    pub doc_type: String,
    pub doc_type_version: u32,
    pub content_type: String,
    pub mime_type: String,
    pub stream_ext: String,
    pub has_video: bool,
    pub has_audio: bool,
    pub timecode_scale_ns: u64,
    pub video_codecs: Vec<String>,
    pub audio_codecs: Vec<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Output event yielded by `MkvDemuxer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MkvDemuxOutput {
    /// Initial or updated header snapshot with epoch and parsed stream metadata.
    HeaderSnapshotReady {
        snapshot: HeaderSnapshot,
        metadata: Box<MkvMetadata>,
        epoch: u64,
    },
    /// Relayed media block / cluster packet.
    ContentPacket {
        packet: Bytes,
        epoch: u64,
        position: u64,
        is_keyframe: bool,
    },
    /// End of stream reached cleanly.
    EndOfStream,
}

/// Inspects a `SimpleBlock` or `BlockGroup` payload to determine if it is a random access keyframe (REQ-055).
///
/// Parity with PeerCast-YT `hasKeyFrame`:
/// - SimpleBlock: bit 7 of flags byte (`flags & 0x80 != 0`) indicates keyframe.
/// - BlockGroup: absent `ReferenceBlock` (`0xFB`) indicates keyframe.
pub fn inspect_mkv_keyframe(element_id: u32, payload: &[u8]) -> bool {
    if element_id == ID_SIMPLE_BLOCK {
        // SimpleBlock payload format:
        // 1. Track Number: VINT (1..8 bytes)
        // 2. Relative Timecode: 2 bytes (i16)
        // 3. Flags: 1 byte (bit 7 is Keyframe flag: 0x80)
        if let Ok(Some(track_vint)) = VInt::parse_size(payload) {
            let flags_offset = track_vint.len + 2;
            if flags_offset < payload.len() {
                let flags = payload[flags_offset];
                return (flags & 0x80) != 0;
            }
        }
    } else if element_id == ID_BLOCK_GROUP {
        // BlockGroup format: contains Block (0xA1), optional ReferenceBlock (0xFB), BlockDuration (0x9B), etc.
        // If ReferenceBlock is present, it is an interframe (references previous frame).
        // If ReferenceBlock is absent, it is a keyframe!
        let mut cursor = 0;
        let mut has_block = false;
        let mut has_reference = false;

        while cursor < payload.len() {
            if let Ok(Some(header)) = MkvElementHeader::parse(&payload[cursor..]) {
                let id = header.id.value as u32;
                if id == ID_BLOCK {
                    has_block = true;
                } else if id == ID_REFERENCE_BLOCK {
                    has_reference = true;
                }
                let step = header.header_len() + header.size.value as usize;
                if step == 0 || cursor + step > payload.len() {
                    break;
                }
                cursor += step;
            } else {
                break;
            }
        }

        if has_block && !has_reference {
            return true;
        }
    }

    false
}

/// Parses the EBML header element body and extracts DocType and versions.
pub fn parse_ebml_element(data: &[u8]) -> Result<(String, u32, u32, u32), MkvParseError> {
    let mut cursor = 0;
    let mut doc_type = "matroska".to_string();
    let mut doc_type_version = 1u32;
    let mut max_id_length = 4u32;
    let mut max_size_length = 8u32;

    while cursor < data.len() {
        if let Some(header) = MkvElementHeader::parse(&data[cursor..])? {
            let id = header.id.value as u32;
            let sz = header.size.value as usize;
            let start = cursor + header.header_len();
            let end = start + sz;
            if end > data.len() {
                break;
            }
            let val_slice = &data[start..end];

            match id {
                ID_DOC_TYPE => {
                    if let Ok(s) = std::str::from_utf8(val_slice) {
                        doc_type = s.trim_matches('\0').to_string();
                    }
                }
                ID_DOC_TYPE_VERSION => {
                    let mut v = 0u32;
                    for &b in val_slice {
                        v = (v << 8) | (b as u32);
                    }
                    doc_type_version = v;
                }
                ID_EBML_MAX_ID_LENGTH => {
                    let mut v = 0u32;
                    for &b in val_slice {
                        v = (v << 8) | (b as u32);
                    }
                    max_id_length = v;
                }
                ID_EBML_MAX_SIZE_LENGTH => {
                    let mut v = 0u32;
                    for &b in val_slice {
                        v = (v << 8) | (b as u32);
                    }
                    max_size_length = v;
                }
                _ => {}
            }
            cursor = end;
        } else {
            break;
        }
    }

    Ok((doc_type, doc_type_version, max_id_length, max_size_length))
}

/// Demuxer state machine (Global Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MkvDemuxerState {
    AwaitingEbml,
    AwaitingSegment,
    ReadingHeaders,
    StreamingClusters,
    Ended,
}

/// Independent streaming demuxer for Matroska / WebM containers (REQ-051, REQ-055, REQ-056).
#[derive(Debug)]
pub struct MkvDemuxer {
    state: MkvDemuxerState,
    buffer: BytesMut,
    max_buffer_capacity: usize,
    max_element_size: u64,
    current_epoch: u64,
    stream_position: u64,
    header_elements: Vec<MkvElement>,
    metadata: Option<MkvMetadata>,
    header_snapshot: Option<HeaderSnapshot>,
    timecode_scale_ns: u64,
    doc_type: String,
    doc_type_version: u32,
    max_id_length: usize,
    max_size_length: usize,
}

impl Default for MkvDemuxer {
    fn default() -> Self {
        Self::new()
    }
}

impl MkvDemuxer {
    /// Creates a new MkvDemuxer with default limits.
    pub fn new() -> Self {
        Self::with_limits(
            DEFAULT_MAX_MKV_BUFFER_CAPACITY,
            DEFAULT_MAX_MKV_ELEMENT_SIZE,
        )
    }

    /// Creates a new MkvDemuxer with explicit memory bounds (Global Rule 3).
    pub fn with_limits(max_buffer_capacity: usize, max_element_size: u64) -> Self {
        Self {
            state: MkvDemuxerState::AwaitingEbml,
            buffer: BytesMut::with_capacity(16384),
            max_buffer_capacity,
            max_element_size,
            current_epoch: 0,
            stream_position: 0,
            header_elements: Vec::new(),
            metadata: None,
            header_snapshot: None,
            timecode_scale_ns: 1_000_000, // 1 ms default
            doc_type: "matroska".to_string(),
            doc_type_version: 1,
            max_id_length: 4,
            max_size_length: 8,
        }
    }

    /// Feeds incoming stream bytes into the demuxer buffer.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), MkvParseError> {
        if self.buffer.len() + chunk.len() > self.max_buffer_capacity {
            return Err(MkvParseError::BufferCapacityExceeded {
                actual: self.buffer.len() + chunk.len(),
                max: self.max_buffer_capacity,
            });
        }
        self.buffer.extend_from_slice(chunk);
        Ok(())
    }

    /// Advances the demuxer and yields the next demux output.
    pub fn next_output(&mut self) -> Result<Option<MkvDemuxOutput>, MkvParseError> {
        // Re-entrancy guard: if ended, return None immediately
        if self.state == MkvDemuxerState::Ended {
            return Ok(None);
        }

        loop {
            if self.buffer.is_empty() {
                return Ok(None);
            }

            // Parse element header at cursor 0
            let header = match MkvElementHeader::parse(&self.buffer)? {
                Some(h) => h,
                None => return Ok(None),
            };

            if header.id.len > self.max_id_length {
                return Err(MkvParseError::ElementIdLengthExceeded(
                    header.id.len,
                    self.max_id_length,
                ));
            }
            if header.size.len > self.max_size_length {
                return Err(MkvParseError::ElementSizeLengthExceeded(
                    header.size.len,
                    self.max_size_length,
                ));
            }

            let id = header.id.value as u32;

            // Handle mid-stream header restart (REQ-051):
            // If an EBML element arrives while streaming or reading headers, reset state and bump epoch
            if id == ID_EBML && self.state != MkvDemuxerState::AwaitingEbml {
                self.state = MkvDemuxerState::AwaitingEbml;
                self.current_epoch += 1;
                self.header_elements.clear();
            }

            match self.state {
                MkvDemuxerState::AwaitingEbml => {
                    if id != ID_EBML {
                        return Err(MkvParseError::InvalidEbmlHeader);
                    }
                    let sz = header.size.value as usize;
                    if sz as u64 > self.max_element_size {
                        return Err(MkvParseError::ElementSizeExceeded {
                            actual: sz as u64,
                            max: self.max_element_size,
                        });
                    }
                    let total_len = header.header_len() + sz;
                    if self.buffer.len() < total_len {
                        return Ok(None);
                    }

                    // Extract EBML element
                    let element_bytes = self.buffer.split_to(total_len).freeze();
                    let data = element_bytes.slice(header.header_len()..);
                    let (dt, dtv, max_id, max_sz) = parse_ebml_element(&data)?;
                    self.doc_type = dt;
                    self.doc_type_version = dtv;
                    self.max_id_length = max_id as usize;
                    self.max_size_length = max_sz as usize;

                    self.header_elements.clear();
                    self.header_elements.push(MkvElement { header, data });
                    self.state = MkvDemuxerState::AwaitingSegment;
                }
                MkvDemuxerState::AwaitingSegment => {
                    if id == ID_SEGMENT {
                        // Segment element header: in Station parity, add Segment header with empty data
                        let total_len = header.header_len();
                        self.buffer.advance(total_len);

                        self.header_elements.push(MkvElement {
                            header,
                            data: Bytes::new(),
                        });
                        self.state = MkvDemuxerState::ReadingHeaders;
                    } else if id == ID_VOID || id == ID_CRC32 {
                        // Skip void/crc padding
                        let sz = header.size.value as usize;
                        let total_len = header.header_len() + sz;
                        if self.buffer.len() < total_len {
                            return Ok(None);
                        }
                        self.buffer.advance(total_len);
                    } else {
                        return Err(MkvParseError::InvalidEbmlHeader);
                    }
                }
                MkvDemuxerState::ReadingHeaders => {
                    if id == ID_CLUSTER {
                        // When the first Cluster is encountered:
                        // 1. Build HeaderSnapshot from all accumulated header elements
                        let mut snapshot_builder = BytesMut::new();
                        for elem in &self.header_elements {
                            snapshot_builder.put_slice(&elem.to_bytes());
                        }
                        let snapshot_bytes = snapshot_builder.freeze();

                        let is_webm = self.doc_type == "webm";
                        let (ctype, mime, ext) = if is_webm {
                            (
                                "WEBM".to_string(),
                                "video/webm".to_string(),
                                ".webm".to_string(),
                            )
                        } else {
                            (
                                "MKV".to_string(),
                                "video/x-matroska".to_string(),
                                ".mkv".to_string(),
                            )
                        };

                        let meta = MkvMetadata {
                            doc_type: self.doc_type.clone(),
                            doc_type_version: self.doc_type_version,
                            content_type: ctype.clone(),
                            mime_type: mime,
                            stream_ext: ext,
                            has_video: true,
                            has_audio: true,
                            timecode_scale_ns: self.timecode_scale_ns,
                            video_codecs: Vec::new(),
                            audio_codecs: Vec::new(),
                            width: None,
                            height: None,
                        };

                        let snapshot = HeaderSnapshot::new(
                            self.current_epoch as i32,
                            self.stream_position as i64,
                            ctype,
                            snapshot_bytes.clone(),
                        );

                        self.metadata = Some(meta.clone());
                        self.header_snapshot = Some(snapshot.clone());
                        self.state = MkvDemuxerState::StreamingClusters;

                        let out = MkvDemuxOutput::HeaderSnapshotReady {
                            snapshot,
                            metadata: Box::new(meta),
                            epoch: self.current_epoch,
                        };
                        self.stream_position += snapshot_bytes.len() as u64;
                        return Ok(Some(out));
                    } else {
                        // Accumulate Segment child header element (Info, Tracks, SeekHead, Tags, etc.)
                        let sz = header.size.value as usize;
                        if sz as u64 > self.max_element_size {
                            return Err(MkvParseError::ElementSizeExceeded {
                                actual: sz as u64,
                                max: self.max_element_size,
                            });
                        }
                        let total_len = header.header_len() + sz;
                        if self.buffer.len() < total_len {
                            return Ok(None);
                        }

                        let element_bytes = self.buffer.split_to(total_len).freeze();
                        let data = element_bytes.slice(header.header_len()..);

                        if id == ID_INFO {
                            // Extract TimecodeScale if present
                            let mut cursor = 0;
                            while cursor < data.len() {
                                if let Ok(Some(child)) = MkvElementHeader::parse(&data[cursor..]) {
                                    if child.id.value as u32 == ID_TIMECODE_SCALE {
                                        let csz = child.size.value as usize;
                                        let cstart = cursor + child.header_len();
                                        if cstart + csz <= data.len() {
                                            let mut scale = 0u64;
                                            for &b in &data[cstart..cstart + csz] {
                                                scale = (scale << 8) | (b as u64);
                                            }
                                            if scale > 0 {
                                                self.timecode_scale_ns = scale;
                                            }
                                        }
                                    }
                                    let step = child.header_len() + child.size.value as usize;
                                    if step == 0 || cursor + step > data.len() {
                                        break;
                                    }
                                    cursor += step;
                                } else {
                                    break;
                                }
                            }
                        }

                        self.header_elements.push(MkvElement { header, data });
                    }
                }
                MkvDemuxerState::StreamingClusters => {
                    if id == ID_CLUSTER {
                        // Cluster header: advance buffer by cluster header length and emit as packet
                        let total_len = header.header_len();
                        let raw_bytes = self.buffer.split_to(total_len).freeze();
                        let pos = self.stream_position;
                        self.stream_position += total_len as u64;
                        return Ok(Some(MkvDemuxOutput::ContentPacket {
                            packet: raw_bytes,
                            epoch: self.current_epoch,
                            position: pos,
                            is_keyframe: true, // Cluster boundary is a safe sync point
                        }));
                    } else if id == ID_SIMPLE_BLOCK || id == ID_BLOCK_GROUP {
                        let sz = header.size.value as usize;
                        if sz as u64 > self.max_element_size {
                            return Err(MkvParseError::ElementSizeExceeded {
                                actual: sz as u64,
                                max: self.max_element_size,
                            });
                        }
                        let total_len = header.header_len() + sz;
                        if self.buffer.len() < total_len {
                            return Ok(None);
                        }

                        let block_bytes = self.buffer.split_to(total_len).freeze();
                        let payload = &block_bytes[header.header_len()..];
                        let is_keyframe = inspect_mkv_keyframe(id, payload);

                        let pos = self.stream_position;
                        self.stream_position += total_len as u64;
                        return Ok(Some(MkvDemuxOutput::ContentPacket {
                            packet: block_bytes,
                            epoch: self.current_epoch,
                            position: pos,
                            is_keyframe,
                        }));
                    } else {
                        // Other cluster elements (Timecode, PrevSize, Void, CRC32, etc.)
                        let sz = header.size.value as usize;
                        if sz as u64 > self.max_element_size {
                            return Err(MkvParseError::ElementSizeExceeded {
                                actual: sz as u64,
                                max: self.max_element_size,
                            });
                        }
                        let total_len = header.header_len() + sz;
                        if self.buffer.len() < total_len {
                            return Ok(None);
                        }

                        let elt_bytes = self.buffer.split_to(total_len).freeze();
                        let pos = self.stream_position;
                        self.stream_position += total_len as u64;
                        return Ok(Some(MkvDemuxOutput::ContentPacket {
                            packet: elt_bytes,
                            epoch: self.current_epoch,
                            position: pos,
                            is_keyframe: false,
                        }));
                    }
                }
                MkvDemuxerState::Ended => {
                    return Ok(None);
                }
            }
        }
    }

    /// Explicitly signals EOF on input stream.
    pub fn signal_eof(&mut self) -> Result<Option<MkvDemuxOutput>, MkvParseError> {
        if self.state == MkvDemuxerState::Ended {
            return Ok(None);
        }
        self.state = MkvDemuxerState::Ended;
        Ok(Some(MkvDemuxOutput::EndOfStream))
    }

    /// Returns the active metadata, if parsed.
    pub fn metadata(&self) -> Option<&MkvMetadata> {
        self.metadata.as_ref()
    }

    /// Returns the active header snapshot, if generated.
    pub fn header_snapshot(&self) -> Option<&HeaderSnapshot> {
        self.header_snapshot.as_ref()
    }

    /// Returns the current stream epoch.
    pub fn current_epoch(&self) -> u64 {
        self.current_epoch
    }

    /// Returns the current stream position in bytes.
    pub fn stream_position(&self) -> u64 {
        self.stream_position
    }

    /// Returns the current state of the demuxer.
    pub fn state(&self) -> MkvDemuxerState {
        self.state
    }
}
