//! YellowPage (YP) channel announcements, PCP profile, ownership refcount, and update lifecycle (REQ-035, REQ-062, REQ-063).
//!
//! # Architectural Invariants
//! 1. **Station Announcement Format (REQ-035)**:
//!    YP announcements send a `bcst` atom wrapping Station-format `chan` and `host` atoms with:
//!    - `ttl = 1`
//!    - `hops = 0`
//!    - `from = session_id`
//!    - `group = 1` (`BroadcastGroup::ROOT`)
//!    - `cid = channel_id`
//!
//!    Do NOT use C++ channel ID hashing for Station-compatible announcements.
//! 2. **Reference Update Policy & 3/2 Backoff (REQ-062)**:
//!    - Periodic channel updates are transmitted every 30 seconds (`UPDATE_INTERVAL`).
//!    - On connection failure, backoff starts at 1,000ms, advances via integer arithmetic
//!      `delay = (delay * 3) / 2`, and is capped at 30,000ms.
//!    - Jitter is supported for distributed smoothing (default ±10% when enabled, 0 in tests).
//! 3. **Terminal Responses & Shutdown**:
//!    - `Banned`: If the directory sends `oleh` with `dis\0 != 0` (or banned quit), the client
//!      stops immediately and halts all retries.
//!    - `ServerQuit`: If the directory sends a general `quit` atom, client stops retries.
//!    - `Final Owner Removal`: When all channel owners are removed (refcount drops to 0), the client
//!      shuts down its connection and transmits a clean `quit` atom. No announcements remain active.
//! 4. **Safety Against Public YP Misdirection (REQ-063)**:
//!    To prevent accidental automated broadcast spamming against public YP directories,
//!    non-loopback/public destinations are strictly forbidden unless explicitly authorized
//!    by `allow_public_announcements`.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Mutex};

use peercast_core::atom::Atom;
use peercast_core::channel_id::ChannelId;
use peercast_core::gossip::BroadcastGroup;
use peercast_core::host::HostFlags1;
use peercast_core::id4::ID4;

/// Default periodic announcement update interval (Station reference policy: 30 seconds).
pub const DEFAULT_YP_UPDATE_INTERVAL: Duration = Duration::from_secs(30);

/// Initial retry delay following a transient connection failure.
pub const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(1000);

/// Maximum capped retry backoff delay.
pub const MAX_RETRY_DELAY: Duration = Duration::from_millis(30000);

const SHUTDOWN_QUIT_TIMEOUT: Duration = Duration::from_millis(100);

/// Servant protocol version advertised during YP handshake (Station default: 1218).
pub const DEFAULT_SERVANT_VERSION: u32 = 1218;

/// Errors produced during YP client operations.
#[derive(Debug, Error)]
pub enum YpError {
    #[error("Unauthorized public YP destination: {0} (must enable allow_public_announcements)")]
    UnauthorizedPublicDestination(String),
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Handshake aborted by server quit code: 0x{0:08X}")]
    QuitReceived(u32),
    #[error("Broadcaster is banned by the YellowPage directory (dis flag non-zero)")]
    Banned,
    #[error("Server requested shutdown (ServerQuit)")]
    ServerQuit,
    #[error("Directory connection was canceled")]
    Canceled,
    #[error("Malformed YP response: {0}")]
    MalformedResponse(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Status of the connection to the YellowPage directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YpConnectionStatus {
    /// No channels registered; connection idle.
    Idle,
    /// Establishing TCP connection or performing handshake.
    Connecting,
    /// Connected and actively announcing channels.
    Connected,
    /// Waiting for backoff delay before reconnecting.
    BackoffWaiting,
    /// Terminal state: Broadcaster BCID is banned by the directory.
    TerminalBanned,
    /// Terminal state: Directory sent general quit requesting termination.
    TerminalServerQuit,
    /// Client stopped and canceled.
    Stopped,
}

/// Tracks Station-compatible 3/2 integer backoff arithmetic (REQ-062).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YpBackoffTracker {
    base_delay: Duration,
    max_delay: Duration,
    current_delay: Duration,
    attempt_count: usize,
}

impl Default for YpBackoffTracker {
    fn default() -> Self {
        Self::new(INITIAL_RETRY_DELAY, MAX_RETRY_DELAY)
    }
}

impl YpBackoffTracker {
    /// Creates a new backoff tracker.
    pub fn new(base_delay: Duration, max_delay: Duration) -> Self {
        let base_delay = base_delay.min(max_delay);
        Self {
            base_delay,
            max_delay,
            current_delay: base_delay,
            attempt_count: 0,
        }
    }

    /// Advances backoff on failure using integer arithmetic `(delay * 3) / 2` capped at `max_delay`.
    pub fn record_failure(&mut self) -> Duration {
        self.attempt_count = self.attempt_count.saturating_add(1);
        let cur_ms = self.current_delay.as_millis() as u64;
        let next_ms = (cur_ms.saturating_mul(3) / 2).min(self.max_delay.as_millis() as u64);
        self.current_delay = Duration::from_millis(next_ms);
        self.current_delay
    }

    /// Resets the backoff tracker upon successful connection.
    pub fn reset_success(&mut self) {
        self.attempt_count = 0;
        self.current_delay = self.base_delay;
    }

    /// Current backoff delay.
    #[inline]
    pub fn current_delay(&self) -> Duration {
        self.current_delay
    }

    /// Number of consecutive failures recorded.
    #[inline]
    pub fn attempt_count(&self) -> usize {
        self.attempt_count
    }
}

/// Metadata describing a channel being announced to the YellowPage directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YpAnnouncementChannel {
    pub channel_id: ChannelId,
    pub broadcast_id: ChannelId,
    pub name: String,
    pub genre: String,
    pub desc: String,
    pub comment: String,
    pub url: String,
    pub bitrate_kbps: u32,
    pub format: String,
    pub contact_endpoint: Option<SocketAddr>,
    pub listeners: u32,
    pub relays: u32,
    pub uptime: Duration,
    pub oldest_pos: Option<u32>,
    pub newest_pos: Option<u32>,
    pub is_relayable: bool,
    pub is_playable: bool,
    pub is_firewalled: bool,
    pub is_receiving: bool,
}

/// Supplies current runtime metadata for an outgoing YP announcement.
pub trait YpAnnouncementProvider: Send + Sync {
    fn snapshot(
        &self,
        current: YpAnnouncementChannel,
    ) -> Pin<Box<dyn Future<Output = YpAnnouncementChannel> + Send + '_>>;
}

impl YpAnnouncementChannel {
    /// Creates a new announcement channel with required identities.
    pub fn new(channel_id: ChannelId, broadcast_id: ChannelId, name: String) -> Self {
        Self {
            channel_id,
            broadcast_id,
            name,
            genre: String::new(),
            desc: String::new(),
            comment: String::new(),
            url: String::new(),
            bitrate_kbps: 0,
            format: "FLV".to_string(),
            contact_endpoint: None,
            listeners: 0,
            relays: 0,
            uptime: Duration::ZERO,
            oldest_pos: None,
            newest_pos: None,
            is_relayable: true,
            is_playable: true,
            is_firewalled: false,
            is_receiving: true,
        }
    }
}

/// Constructs a Station-compatible `bcst` announcement Atom (REQ-035).
///
/// In YP announcement packets:
/// - `bcst` with `ttl = 1`, `hops = 0`, `from = session_id`, `group = 1` (`BroadcastGroup::ROOT`), `cid = channel_id`.
/// - `chan` child: `id`, `bcid`, `info` (name, gnre, desc, url, bitr, type).
/// - `host` child: `cid`, `id` (session_id), `ip`, `port`, `numl`, `numr`, `uptm`, `ver`, `flg1`, `oldp`, `newp`.
pub fn create_station_yp_bcst_atom(
    session_id: ChannelId,
    servant_version: u32,
    channel: &YpAnnouncementChannel,
) -> Atom {
    // 1. Build chan.info children
    let mut info_children = Vec::new();
    if !channel.name.is_empty() {
        info_children.push(Atom::new_string(ID4::INFO_NAME, &channel.name));
    }
    if !channel.genre.is_empty() {
        info_children.push(Atom::new_string(ID4::INFO_GENRE, &channel.genre));
    }
    if !channel.desc.is_empty() {
        info_children.push(Atom::new_string(ID4::INFO_DESC, &channel.desc));
    }
    if !channel.comment.is_empty() {
        info_children.push(Atom::new_string(ID4::INFO_COMMENT, &channel.comment));
    }
    if !channel.url.is_empty() {
        info_children.push(Atom::new_string(ID4::INFO_URL, &channel.url));
    }
    if channel.bitrate_kbps > 0 {
        info_children.push(Atom::new_u32_le(ID4::INFO_BITR, channel.bitrate_kbps));
    }
    if !channel.format.is_empty() {
        info_children.push(Atom::new_string(ID4::INFO_TYPE, &channel.format));
    }

    // 2. Build chan atom
    let chan_atom = Atom::new_parent(
        ID4::PCP_CHAN,
        vec![
            Atom::new_leaf(
                ID4::CHAN_ID,
                Bytes::copy_from_slice(channel.channel_id.as_bytes()),
            ),
            Atom::new_leaf(
                ID4::CHAN_BCID,
                Bytes::copy_from_slice(channel.broadcast_id.as_bytes()),
            ),
            Atom::new_parent(ID4::CHAN_INFO, info_children),
        ],
    );

    // 3. Build host flags (flg1)
    let mut flags_raw = HostFlags1::TRACKER.raw();
    if channel.is_relayable {
        flags_raw |= HostFlags1::RELAY.raw();
    }
    if channel.is_playable {
        flags_raw |= HostFlags1::DIRECT.raw();
    }
    if channel.is_firewalled {
        flags_raw |= HostFlags1::FIREWALLED.raw();
    }
    if channel.is_receiving {
        flags_raw |= HostFlags1::RECV.raw();
    }

    // 4. Build host atom children
    let mut host_children = vec![
        Atom::new_leaf(
            ID4::HOST_CHANID,
            Bytes::copy_from_slice(channel.channel_id.as_bytes()),
        ),
        Atom::new_leaf(ID4::HOST_ID, Bytes::copy_from_slice(session_id.as_bytes())),
        Atom::new_u32_le(ID4::HOST_NUML, channel.listeners),
        Atom::new_u32_le(ID4::HOST_NUMR, channel.relays),
        Atom::new_u32_le(ID4::HOST_UPTIME, channel.uptime.as_secs() as u32),
        Atom::new_u32_le(ID4::HOST_VERSION, servant_version),
        Atom::new_u8(ID4::HOST_FLAGS1, flags_raw),
    ];

    if let Some(ep) = channel.contact_endpoint {
        host_children.push(Atom::new_ip(ID4::HOST_IP, ep.ip()));
        host_children.push(Atom::new_u16_le(ID4::HOST_PORT, ep.port()));
    }
    if let Some(oldp) = channel.oldest_pos {
        host_children.push(Atom::new_u32_le(ID4::HOST_OLDPOS, oldp));
    }
    if let Some(newp) = channel.newest_pos {
        host_children.push(Atom::new_u32_le(ID4::HOST_NEWPOS, newp));
    }

    let host_atom = Atom::new_parent(ID4::PCP_HOST, host_children);

    // 5. Wrap inside bcst Atom (REQ-035: ttl=1, hops=0, group=ROOT)
    Atom::new_parent(
        ID4::PCP_BCST,
        vec![
            Atom::new_u8(ID4::BCST_TTL, 1),
            Atom::new_u8(ID4::BCST_HOPS, 0),
            Atom::new_leaf(
                ID4::BCST_FROM,
                Bytes::copy_from_slice(session_id.as_bytes()),
            ),
            Atom::new_u8(ID4::BCST_GRP, BroadcastGroup::ROOT.raw()),
            Atom::new_leaf(
                ID4::BCST_CID,
                Bytes::copy_from_slice(channel.channel_id.as_bytes()),
            ),
            Atom::new_u32_le(ID4::BCST_VERS, servant_version),
            chan_atom,
            host_atom,
        ],
    )
}

/// Configuration options for the PCP YellowPage client.
#[derive(Debug, Clone)]
pub struct YpClientConfig {
    /// YellowPage directory host and port (e.g. `127.0.0.1:7144`).
    pub directory_endpoint: SocketAddr,
    /// Agent identifier sent in PCP HELO.
    pub agent_name: String,
    /// Servant protocol version (default 1218).
    pub servant_version: u32,
    /// Periodic announcement update interval (default 30s).
    pub update_interval: Duration,
    /// Safety guard: allow non-loopback public destinations (default false per REQ-063).
    pub allow_public_announcements: bool,
}

impl YpClientConfig {
    /// Creates a configuration pointing to a loopback address.
    pub fn for_loopback(port: u16) -> Self {
        Self {
            directory_endpoint: SocketAddr::new([127, 0, 0, 1].into(), port),
            agent_name: "PeerCastStation/Rust".to_string(),
            servant_version: DEFAULT_SERVANT_VERSION,
            update_interval: DEFAULT_YP_UPDATE_INTERVAL,
            allow_public_announcements: false,
        }
    }
}

/// Asynchronous PCP YellowPage Client (REQ-035, REQ-062, REQ-063).
pub struct PCPYellowPageClient {
    session_id: ChannelId,
    broadcast_id: ChannelId,
    config: YpClientConfig,
    channels: Arc<Mutex<HashMap<ChannelId, YpAnnouncementChannel>>>,
    ref_count: Arc<AtomicUsize>,
    status_tx: watch::Sender<YpConnectionStatus>,
    status_rx: watch::Receiver<YpConnectionStatus>,
    shutdown_tx: watch::Sender<bool>,
    retry_base: Duration,
    retry_max: Duration,
    io_timeout: Duration,
    advertised_port: u16,
    announcement_provider: Option<Arc<dyn YpAnnouncementProvider>>,
    observed_endpoint: RwLock<Option<SocketAddr>>,
}

impl PCPYellowPageClient {
    /// Creates a new YellowPage client.
    pub fn new(
        session_id: ChannelId,
        broadcast_id: ChannelId,
        config: YpClientConfig,
    ) -> Result<Self, YpError> {
        // Enforce safety guard against unauthorized public YPs (REQ-063)
        if !config.allow_public_announcements && !config.directory_endpoint.ip().is_loopback() {
            return Err(YpError::UnauthorizedPublicDestination(
                config.directory_endpoint.to_string(),
            ));
        }

        let (status_tx, status_rx) = watch::channel(YpConnectionStatus::Idle);
        let (shutdown_tx, _) = watch::channel(false);

        Ok(Self {
            session_id,
            broadcast_id,
            config,
            channels: Arc::new(Mutex::new(HashMap::new())),
            ref_count: Arc::new(AtomicUsize::new(0)),
            status_tx,
            status_rx,
            shutdown_tx,
            retry_base: INITIAL_RETRY_DELAY,
            retry_max: MAX_RETRY_DELAY,
            io_timeout: Duration::from_secs(10),
            advertised_port: 7144,
            announcement_provider: None,
            observed_endpoint: RwLock::new(None),
        })
    }

    pub fn observed_endpoint(&self) -> Option<SocketAddr> {
        *self
            .observed_endpoint
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Configures the retry backoff delays used by `run_loop`.
    pub fn with_retry_delays(mut self, base: Duration, max: Duration) -> Self {
        self.retry_base = base.min(max);
        self.retry_max = max;
        self
    }

    /// Configures the timeout for individual connection and protocol I/O operations.
    pub fn with_io_timeout(mut self, timeout: Duration) -> Self {
        self.io_timeout = timeout.max(Duration::from_millis(1));
        self
    }

    /// Configures the port advertised in the YP HELO atom.
    pub fn with_advertised_port(mut self, port: u16) -> Self {
        self.advertised_port = port;
        self
    }

    pub fn with_announcement_provider(mut self, provider: Arc<dyn YpAnnouncementProvider>) -> Self {
        self.announcement_provider = Some(provider);
        self
    }

    async fn announcements(&self) -> Vec<YpAnnouncementChannel> {
        let current = self
            .channels
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let Some(provider) = &self.announcement_provider else {
            return current;
        };
        let mut updated = Vec::with_capacity(current.len());
        for channel in current {
            updated.push(provider.snapshot(channel).await);
        }
        updated
    }

    /// Observes the current connection status.
    pub fn status(&self) -> YpConnectionStatus {
        *self.status_rx.borrow()
    }

    /// Subscribes to status change notifications.
    pub fn subscribe_status(&self) -> watch::Receiver<YpConnectionStatus> {
        self.status_rx.clone()
    }

    /// Current channel refcount.
    #[inline]
    pub fn ref_count(&self) -> usize {
        self.ref_count.load(Ordering::SeqCst)
    }

    /// Adds a channel to be announced, incrementing the ownership refcount (REQ-062).
    pub async fn add_channel(&self, channel: YpAnnouncementChannel) {
        let mut map = self.channels.lock().await;
        let cid = channel.channel_id;
        if map.insert(cid, channel).is_none() {
            self.ref_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Updates metadata for an already registered channel.
    pub async fn update_channel(&self, channel: YpAnnouncementChannel) -> bool {
        let mut map = self.channels.lock().await;
        if let Some(existing) = map.get_mut(&channel.channel_id) {
            *existing = channel;
            true
        } else {
            false
        }
    }

    /// Removes a channel from announcement, decrementing the refcount.
    ///
    /// When refcount reaches 0, connection and announcements are automatically terminated.
    pub async fn remove_channel(&self, channel_id: &ChannelId) -> bool {
        let mut map = self.channels.lock().await;
        if map.remove(channel_id).is_some() {
            self.ref_count.fetch_sub(1, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Explicitly shuts down the client and closes all directory connections.
    pub fn shutdown(&self) {
        self.shutdown_tx.send_replace(true);
        let _ = self.status_tx.send(YpConnectionStatus::Stopped);
    }

    /// Runs the announcement worker loop until cancelled, terminal banned/server-quit,
    /// or refcount drops to zero.
    pub async fn run_loop(&self) -> Result<(), YpError> {
        let mut backoff = YpBackoffTracker::new(self.retry_base, self.retry_max);
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        loop {
            // Check cancellation
            if *shutdown_rx.borrow() {
                let _ = self.status_tx.send(YpConnectionStatus::Stopped);
                return Ok(());
            }

            // Check channel ownership refcount (REQ-062)
            if self.ref_count.load(Ordering::SeqCst) == 0 {
                let _ = self.status_tx.send(YpConnectionStatus::Idle);
                return Ok(());
            }

            let _ = self.status_tx.send(YpConnectionStatus::Connecting);

            // Attempt TCP connect and announcement session
            match self.connect_and_announce(&mut shutdown_rx).await {
                Ok(()) => {
                    // Normal completion (e.g. refcount reached 0 or clean shutdown)
                    backoff.reset_success();
                    if self.ref_count.load(Ordering::SeqCst) == 0 {
                        let _ = self.status_tx.send(YpConnectionStatus::Idle);
                        return Ok(());
                    }
                }
                Err(YpError::Banned) => {
                    // Terminal Banned: Stop immediately, do not retry! (REQ-062)
                    let _ = self.status_tx.send(YpConnectionStatus::TerminalBanned);
                    return Err(YpError::Banned);
                }
                Err(YpError::ServerQuit) => {
                    // Terminal ServerQuit: Directory requested shutdown (REQ-062)
                    let _ = self.status_tx.send(YpConnectionStatus::TerminalServerQuit);
                    return Err(YpError::ServerQuit);
                }
                Err(YpError::Canceled) => {
                    let _ = self.status_tx.send(YpConnectionStatus::Stopped);
                    return Ok(());
                }
                Err(_e) => {
                    // Non-terminal error: wait with 1000ms * 3 / 2 backoff capped at 30,000ms
                    let delay = backoff.record_failure();
                    let _ = self.status_tx.send(YpConnectionStatus::BackoffWaiting);

                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {},
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                let _ = self.status_tx.send(YpConnectionStatus::Stopped);
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
    }

    /// Executes a single connected session with the YellowPage directory.
    async fn connect_and_announce(
        &self,
        shutdown_rx: &mut watch::Receiver<bool>,
    ) -> Result<(), YpError> {
        *self
            .observed_endpoint
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        let connect = tokio::select! {
            result = tokio::time::timeout(self.io_timeout, TcpStream::connect(self.config.directory_endpoint)) => result,
            _ = shutdown_rx.changed() => return Err(YpError::Canceled),
        };
        let stream = match connect {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Err(YpError::ConnectionFailed(e.to_string())),
            Err(_) => {
                return Err(YpError::ConnectionFailed(
                    "TCP connect timed out".to_string(),
                ));
            }
        };

        let (mut reader, mut writer) = stream.into_split();

        // 1. Send handshake prelude: "pcp\n" with version 1
        let mut connect_buf = BytesMut::new();
        let _ = Atom::new_leaf(ID4::PCP_CONNECT, Bytes::copy_from_slice(&[1, 0, 0, 0]))
            .encode(&mut connect_buf);
        tokio::select! {
            result = tokio::time::timeout(self.io_timeout, writer.write_all(&connect_buf)) => result
                .map_err(|_| YpError::ConnectionFailed("PCP prelude write timed out".to_string()))??,
            _ = shutdown_rx.changed() => return Err(YpError::Canceled),
        }

        // 2. Send HELO atom
        let helo_atom = Atom::new_parent(
            ID4::PCP_HELO,
            vec![
                Atom::new_string(ID4::HELO_AGENT, &self.config.agent_name),
                Atom::new_u32_le(ID4::HELO_VERSION, self.config.servant_version),
                Atom::new_leaf(
                    ID4::HELO_SESSIONID,
                    Bytes::copy_from_slice(self.session_id.as_bytes()),
                ),
                Atom::new_leaf(
                    ID4::HELO_BCID,
                    Bytes::copy_from_slice(self.broadcast_id.as_bytes()),
                ),
                Atom::new_u16_le(ID4::HELO_PORT, self.advertised_port),
                Atom::new_u16_le(ID4::HELO_PING, self.advertised_port),
            ],
        );
        let mut helo_buf = BytesMut::new();
        let _ = helo_atom.encode(&mut helo_buf);
        tokio::select! {
            result = tokio::time::timeout(self.io_timeout, writer.write_all(&helo_buf)) => result
                .map_err(|_| YpError::ConnectionFailed("HELO write timed out".to_string()))??,
            _ = shutdown_rx.changed() => return Err(YpError::Canceled),
        }

        // 3. Await OLEH from directory
        let mut read_buf = BytesMut::with_capacity(4096);
        let oleh_atom = tokio::select! {
            result = tokio::time::timeout(self.io_timeout, read_next_atom(&mut reader, &mut read_buf)) => result
                .map_err(|_| YpError::ConnectionFailed("OLEH read timed out".to_string()))??,
            _ = shutdown_rx.changed() => return Err(YpError::Canceled),
        };

        // Validate OLEH or check for QUIT / Banned
        if oleh_atom.name() == ID4::PCP_QUIT {
            let code = oleh_atom.get_u32_le().unwrap_or(0);
            if code == 0x01000000 {
                return Err(YpError::ServerQuit);
            }
            return Err(YpError::QuitReceived(code));
        }

        if oleh_atom.name() != ID4::PCP_OLEH {
            return Err(YpError::MalformedResponse(format!(
                "Expected OLEH atom, got {:?}",
                oleh_atom.name()
            )));
        }

        // Check dis\0 flag for terminal Banned status (REQ-062)
        if let Some(children) = oleh_atom.children() {
            for child in children {
                if child.name() == ID4::HELO_DISABLE {
                    let dis_val = child.get_u8().unwrap_or(0);
                    if dis_val != 0 {
                        return Err(YpError::Banned);
                    }
                }
            }
        }

        // The directory observes our address across NAT. Use that address for
        // announcements which do not already provide an explicit endpoint.
        let observed_endpoint = oleh_atom.children().and_then(|children| {
            let ip = children
                .iter()
                .find(|child| child.name() == ID4::HELO_REMOTEIP)?
                .get_ip()
                .ok()?;
            if ip.is_unspecified() || ip.is_multicast() {
                return None;
            }
            let port = children
                .iter()
                .find(|child| child.name() == ID4::HELO_PORT)?
                .get_u16_le()
                .ok()?;
            Some(SocketAddr::new(ip, port))
        });
        *self
            .observed_endpoint
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = observed_endpoint;

        let _ = self.status_tx.send(YpConnectionStatus::Connected);
        let mut published = HashMap::new();

        // 4. Send initial announcements for all active channels immediately
        {
            let mut announcements = self.announcements().await;
            for channel in &mut announcements {
                if channel.contact_endpoint.is_none() {
                    channel.contact_endpoint = observed_endpoint;
                    if observed_endpoint.is_some_and(|endpoint| endpoint.port() == 0) {
                        channel.is_firewalled = true;
                    }
                }
            }
            for ch in &announcements {
                let bcst =
                    create_station_yp_bcst_atom(self.session_id, self.config.servant_version, ch);
                let mut buf = BytesMut::new();
                let _ = bcst.encode(&mut buf);
                tokio::select! {
                    result = tokio::time::timeout(self.io_timeout, writer.write_all(&buf)) => result
                        .map_err(|_| YpError::ConnectionFailed("initial announcement timed out".to_string()))??,
                    _ = shutdown_rx.changed() => {
                        let departed = published.values().cloned().collect::<Vec<_>>();
                        let _ = self.send_withdrawals(&mut writer, &departed, true).await;
                        return Err(YpError::Canceled);
                    },
                }
                published.insert(ch.channel_id, ch.clone());
            }
        }

        // 5. 30-Second periodic update loop (REQ-062)
        let mut update_interval = tokio::time::interval(self.config.update_interval);
        update_interval.tick().await; // Consume initial immediate tick

        loop {
            tokio::select! {
                result = read_next_atom(&mut reader, &mut read_buf) => {
                    let atom = result?;
                    if atom.name() == ID4::PCP_QUIT {
                        let code = atom.get_u32_le().unwrap_or(0);
                        if code == 0x01000000 {
                            return Err(YpError::ServerQuit);
                        }
                        return Err(YpError::QuitReceived(code));
                    }
                }
                _ = update_interval.tick() => {
                    // Check ownership refcount (REQ-062)
                    if self.ref_count.load(Ordering::SeqCst) == 0 {
                        let departed = published.values().cloned().collect::<Vec<_>>();
                        let _ = self.send_withdrawals(&mut writer, &departed, true).await;
                        return Ok(());
                    }

                    // Transmit periodic 30s announcement updates
                    let mut announcements = self.announcements().await;
                    for channel in &mut announcements {
                        if channel.contact_endpoint.is_none() {
                            channel.contact_endpoint = observed_endpoint;
                            if observed_endpoint.is_some_and(|endpoint| endpoint.port() == 0) {
                                channel.is_firewalled = true;
                            }
                        }
                    }
                    let active = announcements.into_iter()
                        .map(|channel| (channel.channel_id, channel)).collect::<HashMap<_, _>>();
                    let departed = published.iter().filter(|(id, _)| !active.contains_key(id))
                        .map(|(_, channel)| channel.clone()).collect::<Vec<_>>();
                    self.send_withdrawals(&mut writer, &departed, false).await?;
                    published.retain(|id, _| active.contains_key(id));
                    for ch in active.values() {
                        let bcst = create_station_yp_bcst_atom(
                            self.session_id,
                            self.config.servant_version,
                            ch,
                        );
                        let mut buf = BytesMut::new();
                        let _ = bcst.encode(&mut buf);
                        tokio::select! {
                            result = tokio::time::timeout(self.io_timeout, writer.write_all(&buf)) => result
                                .map_err(|_| YpError::ConnectionFailed("announcement write timed out".to_string()))??,
                            _ = shutdown_rx.changed() => {
                                let departed = published.values().cloned().collect::<Vec<_>>();
                                let _ = self.send_withdrawals(&mut writer, &departed, true).await;
                                return Err(YpError::Canceled);
                            },
                        }
                        published.insert(ch.channel_id, ch.clone());
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        let departed = published.values().cloned().collect::<Vec<_>>();
                        let _ = self.send_withdrawals(&mut writer, &departed, true).await;
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn send_withdrawals(
        &self,
        writer: &mut tokio::net::tcp::OwnedWriteHalf,
        channels: &[YpAnnouncementChannel],
        quit: bool,
    ) -> Result<(), YpError> {
        let mut buffer = BytesMut::new();
        for channel in channels {
            let mut departed = channel.clone();
            departed.is_receiving = false;
            departed.is_playable = false;
            departed.is_relayable = false;
            let _ = create_station_yp_bcst_atom(
                self.session_id,
                self.config.servant_version,
                &departed,
            )
            .encode(&mut buffer);
        }
        if quit {
            let _ = Atom::new_u32_le(ID4::PCP_QUIT, 0).encode(&mut buffer);
        }
        if !buffer.is_empty() {
            tokio::time::timeout(
                self.io_timeout.min(SHUTDOWN_QUIT_TIMEOUT),
                writer.write_all(&buffer),
            )
            .await
            .map_err(|_| YpError::ConnectionFailed("withdrawal write timed out".into()))??;
        }
        Ok(())
    }
}

/// In-memory mock YellowPage server for loopback integration testing (REQ-063).
pub struct MockYellowPageServer {
    listener: TcpListener,
    port: u16,
    banned_mode: bool,
    server_quit_mode: bool,
    received_bcsts: Arc<Mutex<Vec<Atom>>>,
}

impl MockYellowPageServer {
    /// Binds a new mock YP server on a free loopback port.
    pub async fn bind() -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener,
            port,
            banned_mode: false,
            server_quit_mode: false,
            received_bcsts: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Port number assigned to this server.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Configures the server to reject broadcaster with terminal Banned flag (`dis = 1`).
    pub fn set_banned_mode(&mut self, banned: bool) {
        self.banned_mode = banned;
    }

    /// Configures the server to abort handshake with terminal ServerQuit (`quit 0x01000000`).
    pub fn set_server_quit_mode(&mut self, server_quit: bool) {
        self.server_quit_mode = server_quit;
    }

    /// Access the list of `bcst` atoms received by the mock server.
    pub fn received_bcsts(&self) -> Arc<Mutex<Vec<Atom>>> {
        self.received_bcsts.clone()
    }

    /// Accepts one connection and executes the mock directory protocol.
    pub async fn run_once(&self) -> Result<(), YpError> {
        let (mut socket, _) = self.listener.accept().await?;
        let (mut reader, mut writer) = socket.split();

        let mut read_buf = BytesMut::with_capacity(4096);

        // Read handshake (connect + helo)
        let _connect_atom = read_next_atom(&mut reader, &mut read_buf).await?;
        let _helo_atom = read_next_atom(&mut reader, &mut read_buf).await?;

        if self.server_quit_mode {
            let quit_atom = Atom::new_u32_le(ID4::PCP_QUIT, 0x01000000);
            let mut buf = BytesMut::new();
            let _ = quit_atom.encode(&mut buf);
            writer.write_all(&buf).await?;
            return Ok(());
        }

        // Build OLEH response
        let dis_flag = if self.banned_mode { 1u8 } else { 0u8 };
        let oleh_atom = Atom::new_parent(
            ID4::PCP_OLEH,
            vec![
                Atom::new_leaf(ID4::HELO_SESSIONID, Bytes::copy_from_slice(&[0x77; 16])),
                Atom::new_u8(ID4::HELO_DISABLE, dis_flag),
                Atom::new_u16_le(ID4::HELO_PORT, 7144),
            ],
        );
        let mut oleh_buf = BytesMut::new();
        let _ = oleh_atom.encode(&mut oleh_buf);
        writer.write_all(&oleh_buf).await?;

        if self.banned_mode {
            return Ok(());
        }

        // Receive incoming broadcast announcements
        while let Ok(atom) = read_next_atom(&mut reader, &mut read_buf).await {
            if atom.name() == ID4::PCP_QUIT {
                break;
            }
            if atom.name() == ID4::PCP_BCST {
                let mut list = self.received_bcsts.lock().await;
                list.push(atom);
            }
        }

        Ok(())
    }
}

async fn read_next_atom<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    read_buf: &mut BytesMut,
) -> Result<Atom, YpError> {
    loop {
        if !read_buf.is_empty() {
            let mut bytes = read_buf.split().freeze();
            let original_len = bytes.len();
            match Atom::decode(&mut bytes, &peercast_core::AtomBudget::default()) {
                Ok(Some(atom)) => {
                    let consumed = original_len - bytes.len();
                    debug_assert!(consumed > 0);
                    *read_buf = bytes
                        .try_into_mut()
                        .unwrap_or_else(|bytes| BytesMut::from(bytes.as_ref()));
                    return Ok(atom);
                }
                Ok(None) => {
                    *read_buf = bytes
                        .try_into_mut()
                        .unwrap_or_else(|bytes| BytesMut::from(bytes.as_ref()));
                }
                Err(error) => return Err(YpError::MalformedResponse(error.to_string())),
            }
        }

        let mut temp = [0u8; 1024];
        let n = reader.read(&mut temp).await?;
        if n == 0 {
            return Err(YpError::ConnectionFailed("EOF from peer".to_string()));
        }
        if read_buf.len().saturating_add(n) > peercast_core::AtomBudget::default().max_total_bytes {
            return Err(YpError::MalformedResponse(
                "YP input exceeds atom budget".to_string(),
            ));
        }
        read_buf.extend_from_slice(&temp[..n]);
    }
}

#[cfg(test)]
mod incremental_reader_tests {
    use super::*;
    use tokio::io::duplex;

    #[derive(Debug)]
    struct LiveAnnouncement;

    impl YpAnnouncementProvider for LiveAnnouncement {
        fn snapshot(
            &self,
            mut current: YpAnnouncementChannel,
        ) -> Pin<Box<dyn Future<Output = YpAnnouncementChannel> + Send + '_>> {
            Box::pin(async move {
                current.listeners = 17;
                current
            })
        }
    }

    #[tokio::test]
    async fn announcements_apply_runtime_provider_snapshot() {
        let client = PCPYellowPageClient::new(
            ChannelId::from_bytes([1; 16]),
            ChannelId::from_bytes([2; 16]),
            YpClientConfig::for_loopback(1),
        )
        .unwrap()
        .with_announcement_provider(Arc::new(LiveAnnouncement));
        client
            .add_channel(YpAnnouncementChannel::new(
                ChannelId::from_bytes([3; 16]),
                ChannelId::from_bytes([4; 16]),
                "live snapshot".to_string(),
            ))
            .await;
        let snapshot = client.announcements().await;
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].listeners, 17);
    }

    #[tokio::test]
    async fn read_next_atom_retains_partial_input_and_coalesced_tail() {
        let first = Atom::new_string(ID4(*b"test"), "partial");
        let second = Atom::new_string(ID4(*b"test"), "following");
        let first_wire = first.to_bytes().unwrap();
        let second_wire = second.to_bytes().unwrap();
        let (mut writer, mut reader) = duplex(128);
        let mut read_buf = BytesMut::new();
        let mut combined = Vec::from(first_wire.as_ref());
        combined.extend_from_slice(&second_wire);
        let split = first_wire.len() / 2;

        let mut pending_read = Box::pin(read_next_atom(&mut reader, &mut read_buf));
        writer.write_all(&combined[..split]).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut pending_read)
                .await
                .is_err()
        );
        writer.write_all(&combined[split..]).await.unwrap();
        assert_eq!(pending_read.await.unwrap(), first);
        assert_eq!(
            read_next_atom(&mut reader, &mut read_buf).await.unwrap(),
            second
        );
    }

    #[tokio::test]
    async fn read_next_atom_propagates_oversized_and_malformed_leaf_header() {
        let (mut writer, mut reader) = duplex(64);
        writer.write_all(b"test\xff\xff\xff\x7f").await.unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            read_next_atom(&mut reader, &mut BytesMut::new()),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(matches!(error, YpError::MalformedResponse(_)));
    }

    #[tokio::test]
    async fn shutdown_before_run_loop_returns_without_connecting() {
        let client = PCPYellowPageClient::new(
            ChannelId::from_bytes([1; 16]),
            ChannelId::from_bytes([2; 16]),
            YpClientConfig::for_loopback(1),
        )
        .unwrap();
        client
            .add_channel(YpAnnouncementChannel::new(
                ChannelId::from_bytes([3; 16]),
                ChannelId::from_bytes([4; 16]),
                "shutdown test".to_string(),
            ))
            .await;
        client.shutdown();
        tokio::time::timeout(Duration::from_millis(100), client.run_loop())
            .await
            .expect("shutdown must short-circuit the initial loop")
            .unwrap();
        assert_eq!(client.status(), YpConnectionStatus::Stopped);
    }

    async fn run_observed_endpoint_fixture(
        observed_port: u16,
        contact_endpoint: Option<SocketAddr>,
        update_interval: Duration,
        announcement_count: usize,
    ) -> Vec<Atom> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let directory_endpoint = listener.local_addr().unwrap();
        let (bcst_tx, bcst_rx) = tokio::sync::oneshot::channel();
        let fixture = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (mut reader, mut writer) = socket.split();
            let mut read_buf = BytesMut::with_capacity(4096);
            let _connect = read_next_atom(&mut reader, &mut read_buf).await.unwrap();
            let helo = read_next_atom(&mut reader, &mut read_buf).await.unwrap();
            let helo_children = helo.children().unwrap();
            assert_eq!(
                helo_children
                    .iter()
                    .find(|child| child.name() == ID4::HELO_PORT)
                    .and_then(|child| child.get_u16_le().ok()),
                Some(17144)
            );
            assert_eq!(
                helo_children
                    .iter()
                    .find(|child| child.name() == ID4::HELO_PING)
                    .and_then(|child| child.get_u16_le().ok()),
                Some(17144)
            );

            let oleh = Atom::new_parent(
                ID4::PCP_OLEH,
                vec![
                    Atom::new_ip(ID4::HELO_REMOTEIP, "127.0.0.1".parse().unwrap()),
                    Atom::new_u16_le(ID4::HELO_PORT, observed_port),
                ],
            );
            let mut oleh_buf = BytesMut::new();
            oleh.encode(&mut oleh_buf).unwrap();
            writer.write_all(&oleh_buf).await.unwrap();

            let mut bcsts = Vec::with_capacity(announcement_count);
            for _ in 0..announcement_count {
                bcsts.push(read_next_atom(&mut reader, &mut read_buf).await.unwrap());
            }
            bcst_tx.send(bcsts).unwrap();
            while let Ok(atom) = read_next_atom(&mut reader, &mut read_buf).await {
                if atom.name() == ID4::PCP_QUIT {
                    break;
                }
            }
        });

        let mut config = YpClientConfig::for_loopback(directory_endpoint.port());
        config.directory_endpoint = directory_endpoint;
        config.update_interval = update_interval;
        let client = Arc::new(
            PCPYellowPageClient::new(
                ChannelId::from_bytes([1; 16]),
                ChannelId::from_bytes([2; 16]),
                config,
            )
            .unwrap()
            .with_advertised_port(17144),
        );
        let mut channel = YpAnnouncementChannel::new(
            ChannelId::from_bytes([3; 16]),
            ChannelId::from_bytes([4; 16]),
            "loopback endpoint regression".to_string(),
        );
        channel.is_receiving = true;
        channel.contact_endpoint = contact_endpoint;
        client.add_channel(channel).await;

        let running_client = Arc::clone(&client);
        let run_loop = tokio::spawn(async move { running_client.run_loop().await });
        let bcsts = tokio::time::timeout(Duration::from_secs(2), bcst_rx)
            .await
            .expect("fixture should receive the initial announcement")
            .expect("fixture should send the announcement");
        assert_eq!(
            client.observed_endpoint(),
            Some(SocketAddr::new("127.0.0.1".parse().unwrap(), observed_port,))
        );
        client.shutdown();
        run_loop.await.unwrap().unwrap();
        fixture.await.unwrap();
        bcsts
    }

    fn host_endpoint_and_flags(bcst: &Atom) -> (Option<SocketAddr>, u8) {
        let host = bcst
            .children()
            .unwrap()
            .iter()
            .find(|child| child.name() == ID4::PCP_HOST)
            .expect("announcement should contain host atom");
        let host_children = host.children().unwrap();
        let host_ip = host_children
            .iter()
            .find(|child| child.name() == ID4::HOST_IP)
            .and_then(|child| child.get_ip().ok());
        let host_port = host_children
            .iter()
            .find(|child| child.name() == ID4::HOST_PORT)
            .and_then(|child| child.get_u16_le().ok());
        let flags = host_children
            .iter()
            .find(|child| child.name() == ID4::HOST_FLAGS1)
            .and_then(|child| child.get_u8().ok())
            .expect("host flags should be present");
        (
            host_ip
                .zip(host_port)
                .map(|(ip, port)| SocketAddr::new(ip, port)),
            flags,
        )
    }

    #[tokio::test]
    async fn loopback_yp_announcement_includes_observed_host_endpoint() {
        let bcsts = run_observed_endpoint_fixture(17144, None, Duration::from_secs(30), 1).await;
        let (endpoint, flags) = host_endpoint_and_flags(&bcsts[0]);
        assert_eq!(endpoint, Some("127.0.0.1:17144".parse().unwrap()));
        assert_eq!(flags & HostFlags1::FIREWALLED.raw(), 0);
    }

    #[tokio::test]
    async fn periodic_yp_announcement_retains_observed_host_endpoint() {
        let bcsts = run_observed_endpoint_fixture(17144, None, Duration::from_millis(20), 2).await;
        assert_eq!(bcsts.len(), 2);
        for bcst in &bcsts {
            let (endpoint, flags) = host_endpoint_and_flags(bcst);
            assert_eq!(endpoint, Some("127.0.0.1:17144".parse().unwrap()));
            assert_eq!(flags & HostFlags1::FIREWALLED.raw(), 0);
        }
    }

    #[tokio::test]
    async fn explicit_contact_endpoint_takes_precedence_over_observed_endpoint() {
        let explicit = "192.0.2.7:7144".parse().unwrap();
        let bcsts =
            run_observed_endpoint_fixture(17144, Some(explicit), Duration::from_secs(30), 1).await;
        let (endpoint, _) = host_endpoint_and_flags(&bcsts[0]);
        assert_eq!(endpoint, Some(explicit));
    }

    #[tokio::test]
    async fn zero_observed_port_sets_firewalled_host_flag() {
        let bcsts = run_observed_endpoint_fixture(0, None, Duration::from_secs(30), 1).await;
        let (endpoint, flags) = host_endpoint_and_flags(&bcsts[0]);
        assert_eq!(endpoint, Some("127.0.0.1:0".parse().unwrap()));
        assert_ne!(flags & HostFlags1::FIREWALLED.raw(), 0);
    }

    #[tokio::test]
    async fn shutdown_withdraws_channel_host_before_quit() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let directory_endpoint = listener.local_addr().unwrap();
        let (initial_tx, initial_rx) = tokio::sync::oneshot::channel();
        let (after_shutdown_tx, after_shutdown_rx) = tokio::sync::oneshot::channel();
        let fixture = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (mut reader, mut writer) = socket.split();
            let mut read_buf = BytesMut::with_capacity(4096);
            let _connect = read_next_atom(&mut reader, &mut read_buf).await.unwrap();
            let _helo = read_next_atom(&mut reader, &mut read_buf).await.unwrap();

            let oleh = Atom::new_parent(
                ID4::PCP_OLEH,
                vec![
                    Atom::new_ip(ID4::HELO_REMOTEIP, "127.0.0.1".parse().unwrap()),
                    Atom::new_u16_le(ID4::HELO_PORT, 17144),
                ],
            );
            let mut oleh_buf = BytesMut::new();
            oleh.encode(&mut oleh_buf).unwrap();
            writer.write_all(&oleh_buf).await.unwrap();

            let initial = read_next_atom(&mut reader, &mut read_buf).await.unwrap();
            initial_tx.send(initial).unwrap();
            let after_shutdown = read_next_atom(&mut reader, &mut read_buf).await.unwrap();
            let is_quit = after_shutdown.name() == ID4::PCP_QUIT;
            after_shutdown_tx.send(after_shutdown).unwrap();
            if !is_quit {
                while let Ok(atom) = read_next_atom(&mut reader, &mut read_buf).await {
                    if atom.name() == ID4::PCP_QUIT {
                        break;
                    }
                }
            }
        });

        let mut config = YpClientConfig::for_loopback(directory_endpoint.port());
        config.directory_endpoint = directory_endpoint;
        let client = Arc::new(
            PCPYellowPageClient::new(
                ChannelId::from_bytes([1; 16]),
                ChannelId::from_bytes([2; 16]),
                config,
            )
            .unwrap()
            .with_advertised_port(17144),
        );
        let mut channel = YpAnnouncementChannel::new(
            ChannelId::from_bytes([3; 16]),
            ChannelId::from_bytes([4; 16]),
            "withdrawal regression".to_string(),
        );
        channel.is_receiving = true;
        client.add_channel(channel).await;

        let running_client = Arc::clone(&client);
        let run_loop = tokio::spawn(async move { running_client.run_loop().await });
        let initial = tokio::time::timeout(Duration::from_secs(2), initial_rx)
            .await
            .expect("fixture should receive initial announcement")
            .expect("fixture should send initial announcement");
        client.shutdown();
        let after_shutdown = tokio::time::timeout(Duration::from_secs(2), after_shutdown_rx)
            .await
            .expect("fixture should receive withdrawal or quit")
            .expect("fixture should send the post-shutdown atom");
        run_loop.await.unwrap().unwrap();
        fixture.await.unwrap();

        assert_eq!(initial.name(), ID4::PCP_BCST);
        assert_eq!(
            after_shutdown.name(),
            ID4::PCP_BCST,
            "shutdown should withdraw the host before PCP_QUIT"
        );

        let active_host = initial
            .children()
            .unwrap()
            .iter()
            .find(|child| child.name() == ID4::PCP_HOST)
            .expect("initial announcement should have host");
        let withdrawn_host = after_shutdown
            .children()
            .unwrap()
            .iter()
            .find(|child| child.name() == ID4::PCP_HOST)
            .expect("withdrawal should retain host identity");
        let active_host_children = active_host.children().unwrap();
        let withdrawn_host_children = withdrawn_host.children().unwrap();
        for id in [ID4::HOST_CHANID, ID4::HOST_ID, ID4::HOST_IP, ID4::HOST_PORT] {
            let active_value = active_host_children
                .iter()
                .find(|child| child.name() == id)
                .and_then(|child| child.leaf_bytes());
            let withdrawn_value = withdrawn_host_children
                .iter()
                .find(|child| child.name() == id)
                .and_then(|child| child.leaf_bytes());
            assert_eq!(
                withdrawn_value, active_value,
                "withdrawal must preserve {id:?}"
            );
        }
        let active_flags = active_host_children
            .iter()
            .find(|child| child.name() == ID4::HOST_FLAGS1)
            .unwrap()
            .get_u8()
            .unwrap();
        let withdrawn_flags = withdrawn_host_children
            .iter()
            .find(|child| child.name() == ID4::HOST_FLAGS1)
            .unwrap()
            .get_u8()
            .unwrap();
        let active_mask =
            HostFlags1::RECV.raw() | HostFlags1::DIRECT.raw() | HostFlags1::RELAY.raw();
        assert_eq!(active_flags & active_mask, active_mask);
        assert_eq!(withdrawn_flags & active_mask, 0);
    }

    async fn start_channel_fixture(
        channels: Vec<YpAnnouncementChannel>,
        update_interval: Duration,
    ) -> (
        Arc<PCPYellowPageClient>,
        tokio::task::JoinHandle<Result<(), YpError>>,
        tokio::task::JoinHandle<()>,
        tokio::sync::mpsc::UnboundedReceiver<Atom>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let directory_endpoint = listener.local_addr().unwrap();
        let (atom_tx, atom_rx) = tokio::sync::mpsc::unbounded_channel();
        let fixture = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (mut reader, mut writer) = socket.split();
            let mut read_buf = BytesMut::with_capacity(4096);
            let _connect = read_next_atom(&mut reader, &mut read_buf).await.unwrap();
            let _helo = read_next_atom(&mut reader, &mut read_buf).await.unwrap();
            let oleh = Atom::new_parent(
                ID4::PCP_OLEH,
                vec![
                    Atom::new_ip(ID4::HELO_REMOTEIP, "127.0.0.1".parse().unwrap()),
                    Atom::new_u16_le(ID4::HELO_PORT, 17144),
                ],
            );
            let mut oleh_buf = BytesMut::new();
            oleh.encode(&mut oleh_buf).unwrap();
            writer.write_all(&oleh_buf).await.unwrap();

            loop {
                let atom = read_next_atom(&mut reader, &mut read_buf).await.unwrap();
                let is_quit = atom.name() == ID4::PCP_QUIT;
                atom_tx.send(atom).unwrap();
                if is_quit {
                    break;
                }
            }
        });

        let mut config = YpClientConfig::for_loopback(directory_endpoint.port());
        config.directory_endpoint = directory_endpoint;
        config.update_interval = update_interval;
        let client = Arc::new(
            PCPYellowPageClient::new(
                ChannelId::from_bytes([1; 16]),
                ChannelId::from_bytes([2; 16]),
                config,
            )
            .unwrap()
            .with_advertised_port(17144),
        );
        for channel in channels {
            client.add_channel(channel).await;
        }
        let running_client = Arc::clone(&client);
        let run_loop = tokio::spawn(async move { running_client.run_loop().await });
        (client, run_loop, fixture, atom_rx)
    }

    async fn next_fixture_atom(atoms: &mut tokio::sync::mpsc::UnboundedReceiver<Atom>) -> Atom {
        tokio::time::timeout(Duration::from_secs(2), atoms.recv())
            .await
            .expect("fixture atom should arrive before timeout")
            .expect("fixture channel should remain open")
    }

    fn announcement_channel_id(atom: &Atom) -> Vec<u8> {
        atom.children()
            .unwrap()
            .iter()
            .find(|child| child.name() == ID4::BCST_CID)
            .expect("announcement should have channel ID")
            .leaf_bytes()
            .unwrap()
            .to_vec()
    }

    fn test_channel(id: [u8; 16], bcid: [u8; 16], name: &str) -> YpAnnouncementChannel {
        let mut channel = YpAnnouncementChannel::new(
            ChannelId::from_bytes(id),
            ChannelId::from_bytes(bcid),
            name.to_string(),
        );
        channel.is_receiving = true;
        channel
    }

    fn assert_withdrawn(atom: &Atom, channel_id: [u8; 16]) {
        assert_eq!(atom.name(), ID4::PCP_BCST);
        assert_eq!(announcement_channel_id(atom), channel_id);
        let (_, flags) = host_endpoint_and_flags(atom);
        let active_mask =
            HostFlags1::RECV.raw() | HostFlags1::DIRECT.raw() | HostFlags1::RELAY.raw();
        assert_eq!(flags & active_mask, 0);
    }

    #[tokio::test]
    async fn removing_final_channel_withdraws_then_quits() {
        let cid = [3; 16];
        let channel = test_channel(cid, [4; 16], "final owner");
        let (client, run_loop, fixture, mut atoms) =
            start_channel_fixture(vec![channel], Duration::from_millis(30)).await;
        let initial = next_fixture_atom(&mut atoms).await;
        assert_eq!(initial.name(), ID4::PCP_BCST);
        assert_eq!(announcement_channel_id(&initial), cid);

        assert!(client.remove_channel(&ChannelId::from_bytes(cid)).await);
        let withdrawal = next_fixture_atom(&mut atoms).await;
        let quit = next_fixture_atom(&mut atoms).await;
        run_loop.await.unwrap().unwrap();
        fixture.await.unwrap();

        assert_withdrawn(&withdrawal, cid);
        assert_eq!(quit.name(), ID4::PCP_QUIT);
    }

    #[tokio::test]
    async fn removing_one_channel_withdraws_only_it_then_shutdown_withdraws_remaining() {
        let removed_id = [3; 16];
        let remaining_id = [5; 16];
        let channels = vec![
            test_channel(removed_id, [4; 16], "removed owner"),
            test_channel(remaining_id, [6; 16], "remaining owner"),
        ];
        let (client, run_loop, fixture, mut atoms) =
            start_channel_fixture(channels, Duration::from_millis(30)).await;
        let initial_a = next_fixture_atom(&mut atoms).await;
        let initial_b = next_fixture_atom(&mut atoms).await;
        let initial_ids = [
            announcement_channel_id(&initial_a),
            announcement_channel_id(&initial_b),
        ];
        assert!(initial_ids.contains(&removed_id.to_vec()));
        assert!(initial_ids.contains(&remaining_id.to_vec()));

        assert!(
            client
                .remove_channel(&ChannelId::from_bytes(removed_id))
                .await
        );
        let removed_withdrawal = next_fixture_atom(&mut atoms).await;
        let remaining_update = next_fixture_atom(&mut atoms).await;
        assert_withdrawn(&removed_withdrawal, removed_id);
        assert_eq!(remaining_update.name(), ID4::PCP_BCST);
        assert_eq!(announcement_channel_id(&remaining_update), remaining_id);
        let (_, active_flags) = host_endpoint_and_flags(&remaining_update);
        let active_mask =
            HostFlags1::RECV.raw() | HostFlags1::DIRECT.raw() | HostFlags1::RELAY.raw();
        assert_eq!(active_flags & active_mask, active_mask);

        client.shutdown();
        let remaining_withdrawal = next_fixture_atom(&mut atoms).await;
        let quit = next_fixture_atom(&mut atoms).await;
        run_loop.await.unwrap().unwrap();
        fixture.await.unwrap();
        assert_withdrawn(&remaining_withdrawal, remaining_id);
        assert_eq!(quit.name(), ID4::PCP_QUIT);
    }
}
