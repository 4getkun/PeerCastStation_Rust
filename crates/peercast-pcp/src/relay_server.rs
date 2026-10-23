use bytes::Bytes;
use std::net::IpAddr;
use thiserror::Error;

use peercast_core::atom::{Atom, AtomBudget, AtomError};
use peercast_core::channel_id::ChannelId;
use peercast_core::errors::QuitReason;
use peercast_core::host::Host;
use peercast_core::id4::ID4;

use crate::http_prelude::{
    parse_relay_request, PcpHttpError, PcpHttpLimits, PcpRelayRequest,
};
use crate::relay_client::{RelayNetworkProfile, MAX_CANDIDATE_HOSTS, SERVANT_VERSION};

/// Minimum supported servant version in `helo` (matching Station PCPOutputStream.cs:503).
pub const MIN_HELO_VERSION: u32 = 1200;

/// Exhaustive phases of the PCP HTTP relay server state machine (Global Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RelayServerPhase {
    #[default]
    WaitingRequest,
    WaitingDecision,
    WaitingHelo200,
    WaitingHelo503,
    Streaming,
    Terminal(QuitReason),
}

impl RelayServerPhase {
    #[inline]
    pub fn is_terminal(&self) -> bool {
        matches!(self, RelayServerPhase::Terminal(_))
    }
}

/// Errors produced during server-side relay handshake.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum RelayServerError {
    #[error("HTTP prelude error: {0}")]
    Http(#[from] PcpHttpError),

    #[error("Atom codec error: {0}")]
    Atom(#[from] AtomError),

    #[error("Unexpected Atom tag in phase {phase:?}: {tag:?}")]
    UnexpectedAtom {
        phase: RelayServerPhase,
        tag: ID4,
    },

    #[error("Invalid server phase for operation: current is {0:?}")]
    InvalidPhase(RelayServerPhase),

    #[error("Missing client session ID in helo")]
    MissingSessionId,

    #[error("Missing x-peercast-pcp header in relay request (REQ-034)")]
    MissingPcpHeader,

    #[error("Network family mismatch: server is {server_profile:?}, remote IP is {remote_ip:?}, header version is {pcp_version:?}")]
    NetworkFamilyMismatch {
        server_profile: RelayNetworkProfile,
        remote_ip: Option<IpAddr>,
        pcp_version: Option<u32>,
    },

    #[error("Client servant version {0} is too old (minimum required is {MIN_HELO_VERSION})")]
    BadAgentVersion(u32),

    #[error("Client initiated quit: {0}")]
    ClientQuit(QuitReason),
}

/// Semantic events yielded by the relay server state machine.
#[derive(Debug, PartialEq, Eq)]
pub enum RelayServerEvent {
    /// Received HTTP GET relay request. Application must decide to accept or reject.
    RequestReceived(PcpRelayRequest),
    /// Handshake completed on 200 OK path. Outbound bytes contain oleh + ok(1).
    HandshakeCompleted {
        outbound_bytes: Bytes,
        client_session_id: ChannelId,
        client_agent: Option<String>,
        client_port: Option<u16>,
    },
    /// Handshake completed on 503 Unavailable path. Outbound bytes contain oleh + candidate hosts + quit(1003).
    FullHandshakeCompleted {
        outbound_bytes: Bytes,
        client_session_id: ChannelId,
    },
    /// Client initiated quit.
    Quit(QuitReason),
    /// Received control packet during streaming.
    Control(Atom),
}

/// PCP HTTP relay server state machine driving client request processing, handshake, and admission (REQ-032..REQ-034).
pub struct RelayServerStateMachine {
    phase: RelayServerPhase,
    session_id: ChannelId,
    network_profile: RelayNetworkProfile,
    http_limits: PcpHttpLimits,
    atom_budget: AtomBudget,
    remote_ip: Option<IpAddr>,
    server_agent: String,
    request: Option<PcpRelayRequest>,
    candidate_hosts: Vec<Host>,
    client_session_id: Option<ChannelId>,
    client_agent: Option<String>,
    client_port: Option<u16>,
    client_version: Option<u32>,
}

impl RelayServerStateMachine {
    pub fn new(
        session_id: ChannelId,
        network_profile: RelayNetworkProfile,
        remote_ip: Option<IpAddr>,
    ) -> Self {
        Self {
            phase: RelayServerPhase::WaitingRequest,
            session_id,
            network_profile,
            http_limits: PcpHttpLimits::default(),
            atom_budget: AtomBudget::default(),
            remote_ip,
            server_agent: "PeerCastStation/1.0".to_string(),
            request: None,
            candidate_hosts: Vec::new(),
            client_session_id: None,
            client_agent: None,
            client_port: None,
            client_version: None,
        }
    }

    pub fn with_server_agent(mut self, agent: impl Into<String>) -> Self {
        self.server_agent = agent.into();
        self
    }

    pub fn network_profile(&self) -> RelayNetworkProfile {
        self.network_profile
    }

    pub fn phase(&self) -> RelayServerPhase {
        self.phase
    }

    pub fn request(&self) -> Option<&PcpRelayRequest> {
        self.request.as_ref()
    }

    pub fn client_session_id(&self) -> Option<ChannelId> {
        self.client_session_id
    }

    pub fn client_agent(&self) -> Option<&str> {
        self.client_agent.as_deref()
    }

    pub fn client_port(&self) -> Option<u16> {
        self.client_port
    }

    pub fn client_version(&self) -> Option<u32> {
        self.client_version
    }

    /// Validates request header version against channel network profile and remote IP (REQ-034, Station parity `PCPOutputStream.cs:938-942`).
    ///
    /// - If `x-peercast-pcp` header is missing, returns `MissingPcpHeader` (maps to HTTP 400 Bad Request).
    /// - For `RelayNetworkProfile::IPv4`: requires `pcp_version == 1` and IPv4 address family.
    ///   IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) are unmapped to IPv4 and accepted.
    /// - For `RelayNetworkProfile::IPv6`: requires `pcp_version == 100` and native IPv6 address family.
    ///   IPv4-mapped IPv6 addresses are rejected as non-native IPv6.
    /// - If family or version mismatch occurs, returns `NetworkFamilyMismatch` (maps to HTTP 404 Not Found).
    pub fn validate_request_family(&self) -> Result<(), RelayServerError> {
        let req = match &self.request {
            Some(r) => r,
            None => return Err(RelayServerError::InvalidPhase(self.phase)),
        };

        let pcp_version = req.pcp_version.ok_or(RelayServerError::MissingPcpHeader)?;

        let unmapped_ip = self.remote_ip.map(|ip| match ip {
            IpAddr::V4(v4) => IpAddr::V4(v4),
            IpAddr::V6(v6) => {
                let octets = v6.octets();
                if octets[0..10] == [0; 10] && octets[10] == 0xff && octets[11] == 0xff {
                    IpAddr::V4(std::net::Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]))
                } else {
                    IpAddr::V6(v6)
                }
            }
        });

        match self.network_profile {
            RelayNetworkProfile::IPv4 => {
                if pcp_version != 1 {
                    return Err(RelayServerError::NetworkFamilyMismatch {
                        server_profile: self.network_profile,
                        remote_ip: self.remote_ip,
                        pcp_version: Some(pcp_version),
                    });
                }
                if let Some(ip) = unmapped_ip {
                    if !ip.is_ipv4() {
                        return Err(RelayServerError::NetworkFamilyMismatch {
                            server_profile: self.network_profile,
                            remote_ip: self.remote_ip,
                            pcp_version: Some(pcp_version),
                        });
                    }
                }
            }
            RelayNetworkProfile::IPv6 => {
                if pcp_version != 100 {
                    return Err(RelayServerError::NetworkFamilyMismatch {
                        server_profile: self.network_profile,
                        remote_ip: self.remote_ip,
                        pcp_version: Some(pcp_version),
                    });
                }
                if let Some(ip) = unmapped_ip {
                    if !ip.is_ipv6() {
                        return Err(RelayServerError::NetworkFamilyMismatch {
                            server_profile: self.network_profile,
                            remote_ip: self.remote_ip,
                            pcp_version: Some(pcp_version),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    /// Rejects the relay request as 400 Bad Request and terminates (REQ-034).
    pub fn reject_bad_request(&mut self, message: &str) -> Vec<u8> {
        self.phase = RelayServerPhase::Terminal(QuitReason::BAD_RESPONSE);
        format!(
            "HTTP/1.0 400 Bad Request\r\n\
             Server: {}\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {}",
            self.server_agent,
            message.len(),
            message
        )
        .into_bytes()
    }

    /// Accepts the relay request and transitions to `WaitingHelo200`.
    ///
    /// Generates standard HTTP 200 OK prelude bytes to send to client.
    pub fn accept(&mut self) -> Result<Vec<u8>, RelayServerError> {
        if self.phase != RelayServerPhase::WaitingDecision {
            return Err(RelayServerError::InvalidPhase(self.phase));
        }
        self.phase = RelayServerPhase::WaitingHelo200;
        let response = format!(
            "HTTP/1.0 200 OK\r\n\
             Server: {}\r\n\
             Content-Type: application/x-peercast-pcp\r\n\
             \r\n",
            self.server_agent
        );
        Ok(response.into_bytes())
    }

    /// Rejects the relay request as full (503 Service Unavailable) and transitions to `WaitingHelo503`.
    ///
    /// Generates standard HTTP 503 Service Unavailable prelude bytes to send to client.
    /// Candidate hosts are preserved (up to `MAX_CANDIDATE_HOSTS`) to be emitted after `helo/oleh`.
    pub fn reject_full(&mut self, mut hosts: Vec<Host>) -> Result<Vec<u8>, RelayServerError> {
        if self.phase != RelayServerPhase::WaitingDecision {
            return Err(RelayServerError::InvalidPhase(self.phase));
        }
        hosts.truncate(MAX_CANDIDATE_HOSTS);
        self.candidate_hosts = hosts;
        self.phase = RelayServerPhase::WaitingHelo503;
        let response = format!(
            "HTTP/1.0 503 Service Unavailable\r\n\
             Server: {}\r\n\
             \r\n",
            self.server_agent
        );
        Ok(response.into_bytes())
    }

    /// Rejects the relay request as not found / off-air (404 Not Found) and terminates (REQ-032).
    ///
    /// Atom decoding is strictly forbidden on this path.
    pub fn reject_not_found(&mut self) -> Vec<u8> {
        self.phase = RelayServerPhase::Terminal(QuitReason::OFF_AIR);
        format!(
            "HTTP/1.0 404 Not Found\r\n\
             Server: {}\r\n\
             \r\n",
            self.server_agent
        )
        .into_bytes()
    }

    /// Rejects with an arbitrary HTTP status code and terminates.
    pub fn reject_error(&mut self, code: u16, text: &str, reason: QuitReason) -> Vec<u8> {
        self.phase = RelayServerPhase::Terminal(reason);
        format!(
            "HTTP/1.0 {} {}\r\n\
             Server: {}\r\n\
             \r\n",
            code, text, self.server_agent
        )
        .into_bytes()
    }

    /// Feeds incoming network bytes from the client and advances the server state machine.
    ///
    /// Consumes only bytes belonging to current messages, cleanly leaving any following
    /// bytes intact in `buf` for subsequent reads.
    pub fn feed_bytes(&mut self, buf: &mut Bytes) -> Result<Option<RelayServerEvent>, RelayServerError> {
        match self.phase {
            RelayServerPhase::WaitingRequest => {
                match parse_relay_request(buf, &self.http_limits) {
                    Ok(Some(req)) => {
                        self.request = Some(req.clone());
                        self.phase = RelayServerPhase::WaitingDecision;
                        Ok(Some(RelayServerEvent::RequestReceived(req)))
                    }
                    Ok(None) => Ok(None),
                    Err(e) => {
                        self.phase = RelayServerPhase::Terminal(QuitReason::BAD_RESPONSE);
                        Err(RelayServerError::Http(e))
                    }
                }
            }

            RelayServerPhase::WaitingDecision => {
                // Cannot advance until application calls accept(), reject_full(), or reject_not_found()
                Ok(None)
            }

            RelayServerPhase::WaitingHelo200 => {
                let atom = match Atom::decode(buf, &self.atom_budget)? {
                    Some(a) => a,
                    None => return Ok(None),
                };

                if atom.name() == ID4::PCP_QUIT {
                    let code = atom.get_u32_le().unwrap_or(QuitReason::BASE_QUIT);
                    let reason = QuitReason::from_raw(code);
                    self.phase = RelayServerPhase::Terminal(reason);
                    return Ok(Some(RelayServerEvent::Quit(reason)));
                }

                if atom.name() != ID4::PCP_HELO {
                    self.phase = RelayServerPhase::Terminal(QuitReason::NOT_IDENTIFIED);
                    return Err(RelayServerError::UnexpectedAtom {
                        phase: self.phase,
                        tag: atom.name(),
                    });
                }

                self.parse_helo_fields(&atom)?;
                let client_sid = self.client_session_id.ok_or(RelayServerError::MissingSessionId)?;
                if self.client_version.unwrap_or(0) < MIN_HELO_VERSION {
                    self.phase = RelayServerPhase::Terminal(QuitReason::BAD_AGENT);
                    return Err(RelayServerError::BadAgentVersion(self.client_version.unwrap_or(0)));
                }

                // Build outbound: oleh + ok(1) (REQ-032)
                let outbound_bytes = self.build_oleh_and_ok()?;
                self.phase = RelayServerPhase::Streaming;

                Ok(Some(RelayServerEvent::HandshakeCompleted {
                    outbound_bytes,
                    client_session_id: client_sid,
                    client_agent: self.client_agent.clone(),
                    client_port: self.client_port,
                }))
            }

            RelayServerPhase::WaitingHelo503 => {
                let atom = match Atom::decode(buf, &self.atom_budget)? {
                    Some(a) => a,
                    None => return Ok(None),
                };

                if atom.name() == ID4::PCP_QUIT {
                    let code = atom.get_u32_le().unwrap_or(QuitReason::BASE_QUIT);
                    let reason = QuitReason::from_raw(code);
                    self.phase = RelayServerPhase::Terminal(reason);
                    return Ok(Some(RelayServerEvent::Quit(reason)));
                }

                if atom.name() != ID4::PCP_HELO {
                    self.phase = RelayServerPhase::Terminal(QuitReason::NOT_IDENTIFIED);
                    return Err(RelayServerError::UnexpectedAtom {
                        phase: self.phase,
                        tag: atom.name(),
                    });
                }

                self.parse_helo_fields(&atom)?;
                let client_sid = self.client_session_id.ok_or(RelayServerError::MissingSessionId)?;
                if self.client_version.unwrap_or(0) < MIN_HELO_VERSION {
                    self.phase = RelayServerPhase::Terminal(QuitReason::BAD_AGENT);
                    return Err(RelayServerError::BadAgentVersion(self.client_version.unwrap_or(0)));
                }

                // Build outbound: oleh + host candidates + quit(1003) (REQ-032, REQ-033)
                let outbound_bytes = self.build_oleh_hosts_and_quit()?;
                self.phase = RelayServerPhase::Terminal(QuitReason::UNAVAILABLE);

                Ok(Some(RelayServerEvent::FullHandshakeCompleted {
                    outbound_bytes,
                    client_session_id: client_sid,
                }))
            }

            RelayServerPhase::Streaming => {
                let atom = match Atom::decode(buf, &self.atom_budget)? {
                    Some(a) => a,
                    None => return Ok(None),
                };

                if atom.name() == ID4::PCP_QUIT {
                    let code = atom.get_u32_le().unwrap_or(QuitReason::BASE_QUIT);
                    let reason = QuitReason::from_raw(code);
                    self.phase = RelayServerPhase::Terminal(reason);
                    Ok(Some(RelayServerEvent::Quit(reason)))
                } else {
                    Ok(Some(RelayServerEvent::Control(atom)))
                }
            }

            RelayServerPhase::Terminal(reason) => Ok(Some(RelayServerEvent::Quit(reason))),
        }
    }

    fn parse_helo_fields(&mut self, atom: &Atom) -> Result<(), RelayServerError> {
        if let Some(children) = atom.children() {
            for child in children {
                match child.name() {
                    ID4::HELO_SESSIONID => {
                        if let Some(bytes) = child.leaf_bytes() {
                            if bytes.len() == 16 {
                                let mut arr = [0u8; 16];
                                arr.copy_from_slice(bytes);
                                self.client_session_id = Some(ChannelId::from_bytes(arr));
                            }
                        }
                    }
                    ID4::HELO_AGENT => {
                        if let Ok(s) = child.get_string() {
                            self.client_agent = Some(s.to_string());
                        }
                    }
                    ID4::HELO_PORT => {
                        if let Ok(p) = child.get_u16_le() {
                            self.client_port = Some(p);
                        } else if let Ok(p) = child.get_u32_le() {
                            if p <= 65535 {
                                self.client_port = Some(p as u16);
                            }
                        }
                    }
                    ID4::HELO_VERSION => {
                        if let Ok(v) = child.get_u32_le() {
                            self.client_version = Some(v);
                        }
                    }
                    _ => {
                        // Preserve / ignore unknown helo extensions safely
                    }
                }
            }
        }
        Ok(())
    }

    fn build_oleh_and_ok(&self) -> Result<Bytes, RelayServerError> {
        let mut oleh_children = vec![
            Atom::new_string(ID4::HELO_AGENT, &self.server_agent),
            Atom::new_leaf(
                ID4::HELO_SESSIONID,
                Bytes::copy_from_slice(self.session_id.as_bytes()),
            ),
            Atom::new_u32_le(ID4::HELO_VERSION, SERVANT_VERSION),
        ];
        if let Some(ip) = self.remote_ip {
            oleh_children.push(Atom::new_ip(ID4::HELO_REMOTEIP, ip));
        }
        if let Some(port) = self.client_port {
            oleh_children.push(Atom::new_u16_le(ID4::HELO_PORT, port));
        }
        let oleh = Atom::new_parent(ID4::PCP_OLEH, oleh_children);
        let ok = Atom::new_u32_le(ID4::PCP_OK, 1);

        let mut out = Vec::new();
        oleh.encode(&mut out)?;
        ok.encode(&mut out)?;
        Ok(Bytes::from(out))
    }

    fn build_oleh_hosts_and_quit(&self) -> Result<Bytes, RelayServerError> {
        let mut oleh_children = vec![
            Atom::new_string(ID4::HELO_AGENT, &self.server_agent),
            Atom::new_leaf(
                ID4::HELO_SESSIONID,
                Bytes::copy_from_slice(self.session_id.as_bytes()),
            ),
            Atom::new_u32_le(ID4::HELO_VERSION, SERVANT_VERSION),
        ];
        if let Some(ip) = self.remote_ip {
            oleh_children.push(Atom::new_ip(ID4::HELO_REMOTEIP, ip));
        }
        let oleh = Atom::new_parent(ID4::PCP_OLEH, oleh_children);

        let mut out = Vec::new();
        oleh.encode(&mut out)?;

        for host in &self.candidate_hosts {
            host.to_atom().encode(&mut out)?;
        }

        let quit = Atom::new_u32_le(ID4::PCP_QUIT, QuitReason::UNAVAILABLE.raw());
        quit.encode(&mut out)?;

        Ok(Bytes::from(out))
    }
}
