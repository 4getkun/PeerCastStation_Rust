use peercast_desktop::backend::DesktopBackend;
use serde_json::Value;
use std::net::SocketAddr;
use std::time::Duration;

fn usable_tracker(row: &Value) -> bool {
    let Some(id) = row.get("channelId").and_then(Value::as_str) else {
        return false;
    };
    if id.len() != 32 || id.bytes().all(|byte| byte == b'0') {
        return false;
    }
    let Some(tracker) = row.get("tracker").and_then(Value::as_str) else {
        return false;
    };
    tracker.parse::<SocketAddr>().is_ok_and(|addr| {
        addr.port() != 0 && !addr.ip().is_unspecified() && !addr.ip().is_multicast()
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit live Yellow Page probe; may contact public peers"]
async fn real_yellow_page_channel_can_be_received_without_player_launch() {
    let dir = tempfile::tempdir().expect("temporary config directory");
    let backend = tokio::time::timeout(
        Duration::from_secs(8),
        DesktopBackend::open(dir.path().join("desktop.json")),
    )
    .await
    .expect("backend start timed out")
    .expect("backend start failed");

    let outcome: Result<(), String> = async {
        let rows = tokio::time::timeout(Duration::from_secs(20), backend.get_yp_channels(true))
            .await
            .map_err(|_| "directory request timed out".to_owned())??;
        let rows = rows
            .as_array()
            .ok_or_else(|| "directory response was not an array".to_owned())?;
        let requested_name = std::env::var("PEERCAST_PROBE_CHANNEL_NAME").ok();
        let mut candidates: Vec<&Value> = rows
            .iter()
            .filter(|row| {
                let Some(name) = row.get("name").and_then(Value::as_str) else {
                    return false;
                };
                requested_name.as_deref().map_or(name != "シン・ゴドー", |wanted| name == wanted)
                    && usable_tracker(row)
                    && (requested_name.is_some()
                        || row
                            .get("contentType")
                            .and_then(Value::as_str)
                            .is_some_and(|kind| kind.eq_ignore_ascii_case("FLV")))
            })
            .collect();
        candidates.truncate(if requested_name.is_some() { 1 } else { 4 });
        if candidates.is_empty() {
            return Err("no matching playable directory entries".into());
        }

        let mut prepare_rejections = 0usize;
        let mut prepare_timeouts = 0usize;
        let mut timed_out = 0usize;
        let mut snapshot_failures = 0usize;
        for row in candidates {
            let name = row["name"].as_str().unwrap_or("(unnamed)");
            let id = row["channelId"].as_str().unwrap_or_default();
            let yellow_page = row["yellowPage"].as_str().unwrap_or_default();
            match tokio::time::timeout(
                Duration::from_secs(5),
                backend.prepare_yp_channel(id, yellow_page),
            )
            .await
            {
                Ok(Ok(_playlist_url)) => {}
                Ok(Err(_)) => {
                    prepare_rejections += 1;
                    continue;
                }
                Err(_) => {
                    prepare_timeouts += 1;
                    continue;
                }
            }

            let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
            let mut received = false;
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_secs(2), backend.snapshot()).await {
                    Ok(Ok(snapshot)) => {
                        if snapshot
                            .channels
                            .iter()
                            .any(|channel| channel.id.eq_ignore_ascii_case(id) && channel.status == "受信中")
                        {
                            eprintln!("received channel name={name:?} id={id} available=true");
                            received = true;
                            break;
                        }
                    }
                    Ok(Err(_)) | Err(_) => snapshot_failures += 1,
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            if received {
                return Ok(());
            }
            if let Ok(snapshot) = backend.snapshot().await {
                let marker = format!("channel_id={}", id.to_ascii_uppercase());
                let messages: Vec<_> = snapshot.logs.iter()
                    .filter(|entry| entry.message.to_ascii_uppercase().contains(&marker.to_ascii_uppercase()))
                    .map(|entry| entry.message.as_str()).collect();
                eprintln!("unavailable channel name={name:?} http503={} http200={} attempt_errors={} no_candidates={}",
                    messages.iter().filter(|line| line.contains("HTTP 503")).count(),
                    messages.iter().filter(|line| line.contains("HTTP 200")).count(),
                    messages.iter().filter(|line| line.contains("attempt failed")).count(),
                    messages.iter().filter(|line| line.contains("candidate_count=0")).count());
            }
            timed_out += 1;
        }

        eprintln!(
            "no channel received: prepare_rejections={prepare_rejections} prepare_timeouts={prepare_timeouts} receive_timeouts={timed_out} snapshot_failures={snapshot_failures} reason=no_candidate_reached_receiving_state"
        );
        Err("no candidate reached receiving state".into())
    }
    .await;

    let close_result = tokio::time::timeout(Duration::from_secs(8), backend.close())
        .await
        .map_err(|_| "backend shutdown timed out".to_owned())
        .and_then(|result| result);
    close_result.expect("backend shutdown failed");
    outcome.expect("live probe failed");
}
