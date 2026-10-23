use std::sync::Arc;
use std::time::Duration;

use peercast_core::channel_id::ChannelId;
use peercast_core::gossip::BroadcastGroup;
use peercast_core::id4::ID4;
use peercast_pcp::yellow_page::{
    create_station_yp_bcst_atom, MockYellowPageServer, PCPYellowPageClient,
    YpAnnouncementChannel, YpBackoffTracker, YpClientConfig, YpConnectionStatus, YpError,
};

fn sample_channel() -> YpAnnouncementChannel {
    let cid = ChannelId::from_bytes([0x11; 16]);
    let bcid = ChannelId::from_bytes([0x22; 16]);
    let mut ch = YpAnnouncementChannel::new(cid, bcid, "Test Radio".to_string());
    ch.genre = "J-Pop".to_string();
    ch.desc = "Streaming 24/7".to_string();
    ch.url = "http://example.com/stream".to_string();
    ch.bitrate_kbps = 128;
    ch.format = "FLV".to_string();
    ch.contact_endpoint = Some("192.168.1.50:7144".parse().unwrap());
    ch.listeners = 5;
    ch.relays = 2;
    ch.uptime = Duration::from_secs(3600);
    ch.oldest_pos = Some(1000);
    ch.newest_pos = Some(50000);
    ch.is_relayable = true;
    ch.is_playable = true;
    ch.is_firewalled = false;
    ch.is_receiving = true;
    ch
}

/// Acceptance criterion 1:
/// Verify Station-format bcst announcement Atom structure and REQ-035.
#[test]
fn test_p34_station_yp_bcst_atom_parity_and_req035() {
    let session_id = ChannelId::from_bytes([0x55; 16]);
    let ch = sample_channel();

    let bcst = create_station_yp_bcst_atom(session_id, 1218, &ch);
    assert_eq!(bcst.name(), ID4::PCP_BCST);

    let children = bcst.children().expect("bcst must have children");

    // 1. Verify bcst header attributes
    let ttl = children.iter().find(|c| c.name() == ID4::BCST_TTL).unwrap().get_u8().unwrap();
    assert_eq!(ttl, 1, "REQ-035: bcst ttl must be 1");

    let hops = children.iter().find(|c| c.name() == ID4::BCST_HOPS).unwrap().get_u8().unwrap();
    assert_eq!(hops, 0, "REQ-035: bcst hops must be 0");

    let group = children.iter().find(|c| c.name() == ID4::BCST_GRP).unwrap().get_u8().unwrap();
    assert_eq!(group, BroadcastGroup::ROOT.raw(), "REQ-035: bcst group must be ROOT (1)");

    let from_bytes = children.iter().find(|c| c.name() == ID4::BCST_FROM).unwrap().leaf_bytes().unwrap();
    assert_eq!(&from_bytes[..], session_id.as_bytes(), "REQ-035: from must equal session_id");

    let cid_bytes = children.iter().find(|c| c.name() == ID4::BCST_CID).unwrap().leaf_bytes().unwrap();
    assert_eq!(&cid_bytes[..], ch.channel_id.as_bytes(), "REQ-035: cid must equal channel_id (no C++ hashing)");

    // 2. Verify chan atom child
    let chan = children.iter().find(|c| c.name() == ID4::PCP_CHAN).expect("must contain chan atom");
    let chan_children = chan.children().expect("chan must have children");
    let chan_id = chan_children.iter().find(|c| c.name() == ID4::CHAN_ID).unwrap().leaf_bytes().unwrap();
    assert_eq!(&chan_id[..], ch.channel_id.as_bytes());

    let chan_bcid = chan_children.iter().find(|c| c.name() == ID4::CHAN_BCID).unwrap().leaf_bytes().unwrap();
    assert_eq!(&chan_bcid[..], ch.broadcast_id.as_bytes());

    let chan_info = chan_children.iter().find(|c| c.name() == ID4::CHAN_INFO).unwrap();
    let info_children = chan_info.children().unwrap();
    let name_str = info_children.iter().find(|c| c.name() == ID4::INFO_NAME).unwrap().get_string().unwrap();
    assert_eq!(name_str, "Test Radio");

    let genre_str = info_children.iter().find(|c| c.name() == ID4::INFO_GENRE).unwrap().get_string().unwrap();
    assert_eq!(genre_str, "J-Pop");

    // 3. Verify host atom child
    let host = children.iter().find(|c| c.name() == ID4::PCP_HOST).expect("must contain host atom");
    let host_children = host.children().expect("host must have children");
    let host_cid = host_children.iter().find(|c| c.name() == ID4::HOST_CHANID).unwrap().leaf_bytes().unwrap();
    assert_eq!(&host_cid[..], ch.channel_id.as_bytes());

    let host_sid = host_children.iter().find(|c| c.name() == ID4::HOST_ID).unwrap().leaf_bytes().unwrap();
    assert_eq!(&host_sid[..], session_id.as_bytes());

    let numl = host_children.iter().find(|c| c.name() == ID4::HOST_NUML).unwrap().get_u32_le().unwrap();
    assert_eq!(numl, 5);

    let numr = host_children.iter().find(|c| c.name() == ID4::HOST_NUMR).unwrap().get_u32_le().unwrap();
    assert_eq!(numr, 2);

    let uptm = host_children.iter().find(|c| c.name() == ID4::HOST_UPTIME).unwrap().get_u32_le().unwrap();
    assert_eq!(uptm, 3600);
}

/// Acceptance criterion 2 (Stop / Block Guard):
/// Rejects unauthorized public YP destinations (REQ-063).
#[test]
fn test_p34_safety_guard_rejects_unauthorized_public_destination() {
    let session_id = ChannelId::from_bytes([0x10; 16]);
    let broadcast_id = ChannelId::from_bytes([0x20; 16]);

    // Public IP destination without authorization
    let public_config = YpClientConfig {
        directory_endpoint: "203.0.113.195:7144".parse().unwrap(),
        agent_name: "PeerCastStation/Rust".to_string(),
        servant_version: 1218,
        update_interval: Duration::from_secs(30),
        allow_public_announcements: false, // Disallowed!
    };

    let result = PCPYellowPageClient::new(session_id, broadcast_id, public_config);
    match result {
        Err(YpError::UnauthorizedPublicDestination(dest)) => {
            assert!(dest.contains("203.0.113.195"));
        }
        _ => panic!("Expected UnauthorizedPublicDestination error for unapproved public IP"),
    }
}

/// Acceptance criterion 3:
/// Station reference retry backoff: 1000ms * 3 / 2 capped at 30000ms using integer arithmetic.
#[test]
fn test_p34_backoff_arithmetic_1000_3_2_capped_30000() {
    let mut tracker = YpBackoffTracker::default();
    assert_eq!(tracker.current_delay(), Duration::from_millis(1000));

    // 1st failure: 1000 * 3 / 2 = 1500ms
    assert_eq!(tracker.record_failure(), Duration::from_millis(1500));
    assert_eq!(tracker.attempt_count(), 1);

    // 2nd failure: 1500 * 3 / 2 = 2250ms
    assert_eq!(tracker.record_failure(), Duration::from_millis(2250));

    // 3rd failure: 2250 * 3 / 2 = 3375ms
    assert_eq!(tracker.record_failure(), Duration::from_millis(3375));

    // 4th failure: 3375 * 3 / 2 = 5062ms
    assert_eq!(tracker.record_failure(), Duration::from_millis(5062));

    // 5th failure: 5062 * 3 / 2 = 7593ms
    assert_eq!(tracker.record_failure(), Duration::from_millis(7593));

    // 6th failure: 7593 * 3 / 2 = 11389ms
    assert_eq!(tracker.record_failure(), Duration::from_millis(11389));

    // 7th failure: 11389 * 3 / 2 = 17083ms
    assert_eq!(tracker.record_failure(), Duration::from_millis(17083));

    // 8th failure: 17083 * 3 / 2 = 25624ms
    assert_eq!(tracker.record_failure(), Duration::from_millis(25624));

    // 9th failure: 25624 * 3 / 2 = 38436ms -> capped at 30,000ms!
    assert_eq!(tracker.record_failure(), Duration::from_millis(30000));

    // Subsequent failures stay capped at 30,000ms
    assert_eq!(tracker.record_failure(), Duration::from_millis(30000));
    assert_eq!(tracker.record_failure(), Duration::from_millis(30000));

    // Success resets backoff
    tracker.reset_success();
    assert_eq!(tracker.current_delay(), Duration::from_millis(1000));
    assert_eq!(tracker.attempt_count(), 0);
}

/// Acceptance criterion 4:
/// Loopback integration: handshake, initial announcement transmission, and status tracking.
#[tokio::test]
async fn test_p34_loopback_announcement_and_status() {
    let mock_server = MockYellowPageServer::bind().await.unwrap();
    let port = mock_server.port();

    let session_id = ChannelId::from_bytes([0x33; 16]);
    let broadcast_id = ChannelId::from_bytes([0x44; 16]);
    let config = YpClientConfig::for_loopback(port);

    let client = PCPYellowPageClient::new(session_id, broadcast_id, config).unwrap();
    let channel = sample_channel();
    client.add_channel(channel.clone()).await;
    assert_eq!(client.ref_count(), 1);

    // Spawn mock server in background
    let received_bcsts = mock_server.received_bcsts();
    tokio::spawn(async move {
        let _ = mock_server.run_once().await;
    });

    // Run client loop in background
    let client_handle = {
        let c = Arc::new(client);
        let c_clone = c.clone();
        tokio::spawn(async move {
            let _ = c_clone.run_loop().await;
        });
        c
    };

    // Await connected status
    let mut status_rx = client_handle.subscribe_status();
    tokio::time::timeout(Duration::from_secs(3), async {
        while *status_rx.borrow_and_update() != YpConnectionStatus::Connected {
            let _ = status_rx.changed().await;
        }
    })
    .await
    .expect("Client should connect to mock YP");

    assert_eq!(client_handle.status(), YpConnectionStatus::Connected);

    // Verify mock server received the bcst announcement
    tokio::time::sleep(Duration::from_millis(100)).await;
    let list = received_bcsts.lock().await;
    assert!(!list.is_empty(), "Mock YP must receive the announcement bcst");
    let received = &list[0];
    assert_eq!(received.name(), ID4::PCP_BCST);

    // Clean shutdown
    client_handle.shutdown();
}

/// Acceptance criterion 5:
/// Terminal Banned status stops retries immediately (REQ-062).
#[tokio::test]
async fn test_p34_terminal_banned_stops_retries_immediately() {
    let mut mock_server = MockYellowPageServer::bind().await.unwrap();
    mock_server.set_banned_mode(true); // OLEH with dis = 1
    let port = mock_server.port();

    let session_id = ChannelId::from_bytes([0x33; 16]);
    let broadcast_id = ChannelId::from_bytes([0x44; 16]);
    let config = YpClientConfig::for_loopback(port);

    let client = PCPYellowPageClient::new(session_id, broadcast_id, config).unwrap();
    client.add_channel(sample_channel()).await;

    tokio::spawn(async move {
        let _ = mock_server.run_once().await;
    });

    let res = client.run_loop().await;
    match res {
        Err(YpError::Banned) => {}
        _ => panic!("Expected Banned error when YP sends dis=1"),
    }

    assert_eq!(client.status(), YpConnectionStatus::TerminalBanned);
}

/// Acceptance criterion 6:
/// Terminal ServerQuit stops retries immediately (REQ-062).
#[tokio::test]
async fn test_p34_terminal_server_quit_stops_retries() {
    let mut mock_server = MockYellowPageServer::bind().await.unwrap();
    mock_server.set_server_quit_mode(true); // Sends quit 0x01000000
    let port = mock_server.port();

    let session_id = ChannelId::from_bytes([0x33; 16]);
    let broadcast_id = ChannelId::from_bytes([0x44; 16]);
    let config = YpClientConfig::for_loopback(port);

    let client = PCPYellowPageClient::new(session_id, broadcast_id, config).unwrap();
    client.add_channel(sample_channel()).await;

    tokio::spawn(async move {
        let _ = mock_server.run_once().await;
    });

    let res = client.run_loop().await;
    match res {
        Err(YpError::ServerQuit) => {}
        _ => panic!("Expected ServerQuit error when YP sends general quit"),
    }

    assert_eq!(client.status(), YpConnectionStatus::TerminalServerQuit);
}

/// Acceptance criterion 7:
/// Channel ownership refcount and shutdown: no announcement remains after final owner removal.
#[tokio::test]
async fn test_p34_ownership_refcount_and_final_owner_removal() {
    let session_id = ChannelId::from_bytes([0x33; 16]);
    let broadcast_id = ChannelId::from_bytes([0x44; 16]);
    let config = YpClientConfig::for_loopback(7144);

    let client = PCPYellowPageClient::new(session_id, broadcast_id, config).unwrap();

    let mut ch1 = sample_channel();
    ch1.channel_id = ChannelId::from_bytes([0x01; 16]);
    let mut ch2 = sample_channel();
    ch2.channel_id = ChannelId::from_bytes([0x02; 16]);

    // Add channels: refcount increments
    client.add_channel(ch1.clone()).await;
    assert_eq!(client.ref_count(), 1);
    client.add_channel(ch2.clone()).await;
    assert_eq!(client.ref_count(), 2);

    // Remove first channel: refcount = 1
    assert!(client.remove_channel(&ch1.channel_id).await);
    assert_eq!(client.ref_count(), 1);

    // Remove final channel: refcount = 0
    assert!(client.remove_channel(&ch2.channel_id).await);
    assert_eq!(client.ref_count(), 0);

    // When refcount == 0, run_loop exits immediately with Idle status (no announcement remains)
    let res = client.run_loop().await;
    assert!(res.is_ok());
    assert_eq!(client.status(), YpConnectionStatus::Idle);
}
