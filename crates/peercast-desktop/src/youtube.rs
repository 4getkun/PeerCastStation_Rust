use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use uuid::Uuid;

const MAX_CHANNELS: usize = 20;
const REFRESH_INTERVAL_MS: u64 = 5 * 60 * 1000;
const REQUEST_COOLDOWN_MS: u64 = 30 * 1000;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    YouTube,
    Twitch,
}

impl Provider {
    fn brand(self) -> &'static str {
        match self {
            Self::YouTube => "YouTube",
            Self::Twitch => "Twitch",
        }
    }

    fn is_valid_channel_id(self, id: &str) -> bool {
        match self {
            Self::YouTube => is_channel_id(id),
            Self::Twitch => crate::twitch_process::valid_login(id),
        }
    }

    fn channel_url(self, id: &str) -> String {
        match self {
            Self::YouTube => format!("https://www.youtube.com/channel/{id}/streams"),
            Self::Twitch => format!("https://www.twitch.tv/{id}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeStream {
    pub id: String,
    pub title: String,
    pub url: String,
    pub status: String,
    pub scheduled_at_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeChannel {
    pub id: String,
    pub url: String,
    pub name: String,
    pub channel_id: Option<String>,
    pub enabled: bool,
    pub last_checked_ms: Option<u64>,
    pub error: Option<String>,
    pub streams: Vec<YouTubeStream>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeSnapshot {
    #[serde(default)]
    pub channels: Vec<YouTubeChannel>,
    #[serde(default = "default_interval_minutes")]
    pub interval_minutes: u64,
    #[serde(default)]
    pub refreshing: bool,
    #[serde(default)]
    pub last_refresh_ms: Option<u64>,
    #[serde(default)]
    pub next_refresh_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeRequest {
    pub id: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeData {
    pub name: Option<String>,
    pub channel_id: Option<String>,
    pub streams: Vec<YouTubeStream>,
}

struct MonitorState {
    snapshot: YouTubeSnapshot,
    pending: HashSet<String>,
    force_requested: bool,
}

pub struct YouTubeMonitor {
    path: PathBuf,
    provider: Provider,
    state: Mutex<MonitorState>,
}

impl YouTubeMonitor {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::from_snapshot(path.into(), empty_snapshot(), Provider::YouTube)
    }

    pub fn new_twitch(path: impl Into<PathBuf>) -> Self {
        Self::from_snapshot(path.into(), empty_snapshot(), Provider::Twitch)
    }

    pub fn load(path: impl Into<PathBuf>) -> Result<Self, String> {
        Self::load_for(path.into(), Provider::YouTube)
    }

    pub fn load_twitch(path: impl Into<PathBuf>) -> Result<Self, String> {
        Self::load_for(path.into(), Provider::Twitch)
    }

    fn load_for(path: PathBuf, provider: Provider) -> Result<Self, String> {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::from_snapshot(path, empty_snapshot(), provider));
            }
            Err(error) => {
                return Err(format!(
                    "{}設定を読めませんでした: {error}",
                    provider.brand()
                ))
            }
        };
        let mut snapshot: YouTubeSnapshot = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}設定のJSONが壊れています: {error}", provider.brand()))?;
        if snapshot.channels.len() > MAX_CHANNELS {
            return Err(format!(
                "{}チャンネル数が上限{MAX_CHANNELS}件を超えています",
                provider.brand()
            ));
        }
        if snapshot.interval_minutes != 5 {
            snapshot.interval_minutes = 5;
        }
        // A process restart abandons outstanding work; the cached streams remain useful.
        snapshot.refreshing = false;
        Ok(Self::from_snapshot(path, snapshot, provider))
    }

    fn from_snapshot(path: PathBuf, snapshot: YouTubeSnapshot, provider: Provider) -> Self {
        Self {
            path,
            provider,
            state: Mutex::new(MonitorState {
                snapshot,
                pending: HashSet::new(),
                force_requested: false,
            }),
        }
    }

    pub fn snapshot(&self) -> YouTubeSnapshot {
        self.lock().snapshot.clone()
    }

    pub fn add(&self, url: impl AsRef<str>, now_ms: u64) -> Result<YouTubeSnapshot, String> {
        let url = url.as_ref().trim();
        if url.is_empty() {
            return Err(format!("{} URLが空です", self.provider.brand()));
        }
        let mut state = self.lock();
        if state.snapshot.channels.len() >= MAX_CHANNELS {
            return Err(format!(
                "{}チャンネルは最大{MAX_CHANNELS}件です",
                self.provider.brand()
            ));
        }
        if state
            .snapshot
            .channels
            .iter()
            .any(|channel| channel.url.eq_ignore_ascii_case(url))
        {
            return Err(format!(
                "この{}チャンネルは登録済みです",
                self.provider.brand()
            ));
        }
        let mut candidate = state.snapshot.clone();
        candidate.channels.push(YouTubeChannel {
            id: Uuid::new_v4().to_string(),
            url: url.to_owned(),
            name: url.to_owned(),
            channel_id: None,
            enabled: true,
            last_checked_ms: None,
            error: None,
            streams: Vec::new(),
        });
        candidate.next_refresh_ms = if candidate.next_refresh_ms == 0 {
            now_ms
        } else {
            candidate.next_refresh_ms.min(now_ms)
        };
        self.commit(&mut state, candidate)?;
        Ok(state.snapshot.clone())
    }

    pub fn remove(&self, id: &str) -> Result<YouTubeSnapshot, String> {
        let mut state = self.lock();
        let mut candidate = state.snapshot.clone();
        let old_len = candidate.channels.len();
        candidate.channels.retain(|channel| channel.id != id);
        if candidate.channels.len() == old_len {
            return Err(format!(
                "{}チャンネルが見つかりません",
                self.provider.brand()
            ));
        }
        self.commit(&mut state, candidate)?;
        state.pending.remove(id);
        state.snapshot.refreshing = !state.pending.is_empty();
        Ok(state.snapshot.clone())
    }

    pub fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
        now_ms: u64,
    ) -> Result<YouTubeSnapshot, String> {
        let mut state = self.lock();
        let mut candidate = state.snapshot.clone();
        let channel = candidate
            .channels
            .iter_mut()
            .find(|channel| channel.id == id)
            .ok_or_else(|| format!("{}チャンネルが見つかりません", self.provider.brand()))?;
        channel.enabled = enabled;
        if enabled && channel.last_checked_ms.is_none() {
            candidate.next_refresh_ms = if candidate.next_refresh_ms == 0 {
                now_ms
            } else {
                candidate.next_refresh_ms.min(now_ms)
            };
        }
        self.commit(&mut state, candidate)?;
        if !enabled {
            state.pending.remove(id);
            state.snapshot.refreshing = !state.pending.is_empty();
        }
        Ok(state.snapshot.clone())
    }

    pub fn request_refresh(&self, now_ms: u64) -> Result<YouTubeSnapshot, String> {
        let mut state = self.lock();
        if state.snapshot.refreshing || state.force_requested {
            return Err(format!("{}一覧を更新中です", self.provider.brand()));
        }
        if !state
            .snapshot
            .channels
            .iter()
            .any(|channel| channel.enabled)
        {
            return Err(format!(
                "有効な{}チャンネルがありません",
                self.provider.brand()
            ));
        }
        if state.snapshot.last_refresh_ms.is_some_and(|last| {
            now_ms.saturating_sub(last) < REQUEST_COOLDOWN_MS
                || (now_ms < last && last - now_ms < REQUEST_COOLDOWN_MS)
        }) {
            return Err(format!(
                "{}一覧は30秒以内に更新済みです",
                self.provider.brand()
            ));
        }
        let mut candidate = state.snapshot.clone();
        candidate.last_refresh_ms = Some(now_ms);
        candidate.next_refresh_ms = now_ms.saturating_add(REFRESH_INTERVAL_MS);
        candidate.refreshing = true;
        self.commit(&mut state, candidate)?;
        state.force_requested = true;
        Ok(state.snapshot.clone())
    }

    pub fn due_requests(&self, now_ms: u64) -> Vec<ProbeRequest> {
        let mut state = self.lock();
        let force = state.force_requested;
        let due_ids = state
            .snapshot
            .channels
            .iter()
            .filter(|channel| {
                channel.enabled
                    && !state.pending.contains(&channel.id)
                    && (force
                        || channel.last_checked_ms.is_none_or(|checked| {
                            now_ms.saturating_sub(checked) >= REFRESH_INTERVAL_MS
                        }))
            })
            .map(|channel| channel.id.clone())
            .collect::<Vec<_>>();
        let requests = due_ids
            .iter()
            .filter_map(|id| {
                state
                    .snapshot
                    .channels
                    .iter()
                    .find(|channel| &channel.id == id)
                    .map(|channel| ProbeRequest {
                        id: channel.id.clone(),
                        url: channel.url.clone(),
                    })
            })
            .collect::<Vec<_>>();
        state.pending.extend(due_ids);
        if force {
            state.force_requested = false;
        }
        state.snapshot.refreshing = !state.pending.is_empty();
        if !requests.is_empty() {
            state.snapshot.last_refresh_ms = Some(now_ms);
            state.snapshot.next_refresh_ms = now_ms.saturating_add(REFRESH_INTERVAL_MS);
        }
        requests
    }

    pub fn complete(&self, id: &str, result: Result<ProbeData, String>, now_ms: u64) -> Vec<Value> {
        let mut state = self.lock();
        if !state.pending.remove(id) {
            return Vec::new();
        }
        let Some(index) = state
            .snapshot
            .channels
            .iter()
            .position(|channel| channel.id == id)
        else {
            state.snapshot.refreshing = !state.pending.is_empty();
            return Vec::new();
        };
        let was_enabled = state.snapshot.channels[index].enabled;
        let mut candidate = state.snapshot.clone();
        let previous_live_ids = candidate.channels[index]
            .streams
            .iter()
            .filter(|stream| stream.status == "live")
            .map(|stream| stream.id.clone())
            .collect::<HashSet<_>>();
        let (identity_mismatch, duplicate_channel_id) = match &result {
            Ok(data) => {
                let mismatch = data.channel_id.as_ref().is_some_and(|channel_id| {
                    candidate.channels[index]
                        .channel_id
                        .as_ref()
                        .is_some_and(|known| known != channel_id)
                });
                let duplicate = data.channel_id.as_ref().is_some_and(|channel_id| {
                    candidate
                        .channels
                        .iter()
                        .enumerate()
                        .any(|(other_index, other)| {
                            other_index != index && other.channel_id.as_ref() == Some(channel_id)
                        })
                });
                (mismatch, duplicate)
            }
            Err(_) => (false, false),
        };
        let channel = &mut candidate.channels[index];
        channel.last_checked_ms = Some(now_ms);
        let mut rows = Vec::new();
        match result {
            Err(error) => channel.error = Some(error),
            Ok(data) => {
                if let Some(channel_id) = &data.channel_id {
                    if !self.provider.is_valid_channel_id(channel_id) {
                        channel.error = Some(format!(
                            "{}チャンネルIDの形式が正しくありません",
                            self.provider.brand()
                        ));
                    } else if identity_mismatch {
                        channel.error = Some(format!(
                            "監視中の{}チャンネルIDと取得結果が一致しません",
                            self.provider.brand()
                        ));
                    } else if duplicate_channel_id {
                        channel.error = Some(format!(
                            "この{}チャンネルIDは別のURLで登録済みです",
                            self.provider.brand()
                        ));
                    } else {
                        channel.channel_id = Some(channel_id.clone());
                        channel.url = self.provider.channel_url(channel_id);
                        channel.error = None;
                        if let Some(name) = data
                            .name
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                        {
                            channel.name = name.to_owned();
                        }
                        channel.streams = data.streams.clone();
                        if was_enabled {
                            for stream in data.streams.iter().filter(|stream| {
                                stream.status == "live" && !previous_live_ids.contains(&stream.id)
                            }) {
                                rows.push(json!({
                                    "name": channel.name,
                                    "isFavorite": true,
                                    "contentType": self.provider.brand(),
                                    "notificationKey": format!("{}:{channel_id}", match self.provider { Provider::YouTube => "youtube", Provider::Twitch => "twitch" }),
                                    "videoId": stream.id,
                                    "streamId": stream.id,
                                }));
                            }
                        }
                    }
                } else {
                    channel.error = Some(format!(
                        "{}チャンネルIDを確認できませんでした",
                        self.provider.brand()
                    ));
                }
            }
        }
        candidate.refreshing = !state.pending.is_empty();
        if let Err(error) = self.save(&candidate) {
            // Keep the prior cache and release the request so a later tick can retry.
            state.snapshot.refreshing = !state.pending.is_empty();
            state.snapshot.channels[index].last_checked_ms = Some(now_ms);
            state.snapshot.channels[index].error = Some(format!(
                "{}結果を保存できませんでした: {error}",
                self.provider.brand()
            ));
            return Vec::new();
        }
        state.snapshot = candidate;
        if !was_enabled {
            rows.clear();
        }
        rows
    }

    fn commit(&self, state: &mut MonitorState, candidate: YouTubeSnapshot) -> Result<(), String> {
        self.save(&candidate)?;
        state.snapshot = candidate;
        Ok(())
    }

    fn save(&self, snapshot: &YouTubeSnapshot) -> Result<(), String> {
        save_snapshot(&self.path, snapshot, self.provider.brand())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MonitorState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub fn parse_probe_json(bytes: &[u8]) -> Result<ProbeData, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("yt-dlp JSONを解析できませんでした: {error}"))?;
    let root = value
        .as_object()
        .ok_or("yt-dlp JSONのルートがオブジェクトではありません")?;

    let name = optional_string(root.get("channel"), "channel")?;
    let root_id = optional_string(root.get("id"), "id")?;
    let channel_id = optional_string(root.get("channel_id"), "channel_id")?
        .or_else(|| root_id.filter(|id| is_channel_id(id)));
    let channel_id = channel_id
        .filter(|id| is_channel_id(id))
        .ok_or("yt-dlp channel_idがないか形式が正しくありません")?;

    let entries = match root.get("entries") {
        Some(Value::Array(entries)) => entries,
        Some(_) => return Err("yt-dlp entriesが配列ではありません".into()),
        None => return Err("yt-dlp entriesがありません".into()),
    };
    let mut streams = Vec::new();
    let mut has_known_live_status = false;
    for entry in entries {
        if entry.is_null() {
            continue;
        }
        let (parsed_streams, status_known) = parse_entry(entry)?;
        streams.extend(parsed_streams);
        has_known_live_status |= status_known;
    }
    if !entries.is_empty() && !has_known_live_status {
        return Err("yt-dlp entriesに既知のlive_statusがありません".into());
    }
    Ok(ProbeData {
        name,
        channel_id: Some(channel_id),
        streams,
    })
}

fn parse_entry(entry: &Value) -> Result<(Vec<YouTubeStream>, bool), String> {
    let object = entry
        .as_object()
        .ok_or("yt-dlp entriesの要素がオブジェクトではありません")?;
    let id = object
        .get("id")
        .ok_or("yt-dlp動画IDがありません")?
        .as_str()
        .ok_or("yt-dlp動画IDが文字列ではありません")?;
    if !is_video_id(id) {
        return Err("yt-dlp動画IDの形式が正しくありません".into());
    }
    let status = match object.get("live_status") {
        None | Some(Value::Null) => return Ok((Vec::new(), false)),
        Some(Value::String(status)) if status == "is_live" => "live",
        Some(Value::String(status)) if status == "is_upcoming" => "upcoming",
        Some(Value::String(status))
            if matches!(
                status.as_str(),
                "not_live" | "was_live" | "post_live" | "is_post_live"
            ) =>
        {
            return Ok((Vec::new(), true));
        }
        Some(Value::String(status)) => {
            return Err(format!("yt-dlp live_statusが不明です: {status}"));
        }
        Some(_) => return Err("yt-dlp live_statusが文字列ではありません".into()),
    };
    let title = match object.get("title") {
        None | Some(Value::Null) => id,
        Some(Value::String(title)) => title,
        Some(_) => return Err("yt-dlp titleが文字列ではありません".into()),
    };
    let scheduled_at_ms = match object.get("release_timestamp") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => {
            let seconds = number
                .as_i64()
                .and_then(|seconds| u64::try_from(seconds).ok())
                .ok_or("yt-dlp release_timestampが非負の整数ではありません")?;
            Some(seconds.saturating_mul(1000))
        }
        Some(_) => return Err("yt-dlp release_timestampが数値ではありません".into()),
    };
    Ok((
        vec![YouTubeStream {
            id: id.to_owned(),
            title: title.to_owned(),
            url: format!("https://www.youtube.com/watch?v={id}"),
            status: status.to_owned(),
            scheduled_at_ms: if status == "upcoming" {
                scheduled_at_ms
            } else {
                None
            },
        }],
        true,
    ))
}

fn optional_string(value: Option<&Value>, field: &str) -> Result<Option<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("yt-dlp {field}が文字列ではありません")),
    }
}

fn is_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn is_channel_id(value: &str) -> bool {
    value.len() == 24
        && value.starts_with("UC")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn default_interval_minutes() -> u64 {
    5
}

fn empty_snapshot() -> YouTubeSnapshot {
    YouTubeSnapshot {
        channels: Vec::new(),
        interval_minutes: 5,
        refreshing: false,
        last_refresh_ms: None,
        next_refresh_ms: 0,
    }
}

fn save_snapshot(path: &Path, snapshot: &YouTubeSnapshot, brand: &str) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(snapshot)
        .map_err(|error| format!("{brand}設定をJSON化できませんでした: {error}"))?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("{brand}設定の保存先を作れませんでした: {error}"))?;
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("youtube.json");
    let temp = parent.join(format!(
        "{name}.tmp.{}_{}",
        std::process::id(),
        TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(format!("{brand}設定を保存できませんでした: {error}"));
    }
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CHANNEL_A: &str = "UCabcdefghijklmnopqrstuv";
    const CHANNEL_B: &str = "UCbcdefghijklmnopqrstuvw";
    const LIVE_A: &str = "abcdefghijk";
    const LIVE_B: &str = "bcdefghijkl";
    const UPCOMING: &str = "cdefghijklm";

    fn monitor() -> (tempfile::TempDir, YouTubeMonitor) {
        let dir = tempfile::tempdir().unwrap();
        let monitor = YouTubeMonitor::new(dir.path().join("config.youtube.json"));
        (dir, monitor)
    }

    fn probe(channel_id: &str, name: &str, entries: Value) -> ProbeData {
        parse_probe_json(
            json!({"channel": name, "channel_id": channel_id, "entries": entries})
                .to_string()
                .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn starts_empty_and_persists_added_channels_with_camel_case_shape() {
        let (dir, monitor) = monitor();
        assert_eq!(monitor.snapshot().interval_minutes, 5);
        let snapshot = monitor.add("https://www.youtube.com/@sample", 123).unwrap();
        assert_eq!(snapshot.channels.len(), 1);
        assert_eq!(snapshot.channels[0].name, "https://www.youtube.com/@sample");
        assert_eq!(snapshot.channels[0].id.len(), 36);
        let text = fs::read_to_string(dir.path().join("config.youtube.json")).unwrap();
        assert!(text.contains("\"channelId\""));
        assert!(text.contains("\"nextRefreshMs\""));
    }

    #[test]
    fn enforces_channel_limit_and_duplicate_url_registration() {
        let (_dir, monitor) = monitor();
        monitor.add("https://www.youtube.com/@one", 0).unwrap();
        assert!(monitor.add("https://www.youtube.com/@one", 1).is_err());
        for index in 1..MAX_CHANNELS {
            monitor
                .add(
                    format!("https://www.youtube.com/@channel{index}"),
                    index as u64,
                )
                .unwrap();
        }
        assert_eq!(monitor.snapshot().channels.len(), MAX_CHANNELS);
        assert!(monitor
            .add("https://www.youtube.com/@overflow", 99)
            .is_err());
    }

    #[test]
    fn allows_duplicate_display_names_but_rejects_duplicate_channel_ids() {
        let (_dir, monitor) = monitor();
        let first = monitor
            .add("https://youtube.com/@first", 0)
            .unwrap()
            .channels[0]
            .id
            .clone();
        let second = monitor
            .add("https://youtube.com/@second", 0)
            .unwrap()
            .channels[1]
            .id
            .clone();
        monitor.due_requests(0);
        let first_rows = monitor.complete(&first, Ok(probe(CHANNEL_A, "Same", json!([]))), 10);
        assert!(first_rows.is_empty());
        monitor.due_requests(5 * 60 * 1000);
        let second_rows = monitor.complete(
            &second,
            Ok(probe(CHANNEL_A, "Same", json!([]))),
            5 * 60 * 1000 + 1,
        );
        assert!(second_rows.is_empty());
        assert!(monitor.snapshot().channels[1]
            .error
            .as_deref()
            .unwrap()
            .contains("別のURL"));
        assert_eq!(monitor.snapshot().channels[0].name, "Same");
    }

    #[test]
    fn first_success_pins_handle_url_to_resolved_channel_id() {
        let (_dir, monitor) = monitor();
        let id = monitor
            .add("https://www.youtube.com/@weathernews", 0)
            .unwrap()
            .channels[0]
            .id
            .clone();
        monitor.due_requests(0);
        let rows = monitor.complete(
            &id,
            Ok(probe(
                CHANNEL_A,
                "Weather News",
                json!([{"id": LIVE_A, "live_status": "is_live"}]),
            )),
            1,
        );
        assert_eq!(rows.len(), 1);
        let channel = &monitor.snapshot().channels[0];
        assert_eq!(channel.channel_id.as_deref(), Some(CHANNEL_A));
        assert_eq!(
            channel.url,
            format!("https://www.youtube.com/channel/{CHANNEL_A}/streams")
        );
    }

    #[test]
    fn channel_id_mismatch_preserves_identity_and_cached_data_without_notifications() {
        let (_dir, monitor) = monitor();
        let id = monitor
            .add("https://www.youtube.com/@weathernews", 0)
            .unwrap()
            .channels[0]
            .id
            .clone();
        monitor.due_requests(0);
        let initial_rows = monitor.complete(
            &id,
            Ok(probe(
                CHANNEL_A,
                "Original name",
                json!([{"id": LIVE_A, "title": "Original live", "live_status": "is_live"}]),
            )),
            1,
        );
        assert_eq!(initial_rows.len(), 1);
        let prior = monitor.snapshot().channels[0].clone();

        monitor.due_requests(5 * 60 * 1000 + 1);
        let rows = monitor.complete(
            &id,
            Ok(probe(
                CHANNEL_B,
                "Wrong channel",
                json!([{"id": LIVE_B, "title": "Wrong live", "live_status": "is_live"}]),
            )),
            5 * 60 * 1000 + 2,
        );
        assert!(rows.is_empty());
        let channel = &monitor.snapshot().channels[0];
        assert_eq!(channel.channel_id, prior.channel_id);
        assert_eq!(channel.url, prior.url);
        assert_eq!(channel.name, prior.name);
        assert_eq!(channel.streams, prior.streams);
        assert!(channel
            .error
            .as_deref()
            .unwrap()
            .contains("IDと取得結果が一致しません"));
    }

    #[test]
    fn failed_persistence_rolls_back_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocked.json");
        fs::create_dir(&path).unwrap();
        let monitor = YouTubeMonitor::new(path);
        assert!(monitor.add("https://youtube.com/@sample", 1).is_err());
        assert!(monitor.snapshot().channels.is_empty());
    }

    #[test]
    fn failed_completion_releases_work_and_can_retry_after_storage_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("youtube.json");
        let monitor = YouTubeMonitor::new(path.clone());
        let id = monitor
            .add("https://youtube.com/@sample", 0)
            .unwrap()
            .channels[0]
            .id
            .clone();
        monitor.due_requests(0);
        monitor.complete(&id, Ok(probe(CHANNEL_A, "Sample", json!([]))), 1);
        let previous = fs::read(&path).unwrap();

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        monitor.due_requests(5 * 60 * 1000 + 1);
        let live = probe(
            CHANNEL_A,
            "Sample updated",
            json!([{"id": LIVE_A, "title": "New live", "live_status": "is_live"}]),
        );
        assert!(monitor
            .complete(&id, Ok(live.clone()), 5 * 60 * 1000 + 2)
            .is_empty());
        let failed = monitor.snapshot();
        assert!(!failed.refreshing);
        assert!(failed.channels[0]
            .error
            .as_deref()
            .unwrap()
            .contains("保存できませんでした"));
        assert!(failed.channels[0].streams.is_empty());
        assert_eq!(failed.channels[0].last_checked_ms, Some(5 * 60 * 1000 + 2));
        assert!(monitor.due_requests(5 * 60 * 1000 + 4).is_empty());

        fs::remove_dir(&path).unwrap();
        assert!(monitor.request_refresh(5 * 60 * 1000 + 30_002).is_ok());
        assert_eq!(monitor.due_requests(5 * 60 * 1000 + 30_002).len(), 1);
        let rows = monitor.complete(&id, Ok(live), 5 * 60 * 1000 + 30_003);
        assert_eq!(rows.len(), 1);
        assert!(monitor.snapshot().channels[0].error.is_none());
        assert_eq!(monitor.snapshot().channels[0].streams[0].id, LIVE_A);
        assert_ne!(fs::read(path).unwrap(), previous);
    }

    #[test]
    fn load_restores_cache_and_drops_runtime_refreshing_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("youtube.json");
        let monitor = YouTubeMonitor::new(path.clone());
        let id = monitor
            .add("https://youtube.com/@sample", 42)
            .unwrap()
            .channels[0]
            .id
            .clone();
        monitor.due_requests(42);
        let data = probe(
            CHANNEL_A,
            "Sample",
            json!([{"id": LIVE_A, "title": "Live", "live_status": "is_live"}]),
        );
        monitor.complete(&id, Ok(data), 43);
        let restored = YouTubeMonitor::load(path).unwrap();
        assert!(!restored.snapshot().refreshing);
        assert_eq!(restored.snapshot().channels[0].streams[0].title, "Live");
        assert_eq!(restored.snapshot().channels[0].last_checked_ms, Some(43));
    }

    #[test]
    fn corrupt_file_is_reported_without_being_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("youtube.json");
        fs::write(&path, b"{broken").unwrap();
        assert!(YouTubeMonitor::load(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{broken");
    }

    #[test]
    fn refresh_cooldown_and_scheduled_requests_are_deduplicated() {
        let (_dir, monitor) = monitor();
        let a = monitor.add("https://youtube.com/@a", 0).unwrap().channels[0]
            .id
            .clone();
        let b = monitor.add("https://youtube.com/@b", 0).unwrap().channels[1]
            .id
            .clone();
        assert_eq!(monitor.due_requests(0).len(), 2);
        assert!(monitor.due_requests(0).is_empty());
        assert_eq!(
            monitor.complete(&a, Ok(probe(CHANNEL_A, "A", json!([]))), 1),
            Vec::<Value>::new()
        );
        assert_eq!(
            monitor.complete(&b, Ok(probe(CHANNEL_B, "B", json!([]))), 1),
            Vec::<Value>::new()
        );
        assert!(monitor.request_refresh(29_999).is_err());
        monitor.request_refresh(30_000).unwrap();
        assert!(monitor.request_refresh(59_999).is_err());
        assert_eq!(monitor.due_requests(30_000).len(), 2);
        assert!(monitor.due_requests(30_001).is_empty());
        assert_eq!(monitor.snapshot().last_refresh_ms, Some(30_000));
    }

    #[test]
    fn disabled_removed_or_stale_requests_never_notify() {
        let (_dir, monitor) = monitor();
        let a = monitor.add("https://youtube.com/@a", 0).unwrap().channels[0]
            .id
            .clone();
        monitor.due_requests(0);
        monitor.set_enabled(&a, false, 1).unwrap();
        let data = probe(
            CHANNEL_A,
            "A",
            json!([{"id": LIVE_A, "live_status": "is_live"}]),
        );
        assert!(monitor.complete(&a, Ok(data.clone()), 2).is_empty());
        monitor.set_enabled(&a, true, 3).unwrap();
        monitor.due_requests(5 * 60 * 1000);
        monitor.remove(&a).unwrap();
        assert!(monitor.complete(&a, Ok(data), 5 * 60 * 1000 + 1).is_empty());
        assert!(monitor.snapshot().channels.is_empty());
    }

    #[test]
    fn failed_probe_keeps_cached_streams_and_sets_error() {
        let (_dir, monitor) = monitor();
        let id = monitor.add("https://youtube.com/@a", 0).unwrap().channels[0]
            .id
            .clone();
        monitor.due_requests(0);
        let data = probe(
            CHANNEL_A,
            "A",
            json!([{"id": LIVE_A, "live_status": "is_live"}]),
        );
        assert_eq!(monitor.complete(&id, Ok(data), 1).len(), 1);
        monitor.due_requests(5 * 60 * 1000 + 1);
        assert!(monitor
            .complete(&id, Err("network error".into()), 5 * 60 * 1000 + 2)
            .is_empty());
        let channel = &monitor.snapshot().channels[0];
        assert_eq!(channel.error.as_deref(), Some("network error"));
        assert_eq!(channel.streams[0].id, LIVE_A);
    }

    #[test]
    fn parser_handles_live_upcoming_multiple_entries_and_ignores_ended_streams() {
        let parsed = parse_probe_json(
            json!({
                "channel": "Creator",
                "channel_id": CHANNEL_A,
                "entries": [
                    {"id": LIVE_A, "title": "Live now", "live_status": "is_live", "is_live": null},
                    {"id": UPCOMING, "title": "Soon", "live_status": "is_upcoming", "release_timestamp": 1_800_000_000},
                    {"id": LIVE_B, "title": "Ended", "live_status": "was_live"}
                ]
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(parsed.name.as_deref(), Some("Creator"));
        assert_eq!(parsed.channel_id.as_deref(), Some(CHANNEL_A));
        assert_eq!(parsed.streams.len(), 2);
        assert_eq!(parsed.streams[0].status, "live");
        assert_eq!(parsed.streams[1].status, "upcoming");
        assert_eq!(parsed.streams[1].scheduled_at_ms, Some(1_800_000_000_000));
    }

    #[test]
    fn parser_accepts_single_live_json_and_empty_offline_channel() {
        let single = parse_probe_json(
            json!({
                "channel": "Creator",
                "channel_id": CHANNEL_A,
                "entries": [{"id": LIVE_A, "title": "Live", "live_status": "is_live"}]
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(single.streams.len(), 1);
        let offline = parse_probe_json(
            json!({"channel": "Creator", "channel_id": CHANNEL_A, "entries": []})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
        assert!(offline.streams.is_empty());
    }

    #[test]
    fn parser_rejects_incomplete_channel_shape_and_unknown_only_live_statuses() {
        assert!(parse_probe_json(br#"{"channel_id":"UCabcdefghijklmnopqrstuv"}"#).is_err());
        assert!(
            parse_probe_json(br#"{"channel_id":"UCabcdefghijklmnopqrstuv","entries":null}"#)
                .is_err()
        );
        assert!(parse_probe_json(br#"{"entries":[]}"#).is_err());

        let root_id_fallback =
            parse_probe_json(br#"{"id":"UCabcdefghijklmnopqrstuv","entries":[]}"#).unwrap();
        assert_eq!(root_id_fallback.channel_id.as_deref(), Some(CHANNEL_A));
        assert!(root_id_fallback.streams.is_empty());

        assert!(parse_probe_json(
            br#"{"channel_id":"UCabcdefghijklmnopqrstuv","entries":[{"id":"abcdefghijk","live_status":null}]}"#
        )
        .is_err());
        assert!(parse_probe_json(
            br#"{"channel_id":"UCabcdefghijklmnopqrstuv","entries":[{"id":"abcdefghijk"}]}"#
        )
        .is_err());

        let offline = parse_probe_json(
            br#"{"channel_id":"UCabcdefghijklmnopqrstuv","entries":[{"id":"abcdefghijk","live_status":"was_live"},{"id":"bcdefghijkl","live_status":null}]}"#,
        )
        .unwrap();
        assert!(offline.streams.is_empty());
    }

    #[test]
    fn parser_rejects_invalid_json_ids_types_and_unknown_shapes() {
        assert!(parse_probe_json(b"not json").is_err());
        assert!(parse_probe_json(b"[]").is_err());
        assert!(parse_probe_json(br#"{"channel_id":4,"entries":[]}"#).is_err());
        assert!(parse_probe_json(br#"{"channel_id":"UCbad","entries":[]}"#).is_err());
        assert!(
            parse_probe_json(br#"{"channel_id":"UCabcdefghijklmnopqrstuv","entries":{}}"#).is_err()
        );
        assert!(parse_probe_json(
            br#"{"channel_id":"UCabcdefghijklmnopqrstuv","entries":["bad"]}"#
        )
        .is_err());
        assert!(parse_probe_json(br#"{"channel_id":"UCabcdefghijklmnopqrstuv","entries":[{"id":"bad","live_status":"is_live"}]}"#).is_err());
        assert!(parse_probe_json(br#"{"channel_id":"UCabcdefghijklmnopqrstuv","entries":[{"id":"abcdefghijk","live_status":true}]}"#).is_err());
        assert!(parse_probe_json(br#"{"channel_id":"UCabcdefghijklmnopqrstuv","entries":[{"id":"abcdefghijk","live_status":"mystery"}]}"#).is_err());
        assert!(parse_probe_json(br#"{"channel_id":"UCabcdefghijklmnopqrstuv","entries":[{"id":"abcdefghijk","live_status":"is_upcoming","release_timestamp":1.5}]}"#).is_err());
        assert!(parse_probe_json(br#"{"channel_id":"UCabcdefghijklmnopqrstuv","entries":[{"id":"abcdefghijk","live_status":"is_upcoming","release_timestamp":-1}]}"#).is_err());
    }

    #[test]
    fn notifications_are_for_each_live_stream_and_share_channel_key() {
        let (_dir, monitor) = monitor();
        let id = monitor.add("https://youtube.com/@a", 0).unwrap().channels[0]
            .id
            .clone();
        monitor.due_requests(0);
        let data = probe(
            CHANNEL_A,
            "Creator",
            json!([
                {"id": LIVE_A, "title": "Live A", "live_status": "is_live"},
                {"id": LIVE_B, "title": "Live B", "live_status": "is_live"},
                {"id": UPCOMING, "title": "Soon", "live_status": "is_upcoming"}
            ]),
        );
        let rows = monitor.complete(&id, Ok(data), 1);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["notificationKey"], format!("youtube:{CHANNEL_A}"));
        assert_eq!(rows[0]["videoId"], LIVE_A);
        assert_eq!(rows[1]["videoId"], LIVE_B);
        assert_eq!(rows[0]["contentType"], "YouTube");
    }

    #[test]
    fn suppressed_live_start_is_not_retried_after_ledger_cooldown() {
        let (dir, monitor) = monitor();
        let ledger_path = dir.path().join("notifications.json");
        let mut ledger =
            crate::favorite_notifications::FavoriteNotificationLedger::new(ledger_path);
        let key = format!("youtube:{CHANNEL_A}");
        ledger.mark_sent(std::slice::from_ref(&key), 1);
        ledger.save().unwrap();

        let id = monitor.add("https://youtube.com/@a", 0).unwrap().channels[0]
            .id
            .clone();
        monitor.due_requests(0);
        let live = probe(
            CHANNEL_A,
            "A",
            json!([{"id": LIVE_A, "live_status": "is_live"}]),
        );
        let start_rows = monitor.complete(&id, Ok(live.clone()), 2);
        assert_eq!(ledger.due(&start_rows, 2).len(), 0);

        monitor.due_requests(5 * 60 * 1000 + 2);
        let repeated_rows = monitor.complete(&id, Ok(live), 12 * 60 * 60 * 1000 + 2);
        assert!(ledger
            .due(&repeated_rows, 12 * 60 * 60 * 1000 + 2)
            .is_empty());
    }

    #[test]
    fn twitch_cached_live_and_error_recovery_do_not_create_new_notification_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("twitch.json");
        let monitor = YouTubeMonitor::load_twitch(&path).unwrap();
        let id = monitor
            .add("https://www.twitch.tv/streamer", 0)
            .unwrap()
            .channels[0]
            .id
            .clone();
        let live = ProbeData {
            name: Some("Streamer".into()),
            channel_id: Some("streamer".into()),
            streams: vec![YouTubeStream {
                id: "123456".into(),
                title: "Live now".into(),
                url: "https://www.twitch.tv/streamer".into(),
                status: "live".into(),
                scheduled_at_ms: None,
            }],
        };
        monitor.due_requests(0);
        assert_eq!(monitor.complete(&id, Ok(live.clone()), 1).len(), 1);

        let restored = YouTubeMonitor::load_twitch(path).unwrap();
        restored.due_requests(5 * 60 * 1000 + 1);
        assert!(restored
            .complete(&id, Ok(live.clone()), 5 * 60 * 1000 + 2)
            .is_empty());
        restored.due_requests(10 * 60 * 1000 + 2);
        assert!(restored
            .complete(&id, Err("temporary failure".into()), 10 * 60 * 1000 + 3)
            .is_empty());
        restored.due_requests(15 * 60 * 1000 + 3);
        assert!(restored
            .complete(&id, Ok(live), 15 * 60 * 1000 + 4)
            .is_empty());
        restored.due_requests(20 * 60 * 1000 + 4);
        let rows = restored.complete(
            &id,
            Ok(ProbeData {
                name: Some("Streamer".into()),
                channel_id: Some("streamer".into()),
                streams: vec![YouTubeStream {
                    id: "654321".into(),
                    title: "Another live".into(),
                    url: "https://www.twitch.tv/streamer".into(),
                    status: "live".into(),
                    scheduled_at_ms: None,
                }],
            }),
            20 * 60 * 1000 + 5,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["streamId"], "654321");
    }

    #[test]
    fn upcoming_to_live_and_new_live_ids_create_notification_events() {
        let (_dir, monitor) = monitor();
        let id = monitor.add("https://youtube.com/@a", 0).unwrap().channels[0]
            .id
            .clone();
        monitor.due_requests(0);
        assert!(monitor
            .complete(
                &id,
                Ok(probe(
                    CHANNEL_A,
                    "A",
                    json!([{"id": UPCOMING, "live_status": "is_upcoming"}]),
                )),
                1,
            )
            .is_empty());
        monitor.due_requests(5 * 60 * 1000 + 1);
        let rows = monitor.complete(
            &id,
            Ok(probe(
                CHANNEL_A,
                "A",
                json!([
                    {"id": UPCOMING, "live_status": "is_live"},
                    {"id": LIVE_A, "live_status": "is_live"}
                ]),
            )),
            5 * 60 * 1000 + 2,
        );
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn twitch_monitor_persists_live_login_and_uses_separate_notification_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("twitch.json");
        let monitor = YouTubeMonitor::load_twitch(&path).unwrap();
        let id = monitor
            .add("https://www.twitch.tv/streamer", 10)
            .unwrap()
            .channels[0]
            .id
            .clone();
        let request = monitor.due_requests(10).pop().unwrap();
        let rows = monitor.complete(
            &id,
            Ok(ProbeData {
                name: Some("Streamer".into()),
                channel_id: Some("streamer".into()),
                streams: vec![YouTubeStream {
                    id: "123456".into(),
                    title: "Live now".into(),
                    url: "https://www.twitch.tv/streamer".into(),
                    status: "live".into(),
                    scheduled_at_ms: None,
                }],
            }),
            11,
        );
        assert_eq!(request.url, "https://www.twitch.tv/streamer");
        assert_eq!(rows[0]["notificationKey"], "twitch:streamer");
        assert_eq!(rows[0]["contentType"], "Twitch");
        let restored = YouTubeMonitor::load_twitch(&path).unwrap();
        assert_eq!(
            restored.snapshot().channels[0].channel_id.as_deref(),
            Some("streamer")
        );
        assert_eq!(restored.snapshot().channels[0].streams[0].status, "live");
    }

    #[test]
    fn twitch_notifications_share_generic_ledger_without_cross_provider_cooldown() {
        let temp = tempfile::tempdir().unwrap();
        let mut ledger = crate::favorite_notifications::FavoriteNotificationLedger::new(
            temp.path().join("notifications.json"),
        );
        let twitch = json!({"name":"Streamer","isFavorite":true,"contentType":"Twitch","notificationKey":"twitch:streamer"});
        let youtube = json!({"name":"Streamer","isFavorite":true,"contentType":"YouTube","notificationKey":"youtube:UCabcdefghijklmnopqrstuv"});
        let rows = [twitch.clone(), youtube.clone()];
        let due = ledger.due(&rows, 1000);
        assert_eq!(due.len(), 2);
        assert_eq!(due[0].key, "twitch:streamer");
        assert_eq!(due[1].key, "youtube:UCabcdefghijklmnopqrstuv");
        ledger.mark_sent(
            &due.iter()
                .map(|entry| entry.key.clone())
                .collect::<Vec<_>>(),
            1000,
        );
        let path = temp.path().join("notifications.json");
        ledger.save().unwrap();
        let restored =
            crate::favorite_notifications::FavoriteNotificationLedger::load(path).unwrap();
        assert!(restored
            .due(&rows, 1000 + 12 * 60 * 60 * 1000 - 1)
            .is_empty());
        assert_eq!(restored.due(&rows, 1000 + 12 * 60 * 60 * 1000).len(), 2);
    }

    #[test]
    fn refresh_request_runs_only_enabled_channels() {
        let (_dir, monitor) = monitor();
        let enabled = monitor
            .add("https://youtube.com/@enabled", 0)
            .unwrap()
            .channels[0]
            .id
            .clone();
        let disabled = monitor
            .add("https://youtube.com/@disabled", 0)
            .unwrap()
            .channels[1]
            .id
            .clone();
        monitor.set_enabled(&disabled, false, 0).unwrap();
        monitor.request_refresh(40_000).unwrap();
        let requests = monitor.due_requests(40_000);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].id, enabled);
    }

    #[test]
    fn manual_refresh_rejects_pending_work_and_no_enabled_channels() {
        let (_dir, monitor) = monitor();
        let id = monitor.add("https://youtube.com/@a", 0).unwrap().channels[0]
            .id
            .clone();
        assert!(monitor.request_refresh(0).is_ok());
        assert!(monitor.request_refresh(30_000).is_err());
        assert_eq!(monitor.due_requests(0).len(), 1);
        assert!(monitor.request_refresh(31_000).is_err());
        monitor.complete(&id, Ok(probe(CHANNEL_A, "A", json!([]))), 32_000);
        monitor.set_enabled(&id, false, 33_000).unwrap();
        assert!(monitor.request_refresh(62_000).is_err());
        assert!(!monitor.snapshot().refreshing);
    }

    #[test]
    fn simultaneous_snapshot_and_completion_are_safe() {
        let (_dir, monitor) = monitor();
        let id = monitor.add("https://youtube.com/@a", 0).unwrap().channels[0]
            .id
            .clone();
        monitor.due_requests(0);
        std::thread::scope(|scope| {
            let observer = scope.spawn(|| {
                for _ in 0..100 {
                    let snapshot = monitor.snapshot();
                    assert!(snapshot.channels.len() <= 1);
                }
            });
            let worker = scope.spawn(|| {
                let data = probe(CHANNEL_A, "A", json!([]));
                monitor.complete(&id, Ok(data), 1);
            });
            observer.join().unwrap();
            worker.join().unwrap();
        });
        assert_eq!(
            monitor.snapshot().channels[0].channel_id.as_deref(),
            Some(CHANNEL_A)
        );
    }
}
