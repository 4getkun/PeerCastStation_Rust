use peercast_desktop::{
    player::{configured_player_command, split_arguments, PlayerContext, PlayerSettings},
    preferences::DesktopPreferences,
    recording_config::RecordingSettings,
    recording_path::{create_recording_target, safe_channel_name, RecordingFileName},
    rules::{decorate_channels, normalize_rules, preview_rules, ChannelRule},
};
use serde_json::json;
use std::{ffi::OsStr, fs};
use tokio::io::AsyncWriteExt;

const URL: &str = "http://127.0.0.1:12345/pls/11111111111111111111111111111111";

fn rule(id: &str, pattern: &str) -> ChannelRule {
    ChannelRule {
        id: id.into(),
        name: id.into(),
        pattern: pattern.into(),
        ..ChannelRule::default()
    }
}

#[test]
fn windows_arguments_keep_paths_empty_arguments_and_japanese_spaces() {
    assert_eq!(
        split_arguments(r#"--one "C:\media folder\file.ini" "" 配信　テスト "a\"b""#).unwrap(),
        vec![
            "--one",
            "C:\\media folder\\file.ini",
            "",
            "配信　テスト",
            "a\"b"
        ]
    );
    assert_eq!(
        split_arguments(r#""C:\media\\" tail"#).unwrap(),
        vec!["C:\\media\\", "tail"]
    );
    assert!(split_arguments("\"unclosed").is_err());
    assert!(split_arguments(&vec!["x"; 129].join(" ")).is_err());
}

#[test]
fn channel_metadata_cannot_add_arguments_or_expand_another_placeholder() {
    let temp = tempfile::tempdir().unwrap();
    let player = temp.path().join("custom player.exe");
    fs::write(&player, b"fixture, never executed").unwrap();
    let context = PlayerContext::from_yp(URL.into(), &json!({"name":"配信\" --other <contact/>","contactUrl":"https://example.com/thread?a=1&b=2"})).unwrap();
    let settings = PlayerSettings {
        executable: Some(player.display().to_string()),
        arguments: "--title \"<channelname/>\" --source=<stream/> \"<contact/>\"".into(),
    };
    let command =
        configured_player_command(&temp.path().join("app.exe"), &settings, &context).unwrap();
    assert_eq!(command.get_program(), player);
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        vec![
            OsStr::new("--title"),
            OsStr::new("配信\" --other <contact/>"),
            OsStr::new(&format!("--source={URL}")),
            OsStr::new("https://example.com/thread?a=1&b=2")
        ]
    );
    assert_eq!(command.get_current_dir().unwrap(), temp.path());
}

#[test]
fn configurable_player_validates_targets_and_preserves_portable_defaults() {
    let default = serde_json::from_value::<PlayerSettings>(json!({})).unwrap();
    assert_eq!(default, PlayerSettings::default());
    for executable in ["bad.cmd", "bad.exe\n", "javascript:run"] {
        assert!(PlayerSettings {
            executable: Some(executable.into()),
            ..default.clone()
        }
        .normalize()
        .is_err());
    }
    assert!(PlayerSettings {
        arguments: "\"bad".into(),
        ..default
    }
    .normalize()
    .is_err());
    assert!(PlayerContext::from_yp(
        "https://example.com/pls/11111111111111111111111111111111".into(),
        &json!({})
    )
    .is_err());
}

#[test]
fn rule_priority_last_match_overrides_and_disabled_rules_do_not_apply() {
    let favorite = rule("favorite", "game");
    let mut hidden = rule("hidden", "game");
    hidden.favorite = false;
    hidden.hidden = true;
    hidden.text_color = Some("#aabbcc".into());
    let mut disabled = rule("disabled", "(");
    disabled.enabled = false;
    let rows = vec![
        json!({"name":"Game channel","channelId":"111","listeners":10}),
        json!({"name":"other","channelId":"222"}),
    ];
    let decorated = decorate_channels(
        rows.clone(),
        &[favorite.clone(), hidden.clone(), disabled],
        "http://127.0.0.1:12345",
    )
    .unwrap();
    assert_eq!(decorated[0]["isHidden"], true);
    assert_eq!(decorated[0]["isFavorite"], false);
    assert_eq!(decorated[0]["matchedRuleId"], "hidden");
    assert_eq!(decorated[0]["ruleTextColor"], "#AABBCC");
    let preview = preview_rules(&rows, &[hidden, favorite], "http://127.0.0.1:12345").unwrap();
    assert_eq!(
        (
            preview.total,
            preview.matched,
            preview.favorites,
            preview.hidden
        ),
        (2, 1, 1, 0)
    );
}

#[test]
fn rule_matching_handles_width_literal_regex_and_all_metadata_targets() {
    let mut width = rule("width", "ゲーム");
    let row = json!({"name":"ｹﾞｰﾑ配信","description":"a+b","yellowPage":"P@","listeners":123,"channelId":"ABC"});
    assert!(peercast_desktop::rules::rule_matches(&width, &row, "http://127.0.0.1:1234").unwrap());
    width.normalize_width = false;
    assert!(!peercast_desktop::rules::rule_matches(&width, &row, "http://127.0.0.1:1234").unwrap());
    let mut literal = rule("literal", "a+b");
    literal.fields = vec!["details".into()];
    assert!(
        peercast_desktop::rules::rule_matches(&literal, &row, "http://127.0.0.1:1234").unwrap()
    );
    literal.use_regex = true;
    assert!(
        !peercast_desktop::rules::rule_matches(&literal, &row, "http://127.0.0.1:1234").unwrap()
    );
    let mut listeners = rule("listeners", "^123$");
    listeners.use_regex = true;
    listeners.fields = vec!["listeners".into()];
    assert!(
        peercast_desktop::rules::rule_matches(&listeners, &row, "http://127.0.0.1:1234").unwrap()
    );
    let mut stream = rule("stream", "/stream/ABC");
    stream.fields = vec!["streamUrl".into()];
    assert!(peercast_desktop::rules::rule_matches(&stream, &row, "http://127.0.0.1:1234").unwrap());
}

#[test]
fn invalid_rules_are_rejected_before_persistence_or_styling() {
    let mut color = rule("color", "name");
    color.text_color = Some("red;display:none".into());
    assert!(normalize_rules(vec![color]).is_err());
    let mut regex = rule("regex", "(");
    regex.use_regex = true;
    assert!(normalize_rules(vec![regex]).is_err());
    assert!(normalize_rules(vec![rule("same", "a"), rule("same", "b")]).is_err());
    let mut field = rule("field", "a");
    field.fields = vec!["unknown".into()];
    assert!(normalize_rules(vec![field]).is_err());
    let mut sound = rule("sound", "a");
    sound.sound_path = Some("program.exe".into());
    assert!(normalize_rules(vec![sound]).is_err());
}

#[test]
fn legacy_desktop_preferences_and_schedule_have_safe_defaults() {
    let preferences: DesktopPreferences = serde_json::from_value(json!({})).unwrap();
    assert!(!preferences.recording.enabled);
    assert_eq!(preferences.recording.max_concurrent, 5);
    let schedule = RecordingSettings {
        all_day: false,
        watch_start: "23:00".into(),
        watch_end: "01:00".into(),
        ..RecordingSettings::default()
    };
    assert!(schedule.within_schedule(23 * 60 + 30));
    assert!(schedule.within_schedule(30));
    assert!(!schedule.within_schedule(12 * 60));
    assert!(RecordingSettings {
        watch_start: "25:00".into(),
        ..schedule.clone()
    }
    .normalize()
    .is_err());
    assert!(RecordingSettings {
        max_concurrent: 0,
        ..schedule
    }
    .normalize()
    .is_err());
}

#[tokio::test]
async fn recording_paths_cannot_escape_or_overwrite_prior_files() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app.exe");
    let settings = RecordingSettings {
        directory: temp.path().join("recordings").display().to_string(),
        ..RecordingSettings::default()
    };
    let filename = RecordingFileName::new("../../CON", "", chrono::Local::now());
    let mut first = create_recording_target(&app, &settings, &filename, "FLV", 1).unwrap();
    first.file.write_all(b"FIRST").await.unwrap();
    first.file.flush().await.unwrap();
    let second = create_recording_target(&app, &settings, &filename, "FLV", 1).unwrap();
    assert_ne!(first.path, second.path);
    let root = fs::canonicalize(temp.path().join("recordings")).unwrap();
    assert!(first.path.starts_with(&root));
    assert_eq!(first.path.extension().unwrap(), "flv");
    assert_eq!(fs::read(&first.path).unwrap(), b"FIRST");
    assert_eq!(safe_channel_name("CON"), "_CON");
    assert!(!safe_channel_name("../a\\b:c*").contains(['/', '\\', ':', '*']));
}

#[tokio::test]
async fn recording_names_number_only_files_after_the_first_and_keep_session_metadata() {
    use chrono::TimeZone;
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app.exe");
    let settings = RecordingSettings {
        directory: temp.path().join("recordings").display().to_string(),
        channel_subfolders: false,
        ..RecordingSettings::default()
    };
    let began = chrono::Local
        .with_ymd_and_hms(2026, 10, 25, 21, 30, 5)
        .single()
        .unwrap();
    let filename = RecordingFileName::new(
        "チャンネル名",
        "steam NEXTフェス ～気になったゲームをやりつくす！への旅路～ - <Free>",
        began,
    );
    let stem =
        "チャンネル名_20261025_213005_steam NEXTフェス ～気になったゲームをやりつくす！への旅路～";
    for (part, suffix) in [(1, ""), (2, "_002"), (3, "_003")] {
        let mut target = create_recording_target(&app, &settings, &filename, "FLV", part).unwrap();
        assert_eq!(
            target.path.file_name().unwrap().to_str().unwrap(),
            format!("{stem}{suffix}.flv")
        );
        target
            .file
            .write_all(format!("part {part}").as_bytes())
            .await
            .unwrap();
        target.file.flush().await.unwrap();
    }
    let duplicate = create_recording_target(&app, &settings, &filename, "FLV", 1).unwrap();
    assert_eq!(
        duplicate.path.file_name().unwrap().to_str().unwrap(),
        format!("{stem} (2).flv")
    );
    assert_eq!(
        fs::read(temp.path().join("recordings").join(format!("{stem}.flv"))).unwrap(),
        b"part 1"
    );
    let no_title = RecordingFileName::new("チャンネル名", "  ", began);
    let target = create_recording_target(&app, &settings, &no_title, "WMV", 1).unwrap();
    assert_eq!(
        target.path.file_name().unwrap().to_str().unwrap(),
        "チャンネル名_20261025_213005.asf"
    );
}
