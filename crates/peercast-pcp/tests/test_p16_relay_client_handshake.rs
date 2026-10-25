use bytes::Bytes;
use peercast_core::atom::Atom;
use peercast_core::channel_id::ChannelId;
use peercast_core::errors::QuitReason;
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
fn test_p16_full_200_handshake_success_and_ok_validation() {
    let cid = sample_channel_id();
    let sid = sample_session_id();
    let mut sm = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0);

    // 1. Initial request generation
    let req_bytes = sm.build_relay_request("localhost:7144", "PeerCastStation/Test");
    let req_str = String::from_utf8(req_bytes).unwrap();
    assert!(req_str.starts_with("GET /channel/0123456789ABCDEF0123456789ABCDEF HTTP/1.0\r\n"));
    assert!(req_str.contains("x-peercast-pcp: 1\r\n"));
    assert!(!req_str.contains("pcnt"), "Must never contain pcnt");
    assert!(
        !req_str.contains("pcp\n"),
        "Must never contain raw pcp newline in HTTP request"
    );
    assert_eq!(sm.phase(), RelayClientPhase::WaitingResponse);

    // 2. Server sends 200 OK prelude
    let mut buf = Bytes::from_static(b"HTTP/1.0 200 OK\r\nServer: PeerCast\r\n\r\n");
    let ev1 = sm.feed_bytes(&mut buf).unwrap().expect("Event 200");
    assert_eq!(ev1, RelayEvent::Connected200);
    assert_eq!(sm.phase(), RelayClientPhase::WaitingOleh200);

    // 3. Client sends helo
    let helo_bytes = sm.build_helo("PeerCastStation/Test", Some(7144)).unwrap();
    assert!(!helo_bytes.is_empty());

    // 4. Server sends oleh
    let server_sid = ChannelId::from_hex("11112222333344445555666677778888").unwrap();
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
    let mut oleh_buf = oleh.to_bytes().unwrap();
    let ev2 = sm.feed_bytes(&mut oleh_buf).unwrap().expect("Event oleh");
    assert!(matches!(
        ev2,
        RelayEvent::HandshakeCompleted { remote_session_id, .. } if remote_session_id == Some(server_sid)
    ));
    assert_eq!(sm.phase(), RelayClientPhase::WaitingOk);

    // 5. Server sends ok(1)
    let ok_atom = Atom::new_u32_le(ID4::PCP_OK, 1);
    let mut ok_buf = ok_atom.to_bytes().unwrap();
    let ev3 = sm.feed_bytes(&mut ok_buf).unwrap();
    assert_eq!(ev3, None); // ok(1) consumed internally to enter Streaming
    assert_eq!(sm.phase(), RelayClientPhase::Streaming);
}

#[test]
fn test_p16_rejects_invalid_ok_values() {
    let cid = sample_channel_id();
    let sid = sample_session_id();
    let mut sm = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0);

    // Bootstrap directly to WaitingOk
    let mut buf_200 = Bytes::from_static(b"HTTP/1.0 200 OK\r\n\r\n");
    sm.feed_bytes(&mut buf_200).unwrap();

    let oleh = Atom::new_empty_parent(ID4::PCP_OLEH);
    let mut buf_oleh = oleh.to_bytes().unwrap();
    sm.feed_bytes(&mut buf_oleh).unwrap();
    assert_eq!(sm.phase(), RelayClientPhase::WaitingOk);

    // Server sends invalid ok value 2 (must be 1)
    let bad_ok = Atom::new_u32_le(ID4::PCP_OK, 2);
    let mut bad_buf = bad_ok.to_bytes().unwrap();
    let err = sm.feed_bytes(&mut bad_buf).unwrap_err();
    assert_eq!(err, RelayError::InvalidOkValue(2));
    assert!(sm.phase().is_terminal());
}

#[test]
fn test_p16_control_messages_interleaved_with_media_stream() {
    let cid = sample_channel_id();
    let sid = sample_session_id();
    let mut sm = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0);

    // Advance to Streaming phase
    let mut buf_200 = Bytes::from_static(b"HTTP/1.0 200 OK\r\n\r\n");
    sm.feed_bytes(&mut buf_200).unwrap();
    let oleh = Atom::new_empty_parent(ID4::PCP_OLEH);
    let mut buf_oleh = oleh.to_bytes().unwrap();
    sm.feed_bytes(&mut buf_oleh).unwrap();
    let ok = Atom::new_u32_le(ID4::PCP_OK, 1);
    let mut buf_ok = ok.to_bytes().unwrap();
    sm.feed_bytes(&mut buf_ok).unwrap();
    assert_eq!(sm.phase(), RelayClientPhase::Streaming);

    // 1. First: Media Packet atom
    let pkt1 = Atom::new_parent(
        ID4::CHAN_PKT,
        vec![
            Atom::new_u32_le(ID4::PKT_POS, 100),
            Atom::new_leaf(ID4::PKT_DATA, vec![0x11; 64]),
        ],
    );

    // 2. Interleaved: Host candidate atom (REQ-032: Must NOT fail if not media!)
    let host_atom = Atom::new_parent(ID4::PCP_HOST, vec![Atom::new_u32_le(ID4::HOST_PORT, 7144)]);

    // 3. Interleaved: Broadcast atom
    let bcst_atom = Atom::new_parent(ID4::PCP_BCST, vec![Atom::new_u8(ID4::BCST_TTL, 5)]);

    // 4. Second: Media Packet atom
    let pkt2 = Atom::new_parent(
        ID4::CHAN_PKT,
        vec![
            Atom::new_u32_le(ID4::PKT_POS, 164),
            Atom::new_leaf(ID4::PKT_TYPE, ID4::PKT_TYPE_HEAD.as_bytes().to_vec()),
            Atom::new_leaf(ID4::PKT_DATA, vec![0x22; 64]),
        ],
    );

    let pkt3 = Atom::new_parent(
        ID4::CHAN_PKT,
        vec![
            Atom::new_u32_le(ID4::PKT_POS, 228),
            Atom::new_leaf(ID4::PKT_HEAD, vec![0x33; 16]),
        ],
    );

    // Combine all into a single continuous stream buffer
    let mut stream_buf = Vec::new();
    pkt1.encode(&mut stream_buf).unwrap();
    host_atom.encode(&mut stream_buf).unwrap();
    bcst_atom.encode(&mut stream_buf).unwrap();
    pkt2.encode(&mut stream_buf).unwrap();
    pkt3.encode(&mut stream_buf).unwrap();

    let mut buf = Bytes::from(stream_buf);

    // Read item 1: MediaChunk
    let ev1 = sm.feed_bytes(&mut buf).unwrap().expect("Pkt1");
    assert!(matches!(ev1, RelayEvent::MediaChunk(c) if c.pos == 100 && c.len() == 64));

    // Read item 2: Host (interleaved control)
    let ev2 = sm.feed_bytes(&mut buf).unwrap().expect("Host");
    assert!(matches!(ev2, RelayEvent::Host(_)));

    // Read item 3: Broadcast (interleaved control)
    let ev3 = sm.feed_bytes(&mut buf).unwrap().expect("Broadcast");
    assert!(matches!(ev3, RelayEvent::Broadcast(_)));

    // Read item 4: header chunks remain distinct from media.
    let ev4 = sm.feed_bytes(&mut buf).unwrap().expect("Pkt2");
    assert!(matches!(ev4, RelayEvent::HeaderChunk(c) if c.pos == 164 && c.len() == 64));

    let ev5 = sm.feed_bytes(&mut buf).unwrap().expect("Pkt3");
    assert!(matches!(ev5, RelayEvent::HeaderChunk(c) if c.pos == 228 && c.len() == 16));

    assert!(buf.is_empty(), "All interleaved atoms consumed cleanly");
}

#[test]
fn test_p16_503_unavailable_path_through_helo_oleh() {
    let cid = sample_channel_id();
    let sid = sample_session_id();
    let mut sm = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0);

    // 1. Server sends 503 Service Unavailable
    let mut buf_503 = Bytes::from_static(b"HTTP/1.0 503 Service Unavailable\r\n\r\n");
    let ev1 = sm.feed_bytes(&mut buf_503).unwrap().expect("Event 503");
    assert_eq!(ev1, RelayEvent::Unavailable503);
    assert!(sm.is_503_mode());
    assert_eq!(sm.phase(), RelayClientPhase::WaitingOleh503);

    // 2. Server sends oleh
    let oleh = Atom::new_empty_parent(ID4::PCP_OLEH);
    let mut buf_oleh = oleh.to_bytes().unwrap();
    let ev2 = sm.feed_bytes(&mut buf_oleh).unwrap().expect("Oleh event");
    assert!(matches!(ev2, RelayEvent::HandshakeCompleted { .. }));

    // In 503 mode, skips WaitingOk and enters CollectingHosts503 phase
    assert_eq!(sm.phase(), RelayClientPhase::CollectingHosts503);

    // 3. Server sends host candidate
    let host = Atom::new_empty_parent(ID4::PCP_HOST);
    let mut buf_host = host.to_bytes().unwrap();
    let ev3 = sm.feed_bytes(&mut buf_host).unwrap().expect("Host event");
    assert!(matches!(ev3, RelayEvent::Host(_)));

    // 4. Server terminates with quit(1003) (Unavailable)
    let quit = Atom::new_u32_le(ID4::PCP_QUIT, QuitReason::UNAVAILABLE.raw());
    let mut buf_quit = quit.to_bytes().unwrap();
    let ev4 = sm.feed_bytes(&mut buf_quit).unwrap().expect("Quit event");
    assert_eq!(ev4, RelayEvent::Quit(QuitReason::UNAVAILABLE));
    assert_eq!(
        sm.phase(),
        RelayClientPhase::Terminal(QuitReason::UNAVAILABLE)
    );
}

#[test]
fn test_p16_404_stops_immediately_without_atom_parsing() {
    let cid = sample_channel_id();
    let sid = sample_session_id();
    let mut sm = RelayClientStateMachine::new(cid, sid, RelayNetworkProfile::IPv4, 0);

    let mut buf_404 = Bytes::from_static(b"HTTP/1.0 404 Not Found\r\n\r\n");
    let res = sm.feed_bytes(&mut buf_404);
    assert_eq!(res, Err(RelayError::OffAir));
    assert_eq!(sm.phase(), RelayClientPhase::Terminal(QuitReason::OFF_AIR));
}
