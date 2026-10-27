#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use peercast_desktop::backend::{DesktopBackend, Limits, Snapshot, YellowPageEntry};
use peercast_desktop::favorite_notifications::FavoriteNotificationLedger;
use peercast_desktop::player::PlayerLaunch;
use peercast_desktop::updater::{self, UpdateManager};
use peercast_desktop::{
    preferences::DesktopPreferences,
    recording_config::RecordingFilter,
    rules::{ChannelRule, RulePreview},
};
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

type Backend = Arc<DesktopBackend>;
type FavoriteNotifications = Arc<Mutex<FavoriteNotificationLedger>>;

struct YouTubeService {
    monitor: Mutex<Result<peercast_desktop::youtube::YouTubeMonitor, String>>,
}

struct TwitchService {
    monitor: Mutex<Result<peercast_desktop::youtube::YouTubeMonitor, String>>,
}

fn with_youtube<T>(
    service: &YouTubeService,
    action: impl FnOnce(&mut peercast_desktop::youtube::YouTubeMonitor) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = service
        .monitor
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match guard.as_mut() {
        Ok(monitor) => action(monitor),
        Err(error) => Err(error.clone()),
    }
}

fn with_twitch<T>(
    service: &TwitchService,
    action: impl FnOnce(&mut peercast_desktop::youtube::YouTubeMonitor) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = service
        .monitor
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match guard.as_mut() {
        Ok(monitor) => action(monitor),
        Err(error) => Err(error.clone()),
    }
}

fn current_time_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

#[tauri::command]
fn youtube_status(
    window: tauri::WebviewWindow,
    service: State<'_, YouTubeService>,
) -> Result<peercast_desktop::youtube::YouTubeSnapshot, String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    with_youtube(&service, |monitor| Ok(monitor.snapshot()))
}

#[tauri::command]
fn youtube_add(
    window: tauri::WebviewWindow,
    service: State<'_, YouTubeService>,
    url: String,
) -> Result<peercast_desktop::youtube::YouTubeSnapshot, String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    let url = peercast_desktop::youtube_process::channel_url(&url)?;
    with_youtube(&service, |monitor| monitor.add(url, current_time_ms()))
}

#[tauri::command]
fn twitch_status(
    window: tauri::WebviewWindow,
    service: State<'_, TwitchService>,
) -> Result<peercast_desktop::youtube::YouTubeSnapshot, String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    with_twitch(&service, |monitor| Ok(monitor.snapshot()))
}

#[tauri::command]
fn twitch_add(
    window: tauri::WebviewWindow,
    service: State<'_, TwitchService>,
    url: String,
) -> Result<peercast_desktop::youtube::YouTubeSnapshot, String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    let url = peercast_desktop::twitch_process::channel_url(&url)?;
    with_twitch(&service, |monitor| monitor.add(url, current_time_ms()))
}

#[tauri::command]
fn twitch_remove(
    window: tauri::WebviewWindow,
    service: State<'_, TwitchService>,
    id: String,
) -> Result<peercast_desktop::youtube::YouTubeSnapshot, String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    with_twitch(&service, |monitor| monitor.remove(&id))
}

#[tauri::command]
fn twitch_set_enabled(
    window: tauri::WebviewWindow,
    service: State<'_, TwitchService>,
    id: String,
    enabled: bool,
) -> Result<peercast_desktop::youtube::YouTubeSnapshot, String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    with_twitch(&service, |monitor| {
        monitor.set_enabled(&id, enabled, current_time_ms())
    })
}

#[tauri::command]
fn twitch_refresh(
    window: tauri::WebviewWindow,
    service: State<'_, TwitchService>,
) -> Result<peercast_desktop::youtube::YouTubeSnapshot, String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    with_twitch(&service, |monitor| {
        monitor.request_refresh(current_time_ms())
    })
}

#[tauri::command]
fn youtube_remove(
    window: tauri::WebviewWindow,
    service: State<'_, YouTubeService>,
    id: String,
) -> Result<peercast_desktop::youtube::YouTubeSnapshot, String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    with_youtube(&service, |monitor| monitor.remove(&id))
}

#[tauri::command]
fn youtube_set_enabled(
    window: tauri::WebviewWindow,
    service: State<'_, YouTubeService>,
    id: String,
    enabled: bool,
) -> Result<peercast_desktop::youtube::YouTubeSnapshot, String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    with_youtube(&service, |monitor| {
        monitor.set_enabled(&id, enabled, current_time_ms())
    })
}

#[tauri::command]
fn youtube_refresh(
    window: tauri::WebviewWindow,
    service: State<'_, YouTubeService>,
) -> Result<peercast_desktop::youtube::YouTubeSnapshot, String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    with_youtube(&service, |monitor| {
        monitor.request_refresh(current_time_ms())
    })
}

#[tauri::command]
async fn youtube_open(
    window: tauri::WebviewWindow,
    service: State<'_, YouTubeService>,
    video_id: String,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    let url = peercast_desktop::youtube_process::video_url(&video_id)?;
    let stream_url = with_youtube(&service, |monitor| {
        let channel = monitor.snapshot().channels.into_iter().find(|channel| {
            channel.enabled
                && channel.error.is_none()
                && channel
                    .streams
                    .iter()
                    .any(|stream| stream.id == video_id && stream.status == "live")
        });
        channel
            .map(|_| url.clone())
            .ok_or_else(|| "登録したチャンネルの現在の配信を選択してください".into())
    })?;
    peercast_desktop::live_player::open_live(&stream_url).await
}

#[tauri::command]
async fn youtube_open_browser(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    service: State<'_, YouTubeService>,
    video_id: String,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    let url = peercast_desktop::youtube_process::video_url(&video_id)?;
    let known = with_youtube(&service, |monitor| {
        Ok(monitor.snapshot().channels.iter().any(|channel| {
            channel.enabled
                && channel.error.is_none()
                && channel
                    .streams
                    .iter()
                    .any(|stream| stream.id == video_id && stream.status == "live")
        }))
    })?;
    if !known {
        return Err("登録したチャンネルの現在の配信を選択してください".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn twitch_open(
    window: tauri::WebviewWindow,
    service: State<'_, TwitchService>,
    stream_id: String,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    let url = with_twitch(&service, |monitor| {
        let channel = monitor
            .snapshot()
            .channels
            .into_iter()
            .find(|channel| {
                channel.enabled
                    && channel.error.is_none()
                    && channel
                        .streams
                        .iter()
                        .any(|stream| stream.id == stream_id && stream.status == "live")
            })
            .ok_or_else(|| "登録したチャンネルの現在の配信を選択してください".to_owned())?;
        peercast_desktop::twitch_process::channel_url(&channel.url)
    })?;
    peercast_desktop::live_player::open_live(&url).await
}

#[tauri::command]
async fn twitch_open_browser(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    service: State<'_, TwitchService>,
    stream_id: String,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    let url = with_twitch(&service, |monitor| {
        let channel = monitor
            .snapshot()
            .channels
            .into_iter()
            .find(|channel| {
                channel.enabled
                    && channel.error.is_none()
                    && channel
                        .streams
                        .iter()
                        .any(|stream| stream.id == stream_id && stream.status == "live")
            })
            .ok_or_else(|| "登録したチャンネルの現在の配信を選択してください".to_owned())?;
        peercast_desktop::twitch_process::channel_url(&channel.url)
    })?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn start_broadcast(
    backend: State<'_, Backend>,
    request: peercast_desktop::broadcast::BroadcastRequest,
) -> Result<Snapshot, String> {
    backend.start_broadcast(request).await
}

#[tauri::command]
async fn stop_broadcast(
    backend: State<'_, Backend>,
    channel_id: String,
) -> Result<Snapshot, String> {
    backend.stop_broadcast(&channel_id).await
}

#[tauri::command]
async fn apply_preferences(
    app: tauri::AppHandle,
    backend: State<'_, Backend>,
    limits: Limits,
    yellow_pages: Vec<YellowPageEntry>,
    desktop: Option<DesktopPreferences>,
) -> Result<Snapshot, String> {
    let before = backend.snapshot().await?;
    let next = desktop.map(DesktopPreferences::normalize).transpose()?;
    let startup_changed = next.as_ref().is_some_and(|settings| {
        settings.window.launch_at_startup != before.desktop.window.launch_at_startup
    });
    let startup_before = if startup_changed {
        Some(
            app.autolaunch()
                .is_enabled()
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    if startup_changed {
        set_startup(&app, next.as_ref().unwrap().window.launch_at_startup)?;
    }
    let result = match next {
        Some(desktop) => {
            backend
                .apply_desktop_preferences(limits, yellow_pages, desktop)
                .await
        }
        None => backend.apply_preferences(limits, yellow_pages).await,
    };
    match result {
        Ok(saved) => {
            let changed = before.desktop.window.width != saved.desktop.window.width
                || before.desktop.window.height != saved.desktop.window.height
                || before.desktop.window.logical_size != saved.desktop.window.logical_size;
            peercast_desktop::native_window::apply(&app, &saved.desktop.window, changed)?;
            Ok(saved)
        }
        Err(error) => {
            if let Some(previous) = startup_before {
                let _ = set_startup(&app, previous);
            }
            Err(error)
        }
    }
}

fn set_startup(app: &tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|error| format!("スタートアップ設定を変更できません: {error}"))
}

#[tauri::command]
async fn choose_file(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    kind: String,
) -> Result<Option<String>, String> {
    if window.label() != "main" {
        return Err("メイン画面から実行してください".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let picker = app.dialog().file().set_parent(&window);
        let selected = match kind.as_str() {
            "player" => picker
                .set_title("プレイヤーを選択")
                .add_filter("実行ファイル", &["exe"])
                .blocking_pick_file(),
            "sound" => picker
                .set_title("通知音を選択")
                .add_filter("WAVファイル", &["wav"])
                .blocking_pick_file(),
            "directory" => picker
                .set_title("録画の保存先を選択")
                .blocking_pick_folder(),
            _ => return Err("選択するファイルの種類が正しくありません".into()),
        };
        selected
            .map(|path| {
                path.into_path()
                    .map(|path| path.to_string_lossy().into_owned())
                    .map_err(|error| error.to_string())
            })
            .transpose()
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn preview_channel_rules(
    backend: State<'_, Backend>,
    rules: Vec<ChannelRule>,
) -> Result<RulePreview, String> {
    backend.preview_channel_rules(rules).await
}

#[tauri::command]
async fn set_channel_preference(
    backend: State<'_, Backend>,
    channel_name: String,
    kind: String,
) -> Result<Snapshot, String> {
    let snapshot = backend.snapshot().await?;
    let mut desktop = snapshot.desktop;
    peercast_desktop::rules::upsert_quick_channel_rule(
        &mut desktop.channel_rules,
        &channel_name,
        &kind,
    )?;
    backend
        .apply_desktop_preferences(snapshot.limits, snapshot.yellow_pages, desktop)
        .await
}

#[tauri::command]
async fn preview_recording_rules(
    backend: State<'_, Backend>,
    rules: Vec<RecordingFilter>,
) -> Result<RulePreview, String> {
    backend
        .preview_channel_rules(rules.iter().map(RecordingFilter::as_channel_rule).collect())
        .await
}

#[tauri::command]
async fn preview_sound(path: String) -> Result<(), String> {
    peercast_desktop::audio::play_sound(path).await
}

#[tauri::command]
async fn play_rule_sound(backend: State<'_, Backend>, rule_id: String) -> Result<(), String> {
    if let Some(path) = backend.rule_sound_path(&rule_id).await? {
        peercast_desktop::audio::play_sound(path).await?;
    }
    Ok(())
}

#[tauri::command]
async fn enqueue_recording(
    backend: State<'_, Backend>,
    channel_id: String,
    yellow_page: String,
    keep: bool,
) -> Result<String, String> {
    backend
        .enqueue_recording(channel_id, yellow_page, keep)
        .await
}

#[tauri::command]
async fn control_recording(
    backend: State<'_, Backend>,
    job_id: String,
    action: String,
) -> Result<Snapshot, String> {
    backend.control_recording(&job_id, &action).await?;
    backend.snapshot().await
}

#[tauri::command]
async fn open_recording_folder(
    backend: State<'_, Backend>,
    job_id: Option<String>,
) -> Result<(), String> {
    let path = if let Some(job_id) = job_id {
        backend
            .snapshot()
            .await?
            .recordings
            .into_iter()
            .find(|job| job.id == job_id)
            .and_then(|job| job.current_file)
            .and_then(|path| {
                std::path::PathBuf::from(path)
                    .parent()
                    .map(std::path::Path::to_path_buf)
            })
            .ok_or("録画ファイルの保存先がまだありません")?
    } else {
        let settings = backend.recording_settings().await?;
        let path = std::path::PathBuf::from(settings.directory);
        if path.is_absolute() {
            path
        } else {
            std::env::current_exe()
                .map_err(|error| error.to_string())?
                .parent()
                .ok_or("アプリの配置先を取得できません")?
                .join(path)
        }
    };
    std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    let explorer = std::path::PathBuf::from(
        std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()),
    )
    .join("explorer.exe");
    std::process::Command::new(explorer)
        .arg({
            let value = path.to_string_lossy();
            if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
                format!(r"\\{unc}")
            } else {
                value.strip_prefix(r"\\?\").unwrap_or(&value).to_owned()
            }
        })
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
async fn control_relay_channel(
    window: tauri::WebviewWindow,
    backend: State<'_, Backend>,
    channel_id: String,
    session_id: String,
    action: String,
) -> Result<Snapshot, String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    backend
        .control_relay_channel(&channel_id, &session_id, &action)
        .await
}

#[tauri::command]
async fn recording_file_action(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    backend: State<'_, Backend>,
    job_id: String,
    action: String,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    if !matches!(action.as_str(), "play" | "reveal" | "trash") {
        return Err("ファイル操作が正しくありません".into());
    }
    let lease = backend
        .reserve_recording_file_action(&job_id, action == "trash")
        .await?;
    let requested_path = lease.path.clone();
    let validated = tauri::async_runtime::spawn_blocking(move || {
        peercast_desktop::recording_file::validate_recording_file(&requested_path)
    })
    .await
    .map_err(|error| error.to_string())??;
    let deleting = action == "trash";
    tauri::async_runtime::spawn_blocking(move || match action.as_str() {
        "play" => app
            .opener()
            .open_path(validated.to_string_lossy(), None::<&str>)
            .map_err(|error| error.to_string()),
        "reveal" => app
            .opener()
            .reveal_item_in_dir(validated)
            .map_err(|error| error.to_string()),
        "trash" => trash::delete(validated)
            .map_err(|error| format!("ごみ箱へ移動できませんでした: {error}")),
        _ => unreachable!("validated file action"),
    })
    .await
    .map_err(|error| error.to_string())??;
    if deleting {
        backend
            .forget_deleted_recording_file(&job_id, &lease.path)
            .await?;
    }
    Ok(())
}

#[tauri::command]
async fn get_yp_channels(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    backend: State<'_, Backend>,
    notifications: State<'_, FavoriteNotifications>,
    refresh: bool,
    notify_favorites: Option<bool>,
) -> Result<serde_json::Value, String> {
    if window.label() != "main" {
        return Err("メイン画面から実行してください".into());
    }
    let mut rows = backend.get_yp_channels(refresh).await?;
    if notify_favorites == Some(true) {
        notify_favorite_rows(&app, &notifications, &mut rows);
    }
    Ok(rows)
}

fn notify_favorite_rows(
    app: &tauri::AppHandle,
    notifications: &Mutex<FavoriteNotificationLedger>,
    rows: &mut serde_json::Value,
) {
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let mut ledger = notifications
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let candidates = ledger.due(rows.as_array().unwrap_or(&Vec::new()), now);
    if !candidates.is_empty() {
        let names = candidates
            .iter()
            .take(5)
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>()
            .join("、");
        let remaining = candidates.len().saturating_sub(5);
        let body = if remaining == 0 {
            format!("{names} が配信しています。")
        } else {
            format!("{names}、ほか{remaining}件が配信しています。")
        };
        match app
            .notification()
            .builder()
            .title("お気に入りの配信が見つかりました")
            .body(body)
            .show()
        {
            Ok(()) => {
                let sound_path = peercast_desktop::favorite_notifications::preferred_sound_path(
                    rows.as_array().map(Vec::as_slice).unwrap_or(&[]),
                    &candidates,
                );
                tauri::async_runtime::spawn(async move {
                    let result = match sound_path {
                        Some(path) => peercast_desktop::audio::play_sound(path).await,
                        None => peercast_desktop::audio::play_favorite_chime().await,
                    };
                    if let Err(error) = result {
                        eprintln!("お気に入り通知音を再生できませんでした: {error}");
                    }
                });
                let keys = candidates
                    .iter()
                    .map(|item| item.key.clone())
                    .collect::<Vec<_>>();
                ledger.mark_sent(&keys, now);
                if let Err(error) = ledger.save() {
                    eprintln!("{error}");
                }
                if let Some(rows) = rows.as_array_mut() {
                    for row in rows {
                        if candidates.iter().any(|item| {
                            row["name"]
                                .as_str()
                                .is_some_and(|name| name.trim() == item.name)
                        }) {
                            row["favoriteNotificationSent"] = serde_json::Value::Bool(true);
                        }
                    }
                }
            }
            Err(error) => eprintln!("お気に入り通知を表示できませんでした: {error}"),
        }
    }
}

#[tauri::command]
async fn play_yp_channel(
    backend: State<'_, Backend>,
    channel_id: String,
    yellow_page: String,
) -> Result<PlayerLaunch, String> {
    backend.play_yp_channel(&channel_id, &yellow_page).await
}

#[tauri::command]
async fn snapshot(backend: State<'_, Backend>) -> Result<Snapshot, String> {
    backend.snapshot().await
}

#[tauri::command]
fn clear_logs(window: tauri::WebviewWindow, backend: State<'_, Backend>) -> Result<(), String> {
    if window.label() != "main" {
        return Err("メイン画面から操作してください".into());
    }
    backend.clear_logs()
}

#[tauri::command]
async fn apply_limits(backend: State<'_, Backend>, limits: Limits) -> Result<Snapshot, String> {
    backend.apply_limits(limits).await
}

#[tauri::command]
async fn apply_yellow_pages(
    backend: State<'_, Backend>,
    yellow_pages: Vec<YellowPageEntry>,
) -> Result<Snapshot, String> {
    backend.apply_yellow_pages(yellow_pages).await
}

#[tauri::command]
async fn run_probe(backend: State<'_, Backend>) -> Result<Snapshot, String> {
    backend.run_probe().await
}

#[tauri::command]
async fn restart_backend(backend: State<'_, Backend>) -> Result<Snapshot, String> {
    backend.restart().await
}

#[tauri::command]
async fn stop_backend(backend: State<'_, Backend>) -> Result<Snapshot, String> {
    backend.stop().await
}

#[tauri::command]
fn start_main_window_drag(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("メインウィンドウ以外は操作できません".to_string());
    }
    window.start_dragging().map_err(|error| error.to_string())
}

#[tauri::command]
fn minimize_main_window(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("メインウィンドウ以外は操作できません".to_string());
    }
    window.minimize().map_err(|error| error.to_string())
}

#[tauri::command]
fn toggle_main_window_maximize(window: tauri::WebviewWindow) -> Result<bool, String> {
    if window.label() != "main" {
        return Err("メインウィンドウ以外は操作できません".to_string());
    }
    if window.is_maximized().map_err(|error| error.to_string())? {
        window.unmaximize().map_err(|error| error.to_string())?;
        Ok(false)
    } else {
        window.maximize().map_err(|error| error.to_string())?;
        Ok(true)
    }
}

#[tauri::command]
async fn close_window(app: tauri::AppHandle, backend: State<'_, Backend>) -> Result<(), String> {
    if backend.begin_close() || app.state::<UpdateManager>().close_after_update_error() {
        let result = backend.close().await;
        app.exit(if result.is_ok() { 0 } else { 1 });
        result
    } else {
        Ok(())
    }
}

fn main() {
    tauri::Builder::default()
        .manage(UpdateManager::default())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            peercast_desktop::native_window::restore_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::Builder::new().app_name("Peercast RT").build())
        .setup(|app| {
            let mut path = app.path().app_local_data_dir()?.join("tauri-test.json");
            let mut arguments = std::env::args_os().skip(1);
            while let Some(argument) = arguments.next() {
                if argument == "--settings" {
                    path = arguments.next().ok_or_else(|| {
                        std::io::Error::other("--settings には設定ファイルのパスが必要です")
                    })?.into();
                }
            }
            let ledger_path = path.with_extension("favorite-notifications.json");
            let ledger = FavoriteNotificationLedger::load(ledger_path.clone()).unwrap_or_else(|error| {
                eprintln!("{error}");
                FavoriteNotificationLedger::new(ledger_path)
            });
            app.manage(Arc::new(Mutex::new(ledger)) as FavoriteNotifications);
            let youtube_path = path.with_extension("youtube.json");
            app.manage(YouTubeService { monitor: Mutex::new(peercast_desktop::youtube::YouTubeMonitor::load(youtube_path)) });
            let twitch_path = path.with_extension("twitch.json");
            app.manage(TwitchService { monitor: Mutex::new(peercast_desktop::youtube::YouTubeMonitor::load_twitch(twitch_path)) });
            let youtube_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let requests = with_youtube(&youtube_app.state::<YouTubeService>(), |monitor| Ok(monitor.due_requests(current_time_ms()))).unwrap_or_default();
                    for request in requests {
                        let result = match std::env::current_exe().map_err(|error| error.to_string()).and_then(|exe| peercast_desktop::youtube_process::bundled_executable(&exe)) {
                            Ok(executable) => peercast_desktop::youtube_process::probe(&executable, &request.url).await,
                            Err(error) => Err(error),
                        };
                        let rows = with_youtube(&youtube_app.state::<YouTubeService>(), |monitor| Ok(monitor.complete(&request.id, result, current_time_ms()))).unwrap_or_default();
                        if !rows.is_empty() {
                            let ledger = youtube_app.state::<FavoriteNotifications>();
                            notify_favorite_rows(&youtube_app, &ledger, &mut serde_json::Value::Array(rows));
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            });
            let twitch_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let requests = with_twitch(&twitch_app.state::<TwitchService>(), |monitor| Ok(monitor.due_requests(current_time_ms()))).unwrap_or_default();
                    for request in requests {
                        let result = match std::env::current_exe().map_err(|error| error.to_string()).and_then(|exe| peercast_desktop::youtube_process::bundled_executable(&exe)) {
                            Ok(executable) => peercast_desktop::twitch_process::probe(&executable, &request.url).await,
                            Err(error) => Err(error),
                        };
                        let rows = with_twitch(&twitch_app.state::<TwitchService>(), |monitor| Ok(monitor.complete(&request.id, result, current_time_ms()))).unwrap_or_default();
                        if !rows.is_empty() {
                            let ledger = twitch_app.state::<FavoriteNotifications>();
                            notify_favorite_rows(&twitch_app, &ledger, &mut serde_json::Value::Array(rows));
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            });
            let backend = tauri::async_runtime::block_on(DesktopBackend::open_detecting(path))
                .map_err(std::io::Error::other)?;
            let saved = tauri::async_runtime::block_on(backend.snapshot()).map_err(std::io::Error::other)?;
            peercast_desktop::native_window::setup(app.handle()).map_err(std::io::Error::other)?;
            peercast_desktop::native_window::apply(app.handle(), &saved.desktop.window, true).map_err(std::io::Error::other)?;
            if saved.desktop.window.launch_at_startup {
                set_startup(app.handle(), true).map_err(std::io::Error::other)?;
            }
            app.manage(backend);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            updater::update_status,
            updater::check_update,
            updater::download_update,
            updater::cancel_update_download,
            updater::install_update,
            snapshot,
            clear_logs,
            start_broadcast,
            stop_broadcast,
            get_yp_channels,
            youtube_status,
            youtube_add,
            youtube_remove,
            youtube_set_enabled,
            youtube_refresh,
            youtube_open,
            youtube_open_browser,
            twitch_status,
            twitch_add,
            twitch_remove,
            twitch_set_enabled,
            twitch_refresh,
            twitch_open,
            twitch_open_browser,
            play_yp_channel,
            apply_limits,
            apply_yellow_pages,
            apply_preferences,
            choose_file,
            preview_channel_rules,
            set_channel_preference,
            preview_recording_rules,
            preview_sound,
            play_rule_sound,
            enqueue_recording,
            control_recording,
            open_recording_folder,
            control_relay_channel,
            recording_file_action,
            run_probe,
            restart_backend,
            stop_backend,
            start_main_window_drag,
            minimize_main_window,
            toggle_main_window_maximize,
            close_window
        ])
        .on_window_event(|window, event| {
            peercast_desktop::native_window::handle_window_event(window, event);
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                api.prevent_close();
                if window.app_handle().get_webview_window("main").is_some_and(|view| {
                    view.eval("if (typeof window.requestMainClose === 'function') { window.requestMainClose(); } else { window.__TAURI__.core.invoke('close_window'); }").is_ok()
                })
                {
                    return;
                }
                let app = window.app_handle().clone();
                let backend = Arc::clone(&app.state::<Backend>());
                if backend.begin_close() {
                    tauri::async_runtime::spawn(async move {
                        let exit_code = if backend.close().await.is_ok() { 0 } else { 1 };
                        app.exit(exit_code);
                    });
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run Peercast RT");
}
