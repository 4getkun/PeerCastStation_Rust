use peercast_core::atom::Atom;
use peercast_core::host::Host;
use peercast_core::id4::ID4;
use std::net::SocketAddr;

fn host_atom(endpoints: &[SocketAddr]) -> Atom {
    let mut children = Vec::new();
    for endpoint in endpoints {
        children.push(Atom::new_ip(ID4::HOST_IP, endpoint.ip()));
        children.push(Atom::new_u16_le(ID4::HOST_PORT, endpoint.port()));
    }
    Atom::new_parent(ID4::PCP_HOST, children)
}

#[test]
fn parses_public_and_private_host_endpoints() {
    let public: SocketAddr = "203.0.113.10:7144".parse().unwrap();
    let private: SocketAddr = "192.168.1.2:9072".parse().unwrap();
    let host = Host::from_atom(&host_atom(&[public, private])).unwrap();

    assert_eq!(host.global_endpoint, Some(public));
    assert_eq!(host.local_endpoint, Some(private));
    assert_eq!(host.contact_address(), Some(public));
}

#[test]
fn unspecified_secondary_endpoint_does_not_replace_public_endpoint() {
    let public: SocketAddr = "203.0.113.10:7144".parse().unwrap();
    let unspecified: SocketAddr = "0.0.0.0:7144".parse().unwrap();
    let host = Host::from_atom(&host_atom(&[public, unspecified])).unwrap();

    assert_eq!(host.global_endpoint, Some(public));
    assert_eq!(host.local_endpoint, None);
    assert_eq!(host.contact_address(), Some(public));
}
