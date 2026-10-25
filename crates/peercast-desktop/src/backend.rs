use crate::broadcast::{BroadcastManager, BroadcastRequest, BroadcastSnapshot};
use crate::csharp_backend::CSharpBackend;
use crate::player::{launch_configured_player, resolved_player_path, PlayerContext, PlayerLaunch};
use crate::preferences::DesktopPreferences;
use crate::recording::{RecordingJobSnapshot, RecordingManager};
use crate::rules::{decorate_channels, preview_rules, ChannelRule, RulePreview};
use peercast_app::runtime::{
    AppRuntime, ConnectionSnapshot, PortSnapshot, RuntimeChannel, RuntimeChannelSnapshot,
};
use peercast_app::runtime_config::{
    ChannelConfig, ChannelMetadata, ChannelSource, YellowPageSettings,
};
use peercast_app::{
    AppConfig, AppMetrics, BoundedLogBuffer, ConfigManager, ConfigPersistenceError, HttpServer,
    LogLevel, ShutdownReason, TaskTracker,
};
use peercast_rpc::server::RpcState;
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{broadcast, Mutex},
    task::JoinHandle,
};

const CHANNEL_ID: &str = "0123456789abcdef0123456789abcdef";
const PROBE_CHUNKS: usize = 20;
const PROBE_CHUNK_BYTES: usize = 8192;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct YellowPageEntry {
    pub name: String,
    pub channels_url: String,
}

fn default_yellow_pages() -> Vec<YellowPageEntry> {
    vec![
        YellowPageEntry {
            name: "SP".into(),
            channels_url: "http://bayonet.ddo.jp/sp/index.txt".into(),
        },
        YellowPageEntry {
            name: "P@".into(),
            channels_url: "https://p-at.net/index.txt".into(),
        },
    ]
}

fn is_legacy_sp_default(config: &AppConfig) -> bool {
    config.runtime.yellow_pages.len() == 1
        && config.runtime.yellow_pages[0].name == "SP"
        && config.runtime.yellow_pages[0].channels_uri.as_deref()
            == Some("http://bayonet.ddo.jp/sp/index.txt")
}

fn normalize_yellow_pages(entries: Vec<YellowPageEntry>) -> Result<Vec<YellowPageEntry>, String> {
    if entries.len() > 32 {
        return Err("Yellow Pageは32件まで登録できます".into());
    }
    let mut result: Vec<YellowPageEntry> = Vec::new();
    for entry in entries {
        let name = entry.name.trim().to_owned();
        if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err("Yellow Page名は1〜80文字で指定してください".into());
        }
        let raw = entry.channels_url.trim();
        if raw.len() > 2048 {
            return Err("URLが長すぎます".into());
        }
        let mut url = reqwest::Url::parse(raw).map_err(|_| "チャンネルURLが正しくありません")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err("認証情報やフラグメントのないHTTP/HTTPS URLを指定してください".into());
        }
        if url.path().ends_with('/') {
            url.set_path(&format!("{}index.txt", url.path()));
        }
        let channels_url = url.to_string();
        if result
            .iter()
            .any(|yp| yp.name == name || yp.channels_url == channels_url)
        {
            return Err("同じ名前またはURLのYellow Pageが既に登録されています".into());
        }
        result.push(YellowPageEntry { name, channels_url });
    }
    Ok(result)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub max_directs: u32,
    pub max_relays: u32,
    pub max_upstream_rate: u32,
}

impl Limits {
    fn from_config(config: &AppConfig) -> Self {
        Self {
            max_directs: config.server.max_directs,
            max_relays: config.server.max_relays,
            max_upstream_rate: config.server.max_upstream_rate,
        }
    }

    pub(crate) fn apply(&self, config: &mut AppConfig) -> Result<(), String> {
        if !(1..=256).contains(&self.max_directs)
            || !(1..=256).contains(&self.max_relays)
            || !(1..=10_000_000).contains(&self.max_upstream_rate)
        {
            return Err("接続数は1〜256、上り帯域は1〜10000000 kbpsで指定してください".into());
        }
        config.server.max_directs = self.max_directs;
        config.server.max_directs_per_channel = self.max_directs.min(8);
        config.server.max_relays = self.max_relays;
        config.server.max_relays_per_channel = self.max_relays.min(8);
        config.server.max_upstream_rate = self.max_upstream_rate;
        config.validate().map_err(|error| error.to_string())
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub passed: bool,
    pub bytes: usize,
    pub elapsed_ms: u128,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSnapshot {
    pub id: String,
    pub name: String,
    pub status: String,
    pub buffered_bytes: usize,
    pub buffered_packets: usize,
    pub connection_attempts: usize,
    pub is_broadcasting: bool,
    pub listeners: Option<u64>,
    pub relays: Option<u64>,
    #[serde(flatten)]
    pub diagnostics: ChannelDiagnostics,
}

fn directory_metadata(entry: &serde_json::Value) -> ChannelMetadata {
    ChannelMetadata {
        genre: entry["genre"].as_str().unwrap_or_default().to_owned(),
        description: entry["description"].as_str().unwrap_or_default().to_owned(),
        comment: entry["comment"].as_str().unwrap_or_default().to_owned(),
        contact_url: entry["contactUrl"].as_str().unwrap_or_default().to_owned(),
        bitrate: entry["bitrate"]
            .as_u64()
            .unwrap_or_default()
            .min(u32::MAX as u64) as u32,
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelDiagnostics {
    pub available: bool,
    pub content_type: String,
    pub purpose: String,
    #[serde(flatten)]
    pub metadata: ChannelMetadata,
    pub uptime_secs: u64,
    pub session_id: Option<String>,
    pub connections: Vec<ConnectionSnapshot>,
    pub network: Option<PortSnapshot>,
}

impl ChannelSnapshot {
    pub fn native(
        config: &ChannelConfig,
        state: RuntimeChannelSnapshot,
        runtime: &AppRuntime,
    ) -> Self {
        Self {
            id: config.channel_id.clone(),
            name: config.name.clone(),
            status: if state.available {
                "受信中"
            } else if matches!(config.source, ChannelSource::Relay { .. }) {
                "接続中"
            } else {
                "入力待ち"
            }
            .into(),
            buffered_bytes: state.buffered_bytes,
            buffered_packets: state.buffered_packets,
            connection_attempts: state.connection_attempts,
            is_broadcasting: matches!(config.source, ChannelSource::HttpPush { .. }),
            listeners: Some(
                state
                    .connections
                    .iter()
                    .filter(|c| c.kind == "direct")
                    .count() as u64,
            ),
            relays: Some(
                state
                    .connections
                    .iter()
                    .filter(|c| c.kind == "relay")
                    .count() as u64,
            ),
            diagnostics: ChannelDiagnostics {
                available: true,
                content_type: config.container_type.clone(),
                purpose: String::new(),
                metadata: config.metadata.clone(),
                uptime_secs: state.uptime_secs,
                session_id: Some(runtime.session_id()),
                connections: state.connections,
                network: Some(runtime.network_snapshot()),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuiLogEntry {
    pub timestamp_ms: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub backend_kind: String,
    pub backend_label: String,
    pub running: bool,
    pub endpoint: String,
    pub ingest_endpoint: String,
    pub config_path: String,
    pub player_path: String,
    pub player_available: bool,
    pub desktop: DesktopPreferences,
    pub uptime_secs: u64,
    pub active_tasks: usize,
    pub limits: Limits,
    pub yellow_pages: Vec<YellowPageEntry>,
    pub channels: Vec<ChannelSnapshot>,
    pub logs: Vec<GuiLogEntry>,
    pub last_probe: Option<ProbeResult>,
    pub recordings: Vec<RecordingJobSnapshot>,
    pub broadcasts: Vec<BroadcastSnapshot>,
}

struct PreparedYpChannel {
    playlist_url: String,
    channel: Option<Arc<RuntimeChannel>>,
    entry: serde_json::Value,
}

struct RunningBackend {
    config: AppConfig,
    addr: SocketAddr,
    runtime: Arc<AppRuntime>,
    channel: Arc<RuntimeChannel>,
    metrics: Arc<AppMetrics>,
    tracker: TaskTracker,
    shutdown: broadcast::Sender<ShutdownReason>,
    task: JoinHandle<Result<(), String>>,
}

pub struct RecordingSource {
    pub stream_url: String,
    pub name: String,
    pub content_type: String,
    pub title: String,
    running: Option<RunningBackend>,
}

impl RecordingSource {
    pub fn diagnostic_runtime(&self) -> Option<Arc<AppRuntime>> {
        self.running
            .as_ref()
            .map(|running| Arc::clone(&running.runtime))
    }

    pub async fn close(&mut self) -> Result<(), String> {
        let Some(mut running) = self.running.take() else {
            return Ok(());
        };
        running.runtime.shutdown().await;
        let _ = running.shutdown.send(ShutdownReason::AdminRpc);
        match tokio::time::timeout(Duration::from_secs(5), &mut running.task).await {
            Ok(result) => result.map_err(|error| error.to_string())??,
            Err(_) => {
                running.task.abort();
                let _ = running.task.await;
                return Err("録画用HTTPサーバーの終了がタイムアウトしました".into());
            }
        }
        running
            .tracker
            .wait_for_drain(Duration::from_secs(5))
            .await
            .map_err(|count| format!("録画用タスクが{count}件残っています"))?;
        Ok(())
    }
}

struct Inner {
    running: Option<RunningBackend>,
    limits: Limits,
    yellow_pages: Vec<YellowPageEntry>,
    desktop: DesktopPreferences,
    directory_channels: Vec<serde_json::Value>,
    last_probe: Option<ProbeResult>,
}

pub struct DesktopBackend {
    inner: Mutex<Inner>,
    // Restart, configuration writes, probes and shutdown share ownership of one runtime.
    operation: Mutex<()>,
    closing: AtomicBool,
    config_path: PathBuf,
    log: Arc<BoundedLogBuffer>,
    recordings: Arc<RecordingManager>,
    external: Option<Arc<CSharpBackend>>,
    broadcasts: Arc<BroadcastManager>,
}

impl DesktopBackend {
    pub async fn open(config_path: PathBuf) -> Result<Arc<Self>, String> {
        Self::open_with_external(config_path, None).await
    }

    pub async fn open_detecting(config_path: PathBuf) -> Result<Arc<Self>, String> {
        let external = CSharpBackend::detect().await;
        Self::open_with_external(config_path, external).await
    }

    pub async fn open_external(config_path: PathBuf, origin: &str) -> Result<Arc<Self>, String> {
        let external = CSharpBackend::connect(origin).await?;
        Self::open_with_external(config_path, Some(external)).await
    }

    async fn open_with_external(
        config_path: PathBuf,
        external: Option<Arc<CSharpBackend>>,
    ) -> Result<Arc<Self>, String> {
        let (limits, yellow_pages, desktop) = match ConfigManager::new(&config_path).load() {
            Ok(config) => {
                let migrate_default = is_legacy_sp_default(&config);
                let limits = Limits::from_config(&config);
                let desktop = if config.desktop.is_null() {
                    DesktopPreferences::default()
                } else {
                    serde_json::from_value(config.desktop).map_err(|error| error.to_string())?
                };
                let mut yellow_pages: Vec<YellowPageEntry> = config
                    .runtime
                    .yellow_pages
                    .into_iter()
                    .filter_map(|yp| {
                        yp.channels_uri.map(|channels_url| YellowPageEntry {
                            name: yp.name,
                            channels_url,
                        })
                    })
                    .collect();
                if migrate_default {
                    yellow_pages = default_yellow_pages();
                }
                (limits, yellow_pages, desktop)
            }
            Err(ConfigPersistenceError::NotFound(_)) => (
                Limits {
                    max_directs: 8,
                    max_relays: 8,
                    max_upstream_rate: 20000,
                },
                default_yellow_pages(),
                DesktopPreferences::default(),
            ),
            Err(error) => return Err(error.to_string()),
        };
        let yellow_pages = normalize_yellow_pages(yellow_pages)?;
        let desktop = desktop.normalize()?;
        let backend = Arc::new(Self {
            inner: Mutex::new(Inner {
                running: None,
                limits,
                yellow_pages,
                desktop,
                directory_channels: Vec::new(),
                last_probe: None,
            }),
            operation: Mutex::new(()),
            closing: AtomicBool::new(false),
            config_path,
            log: Arc::new(BoundedLogBuffer::default()),
            recordings: RecordingManager::new(),
            broadcasts: BroadcastManager::new(external.clone()),
            external,
        });
        backend.restart().await?;
        let recording_settings = backend.recording_settings().await?;
        backend.recordings.update_settings(recording_settings).await;
        backend
            .recordings
            .attach_backend(Arc::downgrade(&backend))
            .await;
        Ok(backend)
    }

    fn check_open(&self) -> Result<(), String> {
        if self.closing.load(Ordering::Acquire) {
            Err("GUIを終了しています".into())
        } else {
            Ok(())
        }
    }

    pub async fn start_broadcast(&self, request: BroadcastRequest) -> Result<Snapshot, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        let request = request.normalize()?;
        let limits = {
            let inner = self.inner.lock().await;
            if inner.running.is_none() {
                return Err("バックエンドを起動してください".into());
            }
            inner.limits.clone()
        };
        let id = self
            .broadcasts
            .start(request.clone(), &limits, Arc::clone(&self.log))
            .await?;
        let saved = {
            let mut inner = self.inner.lock().await;
            let mut desktop = inner.desktop.clone();
            desktop.last_broadcast = Some(request);
            let mut config = inner
                .running
                .as_ref()
                .expect("running backend checked above")
                .config
                .clone();
            config.desktop = serde_json::to_value(&desktop).map_err(|error| error.to_string())?;
            match ConfigManager::new(&self.config_path).save(&config) {
                Ok(()) => {
                    inner.desktop = desktop;
                    if let Some(running) = inner.running.as_mut() {
                        running.config = config;
                    }
                    Ok(())
                }
                Err(error) => Err(format!("配信設定を保存できません: {error}")),
            }
        };
        if let Err(error) = saved {
            return Err(match self.broadcasts.stop(&id).await {
                Ok(()) => error,
                Err(stop) => format!("{error} / 配信停止にも失敗しました: {stop}"),
            });
        }
        self.snapshot().await
    }

    pub async fn stop_broadcast(&self, channel_id: &str) -> Result<Snapshot, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        self.broadcasts.stop(channel_id).await?;
        self.snapshot().await
    }

    async fn start(
        &self,
        limits: &Limits,
        yellow_pages: &[YellowPageEntry],
        desktop: &DesktopPreferences,
    ) -> Result<RunningBackend, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| error.to_string())?;
        let addr = listener.local_addr().map_err(|error| error.to_string())?;
        let mut config = AppConfig::default();
        config.server.port = addr.port();
        config.server.bind_address = "127.0.0.1".into();
        config.lifecycle.headless = true;
        config.runtime.port_mapping.enabled = false;
        config.runtime.directory.request_timeout_ms = 15_000;
        config.runtime.channels.push(ChannelConfig {
            channel_id: CHANNEL_ID.into(),
            name: "GUI接続テスト (RAW)".into(),
            container_type: "RAW".into(),
            source: ChannelSource::HttpPush {
                bind_address: "127.0.0.1:0".parse().unwrap(),
                auth_token: None,
            },
            ..Default::default()
        });
        config.runtime.yellow_pages = yellow_pages
            .iter()
            .map(|yp| YellowPageSettings {
                name: yp.name.clone(),
                channels_uri: Some(yp.channels_url.clone()),
                announcement_enabled: false,
                ..Default::default()
            })
            .collect();
        limits.apply(&mut config)?;
        config.desktop = serde_json::to_value(desktop).map_err(|error| error.to_string())?;
        let tracker = TaskTracker::new();
        let metrics = Arc::new(AppMetrics::new());
        let server = HttpServer::new(
            &config,
            RpcState {
                settings: config.to_servent_settings(),
                ..Default::default()
            },
            Arc::clone(&metrics),
            Arc::clone(&self.log),
            tracker.clone(),
        )
        .map_err(|error| error.to_string())?;
        let runtime = server.runtime();
        runtime.start().await.map_err(|error| error.to_string())?;
        let channel = runtime
            .channel(CHANNEL_ID)
            .ok_or("テストチャンネルが見つかりません")?;
        let (shutdown, receiver) = broadcast::channel(4);
        let task = tokio::spawn(async move {
            server
                .run_on_listener(listener, receiver)
                .await
                .map_err(|error| error.to_string())
        });
        self.log.log(
            LogLevel::Info,
            "desktop",
            &format!("GUI test backend started on {addr}"),
        );
        Ok(RunningBackend {
            config,
            addr,
            runtime,
            channel,
            metrics,
            tracker,
            shutdown,
            task,
        })
    }

    async fn stop_running(&self, mut running: RunningBackend) -> Result<(), String> {
        running.runtime.shutdown().await;
        let _ = running.shutdown.send(ShutdownReason::AdminRpc);
        let result = tokio::time::timeout(Duration::from_secs(5), &mut running.task).await;
        match result {
            Ok(result) => result.map_err(|error| error.to_string())??,
            Err(_) => {
                running.task.abort();
                let _ = running.task.await;
                return Err("HTTPサーバーの終了がタイムアウトしました".into());
            }
        }
        running
            .tracker
            .wait_for_drain(Duration::from_secs(5))
            .await
            .map_err(|count| format!("終了待ちのタスクが{count}件残っています"))?;
        self.log.log(
            LogLevel::Info,
            "desktop",
            "GUI test backend stopped cleanly",
        );
        Ok(())
    }

    pub async fn snapshot(&self) -> Result<Snapshot, String> {
        let recordings = self.recordings.snapshot().await;
        let recording_channels = self.recordings.channels().await;
        let (broadcasts, own_channels) = self.broadcasts.snapshot().await?;
        let helper_running = self.inner.lock().await.running.is_some();
        let external_channels = match &self.external {
            Some(_) if !helper_running => None,
            Some(external) => Some(external.channels().await?),
            None => None,
        };
        let inner = self.inner.lock().await;
        let application_exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let player_path = resolved_player_path(&application_exe, &inner.desktop.player)?;
        let mut channels = Vec::new();
        let (endpoint, uptime_secs, active_tasks) =
            if self.external.is_some() && inner.running.is_none() {
                (String::new(), 0, 0)
            } else if let Some(external) = &self.external {
                channels = external_channels.unwrap_or_default();
                (external.origin().to_owned(), 0, 0)
            } else if let Some(running) = &inner.running {
                if running.task.is_finished() {
                    return Err("バックエンドが予期せず終了しました".into());
                }
                for (config, state) in running.runtime.channel_snapshots().await {
                    channels.push(ChannelSnapshot::native(&config, state, &running.runtime));
                }
                let metrics = running.metrics.snapshot(running.tracker.active_tasks());
                (
                    format!("http://{}", running.addr),
                    metrics.uptime_secs,
                    metrics.active_tasks,
                )
            } else {
                (String::new(), 0, 0)
            };
        let ingest_endpoint = if let Some(running) = &inner.running {
            running
                .runtime
                .ingest_addr(CHANNEL_ID)
                .await
                .map(|addr| format!("http://{addr}/"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        channels.extend(own_channels);
        channels.extend(recording_channels);
        Ok(Snapshot {
            backend_kind: if self.external.is_some() {
                "csharp"
            } else {
                "rust"
            }
            .into(),
            backend_label: self
                .external
                .as_ref()
                .map(|external| external.agent_name().to_owned())
                .unwrap_or_else(|| "Peercast RT".into()),
            running: inner.running.is_some(),
            endpoint,
            ingest_endpoint: if self.external.is_some() {
                String::new()
            } else {
                ingest_endpoint
            },
            config_path: self.config_path.display().to_string(),
            player_path: player_path.display().to_string(),
            player_available: player_path.is_file(),
            desktop: inner.desktop.clone(),
            uptime_secs,
            active_tasks,
            limits: inner.limits.clone(),
            yellow_pages: inner.yellow_pages.clone(),
            channels,
            last_probe: inner.last_probe.clone(),
            recordings,
            broadcasts,
            logs: self
                .log
                .snapshot()
                .into_iter()
                .map(|entry| GuiLogEntry {
                    timestamp_ms: entry.timestamp_ms,
                    level: format!("{:?}", entry.level),
                    target: entry.target,
                    message: entry.message,
                })
                .collect(),
        })
    }

    pub async fn get_yp_channels(&self, refresh: bool) -> Result<serde_json::Value, String> {
        self.check_open()?;
        let runtime = {
            let inner = self.inner.lock().await;
            Arc::clone(
                &inner
                    .running
                    .as_ref()
                    .ok_or("バックエンドを起動してください")?
                    .runtime,
            )
        };
        let rows = runtime.yellow_page_directory().channels(refresh).await?;
        let entries = rows
            .as_array()
            .ok_or("イエローページ一覧の形式が正しくありません")?;
        self.check_open()?;
        let mut inner = self.inner.lock().await;
        if !inner
            .running
            .as_ref()
            .is_some_and(|r| Arc::ptr_eq(&r.runtime, &runtime))
        {
            return Err("設定が変更されました。一覧を更新してください".into());
        }
        let decorated = decorate_channels(
            entries.clone(),
            &inner.desktop.channel_rules,
            &format!("http://{}", inner.running.as_ref().unwrap().addr),
        )?;
        inner.directory_channels = entries.clone();
        Ok(serde_json::Value::Array(decorated))
    }

    pub async fn preview_channel_rules(
        &self,
        rules: Vec<ChannelRule>,
    ) -> Result<RulePreview, String> {
        self.check_open()?;
        let inner = self.inner.lock().await;
        let mut desktop = inner.desktop.clone();
        desktop.channel_rules = rules;
        let desktop = desktop.normalize()?;
        let endpoint = inner
            .running
            .as_ref()
            .map(|running| format!("http://{}", running.addr))
            .unwrap_or_default();
        preview_rules(&inner.directory_channels, &desktop.channel_rules, &endpoint)
    }

    pub async fn rule_sound_path(&self, rule_id: &str) -> Result<Option<String>, String> {
        self.check_open()?;
        let inner = self.inner.lock().await;
        Ok(inner
            .desktop
            .channel_rules
            .iter()
            .find(|rule| rule.id == rule_id && rule.enabled)
            .and_then(|rule| (!rule.mute).then(|| rule.sound_path.clone()).flatten()))
    }

    pub async fn recording_settings(
        &self,
    ) -> Result<crate::recording_config::RecordingSettings, String> {
        self.check_open()?;
        Ok(self.inner.lock().await.desktop.recording.clone())
    }

    pub async fn recording_directory(&self) -> Result<Vec<serde_json::Value>, String> {
        let rows = self.get_yp_channels(false).await?;
        rows.as_array()
            .cloned()
            .ok_or_else(|| "イエローページ一覧の形式が正しくありません".into())
    }

    pub async fn enqueue_recording(
        &self,
        channel_id: String,
        yellow_page: String,
        keep: bool,
    ) -> Result<String, String> {
        self.check_open()?;
        self.recordings.enqueue(channel_id, yellow_page, keep).await
    }

    pub async fn control_recording(&self, job_id: &str, action: &str) -> Result<(), String> {
        self.check_open()?;
        self.recordings.control(job_id, action).await
    }

    pub async fn open_recording_source(
        &self,
        channel_id: &str,
        yellow_page: &str,
    ) -> Result<RecordingSource, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        let (limits, desktop, entry) = {
            let inner = self.inner.lock().await;
            if inner.running.is_none() {
                return Err("バックエンドを起動してください".into());
            }
            let entry = inner
                .directory_channels
                .iter()
                .find(|entry| {
                    entry["channelId"]
                        .as_str()
                        .is_some_and(|id| id.eq_ignore_ascii_case(channel_id))
                        && entry["yellowPage"].as_str() == Some(yellow_page)
                })
                .cloned()
                .ok_or("一覧を更新してチャンネルを選択してください")?;
            (inner.limits.clone(), inner.desktop.clone(), entry)
        };
        if channel_id.eq_ignore_ascii_case(CHANNEL_ID) {
            return Err("接続テスト用IDと重複するチャンネルは録画できません".into());
        }
        let endpoint: SocketAddr = entry["tracker"]
            .as_str()
            .unwrap_or_default()
            .parse()
            .map_err(|_| "チャンネルの接続先が正しくありません")?;
        if endpoint.port() == 0 || endpoint.ip().is_unspecified() || endpoint.ip().is_multicast() {
            return Err("録画用の接続先を持たない掲載情報です".into());
        }
        let name = entry["name"].as_str().unwrap_or("名称未設定").to_owned();
        let content_type = entry["contentType"].as_str().unwrap_or("RAW").to_owned();
        let title = entry["description"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| entry["trackTitle"].as_str())
            .unwrap_or_default()
            .to_owned();
        if let Some(external) = &self.external {
            return Ok(RecordingSource {
                stream_url: external.stream_url(channel_id, endpoint)?,
                name,
                content_type,
                title,
                running: None,
            });
        }
        let id = channel_id.to_ascii_uppercase();
        let config = ChannelConfig {
            channel_id: id.clone(),
            name: name.clone(),
            container_type: content_type.clone(),
            source: ChannelSource::Relay { endpoint },
            metadata: directory_metadata(&entry),
            ..Default::default()
        };
        let running = self.start(&limits, &[], &desktop).await?;
        if let Err(error) = running.runtime.add_relay_channel(config).await {
            self.stop_running(running).await?;
            return Err(error.to_string());
        }
        let stream_url = format!("http://{}/stream/{id}", running.addr);
        if let Err(error) = self.check_open() {
            self.stop_running(running).await?;
            return Err(error);
        }
        Ok(RecordingSource {
            stream_url,
            name,
            content_type,
            title,
            running: Some(running),
        })
    }

    async fn prepare_yp_channel_inner(
        &self,
        channel_id: &str,
        yellow_page: &str,
    ) -> Result<PreparedYpChannel, String> {
        let (running, entry) = {
            let inner = self.inner.lock().await;
            let running = if self.external.is_none() {
                Some(
                    inner
                        .running
                        .as_ref()
                        .ok_or("バックエンドを起動してください")?,
                )
            } else {
                None
            };
            let entry = inner
                .directory_channels
                .iter()
                .find(|entry| {
                    entry["channelId"]
                        .as_str()
                        .is_some_and(|id| id.eq_ignore_ascii_case(channel_id))
                        && entry["yellowPage"].as_str() == Some(yellow_page)
                })
                .cloned()
                .ok_or("一覧を更新してチャンネルを選択してください")?;
            (
                running.map(|running| (Arc::clone(&running.runtime), running.addr)),
                entry,
            )
        };
        if channel_id.eq_ignore_ascii_case(CHANNEL_ID) {
            return Err("接続テスト用IDと重複するチャンネルは視聴できません".into());
        }
        let endpoint: SocketAddr = entry["tracker"]
            .as_str()
            .unwrap_or_default()
            .parse()
            .map_err(|_| "チャンネルの接続先が正しくありません")?;
        if endpoint.port() == 0 || endpoint.ip().is_unspecified() || endpoint.ip().is_multicast() {
            return Err("視聴用の接続先を持たない掲載情報です".into());
        }
        if let Some(external) = &self.external {
            let playlist_url = external.stream_url(channel_id, endpoint)?;
            self.check_open()?;
            return Ok(PreparedYpChannel {
                playlist_url,
                channel: None,
                entry,
            });
        }
        let (runtime, addr) = running.ok_or("バックエンドを起動してください")?;
        let name = entry["name"].as_str().unwrap_or("名称未設定").to_owned();
        let config = ChannelConfig {
            channel_id: channel_id.to_ascii_uppercase(),
            name: name.clone(),
            container_type: entry["contentType"].as_str().unwrap_or("RAW").to_owned(),
            source: ChannelSource::Relay { endpoint },
            metadata: directory_metadata(&entry),
            ..Default::default()
        };
        let channel = runtime
            .add_relay_channel(config)
            .await
            .map_err(|e| e.to_string())?;
        self.check_open()?;
        Ok(PreparedYpChannel {
            playlist_url: format!("http://{addr}/pls/{}", channel_id.to_ascii_uppercase()),
            channel: Some(channel),
            entry,
        })
    }

    pub async fn prepare_yp_channel(
        &self,
        channel_id: &str,
        yellow_page: &str,
    ) -> Result<String, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        self.prepare_yp_channel_inner(channel_id, yellow_page)
            .await
            .map(|prepared| prepared.playlist_url)
    }

    pub async fn play_yp_channel(
        &self,
        channel_id: &str,
        yellow_page: &str,
    ) -> Result<PlayerLaunch, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        let prepared = self
            .prepare_yp_channel_inner(channel_id, yellow_page)
            .await?;
        if let Some(channel) = &prepared.channel {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !channel.snapshot().await.available {
                self.check_open()?;
                if Instant::now() >= deadline {
                    return Err(
                        "配信元への接続がまだ完了していません。状態を確認して再度視聴してください"
                            .into(),
                    );
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        } else if let Some(external) = &self.external {
            external.wait_for_media(&prepared.playlist_url).await?;
        } else {
            return Err("再生バックエンドが見つかりません".into());
        }
        self.check_open()?;
        let desktop = self.inner.lock().await.desktop.clone();
        let context = PlayerContext::from_yp(prepared.playlist_url, &prepared.entry)?;
        launch_configured_player(&desktop.player, context)
    }

    async fn replace(
        &self,
        limits: Limits,
        yellow_pages: Vec<YellowPageEntry>,
        desktop: DesktopPreferences,
    ) -> Result<(), String> {
        let next = self.start(&limits, &yellow_pages, &desktop).await?;
        if let Err(error) = ConfigManager::new(&self.config_path).save(&next.config) {
            self.stop_running(next).await?;
            return Err(error.to_string());
        }
        let previous = {
            let mut inner = self.inner.lock().await;
            inner.limits = limits;
            inner.yellow_pages = yellow_pages;
            inner.desktop = desktop;
            inner.directory_channels.clear();
            inner.last_probe = None;
            inner.running.replace(next)
        };
        if let Some(previous) = previous {
            self.stop_running(previous).await?;
        }
        Ok(())
    }

    pub async fn apply_limits(&self, limits: Limits) -> Result<Snapshot, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        let (yellow_pages, desktop) = {
            let inner = self.inner.lock().await;
            (inner.yellow_pages.clone(), inner.desktop.clone())
        };
        if self.external.is_some() && limits != self.inner.lock().await.limits {
            return Err("C# PeerCastStationの接続数・帯域設定はここから変更できません".into());
        }
        self.replace(limits, yellow_pages, desktop).await?;
        self.snapshot().await
    }

    pub async fn apply_preferences(
        &self,
        limits: Limits,
        yellow_pages: Vec<YellowPageEntry>,
    ) -> Result<Snapshot, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        let yellow_pages = normalize_yellow_pages(yellow_pages)?;
        let desktop = self.inner.lock().await.desktop.clone();
        if self.external.is_some() && limits != self.inner.lock().await.limits {
            return Err("C# PeerCastStationの接続数・帯域設定はここから変更できません".into());
        }
        self.replace(limits, yellow_pages, desktop).await?;
        self.snapshot().await
    }

    pub async fn restart(&self) -> Result<Snapshot, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        let (limits, yellow_pages, desktop) = {
            let inner = self.inner.lock().await;
            (
                inner.limits.clone(),
                inner.yellow_pages.clone(),
                inner.desktop.clone(),
            )
        };
        self.replace(limits, yellow_pages, desktop).await?;
        self.snapshot().await
    }

    pub async fn apply_yellow_pages(
        &self,
        yellow_pages: Vec<YellowPageEntry>,
    ) -> Result<Snapshot, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        let yellow_pages = normalize_yellow_pages(yellow_pages)?;
        let (limits, desktop) = {
            let inner = self.inner.lock().await;
            (inner.limits.clone(), inner.desktop.clone())
        };
        self.replace(limits, yellow_pages, desktop).await?;
        self.snapshot().await
    }

    pub async fn apply_desktop_preferences(
        &self,
        limits: Limits,
        yellow_pages: Vec<YellowPageEntry>,
        desktop: DesktopPreferences,
    ) -> Result<Snapshot, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        let yellow_pages = normalize_yellow_pages(yellow_pages)?;
        let desktop = desktop.normalize()?;
        let (old_limits, old_yellow_pages, active_config) = {
            let inner = self.inner.lock().await;
            (
                inner.limits.clone(),
                inner.yellow_pages.clone(),
                inner.running.as_ref().map(|running| running.config.clone()),
            )
        };
        if self.external.is_some() && limits != old_limits {
            return Err("C# PeerCastStationの接続数・帯域設定はここから変更できません".into());
        }
        if limits != old_limits || yellow_pages != old_yellow_pages {
            self.replace(limits, yellow_pages, desktop).await?;
        } else {
            let mut config = match active_config {
                Some(config) => config,
                None => ConfigManager::new(&self.config_path)
                    .load()
                    .map_err(|error| error.to_string())?,
            };
            config.desktop = serde_json::to_value(&desktop).map_err(|error| error.to_string())?;
            ConfigManager::new(&self.config_path)
                .save(&config)
                .map_err(|error| error.to_string())?;
            let mut inner = self.inner.lock().await;
            inner.desktop = desktop;
            if let Some(running) = inner.running.as_mut() {
                running.config.desktop = config.desktop;
            }
        }
        let recording_settings = self.inner.lock().await.desktop.recording.clone();
        self.recordings.update_settings(recording_settings).await;
        self.snapshot().await
    }

    async fn stop_inner(&self) -> Result<(), String> {
        let running = self.inner.lock().await.running.take();
        if let Some(running) = running {
            self.stop_running(running).await?;
        }
        Ok(())
    }

    pub async fn stop(&self) -> Result<Snapshot, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        self.stop_inner().await?;
        self.snapshot().await
    }

    pub fn begin_close(&self) -> bool {
        !self.closing.swap(true, Ordering::AcqRel)
    }

    pub async fn close(&self) -> Result<(), String> {
        self.closing.store(true, Ordering::Release);
        let recordings_result = self.recordings.shutdown().await;
        let broadcasts_result = self.broadcasts.shutdown().await;
        let _operation = self.operation.lock().await;
        let backend_result = self.stop_inner().await;
        recordings_result?;
        broadcasts_result?;
        backend_result
    }

    pub async fn run_probe(&self) -> Result<Snapshot, String> {
        let _operation = self.operation.lock().await;
        self.check_open()?;
        if self.external.is_some() {
            return Err(
                "C# PeerCastStation接続中はRustのHTTP Push疎通テストを実行できません".into(),
            );
        }
        let (runtime, channel, addr) = {
            let mut inner = self.inner.lock().await;
            inner.last_probe = None;
            let running = inner
                .running
                .as_ref()
                .ok_or("バックエンドを起動してください")?;
            (
                Arc::clone(&running.runtime),
                Arc::clone(&running.channel),
                running.addr,
            )
        };
        let started = Instant::now();
        let result = tokio::time::timeout(Duration::from_secs(8), probe(runtime, channel, addr))
            .await
            .map_err(|_| "配信の疎通テストがタイムアウトしました".to_string())
            .and_then(|result| result);
        match result {
            Ok(bytes) => {
                self.inner.lock().await.last_probe = Some(ProbeResult {
                    passed: true,
                    bytes,
                    elapsed_ms: started.elapsed().as_millis(),
                });
                self.log.log(
                    LogLevel::Info,
                    "desktop",
                    &format!("HTTP Push -> runtime -> HTTP playback verified: {bytes} bytes"),
                );
                self.snapshot().await
            }
            Err(error) => {
                self.inner.lock().await.last_probe = Some(ProbeResult {
                    passed: false,
                    bytes: 0,
                    elapsed_ms: started.elapsed().as_millis(),
                });
                self.log.log(LogLevel::Error, "desktop", &error);
                Err(error)
            }
        }
    }
}

async fn probe(
    runtime: Arc<AppRuntime>,
    channel: Arc<RuntimeChannel>,
    addr: SocketAddr,
) -> Result<usize, String> {
    let ingest = runtime
        .ingest_addr(CHANNEL_ID)
        .await
        .ok_or("HTTP Pushの待受がありません")?;
    let mut push = TcpStream::connect(ingest)
        .await
        .map_err(|error| error.to_string())?;
    push.write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    while !response.ends_with(b"\r\n\r\n") {
        if response.len() >= 8192 {
            return Err("HTTP Push応答が長すぎます".into());
        }
        response.push(push.read_u8().await.map_err(|error| error.to_string())?);
    }
    if !response.starts_with(b"HTTP/1.1 200") {
        return Err("HTTP Pushが拒否されました".into());
    }
    let payload = vec![b'B'; PROBE_CHUNK_BYTES];
    push.write_all(format!("{:x}\r\n", payload.len()).as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    push.write_all(&payload)
        .await
        .map_err(|error| error.to_string())?;
    push.write_all(b"\r\n")
        .await
        .map_err(|error| error.to_string())?;
    while !channel.snapshot().await.available {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(6))
        .build()
        .map_err(|error| error.to_string())?;
    let viewer = client
        .get(format!("http://{addr}/stream/{CHANNEL_ID}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !viewer.status().is_success() {
        return Err(format!("視聴接続が拒否されました: {}", viewer.status()));
    }
    for _ in 1..PROBE_CHUNKS {
        tokio::time::sleep(Duration::from_millis(100)).await;
        push.write_all(format!("{:x}\r\n", payload.len()).as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        push.write_all(&payload)
            .await
            .map_err(|error| error.to_string())?;
        push.write_all(b"\r\n")
            .await
            .map_err(|error| error.to_string())?;
    }
    push.write_all(b"0\r\n\r\n")
        .await
        .map_err(|error| error.to_string())?;
    push.shutdown().await.map_err(|error| error.to_string())?;
    let received = viewer.bytes().await.map_err(|error| error.to_string())?;
    if received.len() != PROBE_CHUNKS * PROBE_CHUNK_BYTES
        || !received.iter().all(|byte| *byte == b'B')
    {
        return Err(format!(
            "配信データが一致しません: {} bytes",
            received.len()
        ));
    }
    Ok(received.len())
}
