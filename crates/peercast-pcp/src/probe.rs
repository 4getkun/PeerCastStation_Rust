use bytes::Bytes;
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Semaphore};

use peercast_core::atom::{Atom, AtomBudget, AtomError};
use peercast_core::channel_id::ChannelId;
use peercast_core::errors::QuitReason;
use peercast_core::id4::ID4;

/// Peer compatibility profile for reverse reachability probes (REQ-034, REQ-036).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProbePeerProfile {
    /// PeerCastStation compatibility: writes PCP version 1 even on IPv6 (Station `PingHostAsync` parity).
    #[default]
    PeerCastStation,
    /// PeerCast-YT compatibility: writes PCP version 1 for IPv4 / IPv4-mapped, and 100 for native IPv6 (YT `servent.cpp:494-498` parity).
    PeerCastYt,
}

/// Result of an executed reverse reachability probe (REQ-036).
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ProbeResult {
    /// Peer is reachable and returned matching session ID.
    Reachable {
        session_id: ChannelId,
        target_addr: SocketAddr,
    },
    /// Peer answered probe, but returned a mismatched session ID.
    SessionMismatch {
        expected: ChannelId,
        actual: ChannelId,
        target_addr: SocketAddr,
    },
    /// Connection refused, network error, or deadline exceeded.
    Unreachable {
        target_addr: SocketAddr,
        reason: String,
    },
}

/// Errors produced during reverse probe configuration or execution.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum ProbeError {
    #[error(
        "Probe target IP ({target}) does not match observed peer IP ({observed}) (SSRF prevention)"
    )]
    ForbiddenTarget { target: IpAddr, observed: IpAddr },

    #[error("Probe port 0 is invalid")]
    InvalidPort,

    #[error("Global or per-peer probe concurrency limit reached")]
    ConcurrencyLimitExceeded,

    #[error("Atom codec error during probe: {0}")]
    Atom(#[from] AtomError),

    #[error("Network I/O error during probe: {0}")]
    Io(String),

    #[error("Probe timed out after {0:?}")]
    Timeout(Duration),

    #[error("Probe protocol error: {0}")]
    Protocol(String),

    #[error("IPv6 probe is currently marked pending / open (REQ-036)")]
    IPv6Pending,
}

/// Configuration limits for reverse reachability probes (REQ-037).
#[derive(Debug, Clone)]
pub struct ProbeLimits {
    /// Maximum concurrent probes allowed globally across the servent (default: 16).
    pub max_concurrent_global: usize,
    /// Timeout for establishing TCP connection to peer (default: 5s).
    pub connect_timeout: Duration,
    /// Total deadline for probe handshake exchange (default: 10s).
    pub exchange_timeout: Duration,
    /// Whether IPv6 reverse probes are enabled (default: false, preserves REQ-036 pending status until explicitly enabled).
    pub enable_ipv6: bool,
}

impl Default for ProbeLimits {
    fn default() -> Self {
        Self {
            max_concurrent_global: 16,
            connect_timeout: Duration::from_secs(5),
            exchange_timeout: Duration::from_secs(10),
            enable_ipv6: false,
        }
    }
}

/// RAII concurrency guard for an active reverse probe.
///
/// Automatically removes the IP from the active set and returns the global semaphore permit upon drop.
pub struct ProbeGuard {
    active_ips: Arc<Mutex<HashSet<IpAddr>>>,
    ip: IpAddr,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl std::fmt::Debug for ProbeGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeGuard").field("ip", &self.ip).finish()
    }
}

impl Drop for ProbeGuard {
    fn drop(&mut self) {
        // Remove from active IP set synchronously
        if let Ok(mut set) = self.active_ips.try_lock() {
            set.remove(&self.ip);
        } else {
            // Fallback for contended lock in async context
            let set_clone = Arc::clone(&self.active_ips);
            let ip = self.ip;
            tokio::spawn(async move {
                let mut set = set_clone.lock().await;
                set.remove(&ip);
            });
        }
    }
}

/// Concurrency and security limiter for reverse reachability probes.
pub struct ProbeLimiter {
    limits: ProbeLimits,
    global_semaphore: Arc<Semaphore>,
    active_ips: Arc<Mutex<HashSet<IpAddr>>>,
}

impl ProbeLimiter {
    pub fn new(limits: ProbeLimits) -> Self {
        let max_global = limits.max_concurrent_global.max(1);
        Self {
            limits,
            global_semaphore: Arc::new(Semaphore::new(max_global)),
            active_ips: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Acquires a probe permit for `target_ip`, validating strict equality with `observed_ip`.
    ///
    /// Forbids probing arbitrary addresses (REQ-037).
    pub async fn acquire(
        &self,
        target_ip: IpAddr,
        observed_ip: IpAddr,
    ) -> Result<ProbeGuard, ProbeError> {
        if target_ip != observed_ip {
            return Err(ProbeError::ForbiddenTarget {
                target: target_ip,
                observed: observed_ip,
            });
        }

        // Try acquire global permit without blocking indefinitely
        let permit = match self.global_semaphore.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => return Err(ProbeError::ConcurrencyLimitExceeded),
        };

        // Try register per-peer IP (prevent multiple simultaneous probes to the same host)
        let mut set = self.active_ips.lock().await;
        if !set.insert(target_ip) {
            return Err(ProbeError::ConcurrencyLimitExceeded);
        }

        Ok(ProbeGuard {
            active_ips: Arc::clone(&self.active_ips),
            ip: target_ip,
            _permit: permit,
        })
    }

    pub fn limits(&self) -> &ProbeLimits {
        &self.limits
    }
}

/// Low-level binary codec for reverse probe wire messages.
pub struct ProbeCodec;

impl ProbeCodec {
    /// Formats the probe connection payload: `pcp\n` preamble Atom + `helo` Atom with version 1 (REQ-035, REQ-036).
    pub fn build_probe_init(
        server_session_id: ChannelId,
        server_agent: &str,
    ) -> Result<Bytes, AtomError> {
        Self::build_probe_init_with_version(1, server_session_id, server_agent)
    }

    /// Formats the probe connection payload with explicit protocol version (REQ-034, REQ-036).
    ///
    /// IPv4/Station uses version 1; native IPv6 on PeerCast-YT uses version 100.
    pub fn build_probe_init_with_version(
        version: u32,
        server_session_id: ChannelId,
        server_agent: &str,
    ) -> Result<Bytes, AtomError> {
        let mut out = Vec::new();

        // 1. Raw pcp\n preamble Atom containing protocol version
        let pcp_atom = Atom::new_u32_le(ID4::PCP_CONNECT, version);
        pcp_atom.encode(&mut out)?;

        // 2. Helo Atom
        let helo = Atom::new_parent(
            ID4::PCP_HELO,
            vec![
                Atom::new_string(ID4::HELO_AGENT, server_agent),
                Atom::new_leaf(
                    ID4::HELO_SESSIONID,
                    Bytes::copy_from_slice(server_session_id.as_bytes()),
                ),
                Atom::new_u32_le(ID4::HELO_VERSION, crate::relay_client::SERVANT_VERSION),
            ],
        );
        helo.encode(&mut out)?;

        Ok(Bytes::from(out))
    }

    /// Parses inbound raw `pcp\n` connect preamble Atom, returning the protocol version (1 or 100).
    pub fn parse_probe_preamble(
        buf: &mut Bytes,
        budget: &AtomBudget,
    ) -> Result<Option<u32>, ProbeError> {
        let atom = match Atom::decode(buf, budget)? {
            Some(a) => a,
            None => return Ok(None),
        };
        if atom.name() != ID4::PCP_CONNECT {
            return Err(ProbeError::Protocol(format!(
                "Expected pcp\\n preamble, got {:?}",
                atom.name()
            )));
        }
        let ver = atom
            .get_u32_le()
            .map_err(|_| ProbeError::Protocol("pcp\\n atom missing u32 version payload".into()))?;
        Ok(Some(ver))
    }

    /// Validates inbound probe preamble version according to peer profile (REQ-034, REQ-036).
    ///
    /// - Station pong listener (`PCPPongOutputStreamFactory.cs:64`) strictly checks for version 1.
    /// - YT pong listener (`PCPStream::readVersion`) accepts both version 1 and 100.
    pub fn validate_pong_preamble(ver: u32, profile: ProbePeerProfile) -> Result<(), ProbeError> {
        match profile {
            ProbePeerProfile::PeerCastStation => {
                if ver != 1 {
                    return Err(ProbeError::Protocol(format!(
                        "Station pong rejects non-1 PCP version: {}",
                        ver
                    )));
                }
                Ok(())
            }
            ProbePeerProfile::PeerCastYt => {
                if ver != 1 && ver != 100 {
                    return Err(ProbeError::Protocol(format!(
                        "YT pong rejects unrecognized PCP version: {}",
                        ver
                    )));
                }
                Ok(())
            }
        }
    }

    /// Formats the pong response: `oleh` Atom containing server session ID.
    pub fn build_pong_oleh(
        server_session_id: ChannelId,
        server_agent: &str,
    ) -> Result<Bytes, AtomError> {
        let mut out = Vec::new();
        let oleh = Atom::new_parent(
            ID4::PCP_OLEH,
            vec![
                Atom::new_string(ID4::HELO_AGENT, server_agent),
                Atom::new_leaf(
                    ID4::HELO_SESSIONID,
                    Bytes::copy_from_slice(server_session_id.as_bytes()),
                ),
            ],
        );
        oleh.encode(&mut out)?;
        Ok(Bytes::from(out))
    }

    /// Formats the closing `quit(1000)` Atom (QuitReason::NORMAL).
    pub fn build_probe_quit() -> Result<Bytes, AtomError> {
        let quit = Atom::new_u32_le(ID4::PCP_QUIT, QuitReason::NORMAL.raw());
        quit.to_bytes()
    }

    /// Parses inbound probe response from peer, expecting `oleh` Atom.
    pub fn parse_probe_oleh(
        buf: &mut Bytes,
        budget: &AtomBudget,
    ) -> Result<Option<ChannelId>, ProbeError> {
        let atom = match Atom::decode(buf, budget)? {
            Some(a) => a,
            None => return Ok(None),
        };

        if atom.name() != ID4::PCP_OLEH {
            return Err(ProbeError::Protocol(format!(
                "Expected oleh Atom, received {:?}",
                atom.name()
            )));
        }

        let mut sid = None;
        if let Some(children) = atom.children() {
            for child in children {
                if child.name() == ID4::HELO_SESSIONID {
                    if let Some(bytes) = child.leaf_bytes() {
                        if bytes.len() == 16 {
                            let mut arr = [0u8; 16];
                            arr.copy_from_slice(bytes);
                            sid = Some(ChannelId::from_bytes(arr));
                        }
                    }
                }
            }
        }

        Ok(sid)
    }
}

/// Executes a reverse reachability probe against an announced peer port (REQ-036).
///
/// Uses the default peer compatibility profile (`ProbePeerProfile::PeerCastStation`).
pub async fn execute_reverse_probe(
    limiter: &ProbeLimiter,
    observed_peer_ip: IpAddr,
    announced_port: u16,
    expected_session_id: ChannelId,
    server_session_id: ChannelId,
    server_agent: &str,
) -> Result<ProbeResult, ProbeError> {
    execute_reverse_probe_with_profile(
        limiter,
        observed_peer_ip,
        announced_port,
        expected_session_id,
        server_session_id,
        server_agent,
        ProbePeerProfile::default(),
    )
    .await
}

/// Executes a reverse reachability probe against an announced peer port with explicit peer profile (REQ-034, REQ-036).
///
/// Ensures:
/// - Connects strictly to `SocketAddr::new(observed_peer_ip, announced_port)`.
/// - Precedes oleh in server handler (REQ-032, A10).
/// - Emits PCP version 1 for IPv4 / IPv4-mapped, or version according to peer profile for native IPv6.
/// - Enforces connect and total deadlines (REQ-037).
/// - Cleanly closes with quit(1000).
pub async fn execute_reverse_probe_with_profile(
    limiter: &ProbeLimiter,
    observed_peer_ip: IpAddr,
    announced_port: u16,
    expected_session_id: ChannelId,
    server_session_id: ChannelId,
    server_agent: &str,
    peer_profile: ProbePeerProfile,
) -> Result<ProbeResult, ProbeError> {
    if announced_port == 0 {
        return Err(ProbeError::InvalidPort);
    }

    let limits = limiter.limits();

    // Check if IPv6 probe is enabled on limiter (REQ-036)
    if observed_peer_ip.is_ipv6() && !limits.enable_ipv6 {
        return Err(ProbeError::IPv6Pending);
    }

    let target_addr = SocketAddr::new(observed_peer_ip, announced_port);

    // Acquire concurrency and security permit (guards against SSRF to arbitrary IPs)
    let _guard = limiter.acquire(observed_peer_ip, observed_peer_ip).await?;

    let is_ipv4_or_mapped = match observed_peer_ip {
        IpAddr::V4(_) => true,
        IpAddr::V6(v6) => {
            let octets = v6.octets();
            octets[0..10] == [0; 10] && octets[10] == 0xff && octets[11] == 0xff
        }
    };

    let pcp_version = if is_ipv4_or_mapped {
        1
    } else {
        match peer_profile {
            ProbePeerProfile::PeerCastStation => 1,
            ProbePeerProfile::PeerCastYt => 100,
        }
    };

    // 1. Establish TCP connection with connect deadline
    let connect_fut = TcpStream::connect(target_addr);
    let mut stream = match tokio::time::timeout(limits.connect_timeout, connect_fut).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return Ok(ProbeResult::Unreachable {
                target_addr,
                reason: format!("Connection failed: {}", e),
            });
        }
        Err(_) => {
            return Ok(ProbeResult::Unreachable {
                target_addr,
                reason: "Connect timeout exceeded".to_string(),
            });
        }
    };

    // 2. Perform probe exchange within total exchange deadline
    let exchange_fut = async {
        // Send pcp\n with selected version + helo
        let init_bytes = ProbeCodec::build_probe_init_with_version(
            pcp_version,
            server_session_id,
            server_agent,
        )?;
        stream
            .write_all(&init_bytes)
            .await
            .map_err(|e| ProbeError::Io(e.to_string()))?;

        // Read response bytes until oleh Atom is complete
        let mut in_buf = bytes::BytesMut::with_capacity(1024);
        let mut read_arr = [0u8; 512];
        let budget = AtomBudget::default();

        let returned_sid = loop {
            let n = stream
                .read(&mut read_arr)
                .await
                .map_err(|e| ProbeError::Io(e.to_string()))?;
            if n == 0 {
                return Err(ProbeError::Protocol(
                    "Connection closed before oleh".to_string(),
                ));
            }
            in_buf.extend_from_slice(&read_arr[..n]);

            let mut freeze_buf = in_buf.clone().freeze();
            if let Some(sid) = ProbeCodec::parse_probe_oleh(&mut freeze_buf, &budget)? {
                break sid;
            }
        };

        // Send quit(1000) and flush
        let quit_bytes = ProbeCodec::build_probe_quit()?;
        let _ = stream.write_all(&quit_bytes).await;
        let _ = stream.flush().await;

        Ok::<Option<ChannelId>, ProbeError>(Some(returned_sid))
    };

    match tokio::time::timeout(limits.exchange_timeout, exchange_fut).await {
        Ok(Ok(Some(actual_sid))) => {
            if actual_sid == expected_session_id {
                Ok(ProbeResult::Reachable {
                    session_id: actual_sid,
                    target_addr,
                })
            } else {
                Ok(ProbeResult::SessionMismatch {
                    expected: expected_session_id,
                    actual: actual_sid,
                    target_addr,
                })
            }
        }
        Ok(Ok(None)) => Ok(ProbeResult::Unreachable {
            target_addr,
            reason: "Peer returned oleh with missing session ID".to_string(),
        }),
        Ok(Err(e)) => Ok(ProbeResult::Unreachable {
            target_addr,
            reason: format!("Protocol error: {}", e),
        }),
        Err(_) => Ok(ProbeResult::Unreachable {
            target_addr,
            reason: "Exchange timeout exceeded".to_string(),
        }),
    }
}
