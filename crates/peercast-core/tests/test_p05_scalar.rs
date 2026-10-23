use peercast_core::id4::{Id4Error, ID4};
use peercast_core::scalar::{Scalar, ScalarError};
use std::net::IpAddr;

#[test]
fn test_id4_pcp_newline_and_nul_padding() {
    // Exact 4-byte raw representations
    assert_eq!(ID4::PCP_CONNECT.as_bytes(), b"pcp\n");
    assert_eq!(ID4::PCP_QUIT.as_bytes(), b"quit");
    assert_eq!(ID4::PCP_OK.as_bytes(), b"ok\0\0");
    assert_eq!(ID4::HELO_SESSIONID.as_bytes(), b"sid\0");

    // Padding helper
    let sid = ID4::from_str_padded("sid").unwrap();
    assert_eq!(sid.as_bytes(), b"sid\0");

    let pcp_nl = ID4::from_str_padded("pcp\n").unwrap();
    assert_eq!(pcp_nl.as_bytes(), b"pcp\n");

    let empty = ID4::from_str_padded("").unwrap();
    assert_eq!(empty.as_bytes(), b"\0\0\0\0");

    // Overlong ID must fail
    let err = ID4::from_str_padded("tools").unwrap_err();
    assert_eq!(err, Id4Error::TooLong(5));
}

#[test]
fn test_scalar_fixed_width_le() {
    // Little-endian u16
    let u16_val = 0x1234u16;
    let b16 = Scalar::write_u16_le(u16_val);
    assert_eq!(b16, [0x34, 0x12]);
    assert_eq!(Scalar::read_u16_le(&b16).unwrap(), 0x1234);

    // Malformed widths must fail without reading out of bounds
    assert!(matches!(
        Scalar::read_u16_le(&[0x34]),
        Err(ScalarError::WidthMismatch { expected: 2, actual: 1 })
    ));
    assert!(matches!(
        Scalar::read_u16_le(&[0x34, 0x12, 0x00]),
        Err(ScalarError::WidthMismatch { expected: 2, actual: 3 })
    ));

    // Little-endian u32
    let u32_val = 0x12345678u32;
    let b32 = Scalar::write_u32_le(u32_val);
    assert_eq!(b32, [0x78, 0x56, 0x34, 0x12]);
    assert_eq!(Scalar::read_u32_le(&b32).unwrap(), 0x12345678);

    assert!(matches!(
        Scalar::read_u32_le(&[0x78, 0x56, 0x34]),
        Err(ScalarError::WidthMismatch { expected: 4, actual: 3 })
    ));
    assert!(matches!(
        Scalar::read_u32_le(&[0x78, 0x56, 0x34, 0x12, 0x00]),
        Err(ScalarError::WidthMismatch { expected: 4, actual: 5 })
    ));

    // Little-endian u64
    let u64_val = 0x0102030405060708u64;
    let b64 = Scalar::write_u64_le(u64_val);
    assert_eq!(b64, [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
    assert_eq!(Scalar::read_u64_le(&b64).unwrap(), 0x0102030405060708);
}

#[test]
fn test_scalar_nul_terminated_string() {
    let s = "PeerCast";
    let encoded = Scalar::write_nul_terminated_utf8(s);
    assert_eq!(&encoded[..], b"PeerCast\0");
    assert_eq!(Scalar::read_nul_terminated_utf8(&encoded).unwrap(), "PeerCast");

    // Missing terminating NUL
    assert!(matches!(
        Scalar::read_nul_terminated_utf8(b"PeerCast"),
        Err(ScalarError::MissingTerminatingNul)
    ));

    // Empty string without NUL
    assert!(matches!(
        Scalar::read_nul_terminated_utf8(b""),
        Err(ScalarError::MissingTerminatingNul)
    ));

    // Empty string with NUL
    assert_eq!(Scalar::read_nul_terminated_utf8(b"\0").unwrap(), "");

    // Invalid UTF-8 sequence
    let bad_utf8 = vec![0xff, 0xfe, 0x00];
    assert!(matches!(
        Scalar::read_nul_terminated_utf8(&bad_utf8),
        Err(ScalarError::InvalidUtf8(_))
    ));
}

#[test]
fn test_scalar_ip_reversal() {
    // IPv4: 127.0.0.1 -> on wire reversed: [1, 0, 0, 127]
    let ip4: IpAddr = "127.0.0.1".parse().unwrap();
    let wire4 = Scalar::write_ip_reversed(&ip4);
    assert_eq!(wire4, vec![1, 0, 0, 127]);
    assert_eq!(Scalar::read_ip_reversed(&wire4).unwrap(), ip4);

    // IPv4: 192.168.1.100 -> on wire: [100, 1, 168, 192]
    let ip4_lan: IpAddr = "192.168.1.100".parse().unwrap();
    let wire4_lan = Scalar::write_ip_reversed(&ip4_lan);
    assert_eq!(wire4_lan, vec![100, 1, 168, 192]);
    assert_eq!(Scalar::read_ip_reversed(&wire4_lan).unwrap(), ip4_lan);

    // IPv6: ::1 -> reversed: [1, 0, 0, ..., 0]
    let ip6: IpAddr = "::1".parse().unwrap();
    let wire6 = Scalar::write_ip_reversed(&ip6);
    let mut expected6 = vec![0u8; 16];
    expected6[0] = 1;
    assert_eq!(wire6, expected6);
    assert_eq!(Scalar::read_ip_reversed(&wire6).unwrap(), ip6);

    // Invalid lengths
    assert_eq!(
        Scalar::read_ip_reversed(&[1, 2, 3]),
        Err(ScalarError::InvalidIpLength(3))
    );
    assert_eq!(
        Scalar::read_ip_reversed(&[0u8; 15]),
        Err(ScalarError::InvalidIpLength(15))
    );
}
