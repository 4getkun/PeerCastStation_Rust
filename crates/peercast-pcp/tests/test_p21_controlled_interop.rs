use bytes::{Bytes, BytesMut};
use sha2::{Digest, Sha256};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use peercast_core::atom::Atom;
use peercast_core::channel_id::ChannelId;
use peercast_core::content::ContinuationFlags;
use peercast_core::errors::QuitReason;
use peercast_core::host::{Host, HostFlags1};
use peercast_core::id4::ID4;
use peercast_pcp::packet::{ChannelPacket, PacketPayload};
use peercast_pcp::probe::{execute_reverse_probe, ProbeLimiter, ProbeLimits, ProbeResult};
use peercast_pcp::relay_client::{
    RelayClientPhase, RelayClientStateMachine, RelayEvent, RelayNetworkProfile, SERVANT_VERSION,
};
use peercast_pcp::relay_server::{RelayServerEvent, RelayServerStateMachine};

fn sample_channel_id() -> ChannelId {
    ChannelId::from_hex("0123456789abcdef0123456789abcdef").unwrap()
}

fn sample_server_sid() -> ChannelId {
    ChannelId::from_hex("11111111111111111111111111111111").unwrap()
}

fn sample_client_sid() -> ChannelId {
    ChannelId::from_hex("22222222222222222222222222222222").unwrap()
}

#[tokio::test]
async fn test_p21_loopback_200_relay_streaming_and_hash_verification() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();

    let channel_id = sample_channel_id();
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();

    // 45,000 bytes test payload (spans across 3 split packets of 15,360 bytes each)
    let payload_fixture: Vec<u8> = (0..45_000).map(|i| (i * 37 % 251) as u8).collect();
    let expected_hash = Sha256::digest(&payload_fixture);

    // Spawn controlled server task
    let payload_fixture_server = payload_fixture.clone();
    let server_handle = tokio::spawn(async move {
        let (mut socket, peer_addr) = listener.accept().await.unwrap();
        let mut server = RelayServerStateMachine::new(
            server_sid,
            RelayNetworkProfile::IPv4,
            Some(peer_addr.ip()),
        );

        let mut read_buf = BytesMut::with_capacity(65536);
        let mut tmp = [0u8; 8192];

        // 1. Read request
        let req = loop {
            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            read_buf.extend_from_slice(&tmp[..n]);
            let mut freeze = read_buf.clone().freeze();
            if let Some(ev) = server.feed_bytes(&mut freeze).unwrap() {
                let consumed = read_buf.len() - freeze.len();
                let _ = read_buf.split_to(consumed);
                match ev {
                    RelayServerEvent::RequestReceived(r) => break r,
                    _ => panic!("Expected RequestReceived"),
                }
            }
        };
        assert_eq!(req.channel_id, channel_id);

        // 2. Accept request -> send HTTP 200 OK
        let http_200 = server.accept().unwrap();
        socket.write_all(&http_200).await.unwrap();

        // 3. Read client helo -> send oleh + ok(1)
        loop {
            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            read_buf.extend_from_slice(&tmp[..n]);
            let mut freeze = read_buf.clone().freeze();
            if let Some(ev) = server.feed_bytes(&mut freeze).unwrap() {
                let consumed = read_buf.len() - freeze.len();
                let _ = read_buf.split_to(consumed);
                match ev {
                    RelayServerEvent::HandshakeCompleted { outbound_bytes, .. } => {
                        socket.write_all(&outbound_bytes).await.unwrap();
                        break;
                    }
                    _ => panic!("Expected HandshakeCompleted"),
                }
            }
        }

        // 4. Send Stream Header packet (Station format: type="head", data=<header_bytes>)
        let head_bytes = Bytes::from_static(b"FLV_STREAM_HEADER_FIXTURE_V1");
        let head_packet = ChannelPacket {
            channel_id: Some(channel_id),
            stream_type: ID4::PKT_TYPE_HEAD,
            pos: 0,
            cont: ContinuationFlags::NONE,
            payload: PacketPayload::Header(head_bytes),
            unknown_extensions: Vec::new(),
        };
        for atom in head_packet.to_atoms().unwrap() {
            let mut b = Vec::new();
            atom.encode(&mut b).unwrap();
            socket.write_all(&b).await.unwrap();
        }

        // 5. Send Stream Body packets (automatically split under 15,360 bytes)
        let body_packet = ChannelPacket {
            channel_id: Some(channel_id),
            stream_type: ID4::PKT_TYPE_DATA,
            pos: 1000,
            cont: ContinuationFlags::NONE,
            payload: PacketPayload::Data(Bytes::from(payload_fixture_server)),
            unknown_extensions: Vec::new(),
        };
        let body_atoms = body_packet.to_atoms().unwrap();
        assert_eq!(body_atoms.len(), 3, "45,000 bytes must split into 3 chunks");
        for atom in body_atoms {
            let mut b = Vec::new();
            atom.encode(&mut b).unwrap();
            socket.write_all(&b).await.unwrap();
        }

        // 6. Read client quit(1000)
        loop {
            let n = socket.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            read_buf.extend_from_slice(&tmp[..n]);
            let mut freeze = read_buf.clone().freeze();
            if let Some(RelayServerEvent::Quit(code)) = server.feed_bytes(&mut freeze).unwrap() {
                assert_eq!(code, QuitReason::NORMAL);
                break;
            }
        }
    });

    // Spawn controlled client task
    let client_handle = tokio::spawn(async move {
        let mut socket = TcpStream::connect(server_addr).await.unwrap();
        let mut client =
            RelayClientStateMachine::new(channel_id, client_sid, RelayNetworkProfile::IPv4, 0);

        // 1. Send HTTP GET request
        let req_bytes =
            client.build_relay_request(&server_addr.to_string(), "PeerCastRustTest/1.0");
        socket.write_all(&req_bytes).await.unwrap();

        let mut read_buf = BytesMut::with_capacity(65536);
        let mut tmp = [0u8; 8192];

        // 2. Read HTTP 200 OK response
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
                    _ => panic!("Expected Connected200"),
                }
            }
        }

        // 3. Send client helo
        let helo_bytes = client
            .build_helo("PeerCastRustTest/1.0", Some(7144))
            .unwrap();
        socket.write_all(&helo_bytes).await.unwrap();

        // 4. Single continuous event loop for handshake completion and media stream reassembly
        let mut handshake_done = false;
        let mut received_header = None;
        let mut reassembled_body = Vec::new();

        while reassembled_body.len() < 45_000 {
            while !read_buf.is_empty() {
                let mut freeze = read_buf.clone().freeze();
                match client.feed_bytes(&mut freeze) {
                    Ok(Some(ev)) => {
                        let consumed = read_buf.len() - freeze.len();
                        let _ = read_buf.split_to(consumed);
                        match ev {
                            RelayEvent::HandshakeCompleted {
                                remote_session_id, ..
                            } => {
                                assert_eq!(remote_session_id, Some(server_sid));
                                handshake_done = true;
                            }
                            RelayEvent::HeaderChunk(header) => {
                                assert!(received_header.replace(header.data).is_none());
                            }
                            RelayEvent::MediaChunk(chunk) => {
                                reassembled_body.extend_from_slice(&chunk.data);
                            }
                            _ => {}
                        }
                    }
                    Ok(None) => {
                        let consumed = read_buf.len() - freeze.len();
                        let _ = read_buf.split_to(consumed);
                        if consumed == 0 {
                            break;
                        }
                    }
                    Err(e) => panic!("Relay feed error: {:?}", e),
                }
            }

            if reassembled_body.len() >= 45_000 {
                break;
            }

            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            read_buf.extend_from_slice(&tmp[..n]);
        }

        assert!(handshake_done);
        assert_eq!(client.phase(), RelayClientPhase::Streaming);

        assert_eq!(
            received_header.as_deref(),
            Some(b"FLV_STREAM_HEADER_FIXTURE_V1".as_ref()),
            "Header must be received bit-accurately"
        );
        let actual_hash = Sha256::digest(&reassembled_body);
        assert_eq!(
            actual_hash, expected_hash,
            "Reassembled media stream body must match SHA-256 fixture digest"
        );

        // 6. Graceful disconnect with quit(1000)
        let quit = Atom::new_u32_le(ID4::PCP_QUIT, QuitReason::NORMAL.raw());
        let mut quit_bytes = Vec::new();
        quit.encode(&mut quit_bytes).unwrap();
        socket.write_all(&quit_bytes).await.unwrap();
    });

    tokio::try_join!(server_handle, client_handle).unwrap();
}

#[tokio::test]
async fn test_p21_loopback_503_candidate_host_distribution() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();

    let channel_id = sample_channel_id();
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();

    let candidate1 = Host {
        session_id: ChannelId::from_hex("33333333333333333333333333333333").unwrap(),
        channel_id,
        broadcast_id: None,
        local_endpoint: None,
        global_endpoint: Some(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
            7144,
        )),
        flags: HostFlags1::RELAY,
        relay_count: 1,
        direct_count: 0,
        uptime: Duration::from_secs(3600),
        version: SERVANT_VERSION,
        hops: 1,
        last_seen: std::time::Instant::now(),
    };

    let candidate_host_expected = candidate1.clone();

    // Server task: reject full with 503
    let server_handle = tokio::spawn(async move {
        let (mut socket, peer_addr) = listener.accept().await.unwrap();
        let mut server = RelayServerStateMachine::new(
            server_sid,
            RelayNetworkProfile::IPv4,
            Some(peer_addr.ip()),
        );

        let mut read_buf = BytesMut::with_capacity(16384);
        let mut tmp = [0u8; 4096];

        // 1. Read request
        loop {
            let n = socket.read(&mut tmp).await.unwrap();
            read_buf.extend_from_slice(&tmp[..n]);
            let mut freeze = read_buf.clone().freeze();
            if let Some(RelayServerEvent::RequestReceived(_)) =
                server.feed_bytes(&mut freeze).unwrap()
            {
                let consumed = read_buf.len() - freeze.len();
                let _ = read_buf.split_to(consumed);
                break;
            }
        }

        // 2. Reject full -> HTTP 503
        let http_503 = server.reject_full(vec![candidate1]).unwrap();
        socket.write_all(&http_503).await.unwrap();

        // 3. Read helo -> send oleh + host + quit(1003)
        loop {
            let n = socket.read(&mut tmp).await.unwrap();
            read_buf.extend_from_slice(&tmp[..n]);
            let mut freeze = read_buf.clone().freeze();
            if let Some(ev) = server.feed_bytes(&mut freeze).unwrap() {
                let consumed = read_buf.len() - freeze.len();
                let _ = read_buf.split_to(consumed);
                if let RelayServerEvent::FullHandshakeCompleted { outbound_bytes, .. } = ev {
                    socket.write_all(&outbound_bytes).await.unwrap();
                    break;
                }
            }
        }
    });

    // Client task: connect and receive candidates
    let client_handle = tokio::spawn(async move {
        let mut socket = TcpStream::connect(server_addr).await.unwrap();
        let mut client =
            RelayClientStateMachine::new(channel_id, client_sid, RelayNetworkProfile::IPv4, 0);

        let req_bytes =
            client.build_relay_request(&server_addr.to_string(), "PeerCastRustTest/1.0");
        socket.write_all(&req_bytes).await.unwrap();

        let mut read_buf = BytesMut::with_capacity(16384);
        let mut tmp = [0u8; 4096];

        // Read 503
        loop {
            let n = socket.read(&mut tmp).await.unwrap();
            read_buf.extend_from_slice(&tmp[..n]);
            let mut freeze = read_buf.clone().freeze();
            if let Some(RelayEvent::Unavailable503) = client.feed_bytes(&mut freeze).unwrap() {
                let consumed = read_buf.len() - freeze.len();
                let _ = read_buf.split_to(consumed);
                break;
            }
        }

        // Send helo
        let helo_bytes = client
            .build_helo("PeerCastRustTest/1.0", Some(7144))
            .unwrap();
        socket.write_all(&helo_bytes).await.unwrap();

        // Read oleh, host, and quit(1003)
        let mut got_quit = false;
        while !got_quit {
            while !read_buf.is_empty() {
                let mut freeze = read_buf.clone().freeze();
                match client.feed_bytes(&mut freeze).unwrap() {
                    Some(RelayEvent::Quit(code)) => {
                        let consumed = read_buf.len() - freeze.len();
                        let _ = read_buf.split_to(consumed);
                        assert_eq!(code, QuitReason::UNAVAILABLE);
                        got_quit = true;
                        break;
                    }
                    Some(_) => {
                        let consumed = read_buf.len() - freeze.len();
                        let _ = read_buf.split_to(consumed);
                    }
                    None => {
                        let consumed = read_buf.len() - freeze.len();
                        let _ = read_buf.split_to(consumed);
                        if consumed == 0 {
                            break;
                        }
                    }
                }
            }
            if got_quit {
                break;
            }
            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            read_buf.extend_from_slice(&tmp[..n]);
        }

        assert_eq!(client.candidate_hosts().len(), 1);
        assert_eq!(
            client.candidate_hosts()[0].session_id,
            candidate_host_expected.session_id
        );
        assert_eq!(
            client.candidate_hosts()[0].global_endpoint,
            candidate_host_expected.global_endpoint
        );
    });

    tokio::try_join!(server_handle, client_handle).unwrap();
}

#[tokio::test]
async fn test_p21_loopback_404_offair_immediate_termination() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();

    let channel_id = sample_channel_id();
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();

    // Server sends 404
    let server_handle = tokio::spawn(async move {
        let (mut socket, peer_addr) = listener.accept().await.unwrap();
        let mut server = RelayServerStateMachine::new(
            server_sid,
            RelayNetworkProfile::IPv4,
            Some(peer_addr.ip()),
        );

        let mut read_buf = BytesMut::with_capacity(4096);
        let mut tmp = [0u8; 1024];

        loop {
            let n = socket.read(&mut tmp).await.unwrap();
            read_buf.extend_from_slice(&tmp[..n]);
            let mut freeze = read_buf.clone().freeze();
            if let Some(RelayServerEvent::RequestReceived(_)) =
                server.feed_bytes(&mut freeze).unwrap()
            {
                break;
            }
        }

        let http_404 = server.reject_not_found();
        socket.write_all(&http_404).await.unwrap();
    });

    // Client receives 404 -> OffAir error, 0 atom decode
    let client_handle = tokio::spawn(async move {
        let mut socket = TcpStream::connect(server_addr).await.unwrap();
        let mut client =
            RelayClientStateMachine::new(channel_id, client_sid, RelayNetworkProfile::IPv4, 0);

        let req_bytes =
            client.build_relay_request(&server_addr.to_string(), "PeerCastRustTest/1.0");
        socket.write_all(&req_bytes).await.unwrap();

        let mut read_buf = BytesMut::with_capacity(4096);
        let mut tmp = [0u8; 1024];
        let n = socket.read(&mut tmp).await.unwrap();
        read_buf.extend_from_slice(&tmp[..n]);

        let mut freeze = read_buf.clone().freeze();
        let res = client.feed_bytes(&mut freeze);
        assert_eq!(res, Err(peercast_pcp::relay_client::RelayError::OffAir));
        assert_eq!(
            client.phase(),
            RelayClientPhase::Terminal(QuitReason::OFF_AIR)
        );
    });

    tokio::try_join!(server_handle, client_handle).unwrap();
}

#[tokio::test]
async fn test_p21_loopback_reconnect_with_cursor_position() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();

    let channel_id = sample_channel_id();
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();

    let cursor_pos = 1_250_000i64;

    // Server task verifies x-peercast-pos
    let server_handle = tokio::spawn(async move {
        let (mut socket, peer_addr) = listener.accept().await.unwrap();
        let mut server = RelayServerStateMachine::new(
            server_sid,
            RelayNetworkProfile::IPv4,
            Some(peer_addr.ip()),
        );

        let mut read_buf = BytesMut::with_capacity(4096);
        let mut tmp = [0u8; 1024];

        let req = loop {
            let n = socket.read(&mut tmp).await.unwrap();
            read_buf.extend_from_slice(&tmp[..n]);
            let mut freeze = read_buf.clone().freeze();
            if let Some(RelayServerEvent::RequestReceived(r)) =
                server.feed_bytes(&mut freeze).unwrap()
            {
                break r;
            }
        };

        // Assert server parsed x-peercast-pos from client HTTP request header
        assert_eq!(req.pos, Some(cursor_pos));
    });

    // Client task sends x-peercast-pos
    let client_handle = tokio::spawn(async move {
        let mut socket = TcpStream::connect(server_addr).await.unwrap();
        let mut client = RelayClientStateMachine::new(
            channel_id,
            client_sid,
            RelayNetworkProfile::IPv4,
            cursor_pos,
        );

        let req_bytes =
            client.build_relay_request(&server_addr.to_string(), "PeerCastRustTest/1.0");
        let req_str = String::from_utf8(req_bytes.clone()).unwrap();
        assert!(req_str.contains("x-peercast-pos: 1250000\r\n"));

        socket.write_all(&req_bytes).await.unwrap();
    });

    tokio::try_join!(server_handle, client_handle).unwrap();
}

#[tokio::test]
async fn test_p21_loopback_reverse_probe() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target_addr = listener.local_addr().unwrap();

    let server_sid = sample_server_sid();
    let peer_sid = sample_client_sid();

    // Mock peer server answering probe with matching session ID
    let peer_handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = BytesMut::with_capacity(1024);
        let mut tmp = [0u8; 512];

        // 1. Read preamble (pcp\n + helo)
        let budget = peercast_core::atom::AtomBudget::default();
        let (found_pcp, found_helo) = loop {
            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            buf.extend_from_slice(&tmp[..n]);
            let mut freeze = buf.clone().freeze();
            if let Some(p) = Atom::decode(&mut freeze, &budget).unwrap() {
                if let Some(h) = Atom::decode(&mut freeze, &budget).unwrap() {
                    break (p, h);
                }
            }
        };

        assert_eq!(found_pcp.name(), ID4::PCP_CONNECT);
        assert_eq!(found_helo.name(), ID4::PCP_HELO);

        // 2. Respond with oleh carrying peer_sid
        let oleh = Atom::new_parent(
            ID4::PCP_OLEH,
            vec![
                Atom::new_string(ID4::HELO_AGENT, "PeerCastPeer/1.0"),
                Atom::new_leaf(
                    ID4::HELO_SESSIONID,
                    Bytes::copy_from_slice(peer_sid.as_bytes()),
                ),
                Atom::new_u32_le(ID4::HELO_VERSION, SERVANT_VERSION),
            ],
        );
        let mut out = Vec::new();
        oleh.encode(&mut out).unwrap();
        socket.write_all(&out).await.unwrap();

        // 3. Read quit(1000)
        let mut q_buf = BytesMut::with_capacity(256);
        let q_atom = loop {
            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            q_buf.extend_from_slice(&tmp[..n]);
            let mut freeze = q_buf.clone().freeze();
            if let Some(q) = Atom::decode(&mut freeze, &budget).unwrap() {
                break q;
            }
        };
        assert_eq!(q_atom.name(), ID4::PCP_QUIT);
        assert_eq!(q_atom.get_u32_le().unwrap(), QuitReason::NORMAL.raw());
    });

    let probe_handle = tokio::spawn(async move {
        let limiter = ProbeLimiter::new(ProbeLimits::default());
        let res = execute_reverse_probe(
            &limiter,
            target_addr.ip(),
            target_addr.port(),
            peer_sid,
            server_sid,
            "PeerCastRustServer/1.0",
        )
        .await
        .unwrap();

        assert_eq!(
            res,
            ProbeResult::Reachable {
                session_id: peer_sid,
                target_addr,
            }
        );
    });

    tokio::try_join!(peer_handle, probe_handle).unwrap();
}
