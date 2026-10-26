use std::{
    fs,
    path::{Path, PathBuf},
};

/// Validates an application-owned recording path before handing it to the OS.
pub fn validate_recording_file(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("録画ファイルのパスが正しくありません".into());
    }
    for entry in path.ancestors() {
        let metadata =
            fs::symlink_metadata(entry).map_err(|_| "録画ファイルまたは保存先が見つかりません")?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("リンク先のファイルは操作できません".into());
            }
        }
        if metadata.file_type().is_symlink() {
            return Err("リンク先のファイルは操作できません".into());
        }
    }
    let canonical = fs::canonicalize(path).map_err(|error| error.to_string())?;
    if !fs::metadata(&canonical)
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("録画ファイルを指定してください".into());
    }
    let extension = canonical
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "flv" | "wmv" | "asf" | "mp3" | "ogg" | "mkv" | "ts" | "mp4" | "bin"
    ) {
        return Err("録画用のメディアファイル以外は操作できません".into());
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_media_and_rejects_missing_directories_executables_and_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let media = temp.path().join("日本語 と空白.flv");
        fs::write(&media, b"fixture").unwrap();
        assert_eq!(
            validate_recording_file(&media).unwrap(),
            fs::canonicalize(&media).unwrap()
        );
        let wmv = temp.path().join("recording.wmv");
        fs::write(&wmv, b"fixture").unwrap();
        assert_eq!(
            validate_recording_file(&wmv).unwrap(),
            fs::canonicalize(&wmv).unwrap()
        );
        let executable = temp.path().join("file.exe");
        fs::write(&executable, b"fixture").unwrap();
        assert!(validate_recording_file(&executable).is_err());
        assert!(validate_recording_file(temp.path()).is_err());
        assert!(validate_recording_file(&temp.path().join("missing.flv")).is_err());
        assert!(validate_recording_file(Path::new("relative.flv")).is_err());
        assert_eq!(fs::read(media).unwrap(), b"fixture");
    }
}
