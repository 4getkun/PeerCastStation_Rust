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
}

impl Default for DirectorySettings {
    fn default() -> Self {
        Self {
            auto_refresh: false,
            refresh_interval_secs: 300,
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
