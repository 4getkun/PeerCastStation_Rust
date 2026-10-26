//! Configured channel workers and the bounded replay-to-playback path.
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use peercast_core::cache::{HeaderSnapshot, SystemCacheClock};
use peercast_core::channel_id::ChannelId;
use peercast_core::content::{Content, ContinuationFlags};
use peercast_core::gossip::{GossipForwardDecision, GossipForwarder, DEFAULT_MIN_FORWARDING_TTL};
use peercast_core::index_txt::{
    parse_index_txt, DirectoryChannelEntry, IndexParseError, IndexParserLimits,
};
use peercast_core::{Atom, ContentRingBuffer};
use peercast_format::safestart::{SafeStartConfig, SafeStartMode};
use peercast_http::ingest::{HttpPushConfig, HttpPushIngestListener};
use peercast_http::streamer::stream_media_to_client_with_write_timeout;
use peercast_http::{HttpRequest, PlaylistBuilder, PlaylistEntry, PlaylistFormat};
use peercast_pcp::probe::{
    execute_reverse_probe_with_profile, ProbeError, ProbeLimiter, ProbeLimits, ProbePeerProfile,
    ProbeResult,
};
use peercast_pcp::yellow_page::{
    PCPYellowPageClient, YpAnnouncementChannel, YpAnnouncementProvider, YpClientConfig,
    YpConnectionStatus,
};
use peercast_rpc::server::{ChannelSummary, RpcState};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{
    broadcast, mpsc, oneshot, watch, Mutex, OwnedSemaphorePermit, RwLock, Semaphore,
};
use tokio::task::JoinHandle;

use crate::runtime_config::{ChannelConfig, ChannelSource};
use crate::{AppConfig, BoundedLogBuffer, LogLevel, TaskTracker, APPLICATION_NAME};

mod diagnostics;
mod directory;
use diagnostics::{ConnectionRegistry, NetworkDiagnostics};
pub use diagnostics::{ConnectionSnapshot, PortSnapshot};
mod output;
mod port_mapping;
mod push;
mod relay;
pub use port_mapping::PortMappingStatus;

struct ChannelAnnouncements {
    channels: HashMap<ChannelId, Arc<RuntimeChannel>>,
    relay_slots: Arc<Semaphore>,
    max_directs_per_channel: usize,
    max_relays_per_channel: usize,
}

impl YpAnnouncementProvider for ChannelAnnouncements {
    fn snapshot(
        &self,
        mut current: YpAnnouncementChannel,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = YpAnnouncementChannel> + Send + '_>>
    {
        Box::pin(async move {
            if let Some(channel) = self.channels.get(&current.channel_id) {
                let state = channel.snapshot().await;
                current.is_receiving = state.available;
                current.is_playable = state.available;
                current.is_relayable = state.available
                    && channel.relay_slots.available_permits() > 0
                    && self.relay_slots.available_permits() > 0;
                current.genre = channel.config.metadata.genre.clone();
                current.desc = channel.config.metadata.description.clone();
                current.comment = channel.config.metadata.comment.clone();
                current.url = channel.config.metadata.contact_url.clone();
                current.bitrate_kbps = channel.config.metadata.bitrate;
                current.oldest_pos = state.oldest_pos.map(|pos| pos as u32);
                current.newest_pos = state.newest_pos.map(|pos| pos as u32);
                current.listeners = (self.max_directs_per_channel
                    - channel.direct_slots.available_permits())
                    as u32;
                current.relays =
                    (self.max_relays_per_channel - channel.relay_slots.available_permits()) as u32;
                current.uptime = channel.started.elapsed();
            }
            current
        })
    }
}

#[derive(Clone)]
enum StreamEvent {
    Body(Content),
    Reset,
}

struct ChannelState {
    ring: ContentRingBuffer,
    header: HeaderSnapshot,
    available: bool,
}

#[derive(Default)]
struct RelayControl {
    endpoint: Option<SocketAddr>,
    stop_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

pub struct RuntimeChannel {
    config: ChannelConfig,
    id: ChannelId,
    state: Mutex<ChannelState>,
    events: broadcast::Sender<StreamEvent>,
    direct_slots: Arc<Semaphore>,
    relay_slots: Arc<Semaphore>,
    candidates: Mutex<Vec<peercast_core::host::Host>>,
    started: Instant,
    connection_attempts: AtomicUsize,
    relay_active: AtomicBool,
    relay_control: Mutex<RelayControl>,
    max_header_bytes: usize,
    connections: Arc<ConnectionRegistry>,
}

#[derive(Debug, Clone)]
pub struct RuntimeChannelSnapshot {
    pub available: bool,
    pub buffered_bytes: usize,
    pub buffered_packets: usize,
    pub oldest_pos: Option<i64>,
    pub newest_pos: Option<i64>,
    pub connection_attempts: usize,
    pub relay_active: bool,
    pub connections: Vec<ConnectionSnapshot>,
    pub uptime_secs: u64,
}

struct Subscription {
    header: HeaderSnapshot,
    replay: Vec<Content>,
    live: broadcast::Receiver<StreamEvent>,
    _global: OwnedSemaphorePermit,
    _channel: OwnedSemaphorePermit,
}

impl RuntimeChannel {
    fn new(config: ChannelConfig, app: &AppConfig) -> io::Result<Self> {
        let id = ChannelId::from_hex(&config.channel_id).map_err(io::Error::other)?;
        let relay_endpoint = match &config.source {
            ChannelSource::Relay { endpoint } => Some(*endpoint),
            ChannelSource::HttpPush { .. } => None,
        };
        let (events, _) = broadcast::channel(app.runtime.streaming.subscriber_queue_packets);
        Ok(Self {
            state: Mutex::new(ChannelState {
                ring: app.create_content_ring_buffer(),
                header: HeaderSnapshot::new(0, 0, &config.container_type, Bytes::new()),
                available: false,
            }),
            id,
            config,
            events,
            direct_slots: Arc::new(Semaphore::new(app.server.max_directs_per_channel as usize)),
            relay_slots: Arc::new(Semaphore::new(app.server.max_relays_per_channel as usize)),
            candidates: Mutex::new(Vec::new()),
            started: Instant::now(),
            connection_attempts: AtomicUsize::new(0),
            relay_active: AtomicBool::new(false),
            relay_control: Mutex::new(RelayControl {
                endpoint: relay_endpoint,
                ..RelayControl::default()
            }),
            max_header_bytes: app.runtime.streaming.max_header_bytes,
            connections: Arc::new(ConnectionRegistry::default()),
        })
    }

    async fn begin_epoch(&self) {
        let mut state = self.state.lock().await;
        state.available = false;
        state.ring.clear();
        let epoch = state.header.epoch.saturating_add(1);
        state.header = HeaderSnapshot::new(epoch, 0, &self.config.container_type, Bytes::new());
        let _ = self.events.send(StreamEvent::Reset);
    }

    async fn set_header(&self, data: Bytes, pos: i64) -> io::Result<()> {
        if data.len() > self.max_header_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "stream header exceeds budget",
            ));
        }
        let mut state = self.state.lock().await;
        let epoch = state.header.epoch.saturating_add(1);
        state.ring.clear();
        state.header = HeaderSnapshot::new(epoch, pos, &self.config.container_type, data);
        state.available = true;
        let _ = self.events.send(StreamEvent::Reset);
        Ok(())
    }

    async fn publish(&self, pos: i64, data: Bytes, flags: ContinuationFlags) {
        let mut state = self.state.lock().await;
        let content = Content::new(state.header.epoch, self.started.elapsed(), pos, data, flags);
        state.ring.push(content.clone());
        state.available = true;
        // Publishing and subscribing share this lock: replay and live packets cannot overlap or gap.
        let _ = self.events.send(StreamEvent::Body(content));
    }

    async fn offline(&self) {
        let mut state = self.state.lock().await;
        state.available = false;
        let _ = self.events.send(StreamEvent::Reset);
    }

    pub fn config(&self) -> &ChannelConfig {
        &self.config
    }

    pub async fn snapshot(&self) -> RuntimeChannelSnapshot {
        let state = self.state.lock().await;
        RuntimeChannelSnapshot {
            available: state.available,
            buffered_bytes: state.ring.total_bytes(),
            buffered_packets: state.ring.len(),
            oldest_pos: state.ring.oldest().map(|c| c.pos),
            newest_pos: state.ring.newest().map(|c| c.pos),
            connection_attempts: self.connection_attempts.load(Ordering::Relaxed),
            relay_active: self.relay_active.load(Ordering::Acquire),
            connections: self.connections.snapshot(),
            uptime_secs: self.started.elapsed().as_secs(),
        }
    }

    async fn subscribe(&self, global: Arc<Semaphore>, relay: bool) -> io::Result<Subscription> {
        let global = global
            .try_acquire_owned()
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "direct capacity reached"))?;
        let slots = if relay {
            &self.relay_slots
        } else {
            &self.direct_slots
        };
        let channel = Arc::clone(slots)
            .try_acquire_owned()
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "channel capacity reached"))?;
        let state = self.state.lock().await;
        if !state.available {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "channel source is not receiving",
            ));
        }
        Ok(Subscription {
            header: state.header.clone(),
            replay: state.ring.iter().cloned().collect(),
            live: self.events.subscribe(),
            _global: global,
            _channel: channel,
        })
    }
}

pub struct AppRuntime {
    config: AppConfig,
    channels: HashMap<ChannelId, Arc<RuntimeChannel>>,
    dynamic_channels: StdRwLock<HashMap<ChannelId, Arc<RuntimeChannel>>>,
    channel_lifecycle: tokio::sync::Mutex<()>,
    tracker: TaskTracker,
    log: Arc<BoundedLogBuffer>,
    rpc_state: Arc<RwLock<RpcState>>,
    shutdown_tx: watch::Sender<bool>,
    started: AtomicBool,
    session_id: ChannelId,
    direct_slots: Arc<Semaphore>,
    relay_slots: Arc<Semaphore>,
    yellow_pages: Mutex<Vec<Arc<PCPYellowPageClient>>>,
    gossip: Mutex<GossipForwarder>,
    probe_limiter: ProbeLimiter,
    ingest_addresses: Mutex<HashMap<ChannelId, SocketAddr>>,
    directory: Arc<directory::DirectoryService>,
    port_mapping: Mutex<PortMappingStatus>,
    network: NetworkDiagnostics,
}

impl AppRuntime {
    pub fn new(
        config: &AppConfig,
        tracker: TaskTracker,
        log: Arc<BoundedLogBuffer>,
        rpc_state: Arc<RwLock<RpcState>>,
    ) -> io::Result<Arc<Self>> {
        config.validate().map_err(io::Error::other)?;
        let session_id = ChannelId::from_bytes(uuid::Uuid::new_v4().into_bytes());
        let mut channels = HashMap::new();
        for channel in &config.runtime.channels {
            let channel = Arc::new(RuntimeChannel::new(channel.clone(), config)?);
            channels.insert(channel.id, channel);
        }
        let (shutdown_tx, _) = watch::channel(false);
        let probe = &config.runtime.probe;
        let gossip = &config.runtime.gossip;
        let directory =
            directory::DirectoryService::new(config, Arc::clone(&log), shutdown_tx.subscribe())?;
        Ok(Arc::new(Self {
            config: config.clone(),
            channels,
            dynamic_channels: StdRwLock::new(HashMap::new()),
            channel_lifecycle: tokio::sync::Mutex::new(()),
            tracker,
            log,
            rpc_state,
            shutdown_tx,
            started: AtomicBool::new(false),
            session_id,
            direct_slots: Arc::new(Semaphore::new(config.server.max_directs as usize)),
            relay_slots: Arc::new(Semaphore::new(config.server.max_relays as usize)),
            yellow_pages: Mutex::new(Vec::new()),
            gossip: Mutex::new(GossipForwarder::with_clock(
                session_id,
                DEFAULT_MIN_FORWARDING_TTL,
                gossip.max_entries,
                Duration::from_secs(gossip.window_secs),
                Arc::new(SystemCacheClock::new()),
            )),
            probe_limiter: ProbeLimiter::new(ProbeLimits {
                max_concurrent_global: probe.max_concurrent_global,
                connect_timeout: Duration::from_millis(probe.connect_timeout_ms),
                exchange_timeout: Duration::from_millis(probe.exchange_timeout_ms),
                ..ProbeLimits::default()
            }),
            ingest_addresses: Mutex::new(HashMap::new()),
            directory,
            port_mapping: Mutex::new(PortMappingStatus::Disabled),
            network: NetworkDiagnostics::new(SocketAddr::new(
                config
                    .server
                    .bind_address
                    .parse()
                    .map_err(io::Error::other)?,
                config.server.port,
            )),
        }))
    }

    pub fn channel(&self, id: &str) -> Option<Arc<RuntimeChannel>> {
        let id = ChannelId::from_hex(id).ok()?;
        self.channels
            .get(&id)
            .cloned()
            .or_else(|| self.dynamic_channels.read().ok()?.get(&id).cloned())
    }

    fn ensure_relay_control_open(&self) -> io::Result<()> {
        if !self.started.load(Ordering::SeqCst) || *self.shutdown_tx.borrow() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "runtime is not running",
            ));
        }
        Ok(())
    }

    async fn start_relay_worker_locked(
        self: &Arc<Self>,
        channel: Arc<RuntimeChannel>,
        endpoint: SocketAddr,
    ) -> io::Result<()> {
        self.ensure_relay_control_open()?;
        if !matches!(&channel.config.source, ChannelSource::Relay { .. }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "channel is not a relay",
            ));
        }

        let (stale_stop, stale_task) = {
            let mut control = channel.relay_control.lock().await;
            if channel.relay_active.load(Ordering::Acquire)
                && control
                    .task
                    .as_ref()
                    .is_some_and(|task| !task.is_finished())
            {
                control.endpoint = Some(endpoint);
                return Ok(());
            }
            channel.relay_active.store(false, Ordering::Release);
            (control.stop_tx.take(), control.task.take())
        };
        if let Some(stop) = stale_stop {
            let _ = stop.send(());
        }
        let stale_result = match stale_task {
            Some(task) => task.await.map_err(io::Error::other),
            None => Ok(()),
        };
        self.ensure_relay_control_open()?;
        channel.offline().await;
        channel.begin_epoch().await;
        stale_result?;

        let mut control = channel.relay_control.lock().await;
        self.ensure_relay_control_open()?;
        if channel.relay_active.load(Ordering::Acquire) {
            control.endpoint = Some(endpoint);
            return Ok(());
        }
        let (stop_tx, mut stop_rx) = oneshot::channel();
        control.endpoint = Some(endpoint);
        control.stop_tx = Some(stop_tx);
        channel.relay_active.store(true, Ordering::Release);
        let runtime = Arc::clone(self);
        let worker_channel = Arc::clone(&channel);
        control.task = Some(self.tracker.spawn(async move {
            tokio::select! {
                _ = relay::run(Arc::clone(&runtime), Arc::clone(&worker_channel), endpoint) => {}
                _ = &mut stop_rx => {}
            }
            worker_channel.offline().await;
            worker_channel.relay_active.store(false, Ordering::Release);
            runtime.channel_changed(&worker_channel).await;
        }));
        drop(control);
        self.channel_changed(&channel).await;
        Ok(())
    }

    async fn stop_relay_worker_locked(&self, channel: &Arc<RuntimeChannel>) -> io::Result<()> {
        let (stop_tx, task) = {
            let mut control = channel.relay_control.lock().await;
            (control.stop_tx.take(), control.task.take())
        };
        if let Some(stop) = stop_tx {
            let _ = stop.send(());
        }
        let join_result = if let Some(task) = task {
            task.await.map_err(io::Error::other)
        } else {
            Ok(())
        };
        channel.relay_active.store(false, Ordering::Release);
        channel.offline().await;
        channel.begin_epoch().await;
        self.channel_changed(channel).await;
        join_result
    }

    fn relay_channel(&self, id: &str) -> io::Result<Arc<RuntimeChannel>> {
        self.channel(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "relay channel not found"))
    }

    /// Stops one configured or dynamic relay without changing its settings.
    pub async fn disconnect_relay_channel(self: &Arc<Self>, id: &str) -> io::Result<()> {
        let _lifecycle = self.channel_lifecycle.lock().await;
        self.ensure_relay_control_open()?;
        let channel = self.relay_channel(id)?;
        if !matches!(&channel.config.source, ChannelSource::Relay { .. }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "channel is not a relay",
            ));
        }
        self.stop_relay_worker_locked(&channel).await
    }

    /// Restarts one configured or dynamic relay using its most recently saved seed.
    pub async fn reconnect_relay_channel(self: &Arc<Self>, id: &str) -> io::Result<()> {
        let _lifecycle = self.channel_lifecycle.lock().await;
        self.ensure_relay_control_open()?;
        let channel = self.relay_channel(id)?;
        let configured_endpoint = match &channel.config.source {
            ChannelSource::Relay { endpoint } => *endpoint,
            ChannelSource::HttpPush { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "channel is not a relay",
                ));
            }
        };
        let endpoint = channel
            .relay_control
            .lock()
            .await
            .endpoint
            .unwrap_or(configured_endpoint);
        self.stop_relay_worker_locked(&channel).await?;
        self.ensure_relay_control_open()?;
        self.start_relay_worker_locked(channel, endpoint).await
    }

    /// Adds a runtime relay channel without changing the configured channel set.
    pub async fn add_relay_channel(
        self: &Arc<Self>,
        config: ChannelConfig,
    ) -> io::Result<Arc<RuntimeChannel>> {
        if !matches!(config.source, ChannelSource::Relay { .. }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "dynamic channel must be a relay",
            ));
        }
        let mut validation = self.config.clone();
        validation.runtime.channels = vec![config.clone()];
        validation.validate().map_err(io::Error::other)?;
        let id = ChannelId::from_hex(&config.channel_id).map_err(io::Error::other)?;

        let _lifecycle = self.channel_lifecycle.lock().await;
        self.ensure_relay_control_open()?;
        if let Some(channel) = self.channels.get(&id) {
            let endpoint = match &channel.config.source {
                ChannelSource::Relay { endpoint } => *endpoint,
                ChannelSource::HttpPush { .. } => {
                    // An existing local broadcast can still be watched; only
                    // explicit relay-control operations reject HTTP inputs.
                    return Ok(Arc::clone(channel));
                }
            };
            let channel = Arc::clone(channel);
            self.start_relay_worker_locked(Arc::clone(&channel), endpoint)
                .await?;
            return Ok(channel);
        }
        let endpoint = match config.source {
            ChannelSource::Relay { endpoint } => endpoint,
            ChannelSource::HttpPush { .. } => unreachable!("validated relay source"),
        };
        let channel = {
            let mut dynamic = self
                .dynamic_channels
                .write()
                .map_err(|_| io::Error::other("dynamic channel registry is poisoned"))?;
            let existing = dynamic.get(&id).cloned();
            if existing.is_none() && dynamic.len() >= 32 {
                return Err(io::Error::other("dynamic relay channel limit reached"));
            }
            if let Some(existing) = existing {
                existing
            } else {
                let channel = Arc::new(RuntimeChannel::new(config.clone(), &self.config)?);
                dynamic.insert(id, Arc::clone(&channel));
                channel
            }
        };
        self.start_relay_worker_locked(Arc::clone(&channel), endpoint)
            .await?;
        Ok(channel)
    }

    fn all_channels(&self) -> Vec<Arc<RuntimeChannel>> {
        let mut channels: Vec<_> = self.channels.values().cloned().collect();
        if let Ok(dynamic) = self.dynamic_channels.read() {
            channels.extend(dynamic.values().cloned());
        }
        channels
    }

    pub async fn channel_snapshots(&self) -> Vec<(ChannelConfig, RuntimeChannelSnapshot)> {
        let mut channels = self.all_channels();
        channels.sort_by_key(|channel| channel.id);
        let mut snapshots = Vec::with_capacity(channels.len());
        for channel in channels {
            snapshots.push((channel.config.clone(), channel.snapshot().await));
        }
        snapshots
    }

    pub fn yellow_page_directory(&self) -> Arc<dyn peercast_rpc::server::YellowPageDirectory> {
        self.directory.clone()
    }

    pub fn network_snapshot(&self) -> PortSnapshot {
        if let Ok(clients) = self.yellow_pages.try_lock() {
            for client in clients.iter() {
                if let Some(endpoint) = client.observed_endpoint() {
                    self.network.observe(
                        Some(endpoint.ip()),
                        Some(endpoint.port()),
                        endpoint.is_ipv6(),
                    );
                }
            }
        }
        self.network.snapshot()
    }

    pub fn session_id(&self) -> String {
        self.session_id.to_string()
    }

    pub async fn port_mapping_status(&self) -> PortMappingStatus {
        self.port_mapping.lock().await.clone()
    }

    pub async fn start(self: &Arc<Self>) -> io::Result<()> {
        if *self.shutdown_tx.borrow() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "runtime has stopped",
            ));
        }
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let result = self.start_workers().await;
        if result.is_err() {
            self.shutdown().await;
        }
        result
    }

    async fn start_workers(self: &Arc<Self>) -> io::Result<()> {
        if self.config.runtime.port_mapping.enabled {
            let runtime = Arc::clone(self);
            self.tracker.spawn(async move {
                port_mapping::run(runtime).await;
            });
        }
        for channel in self.channels.values() {
            match &channel.config.source {
                ChannelSource::Relay { endpoint } => {
                    let channel = Arc::clone(channel);
                    let _lifecycle = self.channel_lifecycle.lock().await;
                    self.start_relay_worker_locked(channel, *endpoint).await?;
                }
                ChannelSource::HttpPush {
                    bind_address,
                    auth_token,
                } => {
                    let settings = &self.config.runtime.ingest;
                    let mut config = HttpPushConfig::new(*bind_address);
                    config.auth_token = auth_token.clone();
                    config.max_chunk_size = settings.max_chunk_bytes;
                    config.max_header_size = settings.max_header_bytes;
                    config.header_timeout =
                        Duration::from_millis(self.config.server.request_timeout_ms);
                    let mut listener = HttpPushIngestListener::new(config);
                    let bound = listener.bind().await.map_err(io::Error::other)?;
                    self.ingest_addresses.lock().await.insert(channel.id, bound);
                    let runtime = Arc::clone(self);
                    let channel = Arc::clone(channel);
                    self.tracker.spawn(async move {
                        push::run(runtime, channel, listener).await;
                    });
                }
            }
        }
        // Only explicitly configured local broadcasts are announced; relayed channels are not advertised as ours.
        for settings in self
            .config
            .runtime
            .yellow_pages
            .iter()
            .filter(|yp| yp.announcement_enabled)
        {
            let mut config = YpClientConfig::for_loopback(settings.endpoint.port());
            config.directory_endpoint = settings.endpoint;
            config.agent_name = APPLICATION_NAME.to_owned();
            config.update_interval = Duration::from_secs(settings.update_interval_secs);
            config.allow_public_announcements = settings.allow_public_announcements;
            let client = Arc::new(
                PCPYellowPageClient::new(self.session_id, self.session_id, config)
                    .map_err(io::Error::other)?
                    .with_retry_delays(
                        Duration::from_millis(settings.retry_base_ms),
                        Duration::from_millis(settings.retry_max_ms),
                    )
                    .with_io_timeout(Duration::from_millis(settings.io_timeout_ms))
                    .with_announcement_provider(Arc::new(ChannelAnnouncements {
                        channels: self.channels.clone(),
                        relay_slots: Arc::clone(&self.relay_slots),
                        max_directs_per_channel: self.config.server.max_directs_per_channel
                            as usize,
                        max_relays_per_channel: self.config.server.max_relays_per_channel as usize,
                    }))
                    .with_advertised_port(self.config.server.port),
            );
            for channel in self.channels.values() {
                if matches!(channel.config.source, ChannelSource::HttpPush { .. }) {
                    if let Some(broadcast_id) = &channel.config.broadcast_id {
                        let mut announcement = YpAnnouncementChannel::new(
                            channel.id,
                            ChannelId::from_hex(broadcast_id).map_err(io::Error::other)?,
                            channel.config.name.clone(),
                        );
                        announcement.format = channel.config.container_type.clone();
                        announcement.genre = channel.config.metadata.genre.clone();
                        announcement.desc = channel.config.metadata.description.clone();
                        announcement.comment = channel.config.metadata.comment.clone();
                        announcement.url = channel.config.metadata.contact_url.clone();
                        announcement.bitrate_kbps = channel.config.metadata.bitrate;
                        let available = channel.snapshot().await.available;
                        announcement.is_receiving = available;
                        announcement.is_playable = available;
                        announcement.is_relayable =
                            available && self.config.server.max_relays_per_channel > 0;
                        client.add_channel(announcement).await;
                    }
                }
            }
            self.yellow_pages.lock().await.push(Arc::clone(&client));
            self.tracker.spawn(async move {
                let _ = client.run_loop().await;
            });
        }
        self.refresh_rpc_channels().await;
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.shutdown_tx.send_replace(true);
        let _lifecycle = self.channel_lifecycle.lock().await;
        for client in self.yellow_pages.lock().await.iter() {
            client.shutdown();
        }
        for channel in self.all_channels() {
            channel.offline().await;
        }
    }

    pub fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    pub async fn ingest_addr(&self, id: &str) -> Option<SocketAddr> {
        let id = ChannelId::from_hex(id).ok()?;
        self.ingest_addresses.lock().await.get(&id).copied()
    }

    pub async fn process_broadcast(&self, atom: &Atom) -> GossipForwardDecision {
        self.gossip.lock().await.evaluate_incoming_bcst(atom)
    }

    async fn channel_changed(&self, channel: &RuntimeChannel) {
        self.refresh_rpc_channels().await;
        if let Some(broadcast_id) = &channel.config.broadcast_id {
            if !matches!(channel.config.source, ChannelSource::HttpPush { .. }) {
                return;
            }
            let state = channel.snapshot().await;
            let mut announcement = YpAnnouncementChannel::new(
                channel.id,
                ChannelId::from_hex(broadcast_id).expect("validated broadcast id"),
                channel.config.name.clone(),
            );
            announcement.format = channel.config.container_type.clone();
            announcement.genre = channel.config.metadata.genre.clone();
            announcement.desc = channel.config.metadata.description.clone();
            announcement.comment = channel.config.metadata.comment.clone();
            announcement.url = channel.config.metadata.contact_url.clone();
            announcement.bitrate_kbps = channel.config.metadata.bitrate;
            announcement.is_receiving = state.available;
            announcement.is_playable = state.available;
            announcement.is_relayable =
                state.available && channel.relay_slots.available_permits() > 0;
            announcement.oldest_pos = state.oldest_pos.map(|pos| pos as u32);
            announcement.newest_pos = state.newest_pos.map(|pos| pos as u32);
            for client in self.yellow_pages.lock().await.iter() {
                client.update_channel(announcement.clone()).await;
            }
        }
    }

    pub async fn yellow_page_statuses(&self) -> Vec<YpConnectionStatus> {
        self.yellow_pages
            .lock()
            .await
            .iter()
            .map(|client| client.status())
            .collect()
    }

    async fn refresh_rpc_channels(&self) {
        let all_channels = self.all_channels();
        if all_channels.is_empty() {
            return;
        }
        let mut channels = Vec::new();
        for channel in all_channels {
            let state = channel.snapshot().await;
            channels.push(ChannelSummary::new(
                channel.id,
                &channel.config.name,
                if state.available { "Receiving" } else { "Idle" },
                0,
                peercast_http::resolve_mime_type(&channel.config.container_type, None),
                0,
                (self.config.server.max_directs_per_channel as usize
                    - channel.direct_slots.available_permits()) as u32,
            ));
        }
        self.rpc_state.write().await.channels = channels;
    }

    pub fn parse_directory(
        &self,
        body: &str,
    ) -> Result<Vec<DirectoryChannelEntry>, IndexParseError> {
        let limits = &self.config.runtime.directory;
        parse_index_txt(
            body,
            &IndexParserLimits {
                max_total_bytes: limits.max_total_bytes,
                max_lines: limits.max_lines,
                max_field_len: limits.max_field_len,
            },
        )
    }

    pub async fn probe(
        &self,
        observed_peer_ip: IpAddr,
        announced_port: u16,
        expected_session_id: ChannelId,
    ) -> Result<ProbeResult, ProbeError> {
        execute_reverse_probe_with_profile(
            &self.probe_limiter,
            observed_peer_ip,
            announced_port,
            expected_session_id,
            self.session_id,
            APPLICATION_NAME,
            ProbePeerProfile::PeerCastStation,
        )
        .await
    }

    pub async fn serve_stream<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        request: &HttpRequest,
        id: &str,
    ) -> io::Result<()> {
        self.serve_stream_from(writer, request, id, None).await
    }

    pub async fn serve_stream_from<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
        request: &HttpRequest,
        id: &str,
        peer: Option<SocketAddr>,
    ) -> io::Result<()> {
        let Some(channel) = self.channel(id) else {
            return write_empty(writer, "404 Channel Not Found").await;
        };
        let session = match channel
            .subscribe(Arc::clone(&self.direct_slots), false)
            .await
        {
            Ok(session) => session,
            Err(_) => return write_empty(writer, "503 Service Unavailable").await,
        };
        let lease = channel.connections.track("direct", peer, "connected");
        let mut writer = diagnostics::MeteredWriter::new(writer, &lease);
        let (tx, rx) = mpsc::channel(self.config.runtime.streaming.subscriber_queue_packets);
        let mut shutdown = self.shutdown_tx.subscribe();
        let mut pump_shutdown = shutdown.clone();
        let Subscription {
            header,
            replay,
            mut live,
            _global,
            _channel,
        } = session;
        // Permits remain owned by the writer until it ends, including slow/disconnected clients.
        let _permits = (_global, _channel);
        self.tracker.spawn(async move {
            let pump = async move {
                for content in replay {
                    if tx.send(content).await.is_err() {
                        return;
                    }
                }
                while let Ok(StreamEvent::Body(content)) = live.recv().await {
                    if tx.send(content).await.is_err() {
                        return;
                    }
                }
            };
            tokio::select! { _ = pump => {}, _ = pump_shutdown.changed() => {} }
        });
        let mode = match channel.config.container_type.to_ascii_uppercase().as_str() {
            "FLV" | "MKV" | "WEBM" => SafeStartMode::VideoKeyframe,
            "TS" => SafeStartMode::TsVideoKeyframe,
            "MP3" | "OGG" => SafeStartMode::AudioOnly,
            "ASF" | "WMV" => SafeStartMode::AnyUnfragmentedBoundary,
            _ => SafeStartMode::Raw,
        };
        let config = SafeStartConfig {
            mode,
            max_wait: Duration::from_millis(self.config.runtime.streaming.safe_start_timeout_ms),
        };
        tokio::select! {
            result = stream_media_to_client_with_write_timeout(&mut writer, request, &header, rx, config, Duration::from_millis(self.config.server.request_timeout_ms)) => result.map_err(io::Error::other),
            _ = shutdown.changed() => Ok(()),
        }
    }

    pub fn playlist(
        &self,
        id: &str,
        format: PlaylistFormat,
        extension: Option<&str>,
        host: &str,
    ) -> Option<Bytes> {
        let channel = self.channel(id)?;
        Some(
            PlaylistBuilder::new(format, host)
                .add_entry(PlaylistEntry {
                    channel_id_hex: channel.id.to_hex_upper(),
                    title: channel.config.name.clone(),
                    extension: extension.unwrap_or("").to_owned(),
                    contact_url: None,
                })
                .build(),
        )
    }
}

async fn write_empty<W: AsyncWrite + Unpin>(writer: &mut W, status: &str) -> io::Result<()> {
    writer
        .write_all(
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
}
