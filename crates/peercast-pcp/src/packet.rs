use bytes::Bytes;
use thiserror::Error;

use peercast_core::atom::{Atom, AtomError};
use peercast_core::cache::{
    BoundedContentCache, CacheClock, CacheError, HeaderSnapshot, PositionContinuity,
    SystemCacheClock,
};
use peercast_core::channel_id::ChannelId;
use peercast_core::content::{Content, ContinuationFlags};
use peercast_core::id4::ID4;

/// Receiver hard limit for a single PCP body data packet (16 KiB, REQ-026, A07).
pub const MAX_PACKET_DATALEN: usize = 16384;

/// Outbound packet body split threshold (15 KiB = 15,360 bytes, matching Station MaxBodyLength).
/// Provides a 1 KiB safety margin under the 16 KiB receiver cap.
pub const SPLIT_BODY_DATALEN: usize = 15 * 1024;

/// Default maximum allowed header size before rejecting as unsupported/oversized (128 KiB).
pub const DEFAULT_MAX_HEADER_LEN: usize = 128 * 1024;

/// Errors produced during packet parsing, splitting, or cache adaptation.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum PacketError {
    #[error("Not a channel packet Atom (tag was {0:?})")]
    NotAChannelPacket(ID4),

    #[error("Packet data exceeds 16 KiB wire limit ({actual} > {max} bytes) (REQ-026, A07)")]
    PacketDataTooLarge { actual: usize, max: usize },

    #[error("Header exceeds maximum allowed budget ({actual} > {max} bytes) (REQ-026)")]
    HeaderTooLarge { actual: usize, max: usize },

    #[error("Missing payload: packet contains neither head nor data leaf")]
    MissingPayload,

    #[error("Cache error: {0}")]
    Cache(#[from] CacheError),

    #[error("Atom codec error: {0}")]
    Atom(#[from] AtomError),
}

/// The payload type carried by a channel packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketPayload {
    /// Stream header initialization or codec config.
    Header(Bytes),
    /// Stream body media data.
    Data(Bytes),
}

impl PacketPayload {
    pub fn len(&self) -> usize {
        match self {
            PacketPayload::Header(b) => b.len(),
            PacketPayload::Data(b) => b.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A parsed, structured PCP channel packet (REQ-026, REQ-050).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelPacket {
    /// Optional channel ID if attached to `chan` parent container.
    pub channel_id: Option<ChannelId>,
    /// Container/stream format tag (from `type` child Atom).
    pub stream_type: ID4,
    /// Unwrapped 64-bit monotonic stream position.
    pub pos: i64,
    /// Continuation flag (0 = frame/safe start boundary, 1 = continuation fragment).
    pub cont: ContinuationFlags,
    /// Packet payload (header or body).
    pub payload: PacketPayload,
    /// Preserved unknown extension atoms within budget (REQ-021).
    pub unknown_extensions: Vec<Atom>,
}

impl ChannelPacket {
    /// Parses a `chan` or `pkt` Atom into a `ChannelPacket`, unwrapping 32-bit wire rollover.
    pub fn from_atom(atom: &Atom, last_unwrapped_pos: Option<i64>) -> Result<Self, PacketError> {
        let mut channel_id = None;
        let pkt_atom = match atom.name() {
            ID4::PCP_CHAN => {
                let mut found_pkt = None;
                if let Some(children) = atom.children() {
                    for child in children {
                        if child.name() == ID4::CHAN_ID {
                            if let Some(bytes) = child.leaf_bytes() {
                                if bytes.len() == 16 {
                                    let mut arr = [0u8; 16];
                                    arr.copy_from_slice(bytes);
                                    channel_id = Some(ChannelId::from_bytes(arr));
                                }
                            }
                        } else if child.name() == ID4::CHAN_PKT {
                            found_pkt = Some(child);
                        }
                    }
                }
                found_pkt.ok_or(PacketError::NotAChannelPacket(atom.name()))?
            }
            ID4::CHAN_PKT => atom,
            _ => return Err(PacketError::NotAChannelPacket(atom.name())),
        };

        let mut stream_type = ID4(*b"raw\0");
        let mut raw_wire_pos = 0u32;
        let mut cont = ContinuationFlags::NONE;
        let mut head_data = None;
        let mut body_data = None;
        let mut unknown_extensions = Vec::new();

        if let Some(children) = pkt_atom.children() {
            for child in children {
                match child.name() {
                    ID4::PKT_TYPE => {
                        if let Some(bytes) = child.leaf_bytes() {
                            if bytes.len() == 4 {
                                let mut arr = [0u8; 4];
                                arr.copy_from_slice(bytes);
                                stream_type = ID4(arr);
                            }
                        }
                    }
                    ID4::PKT_POS => {
                        if let Ok(p) = child.get_u32_le() {
                            raw_wire_pos = p;
                        }
                    }
                    ID4::PKT_CONT => {
                        if let Ok(c) = child.get_u8() {
                            cont = ContinuationFlags(c);
                        }
                    }
                    ID4::PKT_HEAD => {
                        if let Some(b) = child.leaf_bytes() {
                            head_data = Some(b.clone());
                        }
                    }
                    ID4::PKT_DATA => {
                        if let Some(b) = child.leaf_bytes() {
                            body_data = Some(b.clone());
                        }
                    }
                    _ => {
                        // Preserve unknown extensions inside packet container
                        unknown_extensions.push(child.clone());
                    }
                }
            }
        }

        let payload = if stream_type == ID4::PKT_TYPE_HEAD || head_data.is_some() {
            let h = head_data.or(body_data).ok_or(PacketError::MissingPayload)?;
            PacketPayload::Header(h)
        } else {
            let d = body_data.or(head_data).ok_or(PacketError::MissingPayload)?;
            if d.len() > MAX_PACKET_DATALEN {
                return Err(PacketError::PacketDataTooLarge {
                    actual: d.len(),
                    max: MAX_PACKET_DATALEN,
                });
            }
            PacketPayload::Data(d)
        };

        let unwrapped_pos = unwrap_wire_pos(raw_wire_pos, last_unwrapped_pos);

        Ok(Self {
            channel_id,
            stream_type,
            pos: unwrapped_pos,
            cont,
            payload,
            unknown_extensions,
        })
    }

    /// Serializes this packet into outbound wire Atoms, automatically splitting body data
    /// into safe packets <= 15,360 bytes (Station `MaxBodyLength` parity, REQ-026, A07).
    pub fn to_atoms(&self) -> Result<Vec<Atom>, PacketError> {
        match &self.payload {
            PacketPayload::Header(h) => {
                let mut pkt_children = vec![
                    Atom::new_leaf(ID4::PKT_TYPE, Bytes::copy_from_slice(&ID4::PKT_TYPE_HEAD.0)),
                    Atom::new_u32_le(ID4::PKT_POS, self.pos as u32),
                ];
                if self.cont != ContinuationFlags::NONE {
                    pkt_children.push(Atom::new_u8(ID4::PKT_CONT, self.cont.raw()));
                }
                pkt_children.push(Atom::new_leaf(ID4::PKT_DATA, h.clone()));
                for ext in &self.unknown_extensions {
                    pkt_children.push(ext.clone());
                }
                let pkt = Atom::new_parent(ID4::CHAN_PKT, pkt_children);
                let root = self.wrap_chan_if_needed(pkt);
                Ok(vec![root])
            }

            PacketPayload::Data(d) => {
                let stream_type_tag = if self.stream_type == ID4(*b"raw\0") {
                    ID4::PKT_TYPE_DATA
                } else {
                    self.stream_type
                };

                if d.len() <= SPLIT_BODY_DATALEN {
                    let mut pkt_children = vec![
                        Atom::new_leaf(ID4::PKT_TYPE, Bytes::copy_from_slice(&stream_type_tag.0)),
                        Atom::new_u32_le(ID4::PKT_POS, self.pos as u32),
                    ];
                    if self.cont != ContinuationFlags::NONE {
                        pkt_children.push(Atom::new_u8(ID4::PKT_CONT, self.cont.raw()));
                    }
                    pkt_children.push(Atom::new_leaf(ID4::PKT_DATA, d.clone()));
                    for ext in &self.unknown_extensions {
                        pkt_children.push(ext.clone());
                    }
                    let pkt = Atom::new_parent(ID4::CHAN_PKT, pkt_children);
                    let root = self.wrap_chan_if_needed(pkt);
                    Ok(vec![root])
                } else {
                    // Split body data safely under SPLIT_BODY_DATALEN (15,360 bytes, Station parity)
                    let mut atoms = Vec::new();
                    let mut offset = 0;
                    let total = d.len();

                    while offset < total {
                        let chunk_size = (total - offset).min(SPLIT_BODY_DATALEN);
                        let sub_slice = d.slice(offset..offset + chunk_size);
                        let sub_pos = (self.pos + offset as i64) as u32;

                        // First sub-packet preserves original cont; subsequent fragments are cont=1
                        let sub_cont = if offset == 0 {
                            self.cont
                        } else {
                            ContinuationFlags(1)
                        };

                        let mut pkt_children = vec![
                            Atom::new_leaf(
                                ID4::PKT_TYPE,
                                Bytes::copy_from_slice(&stream_type_tag.0),
                            ),
                            Atom::new_u32_le(ID4::PKT_POS, sub_pos),
                        ];
                        if sub_cont != ContinuationFlags::NONE {
                            pkt_children.push(Atom::new_u8(ID4::PKT_CONT, sub_cont.raw()));
                        }
                        pkt_children.push(Atom::new_leaf(ID4::PKT_DATA, sub_slice));

                        // Attach extensions to the first packet
                        if offset == 0 {
                            for ext in &self.unknown_extensions {
                                pkt_children.push(ext.clone());
                            }
                        }

                        let pkt = Atom::new_parent(ID4::CHAN_PKT, pkt_children);
                        let root = self.wrap_chan_if_needed(pkt);
                        atoms.push(root);

                        offset += chunk_size;
                    }

                    Ok(atoms)
                }
            }
        }
    }

    fn wrap_chan_if_needed(&self, pkt: Atom) -> Atom {
        if let Some(cid) = self.channel_id {
            Atom::new_parent(
                ID4::PCP_CHAN,
                vec![
                    Atom::new_leaf(ID4::CHAN_ID, Bytes::copy_from_slice(cid.as_bytes())),
                    pkt,
                ],
            )
        } else {
            pkt
        }
    }
}

/// Unrolls a 32-bit unsigned wire position into a monotonic 64-bit signed cursor.
///
/// Seamlessly handles forward and backward 32-bit counter rollover.
pub fn unwrap_wire_pos(wire_pos: u32, last_unwrapped: Option<i64>) -> i64 {
    let last = match last_unwrapped {
        Some(l) => l,
        None => return wire_pos as i64,
    };

    let last_wire = last as u32;
    let delta = (wire_pos as i64) - (last_wire as i64);

    let adjusted_delta = if delta < -0x8000_0000 {
        delta + 0x1_0000_0000
    } else if delta > 0x8000_0000 {
        delta - 0x1_0000_0000
    } else {
        delta
    };

    last + adjusted_delta
}

/// Result yielded when ingesting a packet into the cache adapter.
#[derive(Debug, PartialEq, Eq)]
pub enum PacketIngestResult {
    /// Ingested a stream header: epoch advanced, old body invalidated.
    HeaderReset { epoch: i32, pos: i64 },
    /// Ingested a body data chunk into the bounded cache.
    BodyPushed {
        continuity: PositionContinuity,
        pos: i64,
        len: usize,
    },
}

/// Ingest adapter connecting channel packets to the bounded content cache (REQ-026, REQ-051).
pub struct ChannelPacketAdapter<C: CacheClock = SystemCacheClock> {
    cache: BoundedContentCache<C>,
    current_epoch: i32,
    last_unwrapped_pos: Option<i64>,
    container_type: String,
    max_header_bytes: usize,
}

impl ChannelPacketAdapter<SystemCacheClock> {
    pub fn new(max_body_bytes: usize, max_body_duration: std::time::Duration) -> Self {
        Self::with_clock(
            max_body_bytes,
            max_body_duration,
            DEFAULT_MAX_HEADER_LEN,
            SystemCacheClock::new(),
        )
    }
}

impl<C: CacheClock> ChannelPacketAdapter<C> {
    pub fn with_clock(
        max_body_bytes: usize,
        max_body_duration: std::time::Duration,
        max_header_bytes: usize,
        clock: C,
    ) -> Self {
        Self {
            cache: BoundedContentCache::with_clock(
                max_body_bytes,
                max_body_duration,
                max_header_bytes,
                clock,
            ),
            current_epoch: 0,
            last_unwrapped_pos: None,
            container_type: "RAW".to_string(),
            max_header_bytes,
        }
    }

    pub fn current_epoch(&self) -> i32 {
        self.current_epoch
    }

    pub fn last_unwrapped_pos(&self) -> Option<i64> {
        self.last_unwrapped_pos
    }

    pub fn cache(&self) -> &BoundedContentCache<C> {
        &self.cache
    }

    pub fn cache_mut(&mut self) -> &mut BoundedContentCache<C> {
        &mut self.cache
    }

    /// Ingests a channel packet, managing epoch invalidation on new headers and pushing body data.
    pub fn ingest_packet(&mut self, pkt: ChannelPacket) -> Result<PacketIngestResult, PacketError> {
        self.last_unwrapped_pos = Some(pkt.pos);

        match pkt.payload {
            PacketPayload::Header(h) => {
                if h.len() > self.max_header_bytes {
                    return Err(PacketError::HeaderTooLarge {
                        actual: h.len(),
                        max: self.max_header_bytes,
                    });
                }

                self.current_epoch += 1;
                let snapshot = HeaderSnapshot::new(
                    self.current_epoch,
                    pkt.pos,
                    self.container_type.clone(),
                    h,
                );
                self.cache.set_header(snapshot)?;

                Ok(PacketIngestResult::HeaderReset {
                    epoch: self.current_epoch,
                    pos: pkt.pos,
                })
            }

            PacketPayload::Data(d) => {
                let len = d.len();
                let content = Content::new(
                    self.current_epoch,
                    std::time::Duration::ZERO,
                    pkt.pos,
                    d,
                    pkt.cont,
                );
                let continuity = self.cache.push_body(content, Some(len))?;

                Ok(PacketIngestResult::BodyPushed {
                    continuity,
                    pos: pkt.pos,
                    len,
                })
            }
        }
    }
}
