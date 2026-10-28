use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::youtube::{Provider, YouTubeSnapshot};

pub fn bundled_player(executable: &Path) -> Result<PathBuf, String> {
    let root = executable
        .parent()
        .ok_or("アプリの保存先を確認できませんでした")?;
    let player = root.join("PCRPlayer").join("PCRPlayer64.exe");
    if !player.is_file() {
        return Err("同梱PCRPlayerが見つかりません。インストールし直してください".into());
    }
    Ok(player)
}

pub fn playback_target(
    snapshot: &YouTubeSnapshot,
    provider: Provider,
    selected_id: &str,
) -> Result<(String, String), String> {
    let channel = snapshot
        .channels
        .iter()
        .find(|channel| {
            channel.enabled
                && channel.error.is_none()
                && channel
                    .streams
                    .iter()
                    .any(|stream| stream.id == selected_id && stream.status == "live")
        })
        .ok_or_else(|| "登録したチャンネルの現在の配信を選択してください".to_owned())?;
    let stream = channel
        .streams
        .iter()
        .find(|stream| stream.id == selected_id && stream.status == "live")
        .expect("matching live stream was checked above");
    let page = match provider {
        Provider::YouTube => crate::youtube_process::video_url(selected_id)?,
        Provider::Twitch => crate::twitch_process::channel_url(&channel.url)?,
    };
    let title = match (clean_title(&stream.title), clean_title(&channel.name)) {
        (title, _) if !title.is_empty() => title,
        (_, name) if !name.is_empty() => name,
        _ => match provider {
            Provider::YouTube => "YouTube ライブ".to_owned(),
            Provider::Twitch => "Twitch ライブ".to_owned(),
        },
    };
    Ok((page, title))
}

fn clean_title(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

pub fn live_command(application_exe: &Path, page: &str, title: &str) -> Result<Command, String> {
    let url = reqwest::Url::parse(page).map_err(|_| "ライブURLが正しくありません")?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("www.youtube.com" | "www.twitch.tv"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return Err("ライブ配信のURLを確認できませんでした".into());
    }
    match url.host_str() {
        Some("www.youtube.com")
            if url.path() == "/watch"
                && url.query_pairs().count() == 1
                && url.query_pairs().any(|(key, value)| {
                    key == "v" && crate::youtube_process::valid_video_id(&value)
                }) => {}
        Some("www.twitch.tv")
            if url.query().is_none()
                && !url.path().trim_matches('/').is_empty()
                && url.path().trim_matches('/').len() <= 25
                && url
                    .path()
                    .trim_matches('/')
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') => {}
        _ => return Err("現在のライブ配信を選択してください".into()),
    }
    let player = bundled_player(application_exe)?;
    let mut command = Command::new(&player);
    command
        .arg(url.as_str())
        .arg(title)
        .current_dir(player.parent().unwrap());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    Ok(command)
}

pub async fn open_live(page: &str, title: &str) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    live_command(&executable, page, title)?
        .spawn()
        .map_err(|error| format!("PCRPlayerを起動できませんでした: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::youtube::{YouTubeChannel, YouTubeStream};

    fn channel(
        provider: Provider,
        id: &str,
        title: &str,
        name: &str,
        status: &str,
    ) -> YouTubeChannel {
        YouTubeChannel {
            id: format!("channel-{id}"),
            url: match provider {
                Provider::YouTube => {
                    "https://www.youtube.com/channel/UCabcdefghijklmnopqrstuv".into()
                }
                Provider::Twitch => "https://www.twitch.tv/example".into(),
            },
            name: name.into(),
            channel_id: None,
            enabled: true,
            last_checked_ms: None,
            error: None,
            streams: vec![YouTubeStream {
                id: id.into(),
                title: title.into(),
                url: String::new(),
                status: status.into(),
                scheduled_at_ms: None,
            }],
        }
    }

    fn snapshot(channels: Vec<YouTubeChannel>) -> YouTubeSnapshot {
        YouTubeSnapshot {
            channels,
            interval_minutes: 5,
            refreshing: false,
            last_refresh_ms: None,
            next_refresh_ms: 0,
        }
    }

    #[test]
    fn bundled_player_requires_existing_binary() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("Peercast-RT.exe");
        assert!(bundled_player(&app).is_err());
        std::fs::create_dir(root.path().join("PCRPlayer")).unwrap();
        let player = root.path().join("PCRPlayer/PCRPlayer64.exe");
        std::fs::write(&player, b"fixture").unwrap();
        assert_eq!(bundled_player(&app).unwrap(), player);
    }

    #[test]
    fn playback_target_uses_selected_youtube_stream_title() {
        let mut channel = channel(
            Provider::YouTube,
            "abcdefghijk",
            " 配信\nタイトル 🎮 ",
            "配信者",
            "live",
        );
        channel.streams.push(YouTubeStream {
            id: "zyxwvutsrqp".into(),
            title: "別の配信".into(),
            url: String::new(),
            status: "live".into(),
            scheduled_at_ms: None,
        });
        let snapshot = snapshot(vec![channel]);
        assert_eq!(
            playback_target(&snapshot, Provider::YouTube, "abcdefghijk").unwrap(),
            (
                "https://www.youtube.com/watch?v=abcdefghijk".into(),
                "配信タイトル 🎮".into()
            )
        );
        assert_eq!(
            playback_target(&snapshot, Provider::YouTube, "zyxwvutsrqp")
                .unwrap()
                .1,
            "別の配信"
        );
    }

    #[test]
    fn playback_target_uses_twitch_title_and_channel_url() {
        assert_eq!(
            playback_target(
                &snapshot(vec![channel(
                    Provider::Twitch,
                    "12345",
                    "Twitch配信",
                    "配信者",
                    "live"
                )]),
                Provider::Twitch,
                "12345"
            )
            .unwrap(),
            ("https://www.twitch.tv/example".into(), "Twitch配信".into())
        );
    }

    #[test]
    fn playback_target_falls_back_to_channel_then_provider_name() {
        let mut live_channel = channel(
            Provider::YouTube,
            "abcdefghijk",
            "\n\0",
            " 配信者\t",
            "live",
        );
        assert_eq!(
            playback_target(
                &snapshot(vec![live_channel.clone()]),
                Provider::YouTube,
                "abcdefghijk"
            )
            .unwrap()
            .1,
            "配信者"
        );
        live_channel.name = "\n\0".into();
        assert_eq!(
            playback_target(
                &snapshot(vec![live_channel]),
                Provider::YouTube,
                "abcdefghijk"
            )
            .unwrap()
            .1,
            "YouTube ライブ"
        );
        assert_eq!(
            playback_target(
                &snapshot(vec![channel(Provider::Twitch, "12345", "", "", "live")]),
                Provider::Twitch,
                "12345"
            )
            .unwrap()
            .1,
            "Twitch ライブ"
        );
    }

    #[test]
    fn playback_target_rejects_non_live_or_unavailable_channels() {
        let mut disabled = channel(Provider::YouTube, "abcdefghijk", "Live", "", "live");
        disabled.enabled = false;
        let mut errored = channel(Provider::YouTube, "abcdefghijk", "Live", "", "live");
        errored.error = Some("error".into());
        let channels = vec![
            channel(Provider::YouTube, "abcdefghijk", "Offline", "", "offline"),
            channel(Provider::YouTube, "abcdefghijk", "Upcoming", "", "upcoming"),
            disabled,
            errored,
            channel(Provider::YouTube, "differentid", "Live", "", "live"),
        ];
        for channel in channels {
            assert!(
                playback_target(&snapshot(vec![channel]), Provider::YouTube, "abcdefghijk")
                    .is_err()
            );
        }
    }

    #[test]
    fn live_command_keeps_title_as_one_literal_argument() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("Peercast-RT.exe");
        std::fs::create_dir(root.path().join("PCRPlayer")).unwrap();
        std::fs::write(root.path().join("PCRPlayer/PCRPlayer64.exe"), b"fixture").unwrap();
        let title = "日本語 \"引用\" & C:\\配信";
        let command =
            live_command(&app, "https://www.youtube.com/watch?v=abcdefghijk", title).unwrap();
        let args: Vec<_> = command.as_std().get_args().collect();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "https://www.youtube.com/watch?v=abcdefghijk");
        assert_eq!(args[1], title);
    }

    #[test]
    fn live_command_rejects_invalid_urls() {
        assert!(
            live_command(Path::new("unused.exe"), "https://example.com/live", "title").is_err()
        );
    }
}
