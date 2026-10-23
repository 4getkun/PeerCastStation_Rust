use bytes::Bytes;
use std::time::Duration;

use peercast_core::atom::Atom;
use peercast_core::cache::PositionContinuity;
use peercast_core::channel_id::ChannelId;
use peercast_core::content::ContinuationFlags;
use peercast_core::id4::ID4;
use peercast_pcp::packet::{
    unwrap_wire_pos, ChannelPacket, ChannelPacketAdapter, PacketError, PacketIngestResult,
    PacketPayload, MAX_PACKET_DATALEN,
};

fn sample_cid() -> ChannelId {
    ChannelId::from_hex("0123456789abcdef0123456789abcdef").unwrap()
}

#[test]
fn test_p20_split_outgoing_body_safely_under_16384_bytes() {
    let cid = sample_cid();
    // 40,000 bytes fixture (exceeds single 16,384 byte packet limit)
    let original_fixture: Vec<u8> = (0..40_000).map(|i| (i % 251) as u8).collect();
    let fixture_bytes = Bytes::from(original_fixture.clone());

    let packet = ChannelPacket {
        channel_id: Some(cid),
        stream_type: ID4(*b"flv\0"),
        pos: 100_000,
        cont: ContinuationFlags::NONE,
        payload: PacketPayload::Data(fixture_bytes.clone()),
        unknown_extensions: vec![Atom::new_u32_le(ID4(*b"test"), 42)],
    };

    let atoms = packet.to_atoms().expect("Split to atoms");
    // 40,000 bytes split at 16,384:
    // Chunk 1: 16,384 bytes, pos: 100,000, cont: 0
    // Chunk 2: 16,384 bytes, pos: 116,384, cont: 1
    // Chunk 3: 7,232 bytes, pos: 132,768, cont: 1
    assert_eq!(atoms.len(), 3, "Must split into exactly 3 sub-packets");

    let mut reassembled = Vec::new();
    let mut expected_pos = 100_000i64;

    for (idx, atom) in atoms.iter().enumerate() {
        let parsed = ChannelPacket::from_atom(atom, Some(expected_pos)).unwrap();
        assert_eq!(parsed.stream_type, ID4(*b"flv\0"));
        assert_eq!(parsed.pos, expected_pos);

        if idx == 0 {
            assert_eq!(parsed.cont, ContinuationFlags::NONE);
            // Verify unknown extension attached to first packet
            assert_eq!(parsed.unknown_extensions.len(), 1);
            assert_eq!(parsed.unknown_extensions[0].name(), ID4(*b"test"));
        } else {
            // Subsequent sub-packets MUST have continuation flag set (cont = 1)
            assert_eq!(parsed.cont, ContinuationFlags(1));
        }

        match parsed.payload {
            PacketPayload::Data(d) => {
                assert!(d.len() <= MAX_PACKET_DATALEN, "Sub-packet data must be <= 16384");
                reassembled.extend_from_slice(&d);
                expected_pos += d.len() as i64;
            }
            _ => panic!("Expected Data payload"),
        }
    }

    // Crucial check: Reassembled fixture bytes MUST match original input bit-for-bit
    assert_eq!(reassembled, original_fixture, "Zero corruption or truncation across split packets");
}

#[test]
fn test_p20_inbound_packet_cap_rejection() {
    let cid = sample_cid();

    // 16,385 bytes payload (1 byte over hard cap)
    let oversized_data = vec![0xaa; MAX_PACKET_DATALEN + 1];
    let bad_atom = Atom::new_parent(
        ID4::PCP_CHAN,
        vec![
            Atom::new_leaf(ID4::CHAN_ID, Bytes::copy_from_slice(cid.as_bytes())),
            Atom::new_parent(
                ID4::CHAN_PKT,
                vec![
                    Atom::new_u32_le(ID4::PKT_POS, 0),
                    Atom::new_leaf(ID4::PKT_DATA, oversized_data),
                ],
            ),
        ],
    );

    let err = ChannelPacket::from_atom(&bad_atom, None).unwrap_err();
    assert_eq!(
        err,
        PacketError::PacketDataTooLarge {
            actual: MAX_PACKET_DATALEN + 1,
            max: MAX_PACKET_DATALEN,
        }
    );

    // Exactly 16,384 bytes payload is valid
    let boundary_data = vec![0xbb; MAX_PACKET_DATALEN];
    let valid_atom = Atom::new_parent(
        ID4::CHAN_PKT,
        vec![
            Atom::new_u32_le(ID4::PKT_POS, 0),
            Atom::new_leaf(ID4::PKT_DATA, boundary_data),
        ],
    );
    assert!(ChannelPacket::from_atom(&valid_atom, None).is_ok());
}

#[test]
fn test_p20_oversized_header_rejection() {
    let mut adapter = ChannelPacketAdapter::new(8 * 1024 * 1024, Duration::from_secs(3));

    // Header exceeding 128 KiB limit
    let oversized_header = vec![0x11; 200 * 1024];
    let pkt = ChannelPacket {
        channel_id: None,
        stream_type: ID4(*b"flv\0"),
        pos: 0,
        cont: ContinuationFlags::NONE,
        payload: PacketPayload::Header(Bytes::from(oversized_header)),
        unknown_extensions: Vec::new(),
    };

    let err = adapter.ingest_packet(pkt).unwrap_err();
    assert!(matches!(err, PacketError::HeaderTooLarge { actual: 204800, max: 131072 }));
}

#[test]
fn test_p20_32bit_position_rollover_unwrapping() {
    // 1. Initial position near 32-bit rollover boundary: 0xFFFF_F000
    let pos_near_max = 0xFFFF_F000u32;
    let unwrapped1 = unwrap_wire_pos(pos_near_max, None);
    assert_eq!(unwrapped1, 0xFFFF_F000i64);

    // 2. Next position rolled over forward past 0 to 0x0000_1000 (+8192 bytes later)
    let pos_rolled = 0x0000_1000u32;
    let unwrapped2 = unwrap_wire_pos(pos_rolled, Some(unwrapped1));
    assert_eq!(
        unwrapped2,
        0x1_0000_1000i64,
        "Rolled over wire position must become continuous 64-bit monotonic offset"
    );

    // 3. Backward jump check (e.g. small retransmission or jitter)
    let pos_jitter = 0x0000_0800u32;
    let unwrapped3 = unwrap_wire_pos(pos_jitter, Some(unwrapped2));
    assert_eq!(unwrapped3, 0x1_0000_0800i64);
}

#[test]
fn test_p20_relay_preserves_unknown_extensions() {
    let ext1 = Atom::new_u32_le(ID4(*b"xext"), 12345);
    let ext2 = Atom::new_string(ID4(*b"xstr"), "PeerCastCustomData");

    let original_pkt = ChannelPacket {
        channel_id: Some(sample_cid()),
        stream_type: ID4(*b"raw\0"),
        pos: 500,
        cont: ContinuationFlags::NONE,
        payload: PacketPayload::Data(Bytes::from_static(b"hello world payload")),
        unknown_extensions: vec![ext1.clone(), ext2.clone()],
    };

    let atoms = original_pkt.to_atoms().unwrap();
    assert_eq!(atoms.len(), 1);

    let parsed = ChannelPacket::from_atom(&atoms[0], None).unwrap();
    assert_eq!(parsed.unknown_extensions.len(), 2);
    assert_eq!(parsed.unknown_extensions[0], ext1);
    assert_eq!(parsed.unknown_extensions[1], ext2);
    assert_eq!(parsed.payload, original_pkt.payload);
}

#[test]
fn test_p20_header_epoch_invalidation_and_restart() {
    let mut adapter = ChannelPacketAdapter::new(8 * 1024 * 1024, Duration::from_secs(3));

    // 1. Establish initial stream header (Epoch 1)
    let head1 = ChannelPacket {
        channel_id: None,
        stream_type: ID4(*b"flv\0"),
        pos: 0,
        cont: ContinuationFlags::NONE,
        payload: PacketPayload::Header(Bytes::from_static(b"FLV_HEADER_1")),
        unknown_extensions: Vec::new(),
    };
    let res1 = adapter.ingest_packet(head1).unwrap();
    assert_eq!(res1, PacketIngestResult::HeaderReset { epoch: 1, pos: 0 });
    assert_eq!(adapter.current_epoch(), 1);

    // 2. Push body chunk for Epoch 1
    let body1 = ChannelPacket {
        channel_id: None,
        stream_type: ID4(*b"flv\0"),
        pos: 100,
        cont: ContinuationFlags::NONE,
        payload: PacketPayload::Data(Bytes::from_static(b"CHUNK_BODY_1")),
        unknown_extensions: Vec::new(),
    };
    let res2 = adapter.ingest_packet(body1).unwrap();
    assert!(matches!(res2, PacketIngestResult::BodyPushed { pos: 100, len: 12, .. }));

    // 3. Stream restarts with new metadata / header (Epoch 2)
    let head2 = ChannelPacket {
        channel_id: None,
        stream_type: ID4(*b"flv\0"),
        pos: 1000,
        cont: ContinuationFlags::NONE,
        payload: PacketPayload::Header(Bytes::from_static(b"FLV_HEADER_2_RESTART")),
        unknown_extensions: Vec::new(),
    };
    let res3 = adapter.ingest_packet(head2).unwrap();
    assert_eq!(res3, PacketIngestResult::HeaderReset { epoch: 2, pos: 1000 });
    assert_eq!(adapter.current_epoch(), 2);

    // Cache must have cleared old body items upon new epoch header
    assert_eq!(adapter.cache().body_len(), 0);

    // 4. Push new body chunk for Epoch 2
    let body2 = ChannelPacket {
        channel_id: None,
        stream_type: ID4(*b"flv\0"),
        pos: 1000,
        cont: ContinuationFlags::NONE,
        payload: PacketPayload::Data(Bytes::from_static(b"CHUNK_BODY_2")),
        unknown_extensions: Vec::new(),
    };
    let res4 = adapter.ingest_packet(body2).unwrap();
    assert_eq!(
        res4,
        PacketIngestResult::BodyPushed {
            continuity: PositionContinuity::Contiguous,
            pos: 1000,
            len: 12,
        }
    );
    assert_eq!(adapter.cache().body_len(), 1);
}

#[test]
fn test_p20_station_format_header_packet_parity() {
    let cid = sample_cid();
    let header_bytes = Bytes::from_static(b"FLV\x01\x05\x00\x00\x00\x09\x00\x00\x00\x00");

    // Construct Atom matching Station PCPOutputStream.cs:780-784 exactly:
    // chan {
    //   id: <cid>,
    //   pkt {
    //     type: "head",
    //     pos: 0,
    //     data: <header_bytes>
    //   }
    // }
    let station_wire_atom = Atom::new_parent(
        ID4::PCP_CHAN,
        vec![
            Atom::new_leaf(ID4::CHAN_ID, Bytes::copy_from_slice(cid.as_bytes())),
            Atom::new_parent(
                ID4::CHAN_PKT,
                vec![
                    Atom::new_leaf(ID4::PKT_TYPE, Bytes::copy_from_slice(b"head")),
                    Atom::new_u32_le(ID4::PKT_POS, 0),
                    Atom::new_leaf(ID4::PKT_DATA, header_bytes.clone()),
                ],
            ),
        ],
    );

    // 1. Parsing must correctly recognize it as PacketPayload::Header (not Data)
    let parsed = ChannelPacket::from_atom(&station_wire_atom, None).unwrap();
    assert_eq!(parsed.stream_type, ID4(*b"head"));
    assert_eq!(parsed.pos, 0);
    assert_eq!(parsed.payload, PacketPayload::Header(header_bytes.clone()));

    // 2. Serializing back must write type="head" and data=<header_bytes>
    let outbound_atoms = parsed.to_atoms().unwrap();
    assert_eq!(outbound_atoms.len(), 1);
    let out_chan = &outbound_atoms[0];
    assert_eq!(out_chan.name(), ID4::PCP_CHAN);

    let out_pkt = out_chan
        .children()
        .unwrap()
        .iter()
        .find(|c| c.name() == ID4::CHAN_PKT)
        .expect("chan_pkt child");

    let out_type = out_pkt
        .children()
        .unwrap()
        .iter()
        .find(|c| c.name() == ID4::PKT_TYPE)
        .expect("type child");
    assert_eq!(out_type.leaf_bytes().unwrap().as_ref(), b"head");

    let out_data = out_pkt
        .children()
        .unwrap()
        .iter()
        .find(|c| c.name() == ID4::PKT_DATA)
        .expect("data child");
    assert_eq!(out_data.leaf_bytes().unwrap(), header_bytes.as_ref());
}
