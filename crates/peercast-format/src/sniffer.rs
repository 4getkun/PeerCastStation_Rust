use bytes::{Bytes, BytesMut};
use thiserror::Error;

/// Supported media container formats for PeerCast distribution (REQ-050, REQ-056).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerType {
    FLV,
    TS,
    MKV,
    WebM,
    ASF,
    Ogg,
    MP3,
    Raw,
    Unsupported,
}

/// Discovered metadata and MIME description for an identified container stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerInfo {
    pub container_type: ContainerType,
    pub content_type_str: String,
    pub mime_type: String,
    pub has_audio: bool,
    pub has_video: bool,
}

/// The result of inspecting a stream prefix for container framing (REQ-050, REQ-056).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SniffResult {
    /// Definitive identification of container type and streams.
    Definitive(ContainerInfo),
    /// Prefix has a valid candidate signature, but more bytes are required to verify.
    NeedsMoreData { needed_bytes: usize },
    /// Input is conclusively not any supported container.
    Unsupported,
}

/// Errors produced during sniffing or prefix buffering.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum SniffError {
    #[error("Sniff buffer exceeded maximum configured bound ({actual} > {max} bytes) (Global Rule 3)")]
    BufferOverflow { actual: usize, max: usize },
}

/// Standard ASF File Properties / Header Object GUID
/// 75B22630-668E-11CF-A6D9-00AA0062CE6C in little-endian byte order:
const ASF_HEADER_GUID: [u8; 16] = [
    0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
];

/// EBML (Matroska / WebM) 4-byte header ID: 0x1A 0x45 0xDF 0xA3
const EBML_HEADER_ID: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

/// Bounded prefix buffer for lossless container detection and reader replay (REQ-050, REQ-056).
///
/// Guarantees:
/// 1. Bounded resource: Capped at `max_capacity` (default 4 KiB, Global Rule 3).
/// 2. Non-consuming detection: Bytes fed to the sniffer can be retrieved intact via `replay()`.
/// 3. Replay fidelity: The target reader receives the exact original byte stream from offset 0.
#[derive(Debug, Clone)]
pub struct SniffBuffer {
    buffer: BytesMut,
    max_capacity: usize,
}

impl Default for SniffBuffer {
    fn default() -> Self {
        Self::new(4096)
    }
}

impl SniffBuffer {
    /// Creates a new SniffBuffer with an explicit maximum prefix capacity.
    pub fn new(max_capacity: usize) -> Self {
        Self {
            buffer: BytesMut::with_capacity(max_capacity.min(4096)),
            max_capacity,
        }
    }

    /// Feeds incoming stream bytes into the prefix buffer without consuming them from the stream.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), SniffError> {
        if self.buffer.len() + chunk.len() > self.max_capacity {
            return Err(SniffError::BufferOverflow {
                actual: self.buffer.len() + chunk.len(),
                max: self.max_capacity,
            });
        }
        self.buffer.extend_from_slice(chunk);
        Ok(())
    }

    /// Evaluates the currently buffered bytes to detect the container format.
    pub fn detect(&self) -> SniffResult {
        detect_container(&self.buffer)
    }

    /// Returns the complete accumulated prefix bytes intact for replay to the selected format reader.
    pub fn replay(&self) -> Bytes {
        self.buffer.clone().freeze()
    }

    /// Number of bytes currently held in the prefix buffer.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Clears the prefix buffer.
    pub fn clear(&mut self) {
        self.buffer.clear();
    }
}

/// Inspects a raw byte slice and classifies the media container format (REQ-050, REQ-056).
///
/// Never relies on file extension or out-of-band hints. Rejects ambiguous signatures.
pub fn detect_container(data: &[u8]) -> SniffResult {
    if data.is_empty() {
        return SniffResult::NeedsMoreData { needed_bytes: 4 };
    }

    // 1. FLV Detection (Adobe Flash Video File Format Spec v10.1 & Station FLVContentReader.cs)
    // Signature: "FLV", version 0x01, TypeFlags, DataOffset (9), PreviousTagSize0 (0)
    if data.starts_with(b"FLV") {
        if data.len() < 13 {
            // Need at least 13 bytes: 9 bytes FLV header + 4 bytes PreviousTagSize0
            return SniffResult::NeedsMoreData { needed_bytes: 13 };
        }
        let version = data[3];
        if version != 0x01 {
            // Only FLV Version 1 is supported by Station and standard flash encoders
            return SniffResult::Unsupported;
        }

        let flags = data[4];
        let has_audio = (flags & 0x04) != 0;
        let has_video = (flags & 0x01) != 0;

        // DataOffset: 4 bytes big-endian
        let data_offset = u32::from_be_bytes([data[5], data[6], data[7], data[8]]) as usize;
        if data_offset < 9 {
            return SniffResult::Unsupported;
        }

        // PreviousTagSize0: bytes 9..13 must be 0x00000000 per specification
        let prev_tag_size_0 = u32::from_be_bytes([data[9], data[10], data[11], data[12]]);
        if prev_tag_size_0 != 0 {
            return SniffResult::Unsupported;
        }

        return SniffResult::Definitive(ContainerInfo {
            container_type: ContainerType::FLV,
            content_type_str: "FLV".to_string(),
            mime_type: "video/x-flv".to_string(),
            has_audio,
            has_video,
        });
    }

    // Short read potential FLV prefix ("F", "FL")
    if (data.len() == 1 && data[0] == b'F') || (data.len() == 2 && &data[0..2] == b"FL") {
        return SniffResult::NeedsMoreData { needed_bytes: 13 };
    }

    // 2. MPEG-2 TS Detection (ISO/IEC 13818-1 & Station TSContentReader.cs)
    // Packets are exactly 188 bytes beginning with sync byte 0x47.
    // To prevent false positives from single 0x47 bytes in random binary streams,
    // verify consistent sync bytes across at least 2 consecutive TS packet boundaries.
    if data[0] == 0x47 {
        if data.len() < 188 {
            return SniffResult::NeedsMoreData { needed_bytes: 188 };
        }
        if data.len() < 376 {
            // If we have 188 bytes and byte 0 is 0x47, but less than 376 bytes, request second packet
            // to definitively distinguish TS from random data starting with 'G' (0x47)
            if data.len() >= 189 && data[188] == 0x47 {
                return SniffResult::Definitive(ContainerInfo {
                    container_type: ContainerType::TS,
                    content_type_str: "TS".to_string(),
                    mime_type: "video/mp2t".to_string(),
                    has_audio: true,
                    has_video: true,
                });
            }
            return SniffResult::NeedsMoreData { needed_bytes: 376 };
        }
        // Check sync byte at 188 and 376
        if data[188] == 0x47 && data[376] == 0x47 {
            return SniffResult::Definitive(ContainerInfo {
                container_type: ContainerType::TS,
                content_type_str: "TS".to_string(),
                mime_type: "video/mp2t".to_string(),
                has_audio: true,
                has_video: true,
            });
        }
    }

    // 3. Matroska / WebM Detection (EBML Header 0x1A 0x45 0xDF 0xA3 & Station MKVContentReader.cs)
    if data.starts_with(&EBML_HEADER_ID) {
        if data.len() < 32 {
            return SniffResult::NeedsMoreData { needed_bytes: 32 };
        }

        // Search for "webm" or "matroska" in the initial EBML header elements
        let is_webm = data[..data.len().min(128)]
            .windows(4)
            .any(|w| w == b"webm");

        let (ctype, mime) = if is_webm {
            ("WEBM", "video/webm")
        } else {
            ("MKV", "video/x-matroska")
        };

        return SniffResult::Definitive(ContainerInfo {
            container_type: if is_webm { ContainerType::WebM } else { ContainerType::MKV },
            content_type_str: ctype.to_string(),
            mime_type: mime.to_string(),
            has_audio: true,
            has_video: true,
        });
    }

    // Partial EBML prefix (0x1A, 0x1A 0x45, etc.)
    if (data.len() == 1 && data[0] == 0x1A)
        || (data.len() == 2 && data[..2] == EBML_HEADER_ID[..2])
        || (data.len() == 3 && data[..3] == EBML_HEADER_ID[..3])
    {
        return SniffResult::NeedsMoreData { needed_bytes: 32 };
    }

    // 4. ASF / WMV / WMA Detection (ASF Header Object GUID & Station ASFContentReader.cs)
    if data.starts_with(&ASF_HEADER_GUID) {
        return SniffResult::Definitive(ContainerInfo {
            container_type: ContainerType::ASF,
            content_type_str: "WMV".to_string(),
            mime_type: "video/x-ms-asf".to_string(),
            has_audio: true,
            has_video: true,
        });
    }

    // WMSP HTTP framing check ("$H" ASF data frames)
    if data.starts_with(b"$H") {
        return SniffResult::Definitive(ContainerInfo {
            container_type: ContainerType::ASF,
            content_type_str: "WMV".to_string(),
            mime_type: "video/x-ms-asf".to_string(),
            has_audio: true,
            has_video: true,
        });
    }

    // Partial ASF GUID prefix
    if data.len() < 16 && &ASF_HEADER_GUID[..data.len()] == data {
        return SniffResult::NeedsMoreData { needed_bytes: 16 };
    }

    // 5. Ogg Detection (OggS capture pattern & Station Ogg support)
    if data.starts_with(b"OggS") {
        return SniffResult::Definitive(ContainerInfo {
            container_type: ContainerType::Ogg,
            content_type_str: "OGG".to_string(),
            mime_type: "application/ogg".to_string(),
            has_audio: true,
            has_video: false,
        });
    }

    // Partial Ogg prefix ("O", "Og", "Ogg")
    if (data.len() == 1 && data[0] == b'O')
        || (data.len() == 2 && &data[..2] == b"Og")
        || (data.len() == 3 && &data[..3] == b"Ogg")
    {
        return SniffResult::NeedsMoreData { needed_bytes: 4 };
    }

    // 6. MP3 / ID3 Audio Detection (ID3 tag or MPEG-1/2 Audio sync word)
    if data.starts_with(b"ID3") {
        return SniffResult::Definitive(ContainerInfo {
            container_type: ContainerType::MP3,
            content_type_str: "MP3".to_string(),
            mime_type: "audio/mpeg".to_string(),
            has_audio: true,
            has_video: false,
        });
    }
    if data.len() >= 2 && data[0] == 0xFF && (data[1] & 0xE0) == 0xE0 && (data[1] & 0x06) != 0 {
        return SniffResult::Definitive(ContainerInfo {
            container_type: ContainerType::MP3,
            content_type_str: "MP3".to_string(),
            mime_type: "audio/mpeg".to_string(),
            has_audio: true,
            has_video: false,
        });
    }

    // If prefix is very short (< 4 bytes) and hasn't matched anything definitive or partial,
    // wait for at least 4 bytes before rejecting as unsupported
    if data.len() < 4 {
        return SniffResult::NeedsMoreData { needed_bytes: 4 };
    }

    // Conclusively unsupported format
    SniffResult::Unsupported
}
