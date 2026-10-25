use bytes::{BufMut, Bytes};
use peercast_core::atom::{Atom, AtomBudget, AtomError};
use peercast_core::id4::ID4;

#[test]
fn test_budget_max_leaf_len_boundary() {
    let custom_budget = AtomBudget {
        max_leaf_len: 100,
        ..Default::default()
    };

    // 1. Exactly limit: 100 bytes payload -> must succeed
    let leaf_100 = Atom::new_leaf(ID4(*b"test"), vec![0u8; 100]);
    let mut bytes_100 = leaf_100.to_bytes().unwrap();
    let res = Atom::decode(&mut bytes_100, &custom_budget);
    assert!(res.is_ok());
    assert_eq!(res.unwrap().unwrap().leaf_bytes().unwrap().len(), 100);

    // 2. Limit + 1: 101 bytes payload -> must fail with LeafLengthExceeded
    let mut header_101 = Vec::new();
    header_101.put_slice(b"test");
    header_101.put_u32_le(101); // 101 bytes length
    header_101.extend(vec![0u8; 101]);

    let mut buf = Bytes::from(header_101);
    let err = Atom::decode(&mut buf, &custom_budget).unwrap_err();
    assert_eq!(
        err,
        AtomError::LeafLengthExceeded {
            actual: 101,
            max: 100
        }
    );

    // 3. Fast failure: even if only the 8-byte header is present,
    // if length is 101, it must fail IMMEDIATELY without waiting for payload bytes!
    let mut header_only = Bytes::from_static(b"test\x65\x00\x00\x00"); // 101 in LE
    let err_fast = Atom::decode(&mut header_only, &custom_budget).unwrap_err();
    assert_eq!(
        err_fast,
        AtomError::LeafLengthExceeded {
            actual: 101,
            max: 100
        }
    );
}

#[test]
fn test_budget_max_children_boundary() {
    let custom_budget = AtomBudget {
        max_children: 5,
        ..Default::default()
    };

    // 1. Limit (5 children) -> must succeed
    let children_5: Vec<Atom> = (0..5).map(|i| Atom::new_u8(ID4(*b"item"), i)).collect();
    let parent_5 = Atom::new_parent(ID4(*b"root"), children_5);
    let mut bytes_5 = parent_5.to_bytes().unwrap();
    let res = Atom::decode(&mut bytes_5, &custom_budget);
    assert!(res.is_ok());
    assert_eq!(res.unwrap().unwrap().children().unwrap().len(), 5);

    // 2. Limit + 1 (6 children) -> must fail immediately with ChildCountExceeded
    let mut header_6 = Bytes::from_static(b"root\x06\x00\x00\x80"); // 6 children with MSB set
    let err = Atom::decode(&mut header_6, &custom_budget).unwrap_err();
    assert_eq!(err, AtomError::ChildCountExceeded { actual: 6, max: 5 });
}

#[test]
fn test_budget_max_depth_boundary() {
    let custom_budget = AtomBudget {
        max_depth: 3,
        ..Default::default()
    };

    // Depth 1: root -> leaf
    let d1 = Atom::new_parent(ID4(*b"d001"), vec![Atom::new_empty_leaf(ID4(*b"leaf"))]);
    assert!(Atom::decode(&mut d1.to_bytes().unwrap(), &custom_budget).is_ok());

    // Depth 3: root -> d2 -> d3 -> leaf
    let d3 = Atom::new_parent(
        ID4(*b"d001"),
        vec![Atom::new_parent(
            ID4(*b"d002"),
            vec![Atom::new_parent(
                ID4(*b"d003"),
                vec![Atom::new_empty_leaf(ID4(*b"leaf"))],
            )],
        )],
    );
    assert!(Atom::decode(&mut d3.to_bytes().unwrap(), &custom_budget).is_ok());

    // Depth 4: root -> d2 -> d3 -> d4 -> leaf (exceeds max_depth 3)
    let d4 = Atom::new_parent(
        ID4(*b"d001"),
        vec![Atom::new_parent(
            ID4(*b"d002"),
            vec![Atom::new_parent(
                ID4(*b"d003"),
                vec![Atom::new_parent(
                    ID4(*b"d004"),
                    vec![Atom::new_empty_leaf(ID4(*b"leaf"))],
                )],
            )],
        )],
    );
    let mut bytes_d4 = d4.to_bytes().unwrap();
    let err = Atom::decode(&mut bytes_d4, &custom_budget).unwrap_err();
    assert_eq!(err, AtomError::DepthExceeded { actual: 4, max: 3 });
}

#[test]
fn test_budget_max_total_nodes() {
    let custom_budget = AtomBudget {
        max_total_nodes: 10,
        ..Default::default()
    };

    // 10 nodes: 1 parent + 9 leaves = 10 nodes (within budget)
    let leaves_9: Vec<Atom> = (0..9).map(|i| Atom::new_u8(ID4(*b"node"), i)).collect();
    let parent_10 = Atom::new_parent(ID4(*b"root"), leaves_9);
    assert!(Atom::decode(&mut parent_10.to_bytes().unwrap(), &custom_budget).is_ok());

    // 11 nodes: 1 parent + 10 leaves = 11 nodes (exceeds budget 10)
    let leaves_10: Vec<Atom> = (0..10).map(|i| Atom::new_u8(ID4(*b"node"), i)).collect();
    let parent_11 = Atom::new_parent(ID4(*b"root"), leaves_10);
    let mut bytes_11 = parent_11.to_bytes().unwrap();
    let err = Atom::decode(&mut bytes_11, &custom_budget).unwrap_err();
    assert_eq!(
        err,
        AtomError::TotalNodesExceeded {
            actual: 11,
            max: 10
        }
    );
}

#[test]
fn test_budget_max_total_bytes() {
    let custom_budget = AtomBudget {
        max_total_bytes: 50,
        ..Default::default()
    };

    // Each leaf with 2 bytes payload is 8 (header) + 2 = 10 bytes.
    // Parent with 4 children: 8 (parent header) + 4 * 10 = 48 bytes (<= 50)
    let children_4: Vec<Atom> = (0..4).map(|_| Atom::new_u16_le(ID4(*b"item"), 1)).collect();
    let parent_48 = Atom::new_parent(ID4(*b"root"), children_4);
    assert!(Atom::decode(&mut parent_48.to_bytes().unwrap(), &custom_budget).is_ok());

    // Parent with 5 children: 8 + 5 * 10 = 58 bytes (> 50)
    let children_5: Vec<Atom> = (0..5).map(|_| Atom::new_u16_le(ID4(*b"item"), 1)).collect();
    let parent_58 = Atom::new_parent(ID4(*b"root"), children_5);
    let mut bytes_58 = parent_58.to_bytes().unwrap();
    let err = Atom::decode(&mut bytes_58, &custom_budget).unwrap_err();
    assert!(matches!(err, AtomError::TotalBytesExceeded { .. }));
}
