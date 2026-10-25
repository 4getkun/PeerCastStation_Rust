use peercast_app::ConfigManager;
use peercast_desktop::{
    backend::{DesktopBackend, YellowPageEntry},
    recording::RecordingManager,
    recording_config::RecordingSettings,
};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::test]
async fn queue_deduplicates_active_entries_and_preserves_order_controls() {
    let manager = RecordingManager::new();
    let first = manager
        .enqueue(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            "Fixture".into(),
            false,
        )
        .await
        .unwrap();
    let duplicate = manager
        .enqueue(
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
            "Fixture".into(),
            true,
        )
        .await
        .unwrap();
    assert_eq!(first, duplicate);
    let second = manager
        .enqueue(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            "Fixture".into(),
            false,
        )
        .await
        .unwrap();
    manager.control(&second, "up").await.unwrap();
    let jobs = manager.snapshot().await;
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].id, second);
    assert_eq!(jobs[1].id, first);
    manager.control(&first, "stop").await.unwrap();
    assert_eq!(manager.snapshot().await[1].status, "stopped");
}

#[tokio::test]
async fn stop_and_remove_only_change_job_records() {
    let manager = RecordingManager::new();
    let id = manager
        .enqueue(
            "cccccccccccccccccccccccccccccccc".into(),
            "Fixture".into(),
            true,
        )
        .await
        .unwrap();
    manager.control(&id, "remove").await.unwrap();
    assert!(manager.snapshot().await.is_empty());
    manager.shutdown().await.unwrap();
    assert!(manager
        .enqueue(
            "dddddddddddddddddddddddddddddddd".into(),
            "Fixture".into(),
            false
        )
        .await
        .is_err());
}

#[test]
fn schedule_supports_daytime_and_midnight_spans() {
    let mut settings = RecordingSettings {
        all_day: false,
        watch_start: "09:00".into(),
        watch_end: "17:00".into(),
        ..RecordingSettings::default()
    };
    assert!(settings.within_schedule(9 * 60));
    assert!(settings.within_schedule(17 * 60));
    assert!(!settings.within_schedule(8 * 60 + 59));
    settings.watch_start = "22:00".into();
    settings.watch_end = "02:00".into();
    assert!(settings.within_schedule(23 * 60));
    assert!(settings.within_schedule(60));
    assert!(!settings.within_schedule(12 * 60));
}

fn atom_leaf(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}
fn atom_parent(tag: &[u8; 4], children: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(0x8000_0000u32 | children.len() as u32).to_le_bytes());
    for child in children {
        out.extend_from_slice(&child);
    }
    out
}
fn peer_cast_fixture(channel_id: &str, body: &[u8]) -> Vec<u8> {
    let sid = atom_leaf(b"sid\0", &[7; 16]);
    let oleh = atom_parent(b"oleh", vec![sid]);
    let ok = atom_leaf(b"ok\0\0", &1u32.to_le_bytes());
    let head = atom_parent(
        b"pkt\0",
        vec![
            atom_leaf(b"type", b"head"),
            atom_leaf(b"pos\0", &0u32.to_le_bytes()),
            atom_leaf(b"data", b"RAW-HEADER"),
        ],
    );
    let pkt = atom_parent(
        b"pkt\0",
        vec![
            atom_leaf(b"type", b"data"),
            atom_leaf(b"pos\0", &0u32.to_le_bytes()),
            atom_leaf(b"data", body),
        ],
    );
    let chan_head = atom_parent(
        b"chan",
        vec![atom_leaf(b"id\0\0", &hex_id(channel_id)), head],
    );
    let chan_data = atom_parent(
        b"chan",
        vec![atom_leaf(b"id\0\0", &hex_id(channel_id)), pkt],
    );
    [oleh, ok, chan_head, chan_data].concat()
}
fn peer_cast_data(channel_id: &str, body: &[u8], pos: u32) -> Vec<u8> {
    let pkt = atom_parent(
        b"pkt\0",
        vec![
            atom_leaf(b"type", b"data"),
            atom_leaf(b"pos\0", &pos.to_le_bytes()),
            atom_leaf(b"data", body),
        ],
    );
    atom_parent(
        b"chan",
        vec![atom_leaf(b"id\0\0", &hex_id(channel_id)), pkt],
    )
}
fn hex_id(value: &str) -> Vec<u8> {
    (0..16)
        .map(|i| u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backend_recording_writes_loopback_media_and_stop_closes_partial_file() {
    const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const MEDIA: &[u8] = b"loopback-media-fixture-0123456789";
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("desktop.json");
    let tracker = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tracker_addr = tracker.local_addr().unwrap().to_string();
    let directory = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let directory_url = format!("http://{}/index.txt", directory.local_addr().unwrap());
    let body = format!(
        "Recording Fixture<>{ID}<>{tracker_addr}<>http://example.com<>test<>fixture<>-1<>-1<>128<>RAW<>Artist<>Album<>Title<>http://example.com<>test<>00:01<>click<>comment<>free<>\n"
    );
    let directory_task = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = directory.accept().await.unwrap();
            let mut req = [0u8; 2048];
            let _ = socket.read(&mut req).await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let backend = DesktopBackend::open(config_path.clone()).await.unwrap();
    backend
        .apply_yellow_pages(vec![YellowPageEntry {
            name: "Fixture".into(),
            channels_url: directory_url.clone(),
        }])
        .await
        .unwrap();
    backend.get_yp_channels(true).await.unwrap();
    let mut desktop = backend.snapshot().await.unwrap().desktop;
    desktop.recording.directory = dir.path().join("recordings").to_string_lossy().into_owned();
    desktop.recording.retry_interval_secs = 1;
    desktop.recording.monitor_interval_secs = 10;
    backend
        .apply_desktop_preferences(
            backend.snapshot().await.unwrap().limits,
            vec![YellowPageEntry {
                name: "Fixture".into(),
                channels_url: directory_url,
            }],
            desktop,
        )
        .await
        .unwrap();
    backend.get_yp_channels(true).await.unwrap();
    directory_task.await.unwrap();
    let media = MEDIA.to_vec();
    let tracker_task = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(8), tracker.accept())
                .await
                .unwrap()
                .unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    return;
                }
                req.extend_from_slice(&buf[..n]);
                if req.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            let wire = peer_cast_fixture(ID, &media);
            socket
                .write_all(b"HTTP/1.0 200 OK\r\nContent-Type: application/x-peercast-pcp\r\n\r\n")
                .await
                .unwrap();
            socket.write_all(&wire).await.unwrap();
            for i in 1..100u32 {
                if socket
                    .write_all(&peer_cast_data(ID, &media, i * media.len() as u32))
                    .await
                    .is_err()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    });
    let job = backend
        .enqueue_recording(ID.into(), "Fixture".into(), false)
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let rows = backend.snapshot().await.unwrap().recordings;
            if let Some(row) = rows.iter().find(|r| {
                r.id == job
                    && r.bytes_written >= (MEDIA.len() + b"RAW-HEADER".len()) as u64
                    && r.segments == 1
            }) {
                break row.clone();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("loopback recording should persist media bytes");
    assert_eq!(
        first.retries, 0,
        "A direct recording must wait for its relay before opening the local HTTP stream"
    );
    let live = tokio::time::timeout(Duration::from_secs(1), backend.snapshot())
        .await
        .expect("connection snapshot must not wait for the recording session lock")
        .unwrap();
    let recorded_channel = live
        .channels
        .iter()
        .find(|channel| {
            channel.id.eq_ignore_ascii_case(ID) && channel.diagnostics.purpose == "recording"
        })
        .expect("recording runtime should be visible in connections");
    assert!(recorded_channel
        .diagnostics
        .connections
        .iter()
        .any(|connection| connection.kind == "source" && connection.status == "connected"));
    assert!(recorded_channel
        .diagnostics
        .connections
        .iter()
        .any(|connection| connection.kind == "direct" && connection.sent_bytes > 0));

    let first_file = first
        .current_file
        .clone()
        .expect("first segment should have a path");
    backend.control_recording(&job, "reconnect").await.unwrap();
    let second = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let rows = backend.snapshot().await.unwrap().recordings;
            if let Some(row) = rows.iter().find(|r| {
                r.id == job
                    && r.segments >= 2
                    && r.current_file.as_deref() != Some(first_file.as_str())
            }) {
                break row.clone();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("manual reconnect should resume recording into segment two");
    let second_file = second
        .current_file
        .clone()
        .expect("second segment should have a path");
    assert!(Path::new(&second_file)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("_002"));
    backend.control_recording(&job, "stop").await.unwrap();
    assert_eq!(
        backend
            .snapshot()
            .await
            .unwrap()
            .recordings
            .iter()
            .find(|r| r.id == job)
            .unwrap()
            .status,
        "stopped"
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while backend
            .snapshot()
            .await
            .unwrap()
            .channels
            .iter()
            .any(|channel| channel.diagnostics.purpose == "recording")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("stopped recording runtime should disappear after cancellation completes");
    tokio::time::timeout(Duration::from_secs(8), backend.close())
        .await
        .unwrap()
        .unwrap();
    for file in [&first_file, &second_file] {
        let bytes = tokio::fs::read(file).await.unwrap();
        assert!(bytes.starts_with(&[b"RAW-HEADER".as_slice(), MEDIA].concat()));
        assert!(std::fs::canonicalize(file)
            .unwrap()
            .starts_with(std::fs::canonicalize(dir.path()).unwrap()));
    }
    tokio::time::timeout(Duration::from_secs(3), tracker_task)
        .await
        .unwrap()
        .unwrap();
    let _ = ConfigManager::new(&config_path).load().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn source_open_failure_retries_then_finishes_at_configured_bound() {
    const ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let dir = tempfile::tempdir().unwrap();
    let backend = DesktopBackend::open(dir.path().join("desktop.json"))
        .await
        .unwrap();
    let directory = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/index.txt", directory.local_addr().unwrap());
    let body = format!(
        "Unavailable<>{ID}<>0.0.0.0:7144<>http://example.com<>test<>fixture<>-1<>-1<>128<>RAW<>Artist<>Album<>Title<>http://example.com<>test<>00:01<>click<>comment<>free<>\n"
    );
    let listing = tokio::spawn(async move {
        let (mut socket, _) = directory.accept().await.unwrap();
        let mut req = [0u8; 1024];
        let _ = socket.read(&mut req).await.unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    let yp = YellowPageEntry {
        name: "Fixture".into(),
        channels_url: url.clone(),
    };
    backend.apply_yellow_pages(vec![yp.clone()]).await.unwrap();
    backend.get_yp_channels(true).await.unwrap();
    listing.await.unwrap();
    let mut desktop = backend.snapshot().await.unwrap().desktop;
    desktop.recording.retry_count = 1;
    desktop.recording.retry_interval_secs = 1;
    backend
        .apply_desktop_preferences(backend.snapshot().await.unwrap().limits, vec![yp], desktop)
        .await
        .unwrap();
    let job = backend
        .enqueue_recording(ID.into(), "Fixture".into(), false)
        .await
        .unwrap();
    let failed = tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let snapshot = backend.snapshot().await.unwrap();
            if let Some(row) = snapshot
                .recordings
                .iter()
                .find(|row| row.id == job && row.status == "failed")
            {
                break row.clone();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("source failure should stop at retry limit");
    assert_eq!(failed.retries, 1);
    assert!(failed.error.unwrap().contains("上限"));
    tokio::time::timeout(Duration::from_secs(8), backend.close())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unready_source_is_closed_when_failed_job_is_removed_and_backend_shuts_down() {
    const ID: &str = "cccccccccccccccccccccccccccccccc";
    let dir = tempfile::tempdir().unwrap();
    let backend = DesktopBackend::open(dir.path().join("desktop.json"))
        .await
        .unwrap();
    let tracker = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tracker_addr = tracker.local_addr().unwrap().to_string();
    let tracker_task = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), tracker.accept())
            .await
            .unwrap()
            .unwrap();
        let mut req = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                return;
            }
            req.extend_from_slice(&buf[..n]);
            if req.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        socket
            .write_all(b"HTTP/1.0 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    });
    let directory = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/index.txt", directory.local_addr().unwrap());
    let body = format!(
        "Retained<>{ID}<>{tracker_addr}<>http://example.com<>test<>fixture<>-1<>-1<>128<>RAW<>Artist<>Album<>Title<>http://example.com<>test<>00:01<>click<>comment<>free<>\n"
    );
    let listing = tokio::spawn(async move {
        let (mut socket, _) = directory.accept().await.unwrap();
        let mut req = [0u8; 1024];
        let _ = socket.read(&mut req).await.unwrap();
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(reply.as_bytes()).await.unwrap();
    });
    let yp = YellowPageEntry {
        name: "Fixture".into(),
        channels_url: url,
    };
    backend.apply_yellow_pages(vec![yp.clone()]).await.unwrap();
    backend.get_yp_channels(true).await.unwrap();
    listing.await.unwrap();
    let mut desktop = backend.snapshot().await.unwrap().desktop;
    desktop.recording.retry_count = 0;
    desktop.recording.retry_interval_secs = 1;
    desktop.recording.disconnect_on_retry_end = false;
    desktop.recording.disconnect_every_retries = None;
    backend
        .apply_desktop_preferences(backend.snapshot().await.unwrap().limits, vec![yp], desktop)
        .await
        .unwrap();
    let job = backend
        .enqueue_recording(ID.into(), "Fixture".into(), false)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let rows = backend.snapshot().await.unwrap().recordings;
            if rows.iter().any(|r| r.id == job && r.status == "failed") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("an unready relay should end at zero retries");
    tracker_task.await.unwrap();
    backend.control_recording(&job, "remove").await.unwrap();
    tokio::time::timeout(Duration::from_secs(8), backend.close())
        .await
        .unwrap()
        .unwrap();
}
