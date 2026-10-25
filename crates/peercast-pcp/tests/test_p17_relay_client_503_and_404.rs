use bytes::Bytes;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use peercast_core::atom::Atom;
use peercast_core::channel_id::ChannelId;
use peercast_core::errors::QuitReason;
use peercast_core::host::{Host, HostFlags1};
use peercast_core::id4::ID4;
use peercast_pcp::relay_client::{
    RelayClientPhase, RelayClientStateMachine, RelayError, RelayEvent, RelayNetworkProfile,
};

fn sample_channel_id() -> ChannelId {
    ChannelId::from_hex("0123456789abcdef0123456789abcdef").unwrap()
}

fn sample_session_id() -> ChannelId {
    ChannelId::from_hex("fedcba9876543210fedcba9876543210").unwrap()
}

#[test]
fn test_p17_503_full_transcript_ordered() {
    let cid = sample_channel_id();
    let sid = sample_session_id();
    let mut sm = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0);

    // 1. Initial request
    let req = sm.build_relay_request("relay.peercast.local:7144", "PeerCastStation/1.0");
    assert!(!req.is_empty());
    assert_eq!(sm.phase(), RelayClientPhase::WaitingResponse);

    // 2. Server sends 503 Service Unavailable
    let mut buf_503 = Bytes::from_static(b"HTTP/1.0 503 Service Unavailable\r\n\r\n");
    let ev1 = sm.feed_bytes(&mut buf_503).unwrap().expect("503 prelude");
    assert_eq!(ev1, RelayEvent::Unavailable503);
    assert_eq!(sm.phase(), RelayClientPhase::WaitingOleh503);
    assert!(sm.is_503_mode());

    // 3. Client sends helo
    let helo_bytes = sm.build_helo("PeerCastStation/1.0", Some(7144)).unwrap();
    assert!(!helo_bytes.is_empty());

    // 4. Server responds with oleh
    let server_sid = ChannelId::from_hex("11111111111111111111111111111111").unwrap();
    let oleh = Atom::new_parent(
        ID4::PCP_OLEH,
        vec![
            Atom::new_leaf(
                ID4::HELO_SESSIONID,
                Bytes::copy_from_slice(server_sid.as_bytes()),
            ),
            Atom::new_u16_le(ID4::HELO_PORT, 7144),
        ],
    );
    let mut buf_oleh = oleh.to_bytes().unwrap();
    let ev2 = sm.feed_bytes(&mut buf_oleh).unwrap().expect("oleh event");
    assert!(matches!(
        ev2,
        RelayEvent::HandshakeCompleted { remote_session_id, .. } if remote_session_id == Some(server_sid)
    ));
    assert_eq!(sm.phase(), RelayClientPhase::CollectingHosts503);

    // 5. Server sends 3 candidate host atoms
    let cand1_sid = ChannelId::from_hex("22222222222222222222222222222222").unwrap();
    let mut host1 = Host::new(cand1_sid, cid);
    host1.global_endpoint = Some(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)),
        7144,
    ));
    host1.flags = HostFlags1::RELAY;

    let cand2_sid = ChannelId::from_hex("33333333333333333333333333333333").unwrap();
    let mut host2 = Host::new(cand2_sid, cid);
    host2.global_endpoint = Some(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2)),
        7145,
    ));
    host2.flags = HostFlags1::RELAY;

    let cand3_sid = ChannelId::from_hex("44444444444444444444444444444444").unwrap();
    let mut host3 = Host::new(cand3_sid, cid);
    host3.global_endpoint = Some(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(198, 51, 100, 3)),
        7146,
    ));
    host3.flags = HostFlags1::RELAY;

    let mut stream_buf = Vec::new();
    host1.to_atom().encode(&mut stream_buf).unwrap();
    host2.to_atom().encode(&mut stream_buf).unwrap();
    host3.to_atom().encode(&mut stream_buf).unwrap();

    // 6. Server sends quit(1003) (UNAVAILABLE)
    let quit = Atom::new_u32_le(ID4::PCP_QUIT, QuitReason::UNAVAILABLE.raw());
    quit.encode(&mut stream_buf).unwrap();

    let mut buf = Bytes::from(stream_buf);

    // Consume host 1
    let ev_h1 = sm.feed_bytes(&mut buf).unwrap().expect("Host 1");
    assert!(matches!(ev_h1, RelayEvent::Host(_)));

    // Consume host 2
    let ev_h2 = sm.feed_bytes(&mut buf).unwrap().expect("Host 2");
    assert!(matches!(ev_h2, RelayEvent::Host(_)));

    // Consume host 3
    let ev_h3 = sm.feed_bytes(&mut buf).unwrap().expect("Host 3");
    assert!(matches!(ev_h3, RelayEvent::Host(_)));

    // Consume quit
    let ev_q = sm.feed_bytes(&mut buf).unwrap().expect("Quit");
    assert_eq!(ev_q, RelayEvent::Quit(QuitReason::UNAVAILABLE));
    assert_eq!(
        sm.phase(),
        RelayClientPhase::Terminal(QuitReason::UNAVAILABLE)
    );
    assert!(buf.is_empty(), "All bytes consumed");

    // Verify collected candidates
    let candidates = sm.candidate_hosts();
    assert_eq!(candidates.len(), 3);
    assert_eq!(candidates[0].session_id, cand1_sid);
    assert_eq!(candidates[0].contact_address(), host1.contact_address());
    assert_eq!(candidates[1].session_id, cand2_sid);
    assert_eq!(candidates[2].session_id, cand3_sid);
}

#[test]
fn test_p17_503_empty_candidates_terminates_cleanly() {
    let cid = sample_channel_id();
    let sid = sample_session_id();
    let mut sm = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0);

    let mut buf = Bytes::from_static(b"HTTP/1.0 503 Service Unavailable\r\n\r\n");
    sm.feed_bytes(&mut buf).unwrap();

    let oleh = Atom::new_empty_parent(ID4::PCP_OLEH);
    let mut buf_oleh = oleh.to_bytes().unwrap();
    sm.feed_bytes(&mut buf_oleh).unwrap();
    assert_eq!(sm.phase(), RelayClientPhase::CollectingHosts503);

    // Server sends immediate quit(1003) without any host atoms
    let quit = Atom::new_u32_le(ID4::PCP_QUIT, QuitReason::UNAVAILABLE.raw());
    let mut buf_quit = quit.to_bytes().unwrap();
    let ev = sm.feed_bytes(&mut buf_quit).unwrap().expect("Quit event");
    assert_eq!(ev, RelayEvent::Quit(QuitReason::UNAVAILABLE));
    assert_eq!(
        sm.phase(),
        RelayClientPhase::Terminal(QuitReason::UNAVAILABLE)
    );

    // Candidates list is clean and empty
    assert!(sm.candidate_hosts().is_empty());
}

#[test]
fn test_p17_503_filters_invalid_candidates() {
    let cid = sample_channel_id();
    let sid = sample_session_id();
    let mut sm = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0);

    let mut buf_prelude = Bytes::from_static(b"HTTP/1.0 503 Service Unavailable\r\n\r\n");
    sm.feed_bytes(&mut buf_prelude).unwrap();
    let oleh = Atom::new_empty_parent(ID4::PCP_OLEH);
    let mut buf_oleh = oleh.to_bytes().unwrap();
    sm.feed_bytes(&mut buf_oleh).unwrap();

    let mut stream_buf = Vec::new();

    // 1. Valid host
    let mut valid_host1 = Host::new(
        ChannelId::from_hex("11111111111111111111111111111111").unwrap(),
        cid,
    );
    valid_host1.global_endpoint = Some(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)),
        7144,
    ));
    valid_host1.to_atom().encode(&mut stream_buf).unwrap();

    // 2. Invalid port (port 0)
    let bad_port_atom = Atom::new_parent(
        ID4::PCP_HOST,
        vec![
            Atom::new_leaf(ID4::HOST_ID, vec![0x22; 16]),
            Atom::new_ip(ID4::HOST_IP, IpAddr::V4(Ipv4Addr::new(198, 51, 100, 2))),
            Atom::new_u16_le(ID4::HOST_PORT, 0), // port 0 is invalid
        ],
    );
    bad_port_atom.encode(&mut stream_buf).unwrap();

    // 3. Unspecified IP (0.0.0.0)
    let bad_ip_atom = Atom::new_parent(
        ID4::PCP_HOST,
        vec![
            Atom::new_leaf(ID4::HOST_ID, vec![0x33; 16]),
            Atom::new_ip(ID4::HOST_IP, IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))),
            Atom::new_u16_le(ID4::HOST_PORT, 7144),
        ],
    );
    bad_ip_atom.encode(&mut stream_buf).unwrap();

    // 4. Self session ID (matching client's sid -> loop prevention)
    let mut self_host = Host::new(sid, cid);
    self_host.global_endpoint = Some(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(198, 51, 100, 4)),
        7144,
    ));
    self_host.to_atom().encode(&mut stream_buf).unwrap();

    // 5. Valid host 2
    let mut valid_host2 = Host::new(
        ChannelId::from_hex("55555555555555555555555555555555").unwrap(),
        cid,
    );
    valid_host2.global_endpoint = Some(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 50)),
        7144,
    ));
    valid_host2.to_atom().encode(&mut stream_buf).unwrap();

    // 6. Terminate with quit(1003)
    let quit = Atom::new_u32_le(ID4::PCP_QUIT, QuitReason::UNAVAILABLE.raw());
    quit.encode(&mut stream_buf).unwrap();

    let mut buf = Bytes::from(stream_buf);

    // Consume all atoms
    while let Ok(Some(ev)) = sm.feed_bytes(&mut buf) {
        if matches!(ev, RelayEvent::Quit(_)) {
            break;
        }
    }

    // Only the 2 valid candidate hosts must be retained
    let candidates = sm.candidate_hosts();
    assert_eq!(candidates.len(), 2, "Only 2 valid candidates must be kept");
    assert_eq!(candidates[0].session_id, valid_host1.session_id);
    assert_eq!(candidates[1].session_id, valid_host2.session_id);
}

#[test]
fn test_p17_503_handles_unknown_quit_codes_predictably() {
    let cid = sample_channel_id();
    let sid = sample_session_id();
    let mut sm = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0);

    let mut buf_prelude = Bytes::from_static(b"HTTP/1.0 503 Service Unavailable\r\n\r\n");
    sm.feed_bytes(&mut buf_prelude).unwrap();
    let oleh = Atom::new_empty_parent(ID4::PCP_OLEH);
    let mut buf_oleh = oleh.to_bytes().unwrap();
    sm.feed_bytes(&mut buf_oleh).unwrap();

    // Server sends quit(4000) (observed Station QUIT+READ base)
    let quit_4000 = Atom::new_u32_le(ID4::PCP_QUIT, 4000);
    let mut buf_quit = quit_4000.to_bytes().unwrap();
    let ev = sm.feed_bytes(&mut buf_quit).unwrap().expect("Quit 4000");

    let expected_reason = QuitReason::from_raw(4000);
    assert_eq!(ev, RelayEvent::Quit(expected_reason));
    assert_eq!(expected_reason.raw(), 4000, "Raw code must be preserved");
    assert_eq!(sm.phase(), RelayClientPhase::Terminal(expected_reason));
}

#[test]
fn test_p17_404_and_malformed_statuses() {
    let cid = sample_channel_id();
    let sid = sample_session_id();

    // 404 Not Found (off-air)
    let mut sm1 = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0);
    let mut buf_404 = Bytes::from_static(b"HTTP/1.0 404 Not Found\r\n\r\n");
    let res1 = sm1.feed_bytes(&mut buf_404);
    assert_eq!(res1, Err(RelayError::OffAir));
    assert_eq!(sm1.phase(), RelayClientPhase::Terminal(QuitReason::OFF_AIR));

    // 500 Internal Server Error
    let mut sm2 = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0);
    let mut buf_500 = Bytes::from_static(b"HTTP/1.0 500 Internal Server Error\r\n\r\n");
    let res2 = sm2.feed_bytes(&mut buf_500);
    assert!(matches!(res2, Err(RelayError::Http(_))));
    assert_eq!(
        sm2.phase(),
        RelayClientPhase::Terminal(QuitReason::BAD_RESPONSE)
    );
}

#[test]
fn test_p17_candidate_capacity_bound() {
    let cid = sample_channel_id();
    let sid = sample_session_id();
    let mut sm = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0)
        .with_max_candidate_hosts(7);

    let mut buf_prelude = Bytes::from_static(b"HTTP/1.0 503 Service Unavailable\r\n\r\n");
    sm.feed_bytes(&mut buf_prelude).unwrap();
    let oleh = Atom::new_empty_parent(ID4::PCP_OLEH);
    let mut buf_oleh = oleh.to_bytes().unwrap();
    sm.feed_bytes(&mut buf_oleh).unwrap();

    // Send more candidates than the configured cap.
    let mut stream_buf = Vec::new();
    for i in 1..=100u32 {
        let mut h = Host::new(ChannelId::from_bytes([i as u8; 16]), cid);
        h.global_endpoint = Some(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, (i % 250 + 1) as u8)),
            7144,
        ));
        h.to_atom().encode(&mut stream_buf).unwrap();
    }
    let quit = Atom::new_u32_le(ID4::PCP_QUIT, QuitReason::UNAVAILABLE.raw());
    quit.encode(&mut stream_buf).unwrap();

    let mut buf = Bytes::from(stream_buf);
    while let Ok(Some(ev)) = sm.feed_bytes(&mut buf) {
        if matches!(ev, RelayEvent::Quit(_)) {
            break;
        }
    }

    assert_eq!(sm.candidate_hosts().len(), 7);
}
