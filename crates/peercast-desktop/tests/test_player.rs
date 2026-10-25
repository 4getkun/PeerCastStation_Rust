use peercast_desktop::player::{bundled_player_path, player_command};
use std::{ffi::OsStr, fs};

#[test]
fn portable_player_command_keeps_three_literal_arguments_after_move() {
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("original app folder");
    fs::create_dir_all(original.join("PCRPlayer")).unwrap();
    fs::write(original.join("PCRPlayer/PCRPlayer64.exe"), b"fixture").unwrap();
    let moved = temp.path().join("moved app folder");
    fs::rename(original, &moved).unwrap();
    let app = moved.join("PeerCastStation.exe");
    let url = "http://127.0.0.1:12345/pls/11111111111111111111111111111111";
    let name = "配信名 \"quoted\" & example";
    let contact = "https://jbbs.shitaraba.net/bbs/read.cgi/game/12345/1234567890/";
    let command = player_command(&app, url, name, contact).unwrap();
    assert_eq!(command.get_program(), bundled_player_path(&app).unwrap());
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![OsStr::new(url), OsStr::new(name), OsStr::new(contact)]
    );
    assert_eq!(command.get_current_dir().unwrap(), moved.join("PCRPlayer"));
}

#[test]
fn player_command_rejects_external_or_malformed_targets_and_missing_binary() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app.exe");
    fs::create_dir(temp.path().join("PCRPlayer")).unwrap();
    fs::write(temp.path().join("PCRPlayer/PCRPlayer64.exe"), b"fixture").unwrap();
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
    }
    let other = temp.path().join("missing/app.exe");
    assert!(player_command(
        &other,
        "http://127.0.0.1:1234/pls/11111111111111111111111111111111",
        "test",
        ""
    )
    .is_err());
}

#[test]
fn invalid_contacts_are_omitted_without_changing_the_stream_target() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app.exe");
    fs::create_dir(temp.path().join("PCRPlayer")).unwrap();
    fs::write(temp.path().join("PCRPlayer/PCRPlayer64.exe"), b"fixture").unwrap();
    let stream = "http://127.0.0.1:1234/pls/11111111111111111111111111111111";
    for contact in [
        "",
        "0",
        "file:///C:/private.txt",
        "javascript:alert(1)",
        "https://user:password@example.com/thread",
    ] {
        let command = player_command(&app, stream, "配信名", contact).unwrap();
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new(stream), OsStr::new("配信名"), OsStr::new("")]
        );
    }
}
