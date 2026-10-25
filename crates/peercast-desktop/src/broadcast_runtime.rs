use std::{
    future::{poll_fn, Future},
    net::SocketAddr,
    sync::Arc,
    task::Poll,
    time::Duration,
};

use crate::{backend::Limits, broadcast::BroadcastRequest};
use peercast_app::{
    runtime_config::{ChannelConfig, ChannelMetadata, ChannelSource, YellowPageSettings},
    AppConfig, AppMetrics, BoundedLogBuffer, HttpServer, LogLevel, ShutdownReason, TaskTracker,
};
use peercast_rpc::server::RpcState;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::{broadcast, oneshot, watch},
    task::JoinHandle,
    time::{timeout, Instant},
};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_STALL_TIMEOUT: Duration = Duration::from_secs(15);
const FORWARD_CHUNK_BYTES: usize = 16 * 1024;

/// Owns the local stream server and the single channel feeding it.
pub struct BroadcastRuntime {
    pub(crate) runtime: Arc<peercast_app::runtime::AppRuntime>,
    pub(crate) input_url: String,
    pub(crate) stream_url: String,
    pub(crate) http_addr: SocketAddr,
    ingest_addr: SocketAddr,
    server_shutdown: broadcast::Sender<ShutdownReason>,
    source_shutdown: watch::Sender<bool>,
    server_task: Option<JoinHandle<Result<(), String>>>,
    source_task: Option<JoinHandle<()>>,
    tracker: TaskTracker,
    closed: bool,
}

impl BroadcastRuntime {
    pub async fn start(
        request: &BroadcastRequest,
        channel_id: &str,
        broadcast_id: &str,
        limits: &Limits,
        log: Arc<BoundedLogBuffer>,
        sp_endpoint: Option<SocketAddr>,
    ) -> Result<Self, String> {
        let content_type = normalize_content_type(&request.content_type)?;
        let source_mode = request.source_mode.to_ascii_lowercase();
        if source_mode != "push" && source_mode != "pull" {
            return Err("sourceModeはpushまたはpullを指定してください".into());
        }
        let source_url = if source_mode == "pull" {
            let url = reqwest::Url::parse(&request.source_uri)
                .map_err(|_| "sourceUriはHTTP/HTTPS URLで指定してください")?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(
                    "sourceUriは認証情報やfragmentのないHTTP/HTTPS URLで指定してください".into(),
                );
            }
            Some(url)
        } else {
            None
        };

        let server_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("HTTP配信サーバーをbindできません: {error}"))?;
        let http_addr = server_listener
            .local_addr()
            .map_err(|error| error.to_string())?;
        let ingest_port = if source_mode == "push" {
            request.ingest_port
        } else {
            0
        };
        let ingest_addr = SocketAddr::from(([127, 0, 0, 1], ingest_port));

        let mut config = AppConfig::default();
        config.server.bind_address = "127.0.0.1".into();
        // The core uses this field as the announced PCP port; the HTTP listener itself is local/ephemeral.
        config.server.port = if request.relay_port == 0 {
            http_addr.port()
        } else {
            request.relay_port
        };
        apply_limits(limits, &mut config)?;
        config.server.allow_remote_admin = false;
        config.runtime.channels = vec![ChannelConfig {
            channel_id: channel_id.to_owned(),
            name: request.name.clone(),
            container_type: content_type.clone(),
            broadcast_id: Some(broadcast_id.to_owned()),
            metadata: ChannelMetadata {
                genre: request.genre.clone(),
                description: request.description.clone(),
                comment: request.comment.clone(),
                contact_url: request.contact_url.clone(),
                bitrate: request.bitrate,
            },
            source: ChannelSource::HttpPush {
                bind_address: ingest_addr,
                auth_token: None,
            },
        }];
        config.runtime.yellow_pages.clear();
        config.runtime.port_mapping.enabled = false;
        if request.announce_sp {
            let endpoint = sp_endpoint
                .ok_or_else(|| "SP announceが有効ですがendpointがありません".to_owned())?;
            let sp = YellowPageSettings {
                name: "SP".into(),
                endpoint,
                announcement_enabled: true,
                allow_public_announcements: true,
                ..YellowPageSettings::default()
            };
            config.runtime.yellow_pages.push(sp);
        }

        let tracker = TaskTracker::new();
        let server = HttpServer::new(
            &config,
            RpcState::default(),
            Arc::new(AppMetrics::new()),
            Arc::clone(&log),
            tracker.clone(),
        )
        .map_err(|error| format!("配信runtimeを作成できません: {error}"))?;
        let runtime = server.runtime();
        runtime
            .start()
            .await
            .map_err(|error| format!("配信入力を開始できません: {error}"))?;
        let ingest_addr = runtime
            .ingest_addr(channel_id)
            .await
            .ok_or_else(|| "HTTP Pushの入力先を取得できません".to_owned())?;
        let input_url = format!("http://{ingest_addr}/");
        let extension = if content_type == "WMV" { "wmv" } else { "flv" };
        let stream_url = format!("http://{http_addr}/stream/{channel_id}.{extension}");

        let (server_shutdown, server_shutdown_rx) = broadcast::channel(2);
        let (server_ready_tx, server_ready_rx) = oneshot::channel();
        let server_task = tokio::spawn(async move {
            let server_loop = server.run_on_listener(server_listener, server_shutdown_rx);
            tokio::pin!(server_loop);
            let mut ready = Some(server_ready_tx);
            let result = poll_fn(|context| match server_loop.as_mut().poll(context) {
                Poll::Pending => {
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Ok(()));
                    }
                    Poll::Pending
                }
                Poll::Ready(result) => {
                    if let Some(ready) = ready.take() {
                        let message = result
                            .as_ref()
                            .err()
                            .map(ToString::to_string)
                            .unwrap_or_else(|| "HTTP listener stopped during startup".into());
                        let _ = ready.send(Err(message));
                    }
                    Poll::Ready(result)
                }
            })
            .await;
            result.map_err(|error| error.to_string())
        });
        server_ready_rx
            .await
            .map_err(|_| "HTTP listenerの起動を確認できません".to_owned())??;

        let (source_shutdown, source_shutdown_rx) = watch::channel(false);
        let source_task = source_url.map(|url| {
            let input_addr = ingest_addr;
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                if let Err(error) = pull_source(url, input_addr, source_shutdown_rx).await {
                    log.log(
                        LogLevel::Warn,
                        "broadcast",
                        &format!("pull source ended: {error}"),
                    );
                }
            })
        });

        Ok(Self {
            runtime,
            input_url,
            stream_url,
            http_addr,
            ingest_addr,
            server_shutdown,
            source_shutdown,
            server_task: Some(server_task),
            source_task,
            tracker,
            closed: false,
        })
    }

    pub fn input_url(&self) -> &str {
        &self.input_url
    }

    pub fn stream_url(&self) -> &str {
        &self.stream_url
    }

    pub fn http_addr(&self) -> SocketAddr {
        self.http_addr
    }

    pub fn ingest_addr(&self) -> SocketAddr {
        self.ingest_addr
    }

    pub async fn close(&mut self) -> Result<(), String> {
        self.shutdown().await
    }

    pub async fn shutdown(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.source_shutdown.send_replace(true);
        let _ = self.server_shutdown.send(ShutdownReason::AdminRpc);

        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        let drain = async {
            self.runtime.shutdown().await;
            if let Some(task) = self.source_task.as_mut() {
                task.await.map_err(|error| error.to_string())?;
                self.source_task.take();
            }
            if let Some(task) = self.server_task.as_mut() {
                task.await.map_err(|error| error.to_string())??;
                self.server_task.take();
            }
            self.tracker
                .wait_for_drain(deadline.saturating_duration_since(Instant::now()))
                .await
                .map_err(|remaining| format!("配信workerが{remaining}件残っています"))?;
            Ok::<(), String>(())
        };
        match timeout(SHUTDOWN_TIMEOUT, drain).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                if let Some(task) = self.source_task.take() {
                    task.abort();
                }
                if let Some(task) = self.server_task.take() {
                    task.abort();
                }
                Err(error)
            }
            Err(_) => {
                if let Some(task) = self.source_task.take() {
                    task.abort();
                }
                if let Some(task) = self.server_task.take() {
                    task.abort();
                }
                Err("配信runtimeの終了が8秒以内に完了しませんでした".into())
            }
        }
    }
}

fn normalize_content_type(value: &str) -> Result<String, String> {
    match value.trim().to_ascii_uppercase().as_str() {
        "FLV" => Ok("FLV".into()),
        "WMV" => Ok("WMV".into()),
        _ => Err("contentTypeはFLVまたはWMVを指定してください".into()),
    }
}

fn apply_limits(limits: &Limits, config: &mut AppConfig) -> Result<(), String> {
    limits.apply(config)
}

async fn pull_source(
    source_url: reqwest::Url,
    ingest_addr: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let response = tokio::select! {
        changed = shutdown.changed() => {
            if changed.is_err() || *shutdown.borrow() { return Ok(()); }
            return Err("pull sourceのcancel状態が不正です".into());
        }
        response = timeout(IO_STALL_TIMEOUT, client.get(source_url).send()) => {
            response.map_err(|_| "pull sourceのHTTP header待ちがtimeoutしました".to_owned())?
                .map_err(|error| error.to_string())?
        }
    };
    if !response.status().is_success() {
        return Err(format!(
            "pull sourceがHTTP {}を返しました",
            response.status()
        ));
    }

    let mut socket = tokio::select! {
        changed = shutdown.changed() => {
            if changed.is_err() || *shutdown.borrow() { return Ok(()); }
            return Err("pull sourceのcancel状態が不正です".into());
        }
        result = timeout(CONNECT_TIMEOUT, TcpStream::connect(ingest_addr)) => {
            result.map_err(|_| "local ingest接続がtimeoutしました".to_owned())?
                .map_err(|error| error.to_string())?
        }
    };
    write_with_stall_timeout(
        &mut socket,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
    )
    .await?;

    let mut response = response;
    loop {
        let next = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return Ok(()); }
                continue;
            }
            next = timeout(IO_STALL_TIMEOUT, response.chunk()) => {
                next.map_err(|_| "pull sourceのbody待ちがtimeoutしました".to_owned())?
                    .map_err(|error| error.to_string())?
            }
        };
        let Some(bytes) = next else { break };
        for chunk in bytes.chunks(FORWARD_CHUNK_BYTES) {
            let prefix = format!("{:X}\r\n", chunk.len());
            write_with_stall_timeout(&mut socket, prefix.as_bytes()).await?;
            write_with_stall_timeout(&mut socket, chunk).await?;
            write_with_stall_timeout(&mut socket, b"\r\n").await?;
        }
    }
    write_with_stall_timeout(&mut socket, b"0\r\n\r\n").await?;
    Ok(())
}

async fn write_with_stall_timeout(socket: &mut TcpStream, bytes: &[u8]) -> Result<(), String> {
    timeout(IO_STALL_TIMEOUT, socket.write_all(bytes))
        .await
        .map_err(|_| "local ingestへの転送がtimeoutしました".to_owned())?
        .map_err(|error| error.to_string())
}
