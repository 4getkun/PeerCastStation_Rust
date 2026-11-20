use crate::{
    player::PlayerSettings,
    rules::{self, ChannelRule},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopPreferences {
    pub player: PlayerSettings,
    pub channel_rules: Vec<ChannelRule>,
    pub recording: crate::recording_config::RecordingSettings,
    pub directory: DirectorySettings,
    pub window: WindowSettings,
    pub last_broadcast: Option<crate::broadcast::BroadcastRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectorySettings {
    pub auto_refresh: bool,
    pub refresh_interval_secs: u64,
    pub youtube_at_bottom: bool,
    pub twitch_at_bottom: bool,
    pub yp_defaults_version: u32,
}

impl Default for DirectorySettings {
    fn default() -> Self {
        Self {
            auto_refresh: true,
            refresh_interval_secs: 300,
            youtube_at_bottom: false,
            twitch_at_bottom: false,
            yp_defaults_version: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowSettings {
    pub width: u32,
    pub height: u32,
    pub presets: Vec<WindowSizePreset>,
    pub logical_size: bool,
    pub minimize_to_tray: bool,
    pub launch_at_startup: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowSizePreset {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: 1600,
            height: 900,
            presets: Vec::new(),
            logical_size: false,
            minimize_to_tray: false,
            launch_at_startup: false,
        }
    }
}

impl DesktopPreferences {
    pub fn normalize(mut self) -> Result<Self, String> {
        self.player = self.player.normalize()?;
        self.channel_rules = rules::normalize_rules(self.channel_rules)?;
        self.recording = self.recording.normalize()?;
        self.last_broadcast = self
            .last_broadcast
            .map(crate::broadcast::BroadcastRequest::normalize)
            .transpose()?;
        if !(15..=3600).contains(&self.directory.refresh_interval_secs) {
            return Err("自動更新間隔は1〜60分で指定してください".into());
        }
        self.directory.refresh_interval_secs =
            self.directory.refresh_interval_secs.div_ceil(60) * 60;
        validate_window_size(self.window.width, self.window.height)?;
        if self.window.presets.len() > 32 {
            return Err("ウインドウサイズプリセットは32件まで登録できます".into());
        }
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for preset in &mut self.window.presets {
            preset.name = preset.name.trim().to_owned();
            if preset.id.trim().is_empty() || preset.id.chars().count() > 80 {
                return Err("ウインドウサイズプリセットのIDは1〜80文字で指定してください".into());
            }
            if preset.name.is_empty() || preset.name.chars().count() > 40 {
                return Err("ウインドウサイズプリセット名は1〜40文字で指定してください".into());
            }
            if !ids.insert(preset.id.as_str()) {
                return Err("ウインドウサイズプリセットのIDは重複できません".into());
            }
            if !names.insert(preset.name.to_lowercase()) {
                return Err("ウインドウサイズプリセット名は重複できません".into());
            }
            validate_window_size(preset.width, preset.height)?;
        }
        Ok(self)
    }
}

fn validate_window_size(width: u32, height: u32) -> Result<(), String> {
    if !(1280..=7680).contains(&width) || !(720..=4320).contains(&height) {
        return Err("ウインドウサイズは幅1280〜7680、高さ720〜4320で指定してください".into());
    }
    Ok(())
}

const LEGACY_APP_IDENTIFIER: &str = "com.fourgetkun.peercast-rt";
const LEGACY_MIGRATION_FILES: [&str; 4] = [
    "tauri-test.json",
    "tauri-test.favorite-notifications.json",
    "tauri-test.youtube.json",
    "tauri-test.twitch.json",
];

/// Copies persisted settings from the former `com.fourgetkun.peercast-rt` data directory
/// into the current PeerCast RS data directory on first launch after renaming.
pub fn migrate_legacy_app_data(target_dir: &std::path::Path) -> std::io::Result<usize> {
    let Some(parent) = target_dir.parent() else {
        return Ok(0);
    };
    let legacy_dir = parent.join(LEGACY_APP_IDENTIFIER);
    if legacy_dir == target_dir || !legacy_dir.is_dir() {
        return Ok(0);
    }
    if target_dir.join(LEGACY_MIGRATION_FILES[0]).exists() {
        return Ok(0);
    }
    let mut copied = 0usize;
    for filename in LEGACY_MIGRATION_FILES {
        let source = legacy_dir.join(filename);
        let destination = target_dir.join(filename);
        if source.is_file() && !destination.exists() {
            std::fs::create_dir_all(target_dir)?;
            std::fs::copy(&source, &destination)?;
            copied += 1;
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod migration_tests {
    use super::migrate_legacy_app_data;

    #[test]
    fn migrates_legacy_peercast_rt_files_once_without_overwriting() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("com.fourgetkun.peercast-rt");
        let target = root.path().join("com.fourgetkun.peercast-rs");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("tauri-test.json"), r#"{"migrated":true}"#).unwrap();
        std::fs::write(legacy.join("tauri-test.youtube.json"), r#"{"channels":[]}"#).unwrap();

        assert_eq!(migrate_legacy_app_data(&target).unwrap(), 2);
        assert_eq!(
            std::fs::read_to_string(target.join("tauri-test.json")).unwrap(),
            r#"{"migrated":true}"#
        );
        assert_eq!(
            std::fs::read_to_string(target.join("tauri-test.youtube.json")).unwrap(),
            r#"{"channels":[]}"#
        );

        std::fs::write(target.join("tauri-test.json"), r#"{"migrated":false}"#).unwrap();
        assert_eq!(migrate_legacy_app_data(&target).unwrap(), 0);
        assert_eq!(
            std::fs::read_to_string(target.join("tauri-test.json")).unwrap(),
            r#"{"migrated":false}"#
        );
    }
}
