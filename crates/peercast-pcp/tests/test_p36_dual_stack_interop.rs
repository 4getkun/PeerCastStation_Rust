use bytes::{Bytes, BytesMut};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use peercast_core::atom::{Atom, AtomBudget};
use peercast_core::channel_id::ChannelId;
use peercast_core::errors::QuitReason;
use peercast_core::host::{Host, HostFlags1};
use peercast_pcp::probe::{
    execute_reverse_probe, execute_reverse_probe_with_profile, ProbeCodec, ProbeError,
    ProbeLimiter, ProbeLimits, ProbePeerProfile, ProbeResult,
};
use peercast_pcp::relay_client::{
    RelayClientPhase, RelayClientStateMachine, RelayEvent, RelayNetworkProfile,
};
use peercast_pcp::relay_server::{
    RelayServerError, RelayServerEvent, RelayServerPhase, RelayServerStateMachine,
};

fn sample_cid() -> ChannelId {
    ChannelId::from_hex("0123456789ABCDEF0123456789ABCDEF").unwrap()
}

fn sample_client_sid() -> ChannelId {
    ChannelId::from_hex("11111111111111111111111111111111").unwrap()
}

fn sample_server_sid() -> ChannelId {
    ChannelId::from_hex("22222222222222222222222222222222").unwrap()
}

/// Acceptance criterion 1:
/// Header 1 / 100 consistency with channel network profile and socket families (REQ-034, Station parity `PCPOutputStream.cs:938-942`).
#[test]
fn test_p36_header_version_and_family_consistency_matrix() {
    let server_sid = sample_server_sid();
    let ipv4_addr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));
    let ipv6_addr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));

    // Case 1: IPv4 channel + x-peercast-pcp: 1 + IPv4 socket -> ACCEPT (200)
    {
        let mut server =
            RelayServerStateMachine::new(server_sid, RelayNetworkProfile::IPv4, Some(ipv4_addr));
        let raw =
            b"GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\nx-peercast-pcp: 1\r\n\r\n";
        server.feed_bytes(&mut Bytes::from_static(raw)).unwrap();
        assert!(server.validate_request_family().is_ok());
    }

    // Case 2: IPv4 channel + x-peercast-pcp: 100 + IPv4 socket -> REJECT (404 mismatch)
    {
        let mut server =
            RelayServerStateMachine::new(server_sid, RelayNetworkProfile::IPv4, Some(ipv4_addr));
        let raw = b"GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\nx-peercast-pcp: 100\r\n\r\n";
        server.feed_bytes(&mut Bytes::from_static(raw)).unwrap();
        let err = server.validate_request_family().unwrap_err();
        assert!(matches!(
            err,
            RelayServerError::NetworkFamilyMismatch { .. }
        ));
        let not_found_bytes = server.reject_not_found();
        assert!(String::from_utf8(not_found_bytes)
            .unwrap()
            .starts_with("HTTP/1.0 404 Not Found"));
    }

    // Case 3: IPv4 channel + missing x-peercast-pcp -> REJECT (400 bad request)
    {
        let mut server =
            RelayServerStateMachine::new(server_sid, RelayNetworkProfile::IPv4, Some(ipv4_addr));
        let raw = b"GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\n\r\n";
        server.feed_bytes(&mut Bytes::from_static(raw)).unwrap();
        let err = server.validate_request_family().unwrap_err();
        assert_eq!(err, RelayServerError::MissingPcpHeader);
        let bad_req_bytes = server.reject_bad_request("Missing x-peercast-pcp header");
        assert!(String::from_utf8(bad_req_bytes)
            .unwrap()
            .starts_with("HTTP/1.0 400 Bad Request"));
    }

    // Case 4: IPv6 channel + x-peercast-pcp: 100 + IPv6 socket -> ACCEPT (200)
    {
        let mut server =
            RelayServerStateMachine::new(server_sid, RelayNetworkProfile::IPv6, Some(ipv6_addr));
        let raw = b"GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\nx-peercast-pcp: 100\r\n\r\n";
        server.feed_bytes(&mut Bytes::from_static(raw)).unwrap();
        assert!(server.validate_request_family().is_ok());
    }

    // Case 5: IPv6 channel + x-peercast-pcp: 1 + IPv6 socket -> REJECT (404 mismatch)
    {
        let mut server =
            RelayServerStateMachine::new(server_sid, RelayNetworkProfile::IPv6, Some(ipv6_addr));
        let raw =
            b"GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\nx-peercast-pcp: 1\r\n\r\n";
        server.feed_bytes(&mut Bytes::from_static(raw)).unwrap();
        let err = server.validate_request_family().unwrap_err();
        assert!(matches!(
            err,
            RelayServerError::NetworkFamilyMismatch { .. }
        ));
    }

    // Case 6: IPv6 channel + x-peercast-pcp: 100 + IPv4 socket -> REJECT (404 mismatch)
    {
        let mut server =
            RelayServerStateMachine::new(server_sid, RelayNetworkProfile::IPv6, Some(ipv4_addr));
        let raw = b"GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\nx-peercast-pcp: 100\r\n\r\n";
        server.feed_bytes(&mut Bytes::from_static(raw)).unwrap();
        let err = server.validate_request_family().unwrap_err();
        assert!(matches!(
            err,
            RelayServerError::NetworkFamilyMismatch { .. }
        ));
    }

    // Case 7: IPv4 channel + x-peercast-pcp: 1 + IPv6 socket -> REJECT (404 mismatch)
    {
        let mut server =
            RelayServerStateMachine::new(server_sid, RelayNetworkProfile::IPv4, Some(ipv6_addr));
        let raw =
            b"GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\nx-peercast-pcp: 1\r\n\r\n";
        server.feed_bytes(&mut Bytes::from_static(raw)).unwrap();
        let err = server.validate_request_family().unwrap_err();
        assert!(matches!(
            err,
            RelayServerError::NetworkFamilyMismatch { .. }
        ));
    }
}

/// Acceptance criterion 2:
/// IPv4-mapped IPv6 address handling (`::ffff:a.b.c.d`) (REQ-034, REQ-071).
#[test]
fn test_p36_ipv4_mapped_address_handling() {
    let server_sid = sample_server_sid();
    // ::ffff:192.168.1.100 (standard IPv4-mapped IPv6 address)
    let mapped_ip = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc0a8, 0x0164));

    // Case 1: IPv4 channel with header 1 + mapped IP -> unmapped to IPv4, so VALID!
    {
        let mut server =
            RelayServerStateMachine::new(server_sid, RelayNetworkProfile::IPv4, Some(mapped_ip));
        let raw =
            b"GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\nx-peercast-pcp: 1\r\n\r\n";
        server.feed_bytes(&mut Bytes::from_static(raw)).unwrap();
        assert!(server.validate_request_family().is_ok());
    }

    // Case 2: IPv4 channel with header 100 + mapped IP -> header mismatch, so REJECTED!
    {
        let mut server =
            RelayServerStateMachine::new(server_sid, RelayNetworkProfile::IPv4, Some(mapped_ip));
        let raw = b"GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\nx-peercast-pcp: 100\r\n\r\n";
        server.feed_bytes(&mut Bytes::from_static(raw)).unwrap();
        assert!(server.validate_request_family().is_err());
    }

    // Case 3: IPv6 channel with header 100 + mapped IP -> logical IP is IPv4, so REJECTED on IPv6 channel!
    {
        let mut server =
            RelayServerStateMachine::new(server_sid, RelayNetworkProfile::IPv6, Some(mapped_ip));
        let raw = b"GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\nx-peercast-pcp: 100\r\n\r\n";
        server.feed_bytes(&mut Bytes::from_static(raw)).unwrap();
        assert!(server.validate_request_family().is_err());
    }
}

/// Acceptance criterion 3:
/// Dual-stack loopback TCP relay handshake (200 OK path) over IPv6 `[::1]` (REQ-032, REQ-034, REQ-071).
#[tokio::test]
async fn test_p36_ipv6_loopback_relay_200_handshake() {
    // Bind to IPv6 loopback [::1]:0
    let listener = match TcpListener::bind("[::1]:0").await {
        Ok(l) => l,
        Err(_) => {
            eprintln!("IPv6 loopback not supported on host; skipping test_p36_ipv6_loopback_relay_200_handshake");
            return;
        }
    };
    let addr = listener.local_addr().unwrap();

    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();
    let cid = sample_cid();

    let server_task = tokio::spawn(async move {
        let (mut socket, peer_addr) = listener.accept().await.unwrap();
        let mut server = RelayServerStateMachine::new(
            server_sid,
            RelayNetworkProfile::IPv6,
            Some(peer_addr.ip()),
        );

        let mut buf = BytesMut::with_capacity(1024);
        let mut tmp = [0u8; 512];

        // 1. Read HTTP request
        loop {
            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            buf.extend_from_slice(&tmp[..n]);

            let mut freeze = buf.clone().freeze();
            if let Some(ev) = server.feed_bytes(&mut freeze).unwrap() {
                let consumed = buf.len() - freeze.len();
                let _ = buf.split_to(consumed);
                assert!(matches!(ev, RelayServerEvent::RequestReceived(_)));
                break;
            }
        }

        // Validate family and accept
        server.validate_request_family().unwrap();
        let http_200 = server.accept().unwrap();
        socket.write_all(&http_200).await.unwrap();

        // 2. Read client helo Atom
        let outbound = loop {
            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            buf.extend_from_slice(&tmp[..n]);

            let mut freeze = buf.clone().freeze();
            if let Some(ev) = server.feed_bytes(&mut freeze).unwrap() {
                let consumed = buf.len() - freeze.len();
                let _ = buf.split_to(consumed);
                match ev {
                    RelayServerEvent::HandshakeCompleted { outbound_bytes, .. } => {
                        break outbound_bytes;
                    }
                    other => panic!("Unexpected event: {:?}", other),
                }
            }
        };

        // 3. Send oleh + ok(1)
        socket.write_all(&outbound).await.unwrap();
        assert_eq!(server.phase(), RelayServerPhase::Streaming);
    });

    let client_task = tokio::spawn(async move {
        let mut socket = TcpStream::connect(addr).await.unwrap();
        let mut client =
            RelayClientStateMachine::new(cid, client_sid, RelayNetworkProfile::IPv6, 0);

        // 1. Send HTTP request with x-peercast-pcp: 100
        let req_bytes = client.build_relay_request(&addr.to_string(), "RustRelay/1.0");
        let req_str = String::from_utf8(req_bytes.to_vec()).unwrap();
        assert!(req_str.contains("x-peercast-pcp: 100\r\n"));
        socket.write_all(&req_bytes).await.unwrap();

        let mut read_buf = BytesMut::with_capacity(1024);
        let mut tmp = [0u8; 512];

        // 2. Read 200 OK
        loop {
            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            read_buf.extend_from_slice(&tmp[..n]);

            let mut freeze = read_buf.clone().freeze();
            if let Some(ev) = client.feed_bytes(&mut freeze).unwrap() {
                let consumed = read_buf.len() - freeze.len();
                let _ = read_buf.split_to(consumed);
                match ev {
                    RelayEvent::Connected200 => break,
                    other => panic!("Expected Connected200, got {:?}", other),
                }
            }
        }

        // 3. Send client helo
        let helo_bytes = client.build_helo("RustRelay/1.0", Some(7144)).unwrap();
        socket.write_all(&helo_bytes).await.unwrap();

        // 4. Read server oleh + ok(1) until streaming phase
        while client.phase() != RelayClientPhase::Streaming {
            while !read_buf.is_empty() {
                let mut freeze = read_buf.clone().freeze();
                let _ = client.feed_bytes(&mut freeze).unwrap();
                let consumed = read_buf.len() - freeze.len();
                let _ = read_buf.split_to(consumed);
                if client.phase() == RelayClientPhase::Streaming {
                    break;
                }
            }
            if client.phase() == RelayClientPhase::Streaming {
                break;
            }
            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            read_buf.extend_from_slice(&tmp[..n]);
        }

        assert_eq!(client.phase(), RelayClientPhase::Streaming);
    });

    let (res_s, res_c) = tokio::join!(server_task, client_task);
    res_s.unwrap();
    res_c.unwrap();
}

/// Acceptance criterion 4:
/// Dual-stack 503 candidate host distribution with 16-byte reversed IPv6 addresses (REQ-024, REQ-032, REQ-034).
#[test]
fn test_p36_dual_stack_503_candidate_host_distribution() {
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();
    let cid = sample_cid();

    let mut server = RelayServerStateMachine::new(
        server_sid,
        RelayNetworkProfile::IPv6,
        Some(IpAddr::V6(Ipv6Addr::LOCALHOST)),
    );

    let raw_request =
        b"GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\nx-peercast-pcp: 100\r\n\r\n";
    let mut req_buf = Bytes::from_static(raw_request);
    server.feed_bytes(&mut req_buf).unwrap();
    server.validate_request_family().unwrap();

    // Create candidate host on IPv6: 2001:db8::1:7144
    let cand_ip = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));
    let cand_ep = SocketAddr::new(cand_ip, 7144);
    let cand_sid = ChannelId::from_hex("33333333333333333333333333333333").unwrap();

    let mut candidate_host = Host::new(cand_sid, cid);
    candidate_host.global_endpoint = Some(cand_ep);
    candidate_host.flags = HostFlags1::RELAY;

    let resp_503 = server.reject_full(vec![candidate_host]).unwrap();
    let resp_str = String::from_utf8_lossy(&resp_503);
    assert!(resp_str.starts_with("HTTP/1.0 503 Service Unavailable\r\n"));

    // Client consumes 503 response and parses candidate hosts
    let mut client = RelayClientStateMachine::new(cid, client_sid, RelayNetworkProfile::IPv6, 0);
    let mut stream_buf = Bytes::from(resp_503);

    // 1. Client reads HTTP 503
    let ev1 = client.feed_bytes(&mut stream_buf).unwrap().unwrap();
    assert_eq!(ev1, RelayEvent::Unavailable503);

    // 2. Client sends helo
    let client_helo = client.build_helo("RustClient/1.0", Some(7144)).unwrap();

    // 3. Server processes helo and emits oleh + candidate hosts + quit(1003)
    let mut helo_stream = client_helo;
    let full_handshake = server.feed_bytes(&mut helo_stream).unwrap().unwrap();
    let server_full_bytes = match full_handshake {
        RelayServerEvent::FullHandshakeCompleted { outbound_bytes, .. } => outbound_bytes,
        other => panic!("Expected FullHandshakeCompleted, got {:?}", other),
    };

    // 4. Feed server oleh to client
    let mut full_resp = server_full_bytes;
    let ev_oleh = client.feed_bytes(&mut full_resp).unwrap().unwrap();
    assert!(matches!(ev_oleh, RelayEvent::HandshakeCompleted { .. }));

    // 5. Feed candidate host to client
    let ev_host = client.feed_bytes(&mut full_resp).unwrap().unwrap();
    assert!(matches!(ev_host, RelayEvent::Host(_)));

    // 6. Feed quit to client
    let ev_quit = client.feed_bytes(&mut full_resp).unwrap().unwrap();
    assert_eq!(ev_quit, RelayEvent::Quit(QuitReason::UNAVAILABLE));

    // Verify candidate hosts parsed by client: 1 host with matching IPv6 and session ID!
    assert_eq!(client.candidate_hosts().len(), 1);
    let parsed_host = &client.candidate_hosts()[0];
    assert_eq!(parsed_host.session_id, cand_sid);
    assert_eq!(parsed_host.contact_address(), Some(cand_ep));
}

/// Acceptance criterion 5:
/// Reverse reachability probe behavior differences (Station vs PeerCast-YT) (REQ-034, REQ-036).
#[tokio::test]
async fn test_p36_probe_behavior_differences_station_vs_yt() {
    let listener = match TcpListener::bind("[::1]:0").await {
        Ok(l) => l,
        Err(_) => {
            eprintln!("IPv6 loopback not supported; skipping probe difference test");
            return;
        }
    };
    let addr = listener.local_addr().unwrap();

    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();

    // 1. Probe listener acting as Station: strictly enforces PCP version 1
    let station_listener_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = BytesMut::with_capacity(1024);
        let mut tmp = [0u8; 512];
        let budget = AtomBudget::default();

        // Read pcp\n
        let n = socket.read(&mut tmp).await.unwrap();
        buf.extend_from_slice(&tmp[..n]);
        let mut freeze = buf.clone().freeze();
        let ver = ProbeCodec::parse_probe_preamble(&mut freeze, &budget)
            .unwrap()
            .unwrap();

        // Station pong validation: strictly requires version 1 (REQ-036)
        if ProbeCodec::validate_pong_preamble(ver, ProbePeerProfile::PeerCastStation).is_ok() {
            // Read helo
            let _helo = Atom::decode(&mut freeze, &budget).unwrap().unwrap();
            // Send oleh
            let oleh = ProbeCodec::build_pong_oleh(client_sid, "PeerCastStation/1.0").unwrap();
            socket.write_all(&oleh).await.unwrap();
            // Read quit
            let mut q_buf = [0u8; 12];
            let _ = socket.read_exact(&mut q_buf).await;
        } else {
            // Station pong rejects: closes socket immediately
            let _ = socket.shutdown().await;
        }
    });

    let limits = ProbeLimits {
        enable_ipv6: true,
        ..ProbeLimits::default()
    };
    let limiter = ProbeLimiter::new(limits);

    // Pinging Station peer over IPv6 using Station profile (version 1) -> SUCCEEDS!
    let res = execute_reverse_probe_with_profile(
        &limiter,
        addr.ip(),
        addr.port(),
        client_sid,
        server_sid,
        "RustServent/1.0",
        ProbePeerProfile::PeerCastStation,
    )
    .await
    .unwrap();

    assert!(matches!(res, ProbeResult::Reachable { .. }));
    station_listener_task.await.unwrap();

    // 2. Now test Station peer receiving YT profile (version 100): Station MUST reject it!
    let listener2 = TcpListener::bind("[::1]:0").await.unwrap();
    let addr2 = listener2.local_addr().unwrap();

    let station_reject_task = tokio::spawn(async move {
        let (mut socket, _) = listener2.accept().await.unwrap();
        let mut buf = BytesMut::with_capacity(1024);
        let mut tmp = [0u8; 512];
        let budget = AtomBudget::default();

        let n = socket.read(&mut tmp).await.unwrap();
        buf.extend_from_slice(&tmp[..n]);
        let mut freeze = buf.clone().freeze();
        let ver = ProbeCodec::parse_probe_preamble(&mut freeze, &budget)
            .unwrap()
            .unwrap();

        // Station pong strictly validates version 1
        if ProbeCodec::validate_pong_preamble(ver, ProbePeerProfile::PeerCastStation).is_err() {
            // Rejection: close connection
            let _ = socket.shutdown().await;
        }
    });

    let res_reject = execute_reverse_probe_with_profile(
        &limiter,
        addr2.ip(),
        addr2.port(),
        client_sid,
        server_sid,
        "RustServent/1.0",
        ProbePeerProfile::PeerCastYt, // Sends version 100 on IPv6
    )
    .await
    .unwrap();

    // Result is Unreachable because Station closed connection upon version 100!
    assert!(matches!(res_reject, ProbeResult::Unreachable { .. }));
    station_reject_task.await.unwrap();

    // 3. Test YT peer receiving YT profile (version 100): YT pong accepts version 100!
    let listener3 = TcpListener::bind("[::1]:0").await.unwrap();
    let addr3 = listener3.local_addr().unwrap();

    let yt_accept_task = tokio::spawn(async move {
        let (mut socket, _) = listener3.accept().await.unwrap();
        let mut buf = BytesMut::with_capacity(1024);
        let mut tmp = [0u8; 512];
        let budget = AtomBudget::default();

        let n = socket.read(&mut tmp).await.unwrap();
        buf.extend_from_slice(&tmp[..n]);
        let mut freeze = buf.clone().freeze();
        let ver = ProbeCodec::parse_probe_preamble(&mut freeze, &budget)
            .unwrap()
            .unwrap();

        // YT pong accepts version 1 or 100
        assert!(ProbeCodec::validate_pong_preamble(ver, ProbePeerProfile::PeerCastYt).is_ok());

        let _helo = Atom::decode(&mut freeze, &budget).unwrap().unwrap();
        let oleh = ProbeCodec::build_pong_oleh(client_sid, "peercast-yt/1.0").unwrap();
        socket.write_all(&oleh).await.unwrap();

        let mut q_buf = [0u8; 12];
        let _ = socket.read_exact(&mut q_buf).await;
    });

    let res_yt = execute_reverse_probe_with_profile(
        &limiter,
        addr3.ip(),
        addr3.port(),
        client_sid,
        server_sid,
        "RustServent/1.0",
        ProbePeerProfile::PeerCastYt,
    )
    .await
    .unwrap();

    assert!(matches!(res_yt, ProbeResult::Reachable { .. }));
    yt_accept_task.await.unwrap();
}

/// Acceptance criterion 6:
/// Default ProbeLimits leaves IPv6 marked pending unless explicitly enabled (REQ-036).
#[tokio::test]
async fn test_p36_default_ipv6_probe_pending_guard() {
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();
    let limiter = ProbeLimiter::new(ProbeLimits::default()); // default has enable_ipv6 = false

    let ipv6_addr = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1));
    let err = execute_reverse_probe(
        &limiter,
        ipv6_addr,
        7144,
        client_sid,
        server_sid,
        "RustServent/1.0",
    )
    .await
    .unwrap_err();

    assert_eq!(err, ProbeError::IPv6Pending);
}

/// Acceptance criterion 7:
/// Reconnect with position cursor over IPv6 (`x-peercast-pos` + `x-peercast-pcp: 100`) (REQ-031, REQ-034).
#[test]
fn test_p36_reconnect_cursor_over_ipv6() {
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();
    let cid = sample_cid();

    // 1. Client creates request with cursor pos 131072 over IPv6
    let mut client =
        RelayClientStateMachine::new(cid, client_sid, RelayNetworkProfile::IPv6, 131072);
    let req_bytes = client.build_relay_request("[::1]:7144", "RustClient/1.0");
    let req_str = String::from_utf8(req_bytes.to_vec()).unwrap();

    assert!(req_str.contains("x-peercast-pcp: 100\r\n"));
    assert!(req_str.contains("x-peercast-pos: 131072\r\n"));

    // 2. Server receives and parses request
    let mut server = RelayServerStateMachine::new(
        server_sid,
        RelayNetworkProfile::IPv6,
        Some(IpAddr::V6(Ipv6Addr::LOCALHOST)),
    );
    let mut in_bytes = Bytes::from(req_bytes);
    let ev = server
        .feed_bytes(&mut in_bytes)
        .unwrap()
        .expect("RequestReceived");

    match ev {
        RelayServerEvent::RequestReceived(req) => {
            assert_eq!(req.channel_id, cid);
            assert_eq!(req.pcp_version, Some(100));
            assert_eq!(req.pos, Some(131072));
        }
        other => panic!("Unexpected event: {:?}", other),
    }

    assert!(server.validate_request_family().is_ok());
}
