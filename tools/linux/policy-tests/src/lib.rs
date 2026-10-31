#[path = "../../../../crates/peercast-desktop/src/platform.rs"]
pub mod platform;
#[path = "../../../../crates/peercast-desktop/src/player.rs"]
pub mod player;
#[cfg(test)]
mod tests {
    use super::player::*;
    use std::path::Path;
    #[test]
    fn linux_default_is_url_only() {
        assert_eq!(
            split_arguments(&PlayerSettings::default().arguments).unwrap(),
            ["<stream/>"]
        );
        assert_eq!(default_player_path().unwrap(), Path::new("/usr/bin/mpv"));
    }
    #[test]
    fn shell_metacharacters_stay_inside_one_argument() {
        let url = "http://127.0.0.1:7144/pls/0123456789ABCDEF0123456789ABCDEF.m3u";
        let context = PlayerContext::from_yp(
            url.into(),
            &serde_json::json!({"name":"a\"; touch /tmp/never; $(whoami)"}),
        )
        .unwrap();
        let settings = PlayerSettings {
            executable: Some("/usr/bin/true".into()),
            arguments: "\"<stream/>\" \"<channelname/>\"".into(),
        };
        let command =
            configured_player_command(Path::new("/opt/peercast"), &settings, &context).unwrap();
        let args = command.get_args().collect::<Vec<_>>();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], url);
        assert_eq!(args[1], context.channel_name.as_str());
        // Inspect only: never spawn the fixture executable.
    }
    #[test]
    fn explicit_executable_and_local_urls_are_required() {
        assert!(PlayerSettings {
            executable: Some("mpv".into()),
            ..Default::default()
        }
        .normalize()
        .is_err());
        assert!(PlayerSettings {
            executable: Some("/tmp/nonexistent-player-unique".into()),
            ..Default::default()
        }
        .normalize()
        .is_ok());
        assert!(PlayerContext::from_yp(
            "https://example.org/pls/0123456789ABCDEF0123456789ABCDEF.m3u".into(),
            &serde_json::json!({})
        )
        .is_err());
    }
    #[test]
    fn direct_stream_default_can_be_configured_for_ffplay() {
        let context = PlayerContext::from_yp(
            "http://127.0.0.1:7144/pls/0123456789ABCDEF0123456789ABCDEF.m3u".into(),
            &serde_json::json!({}),
        )
        .unwrap();
        let settings = PlayerSettings {
            executable: Some("/usr/bin/true".into()),
            arguments: "\"<directstream/>\"".into(),
        };
        let cmd =
            configured_player_command(Path::new("/opt/peercast"), &settings, &context).unwrap();
        assert_eq!(
            cmd.get_args().next().unwrap(),
            "http://127.0.0.1:7144/stream/0123456789ABCDEF0123456789ABCDEF"
        );
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn stale_saved_player_loads_but_launch_rejects_it() {
        let saved = r#"{"executable":"/tmp/nonexistent-player-recovery-fixture","arguments":"\"<stream/>\""}"#;
        let settings: player::PlayerSettings = serde_json::from_str(saved).unwrap();
        let settings = settings
            .normalize()
            .expect("missing player must not block settings repair");
        let context = player::PlayerContext::from_yp(
            "http://127.0.0.1:7144/pls/0123456789ABCDEF0123456789ABCDEF.m3u".into(),
            &serde_json::json!({}),
        )
        .unwrap();
        assert!(player::configured_player_command(
            std::path::Path::new("/opt/app"),
            &settings,
            &context
        )
        .is_err());
    }
}

#[path = "../../../../crates/peercast-desktop/src/backend.rs"]
pub mod backend;
#[path = "../../../../crates/peercast-desktop/src/broadcast.rs"]
pub mod broadcast;
#[path = "../../../../crates/peercast-desktop/src/broadcast_runtime.rs"]
pub mod broadcast_runtime;
#[path = "../../../../crates/peercast-desktop/src/csharp_backend.rs"]
pub mod csharp_backend;
#[path = "../../../../crates/peercast-desktop/src/preferences.rs"]
pub mod preferences;
#[path = "../../../../crates/peercast-desktop/src/recording.rs"]
pub mod recording;
#[path = "../../../../crates/peercast-desktop/src/recording_config.rs"]
pub mod recording_config;
#[path = "../../../../crates/peercast-desktop/src/recording_flv.rs"]
pub mod recording_flv;
#[path = "../../../../crates/peercast-desktop/src/recording_path.rs"]
pub mod recording_path;
#[path = "../../../../crates/peercast-desktop/src/rules.rs"]
pub mod rules;

#[cfg(test)]
mod backend_recovery_tests {
    use super::*;
    #[tokio::test]
    async fn fresh_linux_backend_is_offline_and_stale_player_is_repairable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("linux-desktop.json");
        // All new defaults must be offline before any backend is started.
        assert!(
            !preferences::DesktopPreferences::default()
                .directory
                .auto_refresh
        );
        let backend = backend::DesktopBackend::open_detecting(path.clone())
            .await
            .unwrap();
        let first = backend.snapshot().await.unwrap();
        assert!(first.yellow_pages.is_empty());
        assert!(!first.desktop.directory.auto_refresh);
        backend.close().await.unwrap();
        let mut config = peercast_app::ConfigManager::new(&path).load().unwrap();
        config.desktop["player"]["executable"] =
            serde_json::json!("/tmp/nonexistent-player-backend-recovery-fixture");
        peercast_app::ConfigManager::new(&path)
            .save(&config)
            .unwrap();
        let reopened = backend::DesktopBackend::open(path.clone()).await.unwrap();
        let snapshot = reopened.snapshot().await.unwrap();
        assert!(!snapshot.player_available);
        assert!(snapshot.yellow_pages.is_empty());
        assert!(!snapshot.desktop.directory.auto_refresh);
        reopened.close().await.unwrap();
    }
}

#[cfg(test)]
mod permission_recovery_tests {
    use super::*;
    #[tokio::test]
    async fn non_executable_saved_player_does_not_prevent_startup() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("unmounted-or-noexec-player");
        std::fs::write(&executable, b"not executable; never spawned").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o600)).unwrap();
        let settings = player::PlayerSettings {
            executable: Some(executable.to_string_lossy().into_owned()),
            ..Default::default()
        }
        .normalize()
        .unwrap();
        let path = dir.path().join("linux.json");
        let mut config = peercast_app::AppConfig::default();
        let mut desktop = preferences::DesktopPreferences::default();
        desktop.player = settings.clone();
        config.desktop = serde_json::to_value(desktop).unwrap();
        peercast_app::ConfigManager::new(&path)
            .save(&config)
            .unwrap();
        let backend = backend::DesktopBackend::open_detecting(path).await.unwrap();
        let snapshot = backend.snapshot().await.unwrap();
        assert!(!snapshot.player_available);
        assert!(snapshot.yellow_pages.is_empty());
        let context = player::PlayerContext::from_yp(
            "http://127.0.0.1:7144/pls/0123456789ABCDEF0123456789ABCDEF.m3u".into(),
            &serde_json::json!({}),
        )
        .unwrap();
        assert!(player::configured_player_command(
            std::path::Path::new("/opt/app"),
            &settings,
            &context
        )
        .is_err());
        backend.close().await.unwrap();
    }
}
