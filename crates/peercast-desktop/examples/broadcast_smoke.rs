use peercast_desktop::{backend::DesktopBackend, broadcast::BroadcastRequest};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() < 3 {
        return Err("usage: broadcast_smoke <test-config.json> <local-source-url> [--sp]".into());
    }
    let public = args.iter().any(|v| v == "--sp");
    if args.iter().any(|v| v == "--source-only") {
        let _ = tokio::time::timeout(Duration::from_secs(30), silent_flv_source()).await;
        return Ok(());
    }
    let silent = args[2] == "silence";
    let mut producer = None;
    let backend = if args.iter().any(|v| v == "--native") {
        DesktopBackend::open(args[1].clone().into()).await?
    } else {
        DesktopBackend::open_external(args[1].clone().into(), "http://127.0.0.1:7144").await?
    };
    let result = async {
        let snapshot = backend
            .start_broadcast(BroadcastRequest {
                name: "Peercast RT 配信テスト（無音・短時間）".into(),
                genre: "test".into(),
                description: "Peercast RT 1.0.1 / 自作の無音FLVで接続確認".into(),
                comment: "短時間のテストです。確認後に配信・掲載を停止します。".into(),
                source_mode: if silent { "push" } else { "pull" }.into(),
                source_uri: if silent {
                    String::new()
                } else {
                    args[2].clone()
                },
                ingest_port: 18081,
                bitrate: 200,
                announce_sp: public,
                ..Default::default()
            })
            .await?;
        if silent {
            producer = Some(tokio::spawn(silent_flv_source()));
        }
        let id = snapshot
            .broadcasts
            .first()
            .ok_or("配信開始結果がありません")?
            .channel_id
            .clone();
        println!("started={id} public_sp={public}");
        let stream_url = snapshot.broadcasts[0].stream_url.clone();
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        let mut media = Vec::new();
        let mut listed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        for attempt in 0..30 {
            if tokio::time::Instant::now() > deadline {
                break;
            }
            let snapshot = backend.snapshot().await?;
            let broadcast = snapshot
                .broadcasts
                .iter()
                .find(|row| row.channel_id == id)
                .ok_or("配信状態がありません")?;
            if attempt % 5 == 0 {
                println!(
                    "state={} announcement={}",
                    broadcast.status, broadcast.announcement_status
                );
            }
            if media.is_empty() {
                if let Ok(mut response) = client.get(&stream_url).send().await {
                    if response.status().is_success() {
                        while media.len() < 128 * 1024 {
                            match response.chunk().await {
                                Ok(Some(bytes)) => media.extend_from_slice(&bytes),
                                _ => break,
                            }
                        }
                    }
                }
            }
            if public {
                if let Ok(response) = client
                    .get("http://bayonet.ddo.jp/sp/index.txt")
                    .send()
                    .await
                {
                    if let Ok(text) = response.text().await {
                        listed |= text.to_ascii_lowercase().contains(&id.to_ascii_lowercase());
                    }
                }
            }
            if media.starts_with(b"FLV") && (!public || listed) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        if !media.is_empty() {
            let output = std::path::Path::new(&args[1])
                .parent()
                .unwrap()
                .join("csharp-broadcast-capture.flv");
            let mut file = tokio::fs::File::create(output)
                .await
                .map_err(|e| e.to_string())?;
            file.write_all(&media).await.map_err(|e| e.to_string())?;
            file.flush().await.map_err(|e| e.to_string())?;
        }
        println!(
            "media_bytes={} flv_header={} sp_listed={listed}",
            media.len(),
            media.starts_with(b"FLV")
        );
        if let Some(task) = producer.take() {
            task.abort();
            let _ = task.await;
        }
        backend.stop_broadcast(&id).await?;
        println!("broadcast_stopped=true");
        if public {
            let mut remains = true;
            for _ in 0..15 {
                if let Ok(response) = client
                    .get("http://bayonet.ddo.jp/sp/index.txt")
                    .send()
                    .await
                {
                    if let Ok(text) = response.text().await {
                        remains = text.to_ascii_lowercase().contains(&id.to_ascii_lowercase());
                        if !remains {
                            break;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            println!("sp_listing_removed={}", !remains);
        }
        if !media.starts_with(b"FLV") {
            Err("配信メディアの受信は未確認です".into())
        } else if public && !listed {
            Err("SPへの掲載は未確認です".into())
        } else {
            Ok(())
        }
    }
    .await;
    if let Some(task) = producer {
        task.abort();
        let _ = task.await;
    }
    let closed = backend.close().await;
    result.and(closed)
}

async fn silent_flv_source() -> std::io::Result<()> {
    let mut socket = loop {
        match tokio::net::TcpStream::connect("127.0.0.1:18081").await {
            Ok(socket) => break socket,
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    };
    socket.write_all(b"POST / HTTP/1.1\r\nHost: 127.0.0.1:18081\r\nContent-Type: video/x-flv\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await?;
    async fn chunk(socket: &mut tokio::net::TcpStream, bytes: &[u8]) -> std::io::Result<()> {
        socket
            .write_all(format!("{:X}\r\n", bytes.len()).as_bytes())
            .await?;
        socket.write_all(bytes).await?;
        socket.write_all(b"\r\n").await
    }
    chunk(&mut socket, b"FLV\x01\x04\x00\x00\x00\x09\x00\x00\x00\x00").await?;
    let mut timestamp = 0u32;
    let mut interval = tokio::time::interval(Duration::from_millis(40));
    loop {
        interval.tick().await;
        // PCM little-endian, 11025Hz, 16-bit mono; 441 silent samples per 40ms.
        let payload_len = 883u32;
        let mut tag = vec![
            8,
            0,
            3,
            115,
            (timestamp >> 16) as u8,
            (timestamp >> 8) as u8,
            timestamp as u8,
            (timestamp >> 24) as u8,
            0,
            0,
            0,
            0x36,
        ];
        tag.resize(11 + payload_len as usize, 0);
        tag.extend_from_slice(&(11 + payload_len).to_be_bytes());
        chunk(&mut socket, &tag).await?;
        timestamp += 40;
    }
}
