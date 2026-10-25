use bytes::{Bytes, BytesMut};
use peercast_core::{
    cache::HeaderSnapshot,
    content::{Content, ContinuationFlags},
};
use peercast_format::safestart::{SafeStartConfig, SafeStartMode};
use peercast_http::{
    ingest::read_chunked_stream, router::HttpRequest,
    streamer::stream_media_to_client_with_write_timeout,
};
use std::{
    hint::black_box,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{io::AsyncWrite, sync::mpsc};

#[derive(Default)]
struct Counter {
    bytes: u64,
    digest: u64,
}
impl Counter {
    fn record(&mut self, data: &[u8]) {
        let data = black_box(data);
        self.bytes += data.len() as u64;
        self.digest = self.digest.rotate_left(7) ^ data.len() as u64;
        if let (Some(first), Some(last)) = (data.first(), data.last()) {
            self.digest ^= u64::from(*first) << 8 | u64::from(*last);
        }
    }
}
impl AsyncWrite for Counter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.record(data);
        Poll::Ready(Ok(data.len()))
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        slices: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        let mut n = 0;
        for data in slices {
            self.record(data);
            n += data.len();
        }
        Poll::Ready(Ok(n))
    }
    fn is_write_vectored(&self) -> bool {
        true
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

async fn stream_sample(frames: usize, size: usize) -> (u128, u64, u64) {
    let request =
        HttpRequest::parse(b"GET /stream/0123456789abcdef0123456789abcdef HTTP/1.1\r\n\r\n")
            .unwrap();
    let header = HeaderSnapshot::new(1, 0, "RAW", Bytes::new());
    let bytes = Bytes::from(vec![0x42; size + 16]);
    let (tx, rx) = mpsc::channel(512);
    let producer = tokio::spawn(async move {
        let mut pos = 0;
        for i in 0..frames {
            let data = bytes.slice(..size + (i & 15));
            pos += data.len() as i64;
            tx.send(Content::new(
                1,
                Duration::ZERO,
                pos,
                data,
                ContinuationFlags::NONE,
            ))
            .await
            .unwrap();
        }
    });
    let mut writer = Counter::default();
    let started = Instant::now();
    stream_media_to_client_with_write_timeout(
        &mut writer,
        &request,
        &header,
        rx,
        SafeStartConfig {
            mode: SafeStartMode::Raw,
            ..SafeStartConfig::default()
        },
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    producer.await.unwrap();
    (started.elapsed().as_nanos(), writer.bytes, writer.digest)
}

async fn ingest_sample(chunks: usize, extension: usize) -> (u128, u64, u64) {
    let mut wire = BytesMut::new();
    for _ in 0..chunks {
        wire.extend_from_slice(b"40");
        if extension != 0 {
            wire.extend_from_slice(b";name=");
            wire.extend_from_slice(&vec![b'x'; extension]);
        }
        wire.extend_from_slice(b"\r\n");
        wire.extend_from_slice(&[0x42; 64]);
        wire.extend_from_slice(b"\r\n");
    }
    wire.extend_from_slice(b"0\r\nX-Check: yes\r\n\r\n");
    let (tx, mut rx) = mpsc::channel(512);
    let consumer = tokio::spawn(async move {
        let mut bytes = 0;
        let mut digest = 0u64;
        while let Some(data) = rx.recv().await {
            let data: Bytes = data;
            bytes += data.len() as u64;
            digest = digest.rotate_left(7) ^ u64::from(data[0]);
        }
        (bytes, digest)
    });
    let started = Instant::now();
    read_chunked_stream(&mut wire.as_ref(), Bytes::new(), tx, 1024)
        .await
        .unwrap();
    let (bytes, digest) = consumer.await.unwrap();
    (started.elapsed().as_nanos(), bytes, digest)
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let count = std::env::args()
        .nth(1)
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(200000);
    let repetitions = 7;
    let _ = stream_sample(10000, 64).await;
    let mut rows = Vec::new();
    for size in [64, 16384] {
        let mut runs = Vec::new();
        let mut wire = None;
        for _ in 0..repetitions {
            let (elapsed, bytes, digest) = stream_sample(count, size).await;
            if let Some(expected) = wire {
                assert_eq!((bytes, digest), expected);
            }
            wire = Some((bytes, digest));
            runs.push(elapsed);
        }
        runs.sort_unstable();
        let (bytes, digest) = wire.unwrap();
        rows.push(serde_json::json!({"case":format!("stream-{size}"),"units":count,"medianNs":runs[repetitions/2],"samplesNs":runs,"bytes":bytes,"digest":digest}));
    }
    for extension in [0, 2048] {
        let chunks = count / 10;
        let mut runs = Vec::new();
        let mut wire = None;
        for _ in 0..repetitions {
            let (elapsed, bytes, digest) = ingest_sample(chunks, extension).await;
            if let Some(expected) = wire {
                assert_eq!((bytes, digest), expected);
            }
            wire = Some((bytes, digest));
            runs.push(elapsed);
        }
        runs.sort_unstable();
        let (bytes, digest) = wire.unwrap();
        rows.push(serde_json::json!({"case":format!("ingest-extension-{extension}"),"units":chunks,"medianNs":runs[repetitions/2],"samplesNs":runs,"bytes":bytes,"digest":digest}));
    }
    println!("{}", serde_json::to_string_pretty(&rows).unwrap());
}
