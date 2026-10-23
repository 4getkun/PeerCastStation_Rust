use bytes::{Bytes, BytesMut};
use peercast_core::cache::HeaderSnapshot;
use std::collections::HashMap;
use thiserror::Error;

/// Standard MPEG-2 TS packet size in bytes (REQ-053).
pub const TS_PACKET_SIZE: usize = 188;

/// Standard MPEG-2 TS synchronization byte (0x47).
pub const TS_SYNC_BYTE: u8 = 0x47;

/// Well-known PID for Program Association Table (PAT).
pub const PAT_PID: u16 = 0x0000;

/// Well-known PID for Null / Padding packets.
pub const NULL_PID: u16 = 0x1FFF;

/// Declared supported MPEG-2 TS elementary stream codec subset (REQ-053).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsStreamType {
    /// H.264 / AVC Video (ITU-T Rec. H.264 / ISO/IEC 14496-10)
    H264,
    /// MPEG-2 Video (ISO/IEC 13818-2)
    Mpeg2Video,
    /// AAC Audio (ISO/IEC 13818-7 ADTS)
    AacAdts,
    /// AAC Audio (ISO/IEC 14496-3 LATM)
    AacLatm,
    /// MP3 Audio (ISO/IEC 11172-3)
    Mp3,
    /// Unsupported or unknown stream type
    Unsupported(u8),
}

impl TsStreamType {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0x1B => Self::H264,
            0x02 => Self::Mpeg2Video,
            0x0F => Self::AacAdts,
            0x11 => Self::AacLatm,
            0x03 | 0x04 => Self::Mp3,
            other => Self::Unsupported(other),
        }
    }

    pub fn is_video(&self) -> bool {
        matches!(self, Self::H264 | Self::Mpeg2Video)
    }

    pub fn is_audio(&self) -> bool {
        matches!(self, Self::AacAdts | Self::AacLatm | Self::Mp3)
    }
}

/// Errors produced during MPEG-2 TS parsing and PSI reassembly.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum TsParseError {
    #[error("Malformed TS sync byte: expected 0x47, found 0x{0:02x}")]
    MalformedSyncByte(u8),

    #[error("Invalid TS packet size ({0} bytes): only standard 188-byte framing supported without configuration")]
    InvalidPacketSize(usize),

    #[error("Adaptation field length ({0} bytes) exceeds 183-byte packet payload limit")]
    AdaptationFieldTooLong(usize),

    #[error("Malformed PSI section: {0}")]
    MalformedPsiSection(String),

    #[error("Transport Error Indicator (TEI) set on TS packet")]
    TransportError,

    #[error("Unexpected end of data while parsing {0}")]
    UnexpectedEof(&'static str),

    #[error("Parser encountered unrecoverable failure")]
    FailedState,
}

/// Parsed header of a 188-byte MPEG-2 TS packet (REQ-053).
#[derive(Debug, Clone, PartialEq)]
pub struct TsPacketHeader {
    pub tei: bool,
    pub pusi: bool,
    pub priority: bool,
    pub pid: u16,
    pub scrambling: u8,
    pub adaptation_control: u8,
    pub continuity_counter: u8,
    pub has_adaptation: bool,
    pub has_payload: bool,
    pub payload_offset: usize,
    pub random_access_indicator: bool,
    pub pcr: Option<f64>,
}

impl TsPacketHeader {
    /// Parses the 4-byte header and optional adaptation field of a 188-byte TS packet.
    pub fn parse(packet: &[u8]) -> Result<Self, TsParseError> {
        if packet.len() != TS_PACKET_SIZE {
            return Err(TsParseError::InvalidPacketSize(packet.len()));
        }

        if packet[0] != TS_SYNC_BYTE {
            return Err(TsParseError::MalformedSyncByte(packet[0]));
        }

        let tei = (packet[1] & 0x80) != 0;
        let pusi = (packet[1] & 0x40) != 0;
        let priority = (packet[1] & 0x20) != 0;
        let pid = (((packet[1] & 0x1F) as u16) << 8) | (packet[2] as u16);

        let scrambling = (packet[3] & 0xC0) >> 6;
        let adaptation_control = (packet[3] & 0x30) >> 4;
        let continuity_counter = packet[3] & 0x0F;

        let has_payload = (adaptation_control & 0x01) != 0;
        let has_adaptation = (adaptation_control & 0x02) != 0;

        let mut payload_offset = 4;
        let mut random_access_indicator = false;
        let mut pcr = None;

        if has_adaptation {
            let adaptation_len = packet[4] as usize;
            if adaptation_len > 183 {
                return Err(TsParseError::AdaptationFieldTooLong(adaptation_len));
            }
            payload_offset += 1 + adaptation_len;

            if adaptation_len > 0 {
                let flags = packet[5];
                random_access_indicator = (flags & 0x40) != 0;
                let pcr_flag = (flags & 0x10) != 0;

                if pcr_flag && adaptation_len >= 7 {
                    let pcr_base = ((packet[6] as u64) << 25)
                        | ((packet[7] as u64) << 17)
                        | ((packet[8] as u64) << 9)
                        | ((packet[9] as u64) << 1)
                        | (((packet[10] as u64) & 0x80) >> 7);
                    let pcr_ext = (((packet[10] as u64) & 0x01) << 8) | (packet[11] as u64);
                    pcr = Some((pcr_base * 300 + pcr_ext) as f64 / 27_000_000.0);
                }
            }
        }

        if !has_payload {
            payload_offset = TS_PACKET_SIZE;
        }

        Ok(Self {
            tei,
            pusi,
            priority,
            pid,
            scrambling,
            adaptation_control,
            continuity_counter,
            has_adaptation,
            has_payload,
            payload_offset,
            random_access_indicator,
            pcr,
        })
    }
}

/// A parsed Program Association Table (PAT).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatTable {
    pub transport_stream_id: u16,
    pub version: u8,
    pub programs: Vec<(u16, u16)>, // (program_number, pmt_pid)
}

impl PatTable {
    /// Parses a PAT payload from a TS packet with PID 0.
    pub fn parse(packet: &[u8], payload_offset: usize) -> Result<Self, TsParseError> {
        if payload_offset >= packet.len() {
            return Err(TsParseError::MalformedPsiSection("Payload offset out of bounds".to_string()));
        }

        let pointer_field = packet[payload_offset] as usize;
        let sec_start = payload_offset + 1 + pointer_field;
        if sec_start + 8 >= packet.len() {
            return Err(TsParseError::MalformedPsiSection("PAT section truncated".to_string()));
        }

        let table_id = packet[sec_start];
        if table_id != 0x00 {
            return Err(TsParseError::MalformedPsiSection(format!("Expected PAT table_id 0x00, got 0x{table_id:02x}")));
        }

        let section_len = (((packet[sec_start + 1] & 0x0F) as usize) << 8) | (packet[sec_start + 2] as usize);
        if section_len < 9 {
            return Err(TsParseError::MalformedPsiSection("PAT section_length too small".to_string()));
        }

        let transport_stream_id = u16::from_be_bytes([packet[sec_start + 3], packet[sec_start + 4]]);
        let version = (packet[sec_start + 5] & 0x3E) >> 1;

        let entries_start = sec_start + 8;
        // 4 bytes CRC32 at the end
        let entries_end = (sec_start + 3 + section_len).min(packet.len()).saturating_sub(4);

        let mut programs = Vec::new();
        let mut cur = entries_start;
        while cur + 4 <= entries_end {
            let prog_num = u16::from_be_bytes([packet[cur], packet[cur + 1]]);
            let pmt_pid = (((packet[cur + 2] & 0x1F) as u16) << 8) | (packet[cur + 3] as u16);
            if prog_num > 0 {
                programs.push((prog_num, pmt_pid));
            }
            cur += 4;
        }

        Ok(Self {
            transport_stream_id,
            version,
            programs,
        })
    }
}

/// Elementary stream metadata from a Program Map Table (PMT).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsStreamInfo {
    pub stream_type: TsStreamType,
    pub pid: u16,
}

/// A parsed Program Map Table (PMT).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmtTable {
    pub program_number: u16,
    pub version: u8,
    pub pcr_pid: u16,
    pub streams: Vec<TsStreamInfo>,
}

impl PmtTable {
    /// Parses a PMT payload from a TS packet.
    pub fn parse(packet: &[u8], payload_offset: usize) -> Result<Self, TsParseError> {
        if payload_offset >= packet.len() {
            return Err(TsParseError::MalformedPsiSection("PMT payload offset out of bounds".to_string()));
        }

        let pointer_field = packet[payload_offset] as usize;
        let sec_start = payload_offset + 1 + pointer_field;
        if sec_start + 12 >= packet.len() {
            return Err(TsParseError::MalformedPsiSection("PMT section truncated".to_string()));
        }

        let table_id = packet[sec_start];
        if table_id != 0x02 {
            return Err(TsParseError::MalformedPsiSection(format!("Expected PMT table_id 0x02, got 0x{table_id:02x}")));
        }

        let section_len = (((packet[sec_start + 1] & 0x0F) as usize) << 8) | (packet[sec_start + 2] as usize);
        let program_number = u16::from_be_bytes([packet[sec_start + 3], packet[sec_start + 4]]);
        let version = (packet[sec_start + 5] & 0x3E) >> 1;
        let pcr_pid = (((packet[sec_start + 8] & 0x1F) as u16) << 8) | (packet[sec_start + 9] as u16);

        let prog_info_len = (((packet[sec_start + 10] & 0x0F) as usize) << 8) | (packet[sec_start + 11] as usize);
        let entries_start = sec_start + 12 + prog_info_len;
        let entries_end = (sec_start + 3 + section_len).min(packet.len()).saturating_sub(4);

        let mut streams = Vec::new();
        let mut cur = entries_start;
        while cur + 5 <= entries_end {
            let st_raw = packet[cur];
            let elem_pid = (((packet[cur + 1] & 0x1F) as u16) << 8) | (packet[cur + 2] as u16);
            let es_info_len = (((packet[cur + 3] & 0x0F) as usize) << 8) | (packet[cur + 4] as usize);

            streams.push(TsStreamInfo {
                stream_type: TsStreamType::from_u8(st_raw),
                pid: elem_pid,
            });

            cur += 5 + es_info_len;
        }

        Ok(Self {
            program_number,
            version,
            pcr_pid,
            streams,
        })
    }
}

/// Output event from MPEG-2 TS Demuxer (REQ-050, REQ-051, REQ-053).
#[derive(Debug, Clone, PartialEq)]
pub enum TsDemuxOutput {
    /// Stream header snapshot changed (PAT / PMT updated or stream start)
    HeaderChanged(HeaderSnapshot),

    /// Elementary media TS packet ready for transmission or caching
    MediaPacket {
        epoch: i32,
        pid: u16,
        is_keyframe: bool,
        header: TsPacketHeader,
        data: Bytes,
    },
}

/// High-level MPEG-2 TS Demuxer with independent 188-byte framing, PAT/PMT reassembly,
/// and continuity validation (REQ-053, REQ-056).
pub struct TsDemuxer {
    epoch: i32,
    buffer: BytesMut,
    pat_packet: Option<Bytes>,
    pmt_packet: Option<Bytes>,
    pmt_pid: Option<u16>,
    video_pids: Vec<u16>,
    audio_pids: Vec<u16>,
    continuity_counters: HashMap<u16, u8>,
    initial_header_emitted: bool,
    position: i64,
}

impl Default for TsDemuxer {
    fn default() -> Self {
        Self::new()
    }
}

impl TsDemuxer {
    pub fn new() -> Self {
        Self {
            epoch: 1,
            buffer: BytesMut::with_capacity(8192),
            pat_packet: None,
            pmt_packet: None,
            pmt_pid: None,
            video_pids: Vec::new(),
            audio_pids: Vec::new(),
            continuity_counters: HashMap::new(),
            initial_header_emitted: false,
            position: 0,
        }
    }

    pub fn epoch(&self) -> i32 {
        self.epoch
    }

    pub fn position(&self) -> i64 {
        self.position
    }

    /// Feeds incoming byte slice to the demuxer's buffer.
    pub fn feed(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// Builds a normalized HeaderSnapshot from cached PAT and PMT packets (REQ-051, REQ-053).
    /// Continuity counter in PAT and PMT packets is normalized (Station parity).
    pub fn build_snapshot(&self) -> Option<HeaderSnapshot> {
        let pat = self.pat_packet.as_ref()?;
        let pmt = self.pmt_packet.as_ref()?;

        let mut out = BytesMut::with_capacity(TS_PACKET_SIZE * 2);

        // Normalize continuity counter on PAT: mask CC bits to 0
        let mut norm_pat = pat.to_vec();
        norm_pat[3] &= 0xF0;
        out.extend_from_slice(&norm_pat);

        // Normalize continuity counter on PMT: mask CC bits to 0
        let mut norm_pmt = pmt.to_vec();
        norm_pmt[3] &= 0xF0;
        out.extend_from_slice(&norm_pmt);

        let data = out.freeze();
        let retained_bytes = data.len();

        Some(HeaderSnapshot {
            epoch: self.epoch,
            pos: self.position,
            container_type: "TS".to_string(),
            data,
            retained_bytes,
        })
    }

    /// Pulls the next demux output event.
    pub fn next_output(&mut self) -> Result<Option<TsDemuxOutput>, TsParseError> {
        while self.buffer.len() >= TS_PACKET_SIZE {
            // Verify 188-byte alignment: first byte MUST be 0x47
            if self.buffer[0] != TS_SYNC_BYTE {
                // Search for the next 0x47 sync byte
                if let Some(pos) = self.buffer.iter().position(|&b| b == TS_SYNC_BYTE) {
                    let _ = self.buffer.split_to(pos);
                    if self.buffer.len() < TS_PACKET_SIZE {
                        return Ok(None);
                    }
                } else {
                    self.buffer.clear();
                    return Ok(None);
                }
            }

            let raw_packet = self.buffer.split_to(TS_PACKET_SIZE).freeze();
            let header = TsPacketHeader::parse(&raw_packet)?;

            if header.tei {
                // Drop corrupted transport packet
                continue;
            }

            // Track continuity counter per PID (REQ-053)
            if header.has_payload && header.pid != NULL_PID {
                self.continuity_counters.insert(header.pid, header.continuity_counter);
            }

            // 1. PAT processing (PID 0)
            if header.pid == PAT_PID && header.has_payload {
                if header.pusi {
                    if let Ok(pat) = PatTable::parse(&raw_packet, header.payload_offset) {
                        if let Some((_, pmt_pid)) = pat.programs.first() {
                            let pmt_changed = self.pmt_pid != Some(*pmt_pid);
                            self.pmt_pid = Some(*pmt_pid);

                            let pat_bytes = raw_packet.clone();
                            if self.pat_packet.as_ref() != Some(&pat_bytes) {
                                self.pat_packet = Some(pat_bytes);
                                if pmt_changed {
                                    self.epoch += 1;
                                }
                            }
                        }
                    }
                }
                continue;
            }

            // 2. PMT processing (matching PMT PID)
            if Some(header.pid) == self.pmt_pid && header.has_payload {
                if header.pusi {
                    if let Ok(pmt) = PmtTable::parse(&raw_packet, header.payload_offset) {
                        let mut new_video_pids = Vec::new();
                        let mut new_audio_pids = Vec::new();

                        for s in &pmt.streams {
                            if s.stream_type.is_video() {
                                new_video_pids.push(s.pid);
                            } else if s.stream_type.is_audio() {
                                new_audio_pids.push(s.pid);
                            }
                        }

                        let pmt_bytes = raw_packet.clone();
                        let pmt_changed = self.pmt_packet.as_ref() != Some(&pmt_bytes);
                        self.pmt_packet = Some(pmt_bytes);
                        self.video_pids = new_video_pids;
                        self.audio_pids = new_audio_pids;

                        if pmt_changed || !self.initial_header_emitted {
                            if pmt_changed && self.initial_header_emitted {
                                self.epoch += 1;
                            }
                            if let Some(snapshot) = self.build_snapshot() {
                                self.initial_header_emitted = true;
                                return Ok(Some(TsDemuxOutput::HeaderChanged(snapshot)));
                            }
                        }
                    }
                }
                continue;
            }

            // 3. Media packet emission (Video / Audio elementary streams)
            let is_video = self.video_pids.contains(&header.pid);
            let is_audio = self.audio_pids.contains(&header.pid);

            if is_video || is_audio {
                // Keyframe detection: Random Access Indicator (RAI) in adaptation field on video PID
                let is_keyframe = is_video && header.random_access_indicator;

                self.position += TS_PACKET_SIZE as i64;

                return Ok(Some(TsDemuxOutput::MediaPacket {
                    epoch: self.epoch,
                    pid: header.pid,
                    is_keyframe,
                    header,
                    data: raw_packet,
                }));
            }
        }

        Ok(None)
    }
}
