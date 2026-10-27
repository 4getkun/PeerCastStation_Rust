use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use unicode_normalization::UnicodeNormalization;

const COOLDOWN_MS: u64 = 12 * 60 * 60 * 1000;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FavoriteCandidate {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LedgerData {
    #[serde(default)]
    sent_at_ms_by_key: BTreeMap<String, u64>,
}

/// A small persistent ledger for rate-limiting favorite-channel notifications.
pub struct FavoriteNotificationLedger {
    path: PathBuf,
    sent_at_ms_by_key: BTreeMap<String, u64>,
}

impl FavoriteNotificationLedger {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            sent_at_ms_by_key: BTreeMap::new(),
        }
    }

    pub fn load(path: PathBuf) -> Result<Self, String> {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::new(path));
            }
            Err(error) => return Err(format!("通知履歴を読めませんでした: {error}")),
        };
        let data: LedgerData = serde_json::from_slice(&bytes)
            .map_err(|error| format!("通知履歴のJSONが壊れています: {error}"))?;
        Ok(Self {
            path,
            sent_at_ms_by_key: data.sent_at_ms_by_key,
        })
    }

    pub fn due(&self, rows: &[Value], now_ms: u64) -> Vec<FavoriteCandidate> {
        let mut seen = HashSet::new();
        let mut due = Vec::new();
        for row in rows {
            if row.get("isFavorite") != Some(&Value::Bool(true))
                || row.get("isHidden") == Some(&Value::Bool(true))
                || row.get("ruleMute") == Some(&Value::Bool(true))
                || row["contentType"]
                    .as_str()
                    .is_some_and(|content_type| content_type.eq_ignore_ascii_case("RAW"))
            {
                continue;
            }
            let Some(name) = row["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            let key = notification_key(row).unwrap_or_else(|| normalize_key(name));
            if key.is_empty() || seen.contains(&key) {
                continue;
            }
            if self.sent_at_ms_by_key.get(&key).is_some_and(|last_sent| {
                if *last_sent > now_ms {
                    last_sent - now_ms <= COOLDOWN_MS
                } else {
                    now_ms - last_sent < COOLDOWN_MS
                }
            }) {
                continue;
            }
            seen.insert(key.clone());
            due.push(FavoriteCandidate {
                key,
                name: name.to_owned(),
            });
        }
        due
    }

    pub fn mark_sent(&mut self, keys: &[String], now_ms: u64) {
        for key in keys {
            let normalized = if key.starts_with("youtube:") || key.starts_with("twitch:") {
                key.clone()
            } else {
                normalize_key(key)
            };
            if !normalized.is_empty() {
                self.sent_at_ms_by_key.insert(normalized, now_ms);
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&LedgerData {
            sent_at_ms_by_key: self.sent_at_ms_by_key.clone(),
        })
        .map_err(|error| format!("通知履歴をJSON化できませんでした: {error}"))?;
        if let Some(parent) = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("通知履歴の保存先を作れませんでした: {error}"))?;
        }
        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("favorite-notifications.json");
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            "{file_name}.tmp.{}_{}",
            std::process::id(),
            counter
        ));
        let result = (|| -> io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)?;
            file.write_all(&bytes)?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp_path, &self.path)?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temp_path);
            return Err(format!("通知履歴を保存できませんでした: {error}"));
        }
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    }
}

fn normalize_key(name: &str) -> String {
    name.trim().nfkc().collect::<String>().to_lowercase()
}

fn notification_key(row: &Value) -> Option<String> {
    let key = row["notificationKey"].as_str()?;
    if let Some(id) = key.strip_prefix("youtube:") {
        return crate::youtube_process::valid_channel_id(id).then(|| key.to_owned());
    }
    let id = key.strip_prefix("twitch:")?;
    (crate::twitch_process::valid_login(id) && id == id.to_ascii_lowercase())
        .then(|| key.to_owned())
}

/// A custom rule sound takes precedence over the bundled chime for this batch.
pub fn preferred_sound_path(rows: &[Value], candidates: &[FavoriteCandidate]) -> Option<String> {
    rows.iter().find_map(|row| {
        if row["isFavorite"] != true
            || row["isHidden"] == true
            || row["ruleMute"] == true
            || row["contentType"]
                .as_str()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("RAW"))
        {
            return None;
        }
        let key = notification_key(row)
            .unwrap_or_else(|| normalize_key(row["name"].as_str().unwrap_or_default()));
        if !candidates.iter().any(|candidate| candidate.key == key) {
            return None;
        }
        row["ruleSoundPath"]
            .as_str()
            .filter(|path| !path.trim().is_empty())
            .map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn youtube_channel_ids_do_not_collide_with_names_or_each_other() {
        let temp = tempfile::tempdir().unwrap();
        let mut ledger = FavoriteNotificationLedger::new(temp.path().join("ledger.json"));
        let yp = json!({"name":"同じ名前", "isFavorite":true});
        let yt = json!({"name":"同じ名前", "isFavorite":true, "notificationKey":"youtube:UCNsidkYpIAQ4QaufptQBPHQ"});
        let yt_other = json!({"name":"同じ名前", "isFavorite":true, "notificationKey":"youtube:UCnsidkYpIAQ4QaufptQBPHQ"});
        let candidates = ledger.due(&[yp.clone(), yt.clone(), yt_other.clone()], 1000);
        assert_eq!(candidates.len(), 3);
        ledger.mark_sent(
            &candidates
                .iter()
                .map(|item| item.key.clone())
                .collect::<Vec<_>>(),
            1000,
        );
        ledger.save().unwrap();
        let ledger = FavoriteNotificationLedger::load(temp.path().join("ledger.json")).unwrap();
        assert!(ledger.due(&[yp, yt, yt_other], 2000).is_empty());
    }

    fn row(
        name: &str,
        yellow_page: &str,
        favorite: bool,
        hidden: bool,
        content_type: &str,
    ) -> Value {
        json!({
            "name": name,
            "yellowPage": yellow_page,
            "isFavorite": favorite,
            "isHidden": hidden,
            "contentType": content_type,
        })
    }

    #[test]
    fn suppresses_repeats_until_exactly_twelve_hours_and_normalizes_keys() {
        let path = tempfile::tempdir().unwrap().path().join("ledger.json");
        let mut ledger = FavoriteNotificationLedger::new(path);
        let rows = [row(" ＡＲＴＩＳＴ ", "SP", true, false, "FLV")];
        let first = ledger.due(&rows, 1_000);
        assert_eq!(first[0].key, "artist");
        assert_eq!(first[0].name, "ＡＲＴＩＳＴ");
        ledger.mark_sent(&[first[0].key.clone()], 1_000);
        assert!(ledger.due(&rows, 1_000).is_empty());
        assert!(ledger.due(&rows, 1_000 + COOLDOWN_MS - 1).is_empty());
        assert_eq!(ledger.due(&rows, 1_000 + COOLDOWN_MS).len(), 1);
    }

    #[test]
    fn loads_saved_history_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("ledger.json");
        let rows = [row("Artist", "SP", true, false, "FLV")];
        let mut first = FavoriteNotificationLedger::new(path.clone());
        let candidate = first.due(&rows, 50_000).remove(0);
        first.mark_sent(&[candidate.key], 50_000);
        first.save().unwrap();
        first.mark_sent(&["another artist".into()], 50_001);
        first.save().unwrap();

        let restarted = FavoriteNotificationLedger::load(path).unwrap();
        assert!(restarted.due(&rows, 50_001).is_empty());
        assert_eq!(restarted.sent_at_ms_by_key.len(), 2);
    }

    #[test]
    fn custom_sound_overrides_default_only_for_eligible_due_favorites() {
        let ledger = FavoriteNotificationLedger::new(PathBuf::new());
        let mut hidden = row("Hidden", "SP", true, true, "FLV");
        hidden["ruleSoundPath"] = json!("hidden.wav");
        let mut muted = row("Muted", "SP", true, false, "FLV");
        muted["ruleMute"] = json!(true);
        muted["ruleSoundPath"] = json!("muted.wav");
        let mut raw = row("Artist", "SP", true, false, "RAW");
        raw["ruleSoundPath"] = json!("raw.wav");
        let mut custom = row("ＡＲＴＩＳＴ", "P@", true, false, "FLV");
        custom["ruleSoundPath"] = json!("custom.wav");
        let rows = [
            hidden,
            muted,
            raw,
            row("Artist", "SP", true, false, "FLV"),
            custom,
        ];
        let due = ledger.due(&rows, 10);
        assert_eq!(preferred_sound_path(&rows[..4], &due), None);
        assert_eq!(preferred_sound_path(&rows, &due), Some("custom.wav".into()));
        assert_eq!(preferred_sound_path(&rows, &[]), None);
    }

    #[test]
    fn skips_hidden_raw_and_empty_names_and_deduplicates_yellow_pages() {
        let ledger = FavoriteNotificationLedger::new(PathBuf::new());
        let mut muted = row("Muted", "SP", true, false, "FLV");
        muted["ruleMute"] = Value::Bool(true);
        let rows = [
            muted,
            row("Hidden", "SP", true, true, "FLV"),
            row("Raw", "SP", true, false, "RAW"),
            row("   ", "SP", true, false, "FLV"),
            row("Artist", "SP", true, false, "FLV"),
            row("ＡＲＴＩＳＴ", "P@", true, false, "OGG"),
        ];
        let due = ledger.due(&rows, 10);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].key, "artist");
        assert_eq!(due[0].name, "Artist");
    }

    #[test]
    fn future_timestamps_suppress_only_within_twelve_hours() {
        let mut ledger = FavoriteNotificationLedger::new(PathBuf::new());
        let rows = [row("Artist", "SP", true, false, "FLV")];
        ledger.mark_sent(&["artist".into()], 20_000 + COOLDOWN_MS);
        assert!(ledger.due(&rows, 20_000).is_empty());
        ledger.mark_sent(&["artist".into()], 20_001 + COOLDOWN_MS);
        assert_eq!(ledger.due(&rows, 20_000).len(), 1);
    }

    #[test]
    fn corrupt_file_is_reported_and_left_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("ledger.json");
        let corrupt = b"{broken";
        fs::write(&path, corrupt).unwrap();
        assert!(FavoriteNotificationLedger::load(path.clone()).is_err());
        assert_eq!(fs::read(path).unwrap(), corrupt);
    }
}
