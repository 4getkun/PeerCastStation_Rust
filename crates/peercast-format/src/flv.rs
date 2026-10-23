use bytes::{BufMut, Bytes, BytesMut};
use peercast_core::cache::HeaderSnapshot;
use std::collections::HashMap;
use thiserror::Error;

/// Maximum allowed FLV tag payload size (4 MiB) to prevent unbounded allocations (REQ-027, REQ-051).
pub const DEFAULT_MAX_FLV_TAG_SIZE: usize = 4 * 1024 * 1024;

/// Standard length of FLV file header prelude with PreviousTagSize0 (REQ-052).
pub const FLV_HEADER_PRELUDE_LEN: usize = 13;

/// Errors produced during FLV parsing and validation.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum FlvParseError {
    #[error("Invalid FLV signature: expected 'FLV', found {0:?}")]
    InvalidSignature([u8; 3]),

    #[error("Unsupported FLV version {0}: only version 1 is supported")]
    UnsupportedVersion(u8),

    #[error("Invalid FLV DataOffset {0}: must be at least 9")]
    InvalidDataOffset(u32),

    #[error("Invalid PreviousTagSize0: expected 0, found {0}")]
    InvalidPreviousTagSize0(u32),

    #[error("Malformed tag size ({actual} bytes) exceeds budget of {max} bytes")]
    TagTooLarge { actual: usize, max: usize },

    #[error("Malformed tag footer: expected PreviousTagSize {expected}, got {actual}")]
    InvalidPreviousTagSize { expected: u32, actual: u32 },

    #[error("Unknown or invalid tag type: 0x{0:02x}")]
    InvalidTagType(u8),

    #[error("Malformed reserved bits in tag header: 0x{0:02x}")]
    MalformedReservedBits(u8),

    #[error("Extended FLV tag detected: {0} is not supported in first slice")]
    UnsupportedExtendedCodec(&'static str),

    #[error("Unexpected end of data while parsing {0}")]
    UnexpectedEof(&'static str),

    #[error("Parser is in a failed state due to a previous unrecoverable error")]
    FailedState,

    #[error("Malformed AMF0 metadata: {0}")]
    MalformedMetadata(String),
}

/// FLV Tag Type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlvTagType {
    Audio = 8,
    Video = 9,
    ScriptAmf3 = 15,
    ScriptData = 18,
}

impl FlvTagType {
    pub fn from_u8(val: u8) -> Result<Self, FlvParseError> {
        match val {
            8 => Ok(Self::Audio),
            9 => Ok(Self::Video),
            15 => Ok(Self::ScriptAmf3),
            18 => Ok(Self::ScriptData),
            other => Err(FlvParseError::InvalidTagType(other)),
        }
    }
}

/// Parsed FLV File Header (REQ-052).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlvFileHeader {
    pub has_audio: bool,
    pub has_video: bool,
    pub data_offset: u32,
    pub raw_bytes: Bytes,
}

impl FlvFileHeader {
    /// Parses a 9-byte FLV file header.
    pub fn parse(data: &[u8]) -> Result<Self, FlvParseError> {
        if data.len() < 9 {
            return Err(FlvParseError::UnexpectedEof("FLV file header"));
        }
        if data[0] != b'F' || data[1] != b'L' || data[2] != b'V' {
            return Err(FlvParseError::InvalidSignature([data[0], data[1], data[2]]));
        }
        if data[3] != 1 {
            return Err(FlvParseError::UnsupportedVersion(data[3]));
        }
        let flags = data[4];
        let has_audio = (flags & 0x04) != 0;
        let has_video = (flags & 0x01) != 0;
        let data_offset = u32::from_be_bytes([data[5], data[6], data[7], data[8]]);
        if data_offset < 9 {
            return Err(FlvParseError::InvalidDataOffset(data_offset));
        }
        Ok(Self {
            has_audio,
            has_video,
            data_offset,
            raw_bytes: Bytes::copy_from_slice(&data[..9]),
        })
    }
}

/// Parsed 11-byte FLV Tag Header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlvTagHeader {
    pub tag_type: FlvTagType,
    pub filter: bool,
    pub data_size: u32,
    pub timestamp: u32,
    pub stream_id: u32,
}

impl FlvTagHeader {
    /// Parses an 11-byte FLV tag header.
    pub fn parse(data: &[u8]) -> Result<Self, FlvParseError> {
        if data.len() < 11 {
            return Err(FlvParseError::UnexpectedEof("FLV tag header"));
        }
        let first_byte = data[0];
        if (first_byte & 0xC0) != 0 {
            return Err(FlvParseError::MalformedReservedBits(first_byte));
        }
        let filter = (first_byte & 0x20) != 0;
        let tag_type = FlvTagType::from_u8(first_byte & 0x1F)?;
        let data_size = ((data[1] as u32) << 16) | ((data[2] as u32) << 8) | (data[3] as u32);
        let ts_low = ((data[4] as u32) << 16) | ((data[5] as u32) << 8) | (data[6] as u32);
        let ts_high = data[7] as u32;
        let timestamp = (ts_high << 24) | ts_low;
        let stream_id = ((data[8] as u32) << 16) | ((data[9] as u32) << 8) | (data[10] as u32);

        Ok(Self {
            tag_type,
            filter,
            data_size,
            timestamp,
            stream_id,
        })
    }
}

/// A parsed FLV Tag with header, body and PreviousTagSize footer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlvTag {
    pub header: FlvTagHeader,
    pub body: Bytes,
    pub previous_tag_size: u32,
}

impl FlvTag {
    pub fn is_valid_footer(&self) -> bool {
        self.previous_tag_size == self.header.data_size + 11
    }

    pub fn is_audio(&self) -> bool {
        self.header.tag_type == FlvTagType::Audio
    }

    pub fn is_video(&self) -> bool {
        self.header.tag_type == FlvTagType::Video
    }

    pub fn is_script(&self) -> bool {
        self.header.tag_type == FlvTagType::ScriptData
            || self.header.tag_type == FlvTagType::ScriptAmf3
    }

    /// Checks if this video tag contains an AVC sequence header (AVCDecoderConfigurationRecord).
    pub fn is_avc_sequence_header(&self) -> bool {
        if self.header.tag_type != FlvTagType::Video || self.body.len() < 2 {
            return false;
        }
        // Not extended video, codec is AVC (0x07), and AVCPacketType == 0
        (self.body[0] & 0x0F) == 7 && self.body[1] == 0
    }

    /// Checks if this audio tag contains an AAC sequence header (AudioSpecificConfig).
    pub fn is_aac_sequence_header(&self) -> bool {
        if self.header.tag_type != FlvTagType::Audio || self.body.len() < 2 {
            return false;
        }
        // SoundFormat == 10 (AAC), AACPacketType == 0
        (self.body[0] & 0xF0) == 0xA0 && self.body[1] == 0
    }

    /// Detects video keyframe (frameType 1 = Keyframe, 4 = Generated keyframe).
    pub fn is_keyframe(&self) -> bool {
        if self.header.tag_type != FlvTagType::Video || self.body.is_empty() {
            return false;
        }
        let frame_type = self.body[0] >> 4;
        frame_type == 1 || frame_type == 4
    }

    /// Checks if the tag is an enhanced/extended FLV video tag (ExVideo, bit 7 set).
    pub fn is_extended_video(&self) -> bool {
        if self.header.tag_type != FlvTagType::Video || self.body.is_empty() {
            return false;
        }
        ((self.body[0] >> 4) & 0b1000) != 0
    }

    /// Checks if the tag is an enhanced/extended FLV audio tag (ExAudio, format 9).
    pub fn is_extended_audio(&self) -> bool {
        if self.header.tag_type != FlvTagType::Audio || self.body.is_empty() {
            return false;
        }
        ((self.body[0] & 0xF0) >> 4) == 9
    }

    /// Serializes this tag to standard wire bytes with an optional overridden timestamp.
    /// Wire format: 11-byte TagHeader + Body + 4-byte PreviousTagSize.
    pub fn to_wire_bytes(&self, override_timestamp: Option<u32>) -> Bytes {
        let body_len = self.body.len();
        let mut out = BytesMut::with_capacity(11 + body_len + 4);

        let filter_bit = if self.header.filter { 0x20 } else { 0x00 };
        out.put_u8((self.header.tag_type as u8) | filter_bit);

        out.put_u8(((body_len >> 16) & 0xFF) as u8);
        out.put_u8(((body_len >> 8) & 0xFF) as u8);
        out.put_u8((body_len & 0xFF) as u8);

        let ts = override_timestamp.unwrap_or(self.header.timestamp);
        out.put_u8(((ts >> 16) & 0xFF) as u8);
        out.put_u8(((ts >> 8) & 0xFF) as u8);
        out.put_u8((ts & 0xFF) as u8);
        out.put_u8(((ts >> 24) & 0xFF) as u8);

        out.put_u8(((self.header.stream_id >> 16) & 0xFF) as u8);
        out.put_u8(((self.header.stream_id >> 8) & 0xFF) as u8);
        out.put_u8((self.header.stream_id & 0xFF) as u8);

        out.put_slice(&self.body);

        out.put_u32(11 + body_len as u32);
        out.freeze()
    }
}

/// Lightweight, bounded AMF0 metadata parser for `onMetaData` script tags.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlvMetadata {
    pub name: String,
    pub bitrate_kbps: Option<u32>,
    pub videodatarate: Option<f64>,
    pub audiodatarate: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub framerate: Option<f64>,
    pub duration: Option<f64>,
    pub encoder: Option<String>,
    pub properties: HashMap<String, String>,
}

impl FlvMetadata {
    /// Attempts to parse AMF0 ScriptData from a tag body.
    pub fn parse_amf0(mut data: &[u8]) -> Result<Option<Self>, FlvParseError> {
        if data.is_empty() {
            return Ok(None);
        }

        // AMF0 value 1: must be string
        let val1 = read_amf0_string(&mut data)?;
        let mut meta = FlvMetadata::default();

        let object_slice = if val1 == "@setDataFrame" {
            // OBS / ffmpeg @setDataFrame pattern: next string is "onMetaData"
            let val2 = read_amf0_string(&mut data)?;
            if val2 != "onMetaData" {
                return Ok(None);
            }
            meta.name = val2;
            data
        } else if val1 == "onMetaData" {
            meta.name = val1;
            data
        } else {
            return Ok(None);
        };

        // Parse ECMA array (marker 8) or Object (marker 3)
        let mut cur = object_slice;
        if cur.is_empty() {
            return Ok(Some(meta));
        }

        let marker = cur[0];
        cur = &cur[1..];

        if marker == 8 {
            // ECMA Array: 4-byte count
            if cur.len() < 4 {
                return Ok(Some(meta));
            }
            cur = &cur[4..];
        } else if marker != 3 {
            // Not an object or ECMA array
            return Ok(Some(meta));
        }

        // Parse key-value pairs until marker 9 (ObjectEnd)
        while cur.len() >= 2 {
            let key_len = u16::from_be_bytes([cur[0], cur[1]]) as usize;
            cur = &cur[2..];
            if key_len == 0 && !cur.is_empty() && cur[0] == 9 {
                break;
            }
            if cur.len() < key_len {
                break;
            }
            let key = String::from_utf8_lossy(&cur[..key_len]).to_string();
            cur = &cur[key_len..];

            if cur.is_empty() {
                break;
            }
            let val_marker = cur[0];
            cur = &cur[1..];

            match val_marker {
                0 => {
                    // Number (IEEE 754 8-byte double BE)
                    if cur.len() < 8 {
                        break;
                    }
                    let num = f64::from_be_bytes(cur[..8].try_into().unwrap());
                    cur = &cur[8..];
                    match key.as_str() {
                        "videodatarate" => meta.videodatarate = Some(num),
                        "audiodatarate" => meta.audiodatarate = Some(num),
                        "width" => meta.width = Some(num as u32),
                        "height" => meta.height = Some(num as u32),
                        "framerate" => meta.framerate = Some(num),
                        "duration" => meta.duration = Some(num),
                        _ => {}
                    }
                    meta.properties.insert(key, format!("{num}"));
                }
                1 => {
                    // Boolean (1 byte)
                    if cur.is_empty() {
                        break;
                    }
                    let b = cur[0] != 0;
                    cur = &cur[1..];
                    meta.properties.insert(key, b.to_string());
                }
                2 => {
                    // String (2-byte length + UTF-8)
                    if cur.len() < 2 {
                        break;
                    }
                    let slen = u16::from_be_bytes([cur[0], cur[1]]) as usize;
                    cur = &cur[2..];
                    if cur.len() < slen {
                        break;
                    }
                    let s = String::from_utf8_lossy(&cur[..slen]).to_string();
                    cur = &cur[slen..];
                    if key == "encoder" {
                        meta.encoder = Some(s.clone());
                    } else if key == "maxBitrate" {
                        // Check for strings like "500k"
                        let clean = s.trim_end_matches('k').trim_end_matches('K');
                        if let Ok(b) = clean.parse::<f64>() {
                            meta.bitrate_kbps = Some(b.ceil() as u32);
                        }
                    }
                    meta.properties.insert(key, s);
                }
                8 => {
                    // Nested ECMA Array - skip length + elements
                    if cur.len() < 4 {
                        break;
                    }
                    cur = &cur[4..];
                    skip_amf0_object(&mut cur);
                }
                3 => {
                    // Nested Object - skip elements
                    skip_amf0_object(&mut cur);
                }
                9 => {
                    // ObjectEnd
                    break;
                }
                _ => {
                    // Unsupported marker, terminate object parsing cleanly
                    break;
                }
            }
        }

        // Calculate total bitrate in kbps if not already set from maxBitrate
        if meta.bitrate_kbps.is_none() {
            let mut total = 0.0;
            if let Some(v) = meta.videodatarate {
                total += v;
            }
            if let Some(a) = meta.audiodatarate {
                total += a;
            }
            if total > 0.0 {
                meta.bitrate_kbps = Some(total.ceil() as u32);
            }
        }

        Ok(Some(meta))
    }
}

fn read_amf0_string(data: &mut &[u8]) -> Result<String, FlvParseError> {
    if data.is_empty() || data[0] != 2 {
        return Err(FlvParseError::MalformedMetadata(
            "Expected AMF0 string marker (2)".to_string(),
        ));
    }
    *data = &data[1..];
    if data.len() < 2 {
        return Err(FlvParseError::UnexpectedEof("AMF0 string length"));
    }
    let len = u16::from_be_bytes([data[0], data[1]]) as usize;
    *data = &data[2..];
    if data.len() < len {
        return Err(FlvParseError::UnexpectedEof("AMF0 string body"));
    }
    let s = String::from_utf8_lossy(&data[..len]).to_string();
    *data = &data[len..];
    Ok(s)
}

fn skip_amf0_object(cur: &mut &[u8]) {
    while cur.len() >= 2 {
        let key_len = u16::from_be_bytes([cur[0], cur[1]]) as usize;
        *cur = &cur[2..];
        if key_len == 0 && !cur.is_empty() && cur[0] == 9 {
            *cur = &cur[1..];
            break;
        }
        if cur.len() < key_len {
            break;
        }
        *cur = &cur[key_len..];
        if cur.is_empty() {
            break;
        }
        let marker = cur[0];
        *cur = &cur[1..];
        match marker {
            0 => {
                if cur.len() < 8 {
                    break;
                }
                *cur = &cur[8..];
            }
            1 => {
                if cur.is_empty() {
                    break;
                }
                *cur = &cur[1..];
            }
            2 => {
                if cur.len() < 2 {
                    break;
                }
                let slen = u16::from_be_bytes([cur[0], cur[1]]) as usize;
                *cur = &cur[2..];
                if cur.len() < slen {
                    break;
                }
                *cur = &cur[slen..];
            }
            9 => break,
            _ => break,
        }
    }
}

/// State machine phase for incremental FLV parsing (Global Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlvParserPhase {
    /// Waiting for 13-byte FLV file header prelude (9 bytes header + 4 bytes PreviousTagSize0)
    NeedHeader,
    /// Waiting for 11-byte tag header
    NeedTagHeader,
    /// Waiting for tag body and 4-byte PreviousTagSize footer
    NeedTagBody,
    /// Unrecoverable parser failure
    Failed,
}

/// Event emitted by incremental FLV stream parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlvParserEvent {
    FileHeader(FlvFileHeader),
    Tag(FlvTag),
}

/// Incremental, streaming FLV parser that handles single-byte chunk splits and fragmentation.
#[derive(Debug)]
pub struct FlvStreamParser {
    phase: FlvParserPhase,
    buffer: BytesMut,
    max_tag_size: usize,
    current_header: Option<FlvTagHeader>,
}

impl Default for FlvStreamParser {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_FLV_TAG_SIZE)
    }
}

impl FlvStreamParser {
    pub fn new(max_tag_size: usize) -> Self {
        Self {
            phase: FlvParserPhase::NeedHeader,
            buffer: BytesMut::with_capacity(8192),
            max_tag_size,
            current_header: None,
        }
    }

    pub fn phase(&self) -> FlvParserPhase {
        self.phase
    }

    /// Feeds incoming byte slice to the parser's internal buffer.
    pub fn feed(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// Pulls the next parsed event, if enough bytes have accumulated.
    pub fn next_event(&mut self) -> Result<Option<FlvParserEvent>, FlvParseError> {
        loop {
            // Re-entrancy early return guard (Global Rule 2)
            if self.phase == FlvParserPhase::Failed {
                return Err(FlvParseError::FailedState);
            }

            match self.phase {
                FlvParserPhase::NeedHeader => {
                    // FLV output requires the file header and PreviousTagSize0: 13 bytes (REQ-052)
                    if self.buffer.len() < FLV_HEADER_PRELUDE_LEN {
                        return Ok(None);
                    }
                    let prelude = self.buffer.split_to(FLV_HEADER_PRELUDE_LEN);
                    let header = match FlvFileHeader::parse(&prelude[..9]) {
                        Ok(h) => h,
                        Err(err) => {
                            self.phase = FlvParserPhase::Failed;
                            return Err(err);
                        }
                    };
                    let prev_tag_size_0 =
                        u32::from_be_bytes([prelude[9], prelude[10], prelude[11], prelude[12]]);
                    if prev_tag_size_0 != 0 {
                        self.phase = FlvParserPhase::Failed;
                        return Err(FlvParseError::InvalidPreviousTagSize0(prev_tag_size_0));
                    }

                    self.phase = FlvParserPhase::NeedTagHeader;
                    return Ok(Some(FlvParserEvent::FileHeader(header)));
                }

                FlvParserPhase::NeedTagHeader => {
                    if self.buffer.len() < 11 {
                        return Ok(None);
                    }
                    let tag_header_bytes = self.buffer.split_to(11);
                    let tag_header = match FlvTagHeader::parse(&tag_header_bytes) {
                        Ok(th) => th,
                        Err(err) => {
                            self.phase = FlvParserPhase::Failed;
                            return Err(err);
                        }
                    };

                    if tag_header.data_size as usize > self.max_tag_size {
                        self.phase = FlvParserPhase::Failed;
                        return Err(FlvParseError::TagTooLarge {
                            actual: tag_header.data_size as usize,
                            max: self.max_tag_size,
                        });
                    }

                    self.current_header = Some(tag_header);
                    self.phase = FlvParserPhase::NeedTagBody;
                }

                FlvParserPhase::NeedTagBody => {
                    let header = self.current_header.as_ref().unwrap();
                    let needed_len = header.data_size as usize + 4;
                    if self.buffer.len() < needed_len {
                        return Ok(None);
                    }

                    let payload = self.buffer.split_to(needed_len).freeze();
                    let body = payload.slice(..header.data_size as usize);
                    let footer = &payload[header.data_size as usize..];
                    let previous_tag_size =
                        u32::from_be_bytes([footer[0], footer[1], footer[2], footer[3]]);

                    let expected_prev_tag_size = header.data_size + 11;
                    if previous_tag_size != expected_prev_tag_size {
                        self.phase = FlvParserPhase::Failed;
                        return Err(FlvParseError::InvalidPreviousTagSize {
                            expected: expected_prev_tag_size,
                            actual: previous_tag_size,
                        });
                    }

                    let tag_header = self.current_header.take().unwrap();
                    let tag = FlvTag {
                        header: tag_header,
                        body,
                        previous_tag_size,
                    };

                    self.phase = FlvParserPhase::NeedTagHeader;
                    return Ok(Some(FlvParserEvent::Tag(tag)));
                }

                FlvParserPhase::Failed => unreachable!(),
            }
        }
    }
}

/// Builds and maintains the bounded HeaderSnapshot for FLV channels (REQ-051, REQ-052).
///
/// Produces the mandatory 13-byte prelude ('FLV', version 1, flags, DataOffset 9, PreviousTagSize0 = 0),
/// followed by metadata (onMetaData), AAC sequence header (if present), and AVC sequence header (if present).
/// Emitting 9 bytes alone without PreviousTagSize0 is strictly prohibited!
#[derive(Debug, Clone)]
pub struct FlvHeaderSnapshotBuilder {
    epoch: i32,
    has_audio: bool,
    has_video: bool,
    metadata_tag: Option<FlvTag>,
    audio_sequence_header: Option<FlvTag>,
    video_sequence_header: Option<FlvTag>,
}

impl Default for FlvHeaderSnapshotBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl FlvHeaderSnapshotBuilder {
    pub fn new() -> Self {
        Self {
            epoch: 1,
            has_audio: false,
            has_video: false,
            metadata_tag: None,
            audio_sequence_header: None,
            video_sequence_header: None,
        }
    }

    pub fn epoch(&self) -> i32 {
        self.epoch
    }

    pub fn set_flags(&mut self, has_audio: bool, has_video: bool) {
        self.has_audio = has_audio;
        self.has_video = has_video;
    }

    /// Updates metadata tag. Returns true if metadata changed and triggered epoch increment.
    pub fn update_metadata(&mut self, tag: FlvTag) -> bool {
        if let Some(existing) = &self.metadata_tag {
            if existing.body == tag.body {
                return false;
            }
        }
        self.metadata_tag = Some(tag);
        self.epoch += 1;
        true
    }

    /// Updates AAC audio sequence header. Returns true if header changed and triggered epoch increment.
    pub fn update_audio_header(&mut self, tag: FlvTag) -> bool {
        if let Some(existing) = &self.audio_sequence_header {
            if existing.body == tag.body {
                return false;
            }
        }
        self.audio_sequence_header = Some(tag);
        self.has_audio = true;
        self.epoch += 1;
        true
    }

    /// Updates AVC video sequence header. Returns true if header changed and triggered epoch increment.
    pub fn update_video_header(&mut self, tag: FlvTag) -> bool {
        if let Some(existing) = &self.video_sequence_header {
            if existing.body == tag.body {
                return false;
            }
        }
        self.video_sequence_header = Some(tag);
        self.has_video = true;
        self.epoch += 1;
        true
    }

    /// Builds a HeaderSnapshot containing 13-byte prelude, metadata, audio config, and video config.
    pub fn build_snapshot(&self, pos: i64) -> HeaderSnapshot {
        let mut out = BytesMut::with_capacity(1024);

        // 13-byte FLV File Header + PreviousTagSize0 (REQ-052)
        let flags = (if self.has_audio { 0x04 } else { 0x00 })
            | (if self.has_video { 0x01 } else { 0x00 });
        out.put_u8(b'F');
        out.put_u8(b'L');
        out.put_u8(b'V');
        out.put_u8(1); // Version 1
        out.put_u8(flags);
        out.put_u32(9); // DataOffset = 9
        out.put_u32(0); // PreviousTagSize0 = 0

        // In standard Station / YT FLV headers, sequence headers have timestamp 0
        if let Some(meta) = &self.metadata_tag {
            out.extend_from_slice(&meta.to_wire_bytes(Some(0)));
        }
        if let Some(audio) = &self.audio_sequence_header {
            out.extend_from_slice(&audio.to_wire_bytes(Some(0)));
        }
        if let Some(video) = &self.video_sequence_header {
            out.extend_from_slice(&video.to_wire_bytes(Some(0)));
        }

        let data = out.freeze();
        let retained_bytes = data.len();

        HeaderSnapshot {
            epoch: self.epoch,
            pos,
            container_type: "FLV".to_string(),
            data,
            retained_bytes,
        }
    }
}

/// Output event from `FlvDemuxer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlvDemuxOutput {
    /// A new or updated header snapshot is ready (stream start or epoch reset)
    HeaderChanged(HeaderSnapshot),
    /// Content tag ready for fanout or caching
    ContentTag {
        epoch: i32,
        tag: FlvTag,
        is_keyframe: bool,
    },
}

/// High-level FLV Demuxer that coordinates streaming parsing, header snapshot generation,
/// epoch tracking, and extended tag guards (REQ-050, REQ-051, REQ-052).
#[derive(Debug)]
pub struct FlvDemuxer {
    parser: FlvStreamParser,
    builder: FlvHeaderSnapshotBuilder,
    position: i64,
    metadata: Option<FlvMetadata>,
    initial_header_emitted: bool,
}

impl Default for FlvDemuxer {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_FLV_TAG_SIZE)
    }
}

impl FlvDemuxer {
    pub fn new(max_tag_size: usize) -> Self {
        Self {
            parser: FlvStreamParser::new(max_tag_size),
            builder: FlvHeaderSnapshotBuilder::new(),
            position: 0,
            metadata: None,
            initial_header_emitted: false,
        }
    }

    pub fn position(&self) -> i64 {
        self.position
    }

    pub fn metadata(&self) -> Option<&FlvMetadata> {
        self.metadata.as_ref()
    }

    pub fn epoch(&self) -> i32 {
        self.builder.epoch()
    }

    /// Feeds incoming chunks into the demuxer.
    pub fn feed(&mut self, chunk: &[u8]) {
        self.parser.feed(chunk);
    }

    /// Polls the next demux output event.
    pub fn next_output(&mut self) -> Result<Option<FlvDemuxOutput>, FlvParseError> {
        while let Some(event) = self.parser.next_event()? {
            match event {
                FlvParserEvent::FileHeader(header) => {
                    self.builder.set_flags(header.has_audio, header.has_video);
                    // Do not emit header immediately until first tags (metadata/configs) are known,
                    // or emit minimal snapshot if stream has no tags.
                }

                FlvParserEvent::Tag(tag) => {
                    // Extended codec guard: stop/block when extended tags silently treated as supported
                    if tag.is_extended_video() {
                        return Err(FlvParseError::UnsupportedExtendedCodec("Enhanced ExVideo"));
                    }
                    if tag.is_extended_audio() {
                        return Err(FlvParseError::UnsupportedExtendedCodec("Enhanced ExAudio"));
                    }

                    let is_keyframe = tag.is_keyframe();

                    if tag.is_script() {
                        if let Ok(Some(parsed_meta)) = FlvMetadata::parse_amf0(&tag.body) {
                            self.metadata = Some(parsed_meta);
                            self.builder.update_metadata(tag.clone());
                            let snapshot = self.builder.build_snapshot(self.position);
                            self.initial_header_emitted = true;
                            return Ok(Some(FlvDemuxOutput::HeaderChanged(snapshot)));
                        }
                    } else if tag.is_avc_sequence_header() {
                        self.builder.update_video_header(tag.clone());
                        let snapshot = self.builder.build_snapshot(self.position);
                        self.initial_header_emitted = true;
                        return Ok(Some(FlvDemuxOutput::HeaderChanged(snapshot)));
                    } else if tag.is_aac_sequence_header() {
                        self.builder.update_audio_header(tag.clone());
                        let snapshot = self.builder.build_snapshot(self.position);
                        self.initial_header_emitted = true;
                        return Ok(Some(FlvDemuxOutput::HeaderChanged(snapshot)));
                    }

                    // If we have not emitted an initial header yet, emit one now before first media content
                    if !self.initial_header_emitted {
                        self.initial_header_emitted = true;
                        let snapshot = self.builder.build_snapshot(self.position);
                        return Ok(Some(FlvDemuxOutput::HeaderChanged(snapshot)));
                    }

                    let tag_wire_len = 11 + tag.body.len() + 4;
                    self.position += tag_wire_len as i64;

                    return Ok(Some(FlvDemuxOutput::ContentTag {
                        epoch: self.builder.epoch(),
                        tag,
                        is_keyframe,
                    }));
                }
            }
        }
        Ok(None)
    }
}
