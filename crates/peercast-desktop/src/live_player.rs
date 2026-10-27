use std::path::{Path, PathBuf};
use tokio::process::Command;

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

pub async fn open_live(page: &str) -> Result<(), String> {
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
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let player = bundled_player(&executable)?;
    let mut command = Command::new(&player);
    command
        .arg(url.as_str())
        .current_dir(player.parent().unwrap());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command
        .spawn()
        .map_err(|error| format!("PCRPlayerを起動できませんでした: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
