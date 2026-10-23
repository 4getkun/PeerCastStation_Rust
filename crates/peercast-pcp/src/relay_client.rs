use bytes::Bytes;
use std::net::IpAddr;
use thiserror::Error;

use peercast_core::atom::{Atom, AtomBudget, AtomError};
use peercast_core::channel_id::ChannelId;
use peercast_core::content::{Content, ContinuationFlags};
use peercast_core::errors::QuitReason;
use peercast_core::host::Host;
use peercast_core::id4::ID4;

use crate::http_prelude::{
    parse_relay_response, PcpHttpError, PcpHttpLimits, PcpHttpStatus, PcpRelayResponse,
};

/// Maximum candidate hosts collected during a 503 response sequence (Global Rule 3).
pub const MAX_CANDIDATE_HOSTS: usize = 64;

/// PCP Servant/Agent software version written in `helo`/`oleh` Atoms (1218, matching Station & YT).
pub const SERVANT_VERSION: u32 = 1218;

/// Exhaustive phases of the PCP HTTP relay client state machine (Global Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RelayClientPhase {
    #[default]
    Initial,
    WaitingResponse,
    WaitingOleh200,
    WaitingOleh503,
    WaitingOk,
    Streaming,
    CollectingHosts503,
    Terminal(QuitReason),
}

impl RelayClientPhase {
    #[inline]
    pub fn is_terminal(&self) -> bool {
        matches!(self, RelayClientPhase::Terminal(_))
    }
}

/// Errors produced during relay handshake or streaming.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum RelayError {
    #[error("HTTP prelude error: {0}")]
    Http(#[from] PcpHttpError),

    #[error("Atom codec error: {0}")]
    Atom(#[from] AtomError),

    #[error("Invalid ok Atom value: expected 1, got {0}")]
    InvalidOkValue(u32),

    #[error("Unexpected Atom tag in phase {phase:?}: {tag:?}")]
    UnexpectedAtom {
        phase: RelayClientPhase,
        tag: ID4,
    },

    #[error("Server returned quit: {0}")]
    ServerQuit(QuitReason),

    #[error("Channel is off-air (404 Not Found); Atom decoding forbidden (REQ-032)")]
    OffAir,
}

/// Semantic events yielded by the relay client state machine.
#[derive(Debug, PartialEq, Eq)]
pub enum RelayEvent {
    /// Received HTTP 200 OK prelude.
    Connected200,
    /// Received HTTP 503 Service Unavailable prelude.
    Unavailable503,
    /// Successfully exchanged helo/oleh handshake.
    HandshakeCompleted {
        remote_session_id: Option<ChannelId>,
        remote_ip: Option<IpAddr>,
        remote_port: Option<u16>,
    },
    /// Received a stream media or header chunk.
    MediaChunk(Content),
    /// Received channel metadata (`info` Atom).
    ChannelInfo(Atom),
    /// Received track metadata (`trck` Atom).
    TrackInfo(Atom),
    /// Received candidate peer host (`host` Atom).
    Host(Atom),
    /// Received broadcast control packet (`bcst` Atom).
    Broadcast(Atom),
    /// Peer initiated disconnect with quit code.
    Quit(QuitReason),
}

/// Protocol network family profile for relay negotiation (REQ-034).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RelayNetworkProfile {
    #[default]
    IPv4,
    IPv6,
}

impl RelayNetworkProfile {
    pub fn pcp_version(&self) -> u32 {
        match self {
            RelayNetworkProfile::IPv4 => 1,
            RelayNetworkProfile::IPv6 => 100,
        }
    }
}

/// PCP HTTP relay client state machine driving connection bootstrap and streaming (REQ-031..REQ-034).
pub struct RelayClientStateMachine {
    phase: RelayClientPhase,
    channel_id: ChannelId,
    session_id: ChannelId,
    network_profile: RelayNetworkProfile,
    cursor_pos: i64,
    http_limits: PcpHttpLimits,
    atom_budget: AtomBudget,
    candidate_hosts: Vec<Host>,
    remote_session_id: Option<ChannelId>,
    remote_ip: Option<IpAddr>,
    remote_port: Option<u16>,
    stream_epoch: i32,
}

impl RelayClientStateMachine {
    pub fn new(
        channel_id: ChannelId,
        session_id: ChannelId,
        network_profile: RelayNetworkProfile,
        cursor_pos: i64,
    ) -> Self {
        Self {
            phase: RelayClientPhase::Initial,
            channel_id,
            session_id,
            network_profile,
            cursor_pos,
            http_limits: PcpHttpLimits::default(),
            atom_budget: AtomBudget::default(),
            candidate_hosts: Vec::new(),
            remote_session_id: None,
            remote_ip: None,
            remote_port: None,
            stream_epoch: 1,
        }
    }

    pub fn phase(&self) -> RelayClientPhase {
        self.phase
    }

    /// Returns true if the client is currently processing a 503 Service Unavailable flow.
    pub fn is_503_mode(&self) -> bool {
        matches!(
            self.phase,
            RelayClientPhase::WaitingOleh503 | RelayClientPhase::CollectingHosts503
        )
    }

    /// Returns the candidate hosts collected during a 503 exchange.
    pub fn candidate_hosts(&self) -> &[Host] {
        &self.candidate_hosts
    }

    /// Takes and returns the collected candidate hosts, resetting the internal collection.
    pub fn take_candidate_hosts(&mut self) -> Vec<Host> {
        std::mem::take(&mut self.candidate_hosts)
    }

    pub fn remote_session_id(&self) -> Option<ChannelId> {
        self.remote_session_id
    }

    /// Formats the initial HTTP GET relay request (REQ-031).
    ///
    /// Never includes raw `pcnt` or redundant `pcp\n` binary preambles (REQ-031, REQ-035).
    pub fn build_relay_request(&mut self, host_header: &str, user_agent: &str) -> Vec<u8> {
        self.phase = RelayClientPhase::WaitingResponse;
        let mut s = format!(
            "GET /channel/{} HTTP/1.0\r\n\
             Host: {}\r\n\
             User-Agent: {}\r\n\
             x-peercast-pcp: {}\r\n",
            self.channel_id.to_hex_upper(),
            host_header,
            user_agent,
            self.network_profile.pcp_version()
        );
        if self.cursor_pos > 0 {
            s.push_str(&format!("x-peercast-pos: {}\r\n", self.cursor_pos));
        }
        s.push_str("\r\n");
        s.into_bytes()
    }

    /// Formats client `helo` Atom for handshake.
    pub fn build_helo(&self, agent: &str, port: Option<u16>) -> Result<Bytes, AtomError> {
        let mut children = vec![
            Atom::new_string(ID4::HELO_AGENT, agent),
            Atom::new_leaf(
                ID4::HELO_SESSIONID,
                Bytes::copy_from_slice(self.session_id.as_bytes()),
            ),
            Atom::new_u32_le(ID4::HELO_VERSION, SERVANT_VERSION),
        ];
        if let Some(p) = port {
            children.push(Atom::new_u16_le(ID4::HELO_PORT, p));
        }
        let helo = Atom::new_parent(ID4::PCP_HELO, children);
        helo.to_bytes()
    }

    /// Feeds incoming network bytes and advances the protocol state machine.
    ///
    /// Consumes only bytes belonging to current messages, cleanly leaving any following
    /// bytes intact in `buf` for subsequent reads.
    pub fn feed_bytes(&mut self, buf: &mut Bytes) -> Result<Option<RelayEvent>, RelayError> {
        match self.phase {
            RelayClientPhase::Initial => {
                self.phase = RelayClientPhase::WaitingResponse;
                self.feed_bytes(buf)
            }

            RelayClientPhase::WaitingResponse => {
                match parse_relay_response(buf, &self.http_limits) {
                    Ok(Some(resp)) => {
                        self.handle_http_response(resp)
                    }
                    Ok(None) => Ok(None),
                    Err(PcpHttpError::NotFoundOffAir) => {
                        self.phase = RelayClientPhase::Terminal(QuitReason::OFF_AIR);
                        Err(RelayError::OffAir)
                    }
                    Err(e) => {
                        self.phase = RelayClientPhase::Terminal(QuitReason::BAD_RESPONSE);
                        Err(RelayError::Http(e))
                    }
                }
            }

            RelayClientPhase::WaitingOleh200 => {
                let atom = match Atom::decode(buf, &self.atom_budget)? {
                    Some(a) => a,
                    None => return Ok(None),
                };

                if atom.name() == ID4::PCP_OLEH {
                    self.phase = RelayClientPhase::WaitingOk;
                    self.handle_oleh(atom)
                } else if atom.name() == ID4::PCP_QUIT {
                    let code = atom.get_u32_le().unwrap_or(QuitReason::BASE_QUIT);
                    let reason = QuitReason::from_raw(code);
                    self.phase = RelayClientPhase::Terminal(reason);
                    Ok(Some(RelayEvent::Quit(reason)))
                } else {
                    // Ignore unknown interleaved control packets during handshake
                    Ok(None)
                }
            }

            RelayClientPhase::WaitingOleh503 => {
                let atom = match Atom::decode(buf, &self.atom_budget)? {
                    Some(a) => a,
                    None => return Ok(None),
                };

                if atom.name() == ID4::PCP_OLEH {
                    self.phase = RelayClientPhase::CollectingHosts503;
                    self.handle_oleh(atom)
                } else if atom.name() == ID4::PCP_QUIT {
                    let code = atom.get_u32_le().unwrap_or(QuitReason::BASE_QUIT);
                    let reason = QuitReason::from_raw(code);
                    self.phase = RelayClientPhase::Terminal(reason);
                    Ok(Some(RelayEvent::Quit(reason)))
                } else {
                    // Ignore unknown interleaved control packets during handshake
                    Ok(None)
                }
            }

            RelayClientPhase::CollectingHosts503 => {
                let atom = match Atom::decode(buf, &self.atom_budget)? {
                    Some(a) => a,
                    None => return Ok(None),
                };

                if atom.name() == ID4::PCP_QUIT {
                    let code = atom.get_u32_le().unwrap_or(QuitReason::BASE_QUIT);
                    let reason = QuitReason::from_raw(code);
                    self.phase = RelayClientPhase::Terminal(reason);
                    Ok(Some(RelayEvent::Quit(reason)))
                } else if atom.name() == ID4::PCP_HOST {
                    // Validate candidate host (REQ-032, REQ-043)
                    if let Ok(host) = Host::from_atom(&atom) {
                        if host.is_valid_candidate(&self.session_id, None)
                            && self.candidate_hosts.len() < MAX_CANDIDATE_HOSTS
                        {
                            self.candidate_hosts.push(host);
                        }
                    }
                    Ok(Some(RelayEvent::Host(atom)))
                } else if atom.name() == ID4::PCP_BCST {
                    Ok(Some(RelayEvent::Broadcast(atom)))
                } else {
                    // Retain/ignore unhandled control atoms predictably
                    Ok(None)
                }
            }

            RelayClientPhase::WaitingOk => {
                let atom = match Atom::decode(buf, &self.atom_budget)? {
                    Some(a) => a,
                    None => return Ok(None),
                };

                if atom.name() == ID4::PCP_OK {
                    let val = atom.get_u32_le()?;
                    if val != 1 {
                        self.phase = RelayClientPhase::Terminal(QuitReason::BAD_RESPONSE);
                        return Err(RelayError::InvalidOkValue(val));
                    }
                    self.phase = RelayClientPhase::Streaming;
                    Ok(None) // Handshake ok verified; enter streaming
                } else if atom.name() == ID4::PCP_QUIT {
                    let code = atom.get_u32_le().unwrap_or(QuitReason::BASE_QUIT);
                    let reason = QuitReason::from_raw(code);
                    self.phase = RelayClientPhase::Terminal(reason);
                    Ok(Some(RelayEvent::Quit(reason)))
                } else {
                    // Interleaved control/chan before ok is permitted (REQ-032)
                    self.handle_streaming_atom(atom)
                }
            }

            RelayClientPhase::Streaming => {
                let atom = match Atom::decode(buf, &self.atom_budget)? {
                    Some(a) => a,
                    None => return Ok(None),
                };
                self.handle_streaming_atom(atom)
            }

            RelayClientPhase::Terminal(reason) => Ok(Some(RelayEvent::Quit(reason))),
        }
    }

    fn handle_http_response(&mut self, resp: PcpRelayResponse) -> Result<Option<RelayEvent>, RelayError> {
        match resp.status {
            PcpHttpStatus::Ok200 => {
                self.phase = RelayClientPhase::WaitingOleh200;
                Ok(Some(RelayEvent::Connected200))
            }
            PcpHttpStatus::Unavailable503 => {
                self.phase = RelayClientPhase::WaitingOleh503;
                Ok(Some(RelayEvent::Unavailable503))
            }
            PcpHttpStatus::NotFound404 => {
                self.phase = RelayClientPhase::Terminal(QuitReason::OFF_AIR);
                Err(RelayError::OffAir)
            }
            PcpHttpStatus::Other(_) => {
                self.phase = RelayClientPhase::Terminal(QuitReason::BAD_RESPONSE);
                Err(RelayError::Http(PcpHttpError::MalformedStatusLine(
                    resp.status.to_string(),
                )))
            }
        }
    }

    fn handle_oleh(&mut self, atom: Atom) -> Result<Option<RelayEvent>, RelayError> {
        let mut remote_sid = None;
        let mut remote_ip = None;
        let mut remote_port = None;

        if let Some(children) = atom.children() {
            for child in children {
                if child.name() == ID4::HELO_SESSIONID {
                    if let Some(bytes) = child.leaf_bytes() {
                        if bytes.len() == 16 {
                            let mut arr = [0u8; 16];
                            arr.copy_from_slice(bytes);
                            remote_sid = Some(ChannelId::from_bytes(arr));
                        }
                    }
                } else if child.name() == ID4::HELO_REMOTEIP {
                    remote_ip = child.get_ip().ok();
                } else if child.name() == ID4::HELO_PORT {
                    remote_port = child.get_u16_le().ok();
                }
            }
        }

        self.remote_session_id = remote_sid;
        self.remote_ip = remote_ip;
        self.remote_port = remote_port;

        Ok(Some(RelayEvent::HandshakeCompleted {
            remote_session_id: remote_sid,
            remote_ip,
            remote_port,
        }))
    }

    fn handle_streaming_atom(&mut self, atom: Atom) -> Result<Option<RelayEvent>, RelayError> {
        let tag = atom.name();
        if tag == ID4::PCP_QUIT {
            let code = atom.get_u32_le().unwrap_or(QuitReason::BASE_QUIT);
            let reason = QuitReason::from_raw(code);
            self.phase = RelayClientPhase::Terminal(reason);
            Ok(Some(RelayEvent::Quit(reason)))
        } else if tag == ID4::PCP_HOST {
            Ok(Some(RelayEvent::Host(atom)))
        } else if tag == ID4::PCP_BCST {
            Ok(Some(RelayEvent::Broadcast(atom)))
        } else if tag == ID4::CHAN_INFO {
            Ok(Some(RelayEvent::ChannelInfo(atom)))
        } else if tag == ID4::CHAN_TRCK {
            Ok(Some(RelayEvent::TrackInfo(atom)))
        } else if tag == ID4::PCP_CHAN {
            // Container of channel elements; parse packet or info
            if let Some(children) = atom.children() {
                for child in children {
                    if child.name() == ID4::CHAN_PKT {
                        return self.parse_pkt_atom(child);
                    }
                }
            }
            Ok(Some(RelayEvent::ChannelInfo(atom)))
        } else if tag == ID4::CHAN_PKT {
            self.parse_pkt_atom(&atom)
        } else {
            // Preserves unhandled / control Atoms cleanly (REQ-032)
            Ok(None)
        }
    }

    fn parse_pkt_atom(&mut self, pkt: &Atom) -> Result<Option<RelayEvent>, RelayError> {
        let mut pos = 0i64;
        let mut cont = ContinuationFlags::NONE;
        let mut data = Bytes::new();

        if let Some(children) = pkt.children() {
            for child in children {
                if child.name() == ID4::PKT_POS {
                    pos = child.get_u32_le().unwrap_or(0) as i64;
                } else if child.name() == ID4::PKT_CONT {
                    cont = ContinuationFlags(child.get_u8().unwrap_or(0));
                } else if child.name() == ID4::PKT_DATA || child.name() == ID4::PKT_HEAD {
                    if let Some(b) = child.leaf_bytes() {
                        data = b.clone();
                    }
                }
            }
        }

        let content = Content::new(
            self.stream_epoch,
            std::time::Duration::from_millis(0),
            pos,
            data,
            cont,
        );
        Ok(Some(RelayEvent::MediaChunk(content)))
    }
}
