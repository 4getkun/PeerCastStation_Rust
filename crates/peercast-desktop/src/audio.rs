use crate::rules::validate_sound_path;
use rodio::Source;
use std::{
    fs::File,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

static PLAYING: AtomicBool = AtomicBool::new(false);

struct ResetPlaying;
impl Drop for ResetPlaying {
    fn drop(&mut self) {
        PLAYING.store(false, Ordering::Release);
    }
}

pub async fn play_sound(path: String) -> Result<(), String> {
    validate_sound_path(&path)?;
    if PLAYING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _reset = ResetPlaying;
        let file = File::open(path).map_err(|error| format!("通知音を開けません: {error}"))?;
        if file.metadata().map_err(|error| error.to_string())?.len() > 16 * 1024 * 1024 {
            return Err("通知音は16MB以内のWAVファイルを指定してください".into());
        }
        let decoder = rodio::Decoder::try_from(file)
            .map_err(|error| format!("通知音を読み取れません: {error}"))?;
        let mut output = rodio::DeviceSinkBuilder::open_default_sink()
            .map_err(|error| format!("音声出力を開けません: {error}"))?;
        output.log_on_drop(false);
        let player = rodio::Player::connect_new(output.mixer());
        player.set_volume(0.65);
        player.append(decoder.take_duration(Duration::from_secs(10)));
        player.sleep_until_end();
        Ok(())
    })
    .await
    .map_err(|error| {
        PLAYING.store(false, Ordering::Release);
        error.to_string()
    })?
}
