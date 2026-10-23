//! MP3 (MPEG Audio Layer III) framing parser and capability profile (REQ-055, REQ-056).
//!
//! Scope Decision (P30, Step 3):
//! - MP3 audio is explicitly SUPPORTED for native distribution, direct HTTP playback
//!   (`audio/mpeg`, `.mp3`), and relay without codec transcoding.
//! - Enforces container-aware safe-start: admits frames immediately without waiting for
//!   video keyframes.
//! - Global Rule 2: Explicit state machine with entry re-entrancy guards.
//! - Global Rule 3: Bounded memory buffers and zero unbounded allocations.

use bytes::{Buf, Bytes, BytesMut};
use peercast_core::cache::HeaderSnapshot;
use thiserror::Error;

/// Default maximum buffer capacity for MP3 parsing (4 MiB).
pub const DEFAULT_MAX_MP3_BUFFER_CAPACITY: usize = 4 * 1024 * 1024;

/// Standard MPEG-1 Layer III bitrates in kbps (index 1..=14).
pub const MPEG1_L3_BITRATES_KBPS: [u32; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];

/// Standard MPEG-1 sampling rates in Hz (index 0..=2).
pub const MPEG1_SAMPLING_RATES_HZ: [u32; 4] = [44100, 48000, 32000, 0];

/// Standard MPEG-2 Layer III bitrates in kbps (index 1..=14).
pub const MPEG2_L3_BITRATES_KBPS: [u32; 16] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
];

/// Standard MPEG-2 sampling rates in Hz (index 0..=2).
pub const MPEG2_SAMPLING_RATES_HZ: [u32; 4] = [22050, 24000, 16000, 0];

/// Standard MPEG-2.5 sampling rates in Hz (index 0..=2).
pub const MPEG25_SAMPLING_RATES_HZ: [u32; 4] = [11025, 12000, 8000, 0];

/// Errors encountered during MP3 stream parsing.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum Mp3ParseError {
    #[error("Invalid MPEG sync word: expected 11-bit sync (0xFFE / 0xFFF)")]
    InvalidSyncWord,

    #[error("Unsupported MPEG layer: only Layer III (MP3) is supported")]
    UnsupportedLayer,

    #[error("Invalid bitrate index 0 or 15 in frame header")]
    InvalidBitrateIndex,

    #[error("Invalid sampling rate index 3 in frame header")]
    InvalidSamplingRateIndex,

    #[error("Buffer capacity {actual} exceeded configured maximum {max} (Global Rule 3)")]
    BufferCapacityExceeded { actual: usize, max: usize },

    #[error("Unexpected EOF during stream parsing")]
    UnexpectedEof,
}

/// Parsed MPEG Audio Frame Header (4 bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mp3FrameHeader {
    pub mpeg_version: u8, // 1 = MPEG-1, 2 = MPEG-2, 3 = MPEG-2.5
    pub layer: u8,        // 3 = Layer III
    pub has_crc: bool,
    pub bitrate_kbps: u32,
    pub sample_rate_hz: u32,
    pub padding: bool,
    pub channels: u8, // 1 = Mono, 2 = Stereo / Joint / Dual
    pub frame_len: usize,
}

impl Mp3FrameHeader {
    /// Attempts to parse an MPEG audio frame header from a 4-byte slice.
    pub fn parse(bytes: &[u8]) -> Result<Option<Self>, Mp3ParseError> {
        if bytes.len() < 4 {
            return Ok(None);
        }

        let b0 = bytes[0];
        let b1 = bytes[1];
        let b2 = bytes[2];
        let b3 = bytes[3];

        // 11-bit sync word: b0 == 0xFF and (b1 & 0xE0) == 0xE0
        if b0 != 0xFF || (b1 & 0xE0) != 0xE0 {
            return Err(Mp3ParseError::InvalidSyncWord);
        }

        // MPEG Audio version ID:
        // 00 = MPEG-2.5, 01 = reserved, 10 = MPEG-2, 11 = MPEG-1
        let version_bits = (b1 >> 3) & 0x03;
        let mpeg_version = match version_bits {
            0b00 => 3, // MPEG-2.5
            0b10 => 2, // MPEG-2
            0b11 => 1, // MPEG-1
            _ => return Err(Mp3ParseError::InvalidSyncWord),
        };

        // Layer description:
        // 01 = Layer III, 10 = Layer II, 11 = Layer I
        let layer_bits = (b1 >> 1) & 0x03;
        if layer_bits != 0b01 {
            return Err(Mp3ParseError::UnsupportedLayer);
        }

        let has_crc = (b1 & 0x01) == 0;

        let bitrate_idx = ((b2 >> 4) & 0x0F) as usize;
        if bitrate_idx == 0 || bitrate_idx == 15 {
            return Err(Mp3ParseError::InvalidBitrateIndex);
        }

        let sr_idx = ((b2 >> 2) & 0x03) as usize;
        if sr_idx == 3 {
            return Err(Mp3ParseError::InvalidSamplingRateIndex);
        }

        let (bitrate_kbps, sample_rate_hz) = if mpeg_version == 1 {
            (MPEG1_L3_BITRATES_KBPS[bitrate_idx], MPEG1_SAMPLING_RATES_HZ[sr_idx])
        } else if mpeg_version == 2 {
            (MPEG2_L3_BITRATES_KBPS[bitrate_idx], MPEG2_SAMPLING_RATES_HZ[sr_idx])
        } else {
            (MPEG2_L3_BITRATES_KBPS[bitrate_idx], MPEG25_SAMPLING_RATES_HZ[sr_idx])
        };

        let padding = ((b2 >> 1) & 0x01) != 0;
        let pad_bytes = if padding { 1 } else { 0 };

        // Channel mode: 11 = Single channel (Mono), others = Stereo
        let mode = (b3 >> 6) & 0x03;
        let channels = if mode == 0b11 { 1 } else { 2 };

        // Frame length calculation:
        // MPEG-1: frame_len = (144 * bitrate_bps) / sample_rate + padding
        // MPEG-2/2.5: frame_len = (72 * bitrate_bps) / sample_rate + padding
        let factor = if mpeg_version == 1 { 144 } else { 72 };
        let frame_len = ((factor * bitrate_kbps * 1000) / sample_rate_hz) as usize + pad_bytes;

        Ok(Some(Self {
            mpeg_version,
            layer: 3,
            has_crc,
            bitrate_kbps,
            sample_rate_hz,
            padding,
            channels,
            frame_len,
        }))
    }
}

/// Output event yielded by `Mp3Demuxer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mp3DemuxOutput {
    /// Initial header snapshot established with metadata.
    HeaderSnapshotReady {
        snapshot: HeaderSnapshot,
        epoch: u64,
        bitrate_kbps: u32,
        sample_rate_hz: u32,
    },
    /// Relayed MP3 audio frame.
    ContentFrame {
        frame: Bytes,
        epoch: u64,
        position: u64,
    },
    /// End of stream reached.
    EndOfStream,
}

/// Demuxer state machine (Global Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3DemuxerState {
    AwaitingFirstFrame,
    Streaming,
    Ended,
}

/// Streaming MP3 frame parser and demuxer (REQ-055, REQ-056).
#[derive(Debug)]
pub struct Mp3Demuxer {
    state: Mp3DemuxerState,
    buffer: BytesMut,
    max_buffer_capacity: usize,
    current_epoch: u64,
    stream_position: u64,
    first_frame_info: Option<Mp3FrameHeader>,
    header_snapshot: Option<HeaderSnapshot>,
}

impl Default for Mp3Demuxer {
    fn default() -> Self {
        Self::new()
    }
}

impl Mp3Demuxer {
    /// Creates a new Mp3Demuxer with default limits.
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_MP3_BUFFER_CAPACITY)
    }

    /// Creates an Mp3Demuxer with explicit memory bounds (Global Rule 3).
    pub fn with_limits(max_buffer_capacity: usize) -> Self {
        Self {
            state: Mp3DemuxerState::AwaitingFirstFrame,
            buffer: BytesMut::with_capacity(8192),
            max_buffer_capacity,
            current_epoch: 0,
            stream_position: 0,
            first_frame_info: None,
            header_snapshot: None,
        }
    }

    /// Feeds incoming stream bytes into the demuxer buffer.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), Mp3ParseError> {
        if self.buffer.len() + chunk.len() > self.max_buffer_capacity {
            return Err(Mp3ParseError::BufferCapacityExceeded {
                actual: self.buffer.len() + chunk.len(),
                max: self.max_buffer_capacity,
            });
        }
        self.buffer.extend_from_slice(chunk);
        Ok(())
    }

    /// Advances the demuxer and yields the next demux output.
    pub fn next_output(&mut self) -> Result<Option<Mp3DemuxOutput>, Mp3ParseError> {
        if self.state == Mp3DemuxerState::Ended {
            return Ok(None);
        }

        loop {
            // Check for and skip ID3v2 tag at stream prefix:
            // "ID3" (3 bytes) + version (2 bytes) + flags (1 byte) + synchsafe size (4 bytes) = 10 bytes
            if self.buffer.len() >= 10 && &self.buffer[0..3] == b"ID3" {
                let s0 = (self.buffer[6] & 0x7F) as usize;
                let s1 = (self.buffer[7] & 0x7F) as usize;
                let s2 = (self.buffer[8] & 0x7F) as usize;
                let s3 = (self.buffer[9] & 0x7F) as usize;
                let tag_len = 10 + (s0 << 21 | s1 << 14 | s2 << 7 | s3);
                if self.buffer.len() < tag_len {
                    return Ok(None);
                }
                self.buffer.advance(tag_len);
                continue;
            }

            if self.buffer.len() < 4 {
                return Ok(None);
            }

            // Sync search for 0xFF
            if self.buffer[0] != 0xFF || (self.buffer[1] & 0xE0) != 0xE0 {
                if let Some(pos) = self.buffer.windows(2).position(|w| w[0] == 0xFF && (w[1] & 0xE0) == 0xE0) {
                    self.buffer.advance(pos);
                } else {
                    let keep = self.buffer.len().min(1);
                    let discard = self.buffer.len() - keep;
                    self.buffer.advance(discard);
                    return Ok(None);
                }
            }

            let header = match Mp3FrameHeader::parse(&self.buffer)? {
                Some(h) => h,
                None => return Ok(None),
            };

            if self.buffer.len() < header.frame_len {
                return Ok(None);
            }

            let frame_bytes = self.buffer.split_to(header.frame_len).freeze();

            match self.state {
                Mp3DemuxerState::AwaitingFirstFrame => {
                    let snapshot = HeaderSnapshot::new(
                        self.current_epoch as i32,
                        self.stream_position as i64,
                        "MP3",
                        frame_bytes.clone(),
                    );

                    self.first_frame_info = Some(header.clone());
                    self.header_snapshot = Some(snapshot.clone());
                    self.state = Mp3DemuxerState::Streaming;

                    let out = Mp3DemuxOutput::HeaderSnapshotReady {
                        snapshot,
                        epoch: self.current_epoch,
                        bitrate_kbps: header.bitrate_kbps,
                        sample_rate_hz: header.sample_rate_hz,
                    };
                    self.stream_position += frame_bytes.len() as u64;
                    return Ok(Some(out));
                }
                Mp3DemuxerState::Streaming => {
                    let pos = self.stream_position;
                    self.stream_position += frame_bytes.len() as u64;
                    return Ok(Some(Mp3DemuxOutput::ContentFrame {
                        frame: frame_bytes,
                        epoch: self.current_epoch,
                        position: pos,
                    }));
                }
                Mp3DemuxerState::Ended => {
                    return Ok(None);
                }
            }
        }
    }

    /// Explicitly signals EOF on input stream.
    pub fn signal_eof(&mut self) -> Result<Option<Mp3DemuxOutput>, Mp3ParseError> {
        if self.state == Mp3DemuxerState::Ended {
            return Ok(None);
        }
        self.state = Mp3DemuxerState::Ended;
        Ok(Some(Mp3DemuxOutput::EndOfStream))
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
    pub fn state(&self) -> Mp3DemuxerState {
        self.state
    }
}
