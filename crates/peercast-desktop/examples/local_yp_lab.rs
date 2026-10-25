//! Local-only YP integration harness. It never contacts a public tracker.

use std::{
    env,
    future::{poll_fn, Future},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    task::Poll,
    time::Duration,
};

use bytes::Bytes;
use peercast_app::{
    config::AppConfig,
    lifecycle::{ShutdownReason, TaskTracker},
    logging::BoundedLogBuffer,
    metrics::AppMetrics,
    runtime_config::{ChannelConfig, ChannelMetadata, ChannelSource, YellowPageSettings},
    server::HttpServer,
};
use peercast_core::channel_id::ChannelId;
use peercast_core::AtomBudget;
use peercast_desktop::backend::{DesktopBackend, YellowPageEntry};
use peercast_pcp::probe::ProbeCodec;
use peercast_rpc::server::RpcState;
use serde::Serialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{broadcast, oneshot, watch},
    task::JoinHandle,
    time::{sleep, timeout, Instant},
};

const ROOT_ADDR: &str = "127.0.0.1:17146";
const SOURCE_HTTP_ADDR: &str = "127.0.0.1:17144";
const PUSH_ADDR: &str = "127.0.0.1:18081";
const INDEX_ADDR: &str = "127.0.0.1:18700";
const RECEIVER_ADDR: &str = "127.0.0.1:17148";
const CHANNEL_ID: &str = "31aafc0e4f3d4a2bb12a6a7c0e8f1234";
const BROADCAST_ID: &str = "58c2b0f1aef149739c4511ea0f5d1234";
const CHANNEL_NAME: &str = "Local YP Lab Synthetic FLV";
const GENRE: &str = "local-lab";
const DESCRIPTION: &str = "synthetic silent PCM fixture";
const COMMENT: &str = "loopback experiment only";
const CONTACT: &str = "http://127.0.0.1/local-lab";
const MEDIA_LIMIT: usize = 16 * 1024;
const MAX_INDEX_REQUEST_BYTES: usize = 8 * 1024;

#[derive(Debug)]
struct Args {
    blocked_port: bool,
    root_url: String,
    root_user: String,
    root_password: String,
    converter: PathBuf,
}

#[derive(Serialize)]
struct Report {
    mode: &'static str,
    root_url: String,
    source_listener: String,
    advertised_port: u16,
    push_listener: String,
    channel_id: &'static str,
    broadcast_id: &'static str,
    listed_while_streaming: bool,
    index_host: Option<String>,
    relay_probe_source: String,
    relay_probe_advertised: String,
    receiver_media_bytes: usize,
    receiver_result: String,
    removed_after_stop: bool,
    root_probe_note: &'static str,
}

struct RunningServer {
    runtime: Arc<peercast_app::runtime::AppRuntime>,
    shutdown: broadcast::Sender<ShutdownReason>,
    task: JoinHandle<Result<(), String>>,
    tracker: TaskTracker,
}

impl RunningServer {
    async fn stop(self) -> Result<(), String> {
        self.runtime.shutdown().await;
        let _ = self.shutdown.send(ShutdownReason::AdminRpc);
        timeout(Duration::from_secs(3), self.task)
            .await
            .map_err(|_| "HTTP server did not stop within 3 seconds".to_string())?
            .map_err(|e| e.to_string())??;
        self.tracker
            .wait_for_drain(Duration::from_secs(3))
            .await
            .map_err(|n| format!("{n} runtime task(s) did not drain"))
    }
}

struct Lab {
    root_client: reqwest::Client,
    args: Args,
    evidence_dir: PathBuf,
    source: Option<RunningServer>,
    receiver: Option<RunningServer>,
    producer_stop: watch::Sender<bool>,
    producer: Option<JoinHandle<Result<(), String>>>,
    index_stop: watch::Sender<bool>,
    index_content: Arc<tokio::sync::RwLock<Vec<u8>>>,
    index_task: Option<JoinHandle<()>>,
    directory_backend: Option<Arc<DesktopBackend>>,
    receiver_addr: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    let mode = if args.blocked_port { "blocked" } else { "open" };
    let evidence_dir = PathBuf::from("target/local-yp-experiment").join(format!("evidence-{mode}"));
    tokio::fs::create_dir_all(&evidence_dir).await?;
    let report_path = evidence_dir.join("report.json");
    tokio::fs::write(
        &report_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "mode": mode,
            "outcome": "running",
            "rootUrl": args.root_url,
            "startedAt": chrono::Utc::now().to_rfc3339(),
        }))?,
    )
    .await?;

    let root_url = reqwest::Url::parse(&args.root_url)?;
    if root_url.scheme() != "http"
        || root_url.host_str() != Some("127.0.0.1")
        || root_url.port().is_none()
        || !root_url.username().is_empty()
        || root_url.password().is_some()
    {
        return Err("root URL must be an explicit loopback HTTP URL".into());
    }
    let root_client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let (producer_stop, producer_rx) = watch::channel(false);
    let (index_stop, index_rx) = watch::channel(false);
    let index_listener = TcpListener::bind(INDEX_ADDR).await?;
    let index_content = Arc::new(tokio::sync::RwLock::new(Vec::new()));
    let index_task = tokio::spawn(serve_index(
        index_listener,
        index_rx,
        Arc::clone(&index_content),
    ));
    let mut lab = Lab {
        root_client,
        args,
        evidence_dir,
        source: None,
        receiver: None,
        producer_stop,
        producer: None,
        index_stop,
        index_content,
        index_task: Some(index_task),
        directory_backend: None,
        receiver_addr: RECEIVER_ADDR.parse()?,
    };

    let outcome = lab.run(producer_rx).await;
    let cleanup = lab.cleanup().await;
    let run_error = outcome.err().map(|error| error.to_string());
    let cleanup_errors = cleanup.err().into_iter().collect::<Vec<_>>();
    let mut report = tokio::fs::read(&report_path)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or_else(|| serde_json::json!({ "mode": mode }));
    report["outcome"] = serde_json::json!(if run_error.is_none() && cleanup_errors.is_empty() {
        "completed"
    } else {
        "failed"
    });
    report["cleanupErrors"] = serde_json::json!(cleanup_errors);
    if let Some(error) = &run_error {
        report["failure"] = serde_json::json!(error);
    }
    tokio::fs::write(&report_path, serde_json::to_vec_pretty(&report)?).await?;
    if run_error.is_some() || !cleanup_errors.is_empty() {
        return Err(format!(
            "{}{}",
            run_error.unwrap_or_else(|| "harness completed with cleanup errors".into()),
            cleanup_errors
                .first()
                .map(|error| format!("; cleanup: {error}"))
                .unwrap_or_default()
        )
        .into());
    }
    Ok(())
}

impl Lab {
    async fn run(
        &mut self,
        producer_rx: watch::Receiver<bool>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let source_listener = TcpListener::bind(SOURCE_HTTP_ADDR).await?;
        let source_listener_addr = source_listener.local_addr()?;
        let advertised_port = if self.args.blocked_port {
            source_listener_addr.port() + 1
        } else {
            source_listener_addr.port()
        };
        if self.args.blocked_port && !port_is_bindable(advertised_port).await {
            return Err(
                format!("expected advertised test port {advertised_port} to be closed").into(),
            );
        }
        let mut config = AppConfig::default();
        config.server.bind_address = "127.0.0.1".into();
        config.server.port = advertised_port;
        config.server.allow_remote_admin = false;
        config.runtime.port_mapping.enabled = false;
        config.runtime.channels = vec![ChannelConfig {
            channel_id: CHANNEL_ID.into(),
            name: CHANNEL_NAME.into(),
            container_type: "FLV".into(),
            broadcast_id: Some(BROADCAST_ID.into()),
            metadata: ChannelMetadata {
                genre: GENRE.into(),
                description: DESCRIPTION.into(),
                comment: COMMENT.into(),
                contact_url: CONTACT.into(),
                bitrate: 128,
            },
            source: ChannelSource::HttpPush {
                bind_address: PUSH_ADDR.parse()?,
                auth_token: None,
            },
        }];
        config.runtime.yellow_pages = vec![YellowPageSettings {
            name: "Local experiment root".into(),
            endpoint: ROOT_ADDR.parse()?,
            update_interval_secs: 1,
            retry_base_ms: 250,
            retry_max_ms: 1000,
            io_timeout_ms: 2000,
            allow_public_announcements: false,
            announcement_enabled: true,
            ..YellowPageSettings::default()
        }];
        self.source = Some(start_server(config, source_listener).await?);
        self.producer = Some(tokio::spawn(push_synthetic_media(producer_rx)));

        let first_xml = self
            .wait_for_xml(
                true,
                if self.args.blocked_port {
                    Duration::from_secs(12)
                } else {
                    Duration::from_secs(60)
                },
            )
            .await?;
        tokio::fs::write(self.evidence_dir.join("root-open.xml"), &first_xml).await?;
        let root_listed = channel_has_host(&first_xml);
        if root_listed {
            validate_xml_metadata(&first_xml)?;
        }
        let index = run_converter(&self.args.converter, &first_xml).await?;
        tokio::fs::write(self.evidence_dir.join("index.txt"), &index).await?;
        let hosted_channel = parse_channel_line(&index, CHANNEL_ID);
        *self.index_content.write().await = index.clone();
        let index_host = hosted_channel.as_ref().map(|fields| fields[2].to_string());
        let index_response = self
            .root_client
            .get(format!("http://{INDEX_ADDR}/index.txt"))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        tokio::fs::write(self.evidence_dir.join("index-capture.txt"), &index_response).await?;
        if hosted_channel.is_none() && !self.args.blocked_port {
            self.write_early_report(
                "converter index did not contain the channel after waiting for a host; inspect root-open.xml and wait-diagnostics.log",
                &first_xml,
                &index,
            )
            .await?;
            return Err(
                "converter index omitted the expected channel after the bounded listing wait"
                    .into(),
            );
        }
        if let Some(fields) = parse_channel_line(&index_response, CHANNEL_ID) {
            validate_index_fields(
                &fields,
                if self.args.blocked_port {
                    None
                } else {
                    Some(17144)
                },
            )?;
        } else if !self.args.blocked_port {
            return Err("open-port index omitted the expected channel".into());
        }
        self.verify_product_directory().await?;

        let probe_session = ChannelId::from_hex("f0b1c2d3e4f5061728394a5b6c7d8e9f")?;
        let source_probe = probe_peer(source_listener_addr, probe_session).await;
        let advertised_probe_addr = SocketAddr::from(([127, 0, 0, 1], advertised_port));
        let advertised_probe = probe_peer(advertised_probe_addr, probe_session).await;
        let receiver_tracker = hosted_channel
            .as_ref()
            .map(|f| f[2].to_string())
            .unwrap_or_else(|| advertised_probe_addr.to_string());
        let (receiver_result, receiver_media_bytes) = self.run_receiver(&receiver_tracker).await?;
        if self.args.blocked_port && receiver_media_bytes != 0 {
            return Err(
                "receiver unexpectedly received media from the closed advertised port".into(),
            );
        }
        if !self.args.blocked_port && receiver_media_bytes < MEDIA_LIMIT {
            return Err(format!(
                "open-port receiver got only {receiver_media_bytes} bytes: {receiver_result}"
            )
            .into());
        }
        if let Some(receiver) = self.receiver.take() {
            receiver.stop().await.map_err(std::io::Error::other)?;
        }

        let _ = self.producer_stop.send(true);
        if let Some(producer) = self.producer.take() {
            timeout(Duration::from_secs(3), producer)
                .await??
                .map_err(std::io::Error::other)?;
        }
        if let Some(source) = self.source.take() {
            source.stop().await.map_err(std::io::Error::other)?;
        }
        let final_xml = self.wait_for_xml(false, Duration::from_secs(45)).await?;
        tokio::fs::write(self.evidence_dir.join("root-closed.xml"), &final_xml).await?;
        let final_index = run_converter(&self.args.converter, &final_xml).await?;
        tokio::fs::write(self.evidence_dir.join("index-closed.txt"), &final_index).await?;
        *self.index_content.write().await = final_index;
        let served_final = self
            .root_client
            .get(format!("http://{INDEX_ADDR}/index.txt"))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        tokio::fs::write(
            self.evidence_dir.join("index-closed-capture.txt"),
            &served_final,
        )
        .await?;
        let removed_after_stop = parse_channel_line(&served_final, CHANNEL_ID).is_none();

        let report = Report {
            mode: if self.args.blocked_port {
                "blocked"
            } else {
                "open"
            },
            root_url: self.args.root_url.clone(),
            source_listener: source_listener_addr.to_string(),
            advertised_port,
            push_listener: PUSH_ADDR.into(),
            channel_id: CHANNEL_ID,
            broadcast_id: BROADCAST_ID,
            listed_while_streaming: root_listed && hosted_channel.is_some(),
            index_host,
            relay_probe_source: format!("{}: {source_probe}", source_listener_addr),
            relay_probe_advertised: format!("{}: {advertised_probe}", advertised_probe_addr),
            receiver_media_bytes,
            receiver_result,
            removed_after_stop,
            root_probe_note: "YT root reverse probing requires PCP PING; this client sends no PING, so index behavior is recorded without assuming an automatic port check.",
        };
        tokio::fs::write(
            self.evidence_dir.join("report.json"),
            serde_json::to_vec_pretty(&report)?,
        )
        .await?;
        if !removed_after_stop {
            return Err("channel remained in converted index after source stopped".into());
        }
        Ok(())
    }

    async fn wait_for_xml(
        &mut self,
        should_exist: bool,
        max_wait: Duration,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + max_wait;
        let started = Instant::now();
        let mut next_diagnostic = started + Duration::from_secs(5);
        let mut last = String::new();
        while Instant::now() < deadline {
            if should_exist {
                if self.producer.as_ref().is_some_and(JoinHandle::is_finished) {
                    let task = self.producer.take().expect("producer handle exists");
                    return match task.await {
                        Ok(Err(error)) => Err(format!(
                            "synthetic media producer failed before YP host listing: {error}"
                        )
                        .into()),
                        Ok(Ok(())) => {
                            Err("synthetic media producer ended before requested stop".into())
                        }
                        Err(error) => {
                            Err(format!("synthetic media producer task failed: {error}").into())
                        }
                    };
                }
                if Instant::now() >= next_diagnostic {
                    self.log_wait_diagnostic(started.elapsed()).await?;
                    next_diagnostic += Duration::from_secs(5);
                }
            }
            match self.get_viewxml().await {
                Ok(xml) => {
                    let has_host = channel_has_host(&xml);
                    if has_host == should_exist {
                        return Ok(xml);
                    }
                    last = xml;
                }
                Err(error) => last = format!("viewxml request failed: {error}"),
            }
            sleep(Duration::from_millis(500)).await;
        }
        if !last.is_empty() {
            return Ok(last);
        }
        Err(format!(
            "timed out waiting for channel existence={should_exist}; last response: {}",
            &last[..last.len().min(512)]
        )
        .into())
    }

    async fn log_wait_diagnostic(&self, elapsed: Duration) -> Result<(), std::io::Error> {
        let Some(runtime) = self.source.as_ref().map(|source| &source.runtime) else {
            return Ok(());
        };
        let Some(channel) = runtime.channel(CHANNEL_ID) else {
            return Ok(());
        };
        let snapshot = channel.snapshot().await;
        let yp_status = runtime.yellow_page_statuses().await;
        let producer_finished = self.producer.as_ref().is_none_or(JoinHandle::is_finished);
        let line = format!(
            "t={}s available={} bufferedBytes={} yp={:?} producerFinished={}\n",
            elapsed.as_secs(),
            snapshot.available,
            snapshot.buffered_bytes,
            yp_status,
            producer_finished
        );
        eprint!("{line}");
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.evidence_dir.join("wait-diagnostics.log"))
            .await?;
        file.write_all(line.as_bytes()).await
    }

    async fn write_early_report(
        &self,
        reason: &str,
        xml: &str,
        index: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let report = serde_json::json!({
            "mode": if self.args.blocked_port { "blocked" } else { "open" },
            "outcome": "failed",
            "reason": reason,
            "rootUrl": self.args.root_url,
            "sourceListener": SOURCE_HTTP_ADDR,
            "advertisedPort": if self.args.blocked_port { 17145 } else { 17144 },
            "pushListener": PUSH_ADDR,
            "channelId": CHANNEL_ID,
            "broadcastId": BROADCAST_ID,
            "xmlBytes": xml.len(),
            "converterIndexBytes": index.len(),
            "diagnosticsFile": "wait-diagnostics.log",
        });
        tokio::fs::write(
            self.evidence_dir.join("report.json"),
            serde_json::to_vec_pretty(&report)?,
        )
        .await?;
        Ok(())
    }

    async fn get_viewxml(&self) -> Result<String, Box<dyn std::error::Error>> {
        let response = timeout(
            Duration::from_secs(4),
            self.root_client
                .get(&self.args.root_url)
                .basic_auth(&self.args.root_user, Some(&self.args.root_password))
                .send(),
        )
        .await
        .map_err(|_| "root viewxml request timeout")??;
        Ok(response.error_for_status()?.text().await?)
    }

    async fn run_receiver(
        &mut self,
        tracker: &str,
    ) -> Result<(String, usize), Box<dyn std::error::Error>> {
        let endpoint: SocketAddr = tracker.parse()?;
        let listener = TcpListener::bind(self.receiver_addr).await?;
        let mut config = AppConfig::default();
        config.server.bind_address = "127.0.0.1".into();
        config.server.port = listener.local_addr()?.port();
        config.server.allow_remote_admin = false;
        config.runtime.port_mapping.enabled = false;
        config.runtime.channels = vec![ChannelConfig {
            channel_id: CHANNEL_ID.into(),
            name: "Local YP Lab Receiver".into(),
            container_type: "FLV".into(),
            source: ChannelSource::Relay { endpoint },
            ..ChannelConfig::default()
        }];
        self.receiver = Some(start_server(config, listener).await?);
        if !self.args.blocked_port {
            let runtime = &self.receiver.as_ref().unwrap().runtime;
            timeout(Duration::from_secs(8), async {
                while !runtime
                    .channel(CHANNEL_ID)
                    .unwrap()
                    .snapshot()
                    .await
                    .available
                {
                    sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .map_err(|_| "receiver did not obtain relay media within eight seconds")?;
        }
        let url = format!("http://{RECEIVER_ADDR}/stream/{CHANNEL_ID}.flv");
        let response =
            match timeout(Duration::from_secs(10), self.root_client.get(url).send()).await {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => return Ok((format!("HTTP request failed: {error}"), 0)),
                Err(_) => return Ok(("HTTP request timed out".into(), 0)),
            };
        if !response.status().is_success() {
            return Ok((format!("HTTP {} from receiver", response.status()), 0));
        }
        let mut body = response.bytes_stream();
        let mut capture = Vec::with_capacity(MEDIA_LIMIT);
        while capture.len() < MEDIA_LIMIT {
            let chunk = match timeout(
                Duration::from_secs(15),
                futures_util::StreamExt::next(&mut body),
            )
            .await
            {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(error))) => {
                    return Ok((
                        format!("receiver stream read failed: {error}"),
                        capture.len(),
                    ));
                }
                Ok(None) => {
                    return Ok((
                        format!("receiver stream ended at {} bytes", capture.len()),
                        capture.len(),
                    ));
                }
                Err(_) => {
                    return Ok((
                        format!("receiver stream stalled at {} bytes", capture.len()),
                        capture.len(),
                    ));
                }
            };
            let remaining = MEDIA_LIMIT - capture.len();
            capture.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }
        tokio::fs::write(self.evidence_dir.join("receiver.flv"), &capture).await?;
        Ok(("received-at-least-16KiB".into(), capture.len()))
    }

    async fn verify_product_directory(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let parser_config_path = self.evidence_dir.join("directory-backend.json");
        let mut config = AppConfig::default();
        config.server.bind_address = "127.0.0.1".into();
        config.server.port = ephemeral_port().await?;
        config.server.allow_remote_admin = false;
        config.runtime.yellow_pages.clear();
        config.runtime.port_mapping.enabled = false;
        tokio::fs::write(&parser_config_path, serde_json::to_vec_pretty(&config)?).await?;
        let backend = DesktopBackend::open(parser_config_path)
            .await
            .map_err(std::io::Error::other)?;
        self.directory_backend = Some(Arc::clone(&backend));
        backend
            .apply_yellow_pages(vec![YellowPageEntry {
                name: "Local YP Lab index".into(),
                channels_url: format!("http://{INDEX_ADDR}/index.txt"),
            }])
            .await
            .map_err(std::io::Error::other)?;
        let rows = backend
            .get_yp_channels(true)
            .await
            .map_err(std::io::Error::other)?;
        tokio::fs::write(
            self.evidence_dir.join("product-directory.json"),
            serde_json::to_vec_pretty(&rows)?,
        )
        .await?;
        let matching = rows.as_array().and_then(|rows| {
            rows.iter().find(|row| {
                row.get("channelId")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|id| id.eq_ignore_ascii_case(CHANNEL_ID))
            })
        });
        match matching {
            Some(row) => {
                for (key, expected) in [
                    ("name", CHANNEL_NAME),
                    ("genre", GENRE),
                    ("description", DESCRIPTION),
                    ("comment", COMMENT),
                    ("contentType", "FLV"),
                ] {
                    if row.get(key).and_then(serde_json::Value::as_str) != Some(expected) {
                        return Err(
                            format!("product directory parser field {key} mismatched").into()
                        );
                    }
                }
            }
            None if self.args.blocked_port => {}
            None => return Err("product directory parser did not return the listed channel".into()),
        }
        if let Some(backend) = self.directory_backend.take() {
            backend.close().await.map_err(std::io::Error::other)?;
        }
        Ok(())
    }

    async fn cleanup(&mut self) -> Result<(), String> {
        let mut failures = Vec::new();
        let _ = self.producer_stop.send(true);
        if let Some(producer) = self.producer.take() {
            if let Err(error) = timeout(Duration::from_secs(3), producer).await {
                failures.push(format!("producer timeout: {error}"));
            }
        }
        if let Some(receiver) = self.receiver.take() {
            if let Err(error) = receiver.stop().await {
                failures.push(format!("receiver shutdown: {error}"));
            }
        }
        if let Some(backend) = self.directory_backend.take() {
            if let Err(error) = backend.close().await {
                failures.push(format!("directory backend shutdown: {error}"));
            }
        }
        if let Some(source) = self.source.take() {
            if let Err(error) = source.stop().await {
                failures.push(format!("source shutdown: {error}"));
            }
        }
        let _ = self.index_stop.send(true);
        if let Some(task) = self.index_task.take() {
            if timeout(Duration::from_secs(2), task).await.is_err() {
                failures.push("index server shutdown timeout".into());
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

async fn start_server(
    config: AppConfig,
    listener: TcpListener,
) -> Result<RunningServer, Box<dyn std::error::Error>> {
    let tracker = TaskTracker::new();
    let log = Arc::new(BoundedLogBuffer::default());
    let server = HttpServer::new(
        &config,
        RpcState::default(),
        Arc::new(AppMetrics::new()),
        log,
        tracker.clone(),
    )?;
    let runtime = server.runtime();
    runtime.start().await?;
    let (shutdown, rx) = broadcast::channel(2);
    let (ready_tx, ready_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let server_loop = server.run_on_listener(listener, rx);
        tokio::pin!(server_loop);
        let mut ready = Some(ready_tx);
        let result = poll_fn(|context| match server_loop.as_mut().poll(context) {
            Poll::Pending => {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Ok(()));
                }
                Poll::Pending
            }
            Poll::Ready(result) => {
                if let Some(ready) = ready.take() {
                    let error = result
                        .as_ref()
                        .err()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "HTTP listener stopped during startup".into());
                    let _ = ready.send(Err(error));
                }
                Poll::Ready(result)
            }
        })
        .await;
        result.map_err(|e| e.to_string())
    });
    timeout(Duration::from_secs(3), ready_rx)
        .await
        .map_err(|_| "HTTP server startup timed out")?
        .map_err(|_| "HTTP server startup signal was dropped")??;
    Ok(RunningServer {
        runtime,
        shutdown,
        task,
        tracker,
    })
}

async fn push_synthetic_media(mut stop: watch::Receiver<bool>) -> Result<(), String> {
    let mut socket = timeout(Duration::from_secs(5), TcpStream::connect(PUSH_ADDR))
        .await
        .map_err(|_| "HTTP Push connect timeout".to_string())?
        .map_err(|e| e.to_string())?;
    socket.write_all(b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n")
        .await.map_err(|e| e.to_string())?;
    write_chunk(&mut socket, &flv_header()).await?;
    let mut timestamp = 0u32;
    loop {
        if *stop.borrow() {
            break;
        }
        let tag = pcm_audio_tag(timestamp);
        tokio::select! {
            biased;
            changed = stop.changed() => { if changed.is_err() || *stop.borrow() { break; } }
            _ = sleep(Duration::from_millis(40)) => {
                write_chunk(&mut socket, &tag).await?;
                timestamp = timestamp.wrapping_add(40);
            }
        }
    }
    socket
        .write_all(b"0\r\n\r\n")
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn write_chunk(socket: &mut TcpStream, data: &[u8]) -> Result<(), String> {
    socket
        .write_all(format!("{:X}\r\n", data.len()).as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    socket.write_all(data).await.map_err(|e| e.to_string())?;
    socket.write_all(b"\r\n").await.map_err(|e| e.to_string())
}

fn flv_header() -> Vec<u8> {
    b"FLV\x01\x04\0\0\0\x09\0\0\0\0".to_vec()
}

fn pcm_audio_tag(timestamp: u32) -> Vec<u8> {
    let payload_len = 883u32; // Audio format byte plus 441 zero-valued 16-bit samples.
    let mut tag = vec![
        8,
        0,
        (payload_len >> 8) as u8,
        payload_len as u8,
        (timestamp >> 16) as u8,
        (timestamp >> 8) as u8,
        timestamp as u8,
        (timestamp >> 24) as u8,
        0,
        0,
        0,
        0x36,
    ];
    tag.resize(11 + payload_len as usize, 0);
    tag.extend_from_slice(&(11 + payload_len).to_be_bytes());
    tag
}

async fn run_converter(
    executable: &Path,
    xml: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut child = Command::new(executable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("converter stdin unavailable")?;
    stdin.write_all(xml.as_bytes()).await?;
    drop(stdin);
    let output = timeout(Duration::from_secs(60), child.wait_with_output()).await??;
    if !output.status.success() {
        return Err(format!("converter exited with {}", output.status).into());
    }
    Ok(output.stdout)
}

async fn serve_index(
    listener: TcpListener,
    mut stop: watch::Receiver<bool>,
    content: Arc<tokio::sync::RwLock<Vec<u8>>>,
) {
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            accepted = listener.accept() => {
                if let Ok((mut socket, _)) = accepted {
                    let mut request = Vec::with_capacity(1024);
                    let read_result = timeout(Duration::from_secs(2), async {
                        let mut chunk = [0u8; 1024];
                        loop {
                            let count = socket.read(&mut chunk).await?;
                            if count == 0 { break; }
                            if request.len().saturating_add(count) > MAX_INDEX_REQUEST_BYTES {
                                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "HTTP request headers exceed 8 KiB"));
                            }
                            request.extend_from_slice(&chunk[..count]);
                            if request.windows(4).any(|window| window == b"\r\n\r\n") { break; }
                        }
                        Ok::<(), std::io::Error>(())
                    }).await;
                    if !matches!(read_result, Ok(Ok(()))) {
                        let _ = socket.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        let _ = socket.shutdown().await;
                        continue;
                    }
                    let body = content.read().await.clone();
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                }
            }
        }
    }
}

async fn probe_peer(address: SocketAddr, session: ChannelId) -> String {
    let result: Result<Option<ChannelId>, Box<dyn std::error::Error>> = async {
        let mut stream = timeout(Duration::from_secs(2), TcpStream::connect(address)).await??;
        let request = ProbeCodec::build_probe_init(session, "local-yp-lab")?;
        stream.write_all(&request).await?;
        let mut bytes = vec![0u8; 4096];
        let count = timeout(Duration::from_secs(2), stream.read(&mut bytes)).await??;
        bytes.truncate(count);
        let mut response = Bytes::from(bytes);
        let server_session = ProbeCodec::parse_probe_oleh(&mut response, &AtomBudget::default())?;
        if server_session.is_some() {
            let quit = ProbeCodec::build_probe_quit()?;
            let _ = stream.write_all(&quit).await;
        }
        Ok(server_session)
    }
    .await;
    match result {
        Ok(Some(server_session)) => format!(
            "PCP OLEH received (server session {})",
            server_session.to_hex_lower()
        ),
        Ok(None) => "PCP OLEH incomplete".into(),
        Err(error) => format!("probe failed: {error}"),
    }
}

async fn port_is_bindable(port: u16) -> bool {
    match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(_) => false,
    }
}

async fn ephemeral_port() -> Result<u16, std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    Ok(listener.local_addr()?.port())
}

fn parse_channel_line<'a>(index: &'a [u8], channel_id: &str) -> Option<Vec<&'a str>> {
    let text = std::str::from_utf8(index).ok()?;
    text.lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split("<>").collect();
            fields
                .get(1)
                .is_some_and(|id| id.eq_ignore_ascii_case(channel_id))
                .then_some(fields)
        })
        .next()
}

fn validate_index_fields(
    fields: &[&str],
    expected_tracker_port: Option<u16>,
) -> Result<(), Box<dyn std::error::Error>> {
    for (field, expected) in [
        (0, CHANNEL_NAME),
        (3, CONTACT),
        (4, GENRE),
        (5, DESCRIPTION),
        (8, "128"),
        (9, "FLV"),
        (17, COMMENT),
    ] {
        if fields.get(field).copied() != Some(expected) {
            return Err(format!("index field {field} mismatch: {:?}", fields.get(field)).into());
        }
    }
    if let Some(expected_port) = expected_tracker_port {
        if !fields
            .get(2)
            .is_some_and(|host| host.ends_with(&format!(":{expected_port}")))
        {
            return Err(
                format!("index tracker did not advertise expected port {expected_port}").into(),
            );
        }
    }
    Ok(())
}

fn validate_xml_metadata(xml: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !xml
        .to_ascii_lowercase()
        .contains(&CHANNEL_ID.to_ascii_lowercase())
    {
        return Err("root viewxml omitted expected channel ID".into());
    }
    for expected in [CHANNEL_NAME, GENRE, DESCRIPTION, COMMENT, CONTACT] {
        if !xml.contains(expected) {
            return Err(format!("root viewxml omitted expected value: {expected}").into());
        }
    }
    Ok(())
}

fn channel_has_host(xml: &str) -> bool {
    let lower = xml.to_ascii_lowercase();
    let id_at = match lower.find(&format!("id=\"{}\"", CHANNEL_ID.to_ascii_lowercase())) {
        Some(index) => index,
        None => return false,
    };
    let channel_start = match lower[..id_at].rfind("<channel ") {
        Some(index) => index,
        None => return false,
    };
    let channel_end = lower[id_at..]
        .find("</channel>")
        .map(|offset| id_at + offset)
        .unwrap_or(lower.len());
    lower[channel_start..channel_end].contains("<host ")
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut args = Args {
        blocked_port: false,
        root_url: format!("http://{ROOT_ADDR}/admin?cmd=viewxml"),
        root_user: "admin".into(),
        root_password: "local-yp-only".into(),
        converter: PathBuf::from(
            "target/local-yp-experiment/local-index/target/debug/local-index.exe",
        ),
    };
    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--blocked-port" => args.blocked_port = true,
            "--root-url" => args.root_url = it.next().ok_or("--root-url requires a URL")?,
            "--root-user" => args.root_user = it.next().ok_or("--root-user requires a value")?,
            "--root-password" => {
                args.root_password = it.next().ok_or("--root-password requires a value")?
            }
            "--converter" => {
                args.converter = PathBuf::from(it.next().ok_or("--converter requires a path")?)
            }
            "--help" | "-h" => {
                println!(
                    "local_yp_lab [--blocked-port] [--root-url URL] [--root-user USER] [--root-password PASS] [--converter PATH]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    Ok(args)
}
