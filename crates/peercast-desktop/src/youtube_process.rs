use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

pub const YT_DLP_VERSION: &str = "2026.08.19";
pub const YT_DLP_SHA256: &str = "66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a";
const OUTPUT_LIMIT: u64 = 1024 * 1024;

/// Only channel pages are accepted; arbitrary URLs and yt-dlp options never cross IPC.
pub fn channel_url(input: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(input.trim())
        .map_err(|_| "YouTubeチャンネルのURLを入力してください")?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("www.youtube.com" | "youtube.com" | "m.youtube.com")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("https://www.youtube.com/@名前 または /channel/UC… を指定してください".into());
    }
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    let base = match parts.as_slice() {
        [handle] | [handle, "streams"]
            if handle.starts_with('@')
                && handle.len() > 1
                && handle.len() <= 300
                && handle
                    .chars()
                    .skip(1)
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '%')) =>
        {
            format!("/{handle}")
        }
        ["channel", id] | ["channel", id, "streams"] if valid_channel_id(id) => {
            format!("/channel/{id}")
        }
        _ => return Err("動画URLではなく、YouTubeチャンネルのURLを指定してください".into()),
    };
    Ok(format!("https://www.youtube.com{base}/streams"))
}

pub fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub fn valid_channel_id(id: &str) -> bool {
    id.starts_with("UC")
        && id.len() == 24
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub fn video_url(id: &str) -> Result<String, String> {
    if !valid_video_id(id) {
        return Err("YouTube動画IDが正しくありません".into());
    }
    Ok(format!("https://www.youtube.com/watch?v={id}"))
}

pub fn bundled_executable(application_exe: &Path) -> Result<PathBuf, String> {
    let parent = application_exe
        .parent()
        .ok_or("アプリの配置先を取得できません")?;
    for relative in ["YouTube/yt-dlp.exe", "resources/YouTube/yt-dlp.exe"] {
        let candidate = parent.join(relative);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("同梱のYouTube監視ツールが見つかりません。アプリを再インストールしてください".into())
}

pub async fn probe(
    executable: &Path,
    input_url: &str,
) -> Result<crate::youtube::ProbeData, String> {
    let url = channel_url(input_url)?;
    let executable = executable.to_owned();
    // Verify the exact bundled release, never a PATH executable or user-selected command.
    let verified = tokio::task::spawn_blocking(move || {
        let bytes = std::fs::read(&executable).map_err(|_| "YouTube監視ツールを読めません")?;
        if bytes.len() > 30 * 1024 * 1024
            || format!("{:x}", Sha256::digest(&bytes)) != YT_DLP_SHA256
        {
            return Err(
                "YouTube監視ツールの整合性を確認できません。再インストールしてください".to_owned(),
            );
        }
        Ok(executable)
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
            "--flat-playlist",
            "--playlist-end",
            "30",
            "--dump-single-json",
            "--simulate",
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
        .map_err(|_| "YouTube監視ツールを起動できません")?;
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
                return Err("YouTubeの応答が大きすぎます".to_owned());
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
                return Err("YouTube監視ツールのエラー応答が大きすぎます".to_owned());
            }
            Ok::<_, String>(data)
        };
        let (data, _errors) = tokio::try_join!(output, errors)?;
        let status = child.wait().await.map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("YouTubeの状態を確認できませんでした。アクセス制限・通信状態・チャンネルURLを確認してください".into());
        }
        crate::youtube::parse_probe_json(&data)
    };
    let result = tokio::time::timeout(Duration::from_secs(45), work).await;
    match result {
        Ok(Ok(data)) => Ok(data),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            Err("YouTubeの確認がタイムアウトしました。時間を空けて再試行します".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permits_only_public_channel_urls() {
        assert_eq!(
            channel_url("https://youtube.com/@weathernews").unwrap(),
            "https://www.youtube.com/@weathernews/streams"
        );
        assert_eq!(
            channel_url("https://www.youtube.com/channel/UCNsidkYpIAQ4QaufptQBPHQ/streams")
                .unwrap(),
            "https://www.youtube.com/channel/UCNsidkYpIAQ4QaufptQBPHQ/streams"
        );
        for url in [
            "http://youtube.com/@a",
            "https://youtube.com.evil.test/@a",
            "https://evil.test/@a",
            "https://youtube.com/watch?v=NyADUtuAye4",
            "https://youtube.com/@a?x=1",
            "https://u@youtube.com/@a",
            "file:///C:/a",
            "--exec=bad",
            "https://youtube.com/channel/invalid",
            "https://youtube.com/@a/../../watch",
        ] {
            assert!(channel_url(url).is_err(), "{url}");
        }
    }
    #[test]
    fn video_links_cannot_escape_youtube() {
        assert_eq!(
            video_url("NyADUtuAye4").unwrap(),
            "https://www.youtube.com/watch?v=NyADUtuAye4"
        );
        for id in ["", "../bad", "x&list=evil", "https://evil", "a b12345678"] {
            assert!(video_url(id).is_err());
        }
    }
    #[tokio::test]
    async fn refuses_a_substituted_helper_before_execution() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("yt-dlp.exe");
        std::fs::write(&exe, b"not the verified release").unwrap();
        let error = probe(&exe, "https://youtube.com/@weathernews")
            .await
            .err()
            .unwrap();
        assert!(error.contains("整合性"));
    }
}
