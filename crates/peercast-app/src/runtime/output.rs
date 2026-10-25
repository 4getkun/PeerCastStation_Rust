use super::*;
use bytes::BytesMut;
use peercast_core::atom::AtomBudget;
use peercast_core::id4::ID4;
use peercast_pcp::packet::{ChannelPacket, PacketPayload};
use peercast_pcp::relay_client::RelayNetworkProfile;
use peercast_pcp::relay_server::{RelayServerError, RelayServerEvent, RelayServerStateMachine};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::net::TcpStream;

impl AppRuntime {
    /// Answers the standalone PCP reachability handshake on the shared listener.
    pub async fn serve_pong(&self, stream: &mut TcpStream, initial: &[u8]) -> io::Result<()> {
        use peercast_pcp::probe::{ProbeCodec, ProbePeerProfile};

        let limit = self.config.server.max_request_buffer_bytes;
        let mut pending = BytesMut::from(initial);
        let mut received = initial.len();
        let budget = AtomBudget {
            max_total_bytes: limit.min(AtomBudget::default().max_total_bytes),
            max_leaf_len: limit.min(AtomBudget::default().max_leaf_len),
            ..Default::default()
        };
        let handshake = async {
            let preamble =
                read_probe_atom(stream, &mut pending, &budget, &mut received, limit).await?;
            if preamble.name() != ID4::PCP_CONNECT {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected PCP preamble",
                ));
            }
            let version = preamble.get_u32_le().map_err(io::Error::other)?;
            ProbeCodec::validate_pong_preamble(version, ProbePeerProfile::PeerCastStation)
                .map_err(io::Error::other)?;
            let helo = read_probe_atom(stream, &mut pending, &budget, &mut received, limit).await?;
            let identified = helo.name() == ID4::PCP_HELO
                && helo
                    .find_child(ID4::HELO_SESSIONID)
                    .and_then(Atom::leaf_bytes)
                    .is_some_and(|sid| sid.len() == 16);
            if !identified {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected identified PCP HELO",
                ));
            }
            let response = ProbeCodec::build_pong_oleh(self.session_id, APPLICATION_NAME)
                .map_err(io::Error::other)?;
            stream.write_all(&response).await?;
            self.log.log(
                LogLevel::Debug,
                "pcp-pong",
                &format!("standalone pong sent session_id={}", self.session_id),
            );
            Ok(())
        };
        tokio::time::timeout(
            Duration::from_millis(self.config.server.request_timeout_ms.min(3000)),
            handshake,
        )
        .await
        .map_err(io::Error::other)?
    }

    /// Serves a PCP relay using the same configured header, replay ring and live publisher as HTTP.
    pub async fn serve_relay(
        &self,
        stream: &mut TcpStream,
        peer: SocketAddr,
        request: &[u8],
    ) -> io::Result<()> {
        let observed_ip = match peer.ip() {
            IpAddr::V6(ip) => ip
                .to_ipv4_mapped()
                .map(IpAddr::V4)
                .unwrap_or(IpAddr::V6(ip)),
            ip => ip,
        };
        let profile = if observed_ip.is_ipv6() {
            RelayNetworkProfile::IPv6
        } else {
            RelayNetworkProfile::IPv4
        };
        let mut machine = RelayServerStateMachine::new(self.session_id, profile, Some(observed_ip))
            .with_server_agent(APPLICATION_NAME)
            .with_max_candidate_hosts(self.config.runtime.relay.max_candidate_hosts);
        let mut input = Bytes::copy_from_slice(request);
        let event = machine.feed_bytes(&mut input).map_err(io::Error::other)?;
        let Some(RelayServerEvent::RequestReceived(request)) = event else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected relay HTTP request",
            ));
        };
        self.log.log(
            LogLevel::Debug,
            "pcp-relay",
            &format!(
                "relay request received channel_id={} peer={peer}",
                request.channel_id
            ),
        );
        if let Err(error) = machine.validate_request_family() {
            self.log.log(
                LogLevel::Warn,
                "pcp-relay",
                &format!(
                    "relay request rejected channel_id={} peer={peer}: {error}",
                    request.channel_id
                ),
            );
            let bytes = match error {
                RelayServerError::NetworkFamilyMismatch { .. } => machine.reject_not_found(),
                _ => machine.reject_bad_request(&error.to_string()),
            };
            return stream.write_all(&bytes).await;
        }
        let Some(channel) = self.channels.get(&request.channel_id) else {
            self.log.log(
                LogLevel::Warn,
                "pcp-relay",
                &format!(
                    "unknown relay channel channel_id={} peer={peer}",
                    request.channel_id
                ),
            );
            return stream.write_all(&machine.reject_not_found()).await;
        };
        let session = match channel.subscribe(Arc::clone(&self.relay_slots), true).await {
            Ok(session) => Some(session),
            Err(error) if error.kind() == io::ErrorKind::NotConnected => {
                self.log.log(
                    LogLevel::Warn,
                    "pcp-relay",
                    &format!(
                        "relay capacity full channel_id={} peer={peer}",
                        request.channel_id
                    ),
                );
                return stream.write_all(&machine.reject_not_found()).await;
            }
            Err(error) => {
                self.log.log(
                    LogLevel::Warn,
                    "pcp-relay",
                    &format!(
                        "relay capacity full channel_id={} peer={peer}: {error}",
                        request.channel_id
                    ),
                );
                None
            }
        };
        let prelude = if session.is_some() {
            machine.accept()
        } else {
            machine.reject_full(channel.candidates.lock().await.clone())
        }
        .map_err(io::Error::other)?;
        let timeout = Duration::from_millis(self.config.server.request_timeout_ms);
        write_with_deadline(stream, &prelude, timeout).await?;
        let mut pending = BytesMut::new();
        let handshake =
            tokio::time::timeout(timeout, next_event(stream, &mut pending, &mut machine))
                .await
                .map_err(io::Error::other)??;
        let (outbound, connection_lease) = match handshake {
            Some(RelayServerEvent::HandshakeCompleted {
                outbound_bytes,
                client_session_id,
                client_port,
                ..
            }) => {
                let connection_lease = session
                    .as_ref()
                    .map(|_| channel.connections.track("relay", Some(peer), "connected"));
                let probe_attempted = client_port.is_some_and(|port| port != 0);
                // Probe only the TCP-observed IP and the announced port; never accept an arbitrary target IP.
                let reachable_port = if let Some(port) = client_port.filter(|port| *port != 0) {
                    self.log.log(
                        LogLevel::Debug,
                        "reverse-probe",
                        &format!(
                            "probe started channel_id={} endpoint={observed_ip}:{port}",
                            channel.id
                        ),
                    );
                    match self.probe(observed_ip, port, client_session_id).await {
                        Ok(ProbeResult::Reachable { target_addr, .. }) => {
                            self.log.log(
                                LogLevel::Info,
                                "reverse-probe",
                                &format!(
                                    "probe verified channel_id={} endpoint={target_addr}",
                                    channel.id
                                ),
                            );
                            port
                        }
                        Ok(ProbeResult::SessionMismatch { target_addr, .. }) => {
                            self.log.log(
                                LogLevel::Warn,
                                "reverse-probe",
                                &format!(
                                    "probe session mismatch channel_id={} endpoint={target_addr}",
                                    channel.id,
                                ),
                            );
                            0
                        }
                        Ok(ProbeResult::Unreachable {
                            target_addr,
                            reason,
                        }) => {
                            self.log.log(
                                LogLevel::Warn,
                                "reverse-probe",
                                &format!(
                                    "probe unreachable channel_id={} endpoint={target_addr}: {reason}",
                                    channel.id
                                ),
                            );
                            0
                        }
                        Err(error) => {
                            self.log.log(
                                LogLevel::Warn,
                                "reverse-probe",
                                &format!(
                                    "probe failed channel_id={} endpoint={observed_ip}:{port}: {error}",
                                    channel.id
                                ),
                            );
                            0
                        }
                    }
                } else {
                    self.log.log(
                        LogLevel::Debug,
                        "reverse-probe",
                        &format!("probe skipped channel_id={} no port advertised", channel.id),
                    );
                    0
                };
                let port_open = if client_port == Some(0) {
                    Some(false)
                } else {
                    probe_attempted.then_some(reachable_port != 0)
                };
                if let Some(lease) = &connection_lease {
                    lease.connected(Some(client_session_id.to_string()), None, port_open);
                }
                (
                    with_verified_port(outbound_bytes, reachable_port)?,
                    connection_lease,
                )
            }
            Some(RelayServerEvent::FullHandshakeCompleted { outbound_bytes, .. }) => {
                return write_with_deadline(stream, &outbound_bytes, timeout).await;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "incomplete relay handshake",
                ))
            }
        };
        write_with_deadline(stream, &outbound, timeout).await?;
        let Some(session) = session else {
            return Ok(());
        };
        let _connection_lease = connection_lease;
        let Subscription {
            header,
            replay,
            mut live,
            _global,
            _channel,
        } = session;
        let _permits = (_global, _channel);
        let channel_info = Atom::new_parent(
            ID4::PCP_CHAN,
            vec![
                Atom::new_leaf(
                    ID4::CHAN_ID,
                    bytes::Bytes::copy_from_slice(channel.id.as_bytes()),
                ),
                Atom::new_parent(ID4::CHAN_INFO, {
                    let metadata = &channel.config.metadata;
                    let mut info = vec![Atom::new_string(ID4::INFO_NAME, &channel.config.name)];
                    if !metadata.genre.is_empty() {
                        info.push(Atom::new_string(ID4::INFO_GENRE, &metadata.genre));
                    }
                    if !metadata.description.is_empty() {
                        info.push(Atom::new_string(ID4::INFO_DESC, &metadata.description));
                    }
                    if !metadata.comment.is_empty() {
                        info.push(Atom::new_string(ID4::INFO_COMMENT, &metadata.comment));
                    }
                    if !metadata.contact_url.is_empty() {
                        info.push(Atom::new_string(ID4::INFO_URL, &metadata.contact_url));
                    }
                    if metadata.bitrate > 0 {
                        info.push(Atom::new_u32_le(ID4::INFO_BITR, metadata.bitrate));
                    }
                    info.push(Atom::new_string(
                        ID4::INFO_TYPE,
                        &channel.config.container_type,
                    ));
                    info
                }),
            ],
        );
        write_with_deadline(
            stream,
            &channel_info.to_bytes().map_err(io::Error::other)?,
            timeout,
        )
        .await?;
        let (mut reader, mut writer) = stream.split();
        let header_payload_bytes = header.data.len();
        send_packet(
            &mut writer,
            channel.id,
            header.pos,
            ContinuationFlags::NONE,
            PacketPayload::Header(header.data),
            timeout,
        )
        .await?;
        if let Some(lease) = &_connection_lease {
            lease.sent(header_payload_bytes);
        }
        let cursor = request.pos.unwrap_or(-1);
        for content in replay
            .into_iter()
            .filter(|content| header.pos >= cursor || content.pos >= cursor)
        {
            let payload_bytes = content.data.len();
            send_packet(
                &mut writer,
                channel.id,
                content.pos,
                content.cont_flags,
                PacketPayload::Data(content.data),
                timeout,
            )
            .await?;
            if let Some(lease) = &_connection_lease {
                lease.sent(payload_bytes);
            }
        }
        loop {
            tokio::select! {
                event = live.recv() => match event {
                    Ok(StreamEvent::Body(content)) => {
                        let payload_bytes = content.data.len();
                        send_packet(&mut writer, channel.id, content.pos, content.cont_flags, PacketPayload::Data(content.data), timeout).await?;
                        if let Some(lease) = &_connection_lease {
                            lease.sent(payload_bytes);
                        }
                    },
                    _ => return Ok(()),
                },
                event = next_event(&mut reader, &mut pending, &mut machine) => match event? {
                    Some(RelayServerEvent::Control(atom)) if atom.name() == ID4::PCP_BCST => { let _ = self.process_broadcast(&atom).await; },
                    Some(RelayServerEvent::Quit(_)) | None => return Ok(()),
                    _ => {},
                }
            }
        }
    }
}

async fn read_probe_atom<R: AsyncRead + Unpin>(
    stream: &mut R,
    pending: &mut BytesMut,
    budget: &AtomBudget,
    received: &mut usize,
    limit: usize,
) -> io::Result<Atom> {
    loop {
        let mut bytes = pending.split().freeze();
        let atom = Atom::decode(&mut bytes, budget).map_err(io::Error::other)?;
        *pending = bytes
            .try_into_mut()
            .unwrap_or_else(|bytes| BytesMut::from(bytes.as_ref()));
        if let Some(atom) = atom {
            return Ok(atom);
        }
        let remaining = limit.saturating_sub(*received);
        if remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PCP probe exceeds request limit",
            ));
        }
        let mut chunk = [0u8; 1024];
        let capacity = chunk.len().min(remaining);
        let n = stream.read(&mut chunk[..capacity]).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "incomplete PCP probe",
            ));
        }
        *received += n;
        pending.extend_from_slice(&chunk[..n]);
    }
}

async fn next_event<R: AsyncRead + Unpin>(
    reader: &mut R,
    pending: &mut BytesMut,
    machine: &mut RelayServerStateMachine,
) -> io::Result<Option<RelayServerEvent>> {
    let budget = AtomBudget::default().max_total_bytes;
    loop {
        let mut bytes = pending.split().freeze();
        let before = bytes.len();
        let event = machine.feed_bytes(&mut bytes).map_err(io::Error::other)?;
        let consumed = before - bytes.len();
        *pending = bytes
            .try_into_mut()
            .unwrap_or_else(|bytes| BytesMut::from(bytes.as_ref()));
        if event.is_some() {
            return Ok(event);
        }
        if consumed > 0 {
            continue;
        }
        let mut chunk = [0u8; 4096];
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        if pending.len().saturating_add(n) > budget {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "relay control budget exceeded",
            ));
        }
        pending.extend_from_slice(&chunk[..n]);
    }
}

fn with_verified_port(mut bytes: Bytes, port: u16) -> io::Result<Bytes> {
    let mut output = Vec::new();
    while let Some(atom) =
        Atom::decode(&mut bytes, &AtomBudget::default()).map_err(io::Error::other)?
    {
        let atom = if atom.name() == ID4::PCP_OLEH {
            let mut children: Vec<_> = atom
                .children()
                .unwrap_or_default()
                .iter()
                .filter(|child| child.name() != ID4::HELO_PORT)
                .cloned()
                .collect();
            children.push(Atom::new_u16_le(ID4::HELO_PORT, port));
            Atom::new_parent(ID4::PCP_OLEH, children)
        } else {
            atom
        };
        atom.encode(&mut output).map_err(io::Error::other)?;
    }
    Ok(Bytes::from(output))
}

async fn send_packet<W: AsyncWrite + Unpin>(
    writer: &mut W,
    channel_id: ChannelId,
    pos: i64,
    cont: ContinuationFlags,
    payload: PacketPayload,
    timeout: Duration,
) -> io::Result<()> {
    let stream_type = if matches!(payload, PacketPayload::Header(_)) {
        ID4::PKT_TYPE_HEAD
    } else {
        ID4::PKT_TYPE_DATA
    };
    let packet = ChannelPacket {
        channel_id: Some(channel_id),
        stream_type,
        pos,
        cont,
        payload,
        unknown_extensions: Vec::new(),
    };
    for atom in packet.to_atoms().map_err(io::Error::other)? {
        write_with_deadline(writer, &atom.to_bytes().map_err(io::Error::other)?, timeout).await?;
    }
    Ok(())
}

async fn write_with_deadline<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    tokio::time::timeout(timeout, writer.write_all(bytes))
        .await
        .map_err(io::Error::other)?
}
