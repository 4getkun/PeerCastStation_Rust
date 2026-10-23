use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use peercast_core::atom::Atom;
use peercast_core::cache::FakeClock;
use peercast_core::channel_id::ChannelId;
use peercast_core::gossip::{
    select_forwarding_recipients, BroadcastGroup, GossipDropReason, GossipForwardDecision,
    GossipForwarder, PeerBroadcastTarget,
};
use peercast_core::id4::ID4;

#[allow(clippy::too_many_arguments)]
fn create_bcst_atom(
    ttl: u8,
    hops: u8,
    from: ChannelId,
    dest: Option<ChannelId>,
    group: u8,
    channel_id: ChannelId,
    payload_tag: ID4,
    payload_data: &[u8],
) -> Atom {
    let mut children = vec![
        Atom::new_u8(ID4::BCST_TTL, ttl),
        Atom::new_u8(ID4::BCST_HOPS, hops),
        Atom::new_leaf(ID4::BCST_FROM, Bytes::copy_from_slice(from.as_bytes())),
        Atom::new_u8(ID4::BCST_GRP, group),
        Atom::new_leaf(ID4::BCST_CID, Bytes::copy_from_slice(channel_id.as_bytes())),
    ];

    if let Some(d) = dest {
        children.push(Atom::new_leaf(
            ID4::BCST_DEST,
            Bytes::copy_from_slice(d.as_bytes()),
        ));
    }

    children.push(Atom::new_leaf(
        payload_tag,
        Bytes::copy_from_slice(payload_data),
    ));

    Atom::new_parent(ID4::PCP_BCST, children)
}

fn extract_u8_child(atom: &Atom, tag: ID4) -> Option<u8> {
    atom.children()?
        .iter()
        .find(|c| c.name() == tag)?
        .get_u8()
        .ok()
}

/// Acceptance criterion 1:
/// Verify exact semantics: TTL is decremented and Hops are INCREMENTED.
#[test]
fn test_p33_ttl_decrement_and_hop_increment_exact_semantics() {
    let self_sid = ChannelId::from_bytes([0xEE; 16]);
    let from_sid = ChannelId::from_bytes([0x01; 16]);
    let cid = ChannelId::from_bytes([0xAA; 16]);

    let clock = Arc::new(FakeClock::new(0));
    let mut forwarder = GossipForwarder::with_clock(
        self_sid,
        2, // min_forwarding_ttl = 2 (Station requires ttl > 1)
        512,
        Duration::from_secs(60),
        clock,
    );

    // Incoming: TTL = 10, Hops = 2
    let incoming = create_bcst_atom(
        10,
        2,
        from_sid,
        None, // Broadcast to all
        BroadcastGroup::ALL.raw(),
        cid,
        ID4::PCP_CHAN,
        b"channel_info_payload",
    );

    let decision = forwarder.evaluate_incoming_bcst(&incoming);
    assert!(decision.process_locally);
    assert!(decision.drop_reason.is_none());
    assert!(decision.forward_packet.is_some());

    let fwd = decision.forward_packet.unwrap();
    let new_ttl = extract_u8_child(&fwd, ID4::BCST_TTL).unwrap();
    let new_hops = extract_u8_child(&fwd, ID4::BCST_HOPS).unwrap();

    // CRITICAL REQ-044 STOP/BLOCK CHECK:
    // TTL decremented: 10 -> 9
    // Hops INCREMENTED: 2 -> 3 (Never decrement hops!)
    assert_eq!(new_ttl, 9, "TTL must be decremented by 1");
    assert_eq!(new_hops, 3, "Hops must be INCREMENTED by 1");
}

/// Acceptance criterion 2:
/// TTL boundary conditions: TTL = 0, TTL = 1, TTL = 2.
#[test]
fn test_p33_ttl_thresholds_0_1_2() {
    let self_sid = ChannelId::from_bytes([0xEE; 16]);
    let from_sid = ChannelId::from_bytes([0x01; 16]);
    let cid = ChannelId::from_bytes([0xAA; 16]);

    let clock = Arc::new(FakeClock::new(0));
    let mut forwarder = GossipForwarder::with_clock(
        self_sid,
        2, // Station requires incoming ttl > 1 (i.e. >= 2)
        512,
        Duration::from_secs(60),
        clock,
    );

    // Case 1: TTL = 0 -> Expired, cannot forward
    let bcst_ttl0 = create_bcst_atom(0, 5, from_sid, None, BroadcastGroup::ALL.raw(), cid, ID4::PCP_CHAN, b"data0");
    let dec0 = forwarder.evaluate_incoming_bcst(&bcst_ttl0);
    assert_eq!(
        dec0.drop_reason,
        Some(GossipDropReason::ExpiredTtl { incoming_ttl: 0 })
    );
    assert!(dec0.forward_packet.is_none());

    // Case 2: TTL = 1 -> Expired for forwarding (Station requires ttl > 1)
    let bcst_ttl1 = create_bcst_atom(1, 5, from_sid, None, BroadcastGroup::ALL.raw(), cid, ID4::PCP_CHAN, b"data1");
    let dec1 = forwarder.evaluate_incoming_bcst(&bcst_ttl1);
    assert_eq!(
        dec1.drop_reason,
        Some(GossipDropReason::ExpiredTtl { incoming_ttl: 1 })
    );
    assert!(dec1.forward_packet.is_none());
    assert!(dec1.process_locally, "TTL=1 broadcast with no dest is still delivered locally");

    // Case 3: TTL = 2 -> Forwardable! New TTL = 1, New Hops = 6
    let bcst_ttl2 = create_bcst_atom(2, 5, from_sid, None, BroadcastGroup::ALL.raw(), cid, ID4::PCP_CHAN, b"data2");
    let dec2 = forwarder.evaluate_incoming_bcst(&bcst_ttl2);
    assert!(dec2.drop_reason.is_none());
    assert!(dec2.forward_packet.is_some());
    let fwd2 = dec2.forward_packet.unwrap();
    assert_eq!(extract_u8_child(&fwd2, ID4::BCST_TTL), Some(1));
    assert_eq!(extract_u8_child(&fwd2, ID4::BCST_HOPS), Some(6));
}

/// Acceptance criterion 3:
/// Hop overflow protection: Hops = 255 does not wrap around to 0.
#[test]
fn test_p33_hop_overflow_protection_at_255() {
    let self_sid = ChannelId::from_bytes([0xEE; 16]);
    let from_sid = ChannelId::from_bytes([0x01; 16]);
    let cid = ChannelId::from_bytes([0xAA; 16]);

    let clock = Arc::new(FakeClock::new(0));
    let mut forwarder = GossipForwarder::with_clock(
        self_sid,
        2,
        512,
        Duration::from_secs(60),
        clock,
    );

    // Incoming with hops = 255 (maximum u8)
    let incoming = create_bcst_atom(
        5,
        255,
        from_sid,
        None,
        BroadcastGroup::ALL.raw(),
        cid,
        ID4::PCP_CHAN,
        b"data_hop_max",
    );

    let decision = forwarder.evaluate_incoming_bcst(&incoming);
    assert!(decision.forward_packet.is_some());
    let fwd = decision.forward_packet.unwrap();

    let new_hops = extract_u8_child(&fwd, ID4::BCST_HOPS).unwrap();
    // Must saturate at 255 and NOT overflow/wrap to 0!
    assert_eq!(
        new_hops, 255,
        "Hops must saturate at 255 to prevent overflow wrap-around to 0"
    );
}

/// Acceptance criterion 4:
/// Local destination is processed locally and NOT forwarded.
#[test]
fn test_p33_local_destination_not_forwarded() {
    let self_sid = ChannelId::from_bytes([0xEE; 16]);
    let from_sid = ChannelId::from_bytes([0x01; 16]);
    let other_sid = ChannelId::from_bytes([0x22; 16]);
    let cid = ChannelId::from_bytes([0xAA; 16]);

    let clock = Arc::new(FakeClock::new(0));
    let mut forwarder = GossipForwarder::with_clock(
        self_sid,
        2,
        512,
        Duration::from_secs(60),
        clock,
    );

    // 1. Specifically addressed to self: processed locally, NEVER forwarded
    let for_self = create_bcst_atom(
        10,
        0,
        from_sid,
        Some(self_sid),
        BroadcastGroup::ALL.raw(),
        cid,
        ID4::PCP_CHAN,
        b"for_self_only",
    );
    let dec_self = forwarder.evaluate_incoming_bcst(&for_self);
    assert!(dec_self.process_locally);
    assert!(dec_self.forward_packet.is_none());
    assert_eq!(dec_self.drop_reason, Some(GossipDropReason::LocalDestination));

    // 2. Specifically addressed to another node: NOT processed locally, BUT forwarded
    let for_other = create_bcst_atom(
        10,
        0,
        from_sid,
        Some(other_sid),
        BroadcastGroup::ALL.raw(),
        cid,
        ID4::PCP_CHAN,
        b"for_other_only",
    );
    let dec_other = forwarder.evaluate_incoming_bcst(&for_other);
    assert!(!dec_other.process_locally, "Not addressed to us; do not process locally");
    assert!(dec_other.forward_packet.is_some(), "Addressed to remote node; eligible to forward");
    assert!(dec_other.drop_reason.is_none());
}

/// Acceptance criterion 5:
/// Reflection loop prevention: packet originated from self is dropped.
#[test]
fn test_p33_reflection_loop_prevention() {
    let self_sid = ChannelId::from_bytes([0xEE; 16]);
    let cid = ChannelId::from_bytes([0xAA; 16]);

    let clock = Arc::new(FakeClock::new(0));
    let mut forwarder = GossipForwarder::with_clock(
        self_sid,
        2,
        512,
        Duration::from_secs(60),
        clock,
    );

    // Echo/reflection of our own broadcast: from == self_sid
    let own_echo = create_bcst_atom(
        10,
        1,
        self_sid, // From ourselves!
        None,
        BroadcastGroup::ALL.raw(),
        cid,
        ID4::PCP_CHAN,
        b"echoed_data",
    );

    let dec = forwarder.evaluate_incoming_bcst(&own_echo);
    assert!(!dec.process_locally);
    assert!(dec.forward_packet.is_none());
    assert_eq!(dec.drop_reason, Some(GossipDropReason::OwnReflectionLoop));
}

/// Acceptance criterion 6:
/// Unknown or malformed broadcast group handling.
#[test]
fn test_p33_unknown_broadcast_group() {
    let self_sid = ChannelId::from_bytes([0xEE; 16]);
    let from_sid = ChannelId::from_bytes([0x01; 16]);
    let cid = ChannelId::from_bytes([0xAA; 16]);

    let clock = Arc::new(FakeClock::new(0));
    let mut forwarder = GossipForwarder::with_clock(
        self_sid,
        2,
        512,
        Duration::from_secs(60),
        clock,
    );

    // Group = 0 (empty/invalid)
    let bcst_g0 = create_bcst_atom(5, 0, from_sid, None, 0, cid, ID4::PCP_CHAN, b"g0");
    let dec0 = forwarder.evaluate_incoming_bcst(&bcst_g0);
    assert_eq!(dec0.drop_reason, Some(GossipDropReason::UnknownGroup(0)));

    // Group = 0x80 (undefined upper bits)
    let bcst_g80 = create_bcst_atom(5, 0, from_sid, None, 0x80, cid, ID4::PCP_CHAN, b"g80");
    let dec80 = forwarder.evaluate_incoming_bcst(&bcst_g80);
    assert_eq!(dec80.drop_reason, Some(GossipDropReason::UnknownGroup(0x80)));

    // Valid group = 4 (Relays)
    let bcst_relays = create_bcst_atom(5, 0, from_sid, None, BroadcastGroup::RELAYS.raw(), cid, ID4::PCP_CHAN, b"relays");
    let dec_relays = forwarder.evaluate_incoming_bcst(&bcst_relays);
    assert!(dec_relays.forward_packet.is_some());
    assert_eq!(dec_relays.target_groups, BroadcastGroup::RELAYS);
}

/// Acceptance criterion 7:
/// Storm suppression deduplication and bounded memory cache (REQ-044, Global Rule 3).
#[test]
fn test_p33_storm_suppression_and_bounded_dedup_cache() {
    let clock = Arc::new(FakeClock::new(0));
    let self_sid = ChannelId::from_bytes([0xEE; 16]);
    let from_sid = ChannelId::from_bytes([0x01; 16]);
    let cid = ChannelId::from_bytes([0xAA; 16]);

    let mut forwarder = GossipForwarder::with_clock(
        self_sid,
        2,
        3, // max_dedup_entries = 3 (strictly bounded!)
        Duration::from_secs(60), // 60s storm suppression window
        clock.clone(),
    );

    let packet_a = create_bcst_atom(5, 0, from_sid, None, BroadcastGroup::ALL.raw(), cid, ID4::PCP_CHAN, b"unique_a");
    let packet_b = create_bcst_atom(5, 0, from_sid, None, BroadcastGroup::ALL.raw(), cid, ID4::PCP_CHAN, b"unique_b");
    let packet_c = create_bcst_atom(5, 0, from_sid, None, BroadcastGroup::ALL.raw(), cid, ID4::PCP_CHAN, b"unique_c");
    let packet_d = create_bcst_atom(5, 0, from_sid, None, BroadcastGroup::ALL.raw(), cid, ID4::PCP_CHAN, b"unique_d");

    // 1. Packet A arrives at t=0: accepted and forwarded
    let dec_a1 = forwarder.evaluate_incoming_bcst(&packet_a);
    assert!(dec_a1.forward_packet.is_some());

    // 2. Duplicate Packet A arrives at t=10s: storm suppression DROPS it!
    clock.set(Duration::from_secs(10));
    let dec_a2 = forwarder.evaluate_incoming_bcst(&packet_a);
    assert_eq!(
        dec_a2.drop_reason,
        Some(GossipDropReason::DuplicateStormSuppressed)
    );
    assert!(dec_a2.forward_packet.is_none());

    // 3. Fill bounded cache to capacity (3 entries):
    clock.set(Duration::from_secs(20));
    forwarder.evaluate_incoming_bcst(&packet_b);
    clock.set(Duration::from_secs(30));
    forwarder.evaluate_incoming_bcst(&packet_c);
    assert_eq!(forwarder.dedup_cache().len(), 3);

    // 4. Packet D arrives at t=40s: capacity 3 is exceeded.
    // Must evict oldest entry (packet_a from t=0) to keep memory bounded!
    clock.set(Duration::from_secs(40));
    forwarder.evaluate_incoming_bcst(&packet_d);
    assert_eq!(
        forwarder.dedup_cache().len(),
        3,
        "Deduplication cache must remain bounded at max_entries"
    );

    // 5. Advance clock past suppression window (t=150s):
    // Physical pruning releases heap memory
    clock.set(Duration::from_secs(150));
    let pruned = forwarder.dedup_cache_mut().prune_expired();
    assert_eq!(pruned, 3);
    assert_eq!(forwarder.dedup_cache().len(), 0);
    assert!(forwarder.dedup_cache().is_empty());
}

/// Acceptance criterion 8:
/// Source exclusion in broadcast routing (never forward back to sender).
#[test]
fn test_p33_source_exclusion_routing() {
    let sid_src = ChannelId::from_bytes([0x01; 16]);
    let sid_b = ChannelId::from_bytes([0x02; 16]);
    let sid_c = ChannelId::from_bytes([0x03; 16]);

    let addr_src: SocketAddr = "1.1.1.1:7144".parse().unwrap();
    let addr_b: SocketAddr = "2.2.2.2:7144".parse().unwrap();
    let addr_c: SocketAddr = "3.3.3.3:7144".parse().unwrap();

    let peers = vec![
        PeerBroadcastTarget {
            session_id: Some(sid_src),
            address: addr_src,
            is_upstream: true, // Upstream sender
        },
        PeerBroadcastTarget {
            session_id: Some(sid_b),
            address: addr_b,
            is_upstream: false, // Downstream relay 1
        },
        PeerBroadcastTarget {
            session_id: Some(sid_c),
            address: addr_c,
            is_upstream: false, // Downstream relay 2
        },
    ];

    let decision = GossipForwardDecision {
        process_locally: true,
        forward_packet: Some(Atom::new_parent(ID4::PCP_BCST, vec![])),
        target_groups: BroadcastGroup::RELAYS,
        drop_reason: None,
    };

    // Forwarding with source peer specified:
    let recipients = select_forwarding_recipients(
        &decision,
        Some(&sid_src),
        Some(&addr_src),
        &peers,
    );

    // Sender (Peer A) MUST BE EXCLUDED!
    assert_eq!(recipients.len(), 2);
    assert!(!recipients.iter().any(|p| p.address == addr_src));
    assert!(recipients.iter().any(|p| p.address == addr_b));
    assert!(recipients.iter().any(|p| p.address == addr_c));
}
