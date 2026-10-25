use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use peercast_core::cache::FakeClock;
use peercast_core::channel_id::ChannelId;
use peercast_core::host::{Host, HostFlags1};
use peercast_core::host_registry::{
    is_candidate_eligible, EndpointCooldownTable, EndpointIdentity, HostRegistry, UpsertAction,
    DEFAULT_COOLDOWN_DURATION, DEFAULT_FRESHNESS_THRESHOLD,
};

fn make_host(cid_byte: u8, sid_byte: u8, addr: SocketAddr, relay_count: u32, hops: u32) -> Host {
    let mut cid_bytes = [0u8; 16];
    cid_bytes[0] = cid_byte;
    let channel_id = ChannelId::from_bytes(cid_bytes);

    let mut sid_bytes = [0u8; 16];
    sid_bytes[0] = sid_byte;
    let session_id = ChannelId::from_bytes(sid_bytes);

    let mut host = Host::new(session_id, channel_id);
    host.global_endpoint = Some(addr);
    host.relay_count = relay_count;
    host.hops = hops;
    host.flags = HostFlags1::RELAY;
    host
}

/// Acceptance criterion 1:
/// Fake-clock tests prove freshness does not accidentally clear failure cooldown or vice versa.
#[test]
fn test_p31_freshness_does_not_clear_cooldown() {
    let clock = Arc::new(FakeClock::new(0));
    let mut registry = HostRegistry::with_clock(
        DEFAULT_FRESHNESS_THRESHOLD, // 180s
        512,
        clock.clone(),
    );
    let mut cooldown = EndpointCooldownTable::with_clock(
        DEFAULT_COOLDOWN_DURATION, // 180s
        512,
        clock.clone(),
    );

    let self_sid = ChannelId::from_bytes([0xEE; 16]);
    let addr: SocketAddr = "192.168.1.100:7144".parse().unwrap();
    let host_a = make_host(1, 10, addr, 0, 1);
    let channel_id = host_a.channel_id;

    // t = 10s: Register Host A in registry
    clock.advance(Duration::from_secs(10));
    assert_eq!(registry.upsert(host_a.clone()), UpsertAction::Inserted);
    assert!(registry.is_fresh(&channel_id, &host_a.session_id));
    assert!(!cooldown.is_cooled_down(&EndpointIdentity::for_channel(channel_id, addr)));
    assert!(is_candidate_eligible(
        &host_a, &registry, &cooldown, &self_sid, None
    ));

    // t = 20s: Connection fails to Host A. Place Host A in failure cooldown.
    clock.advance(Duration::from_secs(10));
    cooldown.record_failure(
        EndpointIdentity::for_channel(channel_id, addr),
        Some("Connection refused".to_string()),
    );
    assert!(cooldown.is_cooled_down(&EndpointIdentity::for_channel(channel_id, addr)));
    // Ineligible because in cooldown
    assert!(!is_candidate_eligible(
        &host_a, &registry, &cooldown, &self_sid, None
    ));

    // t = 30s: A gossip packet announces Host A is alive (e.g. updated hops or uptime).
    clock.advance(Duration::from_secs(10));
    let mut updated_host_a = host_a.clone();
    updated_host_a.hops = 2;
    assert_eq!(
        registry.upsert(updated_host_a.clone()),
        UpsertAction::Updated
    );

    // CRITICAL REQ-042 INVARIANT:
    // Freshness update MUST NOT clear active failure cooldown!
    assert!(
        cooldown.is_cooled_down(&EndpointIdentity::for_channel(channel_id, addr)),
        "Gossip/freshness update must NEVER clear active failure cooldown!"
    );
    assert!(
        !is_candidate_eligible(&updated_host_a, &registry, &cooldown, &self_sid, None),
        "Candidate must remain ineligible while failure cooldown is active"
    );

    // t = 190s:
    // Failure age: 190s - 20s = 170s <= 180s (still cooling down!)
    // Host age:    190s - 30s = 160s <= 180s (still fresh)
    clock.set(Duration::from_secs(190));
    assert!(cooldown.is_cooled_down(&EndpointIdentity::for_channel(channel_id, addr)));
    assert!(registry.is_fresh(&channel_id, &host_a.session_id));
    assert!(!is_candidate_eligible(
        &updated_host_a,
        &registry,
        &cooldown,
        &self_sid,
        None
    ));

    // t = 205s:
    // Failure age: 205s - 20s = 185s > 180s (Cooldown has now EXPIRED!)
    // Host age:    205s - 30s = 175s <= 180s (Freshness still active because of t=30 refresh!)
    clock.set(Duration::from_secs(205));
    assert!(
        !cooldown.is_cooled_down(&EndpointIdentity::for_channel(channel_id, addr)),
        "Failure cooldown should have expired after 180 seconds"
    );
    assert!(
        registry.is_fresh(&channel_id, &host_a.session_id),
        "Host should still be fresh based on its own refresh timer"
    );
    // Now eligible again!
    assert!(is_candidate_eligible(
        &updated_host_a,
        &registry,
        &cooldown,
        &self_sid,
        None
    ));

    // t = 215s:
    // Host age: 215s - 30s = 185s > 180s (Host freshness has now EXPIRED!)
    clock.set(Duration::from_secs(215));
    assert!(!registry.is_fresh(&channel_id, &host_a.session_id));
    assert!(!is_candidate_eligible(
        &updated_host_a,
        &registry,
        &cooldown,
        &self_sid,
        None
    ));
}

/// Acceptance criterion 2:
/// Failure recording does not modify or reset host freshness timestamp or node age.
#[test]
fn test_p31_failure_does_not_modify_host_freshness() {
    let clock = Arc::new(FakeClock::new(0));
    let mut registry = HostRegistry::with_clock(Duration::from_secs(180), 512, clock.clone());
    let mut cooldown =
        EndpointCooldownTable::with_clock(Duration::from_secs(180), 512, clock.clone());

    let addr: SocketAddr = "10.0.0.1:7144".parse().unwrap();
    let host_b = make_host(2, 20, addr, 1, 1);
    let channel_id = host_b.channel_id;

    // t = 10s: Register Host B
    clock.set(Duration::from_secs(10));
    registry.upsert(host_b.clone());

    // t = 70s: Host B age is 60s
    clock.set(Duration::from_secs(70));
    let raw_entry = registry.get_raw(&channel_id, &host_b.session_id).unwrap();
    assert_eq!(
        raw_entry.age_at(Duration::from_secs(70)),
        Duration::from_secs(60)
    );

    // Record failure in cooldown table at t = 70s
    cooldown.record_failure(
        EndpointIdentity::for_channel(channel_id, addr),
        Some("Timeout".to_string()),
    );

    // Host B's registration timestamp MUST remain 10s and age at 70s MUST remain 60s
    let raw_entry_after = registry.get_raw(&channel_id, &host_b.session_id).unwrap();
    assert_eq!(raw_entry_after.last_seen_offset, Duration::from_secs(10));
    assert_eq!(
        raw_entry_after.age_at(Duration::from_secs(70)),
        Duration::from_secs(60),
        "Failure cooldown recording must NOT alter node age or last_seen timestamp in registry"
    );
}

/// Stop / block guard:
/// Verify independent thresholds (single 180s timer does NOT control unrelated concepts).
#[test]
fn test_p31_independent_threshold_configuration() {
    let clock = Arc::new(FakeClock::new(0));
    // Host freshness = 60s, cooldown = 300s
    let mut registry = HostRegistry::with_clock(Duration::from_secs(60), 512, clock.clone());
    let mut cooldown =
        EndpointCooldownTable::with_clock(Duration::from_secs(300), 512, clock.clone());

    let addr: SocketAddr = "10.1.1.1:7144".parse().unwrap();
    let host = make_host(3, 30, addr, 0, 1);
    let cid = host.channel_id;
    let ep = EndpointIdentity::for_channel(cid, addr);

    registry.upsert(host.clone());
    cooldown.record_failure(ep.clone(), None);

    // t = 70s:
    // Freshness expired (70s > 60s)
    // Cooldown still active (70s <= 300s)
    clock.set(Duration::from_secs(70));
    assert!(!registry.is_fresh(&cid, &host.session_id));
    assert!(cooldown.is_cooled_down(&ep));

    // Reverse configuration: Host freshness = 300s, cooldown = 60s
    let mut registry2 = HostRegistry::with_clock(Duration::from_secs(300), 512, clock.clone());
    let mut cooldown2 =
        EndpointCooldownTable::with_clock(Duration::from_secs(60), 512, clock.clone());

    clock.set(Duration::from_secs(0));
    registry2.upsert(host.clone());
    cooldown2.record_failure(ep.clone(), None);

    // t = 70s:
    // Cooldown expired (70s > 60s)
    // Host freshness still valid (70s <= 300s)
    clock.set(Duration::from_secs(70));
    assert!(registry2.is_fresh(&cid, &host.session_id));
    assert!(!cooldown2.is_cooled_down(&ep));
}

/// Session and endpoint deduplication (REQ-042).
#[test]
fn test_p31_session_and_endpoint_deduplication() {
    let clock = Arc::new(FakeClock::new(0));
    let mut registry = HostRegistry::with_clock(DEFAULT_FRESHNESS_THRESHOLD, 512, clock.clone());

    let addr: SocketAddr = "172.16.0.5:7144".parse().unwrap();
    let host_v1 = make_host(5, 50, addr, 1, 2);
    let cid = host_v1.channel_id;
    let sid_1 = host_v1.session_id;

    // 1. Initial insert
    assert_eq!(registry.upsert(host_v1.clone()), UpsertAction::Inserted);
    assert_eq!(registry.len(), 1);

    // 2. Metadata update with same session ID
    let mut host_v2 = host_v1.clone();
    host_v2.relay_count = 5;
    host_v2.hops = 3;
    assert_eq!(registry.upsert(host_v2.clone()), UpsertAction::Updated);
    assert_eq!(registry.len(), 1);
    let fresh = registry.get_fresh(&cid, &sid_1).unwrap();
    assert_eq!(fresh.relay_count, 5);
    assert_eq!(fresh.hops, 3);

    // 3. Peer restarts: same IP:port, but NEW session ID
    let host_v3 = make_host(5, 51, addr, 0, 1);
    let sid_2 = host_v3.session_id;
    assert_ne!(sid_1, sid_2);

    assert_eq!(
        registry.upsert(host_v3.clone()),
        UpsertAction::ReplacedEndpoint
    );
    // Old session_1 is replaced, count remains 1
    assert_eq!(registry.len(), 1);
    assert!(registry.get_fresh(&cid, &sid_1).is_none());
    assert!(registry.get_fresh(&cid, &sid_2).is_some());
}

/// Bounded memory and physical cleanup (REQ-042, Global Rule 3).
#[test]
fn test_p31_physical_cleanup_and_bounded_capacity() {
    let clock = Arc::new(FakeClock::new(0));

    // Test HostRegistry bounded capacity
    let mut registry = HostRegistry::with_clock(
        Duration::from_secs(100),
        2, // max_hosts = 2
        clock.clone(),
    );

    let h1 = make_host(1, 1, "1.1.1.1:7144".parse().unwrap(), 0, 1);
    let h2 = make_host(1, 2, "1.1.1.2:7144".parse().unwrap(), 0, 1);
    let h3 = make_host(1, 3, "1.1.1.3:7144".parse().unwrap(), 0, 1);

    clock.set(Duration::from_secs(10));
    registry.upsert(h1.clone());
    clock.set(Duration::from_secs(20));
    registry.upsert(h2.clone());
    assert_eq!(registry.len(), 2);

    // Insert h3 at t=30: capacity 2 is exceeded.
    // Oldest entry (h1 at t=10) should be evicted.
    clock.set(Duration::from_secs(30));
    registry.upsert(h3.clone());
    assert_eq!(registry.len(), 2);
    assert!(registry.get_raw(&h1.channel_id, &h1.session_id).is_none());
    assert!(registry.get_raw(&h2.channel_id, &h2.session_id).is_some());
    assert!(registry.get_raw(&h3.channel_id, &h3.session_id).is_some());

    // Advance clock past expiration (t = 200s, elapsed > 100s)
    clock.set(Duration::from_secs(200));
    // Physical pruning releases heap memory
    let pruned = registry.prune_expired();
    assert_eq!(pruned, 2);
    assert_eq!(registry.len(), 0);
    assert!(registry.is_empty());

    // Test EndpointCooldownTable bounded capacity and physical pruning
    let mut cooldown = EndpointCooldownTable::with_clock(
        Duration::from_secs(100),
        2, // max_entries = 2
        clock.clone(),
    );

    let ep1 = EndpointIdentity::global("2.2.2.1:7144".parse().unwrap());
    let ep2 = EndpointIdentity::global("2.2.2.2:7144".parse().unwrap());
    let ep3 = EndpointIdentity::global("2.2.2.3:7144".parse().unwrap());

    clock.set(Duration::from_secs(10));
    cooldown.record_failure(ep1.clone(), None);
    clock.set(Duration::from_secs(20));
    cooldown.record_failure(ep2.clone(), None);
    assert_eq!(cooldown.len(), 2);

    // Record failure for ep3 at t=30: evicts ep1 (oldest failure)
    clock.set(Duration::from_secs(30));
    cooldown.record_failure(ep3.clone(), None);
    assert_eq!(cooldown.len(), 2);
    assert!(!cooldown.is_cooled_down(&ep1));
    assert!(cooldown.is_cooled_down(&ep2));
    assert!(cooldown.is_cooled_down(&ep3));

    // Advance clock past cooldown duration (t = 200s)
    clock.set(Duration::from_secs(200));
    let pruned_cd = cooldown.prune_expired();
    assert_eq!(pruned_cd, 2);
    assert_eq!(cooldown.len(), 0);
    assert!(cooldown.is_empty());
}

/// Global vs channel-specific failure cooldown semantics.
#[test]
fn test_p31_global_vs_channel_cooldown() {
    let clock = Arc::new(FakeClock::new(0));
    let mut cooldown =
        EndpointCooldownTable::with_clock(Duration::from_secs(180), 512, clock.clone());

    let addr: SocketAddr = "192.168.0.50:7144".parse().unwrap();
    let cid_1 = ChannelId::from_bytes([0x01; 16]);
    let cid_2 = ChannelId::from_bytes([0x02; 16]);

    // 1. Channel 1 specific failure (e.g. 404 Not Found on channel 1)
    cooldown.record_failure(
        EndpointIdentity::for_channel(cid_1, addr),
        Some("404 Channel Not Found".to_string()),
    );

    assert!(cooldown.is_cooled_down(&EndpointIdentity::for_channel(cid_1, addr)));
    // Channel 2 should NOT be cooled down by Channel 1's failure
    assert!(!cooldown.is_cooled_down(&EndpointIdentity::for_channel(cid_2, addr)));

    // 2. Host-wide global failure (e.g. Connection Refused / Host Down)
    let addr_global: SocketAddr = "192.168.0.60:7144".parse().unwrap();
    cooldown.record_failure(
        EndpointIdentity::global(addr_global),
        Some("Connection Refused".to_string()),
    );

    // Global cooldown applies to any channel queried on that host address
    assert!(cooldown.is_cooled_down(&EndpointIdentity::global(addr_global)));
    assert!(cooldown.is_cooled_down(&EndpointIdentity::for_channel(cid_1, addr_global)));
    assert!(cooldown.is_cooled_down(&EndpointIdentity::for_channel(cid_2, addr_global)));
}

/// Candidate peer eligibility matrix (REQ-042, REQ-043).
#[test]
fn test_p31_candidate_eligibility_matrix() {
    let clock = Arc::new(FakeClock::new(0));
    let mut registry = HostRegistry::with_clock(DEFAULT_FRESHNESS_THRESHOLD, 512, clock.clone());
    let mut cooldown =
        EndpointCooldownTable::with_clock(DEFAULT_COOLDOWN_DURATION, 512, clock.clone());

    let self_sid = ChannelId::from_bytes([0xAA; 16]);
    let self_addr: SocketAddr = "10.0.0.99:7144".parse().unwrap();

    let valid_addr: SocketAddr = "10.0.0.1:7144".parse().unwrap();
    let valid_host = make_host(1, 1, valid_addr, 0, 1);
    registry.upsert(valid_host.clone());

    // Case 1: Fresh host, not cooled down, valid address -> ELIGIBLE
    assert!(is_candidate_eligible(
        &valid_host,
        &registry,
        &cooldown,
        &self_sid,
        Some(&self_addr)
    ));

    // Case 2: Cooled down host -> INELIGIBLE
    cooldown.record_failure(
        EndpointIdentity::for_channel(valid_host.channel_id, valid_addr),
        None,
    );
    assert!(!is_candidate_eligible(
        &valid_host,
        &registry,
        &cooldown,
        &self_sid,
        Some(&self_addr)
    ));
    cooldown.clear_all();

    // Case 3: Stale host (not fresh) -> INELIGIBLE
    clock.advance(Duration::from_secs(200));
    assert!(!is_candidate_eligible(
        &valid_host,
        &registry,
        &cooldown,
        &self_sid,
        Some(&self_addr)
    ));
    clock.set(Duration::from_secs(0)); // Reset clock
    registry.upsert(valid_host.clone()); // Re-freshen

    // Case 4: Self session ID loopback -> INELIGIBLE
    let mut self_host = valid_host.clone();
    self_host.session_id = self_sid;
    assert!(!is_candidate_eligible(
        &self_host,
        &registry,
        &cooldown,
        &self_sid,
        Some(&self_addr)
    ));

    // Case 5: Self socket address loopback -> INELIGIBLE
    let mut self_addr_host = valid_host.clone();
    self_addr_host.global_endpoint = Some(self_addr);
    assert!(!is_candidate_eligible(
        &self_addr_host,
        &registry,
        &cooldown,
        &self_sid,
        Some(&self_addr)
    ));

    // Case 6: Unspecified IP address (0.0.0.0) -> INELIGIBLE
    let mut unspec_host = valid_host.clone();
    unspec_host.global_endpoint = Some("0.0.0.0:7144".parse().unwrap());
    assert!(!is_candidate_eligible(
        &unspec_host,
        &registry,
        &cooldown,
        &self_sid,
        Some(&self_addr)
    ));

    // Case 7: Port 0 -> INELIGIBLE
    let mut port0_host = valid_host.clone();
    port0_host.global_endpoint = Some("10.0.0.1:0".parse().unwrap());
    assert!(!is_candidate_eligible(
        &port0_host,
        &registry,
        &cooldown,
        &self_sid,
        Some(&self_addr)
    ));
}
