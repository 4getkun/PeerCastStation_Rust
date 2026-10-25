use crate::rules::{normalize_rules, ChannelRule};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordingSettings {
    pub enabled: bool,
    pub directory: String,
    pub channel_subfolders: bool,
    pub limit_concurrent: bool,
    pub max_concurrent: usize,
    pub retry_count: u32,
    pub retry_interval_secs: u64,
    pub disconnect_every_retries: Option<u32>,
    pub disconnect_on_retry_end: bool,
    pub split_minutes: Option<u64>,
    pub split_size_mb: Option<u64>,
    pub all_day: bool,
    pub watch_start: String,
    pub watch_end: String,
    pub monitor_interval_secs: u64,
    pub rules: Vec<RecordingFilter>,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: "recordings".into(),
            channel_subfolders: true,
            limit_concurrent: true,
            max_concurrent: 5,
            retry_count: 30,
            retry_interval_secs: 10,
            disconnect_every_retries: Some(5),
            disconnect_on_retry_end: true,
            split_minutes: None,
            split_size_mb: None,
            all_day: true,
            watch_start: "00:00".into(),
            watch_end: "23:59".into(),
            monitor_interval_secs: 60,
            rules: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordingFilter {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub pattern: String,
    pub fields: Vec<String>,
    pub use_regex: bool,
    pub normalize_width: bool,
    pub match_case: bool,
}

impl Default for RecordingFilter {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "新しい録画条件".into(),
            enabled: true,
            pattern: String::new(),
            fields: vec!["name".into()],
            use_regex: false,
            normalize_width: true,
            match_case: false,
        }
    }
}

impl RecordingFilter {
    pub fn as_channel_rule(&self) -> ChannelRule {
        ChannelRule {
            id: self.id.clone(),
            name: self.name.clone(),
            enabled: self.enabled,
            pattern: self.pattern.clone(),
            fields: self.fields.clone(),
            use_regex: self.use_regex,
            normalize_width: self.normalize_width,
            match_case: self.match_case,
            ..ChannelRule::default()
        }
    }
}

pub fn clock_minutes(value: &str) -> Result<u32, String> {
    if value.len() != 5
        || value.as_bytes()[2] != b':'
        || !value.as_bytes()[..2]
            .iter()
            .chain(value.as_bytes()[3..].iter())
            .all(u8::is_ascii_digit)
    {
        return Err("監視時刻はHH:MM形式で指定してください".into());
    }
    let hours: u32 = value[..2]
        .parse()
        .map_err(|_| "監視時刻が正しくありません")?;
    let minutes: u32 = value[3..]
        .parse()
        .map_err(|_| "監視時刻が正しくありません")?;
    if hours > 23 || minutes > 59 {
        return Err("監視時刻が範囲外です".into());
    }
    Ok(hours * 60 + minutes)
}

impl RecordingSettings {
    pub fn normalize(mut self) -> Result<Self, String> {
        self.directory = self.directory.trim().to_owned();
        if self.directory.is_empty()
            || self.directory.len() > 32767
            || self.directory.chars().any(char::is_control)
        {
            return Err("録画の保存先フォルダーを指定してください".into());
        }
        if !(1..=32).contains(&self.max_concurrent)
            || self.retry_count > 1000
            || !(1..=3600).contains(&self.retry_interval_secs)
            || !(10..=3600).contains(&self.monitor_interval_secs)
        {
            return Err("同時録画数は1〜32件、再試行は1000回以下、間隔は1〜3600秒、監視間隔は10〜3600秒で指定してください".into());
        }
        if self
            .disconnect_every_retries
            .is_some_and(|value| value == 0 || value > 1000)
            || self
                .split_minutes
                .is_some_and(|value| value == 0 || value > 10080)
            || self
                .split_size_mb
                .is_some_and(|value| value == 0 || value > 1_000_000)
        {
            return Err("再接続・ファイル分割の設定値が範囲外です".into());
        }
        clock_minutes(&self.watch_start)?;
        clock_minutes(&self.watch_end)?;
        normalize_rules(
            self.rules
                .iter()
                .map(RecordingFilter::as_channel_rule)
                .collect(),
        )?;
        Ok(self)
    }

    pub fn within_schedule(&self, minutes: u32) -> bool {
        if self.all_day {
            return true;
        }
        let (Ok(start), Ok(end)) = (
            clock_minutes(&self.watch_start),
            clock_minutes(&self.watch_end),
        ) else {
            return false;
        };
        if start <= end {
            minutes >= start && minutes <= end
        } else {
            minutes >= start || minutes <= end
        }
    }

    pub fn concurrency(&self) -> usize {
        if self.limit_concurrent {
            self.max_concurrent
        } else {
            32
        }
    }
}
