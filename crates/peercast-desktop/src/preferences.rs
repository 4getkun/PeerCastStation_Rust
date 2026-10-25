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
    pub minimize_to_tray: bool,
    pub launch_at_startup: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: 1600,
            height: 900,
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
        if !(1280..=2560).contains(&self.window.width)
            || !(720..=1440).contains(&self.window.height)
        {
            return Err("ウインドウサイズは幅1280〜2560、高さ720〜1440で指定してください".into());
        }
        Ok(self)
    }
}
