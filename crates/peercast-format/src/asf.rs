//! ASF (Advanced Systems Format) and MMS/WMSP framing parser (REQ-054, REQ-056).
//!
//! Enforces:
//! - REQ-054: ASF parsing follows GUID + u64 object size and child boundaries, with a
//!   24-byte generic prefix and six additional top-level header bytes before children.
//!   Strictly forbids guessing an unverified "50-byte minimum."
//!   Distinguishes raw ASF objects from MMS/WMSP-framed transport. The audited Station
//!   reader forwards complete MMS header frames; raw ASF-only is not MMS-compatible.
//! - REQ-051: Header snapshot retention, content epoch bumps on header replacement, and
//!   invalidation of stale stream state.
//! - REQ-056: Full support for ASF, WMV, and WMA containers and codecs with valid,
//!   truncated, oversized, and change-of-header verification.
//! - Global Rule 2: Explicit state machine with entry re-entrancy guards.
//! - Global Rule 3: Bounded memory buffers and zero unbounded allocations.

use bytes::{Bytes, BytesMut};
use peercast_core::cache::HeaderSnapshot;
use std::fmt;
use thiserror::Error;

/// Default maximum allowed ASF object size (16 MiB) to prevent unbounded memory allocation (Global Rule 3).
pub const DEFAULT_MAX_ASF_OBJECT_SIZE: u64 = 16 * 1024 * 1024;

/// Default maximum parser buffer capacity (16 MiB).
pub const DEFAULT_MAX_ASF_BUFFER_CAPACITY: usize = 16 * 1024 * 1024;

/// Generic ASF Object Header length: 16 bytes GUID + 8 bytes u64 size = 24 bytes (REQ-054).
pub const ASF_OBJECT_HEADER_LEN: usize = 24;

/// Additional bytes in Top-Level Header before child objects: 4 bytes count + 2 reserved bytes = 6 bytes (REQ-054).
pub const ASF_TOP_LEVEL_HEADER_PRELUDE_LEN: usize = 6;

/// Total minimum length of a valid Top-Level Header Object (24 + 6 = 30 bytes).
pub const ASF_TOP_LEVEL_HEADER_MIN_LEN: usize =
    ASF_OBJECT_HEADER_LEN + ASF_TOP_LEVEL_HEADER_PRELUDE_LEN;

/// 16-byte Microsoft GUID representation on wire (standard COM little-endian layout for groups 1..3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AsfGuid(pub [u8; 16]);

impl AsfGuid {
    /// Creates an AsfGuid from 16 raw wire bytes.
    pub const fn from_raw(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Returns the raw 16 wire bytes.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Formats GUID as standard canonical string: `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`.
    pub fn to_string_guid(&self) -> String {
        let b = &self.0;
        let d1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let d2 = u16::from_le_bytes([b[4], b[5]]);
        let d3 = u16::from_le_bytes([b[6], b[7]]);
        format!(
            "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
            d1, d2, d3, b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
        )
    }

    /// Parses a standard COM GUID string into wire byte order.
    pub fn parse_str(s: &str) -> Result<Self, AsfParseError> {
        let cleaned = s.replace(['{', '}', '-'], "");
        if cleaned.len() != 32 {
            return Err(AsfParseError::MalformedGuid(s.to_string()));
        }
        let mut raw = [0u8; 16];
        for i in 0..16 {
            raw[i] = u8::from_str_radix(&cleaned[i * 2..i * 2 + 2], 16)
                .map_err(|_| AsfParseError::MalformedGuid(s.to_string()))?;
        }
        // Canonical string XXXXXXXX-XXXX-XXXX-... has big-endian hex groups.
        // Convert groups 1, 2, 3 to little-endian wire order:
        raw[0..4].reverse();
        raw[4..6].reverse();
        raw[6..8].reverse();
        Ok(Self(raw))
    }
}

impl fmt::Debug for AsfGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AsfGuid({})", self.to_string_guid())
    }
}

impl fmt::Display for AsfGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_guid())
    }
}

// Standard ASF GUID Constants (Wire little-endian order)
// 1. Top-Level Header Object: 75B22630-668E-11CF-A6D9-00AA0062CE6C
pub const ASF_TOP_LEVEL_HEADER_GUID: AsfGuid = AsfGuid([
    0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
]);

// 2. Data Object: 75B22636-668E-11CF-A6D9-00AA0062CE6C
pub const ASF_DATA_OBJECT_GUID: AsfGuid = AsfGuid([
    0x36, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
]);

// 3. File Properties Object: 8CABDCA1-A947-11CF-8EE4-00C00C205365
pub const ASF_FILE_PROPERTIES_GUID: AsfGuid = AsfGuid([
    0xA1, 0xDC, 0xAB, 0x8C, 0x47, 0xA9, 0xCF, 0x11, 0x8E, 0xE4, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65,
]);

// 4. Stream Properties Object: B7DC0791-A9B7-11CF-8EE6-00C00C205365
pub const ASF_STREAM_PROPERTIES_GUID: AsfGuid = AsfGuid([
    0x91, 0x07, 0xDC, 0xB7, 0xB7, 0xA9, 0xCF, 0x11, 0x8E, 0xE6, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65,
]);

// 5. Stream Bitrate Properties Object: 7BF875CE-468D-11D1-8D82-006097C9A2B2
pub const ASF_STREAM_BITRATE_GUID: AsfGuid = AsfGuid([
    0xCE, 0x75, 0xF8, 0x7B, 0x8D, 0x46, 0xD1, 0x11, 0x8D, 0x82, 0x00, 0x60, 0x97, 0xC9, 0xA2, 0xB2,
]);

// 6. Content Description Object: 75B22633-668E-11CF-A6D9-00AA0062CE6C
pub const ASF_CONTENT_DESCRIPTION_GUID: AsfGuid = AsfGuid([
    0x33, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
]);

// 7. Audio Stream Type GUID: F8699E40-5B4D-11CF-A8FD-00805F5C442B
pub const ASF_STREAM_TYPE_AUDIO_GUID: AsfGuid = AsfGuid([
    0x40, 0x9E, 0x69, 0xF8, 0x4D, 0x5B, 0xCF, 0x11, 0xA8, 0xFD, 0x00, 0x80, 0x5F, 0x5C, 0x44, 0x2B,
]);

// 8. Video Stream Type GUID: BC19EFC0-5B4D-11CF-A8FD-00805F5C442B
pub const ASF_STREAM_TYPE_VIDEO_GUID: AsfGuid = AsfGuid([
    0xC0, 0xEF, 0x19, 0xBC, 0x4D, 0x5B, 0xCF, 0x11, 0xA8, 0xFD, 0x00, 0x80, 0x5F, 0x5C, 0x44, 0x2B,
]);

/// Errors encountered during ASF / MMS stream demuxing and parsing.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum AsfParseError {
    #[error("Invalid ASF object size {0}: must be at least 24 bytes (REQ-054)")]
    InvalidObjectSize(u64),

    #[error("ASF object size {actual} exceeded maximum allowed limit {max} (Global Rule 3)")]
    ObjectSizeExceeded { actual: u64, max: u64 },

    #[error("Object size {0} exceeds addressable memory on this architecture")]
    ObjectSizeOverflow(u64),

    #[error("Invalid Top-Level Header prelude: size {0} is less than minimum 30 bytes")]
    InvalidTopLevelHeaderSize(u64),

    #[error("Truncated ASF object: needed {needed} bytes, but only {available} available")]
    TruncatedObject { needed: usize, available: usize },

    #[error("Truncated WMSP frame: needed {needed} bytes, but only {available} available")]
    TruncatedFrame { needed: usize, available: usize },

    #[error("Invalid WMSP frame header (must start with '$' or flagged '$')")]
    InvalidWmspFrameHeader,

    #[error("Malformed GUID string: {0}")]
    MalformedGuid(String),

    #[error("Buffer capacity {actual} exceeded configured maximum {max} (Global Rule 3)")]
    BufferCapacityExceeded { actual: usize, max: usize },

    #[error("Unexpected EOF during stream parsing")]
    UnexpectedEof,
}

/// Generic ASF Object Header (24 bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsfObjectHeader {
    pub guid: AsfGuid,
    pub size: u64,
}

impl AsfObjectHeader {
    /// Parses an object header from a slice.
    pub fn parse(slice: &[u8], max_size: u64) -> Result<Self, AsfParseError> {
        if slice.len() < ASF_OBJECT_HEADER_LEN {
            return Err(AsfParseError::TruncatedObject {
                needed: ASF_OBJECT_HEADER_LEN,
                available: slice.len(),
            });
        }
        let mut guid_bytes = [0u8; 16];
        guid_bytes.copy_from_slice(&slice[0..16]);
        let size = u64::from_le_bytes([
            slice[16], slice[17], slice[18], slice[19], slice[20], slice[21], slice[22], slice[23],
        ]);

        if size < ASF_OBJECT_HEADER_LEN as u64 {
            return Err(AsfParseError::InvalidObjectSize(size));
        }
        if size > max_size {
            return Err(AsfParseError::ObjectSizeExceeded {
                actual: size,
                max: max_size,
            });
        }
        if size > usize::MAX as u64 {
            return Err(AsfParseError::ObjectSizeOverflow(size));
        }

        Ok(Self {
            guid: AsfGuid::from_raw(guid_bytes),
            size,
        })
    }
}

/// Parsed ASF File Properties Object (GUID 8CABDCA1-A947-11CF-8EE4-00C00C205365).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsfFileProperties {
    pub file_id: [u8; 16],
    pub file_size: u64,
    pub creation_date_ticks: u64,
    pub data_packets_count: u64,
    pub play_duration_ticks: u64,
    pub send_duration_ticks: u64,
    pub preroll_ms: u64,
    pub flags: u32,
    pub min_packet_size: u32,
    pub max_packet_size: u32,
    pub max_bitrate_bps: u32,
}

impl AsfFileProperties {
    pub fn parse(payload: &[u8]) -> Option<Self> {
        if payload.len() < 64 {
            return None;
        }
        let mut file_id = [0u8; 16];
        file_id.copy_from_slice(&payload[0..16]);
        let file_size = u64::from_le_bytes(payload[16..24].try_into().ok()?);
        let creation_date_ticks = u64::from_le_bytes(payload[24..32].try_into().ok()?);
        let data_packets_count = u64::from_le_bytes(payload[32..40].try_into().ok()?);
        let play_duration_ticks = u64::from_le_bytes(payload[40..48].try_into().ok()?);
        let send_duration_ticks = u64::from_le_bytes(payload[48..56].try_into().ok()?);
        let preroll_ms = u64::from_le_bytes(payload[56..64].try_into().ok()?);

        if payload.len() < 80 {
            return None;
        }
        let flags = u32::from_le_bytes(payload[64..68].try_into().ok()?);
        let min_packet_size = u32::from_le_bytes(payload[68..72].try_into().ok()?);
        let max_packet_size = u32::from_le_bytes(payload[72..76].try_into().ok()?);
        let max_bitrate_bps = u32::from_le_bytes(payload[76..80].try_into().ok()?);

        Some(Self {
            file_id,
            file_size,
            creation_date_ticks,
            data_packets_count,
            play_duration_ticks,
            send_duration_ticks,
            preroll_ms,
            flags,
            min_packet_size,
            max_packet_size,
            max_bitrate_bps,
        })
    }
}

/// Bitrate entry in ASF Stream Bitrate Properties Object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsfBitrateRecord {
    pub stream_number: u16,
    pub average_bitrate_bps: u32,
}

/// Parsed ASF Stream Properties Object (GUID B7DC0791-A9B7-11CF-8EE6-00C00C205365).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsfStreamProperties {
    pub stream_type: AsfGuid,
    pub error_correction_type: AsfGuid,
    pub time_offset_ticks: u64,
    pub type_specific_data_len: u32,
    pub error_correction_data_len: u32,
    pub stream_number: u16,
}

impl AsfStreamProperties {
    pub fn parse(payload: &[u8]) -> Option<Self> {
        if payload.len() < 50 {
            return None;
        }
        let mut stream_type = [0u8; 16];
        stream_type.copy_from_slice(&payload[0..16]);
        let mut error_corr = [0u8; 16];
        error_corr.copy_from_slice(&payload[16..32]);
        let time_offset = u64::from_le_bytes(payload[32..40].try_into().ok()?);
        let type_specific_len = u32::from_le_bytes(payload[40..44].try_into().ok()?);
        let error_corr_len = u32::from_le_bytes(payload[44..48].try_into().ok()?);
        let flags = u16::from_le_bytes(payload[48..50].try_into().ok()?);
        let stream_number = flags & 0x7F;

        Some(Self {
            stream_type: AsfGuid::from_raw(stream_type),
            error_correction_type: AsfGuid::from_raw(error_corr),
            time_offset_ticks: time_offset,
            type_specific_data_len: type_specific_len,
            error_correction_data_len: error_corr_len,
            stream_number,
        })
    }
}

/// Parsed ASF Content Description Object (GUID 75B22633-668E-11CF-A6D9-00AA0062CE6C).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AsfContentDescription {
    pub title: String,
    pub author: String,
    pub copyright: String,
    pub description: String,
    pub rating: String,
}

impl AsfContentDescription {
    pub fn parse(payload: &[u8]) -> Option<Self> {
        if payload.len() < 10 {
            return None;
        }
        let title_len = u16::from_le_bytes(payload[0..2].try_into().ok()?) as usize;
        let author_len = u16::from_le_bytes(payload[2..4].try_into().ok()?) as usize;
        let copy_len = u16::from_le_bytes(payload[4..6].try_into().ok()?) as usize;
        let desc_len = u16::from_le_bytes(payload[6..8].try_into().ok()?) as usize;
        let rating_len = u16::from_le_bytes(payload[8..10].try_into().ok()?) as usize;

        let total_needed = 10 + title_len + author_len + copy_len + desc_len + rating_len;
        if payload.len() < total_needed {
            return None;
        }

        let mut offset = 10;
        let decode_utf16le = |slice: &[u8]| -> String {
            let u16s: Vec<u16> = slice
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[b0, b1]| u16::from_le_bytes([b0, b1]))
                .take_while(|&c| c != 0) // strip trailing NUL
                .collect();
            String::from_utf16_lossy(&u16s)
        };

        let title = decode_utf16le(&payload[offset..offset + title_len]);
        offset += title_len;
        let author = decode_utf16le(&payload[offset..offset + author_len]);
        offset += author_len;
        let copyright = decode_utf16le(&payload[offset..offset + copy_len]);
        offset += copy_len;
        let description = decode_utf16le(&payload[offset..offset + desc_len]);
        offset += desc_len;
        let rating = decode_utf16le(&payload[offset..offset + rating_len]);

        Some(Self {
            title,
            author,
            copyright,
            description,
            rating,
        })
    }
}

/// Extracted stream metadata and codec description (Parity with upstream Station `ASFContentReader`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsfMetadata {
    /// Format identifier: "WMV", "WMA", or "ASF".
    pub content_type: String,
    /// MIME type: "video/x-ms-wmv", "audio/x-ms-wma", or "video/x-ms-asf".
    pub mime_type: String,
    /// Stream extension: ".wmv", ".wma", or ".asf".
    pub stream_ext: String,
    /// Total bitrate in kbps (derived from StreamBitrateProperties or FileProperties).
    pub bitrate_kbps: u32,
    pub has_video: bool,
    pub has_audio: bool,
    pub file_properties: Option<AsfFileProperties>,
    pub stream_properties: Vec<AsfStreamProperties>,
    pub bitrate_records: Vec<AsfBitrateRecord>,
    pub content_description: Option<AsfContentDescription>,
}

/// WMSP Frame Type character identifier (Windows Media HTTP Streaming Protocol / MMS over HTTP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WmspFrameType {
    /// "$H": Header frame containing ASF Top-Level Header
    Header,
    /// "$D": Data frame containing ASF data packet
    Data,
    /// "$C": Command frame
    Command,
    /// "$E": End of Stream frame
    End,
    /// "$M": Media packet frame
    Media,
    /// "$P": Packet frame
    Packet,
    /// Other frame type
    Other(u8),
}

impl WmspFrameType {
    pub fn from_subtype_byte(b: u8) -> Self {
        match b {
            b'H' => Self::Header,
            b'D' => Self::Data,
            b'C' => Self::Command,
            b'E' => Self::End,
            b'M' => Self::Media,
            b'P' => Self::Packet,
            other => Self::Other(other),
        }
    }
}

/// Windows Media HTTP Streaming Protocol (WMSP / MMS) Frame.
/// Wire layout:
/// - byte 0: Type char 1 (bit 7 = B flag, bits 0..6 = '$' (0x24))
/// - byte 1: Type char 2 ('H', 'D', 'C', 'E', 'M', 'P')
/// - bytes 2..4: Length (u16 little-endian)
///
/// Total frame length = 4 + Length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WmspFrame {
    pub frame_type: WmspFrameType,
    pub b_flag: bool,
    pub length: u16,
    pub raw_bytes: Bytes,
}

impl WmspFrame {
    /// Attempts to parse a single WMSP frame from the prefix of a slice.
    pub fn parse(slice: &[u8]) -> Result<Option<(Self, usize)>, AsfParseError> {
        if slice.len() < 4 {
            return Ok(None);
        }
        let b0 = slice[0];
        if (b0 & 0x7F) != b'$' {
            return Err(AsfParseError::InvalidWmspFrameHeader);
        }
        let b_flag = (b0 & 0x80) != 0;
        let frame_type = WmspFrameType::from_subtype_byte(slice[1]);
        let length = u16::from_le_bytes([slice[2], slice[3]]);
        let total_len = 4 + length as usize;

        if slice.len() < total_len {
            return Ok(None);
        }

        let raw_bytes = Bytes::copy_from_slice(&slice[..total_len]);
        Ok(Some((
            Self {
                frame_type,
                b_flag,
                length,
                raw_bytes,
            },
            total_len,
        )))
    }

    /// Returns the inner MMS payload slice for '$H', '$D', or '$M' frames.
    ///
    /// Layout:
    /// - bytes 4..8: LocationId (u32 LE)
    /// - byte 8: Incarnation (u8)
    /// - byte 9: AFFlags (u8)
    /// - bytes 10..12: Packet length (u16 LE)
    /// - bytes 12..: Payload bytes (length - 8)
    pub fn mms_payload(&self) -> Option<&[u8]> {
        match self.frame_type {
            WmspFrameType::Header | WmspFrameType::Data | WmspFrameType::Media => {
                if self.raw_bytes.len() < 12 {
                    return None;
                }
                let packet_len =
                    u16::from_le_bytes([self.raw_bytes[10], self.raw_bytes[11]]) as usize;
                let payload_len = packet_len.saturating_sub(8);
                let end = (12 + payload_len).min(self.raw_bytes.len());
                Some(&self.raw_bytes[12..end])
            }
            _ => None,
        }
    }
}

/// Evaluates Top-Level ASF Header Object bytes and extracts complete stream metadata.
///
/// Follows REQ-054:
/// - 24-byte generic object header (GUID + u64 size).
/// - 6 additional bytes (4-byte child count + 2 reserved bytes) before children.
/// - Minimum top-level object length = 30 bytes (REQ-054: do NOT use guessed 50-byte minimum!).
pub fn parse_asf_top_level_header(
    bytes: &[u8],
    max_object_size: u64,
) -> Result<AsfMetadata, AsfParseError> {
    if bytes.len() < ASF_OBJECT_HEADER_LEN {
        return Err(AsfParseError::TruncatedObject {
            needed: ASF_OBJECT_HEADER_LEN,
            available: bytes.len(),
        });
    }

    let header = AsfObjectHeader::parse(bytes, max_object_size)?;
    if header.guid != ASF_TOP_LEVEL_HEADER_GUID {
        return Err(AsfParseError::MalformedGuid(format!(
            "Expected Top-Level Header GUID {}, got {}",
            ASF_TOP_LEVEL_HEADER_GUID, header.guid
        )));
    }

    if header.size < ASF_TOP_LEVEL_HEADER_MIN_LEN as u64 {
        return Err(AsfParseError::InvalidTopLevelHeaderSize(header.size));
    }

    let header_size = header.size as usize;
    if bytes.len() < header_size {
        return Err(AsfParseError::TruncatedObject {
            needed: header_size,
            available: bytes.len(),
        });
    }

    // Read 6-byte prelude: 4-byte child count + 2 reserved bytes
    let child_count = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]) as usize;
    // bytes[28..30] are reserved (0x01, 0x02)

    let mut cursor = ASF_TOP_LEVEL_HEADER_MIN_LEN;
    let mut file_props: Option<AsfFileProperties> = None;
    let mut stream_props: Vec<AsfStreamProperties> = Vec::new();
    let mut bitrate_records: Vec<AsfBitrateRecord> = Vec::new();
    let mut content_desc: Option<AsfContentDescription> = None;

    for _ in 0..child_count {
        if cursor + ASF_OBJECT_HEADER_LEN > header_size {
            break;
        }
        let child_header = AsfObjectHeader::parse(&bytes[cursor..header_size], max_object_size)?;
        let child_size = child_header.size as usize;
        let child_end = cursor + child_size;
        if child_end > header_size {
            return Err(AsfParseError::TruncatedObject {
                needed: child_end,
                available: header_size,
            });
        }

        let payload = &bytes[cursor + ASF_OBJECT_HEADER_LEN..child_end];

        if child_header.guid == ASF_FILE_PROPERTIES_GUID {
            if let Some(fp) = AsfFileProperties::parse(payload) {
                file_props = Some(fp);
            }
        } else if child_header.guid == ASF_STREAM_PROPERTIES_GUID {
            if let Some(sp) = AsfStreamProperties::parse(payload) {
                stream_props.push(sp);
            }
        } else if child_header.guid == ASF_STREAM_BITRATE_GUID {
            if payload.len() >= 2 {
                let count = u16::from_le_bytes([payload[0], payload[1]]) as usize;
                for i in 0..count {
                    let offset = 2 + 6 * i;
                    if offset + 6 <= payload.len() {
                        let stream_num =
                            u16::from_le_bytes([payload[offset], payload[offset + 1]]) & 0x7F;
                        let avg_br = u32::from_le_bytes([
                            payload[offset + 2],
                            payload[offset + 3],
                            payload[offset + 4],
                            payload[offset + 5],
                        ]);
                        bitrate_records.push(AsfBitrateRecord {
                            stream_number: stream_num,
                            average_bitrate_bps: avg_br,
                        });
                    }
                }
            }
        } else if child_header.guid == ASF_CONTENT_DESCRIPTION_GUID {
            if let Some(cd) = AsfContentDescription::parse(payload) {
                content_desc = Some(cd);
            }
        }

        cursor = child_end;
    }

    // Parity with upstream Station `ASFContentReader.ParseChannelInfo`:
    // If any video stream is present -> "WMV"
    // Else if any audio stream is present -> "WMA"
    // Else -> "ASF"
    let has_video = stream_props
        .iter()
        .any(|s| s.stream_type == ASF_STREAM_TYPE_VIDEO_GUID);
    let has_audio = stream_props
        .iter()
        .any(|s| s.stream_type == ASF_STREAM_TYPE_AUDIO_GUID);

    let (content_type, mime_type, stream_ext) = if has_video {
        (
            "WMV".to_string(),
            "video/x-ms-wmv".to_string(),
            ".wmv".to_string(),
        )
    } else if has_audio {
        (
            "WMA".to_string(),
            "audio/x-ms-wma".to_string(),
            ".wma".to_string(),
        )
    } else {
        (
            "ASF".to_string(),
            "video/x-ms-asf".to_string(),
            ".asf".to_string(),
        )
    };

    // Upstream bitrate calculation:
    // If StreamBitrateProperties present: sum of average bitrates / 1000
    // Else if FileProperties present: max bitrate / 1000
    // Else: 0
    let bitrate_kbps = if !bitrate_records.is_empty() {
        let total: u32 = bitrate_records.iter().map(|r| r.average_bitrate_bps).sum();
        total / 1000
    } else if let Some(ref fp) = file_props {
        fp.max_bitrate_bps / 1000
    } else {
        0
    };

    Ok(AsfMetadata {
        content_type,
        mime_type,
        stream_ext,
        bitrate_kbps,
        has_video,
        has_audio,
        file_properties: file_props,
        stream_properties: stream_props,
        bitrate_records,
        content_description: content_desc,
    })
}

/// ASF Transport Mode (REQ-054, REQ-060).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsfTransportProfile {
    /// Framed Windows Media HTTP Streaming Protocol (WMSP / MMS over HTTP).
    /// Used for application/x-mms-framed distribution. Complete MMS frames are preserved and relayed.
    MmsFramed,
    /// Raw un-framed ASF object stream.
    RawAsf,
}

/// Output event yielded by `AsfDemuxer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsfDemuxOutput {
    /// Initial or updated header snapshot with epoch and parsed stream metadata.
    HeaderSnapshotReady {
        snapshot: HeaderSnapshot,
        metadata: Box<AsfMetadata>,
        epoch: u64,
    },
    /// Relayed media packet (for MMS: complete WMSP frame; for raw ASF: raw packet).
    ContentPacket {
        packet: Bytes,
        epoch: u64,
        position: u64,
    },
    /// End of stream signaled by '$E' frame or clean EOF.
    EndOfStream,
}

/// Demuxer state machine (Global Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsfDemuxerState {
    AwaitingHeader,
    Streaming,
    Ended,
}

/// Stream demuxer for ASF and MMS-framed transport (REQ-054, REQ-056, Global Rule 3).
#[derive(Debug)]
pub struct AsfDemuxer {
    profile: AsfTransportProfile,
    state: AsfDemuxerState,
    buffer: BytesMut,
    max_buffer_capacity: usize,
    max_object_size: u64,
    current_epoch: u64,
    stream_position: u64,
    metadata: Option<AsfMetadata>,
    header_snapshot: Option<HeaderSnapshot>,
}

impl AsfDemuxer {
    /// Creates a new demuxer with the specified transport profile and buffer limits.
    pub fn new(profile: AsfTransportProfile) -> Self {
        Self::with_limits(
            profile,
            DEFAULT_MAX_ASF_BUFFER_CAPACITY,
            DEFAULT_MAX_ASF_OBJECT_SIZE,
        )
    }

    /// Creates a demuxer with explicit memory bounds (Global Rule 3).
    pub fn with_limits(
        profile: AsfTransportProfile,
        max_buffer_capacity: usize,
        max_object_size: u64,
    ) -> Self {
        Self {
            profile,
            state: AsfDemuxerState::AwaitingHeader,
            buffer: BytesMut::with_capacity(8192),
            max_buffer_capacity,
            max_object_size,
            current_epoch: 0,
            stream_position: 0,
            metadata: None,
            header_snapshot: None,
        }
    }

    /// Feeds incoming stream bytes into the demuxer buffer.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), AsfParseError> {
        if self.buffer.len() + chunk.len() > self.max_buffer_capacity {
            return Err(AsfParseError::BufferCapacityExceeded {
                actual: self.buffer.len() + chunk.len(),
                max: self.max_buffer_capacity,
            });
        }
        self.buffer.extend_from_slice(chunk);
        Ok(())
    }

    /// Advances the parser and yields the next demux output.
    pub fn next_output(&mut self) -> Result<Option<AsfDemuxOutput>, AsfParseError> {
        // Re-entrancy guard: if already ended, return None immediately
        if self.state == AsfDemuxerState::Ended {
            return Ok(None);
        }

        match self.profile {
            AsfTransportProfile::MmsFramed => self.next_mms_output(),
            AsfTransportProfile::RawAsf => self.next_raw_asf_output(),
        }
    }

    /// Processes MMS/WMSP-framed input.
    fn next_mms_output(&mut self) -> Result<Option<AsfDemuxOutput>, AsfParseError> {
        loop {
            if self.buffer.len() < 4 {
                return Ok(None);
            }

            let b0 = self.buffer[0];
            if (b0 & 0x7F) != b'$' {
                return Err(AsfParseError::InvalidWmspFrameHeader);
            }
            let length = u16::from_le_bytes([self.buffer[2], self.buffer[3]]) as usize;
            let total_len = 4 + length;

            if self.buffer.len() < total_len {
                return Ok(None);
            }

            // Extract the complete WMSP frame
            let frame_bytes = self.buffer.split_to(total_len).freeze();
            let frame_type = WmspFrameType::from_subtype_byte(frame_bytes[1]);

            match frame_type {
                WmspFrameType::Header => {
                    // Extract MMS payload (offset 12 onwards)
                    if frame_bytes.len() < 12 {
                        return Err(AsfParseError::TruncatedFrame {
                            needed: 12,
                            available: frame_bytes.len(),
                        });
                    }
                    let packet_len =
                        u16::from_le_bytes([frame_bytes[10], frame_bytes[11]]) as usize;
                    let payload_len = packet_len.saturating_sub(8);
                    let end = (12 + payload_len).min(frame_bytes.len());
                    let asf_header_payload = &frame_bytes[12..end];

                    let meta =
                        parse_asf_top_level_header(asf_header_payload, self.max_object_size)?;

                    if self.metadata.is_some() {
                        // Header replacement mid-stream: increment epoch (REQ-051)
                        self.current_epoch += 1;
                    }

                    // Parity contract: Station reader forwards complete MMS header frame (REQ-054)
                    let snapshot = HeaderSnapshot::new(
                        self.current_epoch as i32,
                        self.stream_position as i64,
                        meta.content_type.clone(),
                        frame_bytes.clone(),
                    );

                    self.metadata = Some(meta.clone());
                    self.header_snapshot = Some(snapshot.clone());
                    self.state = AsfDemuxerState::Streaming;

                    let out = AsfDemuxOutput::HeaderSnapshotReady {
                        snapshot,
                        metadata: Box::new(meta),
                        epoch: self.current_epoch,
                    };
                    self.stream_position += frame_bytes.len() as u64;
                    return Ok(Some(out));
                }
                WmspFrameType::Data
                | WmspFrameType::Media
                | WmspFrameType::Command
                | WmspFrameType::Packet => {
                    if self.state == AsfDemuxerState::Streaming {
                        let pos = self.stream_position;
                        self.stream_position += frame_bytes.len() as u64;
                        return Ok(Some(AsfDemuxOutput::ContentPacket {
                            packet: frame_bytes,
                            epoch: self.current_epoch,
                            position: pos,
                        }));
                    }
                    // Ignore data arriving before initial header
                }
                WmspFrameType::End => {
                    self.state = AsfDemuxerState::Ended;
                    return Ok(Some(AsfDemuxOutput::EndOfStream));
                }
                WmspFrameType::Other(_) => {
                    // Forward unknown frame within budget without corrupting stream
                    if self.state == AsfDemuxerState::Streaming {
                        let pos = self.stream_position;
                        self.stream_position += frame_bytes.len() as u64;
                        return Ok(Some(AsfDemuxOutput::ContentPacket {
                            packet: frame_bytes,
                            epoch: self.current_epoch,
                            position: pos,
                        }));
                    }
                }
            }
        }
    }

    /// Processes raw un-framed ASF input.
    fn next_raw_asf_output(&mut self) -> Result<Option<AsfDemuxOutput>, AsfParseError> {
        loop {
            if self.buffer.len() < ASF_OBJECT_HEADER_LEN {
                return Ok(None);
            }

            let header = AsfObjectHeader::parse(&self.buffer, self.max_object_size)?;
            let total_size = header.size as usize;

            if self.buffer.len() < total_size {
                return Ok(None);
            }

            let obj_bytes = self.buffer.split_to(total_size).freeze();

            if header.guid == ASF_TOP_LEVEL_HEADER_GUID {
                let meta = parse_asf_top_level_header(&obj_bytes, self.max_object_size)?;
                if self.metadata.is_some() {
                    self.current_epoch += 1;
                }

                let snapshot = HeaderSnapshot::new(
                    self.current_epoch as i32,
                    self.stream_position as i64,
                    meta.content_type.clone(),
                    obj_bytes.clone(),
                );

                self.metadata = Some(meta.clone());
                self.header_snapshot = Some(snapshot.clone());
                self.state = AsfDemuxerState::Streaming;

                let out = AsfDemuxOutput::HeaderSnapshotReady {
                    snapshot,
                    metadata: Box::new(meta),
                    epoch: self.current_epoch,
                };
                self.stream_position += obj_bytes.len() as u64;
                return Ok(Some(out));
            } else if header.guid == ASF_DATA_OBJECT_GUID
                || self.state == AsfDemuxerState::Streaming
            {
                let pos = self.stream_position;
                self.stream_position += obj_bytes.len() as u64;
                return Ok(Some(AsfDemuxOutput::ContentPacket {
                    packet: obj_bytes,
                    epoch: self.current_epoch,
                    position: pos,
                }));
            }
        }
    }

    /// Returns the currently active metadata, if parsed.
    pub fn metadata(&self) -> Option<&AsfMetadata> {
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
    pub fn state(&self) -> AsfDemuxerState {
        self.state
    }
}
