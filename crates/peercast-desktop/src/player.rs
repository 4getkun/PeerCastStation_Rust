use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(not(target_os = "linux"))]
pub const DEFAULT_PLAYER_ARGUMENTS: &str = "\"<stream/>\" \"<channelname/>\" \"<contact/>\"";

#[cfg(target_os = "linux")]
pub const DEFAULT_PLAYER_ARGUMENTS: &str = "\"<stream/>\"";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayerSettings {
    pub executable: Option<String>,
    pub arguments: String,
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self {
            executable: None,
            arguments: DEFAULT_PLAYER_ARGUMENTS.into(),
        }
    }
}

impl PlayerSettings {
    pub fn normalize(mut self) -> Result<Self, String> {
        if let Some(path) = self.executable.take() {
            if path.chars().any(char::is_control) {
                return Err("プレイヤーには実行ファイルのパスを指定してください".into());
            }
            let path = path.trim().to_owned();
            if !path.is_empty() {
                if path.len() > 32767
                    || path.chars().any(char::is_control)
                    || (cfg!(windows)
                        && !Path::new(&path)
                            .extension()
                            .and_then(|value| value.to_str())
                            .is_some_and(|value| value.eq_ignore_ascii_case("exe")))
                {
                    return Err("プレイヤーには実行ファイルのパスを指定してください".into());
                }
                #[cfg(target_os = "linux")]
                {
                    if !Path::new(&path).is_absolute() {
                        return Err(
                            "プレイヤーは絶対パスで指定してください（例: /usr/bin/mpv）".into()
                        );
                    }
                    // Configuration must remain loadable when a removable player is absent.
                    // Existence and execution permission are checked immediately before launch.
                }
                self.executable = Some(path);
            }
        }
        if self.arguments.len() > 8192 || self.arguments.contains('\0') {
            return Err("起動引数は8192文字以内で指定してください".into());
        }
        split_arguments(&self.arguments)?;
        Ok(self)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerLaunch {
    pub player_path: String,
    pub playlist_url: String,
    pub process_id: u32,
}

#[cfg(not(target_os = "linux"))]
pub fn bundled_player_path(application_exe: &Path) -> Result<PathBuf, String> {
    let application_dir = application_exe
        .parent()
        .ok_or("アプリの配置先を取得できません")?;
    let adjacent = application_dir.join("PCRPlayer").join("PCRPlayer64.exe");
    if adjacent.is_file() {
        return Ok(adjacent);
    }
    let resource = application_dir
        .join("resources")
        .join("PCRPlayer")
        .join("PCRPlayer64.exe");
    if resource.is_file() {
        return Ok(resource);
    }
    Ok(adjacent)
}

#[cfg(target_os = "linux")]
pub fn bundled_player_path(_application_exe: &Path) -> Result<PathBuf, String> {
    // Deliberately do not search PATH or the current directory for executable code.
    Ok(PathBuf::from("/usr/bin/mpv"))
}

pub fn default_player_path() -> Result<PathBuf, String> {
    bundled_player_path(&std::env::current_exe().map_err(|e| e.to_string())?)
}

// PCRPlayer v0.2.7 accepts a stream URL, channel name, and contact URL; no shell is involved.
pub fn player_command(
    application_exe: &Path,
    playlist_url: &str,
    channel_name: &str,
    contact_url: &str,
) -> Result<Command, String> {
    let url = reqwest::Url::parse(playlist_url).map_err(|_| "再生URLが正しくありません")?;
    let channel = url
        .path()
        .strip_prefix("/pls/")
        .map(|s| s.strip_suffix(".m3u").unwrap_or(s));
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.port().is_some_and(|port| port > 0)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !channel.is_some_and(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("このアプリのローカル再生URLだけを開けます".into());
    }
    #[cfg(target_os = "linux")]
    let _ = (channel_name, contact_url);
    #[cfg(not(target_os = "linux"))]
    let contact = reqwest::Url::parse(contact_url)
        .ok()
        .filter(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
        .map(|url| url.to_string())
        .unwrap_or_default();
    let player = bundled_player_path(application_exe)?;
    if !player.is_file() {
        return Err(format!(
            "同梱プレイヤーが見つかりません: {}",
            player.display()
        ));
    }
    #[cfg(target_os = "linux")]
    crate::platform::validate_executable(&player)?;
    let mut command = Command::new(&player);
    #[cfg(not(target_os = "linux"))]
    command
        .arg(playlist_url)
        .arg(channel_name)
        .arg(contact)
        .current_dir(player.parent().expect("player has a parent"));
    #[cfg(target_os = "linux")]
    command.arg(playlist_url);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    Ok(command)
}

pub fn launch_player(
    playlist_url: String,
    channel_name: &str,
    contact_url: &str,
) -> Result<PlayerLaunch, String> {
    let application_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let player = bundled_player_path(&application_exe)?;
    let process_id = crate::platform::spawn_player_process(player_command(
        &application_exe,
        &playlist_url,
        channel_name,
        contact_url,
    )?)
    .map_err(|error| format!("PCRPlayerを起動できませんでした: {error}"))?;
    Ok(PlayerLaunch {
        player_path: player.display().to_string(),
        playlist_url,
        process_id,
    })
}

#[derive(Clone, Debug)]
pub struct PlayerContext {
    pub playlist_url: String,
    pub channel_name: String,
    pub contact_url: String,
    pub direct_stream_url: String,
    pub channel_id: String,
    pub description: String,
    pub comment: String,
    pub genre: String,
    pub content_type: String,
    pub bitrate: String,
    pub listeners: String,
    pub yellow_page: String,
}

fn local_playlist_url(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "再生URLが正しくありません")?;
    let channel = url
        .path()
        .strip_prefix("/pls/")
        .or_else(|| url.path().strip_prefix("/stream/"))
        .map(|value| value.strip_suffix(".m3u").unwrap_or(value));
    let valid_query = if url.path().starts_with("/stream/") {
        let pairs = url.query_pairs().collect::<Vec<_>>();
        pairs.is_empty()
            || (pairs.len() == 1
                && pairs[0].0 == "tip"
                && pairs[0]
                    .1
                    .parse::<std::net::SocketAddr>()
                    .is_ok_and(|address| {
                        address.port() > 0
                            && !address.ip().is_unspecified()
                            && !address.ip().is_multicast()
                    }))
    } else {
        url.query().is_none()
    };
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.port().is_some_and(|port| port > 0)
        || !url.username().is_empty()
        || url.password().is_some()
        || !valid_query
        || url.fragment().is_some()
        || !channel.is_some_and(|value| {
            value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err("このアプリのローカル再生URLだけを開けます".into());
    }
    Ok(url)
}

impl PlayerContext {
    pub fn from_yp(playlist_url: String, row: &serde_json::Value) -> Result<Self, String> {
        let mut direct = local_playlist_url(&playlist_url)?;
        let channel_id = direct
            .path()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .trim_end_matches(".m3u")
            .to_owned();
        direct.set_path(&format!("/stream/{channel_id}"));
        let string = |key: &str| match row.get(key) {
            Some(serde_json::Value::String(value)) => value.clone(),
            Some(serde_json::Value::Number(value)) => value.to_string(),
            _ => String::new(),
        };
        let contact_url = reqwest::Url::parse(&string("contactUrl"))
            .ok()
            .filter(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
            })
            .map(|url| url.to_string())
            .unwrap_or_default();
        Ok(Self {
            playlist_url,
            channel_name: string("name"),
            contact_url,
            direct_stream_url: direct.to_string(),
            channel_id,
            description: string("description"),
            comment: string("comment"),
            genre: string("genre"),
            content_type: string("contentType"),
            bitrate: string("bitrate"),
            listeners: string("listeners"),
            yellow_page: string("yellowPage"),
        })
    }

    fn value(&self, token: &str) -> Option<&str> {
        Some(match token {
            "<stream/>" => &self.playlist_url,
            "<directstream/>" => &self.direct_stream_url,
            "<channelname/>" => &self.channel_name,
            "<contact/>" => &self.contact_url,
            "<id/>" => &self.channel_id,
            "<description/>" => &self.description,
            "<comment/>" => &self.comment,
            "<genre/>" => &self.genre,
            "<type/>" => &self.content_type,
            "<bitrate/>" => &self.bitrate,
            "<listener/>" => &self.listeners,
            "<ypname/>" => &self.yellow_page,
            "<direct/>" => "0",
            _ => return None,
        })
    }

    fn expand(&self, argument: &str) -> String {
        let mut result = String::new();
        let mut remaining = argument;
        while let Some(start) = remaining.find('<') {
            result.push_str(&remaining[..start]);
            let candidate = &remaining[start..];
            if let Some(end) = candidate.find("/>") {
                let token = &candidate[..end + 2];
                result.push_str(self.value(token).unwrap_or(token));
                remaining = &candidate[end + 2..];
            } else {
                result.push_str(candidate);
                return result;
            }
        }
        result.push_str(remaining);
        result
    }
}

/// Windows-style double quotes and backslashes. Metadata is substituted only after splitting.
pub fn split_arguments(template: &str) -> Result<Vec<String>, String> {
    let chars: Vec<_> = template.chars().collect();
    let mut index = 0;
    let mut arguments = Vec::new();
    while index < chars.len() {
        while index < chars.len() && matches!(chars[index], ' ' | '\t' | '\r' | '\n') {
            index += 1;
        }
        if index == chars.len() {
            break;
        }
        let mut argument = String::new();
        let mut quoted = false;
        while index < chars.len() && (quoted || !matches!(chars[index], ' ' | '\t' | '\r' | '\n')) {
            if chars[index] == '\\' {
                let start = index;
                while index < chars.len() && chars[index] == '\\' {
                    index += 1;
                }
                let count = index - start;
                if index < chars.len() && chars[index] == '"' {
                    argument.extend(std::iter::repeat_n('\\', count / 2));
                    if count % 2 == 1 {
                        argument.push('"');
                        index += 1;
                        continue;
                    }
                } else {
                    argument.extend(std::iter::repeat_n('\\', count));
                    continue;
                }
            }
            if index < chars.len() && chars[index] == '"' {
                if quoted && chars.get(index + 1) == Some(&'"') {
                    argument.push('"');
                    index += 2;
                } else {
                    quoted = !quoted;
                    index += 1;
                }
            } else if index < chars.len() {
                argument.push(chars[index]);
                index += 1;
            }
        }
        if quoted {
            return Err("起動引数の二重引用符が閉じられていません".into());
        }
        arguments.push(argument);
        if arguments.len() > 128 {
            return Err("起動引数は128個まで指定できます".into());
        }
    }
    Ok(arguments)
}

pub fn resolved_player_path(
    application_exe: &Path,
    settings: &PlayerSettings,
) -> Result<PathBuf, String> {
    let settings = settings.clone().normalize()?;
    match settings.executable {
        None => bundled_player_path(application_exe),
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                Ok(path)
            } else {
                Ok(application_exe
                    .parent()
                    .ok_or("アプリの配置先を取得できません")?
                    .join(path))
            }
        }
    }
}

pub fn configured_player_command(
    application_exe: &Path,
    settings: &PlayerSettings,
    context: &PlayerContext,
) -> Result<Command, String> {
    local_playlist_url(&context.playlist_url)?;
    let settings = settings.clone().normalize()?;
    let player = resolved_player_path(application_exe, &settings)?;
    if !player.is_file() {
        return Err(format!("プレイヤーが見つかりません: {}", player.display()));
    }
    #[cfg(target_os = "linux")]
    crate::platform::validate_executable(&player)?;
    let arguments: Vec<_> = split_arguments(&settings.arguments)?
        .into_iter()
        .map(|argument| context.expand(&argument))
        .collect();
    let length = player.to_string_lossy().encode_utf16().count()
        + arguments
            .iter()
            .map(|argument| argument.encode_utf16().count().saturating_mul(2) + 3)
            .sum::<usize>();
    if length > 32760 || arguments.iter().any(|argument| argument.contains('\0')) {
        return Err("展開後の起動引数が長すぎるか、使用できない文字を含みます".into());
    }
    let mut command = Command::new(&player);
    command.args(arguments).current_dir(
        player
            .parent()
            .ok_or("プレイヤーのフォルダーを取得できません")?,
    );
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    Ok(command)
}

pub fn launch_configured_player(
    settings: &PlayerSettings,
    context: PlayerContext,
) -> Result<PlayerLaunch, String> {
    let application_exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let player = resolved_player_path(&application_exe, settings)?;
    let process_id = crate::platform::spawn_player_process(configured_player_command(
        &application_exe,
        settings,
        &context,
    )?)
    .map_err(|error| format!("プレイヤーを起動できませんでした: {error}"))?;
    Ok(PlayerLaunch {
        player_path: player.display().to_string(),
        playlist_url: context.playlist_url,
        process_id,
    })
}
