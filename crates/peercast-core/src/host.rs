use bytes::Bytes;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use thiserror::Error;

use crate::atom::{Atom, AtomError};
use crate::channel_id::ChannelId;
use crate::id4::ID4;

/// Errors produced during host Atom parsing or candidate validation.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum HostError {
    #[error("Not a host Atom (tag was {0:?})")]
    NotAHost(ID4),
    #[error("Atom decode error: {0}")]
    Atom(#[from] AtomError),
    #[error("Missing or invalid IP address")]
    InvalidIp,
    #[error("Missing or invalid port: {0}")]
    InvalidPort(u16),
}

/// Bitflags representing host capabilities and status (`flg1` tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostFlags1(pub u8);

impl HostFlags1 {
    pub const TRACKER: HostFlags1 = HostFlags1(0x01);
    pub const RELAY: HostFlags1 = HostFlags1(0x02);
    pub const DIRECT: HostFlags1 = HostFlags1(0x04);
    pub const FIREWALLED: HostFlags1 = HostFlags1(0x08);
    pub const PUSH: HostFlags1 = HostFlags1(0x08);
    pub const RECV: HostFlags1 = HostFlags1(0x10);
    pub const CIN: HostFlags1 = HostFlags1(0x20);
    pub const PRIVATE: HostFlags1 = HostFlags1(0x40);

    #[inline]
    pub fn is_tracker(&self) -> bool {
        (self.0 & Self::TRACKER.0) != 0
    }

    #[inline]
    pub fn is_relay(&self) -> bool {
        (self.0 & Self::RELAY.0) != 0
    }

    #[inline]
    pub fn is_direct(&self) -> bool {
        (self.0 & Self::DIRECT.0) != 0
    }

    #[inline]
    pub fn is_firewalled(&self) -> bool {
        (self.0 & Self::FIREWALLED.0) != 0
    }

    #[inline]
    pub fn is_receiving(&self) -> bool {
        (self.0 & Self::RECV.0) != 0
    }

    #[inline]
    pub fn is_control_in(&self) -> bool {
        (self.0 & Self::CIN.0) != 0
    }

    #[inline]
    pub fn is_private(&self) -> bool {
        (self.0 & Self::PRIVATE.0) != 0
    }

    #[inline]
    pub fn raw(&self) -> u8 {
        self.0
    }
}

/// Represents a peer host node in the channel distribution tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// Peer session ID.
    pub session_id: ChannelId,
    /// Channel ID this host belongs to.
    pub channel_id: ChannelId,
    /// Broadcast ID (if known).
    pub broadcast_id: Option<ChannelId>,
    /// Local address (advertised by peer).
    pub local_endpoint: Option<SocketAddr>,
    /// Observed global address.
    pub global_endpoint: Option<SocketAddr>,
    /// Capability and status flags.
    pub flags: HostFlags1,
    /// Number of active downstream relay connections.
    pub relay_count: u32,
    /// Number of active downstream direct play connections.
    pub direct_count: u32,
    /// Host uptime.
    pub uptime: Duration,
    /// Protocol version.
    pub version: u32,
    /// Number of upstream hops to the broadcast root.
    pub hops: u32,
    /// Local monotonic time when this host entry was created or refreshed.
    pub last_seen: Instant,
}

impl Host {
    pub fn new(session_id: ChannelId, channel_id: ChannelId) -> Self {
        Self {
            session_id,
            channel_id,
            broadcast_id: None,
            local_endpoint: None,
            global_endpoint: None,
            flags: HostFlags1::default(),
            relay_count: 0,
            direct_count: 0,
            uptime: Duration::ZERO,
            version: 0,
            hops: 0,
            last_seen: Instant::now(),
        }
    }

    /// Builder to set broadcast ID.
    pub fn with_broadcast_id(mut self, bcid: ChannelId) -> Self {
        self.broadcast_id = Some(bcid);
        self
    }

    /// Returns the effective contact socket address (preferring global, falling back to local).
    pub fn contact_address(&self) -> Option<SocketAddr> {
        self.global_endpoint.or(self.local_endpoint)
    }

    /// Age since last announcement/refresh using monotonic clock (REQ-042).
    pub fn age(&self) -> Duration {
        self.last_seen.elapsed()
    }

    /// Parses a Host descriptor from a decoded `host` Atom (REQ-021, REQ-032).
    pub fn from_atom(atom: &Atom) -> Result<Self, HostError> {
        if atom.name() != ID4::PCP_HOST {
            return Err(HostError::NotAHost(atom.name()));
        }

        let mut session_id = ChannelId::from_bytes([0u8; 16]);
        let mut channel_id = ChannelId::from_bytes([0u8; 16]);
        let mut ips = Vec::with_capacity(2);
        let mut ports = Vec::with_capacity(2);
        let mut flags = HostFlags1::default();
        let mut direct_count = 0u32;
        let mut relay_count = 0u32;
        let mut uptime = Duration::ZERO;
        let mut version = 0u32;
        let mut hops = 0u32;

        if let Some(children) = atom.children() {
            for child in children {
                match child.name() {
                    ID4::HOST_ID | ID4::HELO_SESSIONID => {
                        if let Some(bytes) = child.leaf_bytes() {
                            if bytes.len() == 16 {
                                let mut arr = [0u8; 16];
                                arr.copy_from_slice(bytes);
                                session_id = ChannelId::from_bytes(arr);
                            }
                        }
                    }
                    ID4::HOST_CHANID => {
                        if let Some(bytes) = child.leaf_bytes() {
                            if bytes.len() == 16 {
                                let mut arr = [0u8; 16];
                                arr.copy_from_slice(bytes);
                                channel_id = ChannelId::from_bytes(arr);
                            }
                        }
                    }
                    ID4::HOST_IP => {
                        if let Ok(addr) = child.get_ip() {
                            if ips.len() < 2 {
                                ips.push(addr);
                            }
                        }
                    }
                    ID4::HOST_PORT => {
                        if let Ok(p) = child.get_u16_le() {
                            if ports.len() < 2 {
                                ports.push(p);
                            }
                        } else if let Ok(p) = child.get_u32_le() {
                            if p <= 65535 && ports.len() < 2 {
                                ports.push(p as u16);
                            }
                        }
                    }
                    ID4::HOST_FLAGS1 => {
                        if let Ok(f) = child.get_u8() {
                            flags = HostFlags1(f);
                        }
                    }
                    ID4::HOST_NUML => {
                        if let Ok(n) = child.get_u32_le() {
                            direct_count = n;
                        } else if let Ok(n) = child.get_u16_le() {
                            direct_count = n as u32;
                        }
                    }
                    ID4::HOST_NUMR => {
                        if let Ok(n) = child.get_u32_le() {
                            relay_count = n;
                        } else if let Ok(n) = child.get_u16_le() {
                            relay_count = n as u32;
                        }
                    }
                    ID4::HOST_UPTIME => {
                        if let Ok(u) = child.get_u32_le() {
                            uptime = Duration::from_secs(u as u64);
                        }
                    }
                    ID4::HOST_VERSION => {
                        if let Ok(v) = child.get_u32_le() {
                            version = v;
                        }
                    }
                    ID4::HOST_UPHP => {
                        if let Ok(h) = child.get_u32_le() {
                            hops = h;
                        }
                    }
                    _ => {
                        // Preserve / ignore unrecognized host fields
                    }
                }
            }
        }

        // Repeated IP/port fields describe the global endpoint first, then the local endpoint.
        let ip = ips.first().copied().ok_or(HostError::InvalidIp)?;
        let port = ports.first().copied().ok_or(HostError::InvalidPort(0))?;
        if port == 0 {
            return Err(HostError::InvalidPort(0));
        }
        let local_endpoint = ips
            .get(1)
            .zip(ports.get(1))
            .filter(|(ip, port)| !ip.is_unspecified() && **port != 0)
            .map(|(ip, port)| SocketAddr::new(*ip, *port));

        Ok(Self {
            session_id,
            channel_id,
            broadcast_id: None,
            local_endpoint,
            global_endpoint: Some(SocketAddr::new(ip, port)),
            flags,
            relay_count,
            direct_count,
            uptime,
            version,
            hops,
            last_seen: Instant::now(),
        })
    }

    /// Serializes this Host into a wire `host` Atom (REQ-021, REQ-032).
    pub fn to_atom(&self) -> Atom {
        let mut children = vec![
            Atom::new_leaf(
                ID4::HOST_ID,
                Bytes::copy_from_slice(self.session_id.as_bytes()),
            ),
            Atom::new_leaf(
                ID4::HOST_CHANID,
                Bytes::copy_from_slice(self.channel_id.as_bytes()),
            ),
        ];
        if let Some(ep) = self.contact_address() {
            children.push(Atom::new_ip(ID4::HOST_IP, ep.ip()));
            children.push(Atom::new_u16_le(ID4::HOST_PORT, ep.port()));
        }
        children.push(Atom::new_u8(ID4::HOST_FLAGS1, self.flags.raw()));
        children.push(Atom::new_u32_le(ID4::HOST_NUML, self.direct_count));
        children.push(Atom::new_u32_le(ID4::HOST_NUMR, self.relay_count));
        children.push(Atom::new_u32_le(
            ID4::HOST_UPTIME,
            self.uptime.as_secs() as u32,
        ));
        children.push(Atom::new_u32_le(ID4::HOST_VERSION, self.version));
        children.push(Atom::new_u32_le(ID4::HOST_UPHP, self.hops));
        Atom::new_parent(ID4::PCP_HOST, children)
    }

    /// Validates whether this host is an eligible candidate peer (REQ-032, REQ-043).
    ///
    /// Rejects:
    /// - Missing contact address
    /// - Port 0
    /// - Unspecified IP addresses (`0.0.0.0` or `::`)
    /// - Matching local session ID (self-loop prevention)
    /// - Matching local endpoint address
    pub fn is_valid_candidate(
        &self,
        self_session_id: &ChannelId,
        self_endpoint: Option<&SocketAddr>,
    ) -> bool {
        let ep = match self.contact_address() {
            Some(ep) => ep,
            None => return false,
        };
        if ep.port() == 0 {
            return false;
        }
        if ep.ip().is_unspecified() {
            return false;
        }
        if &self.session_id == self_session_id && !self.session_id.is_zero() {
            return false;
        }
        if let Some(self_ep) = self_endpoint {
            if &ep == self_ep {
                return false;
            }
        }
        true
    }
}
