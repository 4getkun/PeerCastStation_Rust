use crate::{
    backend::{ChannelSnapshot, DesktopBackend, RecordingSource},
    recording_config::RecordingSettings,
    recording_path::{create_recording_target, RecordingFileName},
    rules::decorate_channels,
};
use peercast_app::runtime::AppRuntime;
use serde::Serialize;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, RwLock as StdRwLock, Weak,
    },
    time::Duration,
};
use tokio::sync::{watch, Mutex, Notify};

static NEXT_JOB: AtomicU64 = AtomicU64::new(1);
const HISTORY_LIMIT: usize = 256;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingJobSnapshot {
    pub id: String,
    pub channel_id: String,
    pub yellow_page: String,
    pub name: String,
    pub status: String,
    pub bytes_written: u64,
    pub segments: u32,
    pub retries: u32,
    pub current_file: Option<String>,
    pub error: Option<String>,
    pub keep: bool,
    pub started_at_ms: Option<u64>,
}

struct Job {
    view: RecordingJobSnapshot,
    cancel: Option<watch::Sender<bool>>,
    running: bool,
    file_action_pending: Arc<AtomicBool>,
    session: Arc<Mutex<RecordingSession>>,
    diagnostics: Arc<StdRwLock<Option<Arc<AppRuntime>>>>,
}
struct RecordingSession {
    started_at: Option<chrono::DateTime<chrono::Local>>,
    filename: Option<RecordingFileName>,
    segment: u32,
    source: Option<RecordingSource>,
}
struct State {
    settings: RecordingSettings,
    jobs: Vec<Job>,
    backend: Weak<DesktopBackend>,
    closed: bool,
}
pub struct RecordingManager {
    state: Mutex<State>,
    wake: Notify,
    settings_changed: watch::Sender<u64>,
    stopped: AtomicBool,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    monitor: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

pub struct RecordingFileLease {
    pub path: std::path::PathBuf,
    pending: Arc<AtomicBool>,
    manager: Weak<RecordingManager>,
}

impl Drop for RecordingFileLease {
    fn drop(&mut self) {
        self.pending.store(false, Ordering::Release);
        if let Some(manager) = self.manager.upgrade() {
            manager.wake.notify_one();
        }
    }
}

impl RecordingManager {
    pub fn new() -> Arc<Self> {
        let (settings_changed, _) = watch::channel(0);
        Arc::new(Self {
            state: Mutex::new(State {
                settings: RecordingSettings::default(),
                jobs: Vec::new(),
                backend: Weak::new(),
                closed: false,
            }),
            wake: Notify::new(),
            settings_changed,
            stopped: AtomicBool::new(false),
            tasks: Mutex::new(Vec::new()),
            monitor: Mutex::new(None),
        })
    }
    pub async fn attach_backend(self: &Arc<Self>, backend: Weak<DesktopBackend>) {
        {
            let mut s = self.state.lock().await;
            if s.backend.strong_count() != 0 {
                return;
            }
            s.backend = backend;
        }
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            if let Some(m) = weak.upgrade() {
                m.monitor_loop().await;
            }
        });
        *self.monitor.lock().await = Some(task);
    }
    pub async fn update_settings(&self, settings: RecordingSettings) {
        if let Ok(settings) = settings.normalize() {
            let mut state = self.state.lock().await;
            if state.settings == settings {
                return;
            }
            state.settings = settings;
            drop(state);
            let n = *self.settings_changed.borrow() + 1;
            let _ = self.settings_changed.send(n);
            self.wake.notify_one();
        }
    }
    pub async fn snapshot(&self) -> Vec<RecordingJobSnapshot> {
        self.state
            .lock()
            .await
            .jobs
            .iter()
            .map(|j| j.view.clone())
            .collect()
    }

    pub async fn channels(&self) -> Vec<ChannelSnapshot> {
        let diagnostics = {
            let state = self.state.lock().await;
            state
                .jobs
                .iter()
                .map(|job| Arc::clone(&job.diagnostics))
                .collect::<Vec<_>>()
        };
        let runtimes = diagnostics
            .into_iter()
            .filter_map(|diagnostics| {
                diagnostics
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            })
            .collect::<Vec<_>>();
        let mut channels = Vec::new();
        for runtime in runtimes {
            for (config, state) in runtime.channel_snapshots().await {
                let mut channel = ChannelSnapshot::native(&config, state, &runtime);
                channel.diagnostics.purpose = "recording".into();
                channels.push(channel);
            }
        }
        channels
    }

    pub async fn enqueue(
        self: &Arc<Self>,
        channel_id: String,
        yellow_page: String,
        keep: bool,
    ) -> Result<String, String> {
        let mut s = self.state.lock().await;
        if s.closed {
            return Err("録画マネージャーは終了しています".into());
        }
        if let Some(j) = s.jobs.iter().find(|j| {
            j.view.channel_id.eq_ignore_ascii_case(&channel_id)
                && j.view.yellow_page == yellow_page
                && !matches!(j.view.status.as_str(), "completed" | "failed" | "stopped")
        }) {
            return Ok(j.view.id.clone());
        }
        trim_history(&mut s.jobs);
        if s.jobs.len() >= HISTORY_LIMIT {
            return Err("録画ジョブ履歴が上限に達しています".into());
        }
        let id = format!("recording-{}", NEXT_JOB.fetch_add(1, Ordering::Relaxed));
        s.jobs.push(Job {
            view: RecordingJobSnapshot {
                id: id.clone(),
                channel_id,
                yellow_page,
                name: String::new(),
                status: "queued".into(),
                bytes_written: 0,
                segments: 0,
                retries: 0,
                current_file: None,
                error: None,
                keep,
                started_at_ms: None,
            },
            cancel: None,
            running: false,
            file_action_pending: Arc::new(AtomicBool::new(false)),
            session: Arc::new(Mutex::new(RecordingSession {
                started_at: None,
                filename: None,
                segment: 0,
                source: None,
            })),
            diagnostics: Arc::new(StdRwLock::new(None)),
        });
        drop(s);
        self.wake.notify_one();
        Ok(id)
    }
    pub async fn reserve_file_action(
        self: &Arc<Self>,
        job_id: &str,
        deleting: bool,
    ) -> Result<RecordingFileLease, String> {
        let state = self.state.lock().await;
        let job = state
            .jobs
            .iter()
            .find(|job| job.view.id == job_id)
            .ok_or("録画ジョブが見つかりません")?;
        if deleting && (job.running || job.view.status == "queued") {
            return Err("録画を停止してからファイルをごみ箱へ移動してください".into());
        }
        let path = job
            .view
            .current_file
            .as_ref()
            .ok_or("録画ファイルがまだありません")?;
        if job
            .file_action_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("ファイルを操作中です".into());
        }
        Ok(RecordingFileLease {
            path: path.into(),
            pending: Arc::clone(&job.file_action_pending),
            manager: Arc::downgrade(self),
        })
    }

    pub async fn forget_deleted_file(
        &self,
        job_id: &str,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let mut state = self.state.lock().await;
        let job = state
            .jobs
            .iter_mut()
            .find(|job| job.view.id == job_id)
            .ok_or("録画ジョブが見つかりません")?;
        if !job.file_action_pending.load(Ordering::Acquire)
            || job.view.current_file.as_deref().map(std::path::Path::new) != Some(path)
        {
            return Err("録画ファイルの選択が変わりました".into());
        }
        job.view.current_file = None;
        Ok(())
    }

    pub async fn control(self: &Arc<Self>, job_id: &str, action: &str) -> Result<(), String> {
        let mut s = self.state.lock().await;
        let mut close_session = None;
        let i = s
            .jobs
            .iter()
            .position(|j| j.view.id == job_id)
            .ok_or("録画ジョブが見つかりません")?;
        if s.jobs[i].file_action_pending.load(Ordering::Acquire) {
            return Err("ファイル操作が完了してから録画を操作してください".into());
        }
        match action {
            "start" => {
                let j = &mut s.jobs[i];
                if j.running {
                    return Ok(());
                }
                j.view.status = "queued".into();
                j.view.error = None;
            }
            "stop" => {
                let j = &mut s.jobs[i];
                j.view.keep = false;
                j.view.status = "stopped".into();
                if let Some(c) = &j.cancel {
                    let _ = c.send(true);
                }
                if !j.running {
                    j.cancel = None;
                    close_session = Some((Arc::clone(&j.session), Arc::clone(&j.diagnostics)));
                }
            }
            "remove" => {
                let j = &mut s.jobs[i];
                if let Some(c) = &j.cancel {
                    let _ = c.send(true);
                }
                if j.running {
                    j.view.status = "stopped".into();
                    j.view.error = Some("削除待ち".into());
                } else {
                    close_session = Some((Arc::clone(&j.session), Arc::clone(&j.diagnostics)));
                    s.jobs.remove(i);
                }
            }
            "up" | "down" => {
                let j = if action == "up" {
                    i.checked_sub(1)
                } else if i + 1 < s.jobs.len() {
                    Some(i + 1)
                } else {
                    None
                };
                if let Some(k) = j {
                    s.jobs.swap(i, k);
                }
            }
            "toggle_keep" => {
                let j = &mut s.jobs[i];
                j.view.keep = !j.view.keep;
            }
            "reconnect" => {
                let j = &mut s.jobs[i];
                if let Some(c) = &j.cancel {
                    let _ = c.send(true);
                }
                j.view.status = "queued".into();
                j.view.error = None;
                if !j.running {
                    close_session = Some((Arc::clone(&j.session), Arc::clone(&j.diagnostics)));
                }
            }
            _ => return Err("録画操作が正しくありません".into()),
        }
        drop(s);
        if let Some((session, diagnostics)) = close_session {
            close_session_source(&session, &diagnostics).await;
        }
        self.wake.notify_one();
        Ok(())
    }
    pub async fn shutdown(&self) -> Result<(), String> {
        self.stopped.store(true, Ordering::Release);
        let mut s = self.state.lock().await;
        s.closed = true;
        for j in &mut s.jobs {
            if let Some(c) = &j.cancel {
                let _ = c.send(true);
            }
            if j.running {
                j.view.status = "stopped".into();
            }
        }
        drop(s);
        self.wake.notify_one();
        tokio::time::timeout(Duration::from_secs(8), async {
            if let Some(monitor) = self.monitor.lock().await.take() {
                monitor.abort();
                let _ = monitor.await;
            }
            loop {
                if self.state.lock().await.jobs.iter().all(|j| !j.running) { break; }
                tokio::select! { _=self.wake.notified()=>{}, _=tokio::time::sleep(Duration::from_millis(25))=>{} }
            }
            let tasks = std::mem::take(&mut *self.tasks.lock().await);
            for task in tasks { let _ = task.await; }
            let sessions = { let state=self.state.lock().await; state.jobs.iter().map(|job|(Arc::clone(&job.session),Arc::clone(&job.diagnostics))).collect::<Vec<_>>() };
            for (session, diagnostics) in sessions { close_session_source(&session, &diagnostics).await; }
        }).await.map_err(|_| "録画停止処理が8秒以内に完了しませんでした".to_owned())?;
        Ok(())
    }
    async fn monitor_loop(self: Arc<Self>) {
        let mut previous_auto = HashSet::<(String, String)>::new();
        loop {
            if self.stopped.load(Ordering::Acquire) {
                break;
            }
            let (backend, settings) = {
                let s = self.state.lock().await;
                (s.backend.upgrade(), s.settings.clone())
            };
            if let Some(backend) = backend {
                if let Ok(saved) = backend.recording_settings().await {
                    if saved != settings {
                        self.update_settings(saved).await;
                    }
                }
                let now = chrono::Local::now();
                if settings.enabled && settings.within_schedule(now.hour_minute()) {
                    if let Ok(rows) = backend.recording_directory().await {
                        let rules = settings
                            .rules
                            .iter()
                            .map(|r| r.as_channel_rule())
                            .collect::<Vec<_>>();
                        if let Ok(rows) = decorate_channels(rows, &rules, "") {
                            let mut current_auto = HashSet::new();
                            for row in rows.into_iter().take(128) {
                                if !row["matchedRuleId"].is_string() {
                                    continue;
                                }
                                if let (Some(id), Some(yp)) =
                                    (row["channelId"].as_str(), row["yellowPage"].as_str())
                                {
                                    let key = (id.to_ascii_lowercase(), yp.to_owned());
                                    if current_auto.insert(key.clone())
                                        && !previous_auto.contains(&key)
                                    {
                                        let _ = self.enqueue(id.into(), yp.into(), false).await;
                                    }
                                }
                            }
                            previous_auto = current_auto;
                        }
                    }
                } else {
                    previous_auto.clear();
                }
                self.start_queued(backend).await;
            }
            let secs = settings.monitor_interval_secs.clamp(10, 3600);
            tokio::select! { _=self.wake.notified()=>{}, _=tokio::time::sleep(Duration::from_secs(secs))=>{} }
        }
    }
    async fn start_queued(self: &Arc<Self>, backend: Arc<DesktopBackend>) {
        loop {
            let launch = {
                let mut s = self.state.lock().await;
                if s.closed || self.stopped.load(Ordering::Acquire) {
                    None
                } else {
                    let max = s.settings.concurrency();
                    let active = s.jobs.iter().filter(|j| j.running).count();
                    if active >= max {
                        None
                    } else if let Some(j) = s.jobs.iter_mut().find(|j| {
                        j.view.status == "queued"
                            && !j.running
                            && !j.file_action_pending.load(Ordering::Acquire)
                    }) {
                        let (cancel_tx, cancel_rx) = watch::channel(false);
                        j.running = true;
                        j.view.status = "connecting".into();
                        j.cancel = Some(cancel_tx);
                        Some((
                            j.view.id.clone(),
                            j.view.channel_id.clone(),
                            j.view.yellow_page.clone(),
                            cancel_rx,
                            Arc::clone(&j.session),
                            Arc::clone(&j.diagnostics),
                        ))
                    } else {
                        None
                    }
                }
            };
            let Some((id, cid, yp, cancel_rx, session, diagnostics)) = launch else {
                break;
            };
            let weak = Arc::downgrade(self);
            let backend = Arc::downgrade(&backend);
            let task = tokio::spawn(async move {
                if let (Some(m), Some(b)) = (weak.upgrade(), backend.upgrade()) {
                    let session_for_cleanup = Arc::clone(&session);
                    let diagnostics_for_cleanup = Arc::clone(&diagnostics);
                    let res = run_job(RecordingJobTask {
                        manager: &m,
                        backend: &b,
                        id: &id,
                        cid: &cid,
                        yp: &yp,
                        session,
                        diagnostics,
                        cancel: cancel_rx,
                    })
                    .await;
                    close_session_source(&session_for_cleanup, &diagnostics_for_cleanup).await;
                    let mut s = m.state.lock().await;
                    if let Some(i) = s.jobs.iter().position(|j| j.view.id == id) {
                        let j = &mut s.jobs[i];
                        j.running = false;
                        j.cancel = None;
                        if j.view.error.as_deref() == Some("削除待ち") {
                            s.jobs.remove(i);
                        } else if j.view.status != "stopped" && j.view.status != "queued" {
                            match res {
                                Ok(()) => j.view.status = "completed".into(),
                                Err(e) => {
                                    j.view.status = "failed".into();
                                    j.view.error = Some(e)
                                }
                            }
                        }
                    }
                    drop(s);
                    m.wake.notify_one();
                }
            });
            let mut tasks = self.tasks.lock().await;
            tasks.retain(|task| !task.is_finished());
            tasks.push(task);
        }
    }
}

use chrono::Timelike;
trait Minute {
    fn hour_minute(&self) -> u32;
}
impl Minute for chrono::DateTime<chrono::Local> {
    fn hour_minute(&self) -> u32 {
        self.hour() * 60 + self.minute()
    }
}

fn trim_history(jobs: &mut Vec<Job>) {
    while jobs.len() >= HISTORY_LIMIT {
        if let Some(i) = jobs.iter().position(|j| {
            !j.running
                && !j.file_action_pending.load(Ordering::Acquire)
                && matches!(j.view.status.as_str(), "completed" | "failed" | "stopped")
                && j.session
                    .try_lock()
                    .is_ok_and(|session| session.source.is_none())
        }) {
            jobs.remove(i);
        } else {
            break;
        }
    }
}

struct RecordingJobTask<'a> {
    manager: &'a RecordingManager,
    backend: &'a DesktopBackend,
    id: &'a str,
    cid: &'a str,
    yp: &'a str,
    session: Arc<Mutex<RecordingSession>>,
    diagnostics: Arc<StdRwLock<Option<Arc<AppRuntime>>>>,
    cancel: watch::Receiver<bool>,
}

async fn run_job(task: RecordingJobTask<'_>) -> Result<(), String> {
    let RecordingJobTask {
        manager,
        backend,
        id,
        cid,
        yp,
        session,
        diagnostics,
        mut cancel,
    } = task;
    let mut session = session.lock().await;
    let started_at = session
        .started_at
        .get_or_insert_with(chrono::Local::now)
        .to_owned();
    let mut retry = 0u32;
    let mut total = manager
        .state
        .lock()
        .await
        .jobs
        .iter()
        .find(|j| j.view.id == id)
        .map_or(0, |j| j.view.bytes_written);
    loop {
        if *cancel.borrow() {
            close_source_slot(&mut session.source, &diagnostics).await;
            return Ok(());
        }
        let RecordingSession {
            segment,
            filename,
            source,
            ..
        } = &mut *session;
        match recording_attempt(RecordingAttempt {
            manager,
            backend,
            id,
            cid,
            yp,
            cancel: &mut cancel,
            segment,
            total: &mut total,
            retries: retry,
            filename,
            source,
            diagnostics: &diagnostics,
            started_at,
        })
        .await
        {
            Ok(true) => {
                retry = 0;
                continue;
            } // successful split: reconnect, fresh HTTP headers
            Ok(false) => return Ok(()), // cancelled
            Err(AttemptError::Fatal(error)) => return Err(error),
            Err(AttemptError::Network(error)) => {
                set_job(
                    manager,
                    id,
                    JobUpdate {
                        status: "retrying",
                        name: None,
                        file: None,
                        error: Some(error),
                        bytes: total,
                        segments: *segment,
                        retries: retry,
                    },
                )
                .await
            }
        }
        retry += 1;
        let settings = manager.state.lock().await.settings.clone();
        if settings
            .disconnect_every_retries
            .is_some_and(|every| retry.is_multiple_of(every))
        {
            close_source_slot(source, &diagnostics).await;
        }
        if retry > settings.retry_count {
            let keep = {
                manager
                    .state
                    .lock()
                    .await
                    .jobs
                    .iter()
                    .find(|j| j.view.id == id)
                    .is_some_and(|j| j.view.keep)
            };
            if !keep {
                if settings.disconnect_on_retry_end {
                    close_source_slot(source, &diagnostics).await;
                }
                return Err("再試行回数の上限に達しました".into());
            }
            if settings.disconnect_on_retry_end {
                close_source_slot(source, &diagnostics).await;
            }
            retry = 0;
        }
        tokio::select! { _=cancel.changed()=>{close_source_slot(source, &diagnostics).await;return Ok(())}, _=tokio::time::sleep(Duration::from_secs(settings.retry_interval_secs))=>{} }
    }
}

enum AttemptError {
    Network(String),
    Fatal(String),
}

async fn close_source_slot(
    source: &mut Option<RecordingSource>,
    diagnostics: &StdRwLock<Option<Arc<AppRuntime>>>,
) {
    if let Some(mut source) = source.take() {
        let _ = source.close().await;
    }
    *diagnostics
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

async fn close_session_source(
    session: &Mutex<RecordingSession>,
    diagnostics: &StdRwLock<Option<Arc<AppRuntime>>>,
) {
    let mut session = session.lock().await;
    close_source_slot(&mut session.source, diagnostics).await;
}

async fn recording_attempt(attempt: RecordingAttempt<'_>) -> Result<bool, AttemptError> {
    let RecordingAttempt {
        manager,
        backend,
        id,
        cid,
        yp,
        cancel,
        segment,
        total,
        retries,
        filename,
        source: source_slot,
        diagnostics,
        started_at,
    } = attempt;
    if source_slot.is_none() {
        let source = tokio::select! { _=cancel.changed()=>return Ok(false), r=backend.open_recording_source(cid,yp)=>r.map_err(AttemptError::Network)? };
        *diagnostics
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = source.diagnostic_runtime();
        *source_slot = Some(source);
    }
    let source = source_slot.as_mut().expect("source was just initialized");
    set_job(
        manager,
        id,
        JobUpdate {
            status: "connecting",
            name: Some(source.name.clone()),
            file: None,
            error: None,
            bytes: *total,
            segments: *segment,
            retries,
        },
    )
    .await;
    let client = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            close_source_slot(source_slot, diagnostics).await;
            return Err(AttemptError::Fatal(error.to_string()));
        }
    };
    let stream_url = source.stream_url.clone();
    let response = tokio::select! {
        _=cancel.changed()=>{close_source_slot(source_slot, diagnostics).await;return Ok(false)},
        r=tokio::time::timeout(Duration::from_secs(15),client.get(&stream_url).send())=>match r { Ok(Ok(response))=>response, Ok(Err(e))=>return Err(AttemptError::Network(e.to_string())), Err(_)=>return Err(AttemptError::Network("HTTP応答ヘッダーが15秒以内に届きません".into())) }
    };
    if !response.status().is_success() {
        let status = response.status();
        return Err(AttemptError::Network(format!("HTTP {status}")));
    }
    let mut response = response;
    let mut file: Option<tokio::fs::File> = None;
    let mut flv_normalizer = source
        .content_type
        .eq_ignore_ascii_case("FLV")
        .then(crate::recording_flv::FlvTimestampNormalizer::new);
    let mut began = None;
    let mut part_bytes = 0u64;
    let mut split = false;
    loop {
        let chunk = tokio::select! {
            _=cancel.changed()=>{
                if let (Some(f), Some(normalizer)) = (file.as_mut(), flv_normalizer.as_mut()) {
                    let tail = normalizer.finish();
                    let _ = f.write_all(&tail).await;
                    *total += tail.len() as u64;
                    update_bytes(manager, id, *total).await;
                }
                if let Some(mut f)=file.take(){let _=f.flush().await;}
                close_source_slot(source_slot, diagnostics).await;return Ok(false)
            },
            r=tokio::time::timeout(Duration::from_secs(15),response.chunk())=>match r {
                Ok(Ok(Some(bytes)))=>bytes,
                Ok(Ok(None))=>break,
                Ok(Err(e))=>{
                    if let (Some(f), Some(normalizer)) = (file.as_mut(), flv_normalizer.as_mut()) {
                        let tail = normalizer.finish();
                        let _ = f.write_all(&tail).await;
                        *total += tail.len() as u64;
                        update_bytes(manager, id, *total).await;
                    }
                    if let Some(mut f)=file.take(){let _=f.flush().await;}
                    return Err(AttemptError::Network(e.to_string()))
                },
                Err(_)=>{
                    if let (Some(f), Some(normalizer)) = (file.as_mut(), flv_normalizer.as_mut()) {
                        let tail = normalizer.finish();
                        let _ = f.write_all(&tail).await;
                        *total += tail.len() as u64;
                        update_bytes(manager, id, *total).await;
                    }
                    if let Some(mut f)=file.take(){let _=f.flush().await;}
                    return Err(AttemptError::Network("ストリームの受信が15秒間停止しました".into()))
                }
            }
        };
        if chunk.is_empty() {
            continue;
        }
        if file.is_none() {
            *segment += 1;
            let settings = manager.state.lock().await.settings.clone();
            let exe = match std::env::current_exe() {
                Ok(exe) => exe,
                Err(error) => {
                    close_source_slot(source_slot, diagnostics).await;
                    return Err(AttemptError::Fatal(error.to_string()));
                }
            };
            let target = create_recording_target(
                &exe,
                &settings,
                filename.get_or_insert_with(|| {
                    RecordingFileName::new(&source.name, &source.title, started_at)
                }),
                &source.content_type,
                *segment,
            );
            let target = match target {
                Ok(target) => target,
                Err(error) => {
                    close_source_slot(source_slot, diagnostics).await;
                    return Err(AttemptError::Fatal(error));
                }
            };
            let path = target.path;
            file = Some(target.file);
            began = Some(tokio::time::Instant::now());
            set_job(
                manager,
                id,
                JobUpdate {
                    status: "recording",
                    name: None,
                    file: Some(path.to_string_lossy().into_owned()),
                    error: None,
                    bytes: *total,
                    segments: *segment,
                    retries,
                },
            )
            .await;
        }
        let recording_bytes = if let Some(normalizer) = flv_normalizer.as_mut() {
            match normalizer.push(&chunk) {
                Ok(bytes) => bytes,
                Err(error) => {
                    if let Some(mut partial) = file.take() {
                        let _ = partial.flush().await;
                    }
                    close_source_slot(source_slot, diagnostics).await;
                    return Err(AttemptError::Fatal(error));
                }
            }
        } else {
            chunk.to_vec()
        };
        if let Err(error) = file.as_mut().unwrap().write_all(&recording_bytes).await {
            if let Some(mut partial) = file.take() {
                let _ = partial.flush().await;
            }
            close_source_slot(source_slot, diagnostics).await;
            return Err(AttemptError::Fatal(error.to_string()));
        }
        part_bytes += recording_bytes.len() as u64;
        *total += recording_bytes.len() as u64;
        update_bytes(manager, id, *total).await;
        let settings = manager.state.lock().await.settings.clone();
        let size = settings
            .split_size_mb
            .map(|v| v.saturating_mul(1024 * 1024))
            .is_some_and(|v| part_bytes >= v);
        let time = settings
            .split_minutes
            .is_some_and(|v| began.is_some_and(|b| b.elapsed() >= Duration::from_secs(v * 60)));
        if size || time {
            split = true;
            break;
        }
    }
    if let (Some(f), Some(normalizer)) = (file.as_mut(), flv_normalizer.as_mut()) {
        let tail = normalizer.finish();
        if let Err(error) = f.write_all(&tail).await {
            close_source_slot(source_slot, diagnostics).await;
            return Err(AttemptError::Fatal(error.to_string()));
        }
        *total += tail.len() as u64;
        update_bytes(manager, id, *total).await;
    }
    if let Some(mut f) = file.take() {
        if let Err(error) = f.flush().await {
            close_source_slot(source_slot, diagnostics).await;
            return Err(AttemptError::Fatal(error.to_string()));
        }
        if let Err(error) = f.sync_data().await {
            close_source_slot(source_slot, diagnostics).await;
            return Err(AttemptError::Fatal(error.to_string()));
        }
    }
    if split {
        Ok(true)
    } else {
        Err(AttemptError::Network("ストリームが終了しました".into()))
    }
}

async fn update_bytes(m: &RecordingManager, id: &str, bytes: u64) {
    let mut s = m.state.lock().await;
    if let Some(j) = s.jobs.iter_mut().find(|j| j.view.id == id) {
        j.view.bytes_written = bytes;
    }
}
struct RecordingAttempt<'a> {
    manager: &'a RecordingManager,
    backend: &'a DesktopBackend,
    id: &'a str,
    cid: &'a str,
    yp: &'a str,
    cancel: &'a mut watch::Receiver<bool>,
    segment: &'a mut u32,
    total: &'a mut u64,
    retries: u32,
    filename: &'a mut Option<RecordingFileName>,
    source: &'a mut Option<RecordingSource>,
    diagnostics: &'a StdRwLock<Option<Arc<AppRuntime>>>,
    started_at: chrono::DateTime<chrono::Local>,
}

struct JobUpdate<'a> {
    status: &'a str,
    name: Option<String>,
    file: Option<String>,
    error: Option<String>,
    bytes: u64,
    segments: u32,
    retries: u32,
}

async fn set_job(m: &RecordingManager, id: &str, update: JobUpdate<'_>) {
    let mut s = m.state.lock().await;
    if let Some(j) = s.jobs.iter_mut().find(|j| j.view.id == id) {
        j.view.status = update.status.into();
        if let Some(n) = update.name {
            j.view.name = n
        }
        if update.file.is_some() {
            j.view.current_file = update.file
        }
        j.view.error = update.error;
        j.view.bytes_written = update.bytes;
        j.view.segments = update.segments;
        j.view.retries = update.retries;
        if j.view.started_at_ms.is_none() {
            j.view.started_at_ms = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            )
        }
    }
}

use tokio::io::AsyncWriteExt;

#[cfg(test)]
mod file_action_tests {
    use super::*;

    #[tokio::test]
    async fn file_action_lease_blocks_restart_remove_and_duplicate_operations() {
        let manager = RecordingManager::new();
        let id = manager
            .enqueue(
                "11111111111111111111111111111111".into(),
                "Fixture".into(),
                false,
            )
            .await
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("clip.flv");
        std::fs::write(&path, b"unchanged").unwrap();
        {
            let mut state = manager.state.lock().await;
            state.jobs[0].view.current_file = Some(path.to_string_lossy().into_owned());
            state.jobs[0].view.status = "stopped".into();
        }
        let lease = manager.reserve_file_action(&id, true).await.unwrap();
        assert!(manager.reserve_file_action(&id, false).await.is_err());
        assert!(manager.control(&id, "start").await.is_err());
        assert!(manager.control(&id, "remove").await.is_err());
        assert!(manager
            .forget_deleted_file(&id, &temp.path().join("other.flv"))
            .await
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"unchanged");
        drop(lease);
        manager.control(&id, "start").await.unwrap();
        assert!(manager.reserve_file_action(&id, true).await.is_err());
        manager.control(&id, "stop").await.unwrap();
        let lease = manager.reserve_file_action(&id, true).await.unwrap();
        std::fs::rename(&path, temp.path().join("recycled.flv")).unwrap();
        manager.forget_deleted_file(&id, &lease.path).await.unwrap();
        drop(lease);
        assert!(manager.snapshot().await[0].current_file.is_none());
        assert!(manager.reserve_file_action(&id, false).await.is_err());
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn trash_is_rejected_until_a_stopping_recording_worker_has_finished() {
        let manager = RecordingManager::new();
        let id = manager
            .enqueue(
                "22222222222222222222222222222222".into(),
                "Fixture".into(),
                false,
            )
            .await
            .unwrap();
        {
            let mut state = manager.state.lock().await;
            state.jobs[0].view.current_file = Some("C:/fixture.flv".into());
            state.jobs[0].view.status = "stopped".into();
            state.jobs[0].running = true;
        }
        assert!(manager.reserve_file_action(&id, true).await.is_err());
        manager.state.lock().await.jobs[0].running = false;
        manager.shutdown().await.unwrap();
    }
}
