//! Ogg container framing (RFC 3533) and codec demuxer (REQ-051, REQ-055, REQ-056).
//!
//! Enforces:
//! - REQ-056: Ogg container support is explicitly scoped and NOT equated with every
//!   arbitrary codec it can contain. Supported codecs: Vorbis (`audio/ogg`), Opus (`audio/ogg`),
//!   and Theora (`video/ogg`). Unsupported codecs are safely rejected.
//! - REQ-051: Header snapshot retention, content epoch bumping on stream restart/chaining,
//!   and invalidation of stale stream state.
//! - REQ-055: Format-aware safe-start selection. Audio-only Ogg streams (Vorbis, Opus)
//!   admit packets immediately after headers without waiting for video keyframes.
//! - Global Rule 2: Explicit state machine with entry re-entrancy guards.
//! - Global Rule 3: Bounded memory buffers and zero unbounded allocations.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use peercast_core::cache::HeaderSnapshot;
use thiserror::Error;

/// Standard Ogg capture pattern: `OggS`
pub const OGG_CAPTURE_PATTERN: [u8; 4] = *b"OggS";

/// Base Ogg page header length (before segment table): 27 bytes.
pub const OGG_BASE_HEADER_LEN: usize = 27;

/// Maximum allowed Ogg page body length: 255 segments * 255 bytes = 65,025 bytes (RFC 3533).
pub const MAX_OGG_PAGE_BODY_LEN: usize = 255 * 255;

/// Maximum allowed Ogg total page size: 27 + 255 + 65,025 = 65,307 bytes.
pub const MAX_OGG_PAGE_SIZE: usize = OGG_BASE_HEADER_LEN + 255 + MAX_OGG_PAGE_BODY_LEN;

/// Maximum reassembled packet size limit (2 MiB) to prevent memory exhaustion (Global Rule 3).
pub const DEFAULT_MAX_OGG_PACKET_SIZE: usize = 2 * 1024 * 1024;

/// Default maximum buffer capacity for Ogg stream (8 MiB).
pub const DEFAULT_MAX_OGG_BUFFER_CAPACITY: usize = 8 * 1024 * 1024;

/// Header type flags (RFC 3533).
pub const OGG_FLAG_CONTINUED_PACKET: u8 = 0x01;
pub const OGG_FLAG_BOS: u8 = 0x02; // Beginning of Stream
pub const OGG_FLAG_EOS: u8 = 0x04; // End of Stream

/// Supported Ogg elementary stream codec types (REQ-056).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OggCodec {
    Vorbis,
    Opus,
    Theora,
    Unsupported,
}

impl OggCodec {
    /// Detects codec identity from the initial BOS packet payload.
    pub fn detect_from_bos_packet(payload: &[u8]) -> Self {
        if payload.len() >= 7 && payload[0] == 0x01 && &payload[1..7] == b"vorbis" {
            Self::Vorbis
        } else if payload.len() >= 8 && &payload[0..8] == b"OpusHead" {
            Self::Opus
        } else if payload.len() >= 7 && payload[0] == 0x80 && &payload[1..7] == b"theora" {
            Self::Theora
        } else {
            Self::Unsupported
        }
    }

    /// Returns the number of initial header packets required for decoder initialization.
    pub fn required_header_packet_count(&self) -> usize {
        match self {
            Self::Vorbis => 3, // Identification, Comment, Setup
            Self::Opus => 2,   // OpusHead, OpusTags
            Self::Theora => 3, // Identification, Comment, Setup
            Self::Unsupported => 0,
        }
    }
}

/// Errors encountered during Ogg stream parsing.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum OggParseError {
    #[error("Invalid capture pattern: expected 'OggS'")]
    InvalidCapturePattern,

    #[error("Unsupported Ogg stream structure version: {0} (expected 0)")]
    UnsupportedVersion(u8),

    #[error("Truncated Ogg page: needed {needed} bytes, but only {available} available")]
    TruncatedPage { needed: usize, available: usize },

    #[error("Ogg packet size {actual} exceeded configured maximum limit {max} (Global Rule 3)")]
    PacketSizeExceeded { actual: usize, max: usize },

    #[error("Unsupported codec in Ogg container (REQ-056: Ogg container support is not universal)")]
    UnsupportedCodec,

    #[error("Invalid packet continuation without preceding continued segment")]
    InvalidPacketContinuation,

    #[error("CRC-32 checksum mismatch on Ogg page: expected 0x{expected:08X}, got 0x{actual:08X}")]
    CrcMismatch { expected: u32, actual: u32 },

    #[error("Buffer capacity {actual} exceeded configured maximum {max} (Global Rule 3)")]
    BufferCapacityExceeded { actual: usize, max: usize },

    #[error("Unexpected EOF during stream parsing")]
    UnexpectedEof,
}

/// Computes RFC 3533 standard CRC-32 checksum for an Ogg page (polynomial 0x04C11DB7).
pub fn compute_ogg_crc(data: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in data {
        crc = (crc << 8) ^ OGG_CRC_TABLE[((crc >> 24) as u8 ^ b) as usize];
    }
    crc
}

// Precomputed RFC 3533 CRC-32 lookup table for polynomial 0x04C11DB7
static OGG_CRC_TABLE: [u32; 256] = [
    0x00000000, 0x04C11DB7, 0x09823B6E, 0x0D4326D9, 0x130476DC, 0x17C56B6B, 0x1A864DB2, 0x1E475005,
    0x2608EDB8, 0x22C9F00F, 0x2F8AD6D6, 0x2B4BCB61, 0x350C9B64, 0x31CD86D3, 0x3C8EA00A, 0x384FBDBD,
    0x4C11DB70, 0x48D0C6C7, 0x4593E01E, 0x4152FDA9, 0x5F15ADAC, 0x5BD4B01B, 0x569796C2, 0x52568B75,
    0x6A1936C8, 0x6ED82B7F, 0x639B0DA6, 0x675A1011, 0x791D4014, 0x7DDC5DA3, 0x709F7B7A, 0x745E66CD,
    0x9823B6E0, 0x9CE2AB57, 0x91A18D8E, 0x95609039, 0x8B27C03C, 0x8FE6DD8B, 0x82A5FB52, 0x8664E6E5,
    0xBE2B5B58, 0xBAEA46EF, 0xB7A96036, 0xB3687D81, 0xAD2F2D84, 0xA9EE3033, 0xA4AD16EA, 0xA06C0B5D,
    0xD4326D90, 0xD0F37027, 0xDDB056FE, 0xD9714B49, 0xC7361B4C, 0xC3F706FB, 0xCEB42022, 0xCA753D95,
    0xF23A8028, 0xF6FB9D9F, 0xFBB8BB46, 0xFF79A6F1, 0xE13EF6F4, 0xE5FFEB43, 0xE8BCCD9A, 0xEC7DD02D,
    0x34867077, 0x30476DC0, 0x3D044B19, 0x39C556AE, 0x278206AB, 0x23431B1C, 0x2E003DC5, 0x2AC12072,
    0x128E9DCF, 0x164F8078, 0x1B0CA6A1, 0x1FCDBB16, 0x018AEB13, 0x054BF6A4, 0x0808D07D, 0x0CC9CDCA,
    0x7897AB07, 0x7C56B6B0, 0x71159069, 0x75D48DDE, 0x6B93DDDB, 0x6F52C06C, 0x6211E6B5, 0x66D0FB02,
    0x5E9F46BF, 0x5A5E5B08, 0x571D7DD1, 0x53DC6066, 0x4D9B3063, 0x495A2DD4, 0x44190B0D, 0x40D816BA,
    0xACA5C697, 0xA864DB20, 0xA527FDF9, 0xA1E6E04E, 0xBFA1B04B, 0xBB60ADFC, 0xB6238B25, 0xB2E29692,
    0x8AAD2B2F, 0x8E6C3698, 0x832F1041, 0x87EE0DF6, 0x99A95DF3, 0x9D684044, 0x902B669D, 0x94EA7B2A,
    0xE0B41DE7, 0xE4750050, 0xE9362689, 0xEDF73B3E, 0xF3B06B3B, 0xF771768C, 0xFA325055, 0xFEF34DE2,
    0xC6BCF05F, 0xC27DEDE8, 0xCF3ECB31, 0xCBFFD686, 0xD5B88683, 0xD1799B34, 0xDC3ABDED, 0xD8FBA05A,
    0x690CE0EE, 0x6DCDFD59, 0x608EDB80, 0x644FC637, 0x7A089632, 0x7EC98B85, 0x738AAD5C, 0x774BCF6E,
    0x4F040D56, 0x4BC510E1, 0x46863638, 0x42472B8F, 0x5C007B8A, 0x58C1663D, 0x558240E4, 0x51435D53,
    0x251D3B9E, 0x21DC2629, 0x2C9F00F0, 0x285E1D47, 0x36194D42, 0x32D850F5, 0x3F9B762C, 0x3B5A6B9B,
    0x0315D626, 0x07D4CB91, 0x0A97ED48, 0x0E56F0FF, 0x1011A0FA, 0x14D0BD4D, 0x19939B94, 0x1D528623,
    0xF12F560E, 0xF5EE4BB9, 0xF8AD6D60, 0xFC6C70D7, 0xE22B20D2, 0xE6EA3D65, 0xEBA91BBC, 0xEF68060B,
    0xD727BBB6, 0xD3E6A601, 0xDEA580D8, 0xDA649D6F, 0xC423CD6A, 0xC0E2D0DD, 0xCDA1F604, 0xC960EBB3,
    0xBD3E8D7E, 0xB9FF90C9, 0xB4BCB610, 0xB07DABA7, 0xAE3AFBA2, 0xAAFBE615, 0xA7B8C0CC, 0xA379DD7B,
    0x9B3660C6, 0x9FF77D71, 0x92B45BA8, 0x9675461F, 0x8832161A, 0x8CF30BAD, 0x81B02D74, 0x857130C3,
    0x5D8A9099, 0x594B8D2E, 0x5408ABF7, 0x50C9B640, 0x4E8EE645, 0x4A4FFBF2, 0x470CDD2B, 0x43CDC09C,
    0x7B827D21, 0x7F436096, 0x7200464F, 0x76C15BF8, 0x68860BFD, 0x6C47164A, 0x61043093, 0x65C52D24,
    0x119B4BE9, 0x155A565E, 0x18197087, 0x1CD86D30, 0x029F3D35, 0x065E2082, 0x0B1D065B, 0x0FDC1BEC,
    0x3793A651, 0x3352BBE6, 0x3E119D3F, 0x3AD08088, 0x2497D08D, 0x2056CD3A, 0x2D15EBE3, 0x29D4F654,
    0xC5A92679, 0xC1683BCE, 0xCC2B1D17, 0xC8EA00A0, 0xD6AD50A5, 0xD26C4D12, 0xDF2F6BCB, 0xDBEE767C,
    0xE3A1CBC1, 0xE760D676, 0xEA23F0AF, 0xEEE2ED18, 0xF0A5BD1D, 0xF464A0AA, 0xF9278673, 0xFDE69BC4,
    0x89B8FD09, 0x8D79E0BE, 0x803AC667, 0x84FBDBD0, 0x9ABC8BD5, 0x9E7D9662, 0x933EB0BB, 0x97FFAD0C,
    0xAFB010B1, 0xAB710D06, 0xA6322BDF, 0xA2F33668, 0xBCB4666D, 0xB8757BDA, 0xB5365D03, 0xB1F740B4,
];

/// Parsed Ogg Page Header (RFC 3533).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OggPageHeader {
    pub version: u8,
    pub header_type: u8,
    pub granule_position: u64,
    pub serial_number: u32,
    pub sequence_number: u32,
    pub crc_checksum: u32,
    pub page_segments: u8,
    pub segment_table: Vec<u8>,
    pub body_len: usize,
}

impl OggPageHeader {
    pub fn is_bos(&self) -> bool {
        (self.header_type & OGG_FLAG_BOS) != 0
    }

    pub fn is_eos(&self) -> bool {
        (self.header_type & OGG_FLAG_EOS) != 0
    }

    pub fn is_continuation(&self) -> bool {
        (self.header_type & OGG_FLAG_CONTINUED_PACKET) != 0
    }

    pub fn header_len(&self) -> usize {
        OGG_BASE_HEADER_LEN + self.segment_table.len()
    }

    pub fn total_page_len(&self) -> usize {
        self.header_len() + self.body_len
    }

    /// Attempts to parse an Ogg page header from a slice.
    pub fn parse(slice: &[u8]) -> Result<Option<Self>, OggParseError> {
        if slice.len() < OGG_BASE_HEADER_LEN {
            return Ok(None);
        }
        if slice[0..4] != OGG_CAPTURE_PATTERN {
            return Err(OggParseError::InvalidCapturePattern);
        }
        let version = slice[4];
        if version != 0 {
            return Err(OggParseError::UnsupportedVersion(version));
        }

        let header_type = slice[5];
        let granule_position = u64::from_le_bytes(slice[6..14].try_into().unwrap());
        let serial_number = u32::from_le_bytes(slice[14..18].try_into().unwrap());
        let sequence_number = u32::from_le_bytes(slice[18..22].try_into().unwrap());
        let crc_checksum = u32::from_le_bytes(slice[22..26].try_into().unwrap());
        let page_segments = slice[26];

        let total_header_len = OGG_BASE_HEADER_LEN + page_segments as usize;
        if slice.len() < total_header_len {
            return Ok(None);
        }

        let segment_table = slice[OGG_BASE_HEADER_LEN..total_header_len].to_vec();
        let body_len: usize = segment_table.iter().map(|&s| s as usize).sum();

        Ok(Some(Self {
            version,
            header_type,
            granule_position,
            serial_number,
            sequence_number,
            crc_checksum,
            page_segments,
            segment_table,
            body_len,
        }))
    }
}

/// Complete Ogg Page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OggPage {
    pub header: OggPageHeader,
    pub raw_bytes: Bytes,
}

impl OggPage {
    /// Returns the body slice of the page.
    pub fn body(&self) -> &[u8] {
        &self.raw_bytes[self.header.header_len()..]
    }

    /// Extracts packets contained within this page.
    /// Returns list of completed packet payloads and a boolean indicating if the final packet is continued.
    pub fn extract_packets(&self) -> (Vec<Bytes>, Option<Bytes>) {
        let mut packets = Vec::new();
        let body = self.body();
        let mut offset = 0;
        let mut current_packet = BytesMut::new();

        for &seg_len in &self.header.segment_table {
            let seg_len = seg_len as usize;
            if offset + seg_len <= body.len() {
                current_packet.extend_from_slice(&body[offset..offset + seg_len]);
                offset += seg_len;
            }
            if seg_len < 255 {
                // Segment length < 255 terminates the packet
                packets.push(current_packet.split().freeze());
            }
        }

        let continued = if !current_packet.is_empty() {
            Some(current_packet.freeze())
        } else {
            None
        };

        (packets, continued)
    }
}

/// Metadata description for an active Ogg stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OggMetadata {
    pub codec: OggCodec,
    pub content_type: String,
    pub mime_type: String,
    pub stream_ext: String,
    pub serial_number: u32,
    pub has_video: bool,
    pub has_audio: bool,
    pub channels: Option<u8>,
    pub sample_rate: Option<u32>,
    pub bitrate_kbps: Option<u32>,
}

/// Output event yielded by `OggDemuxer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OggDemuxOutput {
    /// Initial or updated header snapshot with epoch and parsed stream metadata.
    HeaderSnapshotReady {
        snapshot: HeaderSnapshot,
        metadata: Box<OggMetadata>,
        epoch: u64,
    },
    /// Relayed media page.
    ContentPage {
        page: Bytes,
        epoch: u64,
        position: u64,
        is_keyframe: bool,
    },
    /// End of stream reached cleanly.
    EndOfStream,
}

/// Demuxer state machine (Global Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OggDemuxerState {
    AwaitingBos,
    CollectingHeaders,
    StreamingMedia,
    Ended,
}

/// Streaming Ogg Container Demuxer (REQ-051, REQ-055, REQ-056).
#[derive(Debug)]
pub struct OggDemuxer {
    state: OggDemuxerState,
    buffer: BytesMut,
    max_buffer_capacity: usize,
    max_packet_size: usize,
    current_epoch: u64,
    stream_position: u64,
    active_serial: Option<u32>,
    active_codec: OggCodec,
    header_pages: Vec<Bytes>,
    header_packet_count: usize,
    required_header_packet_count: usize,
    metadata: Option<OggMetadata>,
    header_snapshot: Option<HeaderSnapshot>,
}

impl Default for OggDemuxer {
    fn default() -> Self {
        Self::new()
    }
}

impl OggDemuxer {
    /// Creates a new OggDemuxer with default limits.
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_OGG_BUFFER_CAPACITY, DEFAULT_MAX_OGG_PACKET_SIZE)
    }

    /// Creates an OggDemuxer with explicit memory bounds (Global Rule 3).
    pub fn with_limits(max_buffer_capacity: usize, max_packet_size: usize) -> Self {
        Self {
            state: OggDemuxerState::AwaitingBos,
            buffer: BytesMut::with_capacity(16384),
            max_buffer_capacity,
            max_packet_size,
            current_epoch: 0,
            stream_position: 0,
            active_serial: None,
            active_codec: OggCodec::Unsupported,
            header_pages: Vec::new(),
            header_packet_count: 0,
            required_header_packet_count: 0,
            metadata: None,
            header_snapshot: None,
        }
    }

    /// Feeds incoming stream bytes into the demuxer buffer.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), OggParseError> {
        if self.buffer.len() + chunk.len() > self.max_buffer_capacity {
            return Err(OggParseError::BufferCapacityExceeded {
                actual: self.buffer.len() + chunk.len(),
                max: self.max_buffer_capacity,
            });
        }
        self.buffer.extend_from_slice(chunk);
        Ok(())
    }

    /// Advances the demuxer and yields the next demux output.
    pub fn next_output(&mut self) -> Result<Option<OggDemuxOutput>, OggParseError> {
        loop {
            if self.buffer.len() < OGG_BASE_HEADER_LEN {
                return Ok(None);
            }

            // Sync search for 'OggS'
            if self.buffer[0..4] != OGG_CAPTURE_PATTERN {
                if let Some(pos) = self.buffer.windows(4).position(|w| w == OGG_CAPTURE_PATTERN) {
                    self.buffer.advance(pos);
                } else {
                    let keep = self.buffer.len().min(3);
                    let discard = self.buffer.len() - keep;
                    self.buffer.advance(discard);
                    return Ok(None);
                }
            }

            let header = match OggPageHeader::parse(&self.buffer)? {
                Some(h) => h,
                None => return Ok(None),
            };

            // Chained stream detection: if BOS arrives while state is Ended or StreamingMedia,
            // bump epoch and restart header collection for the new bitstream (RFC 3533, REQ-051).
            if header.is_bos() && (self.state == OggDemuxerState::StreamingMedia || self.state == OggDemuxerState::Ended) {
                self.current_epoch += 1;
                self.state = OggDemuxerState::AwaitingBos;
                self.header_pages.clear();
                self.header_packet_count = 0;
            } else if self.state == OggDemuxerState::Ended {
                return Ok(None);
            }

            let total_page_len = header.total_page_len();
            if self.buffer.len() < total_page_len {
                return Ok(None);
            }

            let page_bytes = self.buffer.split_to(total_page_len).freeze();
            let page = OggPage {
                header: header.clone(),
                raw_bytes: page_bytes.clone(),
            };

            match self.state {
                OggDemuxerState::AwaitingBos => {
                    if !header.is_bos() {
                        // Skip stray non-BOS pages before initial stream
                        continue;
                    }

                    let (packets, _) = page.extract_packets();
                    if packets.is_empty() {
                        continue;
                    }

                    let codec = OggCodec::detect_from_bos_packet(&packets[0]);
                    if codec == OggCodec::Unsupported {
                        return Err(OggParseError::UnsupportedCodec);
                    }

                    self.active_serial = Some(header.serial_number);
                    self.active_codec = codec;
                    self.required_header_packet_count = codec.required_header_packet_count();
                    self.header_pages.clear();
                    self.header_pages.push(page_bytes);
                    self.header_packet_count = packets.len();

                    let is_video = codec == OggCodec::Theora;
                    let (ctype, mime, ext) = match codec {
                        OggCodec::Vorbis => ("OGG".to_string(), "audio/ogg".to_string(), ".ogg".to_string()),
                        OggCodec::Opus => ("OPUS".to_string(), "audio/ogg".to_string(), ".opus".to_string()),
                        OggCodec::Theora => ("OGM".to_string(), "video/ogg".to_string(), ".ogm".to_string()),
                        OggCodec::Unsupported => ("OGG".to_string(), "application/ogg".to_string(), ".ogg".to_string()),
                    };

                    self.metadata = Some(OggMetadata {
                        codec,
                        content_type: ctype,
                        mime_type: mime,
                        stream_ext: ext,
                        serial_number: header.serial_number,
                        has_video: is_video,
                        has_audio: !is_video || codec == OggCodec::Theora,
                        channels: None,
                        sample_rate: None,
                        bitrate_kbps: None,
                    });

                    if self.header_packet_count >= self.required_header_packet_count {
                        return self.emit_header_snapshot();
                    } else {
                        self.state = OggDemuxerState::CollectingHeaders;
                    }
                }
                OggDemuxerState::CollectingHeaders => {
                    if Some(header.serial_number) != self.active_serial {
                        continue;
                    }
                    self.header_pages.push(page_bytes);
                    let (packets, _) = page.extract_packets();
                    self.header_packet_count += packets.len();

                    if self.header_packet_count >= self.required_header_packet_count {
                        return self.emit_header_snapshot();
                    }
                }
                OggDemuxerState::StreamingMedia => {
                    if Some(header.serial_number) != self.active_serial {
                        continue;
                    }

                    // Keyframe detection:
                    // For audio-only (Vorbis, Opus): all packets are independent/key (REQ-055).
                    // For Theora: granulepos indicates keyframe if interframe offset is 0.
                    let is_keyframe = if self.active_codec == OggCodec::Theora {
                        // Theora granulepos: high bits are frame count, low bits are interframe count.
                        // If low bits == 0, it is a keyframe!
                        (header.granule_position & 0x3F) == 0
                    } else {
                        true // Audio-only admits immediately
                    };

                    let pos = self.stream_position;
                    self.stream_position += page_bytes.len() as u64;

                    if header.is_eos() {
                        self.state = OggDemuxerState::Ended;
                    }

                    return Ok(Some(OggDemuxOutput::ContentPage {
                        page: page_bytes,
                        epoch: self.current_epoch,
                        position: pos,
                        is_keyframe,
                    }));
                }
                OggDemuxerState::Ended => {
                    return Ok(None);
                }
            }
        }
    }

    /// Helper to assemble and emit HeaderSnapshot from collected header pages.
    fn emit_header_snapshot(&mut self) -> Result<Option<OggDemuxOutput>, OggParseError> {
        let mut builder = BytesMut::new();
        for p in &self.header_pages {
            builder.put_slice(p);
        }
        let snapshot_data = builder.freeze();
        if snapshot_data.len() > self.max_packet_size {
            return Err(OggParseError::PacketSizeExceeded {
                actual: snapshot_data.len(),
                max: self.max_packet_size,
            });
        }
        let meta = self.metadata.clone().unwrap();

        let snapshot = HeaderSnapshot::new(
            self.current_epoch as i32,
            self.stream_position as i64,
            meta.content_type.clone(),
            snapshot_data.clone(),
        );

        self.header_snapshot = Some(snapshot.clone());
        self.state = OggDemuxerState::StreamingMedia;

        let out = OggDemuxOutput::HeaderSnapshotReady {
            snapshot,
            metadata: Box::new(meta),
            epoch: self.current_epoch,
        };
        self.stream_position += snapshot_data.len() as u64;
        Ok(Some(out))
    }

    /// Explicitly signals EOF on input stream.
    pub fn signal_eof(&mut self) -> Result<Option<OggDemuxOutput>, OggParseError> {
        if self.state == OggDemuxerState::Ended {
            return Ok(None);
        }
        self.state = OggDemuxerState::Ended;
        Ok(Some(OggDemuxOutput::EndOfStream))
    }

    /// Returns active metadata, if parsed.
    pub fn metadata(&self) -> Option<&OggMetadata> {
        self.metadata.as_ref()
    }

    /// Returns active header snapshot, if established.
    pub fn header_snapshot(&self) -> Option<&HeaderSnapshot> {
        self.header_snapshot.as_ref()
    }

    /// Returns current stream epoch.
    pub fn current_epoch(&self) -> u64 {
        self.current_epoch
    }

    /// Returns current stream position in bytes.
    pub fn stream_position(&self) -> u64 {
        self.stream_position
    }

    /// Returns current demuxer state.
    pub fn state(&self) -> OggDemuxerState {
        self.state
    }
}
