use bytes::Bytes;
use peercast_core::atom::{Atom, AtomBudget};
use peercast_core::channel_id::ChannelId;
use peercast_core::id4::ID4;
use peercast_pcp::http_prelude::{
    parse_relay_request, parse_relay_response, PcpHttpError, PcpHttpLimits, PcpHttpStatus,
};

#[test]
fn test_header_and_first_atom_in_one_read_transition() {
    let limits = PcpHttpLimits::default();

    // 1. Construct HTTP 200 response followed immediately by binary Atom
    let http_str = "HTTP/1.0 200 OK\r\nx-peercast-pos: 12345\r\nContent-Type: application/x-peercast-pcp\r\n\r\n";
    let first_atom = Atom::new_u32_le(ID4::PCP_OK, 1);
    let atom_bytes = first_atom.to_bytes().unwrap();

    let mut combined = Vec::new();
    combined.extend_from_slice(http_str.as_bytes());
    combined.extend_from_slice(&atom_bytes);

    let mut buf = Bytes::from(combined);

    // 2. Parse HTTP relay response
    let response = parse_relay_response(&mut buf, &limits).unwrap().expect("Response must parse");
    assert_eq!(response.status, PcpHttpStatus::Ok200);
    assert_eq!(response.pos, Some(12345));

    // 3. Crucial check: buf must retain the exact binary tail for Atom decoding (REQ-031)
    assert_eq!(buf.len(), atom_bytes.len());
    let decoded_atom = Atom::decode(&mut buf, &AtomBudget::default())
        .unwrap()
        .expect("Atom must decode from retained tail");
    assert_eq!(decoded_atom, first_atom);
    assert!(buf.is_empty(), "Buffer must be cleanly drained after decoding Atom");
}

#[test]
fn test_404_never_enters_atom_state() {
    let limits = PcpHttpLimits::default();
    let http_404 = "HTTP/1.0 404 Not Found\r\nServer: PeerCast\r\n\r\n";
    let mut buf = Bytes::from(http_404.as_bytes());

    // REQ-032: On 404, stop as off-air/not found; MUST NOT enter Atom state
    let res = parse_relay_response(&mut buf, &limits);
    assert_eq!(res, Err(PcpHttpError::NotFoundOffAir));
}

#[test]
fn test_fragmented_crlf_and_incremental_arrival() {
    let limits = PcpHttpLimits::default();
    let full = "HTTP/1.0 200 OK\r\nServer: PeerCast\r\n\r\n";

    // Split right before the final '\n' of "\r\n\r\n"
    let split_pos = full.len() - 1;
    let mut partial = Bytes::copy_from_slice(&full.as_bytes()[..split_pos]);

    // Incomplete header must return Ok(None) without error
    assert_eq!(parse_relay_response(&mut partial, &limits).unwrap(), None);

    // Now complete with remaining byte
    let mut complete = Bytes::copy_from_slice(full.as_bytes());
    let resp = parse_relay_response(&mut complete, &limits).unwrap().expect("Must succeed when complete");
    assert_eq!(resp.status, PcpHttpStatus::Ok200);
    assert!(complete.is_empty());
}

#[test]
fn test_oversized_header_rejection() {
    let limits = PcpHttpLimits {
        max_header_bytes: 100,
        ..Default::default()
    };

    // Construct 150 bytes without \r\n\r\n
    let long_header = "X-Padding: ".to_string() + &"a".repeat(120);
    let mut buf = Bytes::from(long_header.into_bytes());

    let err = parse_relay_response(&mut buf, &limits).unwrap_err();
    assert!(matches!(err, PcpHttpError::HeaderTooLarge { .. }));
}

#[test]
fn test_malformed_status_line() {
    let limits = PcpHttpLimits::default();
    let malformed = "GARBAGE NOT HTTP\r\n\r\n";
    let mut buf = Bytes::from(malformed.as_bytes());

    let err = parse_relay_response(&mut buf, &limits).unwrap_err();
    assert!(matches!(err, PcpHttpError::MalformedStatusLine(_)));
}

#[test]
fn test_relay_request_valid_and_invalid() {
    let limits = PcpHttpLimits::default();

    // Valid request
    let hex_id = "0123456789abcdef0123456789abcdef";
    let req_str = format!(
        "GET /channel/{} HTTP/1.0\r\nx-peercast-pcp: 1\r\nx-peercast-pos: 4200\r\n\r\n",
        hex_id
    );
    let mut buf = Bytes::from(req_str.into_bytes());

    let req = parse_relay_request(&mut buf, &limits).unwrap().expect("Request must parse");
    assert_eq!(req.channel_id, ChannelId::from_hex(hex_id).unwrap());
    assert_eq!(req.http_version, "HTTP/1.0");
    assert_eq!(req.pcp_version, Some(1));
    assert_eq!(req.pos, Some(4200));

    // Invalid ChannelId hex
    let bad_hex_req = "GET /channel/NOT_A_VALID_HEX HTTP/1.0\r\n\r\n";
    let mut bad_buf = Bytes::from(bad_hex_req.as_bytes());
    let err = parse_relay_request(&mut bad_buf, &limits).unwrap_err();
    assert!(matches!(err, PcpHttpError::InvalidChannelId(_)));
}
