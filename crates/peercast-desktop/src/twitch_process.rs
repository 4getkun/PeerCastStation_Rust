#[cfg(not(target_os = "linux"))]
use sha2::{Digest, Sha256};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

const OUTPUT_LIMIT: u64 = 1024 * 1024;
const OFFLINE_MESSAGES: &[&str] = &[
    "this channel is not currently live",
    "channel is currently offline",
    "not currently live",
];

fn is_offline_message(message: &[u8]) -> bool {
    let message = String::from_utf8_lossy(message).to_ascii_lowercase();
    OFFLINE_MESSAGES
        .iter()
        .any(|needle| message.contains(needle))
}

pub fn channel_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    let url = reqwest::Url::parse(input).map_err(|_| "TwitchチャンネルURLを入力してください")?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("twitch.tv" | "www.twitch.tv"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || has_explicit_port(input)
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("https://www.twitch.tv/<チャンネル名> を指定してください".into());
    }
    let path = url
        .path()
        .strip_prefix('/')
        .and_then(|path| path.strip_suffix('/').or(Some(path)))
        .ok_or("TwitchのチャンネルURLを指定してください")?;
    if path.contains('/') {
        return Err("TwitchのチャンネルURLを指定してください".into());
    }
    let login = path;
    if !valid_login(login) || is_reserved_login(login) {
        return Err("Twitchのチャンネル名として使えないURLです".into());
    }
    Ok(format!(
        "https://www.twitch.tv/{}",
        login.to_ascii_lowercase()
    ))
}

fn has_explicit_port(input: &str) -> bool {
    let Some((scheme, rest)) = input.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    authority
        .rsplit('@')
        .next()
        .is_some_and(|host| host.contains(':'))
}

pub fn valid_login(login: &str) -> bool {
    (1..=25).contains(&login.len())
        && login
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn is_reserved_login(login: &str) -> bool {
    matches!(
        login.to_ascii_lowercase().as_str(),
        "directory"
            | "downloads"
            | "jobs"
            | "p"
            | "search"
            | "settings"
            | "subscriptions"
            | "turbo"
            | "videos"
            | "wallet"
            | "creator-dashboard"
    )
}

pub async fn probe(
    executable: &Path,
    input_url: &str,
) -> Result<crate::youtube::ProbeData, String> {
    let url = channel_url(input_url)?;
    let login = reqwest::Url::parse(&url)
        .ok()
        .and_then(|parsed| parsed.path_segments()?.next().map(str::to_owned))
        .ok_or("Twitchチャンネル名を確認できません")?;
    let executable = executable.to_owned();
    let verified = tokio::task::spawn_blocking(move || {
        #[cfg(target_os = "linux")]
        {
            crate::live_runtime::verify_probe_path(&executable)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let bytes = std::fs::read(&executable).map_err(|_| "YouTube監視ツールを読めません")?;
            if bytes.len() > 30 * 1024 * 1024
                || format!("{:x}", Sha256::digest(&bytes)) != crate::youtube_process::YT_DLP_SHA256
            {
                return Err(
                    "YouTube監視ツールの整合性を確認できません。再インストールしてください"
                        .to_owned(),
                );
            }
            Ok(executable)
        }
    })
    .await
    .map_err(|error| error.to_string())??;

    let mut command = Command::new(verified);
    command
        .args([
            "--ignore-config",
            "--no-plugin-dirs",
            "--no-remote-components",
            "--no-js-runtimes",
            "--no-cache-dir",
            "--dump-single-json",
            "--skip-download",
            "--no-playlist",
            "--socket-timeout",
            "12",
            "--retries",
            "0",
            "--no-colors",
            "--",
            &url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .map_err(|_| "Twitch監視ツールを起動できません")?;
    let stdout = child.stdout.take().ok_or("監視結果を取得できません")?;
    let stderr = child.stderr.take().ok_or("監視結果を取得できません")?;
    let work = async {
        let output = async {
            let mut data = Vec::new();
            stdout
                .take(OUTPUT_LIMIT + 1)
                .read_to_end(&mut data)
                .await
                .map_err(|e| e.to_string())?;
            if data.len() as u64 > OUTPUT_LIMIT {
                return Err("Twitchの応答が大きすぎます".to_owned());
            }
            Ok::<_, String>(data)
        };
        let errors = async {
            let mut data = Vec::new();
            stderr
                .take(65537)
                .read_to_end(&mut data)
                .await
                .map_err(|e| e.to_string())?;
            if data.len() > 65536 {
                return Err("監視ツールのエラー応答が大きすぎます".to_owned());
            }
            Ok::<_, String>(data)
        };
        let (data, errors) = tokio::try_join!(output, errors)?;
        let status = child.wait().await.map_err(|e| e.to_string())?;
        if !status.success() {
            if is_offline_message(&errors) {
                return Ok(crate::youtube::ProbeData {
                    name: None,
                    channel_id: Some(login.clone()),
                    streams: Vec::new(),
                });
            }
            return Err(
                "Twitchの状態を確認できませんでした。通信状態またはチャンネルURLを確認してください"
                    .into(),
            );
        }
        parse_probe_json(&data, &login)
    };
    match tokio::time::timeout(Duration::from_secs(45), work).await {
        Ok(Ok(data)) => Ok(data),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            Err("Twitchの確認がタイムアウトしました。時間を空けて再試行します".into())
        }
    }
}

fn parse_probe_json(bytes: &[u8], login: &str) -> Result<crate::youtube::ProbeData, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("yt-dlp Twitch JSONを解析できませんでした: {error}"))?;
    let root = value
        .as_object()
        .ok_or("yt-dlp Twitch JSONのルートがオブジェクトではありません")?;
    let live = root
        .get("is_live")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        && root.get("live_status").and_then(serde_json::Value::as_str) == Some("is_live");
    if !live {
        return Ok(crate::youtube::ProbeData {
            name: None,
            channel_id: Some(login.to_owned()),
            streams: Vec::new(),
        });
    }
    let id = root
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or("yt-dlp Twitch配信IDがありません")?;
    if id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("yt-dlp Twitch配信IDの形式が正しくありません".into());
    }
    let name = root
        .get("uploader")
        .or_else(|| root.get("channel"))
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned);
    let title = root
        .get("title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(id)
        .to_owned();
    Ok(crate::youtube::ProbeData {
        name,
        channel_id: Some(login.to_owned()),
        streams: vec![crate::youtube::YouTubeStream {
            id: id.to_owned(),
            title,
            url: format!("https://www.twitch.tv/{login}"),
            status: "live".into(),
            scheduled_at_ms: None,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_canonicalizable_twitch_channel_urls() {
        assert_eq!(
            channel_url("https://www.twitch.tv/Example_1/").unwrap(),
            "https://www.twitch.tv/example_1"
        );
        for bad in [
            "http://twitch.tv/name",
            "https://twitch.tv.evil.test/name",
            "https://user@twitch.tv/name",
            "https://twitch.tv/name:443",
            "https://twitch.tv:443/name",
            "https://twitch.tv/name?x=1",
            "https://twitch.tv/name#x",
            "https://twitch.tv/name/videos",
            "https://twitch.tv/directory",
            "https://twitch.tv/a.b",
            "https://twitch.tv/a%2fb",
            "https://twitch.tv/abcdefghijklmnopqrstuvwxyzz",
        ] {
            assert!(channel_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parser_requires_live_flags_and_uses_registered_login() {
        let data = parse_probe_json(br#"{"id":"123456","title":"Live","uploader":"Streamer","is_live":true,"live_status":"is_live"}"#, "streamer").unwrap();
        assert_eq!(data.channel_id.as_deref(), Some("streamer"));
        assert_eq!(data.streams.len(), 1);
        for json in [
            br#"{"id":"123456","is_live":false,"live_status":"not_live"}"#.as_slice(),
            br#"{"id":"123456","is_live":true,"live_status":"not_live"}"#,
        ] {
            assert!(parse_probe_json(json, "streamer")
                .unwrap()
                .streams
                .is_empty());
        }
        assert!(parse_probe_json(br#"[]"#, "streamer").is_err());
        assert!(parse_probe_json(
            br#"{"id":"bad/id","is_live":true,"live_status":"is_live"}"#,
            "streamer"
        )
        .is_err());
    }

    #[test]
    fn only_explicit_offline_errors_become_empty_results() {
        assert!(is_offline_message(
            b"ERROR: [twitch:stream] This channel is not currently live"
        ));
        assert!(is_offline_message(b"The channel is currently offline"));
        assert!(!is_offline_message(b"HTTP Error 503: Service Unavailable"));
        assert!(!is_offline_message(
            b"Login required to access this channel"
        ));
    }
}
