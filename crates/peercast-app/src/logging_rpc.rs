use crate::{BoundedLogBuffer, LogLevel};
use peercast_rpc::server::LogProvider;
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) struct RuntimeLogs(pub Arc<BoundedLogBuffer>);

impl std::fmt::Debug for RuntimeLogs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RuntimeLogs")
    }
}

impl LogProvider for RuntimeLogs {
    fn settings(&self) -> Value {
        let level = match self.0.minimum_level() {
            LogLevel::Off => 0,
            LogLevel::Error => 2,
            LogLevel::Warn => 3,
            LogLevel::Info => 4,
            LogLevel::Debug | LogLevel::Trace => 5,
        };
        json!({"level":level,"levelName":self.0.minimum_level().as_str(),"capacity":self.0.capacity()})
    }

    fn read(&self, from: usize, max_lines: usize) -> Value {
        let (oldest, retained) = self.0.window();
        let latest = oldest.saturating_add(retained.len() as u64);
        let from = if (from as u64) < oldest || (from as u64) > latest {
            oldest
        } else {
            from as u64
        };
        let entries: Vec<_> = retained
            .into_iter()
            .skip((from - oldest) as usize)
            .take(max_lines.min(1000))
            .collect();
        let lines: Vec<_> = entries
            .iter()
            .map(|entry| {
                format!(
                    "{} {} {}: {}",
                    entry.timestamp_ms,
                    entry.level.as_str(),
                    entry.target,
                    entry.message
                )
            })
            .collect();
        json!({"from":from,"lines":entries.len(),"next":from.saturating_add(entries.len() as u64),"oldest":oldest,"latest":latest,"log":lines.join("\n"),"entries":entries})
    }

    fn set_level(&self, level: u64) -> Result<(), String> {
        let level = match level {
            0 => LogLevel::Off,
            1 | 2 => LogLevel::Error,
            3 => LogLevel::Warn,
            4 => LogLevel::Info,
            5 => LogLevel::Debug,
            _ => return Err("settings.level must be an integer from 0 to 5".into()),
        };
        self.0.set_minimum_level(level);
        Ok(())
    }

    fn clear(&self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_polling_survives_eviction_lagging_clients_and_clear() {
        let buffer = Arc::new(BoundedLogBuffer::new(2));
        let provider = RuntimeLogs(Arc::clone(&buffer));
        buffer.log(LogLevel::Info, "test", "first");
        buffer.log(LogLevel::Info, "test", "second");
        let first = provider.read(0, 100);
        assert_eq!(first["next"], 2);
        buffer.log(LogLevel::Info, "test", "third");
        let next = provider.read(2, 100);
        assert_eq!(next["lines"], 1);
        assert_eq!(next["entries"][0]["message"], "third");
        buffer.log(LogLevel::Info, "test", "fourth");
        let lagging = provider.read(0, 100);
        assert_eq!(lagging["from"], 2);
        assert_eq!(lagging["next"], 4);
        provider.clear();
        buffer.log(LogLevel::Info, "test", "after clear");
        let restarted = provider.read(4, 100);
        assert_eq!(restarted["from"], 0);
        assert_eq!(restarted["entries"][0]["message"], "after clear");
    }
}
