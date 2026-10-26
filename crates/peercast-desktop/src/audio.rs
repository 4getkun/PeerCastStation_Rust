use crate::rules::validate_sound_path;
use rodio::Source;
use std::{
    fs::File,
    io::Cursor,
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
    play_source(move || {
        let file = File::open(path).map_err(|error| format!("通知音を開けません: {error}"))?;
        if file.metadata().map_err(|error| error.to_string())?.len() > 16 * 1024 * 1024 {
            return Err("通知音は16MB以内のWAVファイルを指定してください".into());
        }
        rodio::Decoder::try_from(file).map_err(|error| format!("通知音を読み取れません: {error}"))
    })
    .await
}

const FAVORITE_CHIME: &[u8] = include_bytes!("../frontend/assets/sounds/favorite-notification.wav");

pub async fn play_favorite_chime() -> Result<(), String> {
    play_source(|| {
        rodio::Decoder::try_from(Cursor::new(FAVORITE_CHIME))
            .map_err(|error| format!("標準通知音を読み取れません: {error}"))
    })
    .await
}

async fn play_source<S, F>(open: F) -> Result<(), String>
where
    S: Source + Send + 'static,
    F: FnOnce() -> Result<S, String> + Send + 'static,
{
    if PLAYING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _reset = ResetPlaying;
        let decoder = open()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_chime_decodes_to_a_short_unclipped_stereo_sound() {
        let decoder = rodio::Decoder::try_from(Cursor::new(FAVORITE_CHIME)).unwrap();
        assert_eq!(decoder.channels().get(), 2);
        assert_eq!(decoder.sample_rate().get(), 44_100);
        let samples: Vec<_> = decoder.collect();
        assert_eq!(samples.len(), 44_100 * 2 * 5 / 4);
        let peak = samples
            .iter()
            .copied()
            .map(f32::abs)
            .fold(0.0_f32, f32::max);
        assert!((0.20..=0.23).contains(&peak));
        assert_eq!(samples[0], 0.0);
        assert!(samples.last().unwrap().abs() <= 1.0 / 32768.0);
    }
}
