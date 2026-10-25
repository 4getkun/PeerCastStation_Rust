use bytes::{Bytes, BytesMut};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use peercast_core::atom::{Atom, AtomBudget};
use peercast_core::channel_id::ChannelId;
use peercast_core::errors::QuitReason;
use peercast_core::id4::ID4;
use peercast_pcp::probe::{
    execute_reverse_probe, ProbeError, ProbeLimiter, ProbeLimits, ProbeResult,
};

fn sample_server_sid() -> ChannelId {
    ChannelId::from_hex("11111111111111111111111111111111").unwrap()
}

fn sample_client_sid() -> ChannelId {
    ChannelId::from_hex("22222222222222222222222222222222").unwrap()
}

#[tokio::test]
async fn test_p19_probe_success_matching_sid() {
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();

    // Spawn mock client TCP listener on loopback
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let loopback_ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

    let mock_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = BytesMut::with_capacity(1024);
        let mut tmp = [0u8; 512];
        let budget = AtomBudget::default();

        // 1. Read pcp\n and helo
        let (found_pcp, found_helo) = loop {
            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            buf.extend_from_slice(&tmp[..n]);

            let mut freeze = buf.clone().freeze();
            let a1 = Atom::decode(&mut freeze, &budget).unwrap();
            let a2 = Atom::decode(&mut freeze, &budget).unwrap();

            if let (Some(p), Some(h)) = (a1, a2) {
                break (p, h);
            }
        };

        // Verify preamble is pcp\n with u32 value 1 (REQ-035, A09)
        assert_eq!(found_pcp.name(), ID4::PCP_CONNECT);
        assert_eq!(found_pcp.get_u32_le().unwrap(), 1);

        // Verify helo
        assert_eq!(found_helo.name(), ID4::PCP_HELO);

        // 2. Respond with oleh containing matching client_sid
        let oleh = Atom::new_parent(
            ID4::PCP_OLEH,
            vec![
                Atom::new_string(ID4::HELO_AGENT, "MockClient/1.0"),
                Atom::new_leaf(
                    ID4::HELO_SESSIONID,
                    Bytes::copy_from_slice(client_sid.as_bytes()),
                ),
                Atom::new_u32_le(ID4::HELO_VERSION, 1),
            ],
        );
        let oleh_bytes = oleh.to_bytes().unwrap();
        socket.write_all(&oleh_bytes).await.unwrap();

        // 3. Read quit(1000)
        let mut quit_buf = BytesMut::with_capacity(256);
        let q_atom = loop {
            let n = socket.read(&mut tmp).await.unwrap();
            assert!(n > 0);
            quit_buf.extend_from_slice(&tmp[..n]);
            let mut freeze = quit_buf.clone().freeze();
            if let Some(q) = Atom::decode(&mut freeze, &budget).unwrap() {
                break q;
            }
        };
        assert_eq!(q_atom.name(), ID4::PCP_QUIT);
        assert_eq!(q_atom.get_u32_le().unwrap(), QuitReason::NORMAL.raw());
    });

    let limiter = ProbeLimiter::new(ProbeLimits::default());
    let res = execute_reverse_probe(
        &limiter,
        loopback_ip,
        port,
        client_sid,
        server_sid,
        "PeerCastStation/Test",
    )
    .await
    .unwrap();

    assert_eq!(
        res,
        ProbeResult::Reachable {
            session_id: client_sid,
            target_addr: SocketAddr::new(loopback_ip, port),
        }
    );

    mock_task.await.unwrap();
}

#[tokio::test]
async fn test_p19_probe_mismatched_sid() {
    let server_sid = sample_server_sid();
    let expected_client_sid = sample_client_sid();
    let mismatched_sid = ChannelId::from_hex("99999999999999999999999999999999").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let loopback_ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

    let mock_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 512];
        let _ = socket.read(&mut buf).await.unwrap();

        // Peer sends oleh with WRONG session ID
        let oleh = Atom::new_parent(
            ID4::PCP_OLEH,
            vec![Atom::new_leaf(
                ID4::HELO_SESSIONID,
                Bytes::copy_from_slice(mismatched_sid.as_bytes()),
            )],
        );
        let _ = socket.write_all(&oleh.to_bytes().unwrap()).await;
    });

    let limiter = ProbeLimiter::new(ProbeLimits::default());
    let res = execute_reverse_probe(
        &limiter,
        loopback_ip,
        port,
        expected_client_sid,
        server_sid,
        "PeerCastStation/Test",
    )
    .await
    .unwrap();

    assert_eq!(
        res,
        ProbeResult::SessionMismatch {
            expected: expected_client_sid,
            actual: mismatched_sid,
            target_addr: SocketAddr::new(loopback_ip, port),
        }
    );

    mock_task.await.unwrap();
}

#[tokio::test]
async fn test_p19_probe_refused_connection() {
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();

    // Bind and immediately drop listener to guarantee port is closed/refused
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let loopback_ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

    let limiter = ProbeLimiter::new(ProbeLimits::default());
    let res = execute_reverse_probe(
        &limiter,
        loopback_ip,
        port,
        client_sid,
        server_sid,
        "PeerCastStation/Test",
    )
    .await
    .unwrap();

    match res {
        ProbeResult::Unreachable {
            target_addr,
            reason,
        } => {
            assert_eq!(target_addr, SocketAddr::new(loopback_ip, port));
            assert!(reason.contains("Connection failed"));
        }
        _ => panic!("Expected Unreachable due to refused connection"),
    }
}

#[tokio::test]
async fn test_p19_probe_stalled_connection_timeout() {
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let loopback_ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

    let _mock_stalled = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        // Never send anything; sleep to trigger probe timeout
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let limits = ProbeLimits {
        max_concurrent_global: 16,
        connect_timeout: Duration::from_millis(100),
        exchange_timeout: Duration::from_millis(150),
        enable_ipv6: false,
    };
    let limiter = ProbeLimiter::new(limits);

    let res = execute_reverse_probe(
        &limiter,
        loopback_ip,
        port,
        client_sid,
        server_sid,
        "PeerCastStation/Test",
    )
    .await
    .unwrap();

    match res {
        ProbeResult::Unreachable {
            target_addr,
            reason,
        } => {
            assert_eq!(target_addr, SocketAddr::new(loopback_ip, port));
            assert!(reason.contains("Exchange timeout exceeded"));
        }
        _ => panic!("Expected Unreachable due to exchange timeout"),
    }
}

#[tokio::test]
async fn test_p19_security_guards_forbid_arbitrary_ip() {
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();

    let limits = ProbeLimits::default();
    let limiter = ProbeLimiter::new(limits);

    let observed_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50));
    let arbitrary_target_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

    // Caller attempts to probe a target IP different from the observed remote IP
    let err = limiter
        .acquire(arbitrary_target_ip, observed_ip)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ProbeError::ForbiddenTarget {
            target: arbitrary_target_ip,
            observed: observed_ip,
        }
    );

    // Port 0 is rejected
    let port0_err = execute_reverse_probe(
        &limiter,
        observed_ip,
        0,
        client_sid,
        server_sid,
        "PeerCastStation/Test",
    )
    .await
    .unwrap_err();
    assert_eq!(port0_err, ProbeError::InvalidPort);
}

#[tokio::test]
async fn test_p19_ipv6_marked_pending() {
    let server_sid = sample_server_sid();
    let client_sid = sample_client_sid();
    let limiter = ProbeLimiter::new(ProbeLimits::default());

    let ipv6_addr = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1));
    let err = execute_reverse_probe(
        &limiter,
        ipv6_addr,
        7144,
        client_sid,
        server_sid,
        "PeerCastStation/Test",
    )
    .await
    .unwrap_err();

    assert_eq!(err, ProbeError::IPv6Pending);
}

#[tokio::test]
async fn test_p19_concurrency_limiter_and_cancellation() {
    let limits = ProbeLimits {
        max_concurrent_global: 1, // Only 1 concurrent probe globally
        connect_timeout: Duration::from_secs(5),
        exchange_timeout: Duration::from_secs(10),
        enable_ipv6: false,
    };
    let limiter = ProbeLimiter::new(limits);

    let ip1 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));
    let ip2 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));

    // First acquire succeeds
    let guard1 = limiter.acquire(ip1, ip1).await.expect("Acquire 1");

    // Second acquire to a different IP fails due to global limit
    let err_global = limiter.acquire(ip2, ip2).await.unwrap_err();
    assert_eq!(err_global, ProbeError::ConcurrencyLimitExceeded);

    // Also second acquire to the SAME IP fails due to per-peer limit
    let err_per_peer = limiter.acquire(ip1, ip1).await.unwrap_err();
    assert_eq!(err_per_peer, ProbeError::ConcurrencyLimitExceeded);

    // Dropping guard1 releases the slot
    drop(guard1);

    // Now acquire succeeds
    let guard2 = limiter.acquire(ip2, ip2).await.expect("Acquire after drop");
    drop(guard2);
}
