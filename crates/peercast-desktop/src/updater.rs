use crate::backend::DesktopBackend;
use reqwest::{redirect::Policy, StatusCode, Url};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::time::Duration;
use tauri::State;
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::{watch, Mutex as AsyncMutex};

pub const UPDATE_ENDPOINT: &str =
    "https://peercast-rt-downloads.fourgetkun.com/updates/stable/windows-x64/latest.json";
const UPDATE_HOST: &str = "peercast-rt-downloads.fourgetkun.com";
const MAX_MANIFEST: usize = 256 * 1024;
const MAX_PACKAGE: usize = 256 * 1024 * 1024;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub phase: String,
    pub current_version: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub received_bytes: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        Self {
            phase: "idle".into(),
            current_version: env!("CARGO_PKG_VERSION").into(),
            version: None,
            notes: None,
            received_bytes: 0,
            total_bytes: None,
            error: None,
        }
    }
}

#[derive(Default)]
struct Inner {
    status: UpdateStatus,
    candidate: Option<Update>,
    package: Option<Vec<u8>>,
}

pub struct UpdateManager {
    inner: Mutex<Inner>,
    operation: AsyncMutex<()>,
    cancel: watch::Sender<bool>,
    shutdown_finished: AtomicBool,
    shutdown_started: AtomicBool,
}

impl Default for UpdateManager {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            operation: AsyncMutex::new(()),
            cancel: watch::channel(false).0,
            shutdown_finished: AtomicBool::new(false),
            shutdown_started: AtomicBool::new(false),
        }
    }
}

impl UpdateManager {
    fn lock_inner(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn status(&self) -> UpdateStatus {
        self.lock_inner().status.clone()
    }
    fn fail(&self, error: impl ToString) -> String {
        let error = error.to_string();
        let mut inner = self.lock_inner();
        inner.status.phase = "error".into();
        inner.status.error = Some(error.clone());
        error
    }
    pub fn shutdown_finished(&self) -> bool {
        self.shutdown_finished.load(Ordering::Acquire)
    }
    pub fn close_after_update_error(&self) -> bool {
        self.shutdown_started.load(Ordering::Acquire) && self.status().phase == "error"
    }
}

fn authorize(window: &tauri::WebviewWindow) -> Result<(), String> {
    if !crate::platform::UPDATER_SUPPORTED {
        return Err("このOSでは自動更新は未対応です。Linux版は手動で更新してください。".into());
    }
    if window.label() == "main" {
        Ok(())
    } else {
        Err("このウィンドウから更新は操作できません。".into())
    }
}

pub fn allowed_artifact(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some(UPDATE_HOST)
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.path().starts_with("/releases/")
        && url.path().ends_with(".exe")
}

fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(Policy::none())
        .build()
        .map_err(|error| error.to_string())
}

fn append_bounded(bytes: &mut Vec<u8>, chunk: &[u8], limit: usize) -> Result<(), String> {
    if bytes.len().saturating_add(chunk.len()) > limit {
        return Err("更新ファイルがサイズ上限を超えています。".into());
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}

async fn bounded_body(
    mut response: reqwest::Response,
    limit: usize,
    mut progress: impl FnMut(usize, Option<u64>),
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err("更新ファイルがサイズ上限を超えています。".into());
    }
    let total = response.content_length();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        append_bounded(&mut bytes, &chunk, limit)?;
        progress(bytes.len(), total);
    }
    Ok(bytes)
}

#[tauri::command]
pub fn update_status(manager: State<'_, UpdateManager>) -> UpdateStatus {
    if !crate::platform::UPDATER_SUPPORTED {
        return UpdateStatus {
            phase: "unsupported".into(),
            error: Some("Linux版は手動更新です".into()),
            ..UpdateStatus::default()
        };
    }
    manager.status()
}

#[tauri::command]
pub async fn check_update(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    manager: State<'_, UpdateManager>,
) -> Result<UpdateStatus, String> {
    authorize(&window)?;
    let Ok(_operation) = manager.operation.try_lock() else {
        return Ok(manager.status());
    };
    if manager.lock_inner().package.is_some() || manager.shutdown_finished() {
        return Ok(manager.status());
    }
    {
        let mut inner = manager.lock_inner();
        inner.status = UpdateStatus {
            phase: "checking".into(),
            ..UpdateStatus::default()
        };
        inner.candidate = None;
    }
    let result = async {
        let response = client(Duration::from_secs(15))?
            .get(UPDATE_ENDPOINT)
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .send()
            .await
            .map_err(|error| format!("更新情報を取得できません: {error}"))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(("notPublished", None));
        }
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(("upToDate", None));
        }
        if !response.status().is_success() {
            return Err(format!(
                "更新情報を取得できません: HTTP {}",
                response.status()
            ));
        }
        let bytes = bounded_body(response, MAX_MANIFEST, |_, _| {}).await?;
        let manifest: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("更新情報が正しくありません: {error}"))?;
        let updater = app
            .updater_builder()
            .build()
            .map_err(|error| error.to_string())?;
        let candidate = updater
            .restore_update(manifest)
            .map_err(|error| format!("更新情報を確認できません: {error}"))?;
        if let Some(update) = &candidate {
            if !allowed_artifact(&update.download_url) {
                return Err("Peercast RT用の正規の更新URLではありません。".into());
            }
        }
        Ok((
            if candidate.is_some() {
                "available"
            } else {
                "upToDate"
            },
            candidate,
        ))
    }
    .await;
    match result {
        Ok((phase, candidate)) => {
            let mut inner = manager.lock_inner();
            inner.status.phase = phase.into();
            inner.status.version = candidate
                .as_ref()
                .map(|candidate| candidate.version.clone());
            inner.status.notes = candidate
                .as_ref()
                .and_then(|candidate| candidate.body.clone());
            inner.candidate = candidate;
            Ok(inner.status.clone())
        }
        Err(error) => Err(manager.fail(error)),
    }
}

#[tauri::command]
pub async fn download_update(
    window: tauri::WebviewWindow,
    manager: State<'_, UpdateManager>,
) -> Result<UpdateStatus, String> {
    authorize(&window)?;
    let Ok(_operation) = manager.operation.try_lock() else {
        return Ok(manager.status());
    };
    if manager.lock_inner().package.is_some() || manager.shutdown_finished() {
        return Ok(manager.status());
    }
    let candidate = manager
        .lock_inner()
        .candidate
        .clone()
        .ok_or("先に更新を確認してください。")?;
    if !allowed_artifact(&candidate.download_url) {
        return Err(manager.fail("更新URLが正しくありません。"));
    }
    manager.cancel.send_replace(false);
    let mut cancel = manager.cancel.subscribe();
    {
        let mut inner = manager.lock_inner();
        inner.status.phase = "downloading".into();
        inner.status.error = None;
        inner.status.received_bytes = 0;
        inner.status.total_bytes = None;
    }
    let download = async {
        let response = client(Duration::from_secs(600))?
            .get(candidate.download_url.clone())
            .send()
            .await
            .map_err(|error| format!("更新をダウンロードできません: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "更新をダウンロードできません: HTTP {}",
                response.status()
            ));
        }
        let bytes = bounded_body(response, MAX_PACKAGE, |received, total| {
            let mut inner = manager.lock_inner();
            inner.status.received_bytes = received as u64;
            inner.status.total_bytes = total;
        })
        .await?;
        candidate
            .verify(&bytes)
            .map_err(|error| format!("更新ファイルの署名を検証できません: {error}"))?;
        Ok(bytes)
    };
    let result =
        tokio::select! { biased; _ = cancel.changed() => None, result = download => Some(result) };
    match result {
        None => {
            let mut inner = manager.lock_inner();
            inner.status.phase = "available".into();
            inner.status.received_bytes = 0;
            inner.status.total_bytes = None;
            Ok(inner.status.clone())
        }
        Some(Ok(bytes)) => {
            let mut inner = manager.lock_inner();
            inner.package = Some(bytes);
            inner.status.phase = "ready".into();
            Ok(inner.status.clone())
        }
        Some(Err(error)) => Err(manager.fail(error)),
    }
}

#[tauri::command]
pub fn cancel_update_download(
    window: tauri::WebviewWindow,
    manager: State<'_, UpdateManager>,
) -> Result<UpdateStatus, String> {
    authorize(&window)?;
    if manager.status().phase == "downloading" {
        manager.cancel.send_replace(true);
    }
    Ok(manager.status())
}

#[tauri::command]
pub async fn install_update(
    window: tauri::WebviewWindow,
    backend: State<'_, Arc<DesktopBackend>>,
    manager: State<'_, UpdateManager>,
) -> Result<UpdateStatus, String> {
    authorize(&window)?;
    let Ok(_operation) = manager.operation.try_lock() else {
        return Ok(manager.status());
    };
    let (candidate, bytes) = {
        let mut inner = manager.lock_inner();
        let candidate = inner.candidate.clone().ok_or("更新情報がありません。")?;
        let bytes = inner
            .package
            .as_ref()
            .ok_or("先に更新をダウンロードしてください。")?;
        candidate
            .verify(bytes)
            .map_err(|error| format!("更新ファイルを検証できません: {error}"))?;
        let bytes = inner.package.take().expect("verified package exists");
        inner.status.phase = "applying".into();
        (candidate, bytes)
    };
    if !backend.begin_close() {
        manager.lock_inner().package = Some(bytes);
        return Err(manager.fail("アプリの終了処理が既に進んでいます。"));
    }
    manager.shutdown_started.store(true, Ordering::Release);
    if let Err(error) = backend.close().await {
        return Err(manager.fail(format!("録画・配信の終了処理に失敗したため、更新を適用しませんでした。アプリを再起動してください: {error}")));
    }
    manager.shutdown_finished.store(true, Ordering::Release);
    let result = tauri::async_runtime::spawn_blocking(move || candidate.install(bytes)).await;
    match result {
        Ok(Ok(())) => Ok(manager.status()),
        Ok(Err(error)) => Err(manager.fail(format!(
            "更新を適用できませんでした。アプリを終了して起動し直してください: {error}"
        ))),
        Err(error) => Err(manager.fail(format!(
            "更新処理に失敗しました。アプリを終了して起動し直してください: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn artifact_scope_excludes_other_apps_and_transport_escapes() {
        for url in [
            "https://peercast-rt-downloads.fourgetkun.com/releases/1.0.2/Peercast-RT.exe",
            "https://peercast-rt-downloads.fourgetkun.com/releases/1.0.2/app.exe?download=1",
        ] {
            assert!(allowed_artifact(&Url::parse(url).unwrap()));
        }
        for url in [
            "http://peercast-rt-downloads.fourgetkun.com/releases/app.exe",
            "https://evil.test/peercast-rt/releases/app.exe",
            "https://peercast-rt-downloads.fourgetkun.com/updates/app.exe",
            "https://user:pass@peercast-rt-downloads.fourgetkun.com/releases/app.exe",
            "https://peercast-rt-downloads.fourgetkun.com:444/releases/app.exe",
            "https://peercast-rt-downloads.fourgetkun.com/releases/app.exe#x",
            "https://peercast-rt-downloads.fourgetkun.com/releases/../../updates/app.exe",
        ] {
            assert!(!allowed_artifact(&Url::parse(url).unwrap()), "{url}");
        }
    }
    #[test]
    fn bounded_read_rejects_unknown_length_overflow_without_appending() {
        let mut bytes = vec![1, 2];
        assert!(append_bounded(&mut bytes, &[3], 3).is_ok());
        assert!(append_bounded(&mut bytes, &[4], 3).is_err());
        assert_eq!(bytes, vec![1, 2, 3]);
    }
    #[test]
    fn cancellation_and_operations_do_not_discard_a_ready_package() {
        let manager = UpdateManager::default();
        manager.lock_inner().package = Some(vec![1]);
        let operation = manager.operation.try_lock().unwrap();
        assert!(manager.operation.try_lock().is_err());
        manager.cancel.send_replace(true);
        assert!(manager.lock_inner().package.is_some());
        drop(operation);
        assert!(manager.operation.try_lock().is_ok());
    }
}
