use super::*;
use bytes::BytesMut;
use peercast_core::atom::AtomBudget;
use peercast_core::errors::QuitReason;
use peercast_core::host::Host;
use peercast_core::host_registry::{
    EndpointCooldownTable, EndpointIdentity, HostRegistry, DEFAULT_FRESHNESS_THRESHOLD,
};
use peercast_core::parent_selection::{
    LocalityDetector, ParentSelector, ReconnectBudget, RetryDecision, StationWeightedRandomPolicy,
    SystemRng,
};
use peercast_pcp::relay_client::{
    RelayClientStateMachine, RelayError, RelayEvent, RelayNetworkProfile,
};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

struct AttemptResult {
    candidates: Vec<Host>,
    terminal: bool,
    media_received: bool,
}

pub(super) async fn run(runtime: Arc<AppRuntime>, channel: Arc<RuntimeChannel>, seed: SocketAddr) {
    let config = &runtime.config.runtime;
    let budget = ReconnectBudget::new(
        config.reconnect.max_attempts_per_cycle,
        Duration::from_millis(config.reconnect.base_backoff_ms),
        Duration::from_millis(config.reconnect.max_backoff_ms),
    );
    let mut selector = ParentSelector::new(
        StationWeightedRandomPolicy,
        LocalityDetector::default(),
        runtime.session_id,
        None,
        budget,
    );
    let mut hosts = HostRegistry::new(
        DEFAULT_FRESHNESS_THRESHOLD,
        config.relay.max_candidate_hosts,
    );
    let mut cooldown = EndpointCooldownTable::new(
        Duration::from_millis(config.reconnect.base_backoff_ms),
        config.relay.max_candidate_hosts,
    );
    let mut rng = SystemRng::new();
    let mut endpoint = seed;
    let mut shutdown = runtime.shutdown_tx.subscribe();
    loop {
        if *shutdown.borrow() {
            break;
        }
        let attempt_number = channel.connection_attempts.fetch_add(1, Ordering::Relaxed) + 1;
        channel.begin_epoch().await;
        let result = tokio::select! {
            result = attempt(&runtime, &channel, endpoint, attempt_number) => result,
            _ = shutdown.changed() => break,
        };
        channel.offline().await;
        runtime.channel_changed(&channel).await;
        match result {
            Ok(result) => {
                runtime.log.log(
                    LogLevel::Debug,
                    "relay",
                    &format!(
                        "source ended channel_id={} endpoint={} attempt={} media_received={} terminal={} candidate_count={}",
                        channel.id,
                        endpoint,
                        attempt_number,
                        result.media_received,
                        result.terminal,
                        result.candidates.len()
                    ),
                );
                if result.media_received {
                    selector.record_attempt_success();
                }
                *channel.candidates.lock().await = result.candidates.clone();
                for mut host in result.candidates {
                    host.channel_id = channel.id;
                    hosts.upsert(host);
                }
                if result.terminal {
                    break;
                }
            }
            Err(error) => runtime.log.log(
                LogLevel::Warn,
                "relay",
                &format!(
                    "attempt failed channel_id={} endpoint={} attempt={}: {}",
                    channel.id, endpoint, attempt_number, error
                ),
            ),
        }
        let decision = selector.record_attempt_failure(
            EndpointIdentity::for_channel(channel.id, endpoint),
            &mut cooldown,
            None,
        );
        endpoint = selector
            .select_best_parent(&channel.id, &hosts, &cooldown, &mut rng)
            .and_then(|host| host.contact_address())
            .unwrap_or(seed);
        let backoff_delay = match decision {
            RetryDecision::Exhausted { backoff_delay } => backoff_delay,
            RetryDecision::RetryImmediately { .. } => Duration::ZERO,
        };
        runtime.log.log(
            LogLevel::Debug,
            "relay",
            &format!(
                "next connection channel_id={} endpoint={} attempt={} backoff_ms={}",
                channel.id,
                endpoint,
                attempt_number.saturating_add(1),
                backoff_delay.as_millis()
            ),
        );
        if let RetryDecision::Exhausted { backoff_delay } = decision {
            tokio::select! {
                _ = tokio::time::sleep(backoff_delay) => {},
                _ = shutdown.changed() => break,
            }
        }
    }
    channel.offline().await;
    runtime.channel_changed(&channel).await;
}

async fn attempt(
    runtime: &AppRuntime,
    channel: &RuntimeChannel,
    endpoint: SocketAddr,
    attempt_number: usize,
) -> io::Result<AttemptResult> {
    let config = &runtime.config.runtime;
    let lease = channel
        .connections
        .track("source", Some(endpoint), "connecting");
    let mut stream = tokio::time::timeout(
        Duration::from_millis(config.reconnect.connect_timeout_ms),
        TcpStream::connect(endpoint),
    )
    .await
    .map_err(io::Error::other)??;
    runtime.log.log(
        LogLevel::Info,
        "relay",
        &format!(
            "TCP connected channel_id={} endpoint={} attempt={}",
            channel.id, endpoint, attempt_number
        ),
    );
    let profile = if endpoint.is_ipv6() {
        RelayNetworkProfile::IPv6
    } else {
        RelayNetworkProfile::IPv4
    };
    let mut machine = RelayClientStateMachine::new(channel.id, runtime.session_id, profile, 0)
        .with_max_candidate_hosts(config.relay.max_candidate_hosts);
    let request = machine.build_relay_request(&endpoint.to_string(), APPLICATION_NAME);
    let io_timeout = Duration::from_millis(config.reconnect.read_timeout_ms);
    tokio::time::timeout(io_timeout, stream.write_all(&request))
        .await
        .map_err(io::Error::other)??;
    lease.sent(request.len());
    let mut pending = BytesMut::new();
    let mut chunk = [0u8; peercast_pcp::packet::SPLIT_BODY_DATALEN];
    let mut media_received = false;
    let mut header_received = false;
    loop {
        let mut input = pending.split().freeze();
        loop {
            let before = input.len();
            let event = match machine.feed_bytes(&mut input) {
                Ok(event) => event,
                Err(RelayError::OffAir) => {
                    runtime.log.log(
                        LogLevel::Info,
                        "relay",
                        &format!(
                            "HTTP 404 channel_id={} endpoint={} attempt={}",
                            channel.id, endpoint, attempt_number
                        ),
                    );
                    return Ok(AttemptResult {
                        candidates: Vec::new(),
                        terminal: true,
                        media_received,
                    });
                }
                Err(error) => return Err(io::Error::other(error)),
            };
            match event {
                Some(RelayEvent::Connected200) => {
                    runtime.log.log(
                        LogLevel::Info,
                        "relay",
                        &format!(
                            "HTTP 200 channel_id={} endpoint={} attempt={}",
                            channel.id, endpoint, attempt_number
                        ),
                    );
                    let helo = relay_helo(runtime, &machine, endpoint.is_ipv6())?;
                    tokio::time::timeout(io_timeout, stream.write_all(&helo))
                        .await
                        .map_err(io::Error::other)??;
                    lease.sent(helo.len());
                }
                Some(RelayEvent::Unavailable503) => {
                    runtime.log.log(
                        LogLevel::Warn,
                        "relay",
                        &format!(
                            "HTTP 503 channel_id={} endpoint={} attempt={}",
                            channel.id, endpoint, attempt_number
                        ),
                    );
                    let helo = relay_helo(runtime, &machine, endpoint.is_ipv6())?;
                    tokio::time::timeout(io_timeout, stream.write_all(&helo))
                        .await
                        .map_err(io::Error::other)??;
                    lease.sent(helo.len());
                }
                Some(RelayEvent::HandshakeCompleted {
                    remote_ip,
                    remote_port,
                    ..
                }) => {
                    runtime
                        .network
                        .observe(remote_ip, remote_port, endpoint.is_ipv6());
                    lease.connected(
                        machine.remote_session_id().map(|id| id.to_string()),
                        None,
                        None,
                    );
                    lease.set_status("connected");
                    runtime.log.log(
                        LogLevel::Debug,
                        "relay",
                        &format!(
                            "HELO completed channel_id={} endpoint={} attempt={}",
                            channel.id, endpoint, attempt_number
                        ),
                    );
                }
                Some(RelayEvent::HeaderChunk(header)) => {
                    media_received |= !header.data.is_empty();
                    if !header_received {
                        header_received = true;
                        runtime.log.log(
                            LogLevel::Info,
                            "relay",
                            &format!(
                                "initial header received channel_id={} endpoint={} attempt={}",
                                channel.id, endpoint, attempt_number
                            ),
                        );
                    }
                    channel.set_header(header.data, header.pos).await?;
                    runtime.channel_changed(channel).await;
                }
                Some(RelayEvent::MediaChunk(content)) => {
                    media_received |= !content.data.is_empty();
                    let was_available = channel.snapshot().await.available;
                    channel
                        .publish(content.pos, content.data, content.cont_flags)
                        .await;
                    if !was_available {
                        runtime.channel_changed(channel).await;
                    }
                }
                Some(RelayEvent::Broadcast(atom)) => {
                    // Evaluate the configured storm window before accepting repeated control packets.
                    let _ = runtime.process_broadcast(&atom).await;
                }
                Some(RelayEvent::Quit(reason)) => {
                    let terminal = matches!(
                        reason,
                        QuitReason::OFF_AIR
                            | QuitReason::BANNED
                            | QuitReason::SHUTDOWN
                            | QuitReason::BAD_AGENT
                            | QuitReason::NORMAL
                    );
                    return Ok(AttemptResult {
                        candidates: machine.take_candidate_hosts(),
                        terminal,
                        media_received,
                    });
                }
                _ => {}
            }
            if input.len() == before {
                break;
            }
        }
        pending = input
            .try_into_mut()
            .unwrap_or_else(|bytes| BytesMut::from(bytes.as_ref()));
        // The initial media header can arrive after a keyframe interval or
        // parent selection. Keep the normal live-stream timeout once media
        // arrives, but allow startup to complete before reconnecting.
        let read_timeout = if media_received {
            io_timeout
        } else {
            io_timeout.max(Duration::from_secs(30))
        };
        let n = tokio::time::timeout(read_timeout, stream.read(&mut chunk))
            .await
            .map_err(io::Error::other)??;
        lease.received(n);
        if n == 0 {
            return Ok(AttemptResult {
                candidates: machine.take_candidate_hosts(),
                terminal: false,
                media_received,
            });
        }
        if pending.len().saturating_add(n) > AtomBudget::default().max_total_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "relay input exceeds atom budget",
            ));
        }
        pending.extend_from_slice(&chunk[..n]);
    }
}

fn relay_helo(
    runtime: &AppRuntime,
    machine: &RelayClientStateMachine,
    ipv6: bool,
) -> io::Result<bytes::Bytes> {
    let network = runtime.network_snapshot();
    let status = if ipv6 { &network.ipv6 } else { &network.ipv4 };
    let result = match status.as_str() {
        "open" => machine.build_helo(APPLICATION_NAME, Some(network.relay_port)),
        "unknown" => machine.build_helo_with_ping(APPLICATION_NAME, network.relay_port),
        // A loopback-only listener is already known to be unreachable from
        // the Internet. Asking each parent to probe it blocks the handshake.
        _ => machine.build_helo(APPLICATION_NAME, None),
    };
    result.map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_listener_does_not_request_reverse_ping() {
        let runtime = AppRuntime::new(
            &AppConfig::default(),
            TaskTracker::new(),
            Arc::new(BoundedLogBuffer::default()),
            Arc::new(RwLock::new(RpcState::default())),
        )
        .unwrap();
        let machine = RelayClientStateMachine::new(
            ChannelId::from_hex("11111111111111111111111111111111").unwrap(),
            runtime.session_id,
            RelayNetworkProfile::IPv4,
            0,
        );
        let mut bytes = relay_helo(&runtime, &machine, false).unwrap();
        let atom = peercast_core::atom::Atom::decode(&mut bytes, &AtomBudget::default())
            .unwrap()
            .unwrap();
        let children = atom.children().unwrap();
        assert!(children.iter().all(|child| {
            child.name() != peercast_core::id4::ID4::HELO_PING
                && child.name() != peercast_core::id4::ID4::HELO_PORT
        }));
    }
}
