use crate::recording_config::RecordingSettings;
use chrono::Local;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct RecordingFileName {
    channel: String,
    stem: String,
}

impl RecordingFileName {
    pub fn new(channel: &str, title: &str, started_at: chrono::DateTime<Local>) -> Self {
        let channel = safe_channel_name(channel);
        let timestamp = started_at.format("%Y%m%d_%H%M%S");
        let title = title.trim();
        let title = title.strip_suffix(" - <Free>").unwrap_or(title);
        let title = if title.is_empty() || title == "<Free>" {
            String::new()
        } else {
            format!("_{}", safe_channel_name(title))
        };
        let stem = format!("{channel}_{timestamp}{title}");
        let mut units = 0;
        let stem = stem
            .chars()
            .take_while(|value| {
                units += value.len_utf16();
                units <= 220
            })
            .collect::<String>()
            .trim_end_matches([' ', '.'])
            .to_owned();
        Self { channel, stem }
    }
}

pub struct RecordingTarget {
    pub file: tokio::fs::File,
    pub path: PathBuf,
}

pub fn safe_channel_name(value: &str) -> String {
    let mut name: String = value
        .chars()
        .take(64)
        .map(|value| {
            if value.is_control()
                || matches!(value, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            {
                '_'
            } else {
                value
            }
        })
        .collect();
    name = name
        .trim_matches(|value| value == ' ' || value == '.')
        .to_owned();
    if name.is_empty() {
        name = "channel".into();
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        name.insert(0, '_');
    }
    name
}

pub fn media_extension(content_type: &str) -> &'static str {
    match content_type.to_ascii_uppercase().as_str() {
        "FLV" => "flv",
        "WMV" => "wmv",
        "ASF" => "asf",
        "MP3" => "mp3",
        "OGG" => "ogg",
        "MKV" | "WEBM" => "mkv",
        "TS" | "MPEGTS" => "ts",
        "MP4" | "FMP4" => "mp4",
        _ => "bin",
    }
}

fn reject_reparse(path: &Path) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("録画用サブフォルダーにはリンクを使えません".into());
            }
        }
        if metadata.file_type().is_symlink() {
            return Err("録画用サブフォルダーにはリンクを使えません".into());
        }
    }
    Ok(())
}

/// Creates a new file without overwriting any existing recording or following a channel-name path.
pub fn create_recording_target(
    application_exe: &Path,
    settings: &RecordingSettings,
    filename: &RecordingFileName,
    content_type: &str,
    segment: u32,
) -> Result<RecordingTarget, String> {
    let requested =
        crate::platform::recording_directory(application_exe, Path::new(&settings.directory))?;
    fs::create_dir_all(&requested)
        .map_err(|error| format!("保存先フォルダーを作成できません: {error}"))?;
    let root = fs::canonicalize(requested).map_err(|error| error.to_string())?;
    let name = &filename.channel;
    let directory = if settings.channel_subfolders {
        let candidate = root.join(name);
        reject_reparse(&candidate)?;
        fs::create_dir_all(&candidate).map_err(|error| error.to_string())?;
        let canonical = fs::canonicalize(candidate).map_err(|error| error.to_string())?;
        if !canonical.starts_with(&root) {
            return Err("録画用フォルダーが保存先の外を参照しています".into());
        }
        canonical
    } else {
        root
    };
    let extension = media_extension(content_type);
    let part = if segment <= 1 {
        String::new()
    } else {
        format!("_{segment:03}")
    };
    for collision in 0..100 {
        let duplicate = if collision == 0 {
            String::new()
        } else {
            format!(" ({})", collision + 1)
        };
        let path = directory.join(format!("{}{part}{duplicate}.{extension}", filename.stem));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                return Ok(RecordingTarget {
                    file: tokio::fs::File::from_std(file),
                    path,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("録画ファイルを作成できません: {error}")),
        }
    }
    Err("重複しない録画ファイル名を作成できません".into())
}
