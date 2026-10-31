use peercast_desktop::player::{bundled_player_path, player_command};
#[cfg(target_os = "linux")]
use peercast_desktop::player::{configured_player_command, PlayerContext, PlayerSettings};
use std::{ffi::OsStr, fs, path::Path, process::Command};

fn write_player_fixture(path: &Path) {
    fs::write(path, b"fixture, never executed").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn fixture_player_command(
    app: &Path,
    stream: &str,
    name: &str,
    contact: &str,
) -> Result<Command, String> {
    #[cfg(not(target_os = "linux"))]
    return player_command(app, stream, name, contact);
    #[cfg(target_os = "linux")]
    {
        // Linux uses an explicitly configured external player. Do not depend on
        // a host mpv installation or execute the command-building fixture.
        let settings = PlayerSettings {
            executable: Some(
                app.parent()
                    .unwrap()
                    .join("PCRPlayer/PCRPlayer64.exe")
                    .display()
                    .to_string(),
            ),
            ..PlayerSettings::default()
        };
        let context = PlayerContext::from_yp(
            stream.into(),
            &serde_json::json!({"name": name, "contactUrl": contact}),
        )?;
        configured_player_command(app, &settings, &context)
    }
}

#[test]
fn portable_player_command_keeps_platform_arguments_after_move() {
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("original app folder");
    fs::create_dir_all(original.join("PCRPlayer")).unwrap();
    write_player_fixture(&original.join("PCRPlayer/PCRPlayer64.exe"));
    let moved = temp.path().join("moved app folder");
    fs::rename(original, &moved).unwrap();
    let app = moved.join("PeerCastStation.exe");
    let url = "http://127.0.0.1:12345/pls/11111111111111111111111111111111";
    let name = "配信名 \"quoted\" & example";
    let contact = "https://jbbs.shitaraba.net/bbs/read.cgi/game/12345/1234567890/";
    let command = fixture_player_command(&app, url, name, contact).unwrap();
    #[cfg(not(target_os = "linux"))]
    {
        assert_eq!(command.get_program(), bundled_player_path(&app).unwrap());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new(url), OsStr::new(name), OsStr::new(contact)]
        );
    }
    #[cfg(target_os = "linux")]
    {
        assert_eq!(
            bundled_player_path(&app).unwrap(),
            Path::new("/usr/bin/mpv")
        );
        assert_eq!(
            command.get_program(),
            moved.join("PCRPlayer/PCRPlayer64.exe")
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new(url)]
        );
    }
    assert_eq!(command.get_current_dir().unwrap(), moved.join("PCRPlayer"));
}

#[test]
fn player_command_rejects_external_or_malformed_targets_and_missing_binary() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app.exe");
    fs::create_dir(temp.path().join("PCRPlayer")).unwrap();
    write_player_fixture(&temp.path().join("PCRPlayer/PCRPlayer64.exe"));
    for url in [
        "https://example.com/pls/11111111111111111111111111111111",
        "http://127.0.0.1:1234/pls/not-an-id",
        "http://127.0.0.1:0/pls/11111111111111111111111111111111",
        "http://127.0.0.1:1234/pls/11111111111111111111111111111111?command=anything",
        "http://user:password@127.0.0.1:1234/pls/11111111111111111111111111111111",
        "file:///C:/Windows/system32/cmd.exe",
        "javascript:alert(1)",
    ] {
        assert!(
            player_command(&app, url, "test", "").is_err(),
            "accepted {url}"
        );
        assert!(
            fixture_player_command(&app, url, "test", "").is_err(),
            "accepted {url}"
        );
    }
    let other = temp.path().join("missing/app.exe");
    assert!(fixture_player_command(
        &other,
        "http://127.0.0.1:1234/pls/11111111111111111111111111111111",
        "test",
        ""
    )
    .is_err());
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        let player = temp.path().join("PCRPlayer/PCRPlayer64.exe");
        let stream = "http://127.0.0.1:1234/pls/11111111111111111111111111111111";
        fs::set_permissions(&player, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(fixture_player_command(&app, stream, "test", "").is_err());
        fs::set_permissions(&player, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(fixture_player_command(&app, stream, "test", "").is_ok());
    }
}

#[test]
fn invalid_contacts_are_omitted_without_changing_the_stream_target() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app.exe");
    fs::create_dir(temp.path().join("PCRPlayer")).unwrap();
    write_player_fixture(&temp.path().join("PCRPlayer/PCRPlayer64.exe"));
    let stream = "http://127.0.0.1:1234/pls/11111111111111111111111111111111";
    for contact in [
        "",
        "0",
        "file:///C:/private.txt",
        "javascript:alert(1)",
        "https://user:password@example.com/thread",
    ] {
        let command = fixture_player_command(&app, stream, "配信名", contact).unwrap();
        #[cfg(not(target_os = "linux"))]
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new(stream), OsStr::new("配信名"), OsStr::new("")]
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new(stream)]
        );
    }
}
