use peercast_rpc::server::{AccessLevel, LogProvider, RpcDispatcher};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Debug, Default)]
struct PrivateLogs(AtomicUsize);

impl LogProvider for PrivateLogs {
    fn settings(&self) -> Value {
        self.0.fetch_add(1, Ordering::Relaxed);
        json!({"level":5})
    }
    fn read(&self, _: usize, _: usize) -> Value {
        self.0.fetch_add(1, Ordering::Relaxed);
        json!({"log":"private diagnostic record"})
    }
    fn set_level(&self, _: u64) -> Result<(), String> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn clear(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[tokio::test]
async fn playback_access_cannot_read_or_change_an_enabled_log_backend() {
    let logs = Arc::new(PrivateLogs::default());
    let dispatcher = RpcDispatcher::default().with_logging(logs.clone());
    for method in ["getLog", "getLogSettings", "setLogSettings", "clearLog"] {
        let request =
            json!({"jsonrpc":"2.0","id":1,"method":method,"params":{"settings":{"level":5}}})
                .to_string();
        let denied = dispatcher
            .handle_request_str(&request, AccessLevel::Playback)
            .await
            .unwrap();
        let denied: Value = serde_json::from_str(&denied).unwrap();
        assert_eq!(denied["error"]["code"], -32001);
        assert!(!denied.to_string().contains("private diagnostic record"));
    }
    assert_eq!(logs.0.load(Ordering::Relaxed), 0);
    let request =
        json!({"jsonrpc":"2.0","id":2,"method":"getLog","params":{"from":0,"maxLines":1}})
            .to_string();
    let allowed = dispatcher
        .handle_request_str(&request, AccessLevel::Admin)
        .await
        .unwrap();
    assert!(allowed.contains("private diagnostic record"));
    assert_eq!(logs.0.load(Ordering::Relaxed), 1);
}
