use bytes::Bytes;
use peercast_core::atom::{Atom, AtomBudget, AtomError};
use peercast_core::id4::ID4;

#[test]
fn test_decoder_fragmentation_every_split_position() {
    // Construct a multi-level tree with mixed leaves and containers
    let tree = Atom::new_parent(
        ID4::PCP_CHAN,
        vec![
            Atom::new_u32_le(ID4::CHAN_PKT, 42),
            Atom::new_string(ID4::CHAN_INFO, "Station"),
            Atom::new_parent(
                ID4::CHAN_TRCK,
                vec![
                    Atom::new_string(ID4(*b"titl"), "Song Name"),
                    Atom::new_u16_le(ID4(*b"bitr"), 128),
                ],
            ),
        ],
    );

    let encoded = tree.to_bytes().unwrap();
    let total_len = encoded.len();
    let budget = AtomBudget::default();

    // Test every single split point from 0 to total_len
    for split in 0..total_len {
        let mut buf_part1 = encoded.slice(0..split);
        // Before all bytes are available, decode must return Ok(None)
        let res = Atom::decode(&mut buf_part1, &budget).unwrap();
        assert_eq!(res, None, "Split at {} should be incomplete", split);
        // Buffer must not be corrupted or consumed
        assert_eq!(buf_part1.len(), split);

        // When the rest of the bytes arrive
        let mut buf_full = encoded.clone();
        let decoded = Atom::decode(&mut buf_full, &budget)
            .unwrap()
            .expect("Should succeed");
        assert_eq!(decoded, tree);
        assert_eq!(buf_full.len(), 0, "Buffer should be fully consumed");
    }
}

#[test]
fn test_decoder_concatenated_atoms_streaming() {
    let atom1 = Atom::new_u32_le(ID4::PCP_CONNECT, 1);
    let atom2 = Atom::new_string(ID4(*b"name"), "test");
    let atom3 = Atom::new_u32_le(ID4::PCP_QUIT, 1000);

    let mut concatenated = Vec::new();
    atom1.encode(&mut concatenated).unwrap();
    atom2.encode(&mut concatenated).unwrap();
    atom3.encode(&mut concatenated).unwrap();

    let mut buf = Bytes::from(concatenated);
    let budget = AtomBudget::default();

    // First decode: should yield atom1
    let dec1 = Atom::decode(&mut buf, &budget).unwrap().expect("Atom 1");
    assert_eq!(dec1, atom1);

    // Second decode: should yield atom2
    let dec2 = Atom::decode(&mut buf, &budget).unwrap().expect("Atom 2");
    assert_eq!(dec2, atom2);

    // Third decode: should yield atom3
    let dec3 = Atom::decode(&mut buf, &budget).unwrap().expect("Atom 3");
    assert_eq!(dec3, atom3);

    // Buffer should now be empty and next decode returns Ok(None)
    assert!(buf.is_empty());
    assert_eq!(Atom::decode(&mut buf, &budget).unwrap(), None);
}

#[test]
fn test_decoder_eof_clean_versus_truncated() {
    let budget = AtomBudget::default();

    // 1. Clean EOF on empty buffer
    let mut empty = Bytes::new();
    assert_eq!(Atom::decode_eof(&mut empty, &budget).unwrap(), None);

    // 2. Truncated header (e.g. 5 bytes received, peer closed socket)
    let mut partial_header = Bytes::from_static(b"pcp\n\x04");
    let err = Atom::decode_eof(&mut partial_header, &budget).unwrap_err();
    assert_eq!(
        err,
        AtomError::Truncated {
            needed: 8,
            available: 5
        }
    );

    // 3. Truncated payload (header says 4 bytes payload, only 2 bytes provided)
    let mut partial_payload = Bytes::from_static(b"pcp\n\x04\x00\x00\x00\x01\x00");
    let err = Atom::decode_eof(&mut partial_payload, &budget).unwrap_err();
    assert_eq!(
        err,
        AtomError::Truncated {
            needed: 12,
            available: 10
        }
    );
}

#[test]
fn test_decoder_preserves_order_and_duplicates() {
    let child1 = Atom::new_u32_le(ID4(*b"host"), 10);
    let child2 = Atom::new_u32_le(ID4(*b"host"), 20);
    let child3 = Atom::new_u32_le(ID4(*b"host"), 10);

    let original = Atom::new_parent(ID4::PCP_ROOT, vec![child1, child2, child3]);
    let mut bytes = original.to_bytes().unwrap();

    let decoded = Atom::decode(&mut bytes, &AtomBudget::default())
        .unwrap()
        .unwrap();
    assert_eq!(decoded, original);

    // Verify ordering and duplicate retention
    let children = decoded.children().unwrap();
    assert_eq!(children.len(), 3);
    assert_eq!(children[0].get_u32_le().unwrap(), 10);
    assert_eq!(children[1].get_u32_le().unwrap(), 20);
    assert_eq!(children[2].get_u32_le().unwrap(), 10);
}
