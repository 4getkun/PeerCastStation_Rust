//! Live-service dependency policy. Trust is anchored in this compiled manifest,
//! never in adjacent JSON, PATH, or IPC. The application directory must be writable
//! only by trusted installers/users. Hashing does not defeat a malicious same-user
//! concurrent replacement between validation and execution.
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveDependencies {
    pub monitor_ready: bool,
    pub playback_ready: bool,
    pub reason_code: String,
    pub message: String,
}

#[derive(Clone, Debug)]
struct DependencyError {
    code: &'static str,
    message: &'static str,
}
impl DependencyError {
    fn invalid() -> Self {
        Self {
            code: "invalid",
            message: "ライブ依存の整合性を確認できません。検証済みの依存を再配置してください",
        }
    }
    fn missing() -> Self {
        Self {
            code: "missing",
            message:
                "ライブ依存が不足しています。検証済みの Linux 依存パッケージを配置してください",
        }
    }
}

const CACHE_TTL: Duration = Duration::from_secs(60);
static CACHE: OnceLock<Mutex<Option<(Instant, LiveDependencies)>>> = OnceLock::new();

/// Cheap enough for polling: at most one bounded validation per minute. No process
/// is launched. Launch/probe paths deliberately do not rely on this cached result.
pub fn dependencies() -> LiveDependencies {
    cached_dependencies(false)
}
pub fn refresh_dependencies() -> LiveDependencies {
    cached_dependencies(true)
}
fn cached_dependencies(force: bool) -> LiveDependencies {
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if !force {
        if let Some((checked, state)) = cache.as_ref() {
            if checked.elapsed() < CACHE_TTL {
                return state.clone();
            }
        }
    }
    let state = inspect_dependencies();
    *cache = Some((Instant::now(), state.clone()));
    state
}
fn failed(error: DependencyError, monitor_ready: bool) -> LiveDependencies {
    LiveDependencies {
        monitor_ready,
        playback_ready: false,
        reason_code: error.code.into(),
        message: error.message.into(),
    }
}
fn ready() -> LiveDependencies {
    LiveDependencies {
        monitor_ready: true,
        playback_ready: true,
        reason_code: "ready".into(),
        message: String::new(),
    }
}
fn inspect_dependencies() -> LiveDependencies {
    #[cfg(not(target_os = "linux"))]
    {
        ready()
    } // Preserve existing Windows helper/player validation at launch.
    #[cfg(target_os = "linux")]
    {
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(_) => return failed(DependencyError::invalid(), false),
        };
        if let Err(e) = linux::monitor(&exe) {
            return failed(e, false);
        }
        match linux::playback(&exe) {
            Ok(_) => ready(),
            Err(e) => failed(e, true),
        }
    }
}

/// Resolution alone does not require installed dependencies, so registration and
/// persisted channel management continue working while dependencies are missing.
#[cfg(target_os = "linux")]
pub fn monitor_path(application_exe: &Path) -> Result<PathBuf, String> {
    linux::root(application_exe)
        .map(|p| p.join("yt-dlp_linux"))
        .map_err(|e| e.message.into())
}
#[cfg(target_os = "linux")]
pub fn verified_monitor(application_exe: &Path) -> Result<PathBuf, String> {
    linux::monitor(application_exe).map_err(|e| e.message.into())
}
#[cfg(target_os = "linux")]
pub fn verify_probe_path(candidate: &Path) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|_| DependencyError::invalid().message)?;
    let expected = monitor_path(&exe)?;
    if candidate != expected {
        return Err(DependencyError::invalid().message.into());
    }
    verified_monitor(&exe)
}

#[cfg(target_os = "linux")]
pub struct PlaybackTools {
    pub helper: PathBuf,
    pub runtime: PathBuf,
    pub mpv: PathBuf,
}
#[cfg(target_os = "linux")]
pub fn verified_playback(application_exe: &Path) -> Result<PlaybackTools, String> {
    linux::playback(application_exe).map_err(|e| e.message.into())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{fs, io::Read, os::unix::fs::PermissionsExt};

    #[derive(Clone, Copy)]
    struct Artifact {
        schema: u32,
        os: &'static str,
        arch: &'static str,
        name: &'static str,
        version: &'static str,
        relative_path: &'static str,
        source: &'static str,
        sha256: &'static str,
        max_bytes: u64,
    }
    // Pins must come from the reviewed official artifact evidence. None means
    // deliberately not executable; neither placeholder hashes nor user manifests
    // can enable this path. The Linux standalone artifact must include EJS.
    const YT_DLP: Option<Artifact> = Some(Artifact {
        schema: 1,
        os: "linux",
        arch: "x86_64",
        name: "yt-dlp_linux",
        version: "2026.08.19",
        relative_path: "yt-dlp_linux",
        source: "https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/yt-dlp_linux",
        sha256: "58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a",
        // Official artifact observed: 40,446,224 bytes. Not the Windows 30 MiB cap.
        max_bytes: 48 * 1024 * 1024,
    });
    // Parent-reviewed mixed trust (2026-10-30): EJS 0.8.0 is bundled in the
    // pinned artifact. Node / mpv are OS-package-managed, NOT hash-pinned assets.
    // Verified local versions: Node 22.22.1, mpv 0.41.0. Package updates require
    // renewed playback QA; metadata readiness does not attest version/behavior.
    const YT_DLP_HAS_BUNDLED_EJS: bool = true;

    pub(super) fn root(exe: &Path) -> Result<PathBuf, DependencyError> {
        if !cfg!(target_arch = "x86_64") {
            return Err(DependencyError {
                code: "unsupported",
                message: "この CPU 用のライブ依存は未対応です。対応版を使用してください",
            });
        }
        if !exe.is_absolute() {
            return Err(DependencyError::invalid());
        }
        let parent = exe.parent().ok_or_else(DependencyError::invalid)?;
        Ok(parent.join("resources/live/linux-x86_64"))
    }
    fn pin(value: Option<Artifact>) -> Result<Artifact, DependencyError> {
        value.ok_or(DependencyError { code: "missing", message: "Linux ライブ依存の固定情報が未確定です。検証済み依存を含む更新版を使用してください" })
    }
    fn validate_manifest(artifact: Artifact) -> Result<(), DependencyError> {
        let relative = Path::new(artifact.relative_path);
        if artifact.schema != 1
            || artifact.os != "linux"
            || artifact.arch != std::env::consts::ARCH
            || artifact.name.is_empty()
            || artifact.version.is_empty()
            || !artifact.source.starts_with("https://github.com/")
            || artifact.sha256.len() != 64
            || !artifact.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || artifact.max_bytes == 0
            || relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(DependencyError::invalid());
        }
        Ok(())
    }
    fn io_error(error: std::io::Error) -> DependencyError {
        if error.kind() == std::io::ErrorKind::NotFound {
            DependencyError::missing()
        } else {
            DependencyError::invalid()
        }
    }
    /// Check each descendant of the executable directory; no symlink can redirect
    /// a managed directory or artifact outside the fixed layout.
    fn verify(exe: &Path, artifact: Artifact) -> Result<PathBuf, DependencyError> {
        validate_manifest(artifact)?;
        let root = root(exe)?;
        let parent = exe.parent().ok_or_else(DependencyError::invalid)?;
        let path = root.join(artifact.relative_path);
        let relative = path
            .strip_prefix(parent)
            .map_err(|_| DependencyError::invalid())?;
        let mut cursor = parent.to_path_buf();
        for part in relative.components() {
            if !matches!(part, std::path::Component::Normal(_)) {
                return Err(DependencyError::invalid());
            }
            cursor.push(part);
            if fs::symlink_metadata(&cursor)
                .map_err(io_error)?
                .file_type()
                .is_symlink()
            {
                return Err(DependencyError::invalid());
            }
        }
        // Reject special files before opening (opening a FIFO could block).
        let before = fs::symlink_metadata(&path).map_err(io_error)?;
        if !before.is_file()
            || before.len() == 0
            || before.len() > artifact.max_bytes
            || before.permissions().mode() & 0o111 == 0
        {
            return Err(DependencyError::invalid());
        }
        let mut file = fs::File::open(&path).map_err(io_error)?;
        let meta = file.metadata().map_err(io_error)?;
        if !meta.is_file()
            || meta.len() == 0
            || meta.len() > artifact.max_bytes
            || meta.permissions().mode() & 0o111 == 0
        {
            return Err(DependencyError::invalid());
        }
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut read = 0u64;
        loop {
            let count = file.read(&mut buffer).map_err(io_error)?;
            if count == 0 {
                break;
            }
            read += count as u64;
            if read > artifact.max_bytes {
                return Err(DependencyError::invalid());
            }
            digest.update(&buffer[..count]);
        }
        if format!("{:x}", digest.finalize()) != artifact.sha256 {
            return Err(DependencyError::invalid());
        }
        Ok(path)
    }
    pub(super) fn monitor(exe: &Path) -> Result<PathBuf, DependencyError> {
        root(exe)?;
        verify(exe, pin(YT_DLP)?)
    }
    fn system_tool(path: &str, missing: &'static str) -> Result<PathBuf, DependencyError> {
        let path = PathBuf::from(path);
        let canonical = fs::canonicalize(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                DependencyError {
                    code: "missing",
                    message: missing,
                }
            } else {
                DependencyError::invalid()
            }
        })?;
        // Reject redirects to an unexpected executable, including user-managed
        // alternatives. No PATH search or silent fallback.
        if canonical != path {
            return Err(DependencyError::invalid());
        }
        crate::platform::validate_executable(&path).map_err(|_| DependencyError::invalid())?;
        Ok(path)
    }
    pub(super) fn playback(exe: &Path) -> Result<PlaybackTools, DependencyError> {
        let helper = monitor(exe)?;
        let runtime = system_tool(
            "/usr/bin/node",
            "Node.js が不足しています。OS 管理の対応版 Node.js を /usr/bin/node に用意してください",
        )?;
        if !YT_DLP_HAS_BUNDLED_EJS {
            return Err(DependencyError::missing());
        }
        let mpv = system_tool(
            "/usr/bin/mpv",
            "mpv が不足しています。OS 管理の mpv を /usr/bin/mpv に用意してください",
        )?;
        Ok(PlaybackTools {
            helper,
            runtime,
            mpv,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        const FIXTURE: Artifact = Artifact {
            schema: 1,
            os: "linux",
            arch: "x86_64",
            name: "fixture",
            version: "test",
            relative_path: "fixture",
            source: "https://github.com/example/release",
            sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
            max_bytes: 5,
        };
        fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
            let temp = tempfile::tempdir().unwrap();
            let exe = temp.path().join("app");
            let root = root(&exe).unwrap();
            fs::create_dir_all(&root).unwrap();
            let path = root.join("fixture");
            fs::write(&path, b"hello").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            (temp, exe, path)
        }
        #[test]
        fn valid_fixture_and_missing_dependency() {
            let (_temp, exe, path) = fixture();
            assert_eq!(verify(&exe, FIXTURE).unwrap(), path);
            fs::remove_file(&path).unwrap();
            assert_eq!(verify(&exe, FIXTURE).unwrap_err().code, "missing");
        }
        #[test]
        fn invalid_hash_size_mode_and_type() {
            let (_temp, exe, path) = fixture();
            fs::write(&path, b"jello").unwrap();
            assert_eq!(verify(&exe, FIXTURE).unwrap_err().code, "invalid");
            fs::write(&path, b"hello!").unwrap();
            assert!(verify(&exe, FIXTURE).is_err());
            fs::write(&path, b"hello").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(verify(&exe, FIXTURE).is_err());
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
            assert!(verify(&exe, FIXTURE).is_err());
        }
        #[test]
        fn unknown_manifest_and_traversal_fail_closed() {
            let (_temp, exe, _) = fixture();
            for a in [
                Artifact {
                    schema: 99,
                    ..FIXTURE
                },
                Artifact {
                    arch: "unknown",
                    ..FIXTURE
                },
                Artifact {
                    relative_path: "../fixture",
                    ..FIXTURE
                },
                Artifact {
                    relative_path: "/tmp/fixture",
                    ..FIXTURE
                },
                Artifact {
                    sha256: "",
                    ..FIXTURE
                },
            ] {
                assert!(verify(&exe, a).is_err());
            }
        }
        #[test]
        fn artifact_and_directory_symlinks_are_rejected() {
            let (temp, exe, path) = fixture();
            fs::remove_file(&path).unwrap();
            let outside = temp.path().join("outside");
            fs::write(&outside, b"hello").unwrap();
            std::os::unix::fs::symlink(&outside, &path).unwrap();
            assert!(verify(&exe, FIXTURE).is_err());
            fs::remove_file(&path).unwrap();
            fs::remove_dir(path.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(temp.path(), path.parent().unwrap()).unwrap();
            assert!(verify(&exe, FIXTURE).is_err());
        }
        #[test]
        fn absent_managed_artifact_cannot_enable_playback() {
            let temp = tempfile::tempdir().unwrap();
            assert!(playback(&temp.path().join("app")).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serialized_contract_and_cache_are_stable() {
        let a = refresh_dependencies();
        let b = dependencies();
        let a = serde_json::to_value(a).unwrap();
        assert_eq!(a, serde_json::to_value(b).unwrap());
        for name in ["monitorReady", "playbackReady", "reasonCode", "message"] {
            assert!(a.get(name).is_some());
        }
        assert!(["missing", "invalid", "unsupported", "ready"]
            .contains(&a["reasonCode"].as_str().unwrap()));
    }
}
