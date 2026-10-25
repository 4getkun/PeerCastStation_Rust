use crate::{
    backend::{ChannelSnapshot, Limits},
    broadcast_runtime::BroadcastRuntime,
    csharp_backend::CSharpBackend,
};
use peercast_app::BoundedLogBuffer;
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
    task::JoinHandle,
};

pub const SP_ANNOUNCE_URI: &str = "pcp://bayonet.ddo.jp:7146/";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct BroadcastRequest {
    pub name: String,
    pub genre: String,
    pub description: String,
    pub comment: String,
    pub contact_url: String,
    pub content_type: String,
    pub bitrate: u32,
    pub source_mode: String,
    pub source_uri: String,
    pub ingest_port: u16,
    pub relay_port: u16,
    pub announce_sp: bool,
}
impl Default for BroadcastRequest {
    fn default() -> Self {
        Self {
            name: String::new(),
            genre: String::new(),
            description: String::new(),
            comment: String::new(),
            contact_url: String::new(),
            content_type: "FLV".into(),
            bitrate: 0,
            source_mode: "push".into(),
            source_uri: String::new(),
            ingest_port: 8080,
            relay_port: 7144,
            announce_sp: false,
        }
    }
}
impl BroadcastRequest {
    pub fn normalize(mut self) -> Result<Self, String> {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() {
            return Err("チャンネル名を入力してください".into());
        }
        for (value, max) in [
            (&self.name, 128),
            (&self.genre, 128),
            (&self.description, 512),
            (&self.comment, 512),
            (&self.contact_url, 2048),
            (&self.source_uri, 2048),
        ] {
            if value.chars().count() > max || value.chars().any(char::is_control) {
                return Err("配信情報が長すぎるか制御文字が含まれています".into());
            }
        }
        self.content_type = self.content_type.to_ascii_uppercase();
        if !matches!(self.content_type.as_str(), "FLV" | "WMV") {
            return Err("配信形式はFLVまたはWMVを選択してください".into());
        }
        if self.bitrate > 1_000_000 {
            return Err("ビットレートが大きすぎます".into());
        }
        if !self.contact_url.is_empty() {
            http_url(&self.contact_url)?;
        }
        match self.source_mode.as_str() {
            "pull" => {
                http_url(&self.source_uri)?;
            }
            "push" => {
                if self.ingest_port == 0 {
                    return Err("入力ポートは1以上で指定してください".into());
                }
            }
            _ => return Err("入力方式が正しくありません".into()),
        }
        if self.announce_sp && self.relay_port == 0 {
            return Err("SP掲載時はリレー待受ポートを指定してください".into());
        }
        Ok(self)
    }
}
fn http_url(value: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(value).map_err(|_| "HTTP URLを指定してください")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("認証情報・フラグメントのないHTTP/HTTPS URLを指定してください".into());
    }
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BroadcastSnapshot {
    pub channel_id: String,
    pub name: String,
    pub status: String,
    pub input_url: String,
    pub stream_url: String,
    pub announcement_status: String,
    pub managed_by_rt: bool,
}
struct ManagedBroadcast {
    request: BroadcastRequest,
    view: BroadcastSnapshot,
    native: Option<BroadcastRuntime>,
    public_task: Option<JoinHandle<()>>,
}
pub struct BroadcastManager {
    jobs: Mutex<Vec<ManagedBroadcast>>,
    external: Option<Arc<CSharpBackend>>,
    sp: Mutex<Option<(i64, bool)>>,
    operation: Mutex<()>,
    closed: AtomicBool,
}
impl BroadcastManager {
    pub fn new(external: Option<Arc<CSharpBackend>>) -> Arc<Self> {
        Arc::new(Self {
            jobs: Mutex::new(Vec::new()),
            external,
            sp: Mutex::new(None),
            operation: Mutex::new(()),
            closed: AtomicBool::new(false),
        })
    }
    pub async fn start(
        &self,
        request: BroadcastRequest,
        limits: &Limits,
        log: Arc<BoundedLogBuffer>,
    ) -> Result<String, String> {
        let request = request.normalize()?;
        let _op = self.operation.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err("配信マネージャーは終了しています".into());
        }
        if self.jobs.lock().await.len() >= 4 {
            return Err("同時配信は4件までです".into());
        }
        if let Some(external) = &self.external {
            let before = external.channels().await?;
            if before
                .iter()
                .any(|channel| channel.is_broadcasting && channel.name == request.name)
            {
                return Err(
                    "同じ名前の配信がC#版で実行されています。別の名前を指定してください".into(),
                );
            }
            let yp = if request.announce_sp {
                let mut registration = self.sp.lock().await;
                if registration.is_none() {
                    *registration = Some(external.ensure_sp().await?)
                }
                registration.as_ref().map(|entry| entry.0)
            } else {
                None
            };
            let id = match external.start_broadcast(&request, yp).await {
                Ok(id) => id,
                Err(error) => {
                    return Err(match self.cleanup_sp_if_unused().await {
                        Ok(()) => error,
                        Err(cleanup) => format!("{error} / {cleanup}"),
                    });
                }
            };
            if before
                .iter()
                .any(|channel| channel.id.eq_ignore_ascii_case(&id))
            {
                self.cleanup_sp_if_unused().await?;
                return Err("同じ配信がC#版ですでに実行されています".into());
            }
            let input = if request.source_mode == "push" {
                format!("http://127.0.0.1:{}/", request.ingest_port)
            } else {
                redact_url(&request.source_uri)
            };
            let view = BroadcastSnapshot {
                channel_id: id.clone(),
                name: request.name.clone(),
                status: "接続中".into(),
                input_url: input,
                stream_url: format!("{}/stream/{id}", external.origin()),
                announcement_status: if request.announce_sp {
                    "SPへ掲載要求中"
                } else {
                    "掲載しない"
                }
                .into(),
                managed_by_rt: true,
            };
            self.jobs.lock().await.push(ManagedBroadcast {
                request,
                view,
                native: None,
                public_task: None,
            });
            return Ok(id);
        }
        let public_listener = if request.relay_port > 0 {
            Some(
                TcpListener::bind(("0.0.0.0", request.relay_port))
                    .await
                    .map_err(|error| format!("リレー待受ポートを使用できません: {error}"))?,
            )
        } else {
            None
        };
        let sp_endpoint = if request.announce_sp {
            Some(
                tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio::net::lookup_host(("bayonet.ddo.jp", 7146)),
                )
                .await
                .map_err(|_| "SPの名前解決がタイムアウトしました")?
                .map_err(|e| e.to_string())?
                .find(|a| a.is_ipv4())
                .ok_or("SPのIPv4接続先がありません")?,
            )
        } else {
            None
        };
        let id = uuid::Uuid::new_v4().simple().to_string();
        let bid = uuid::Uuid::new_v4().simple().to_string();
        let native = BroadcastRuntime::start(&request, &id, &bid, limits, log, sp_endpoint).await?;
        let task=public_listener.map(|listener|{let runtime=Arc::clone(&native.runtime);tokio::spawn(async move{
            let mut connections=tokio::task::JoinSet::new();
            loop {tokio::select!{
                incoming=listener.accept()=>{let Ok((mut stream,peer))=incoming else{break};let runtime=Arc::clone(&runtime);
                    if connections.len()>=32{continue}
                    connections.spawn(async move{let request=tokio::time::timeout(Duration::from_secs(5),async{
                        let mut request=Vec::new();let mut bytes=[0;1024];
                        loop {let n=stream.read(&mut bytes).await?;if n==0{return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))}request.extend_from_slice(&bytes[..n]);
                            if request.len()>8192{return Err(std::io::Error::from(std::io::ErrorKind::InvalidData))}
                            if request.starts_with(b"pcp\n")&&request.len()>=12{break}
                            if request.windows(4).any(|v|v==b"\r\n\r\n"){break}
                        }
                        Ok::<_,std::io::Error>(request)
                    }).await;
                        let Ok(Ok(request))=request else{return};
                        if request.starts_with(b"pcp\n"){let _=runtime.serve_pong(&mut stream,&request).await;}
                        else if request.starts_with(b"GET /channel/"){let _=runtime.serve_relay(&mut stream,peer,&request).await;}
                        else{let _=stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;}
                    });},
                _=connections.join_next(),if !connections.is_empty()=>{}
            }}
        })});
        let view = BroadcastSnapshot {
            channel_id: id.clone(),
            name: request.name.clone(),
            status: "入力待ち".into(),
            input_url: native.input_url.clone(),
            stream_url: native.stream_url.clone(),
            announcement_status: if request.announce_sp {
                "SPへ掲載要求中"
            } else {
                "掲載しない"
            }
            .into(),
            managed_by_rt: true,
        };
        self.jobs.lock().await.push(ManagedBroadcast {
            request,
            view,
            native: Some(native),
            public_task: task,
        });
        Ok(id)
    }
    async fn cleanup_sp_if_unused(&self) -> Result<(), String> {
        if self
            .jobs
            .lock()
            .await
            .iter()
            .any(|job| job.request.announce_sp)
        {
            return Ok(());
        }
        let mut registration = self.sp.lock().await;
        if let Some((id, owned)) = *registration {
            if owned {
                if let Some(external) = &self.external {
                    external.remove_sp(id).await?;
                }
            }
            *registration = None;
        }
        Ok(())
    }
    pub async fn stop(&self, id: &str) -> Result<(), String> {
        let _op = self.operation.lock().await;
        self.stop_locked(id).await
    }
    async fn stop_locked(&self, id: &str) -> Result<(), String> {
        let job = {
            let mut jobs = self.jobs.lock().await;
            jobs.iter()
                .position(|job| job.view.channel_id.eq_ignore_ascii_case(id))
                .map(|index| jobs.remove(index))
        };
        if let Some(mut job) = job {
            if let Some(task) = job.public_task.take() {
                task.abort();
                let _ = task.await;
            }
            let result = if let Some(native) = job.native.as_mut() {
                native.close().await
            } else if let Some(external) = &self.external {
                external.stop_broadcast(id).await
            } else {
                Ok(())
            };
            if let Err(error) = result {
                self.jobs.lock().await.push(job);
                return Err(error);
            }
        } else if let Some(external) = &self.external {
            external.stop_broadcast(id).await?;
        } else {
            return Err("配信が見つかりません".into());
        }
        self.cleanup_sp_if_unused().await?;
        Ok(())
    }
    pub async fn snapshot(&self) -> Result<(Vec<BroadcastSnapshot>, Vec<ChannelSnapshot>), String> {
        let needs_external = self
            .jobs
            .lock()
            .await
            .iter()
            .any(|job| job.native.is_none());
        let external_channels = if needs_external {
            if let Some(external) = &self.external {
                external.channels().await?
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let mut views = Vec::new();
        let mut channels = Vec::new();
        let mut jobs = self.jobs.lock().await;
        for job in &mut *jobs {
            if let Some(native) = &job.native {
                if let Some(channel) = native.runtime.channel(&job.view.channel_id) {
                    let state = channel.snapshot().await;
                    job.view.status = if state.available {
                        "配信中"
                    } else {
                        "入力待ち"
                    }
                    .into();
                    let mut snapshot =
                        ChannelSnapshot::native(channel.config(), state, &native.runtime);
                    snapshot.status = job.view.status.clone();
                    channels.push(snapshot);
                }
                if job.request.announce_sp {
                    job.view.announcement_status =
                        format!("SP: {:?}", native.runtime.yellow_page_statuses().await);
                }
            } else if let Some(channel) = external_channels
                .iter()
                .find(|channel| channel.id.eq_ignore_ascii_case(&job.view.channel_id))
            {
                job.view.status = channel.status.clone();
                if job.request.announce_sp {
                    if let Some(external) = &self.external {
                        job.view.announcement_status = external
                            .announcement_status(&job.view.channel_id)
                            .await
                            .unwrap_or_else(|_| "SPの掲載状態を確認できません".into());
                    }
                }
            }
            views.push(job.view.clone());
        }
        Ok((views, channels))
    }
    pub async fn shutdown(&self) -> Result<(), String> {
        let _op = self.operation.lock().await;
        self.closed.store(true, Ordering::Release);
        let ids = self
            .jobs
            .lock()
            .await
            .iter()
            .map(|job| job.view.channel_id.clone())
            .collect::<Vec<_>>();
        let mut errors = Vec::new();
        for id in ids {
            if let Err(error) = self.stop_locked(&id).await {
                errors.push(error)
            }
        }
        if let Err(error) = self.cleanup_sp_if_unused().await {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(" / "))
        }
    }
}
fn redact_url(value: &str) -> String {
    reqwest::Url::parse(value)
        .map(|mut url| {
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        })
        .unwrap_or_default()
}
