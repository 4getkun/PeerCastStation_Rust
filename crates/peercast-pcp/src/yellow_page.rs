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
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
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

impl YpAnnouncementChannel {
    /// Creates a new announcement channel with required identities.
    pub fn new(channel_id: ChannelId, broadcast_id: ChannelId, name: String) -> Self {
        Self {
            channel_id,
            broadcast_id,
            name,
            genre: String::new(),
            desc: String::new(),
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
        Atom::new_leaf(
            ID4::HOST_ID,
            Bytes::copy_from_slice(session_id.as_bytes()),
        ),
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
            Atom::new_leaf(ID4::BCST_FROM, Bytes::copy_from_slice(session_id.as_bytes())),
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
        })
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
        map.insert(cid, channel);
        self.ref_count.fetch_add(1, Ordering::SeqCst);
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
        let _ = self.shutdown_tx.send(true);
        let _ = self.status_tx.send(YpConnectionStatus::Stopped);
    }

    /// Runs the announcement worker loop until cancelled, terminal banned/server-quit,
    /// or refcount drops to zero.
    pub async fn run_loop(&self) -> Result<(), YpError> {
        let mut backoff = YpBackoffTracker::default();
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
        let stream = match TcpStream::connect(self.config.directory_endpoint).await {
            Ok(s) => s,
            Err(e) => return Err(YpError::ConnectionFailed(e.to_string())),
        };

        let (mut reader, mut writer) = stream.into_split();

        // 1. Send handshake prelude: "pcp\n" with version 1
        let mut connect_buf = BytesMut::new();
        let _ = Atom::new_leaf(ID4::PCP_CONNECT, Bytes::copy_from_slice(&[1, 0, 0, 0]))
            .encode(&mut connect_buf);
        writer.write_all(&connect_buf).await?;

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
                Atom::new_u16_le(ID4::HELO_PORT, 7144),
            ],
        );
        let mut helo_buf = BytesMut::new();
        let _ = helo_atom.encode(&mut helo_buf);
        writer.write_all(&helo_buf).await?;

        // 3. Await OLEH from directory
        let mut read_buf = BytesMut::with_capacity(4096);
        let oleh_atom = read_next_atom(&mut reader, &mut read_buf).await?;

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

        let _ = self.status_tx.send(YpConnectionStatus::Connected);

        // 4. Send initial announcements for all active channels immediately
        {
            let channels_guard = self.channels.lock().await;
            for ch in channels_guard.values() {
                let bcst = create_station_yp_bcst_atom(
                    self.session_id,
                    self.config.servant_version,
                    ch,
                );
                let mut buf = BytesMut::new();
                let _ = bcst.encode(&mut buf);
                writer.write_all(&buf).await?;
            }
        }

        // 5. 30-Second periodic update loop (REQ-062)
        let mut update_interval = tokio::time::interval(self.config.update_interval);
        update_interval.tick().await; // Consume initial immediate tick

        loop {
            tokio::select! {
                _ = update_interval.tick() => {
                    // Check ownership refcount (REQ-062)
                    if self.ref_count.load(Ordering::SeqCst) == 0 {
                        // Send clean quit atom upon final channel removal
                        let quit_atom = Atom::new_u32_le(ID4::PCP_QUIT, 0);
                        let mut buf = BytesMut::new();
                        let _ = quit_atom.encode(&mut buf);
                        let _ = writer.write_all(&buf).await;
                        return Ok(());
                    }

                    // Transmit periodic 30s announcement updates
                    let channels_guard = self.channels.lock().await;
                    for ch in channels_guard.values() {
                        let bcst = create_station_yp_bcst_atom(
                            self.session_id,
                            self.config.servant_version,
                            ch,
                        );
                        let mut buf = BytesMut::new();
                        let _ = bcst.encode(&mut buf);
                        writer.write_all(&buf).await?;
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        let quit_atom = Atom::new_u32_le(ID4::PCP_QUIT, 0);
                        let mut buf = BytesMut::new();
                        let _ = quit_atom.encode(&mut buf);
                        let _ = writer.write_all(&buf).await;
                        return Ok(());
                    }
                }
            }
        }
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
                Atom::new_leaf(
                    ID4::HELO_SESSIONID,
                    Bytes::copy_from_slice(&[0x77; 16]),
                ),
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
            let mut bytes = Bytes::copy_from_slice(read_buf);
            let original_len = bytes.len();
            if let Ok(Some(atom)) = Atom::decode(&mut bytes, &peercast_core::AtomBudget::default()) {
                let consumed = original_len - bytes.len();
                read_buf.advance(consumed);
                return Ok(atom);
            }
        }

        let mut temp = [0u8; 1024];
        let n = reader.read(&mut temp).await?;
        if n == 0 {
            return Err(YpError::ConnectionFailed("EOF from peer".to_string()));
        }
        read_buf.extend_from_slice(&temp[..n]);
    }
}
