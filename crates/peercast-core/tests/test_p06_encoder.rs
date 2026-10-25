use bytes::Bytes;
use peercast_core::atom::Atom;
use peercast_core::id4::ID4;

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn test_encoder_reference_vectors_independent_bytes() {
    // 1. Raw PCP v1: "7063700a0400000001000000"
    let pcp_v1 = Atom::new_u32_le(ID4::PCP_CONNECT, 1);
    let bytes = pcp_v1.to_bytes().unwrap();
    assert_eq!(
        bytes.as_ref(),
        &hex_to_bytes("7063700a0400000001000000")[..]
    );

    // 2. Raw PCP v100: "7063700a0400000064000000"
    let pcp_v100 = Atom::new_u32_le(ID4::PCP_CONNECT, 100);
    let bytes = pcp_v100.to_bytes().unwrap();
    assert_eq!(
        bytes.as_ref(),
        &hex_to_bytes("7063700a0400000064000000")[..]
    );

    // 3. Normal quit: "7175697404000000e8030000"
    let quit_norm = Atom::new_u32_le(ID4::PCP_QUIT, 1000);
    let bytes = quit_norm.to_bytes().unwrap();
    assert_eq!(
        bytes.as_ref(),
        &hex_to_bytes("7175697404000000e8030000")[..]
    );

    // 4. Read error quit 4000: "7175697404000000a00f0000"
    let quit_4000 = Atom::new_u32_le(ID4::PCP_QUIT, 4000);
    let bytes = quit_4000.to_bytes().unwrap();
    assert_eq!(
        bytes.as_ref(),
        &hex_to_bytes("7175697404000000a00f0000")[..]
    );

    // 5. Empty parent: "68656c6f00000080"
    let empty_parent = Atom::new_empty_parent(ID4::PCP_HELO);
    let bytes = empty_parent.to_bytes().unwrap();
    assert_eq!(bytes.as_ref(), &hex_to_bytes("68656c6f00000080")[..]);

    // 6. Empty leaf: "68656c6f00000000"
    let empty_leaf = Atom::new_empty_leaf(ID4::PCP_HELO);
    let bytes = empty_leaf.to_bytes().unwrap();
    assert_eq!(bytes.as_ref(), &hex_to_bytes("68656c6f00000000")[..]);

    // 7. One-child parent: "68656c6f01000080" + child
    let child = Atom::new_empty_leaf(ID4(*b"test"));
    let parent_one = Atom::new_parent(ID4::PCP_HELO, vec![child]);
    let bytes = parent_one.to_bytes().unwrap();
    let expected = hex_to_bytes("68656c6f010000807465737400000000");
    assert_eq!(bytes.as_ref(), &expected[..]);

    // 8. NUL-padded field ID (sid\0): "736964001000000000112233445566778899aabbccddeeff"
    let sid_payload = hex_to_bytes("00112233445566778899aabbccddeeff");
    let sid_atom = Atom::new_leaf(ID4::HELO_SESSIONID, Bytes::from(sid_payload));
    let bytes = sid_atom.to_bytes().unwrap();
    assert_eq!(
        bytes.as_ref(),
        &hex_to_bytes("736964001000000000112233445566778899aabbccddeeff")[..]
    );

    // 9. UTF-8 NUL leaf ("name" with "あ\0"): "6e616d6504000000e3818200"
    let name_atom = Atom::new_string(ID4(*b"name"), "あ");
    let bytes = name_atom.to_bytes().unwrap();
    assert_eq!(
        bytes.as_ref(),
        &hex_to_bytes("6e616d6504000000e3818200")[..]
    );
}

#[test]
fn test_encoder_preserves_order_and_duplicates() {
    let child1 = Atom::new_u32_le(ID4(*b"host"), 1);
    let child2 = Atom::new_u32_le(ID4(*b"host"), 2);
    let child3 = Atom::new_u32_le(ID4(*b"host"), 1); // duplicate of child1

    let parent = Atom::new_parent(ID4::PCP_ROOT, vec![child1, child2, child3]);
    let bytes = parent.to_bytes().unwrap();

    // Check parent descriptor: 3 children -> 0x80000003
    assert_eq!(&bytes[0..4], b"root");
    assert_eq!(&bytes[4..8], &[0x03, 0x00, 0x00, 0x80]);

    // Child 1: "host" + len 4 + val 1
    assert_eq!(&bytes[8..12], b"host");
    assert_eq!(&bytes[12..16], &[0x04, 0x00, 0x00, 0x00]);
    assert_eq!(&bytes[16..20], &[0x01, 0x00, 0x00, 0x00]);

    // Child 2: "host" + len 4 + val 2
    assert_eq!(&bytes[20..24], b"host");
    assert_eq!(&bytes[24..28], &[0x04, 0x00, 0x00, 0x00]);
    assert_eq!(&bytes[28..32], &[0x02, 0x00, 0x00, 0x00]);

    // Child 3: "host" + len 4 + val 1
    assert_eq!(&bytes[32..36], b"host");
    assert_eq!(&bytes[36..40], &[0x04, 0x00, 0x00, 0x00]);
    assert_eq!(&bytes[40..44], &[0x01, 0x00, 0x00, 0x00]);
}

#[test]
fn test_generic_encoder_supports_large_leaves_without_packet_cap() {
    // REQ-026: Generic encoder does not cap generic leaves at 16 KiB
    let large_data = vec![0xaa; 128 * 1024]; // 128 KiB
    let large_leaf = Atom::new_leaf(ID4(*b"big\0"), Bytes::from(large_data));

    let bytes = large_leaf.to_bytes().unwrap();
    assert_eq!(&bytes[0..4], b"big\0");
    assert_eq!(&bytes[4..8], &(128 * 1024u32).to_le_bytes());
    assert_eq!(bytes.len(), 8 + 128 * 1024);
}
