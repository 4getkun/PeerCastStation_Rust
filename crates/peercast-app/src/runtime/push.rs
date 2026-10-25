use std::io;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use peercast_core::cache::HeaderSnapshot;
use peercast_core::content::ContinuationFlags;
use peercast_format::{
    parse_ebml_element, AsfDemuxOutput, AsfDemuxer, AsfTransportProfile, FlvDemuxOutput,
    FlvDemuxer, MkvDemuxOutput, MkvDemuxer, Mp3DemuxOutput, Mp3Demuxer, OggDemuxOutput, OggDemuxer,
    TsDemuxOutput, TsDemuxer, ID_BLOCK_GROUP, ID_SIMPLE_BLOCK,
};
use peercast_http::ingest::{read_chunked_stream, HttpPushIngestListener};
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, watch};

use super::{AppRuntime, RuntimeChannel};
use crate::{LogLevel, APPLICATION_NAME};

const TCP_READ_BUFFER_BYTES: usize = 8192;
const DEMUX_FEED_SLICE_BYTES: usize = 8192;
const CHUNK_FORWARD_QUEUE_CAPACITY: usize = 1;
const ACCEPT_ERROR_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

enum DemuxOutput {
    Header(HeaderSnapshot),
    Content {
        pos: i64,
        bytes: Bytes,
        flags: ContinuationFlags,
    },
}

enum StreamDemuxer {
    Raw {
        position: i64,
        initialized: bool,
    },
    Flv(FlvDemuxer),
    Ts(TsDemuxer),
    Asf(AsfDemuxer),
    Ogg {
        demuxer: OggDemuxer,
        has_audio: bool,
        has_video: bool,
    },
    Mp3(Mp3Demuxer),
    Mkv {
        demuxer: MkvDemuxer,
        has_audio: bool,
        has_video: bool,
    },
}

impl StreamDemuxer {
    fn new(container_type: &str) -> io::Result<Self> {
        match container_type.to_ascii_uppercase().as_str() {
            "RAW" => Ok(Self::Raw {
                position: 0,
                initialized: false,
            }),
            "FLV" => Ok(Self::Flv(FlvDemuxer::default())),
            "TS" | "MPEG-TS" => Ok(Self::Ts(TsDemuxer::default())),
            "ASF" | "WMV" | "WMA" => Ok(Self::Asf(AsfDemuxer::new(AsfTransportProfile::RawAsf))),
            "MMS" => Ok(Self::Asf(AsfDemuxer::new(AsfTransportProfile::MmsFramed))),
            "OGG" => Ok(Self::Ogg {
                demuxer: OggDemuxer::default(),
                has_audio: false,
                has_video: false,
            }),
            "MP3" => Ok(Self::Mp3(Mp3Demuxer::default())),
            "MKV" | "WEBM" => Ok(Self::Mkv {
                demuxer: MkvDemuxer::default(),
                has_audio: false,
                has_video: false,
            }),
            other => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported HTTP push container type: {other}"),
            )),
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> io::Result<Vec<DemuxOutput>> {
        let mut outputs = Vec::new();
        match self {
            Self::Raw {
                position,
                initialized,
            } => {
                if !*initialized {
                    outputs.push(DemuxOutput::Header(HeaderSnapshot::new(
                        0,
                        0,
                        "RAW",
                        Bytes::new(),
                    )));
                    *initialized = true;
                }
                if !bytes.is_empty() {
                    let len = bytes.len();
                    outputs.push(DemuxOutput::Content {
                        pos: *position,
                        bytes: Bytes::copy_from_slice(bytes),
                        flags: ContinuationFlags::NONE,
                    });
                    *position = position.saturating_add(len as i64);
                }
            }
            Self::Flv(demuxer) => {
                demuxer.feed(bytes);
                while let Some(output) = demuxer.next_output().map_err(parse_error)? {
                    match output {
                        FlvDemuxOutput::HeaderChanged(header) => {
                            outputs.push(DemuxOutput::Header(header));
                        }
                        FlvDemuxOutput::ContentTag {
                            tag, is_keyframe, ..
                        } => {
                            let data = tag.to_wire_bytes(None);
                            let pos = demuxer.position().saturating_sub(data.len() as i64);
                            let flags = if tag.is_audio() {
                                ContinuationFlags::AUDIO_FRAME
                            } else if tag.is_video() && !is_keyframe {
                                ContinuationFlags::INTER_FRAME
                            } else {
                                ContinuationFlags::NONE
                            };
                            outputs.push(DemuxOutput::Content {
                                pos,
                                bytes: data,
                                flags,
                            });
                        }
                    }
                }
            }
            Self::Ts(demuxer) => {
                demuxer.feed(bytes);
                while let Some(output) = demuxer.next_output().map_err(parse_error)? {
                    match output {
                        TsDemuxOutput::HeaderChanged(header) => {
                            outputs.push(DemuxOutput::Header(header));
                        }
                        TsDemuxOutput::MediaPacket { data, .. } => {
                            let pos = demuxer.position().saturating_sub(data.len() as i64);
                            outputs.push(DemuxOutput::Content {
                                pos,
                                bytes: data,
                                // TsDemuxOutput does not distinguish audio PIDs from non-keyframe video.
                                flags: ContinuationFlags::NONE,
                            });
                        }
                    }
                }
            }
            Self::Asf(demuxer) => {
                demuxer.feed(bytes).map_err(parse_error)?;
                while let Some(output) = demuxer.next_output().map_err(parse_error)? {
                    match output {
                        AsfDemuxOutput::HeaderSnapshotReady { snapshot, .. } => {
                            outputs.push(DemuxOutput::Header(snapshot));
                        }
                        AsfDemuxOutput::ContentPacket {
                            packet, position, ..
                        } => outputs.push(DemuxOutput::Content {
                            pos: position.min(i64::MAX as u64) as i64,
                            bytes: packet,
                            flags: ContinuationFlags::NONE,
                        }),
                        AsfDemuxOutput::EndOfStream => {}
                    }
                }
            }
            Self::Ogg {
                demuxer,
                has_audio,
                has_video,
            } => {
                demuxer.feed(bytes).map_err(parse_error)?;
                while let Some(output) = demuxer.next_output().map_err(parse_error)? {
                    match output {
                        OggDemuxOutput::HeaderSnapshotReady {
                            snapshot, metadata, ..
                        } => {
                            *has_audio = metadata.has_audio;
                            *has_video = metadata.has_video;
                            outputs.push(DemuxOutput::Header(snapshot));
                        }
                        OggDemuxOutput::ContentPage {
                            page,
                            position,
                            is_keyframe,
                            ..
                        } => {
                            let flags = if *has_audio && !*has_video {
                                ContinuationFlags::AUDIO_FRAME
                            } else if *has_video && !is_keyframe {
                                ContinuationFlags::INTER_FRAME
                            } else {
                                ContinuationFlags::NONE
                            };
                            outputs.push(DemuxOutput::Content {
                                pos: position.min(i64::MAX as u64) as i64,
                                bytes: page,
                                flags,
                            });
                        }
                        OggDemuxOutput::EndOfStream => {}
                    }
                }
            }
            Self::Mp3(demuxer) => {
                demuxer.feed(bytes).map_err(parse_error)?;
                while let Some(output) = demuxer.next_output().map_err(parse_error)? {
                    match output {
                        Mp3DemuxOutput::HeaderSnapshotReady { snapshot, .. } => {
                            outputs.push(DemuxOutput::Header(snapshot));
                        }
                        Mp3DemuxOutput::ContentFrame {
                            frame, position, ..
                        } => {
                            outputs.push(DemuxOutput::Content {
                                pos: position.min(i64::MAX as u64) as i64,
                                bytes: frame,
                                flags: ContinuationFlags::AUDIO_FRAME,
                            });
                        }
                        Mp3DemuxOutput::EndOfStream => {}
                    }
                }
            }
            Self::Mkv {
                demuxer,
                has_audio,
                has_video,
            } => {
                demuxer.feed(bytes).map_err(parse_error)?;
                while let Some(output) = demuxer.next_output().map_err(parse_error)? {
                    match output {
                        MkvDemuxOutput::HeaderSnapshotReady {
                            snapshot, metadata, ..
                        } => {
                            *has_audio = metadata.has_audio;
                            *has_video = metadata.has_video;
                            outputs.push(DemuxOutput::Header(snapshot));
                        }
                        MkvDemuxOutput::ContentPacket {
                            packet,
                            position,
                            is_keyframe,
                            ..
                        } => {
                            let element_id =
                                parse_ebml_element(&packet).ok().map(|(_, id, _, _)| id);
                            let is_block =
                                matches!(element_id, Some(ID_SIMPLE_BLOCK | ID_BLOCK_GROUP));
                            let flags = if *has_audio && !*has_video && is_block {
                                ContinuationFlags::AUDIO_FRAME
                            } else if *has_video && !is_keyframe && is_block {
                                ContinuationFlags::INTER_FRAME
                            } else {
                                ContinuationFlags::NONE
                            };
                            outputs.push(DemuxOutput::Content {
                                pos: position.min(i64::MAX as u64) as i64,
                                bytes: packet,
                                flags,
                            });
                        }
                        MkvDemuxOutput::EndOfStream => {}
                    }
                }
            }
        }
        Ok(outputs)
    }

    fn signal_eof(&mut self) -> io::Result<()> {
        match self {
            Self::Ogg { demuxer, .. } => {
                let _ = demuxer.signal_eof().map_err(parse_error)?;
            }
            Self::Mp3(demuxer) => {
                let _ = demuxer.signal_eof().map_err(parse_error)?;
            }
            Self::Mkv { demuxer, .. } => {
                let _ = demuxer.signal_eof().map_err(parse_error)?;
            }
            Self::Raw { .. } | Self::Flv(_) | Self::Ts(_) | Self::Asf(_) => {}
        }
        Ok(())
    }
}

fn parse_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

pub(super) async fn run(
    runtime: Arc<AppRuntime>,
    channel: Arc<RuntimeChannel>,
    listener: HttpPushIngestListener,
) {
    let mut shutdown = runtime.shutdown_tx.subscribe();
    loop {
        if *shutdown.borrow() {
            return;
        }
        let accepted = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
            accepted = listener.accept_connection() => accepted,
        };

        match accepted {
            Ok((socket, handshake, prefix)) => {
                channel.begin_epoch().await;
                runtime.channel_changed(&channel).await;

                let demuxer = StreamDemuxer::new(&channel.config.container_type);
                let result = match demuxer {
                    Ok(demuxer) => {
                        ingest_connection(
                            socket,
                            handshake.is_chunked,
                            prefix,
                            demuxer,
                            &runtime,
                            &channel,
                            &mut shutdown,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };

                channel.offline().await;
                runtime.channel_changed(&channel).await;
                if let Err(error) = result {
                    runtime.log.log(
                        LogLevel::Warn,
                        APPLICATION_NAME,
                        &format!(
                            "HTTP push source for {} ended: {error}",
                            channel.config.name
                        ),
                    );
                }
                if *shutdown.borrow() {
                    return;
                }
            }
            Err(error) => {
                runtime.log.log(
                    LogLevel::Warn,
                    APPLICATION_NAME,
                    &format!(
                        "HTTP push accept failed for {}: {error}",
                        channel.config.name
                    ),
                );
                tokio::select! {
                    _ = shutdown.changed() => return,
                    _ = tokio::time::sleep(ACCEPT_ERROR_RETRY_DELAY) => {}
                }
            }
        }
    }
}

async fn ingest_connection(
    socket: tokio::net::TcpStream,
    is_chunked: bool,
    prefix: Bytes,
    mut demuxer: StreamDemuxer,
    runtime: &Arc<AppRuntime>,
    channel: &Arc<RuntimeChannel>,
    shutdown: &mut watch::Receiver<bool>,
) -> io::Result<()> {
    let lease = channel
        .connections
        .track("ingest", socket.peer_addr().ok(), "connected");
    let (mut reader, _writer) = socket.into_split();
    if is_chunked {
        let (sender, mut receiver) = mpsc::channel(CHUNK_FORWARD_QUEUE_CAPACITY);
        let max_chunk_size = runtime.config.runtime.ingest.max_chunk_bytes;
        let producer = runtime.tracker.spawn(async move {
            read_chunked_stream(&mut reader, prefix, sender, max_chunk_size).await
        });

        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        producer.abort();
                        let _ = producer.await;
                        return Ok(());
                    }
                }
                chunk = receiver.recv() => match chunk {
                    Some(bytes) => {
                        if let Err(error) = publish_outputs(&mut demuxer, &bytes, runtime, channel, &lease).await {
                            producer.abort();
                            let _ = producer.await;
                            return Err(error);
                        }
                    }
                    None => break,
                }
            }
        }
        let completed = producer
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        completed.map_err(io::Error::other)?;
        demuxer.signal_eof()?;
        Ok(())
    } else {
        if !prefix.is_empty() {
            publish_outputs(&mut demuxer, &prefix, runtime, channel, &lease).await?;
        }
        let mut buffer = BytesMut::with_capacity(TCP_READ_BUFFER_BYTES);
        buffer.resize(TCP_READ_BUFFER_BYTES, 0);
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
                result = reader.read(&mut buffer) => match result {
                    Ok(0) => {
                        demuxer.signal_eof()?;
                        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "push source closed"));
                    }
                    Ok(read) => publish_outputs(&mut demuxer, &buffer[..read], runtime, channel, &lease).await?,
                    Err(error) => return Err(error),
                }
            }
        }
    }
}

async fn publish_outputs(
    demuxer: &mut StreamDemuxer,
    bytes: &[u8],
    runtime: &Arc<AppRuntime>,
    channel: &Arc<RuntimeChannel>,
    lease: &super::diagnostics::ConnectionLease,
) -> io::Result<()> {
    lease.received(bytes.len());
    for input in bytes.chunks(DEMUX_FEED_SLICE_BYTES) {
        for output in demuxer.feed(input)? {
            match output {
                DemuxOutput::Header(header) => {
                    channel.set_header(header.data, header.pos).await?;
                    runtime.channel_changed(channel).await;
                }
                DemuxOutput::Content { pos, bytes, flags } => {
                    let was_available = channel.snapshot().await.available;
                    channel.publish(pos, bytes, flags).await;
                    if !was_available {
                        runtime.channel_changed(channel).await;
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BufMut;

    fn make_flv_tag(tag_type: u8, timestamp: u32, body: &[u8]) -> Bytes {
        let mut tag = BytesMut::with_capacity(11 + body.len() + 4);
        tag.put_u8(tag_type);
        tag.put_u8((body.len() >> 16) as u8);
        tag.put_u8((body.len() >> 8) as u8);
        tag.put_u8(body.len() as u8);
        tag.put_u8((timestamp >> 16) as u8);
        tag.put_u8((timestamp >> 8) as u8);
        tag.put_u8(timestamp as u8);
        tag.put_u8((timestamp >> 24) as u8);
        tag.put_u8(0);
        tag.put_u8(0);
        tag.put_u8(0);
        tag.extend_from_slice(body);
        tag.put_u32((11 + body.len()) as u32);
        tag.freeze()
    }

    #[test]
    fn flv_split_feed_reconstructs_header_and_single_wire_tag() {
        let prelude = Bytes::from_static(b"FLV\x01\x01\x00\x00\x00\x09\x00\x00\x00\x00");
        let sequence_header = make_flv_tag(0x09, 0, &[0x17, 0x00, 0x00, 0x00, 0x00]);
        let tag = make_flv_tag(0x09, 100, &[0x17, 0x01, 0x00, 0x00, 0x65]);
        let mut input = BytesMut::new();
        input.extend_from_slice(&prelude);
        input.extend_from_slice(&sequence_header);
        input.extend_from_slice(&tag);

        let mut demuxer = StreamDemuxer::new("FLV").unwrap();
        let mut outputs = Vec::new();
        for chunk in input.chunks(5) {
            outputs.extend(demuxer.feed(chunk).unwrap());
        }

        let mut headers = Vec::new();
        let mut contents = Vec::new();
        for output in outputs {
            match output {
                DemuxOutput::Header(snapshot) => headers.push(snapshot.data),
                DemuxOutput::Content { pos, bytes, flags } => contents.push((pos, bytes, flags)),
            }
        }
        let mut expected_header = BytesMut::new();
        expected_header.extend_from_slice(&prelude);
        expected_header.extend_from_slice(&sequence_header);
        let expected_header = expected_header.freeze();
        assert_eq!(headers.last(), Some(&expected_header));
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].0, 0);
        assert_eq!(contents[0].1, tag);
        assert_eq!(contents[0].2, ContinuationFlags::NONE);
        let mut reconstructed = BytesMut::new();
        reconstructed.extend_from_slice(&expected_header);
        reconstructed.extend_from_slice(&contents[0].1);
        assert_eq!(reconstructed, input);
    }

    fn make_mp3_frame(padded: bool) -> Bytes {
        let mut frame = BytesMut::with_capacity(if padded { 418 } else { 417 });
        frame.extend_from_slice(&[0xFF, 0xFB, if padded { 0x92 } else { 0x90 }, 0x00]);
        frame.extend_from_slice(&vec![0xAA; if padded { 414 } else { 413 }]);
        frame.freeze()
    }

    #[test]
    fn mp3_split_feed_reconstructs_first_frame_header_and_following_wire_frame() {
        let first = make_mp3_frame(false);
        let second = make_mp3_frame(true);
        let mut input = BytesMut::new();
        input.extend_from_slice(&first);
        input.extend_from_slice(&second);

        let mut demuxer = StreamDemuxer::new("MP3").unwrap();
        let mut outputs = Vec::new();
        for chunk in input.chunks(31) {
            outputs.extend(demuxer.feed(chunk).unwrap());
        }

        let mut headers = Vec::new();
        let mut contents = Vec::new();
        for output in outputs {
            match output {
                DemuxOutput::Header(snapshot) => headers.push(snapshot.data),
                DemuxOutput::Content { pos, bytes, flags } => contents.push((pos, bytes, flags)),
            }
        }
        assert_eq!(headers, vec![first.clone()]);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].0, first.len() as i64);
        assert_eq!(contents[0].1, second);
        assert_eq!(contents[0].2, ContinuationFlags::AUDIO_FRAME);
        assert_eq!(headers[0].len() + contents[0].1.len(), input.len());
    }
}
