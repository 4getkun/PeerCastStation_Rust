//! Small OS policy boundary. No shell expansion or network discovery.
use std::path::{Path, PathBuf};

pub const LIVE_SERVICES_SUPPORTED: bool = true;
pub const UPDATER_SUPPORTED: bool = cfg!(windows);

pub fn validate_executable(path: &Path) -> Result<(), String> {
    let metadata = path
        .metadata()
        .map_err(|error| format!("プレイヤーを確認できません: {error}"))?;
    if !metadata.is_file() {
        return Err("プレイヤーには実行ファイルを指定してください".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("プレイヤーに実行権限がありません".into());
        }
    }
    Ok(())
}

pub fn recording_directory(application_exe: &Path, requested: &Path) -> Result<PathBuf, String> {
    if requested.is_absolute() {
        return Ok(requested.to_owned());
    }
    #[cfg(target_os = "linux")]
    {
        let _ = application_exe;
        if requested
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err("相対保存先に .. は使用できません。絶対パスを指定してください".into());
        }
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .map(|p| p.join(".local/share"))
            })
            .ok_or("保存先を決定できません。録画設定で絶対パスを指定してください")?;
        Ok(base.join("com.fourgetkun.peercast-rt").join(requested))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(application_exe
            .parent()
            .ok_or("アプリの配置先を取得できません")?
            .join(requested))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linux_policy_supports_live_but_not_updater() {
        if cfg!(target_os = "linux") {
            assert!(LIVE_SERVICES_SUPPORTED);
            assert!(!UPDATER_SUPPORTED);
        }
    }
    #[test]
    fn absolute_recording_choice_is_preserved() {
        assert_eq!(
            recording_directory(Path::new("/opt/app/app"), Path::new("/tmp/recordings")).unwrap(),
            Path::new("/tmp/recordings")
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn relative_recordings_never_live_beside_installed_binary() {
        let p = recording_directory(Path::new("/opt/app/app"), Path::new("recordings")).unwrap();
        assert!(p.is_absolute());
        assert!(p.ends_with("com.fourgetkun.peercast-rt/recordings"));
        assert!(!p.starts_with("/opt/app"));
        assert!(recording_directory(Path::new("/opt/app/app"), Path::new("../escape")).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn executable_requires_file_and_permission() {
        use std::os::unix::fs::PermissionsExt;
        let p = std::env::temp_dir().join(format!("peercast-exec-test-{}", std::process::id()));
        std::fs::write(&p, b"fixture, never executed").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(validate_executable(&p).is_err());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validate_executable(&p).is_ok());
        std::fs::remove_file(&p).unwrap();
        assert!(validate_executable(&p).is_err());
        assert!(validate_executable(&std::env::temp_dir()).is_err());
    }
}

/// Launch on a waiter thread so every successfully spawned Linux child has an owner
/// that reaps it. Thread creation occurs before process creation; failure cannot
/// strand a child. Only startup is acknowledged synchronously, never playback exit.
#[cfg(target_os = "linux")]
pub fn spawn_player_process(command: std::process::Command) -> std::io::Result<u32> {
    spawn_owned_player(command, std::time::Duration::ZERO, None)
}

/// Only the newest launch is exposed. This does not mean media was decoded:
/// `running` acknowledges the process, while terminal status reports its exit.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveLaunchStatus {
    pub generation: u64,
    pub state: String,
    pub message: String,
}
impl Default for LiveLaunchStatus {
    fn default() -> Self {
        Self {
            generation: 0,
            state: "idle".into(),
            message: String::new(),
        }
    }
}
#[cfg(target_os = "linux")]
type LaunchTracker = std::sync::Arc<std::sync::Mutex<LiveLaunchStatus>>;
#[cfg(target_os = "linux")]
fn launch_tracker() -> &'static LaunchTracker {
    static TRACKER: std::sync::OnceLock<LaunchTracker> = std::sync::OnceLock::new();
    TRACKER.get_or_init(|| std::sync::Arc::new(std::sync::Mutex::new(LiveLaunchStatus::default())))
}
pub fn live_launch_status() -> LiveLaunchStatus {
    #[cfg(target_os = "linux")]
    {
        launch_tracker()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    #[cfg(not(target_os = "linux"))]
    {
        LiveLaunchStatus::default()
    }
}
#[cfg(target_os = "linux")]
fn set_launch_status(tracking: &Option<(LaunchTracker, u64)>, state: &str) {
    let Some((tracker, generation)) = tracking else {
        return;
    };
    let mut status = tracker.lock().unwrap_or_else(|p| p.into_inner());
    // Completion of an older player must never overwrite a newer launch.
    if status.generation != *generation {
        return;
    }
    status.state = state.into();
    status.message = if state == "failed" {
        "ライブ再生が終了しました。配信の状態・通信・依存ツールを確認して再試行してください".into()
    } else {
        String::new()
    };
}
/// Live-only bounded startup observation, then an owned waiter reports safe
/// terminal status. No raw player output or URL is retained in the tracker.
#[cfg(target_os = "linux")]
pub fn spawn_live_player_process(command: std::process::Command) -> std::io::Result<(u32, u64)> {
    spawn_tracked_live_player(command, launch_tracker().clone())
}
#[cfg(target_os = "linux")]
fn spawn_tracked_live_player(
    command: std::process::Command,
    tracker: LaunchTracker,
) -> std::io::Result<(u32, u64)> {
    let generation = {
        let mut status = tracker.lock().unwrap_or_else(|p| p.into_inner());
        status.generation += 1;
        status.state = "starting".into();
        status.message.clear();
        status.generation
    };
    let tracking = Some((tracker, generation));
    match spawn_owned_player(
        command,
        std::time::Duration::from_millis(900),
        tracking.clone(),
    ) {
        Ok(pid) => Ok((pid, generation)),
        Err(error) => {
            set_launch_status(&tracking, "failed");
            Err(error)
        }
    }
}

#[cfg(target_os = "linux")]
fn spawn_owned_player(
    mut command: std::process::Command,
    grace: std::time::Duration,
    tracking: Option<(LaunchTracker, u64)>,
) -> std::io::Result<u32> {
    let (started, startup) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("peercast-player-wait".into())
        .spawn(move || match command.spawn() {
            Ok(mut child) => {
                let pid = child.id();
                let deadline = std::time::Instant::now() + grace;
                while std::time::Instant::now() < deadline {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            set_launch_status(
                                &tracking,
                                if status.success() { "exited" } else { "failed" },
                            );
                            let result = if status.success() {
                                Ok(pid)
                            } else {
                                Err(std::io::Error::other("player exited during startup"))
                            };
                            let _ = started.send(result);
                            return;
                        }
                        Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break, // Keep ownership and use blocking wait below.
                    }
                }
                // Even if the caller has gone away, retain ownership and reap.
                set_launch_status(&tracking, "running");
                let _ = started.send(Ok(pid));
                loop {
                    match child.wait() {
                        Ok(status) => {
                            set_launch_status(
                                &tracking,
                                if status.success() { "exited" } else { "failed" },
                            );
                            if !status.success() {
                                eprintln!("player exited unsuccessfully: {status}");
                            }
                            break;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) => {
                            set_launch_status(&tracking, "failed");
                            eprintln!("player wait failed: {:?}", error.kind());
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                let _ = started.send(Err(error));
            }
        })?;
    startup
        .recv()
        .map_err(|_| std::io::Error::other("player startup worker disconnected"))?
}

// Retain existing process ownership behavior outside Linux.
#[cfg(not(target_os = "linux"))]
pub fn spawn_player_process(mut command: std::process::Command) -> std::io::Result<u32> {
    command.spawn().map(|child| child.id())
}

#[cfg(all(test, target_os = "linux"))]
mod player_reaping_tests {
    use super::spawn_player_process;
    use std::{
        path::PathBuf,
        process::Command,
        time::{Duration, Instant},
    };

    fn wait_until_reaped(pid: u32) {
        let proc = PathBuf::from(format!("/proc/{pid}"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while proc.exists() {
            assert!(
                Instant::now() < deadline,
                "child {pid} still present, possibly zombie"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(std::path::Path::new("/proc/self").exists());
    }

    #[test]
    fn short_player_is_reaped_while_parent_remains_alive() {
        let pid = spawn_player_process(Command::new("/usr/bin/true")).unwrap();
        wait_until_reaped(pid);
    }

    #[test]
    fn long_player_launch_returns_before_process_exit() {
        let mut command = Command::new("/usr/bin/sleep");
        command.arg("1");
        let start = Instant::now();
        let pid = spawn_player_process(command).unwrap();
        assert!(start.elapsed() < Duration::from_millis(750));
        assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
        wait_until_reaped(pid);
    }

    #[test]
    fn failed_player_spawn_returns_error_without_child() {
        let error =
            spawn_player_process(Command::new("/nonexistent-peercast-player-fixture/absent"))
                .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn repeated_player_launches_leave_no_zombies() {
        let pids = (0..32)
            .map(|_| spawn_player_process(Command::new("/usr/bin/true")).unwrap())
            .collect::<Vec<_>>();
        for pid in pids {
            wait_until_reaped(pid);
        }
    }
    #[test]
    fn live_player_reports_immediate_failure_and_reaps() {
        assert!(super::spawn_live_player_process(Command::new("/usr/bin/false")).is_err());
        let (pid, _) = super::spawn_live_player_process(Command::new("/usr/bin/true")).unwrap();
        wait_until_reaped(pid);
    }
    fn tracker() -> super::LaunchTracker {
        std::sync::Arc::new(std::sync::Mutex::new(super::LiveLaunchStatus::default()))
    }
    #[test]
    fn late_player_failure_is_reported_without_raw_output() {
        let tracker = tracker();
        // Fixed test-only shell fixture; production launches never use a shell.
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 1.1; exit 7"]);
        let (pid, generation) = super::spawn_tracked_live_player(command, tracker.clone()).unwrap();
        assert_eq!(tracker.lock().unwrap().state, "running");
        wait_until_reaped(pid);
        let deadline = Instant::now() + Duration::from_secs(1);
        while tracker.lock().unwrap().state == "running" {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let status = tracker.lock().unwrap();
        assert_eq!(status.generation, generation);
        assert_eq!(status.state, "failed");
        assert!(!status.message.is_empty());
        assert!(!status.message.contains("http"));
    }
    #[test]
    fn older_player_cannot_overwrite_newer_generation() {
        let tracker = tracker();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 1.1; exit 7"]);
        let (old_pid, old_generation) =
            super::spawn_tracked_live_player(command, tracker.clone()).unwrap();
        let (new_pid, new_generation) =
            super::spawn_tracked_live_player(Command::new("/usr/bin/true"), tracker.clone())
                .unwrap();
        assert!(new_generation > old_generation);
        wait_until_reaped(old_pid);
        wait_until_reaped(new_pid);
        let status = tracker.lock().unwrap();
        assert_eq!(status.generation, new_generation);
        assert_eq!(status.state, "exited");
        assert!(status.message.is_empty());
    }
    #[test]
    fn launch_status_serialization_is_stable() {
        let status = serde_json::to_value(super::LiveLaunchStatus::default()).unwrap();
        assert_eq!(
            status,
            serde_json::json!({"generation":0,"state":"idle","message":""})
        );
    }
}
