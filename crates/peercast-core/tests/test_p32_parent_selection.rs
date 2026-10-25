use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use peercast_core::cache::FakeClock;
use peercast_core::channel_id::ChannelId;
use peercast_core::host::{Host, HostFlags1};
use peercast_core::host_registry::{
    EndpointCooldownTable, EndpointIdentity, HostRegistry, DEFAULT_COOLDOWN_DURATION,
    DEFAULT_FRESHNESS_THRESHOLD,
};
use peercast_core::parent_selection::{
    DeterministicRng, FixedSequenceRng, LocalityDetector, ParentSelectionPolicy, ParentSelector,
    PeerCastYtGreedyHopPolicy, ReconnectBudget, RetryDecision, StationWeightedRandomPolicy,
};

fn make_candidate(
    sid_byte: u8,
    addr: SocketAddr,
    flags: HostFlags1,
    hops: u32,
    relay_count: u32,
) -> Host {
    let mut cid_bytes = [0u8; 16];
    cid_bytes[0] = 0xAA;
    let channel_id = ChannelId::from_bytes(cid_bytes);

    let mut sid_bytes = [0u8; 16];
    sid_bytes[0] = sid_byte;
    let session_id = ChannelId::from_bytes(sid_bytes);

    let mut host = Host::new(session_id, channel_id);
    host.global_endpoint = Some(addr);
    host.flags = flags;
    host.hops = hops;
    host.relay_count = relay_count;
    host
}

/// Acceptance criterion 1:
/// Exact Station-compatible weighted randomized scoring formula verification.
#[test]
fn test_p32_station_weighted_random_scoring_exact_formula() {
    let policy = StationWeightedRandomPolicy;

    let addr: SocketAddr = "192.168.1.10:7144".parse().unwrap();
    // Candidate 1: receiving, relay available, hops = 2, relay_count = 3
    let mut flags = HostFlags1::default();
    flags = HostFlags1(flags.raw() | HostFlags1::RECV.raw() | HostFlags1::RELAY.raw());
    let c1 = make_candidate(1, addr, flags, 2, 3);

    // Test with RNG = 1.0 (maximum random scale)
    let mut rng_one = FixedSequenceRng::new(vec![1.0]);
    // Site-local:
    // local_boost = 8000
    // base_weight = 4000 (recv) + 2000 (relay) + (10 - 2)*100 (hop) + 3*10 (relay_count) = 6830.0
    // total = 8000.0 + 1.0 * 6830.0 = 14830.0
    let score_local = policy.score_candidate(&c1, true, &mut rng_one);
    assert_eq!(score_local, 14830.0);

    // Non-site-local with RNG = 1.0:
    let mut rng_one = FixedSequenceRng::new(vec![1.0]);
    let score_remote = policy.score_candidate(&c1, false, &mut rng_one);
    assert_eq!(score_remote, 6830.0);

    // Test with RNG = 0.5:
    let mut rng_half = FixedSequenceRng::new(vec![0.5]);
    // local_boost (8000) + 0.5 * 6830 = 8000 + 3415 = 11415.0
    let score_half = policy.score_candidate(&c1, true, &mut rng_half);
    assert_eq!(score_half, 11415.0);

    // Candidate 2: not receiving, relay full (no RELAY flag), hops = 12 (> 10), relay_count = 0
    let c2 = make_candidate(2, addr, HostFlags1::default(), 12, 0);
    let mut rng_one = FixedSequenceRng::new(vec![1.0]);
    // base_weight = 0 + 0 + 0 + 0 = 0
    let score_zero_weight = policy.score_candidate(&c2, false, &mut rng_one);
    assert_eq!(score_zero_weight, 0.0);
}

/// Acceptance criterion 2:
/// Locality additive boost (+8000) dominance over remote candidates.
#[test]
fn test_p32_locality_additive_dominance() {
    let policy = StationWeightedRandomPolicy;

    // Remote "perfect" candidate:
    // receiving (+4000), relay (+2000), hops 0 (+1000), relay_count 10 (+100) = 7100 max weight
    let remote_addr: SocketAddr = "8.8.8.8:7144".parse().unwrap();
    let mut flags = HostFlags1::default();
    flags = HostFlags1(flags.raw() | HostFlags1::RECV.raw() | HostFlags1::RELAY.raw());
    let remote_candidate = make_candidate(10, remote_addr, flags, 0, 10);

    // Local "modest" candidate:
    // not receiving (0), relay full (0), hops 10 (0), relay_count 0 (0) = 0 base weight
    let local_addr: SocketAddr = "192.168.1.50:7144".parse().unwrap();
    let local_candidate = make_candidate(11, local_addr, HostFlags1::default(), 10, 0);

    // Even with RNG = 1.0 giving remote candidate its maximum possible score (7100):
    let mut rng = FixedSequenceRng::new(vec![1.0]);
    let score_remote = policy.score_candidate(&remote_candidate, false, &mut rng);
    let score_local = policy.score_candidate(&local_candidate, true, &mut rng);

    assert_eq!(score_remote, 7100.0);
    assert_eq!(score_local, 8000.0);
    // Local candidate dominates because of +8000 site-local boost
    assert!(
        score_local > score_remote,
        "Site-local boost (+8000) must dominate over remote candidates in Station policy"
    );
}

/// Acceptance criterion 3:
/// Pre-scoring eligibility: ineligible nodes MUST be rejected regardless of potential score.
#[test]
fn test_p32_pre_scoring_eligibility_enforcement() {
    let clock = Arc::new(FakeClock::new(0));
    let mut registry = HostRegistry::with_clock(DEFAULT_FRESHNESS_THRESHOLD, 512, clock.clone());
    let mut cooldown =
        EndpointCooldownTable::with_clock(DEFAULT_COOLDOWN_DURATION, 512, clock.clone());

    let self_sid = ChannelId::from_bytes([0xEE; 16]);
    let self_addr: SocketAddr = "10.0.0.100:7144".parse().unwrap();

    let locality = LocalityDetector::new(vec![self_addr], true);
    let selector = ParentSelector::new(
        StationWeightedRandomPolicy,
        locality,
        self_sid,
        Some(self_addr),
        ReconnectBudget::default(),
    );

    let channel_id = ChannelId::from_bytes([0xAA; 16]);

    // 1. High scoring host that matches self session ID (self-loop): MUST BE EXCLUDED
    let mut self_host = make_candidate(
        0xEE,
        "10.0.0.1:7144".parse().unwrap(),
        HostFlags1::RELAY,
        0,
        5,
    );
    self_host.channel_id = channel_id;
    self_host.session_id = self_sid;
    registry.upsert(self_host);

    // 2. High scoring host that matches self contact socket: MUST BE EXCLUDED
    let mut self_socket_host = make_candidate(0xEF, self_addr, HostFlags1::RELAY, 0, 5);
    self_socket_host.channel_id = channel_id;
    registry.upsert(self_socket_host);

    // 3. High scoring host currently in failure cooldown: MUST BE EXCLUDED
    let cooled_addr: SocketAddr = "10.0.0.2:7144".parse().unwrap();
    let mut cooled_host = make_candidate(0x02, cooled_addr, HostFlags1::RELAY, 0, 5);
    cooled_host.channel_id = channel_id;
    registry.upsert(cooled_host.clone());
    cooldown.record_failure(EndpointIdentity::for_channel(channel_id, cooled_addr), None);

    // 4. Stale host: MUST BE EXCLUDED
    let stale_addr: SocketAddr = "10.0.0.3:7144".parse().unwrap();
    let mut stale_host = make_candidate(0x03, stale_addr, HostFlags1::RELAY, 0, 5);
    stale_host.channel_id = channel_id;
    clock.set(Duration::from_secs(10));
    registry.upsert(stale_host);
    // Advance clock past freshness threshold
    clock.set(Duration::from_secs(250));

    // 5. Eligible modest host
    let eligible_addr: SocketAddr = "10.0.0.4:7144".parse().unwrap();
    let mut eligible_host = make_candidate(0x04, eligible_addr, HostFlags1::RELAY, 5, 1);
    eligible_host.channel_id = channel_id;
    registry.upsert(eligible_host.clone());

    // Run selection
    let mut rng = DeterministicRng::new(42);
    let best = selector.select_best_parent(&channel_id, &registry, &cooldown, &mut rng);

    assert!(best.is_some());
    let chosen = best.unwrap();
    // Only the eligible host (0x04) must be selected!
    assert_eq!(chosen.session_id, eligible_host.session_id);
    assert_eq!(chosen.contact_address(), Some(eligible_addr));
}

/// Acceptance criterion 4:
/// Deterministic fixture selection with seeded PRNG.
#[test]
fn test_p32_deterministic_seeded_fixture_selection() {
    let clock = Arc::new(FakeClock::new(0));
    let mut registry = HostRegistry::with_clock(DEFAULT_FRESHNESS_THRESHOLD, 512, clock.clone());
    let cooldown = EndpointCooldownTable::with_clock(DEFAULT_COOLDOWN_DURATION, 512, clock.clone());

    let channel_id = ChannelId::from_bytes([0xAA; 16]);
    let self_sid = ChannelId::from_bytes([0x99; 16]);

    // Insert 4 candidates with varied attributes
    for i in 1..=4 {
        let addr: SocketAddr = format!("10.0.0.{}:7144", i).parse().unwrap();
        let flags = if i % 2 == 0 {
            HostFlags1::RELAY
        } else {
            HostFlags1(HostFlags1::RELAY.raw() | HostFlags1::RECV.raw())
        };
        let mut host = make_candidate(i as u8, addr, flags, i as u32, (5 - i) as u32);
        host.channel_id = channel_id;
        registry.upsert(host);
    }

    let selector = ParentSelector::new(
        StationWeightedRandomPolicy,
        LocalityDetector::default(),
        self_sid,
        None,
        ReconnectBudget::default(),
    );

    // Run 1 with seed 0xCAFE_BABE
    let mut rng1 = DeterministicRng::new(0xCAFE_BABE);
    let ranked1 = selector.rank_candidates(&channel_id, &registry, &cooldown, &mut rng1);

    // Run 2 with identical seed
    let mut rng2 = DeterministicRng::new(0xCAFE_BABE);
    let ranked2 = selector.rank_candidates(&channel_id, &registry, &cooldown, &mut rng2);

    assert_eq!(ranked1.len(), 4);
    assert_eq!(ranked1.len(), ranked2.len());
    for (r1, r2) in ranked1.iter().zip(ranked2.iter()) {
        assert_eq!(r1.0.session_id, r2.0.session_id);
        assert_eq!(r1.1, r2.1);
    }
}

/// Acceptance criterion 5:
/// Named policy diversity: Station weighted random vs PeerCast-YT greedy hop minimization.
#[test]
fn test_p32_policy_diversity_peercast_yt_greedy_differs_from_station() {
    let clock = Arc::new(FakeClock::new(0));
    let mut registry = HostRegistry::with_clock(DEFAULT_FRESHNESS_THRESHOLD, 512, clock.clone());
    let cooldown = EndpointCooldownTable::with_clock(DEFAULT_COOLDOWN_DURATION, 512, clock.clone());

    let channel_id = ChannelId::from_bytes([0xAA; 16]);
    let self_sid = ChannelId::from_bytes([0x99; 16]);

    // Candidate A: Hops = 1 (very close to root), but remote (not site-local)
    let addr_a: SocketAddr = "8.8.8.8:7144".parse().unwrap();
    let mut host_a = make_candidate(1, addr_a, HostFlags1::RELAY, 1, 0);
    host_a.channel_id = channel_id;
    registry.upsert(host_a.clone());

    // Candidate B: Hops = 8 (distant from root), but site-local
    let addr_b: SocketAddr = "192.168.1.200:7144".parse().unwrap();
    let mut host_b = make_candidate(2, addr_b, HostFlags1::RELAY, 8, 0);
    host_b.channel_id = channel_id;
    registry.upsert(host_b.clone());

    let locality = LocalityDetector::new(vec!["192.168.1.1:7144".parse().unwrap()], true);

    // 1. Station Policy: Host B wins due to +8000 site-local boost
    let station_selector = ParentSelector::new(
        StationWeightedRandomPolicy,
        locality.clone(),
        self_sid,
        None,
        ReconnectBudget::default(),
    );
    let mut rng = FixedSequenceRng::new(vec![0.5]);
    let best_station = station_selector
        .select_best_parent(&channel_id, &registry, &cooldown, &mut rng)
        .unwrap();
    assert_eq!(
        best_station.session_id, host_b.session_id,
        "Station policy must prioritize site-local host B over remote host A"
    );

    // 2. PeerCast-YT Policy: Host A wins because it minimizes upstream hops (1 < 8)
    let yt_selector = ParentSelector::new(
        PeerCastYtGreedyHopPolicy,
        locality,
        self_sid,
        None,
        ReconnectBudget::default(),
    );
    let mut rng_dummy = FixedSequenceRng::new(vec![0.0]);
    let best_yt = yt_selector
        .select_best_parent(&channel_id, &registry, &cooldown, &mut rng_dummy)
        .unwrap();
    assert_eq!(
        best_yt.session_id, host_a.session_id,
        "PeerCast-YT policy must prioritize minimum hops (host A) over distant host B"
    );
}

/// Acceptance criterion 6:
/// Station reference retry budget and 3/2 integer backoff arithmetic (REQ-043, REQ-062).
#[test]
fn test_p32_reconnect_budget_and_station_backoff_arithmetic() {
    let mut budget = ReconnectBudget::new(
        5,                            // max 5 attempts per cycle
        Duration::from_millis(1000),  // base 1000ms
        Duration::from_millis(30000), // cap 30000ms
    );

    // Attempt 1 fails: backoff = 1000 * 3 / 2 = 1500ms
    match budget.record_failure() {
        RetryDecision::RetryImmediately { remaining_attempts } => {
            assert_eq!(remaining_attempts, 4);
            assert_eq!(budget.current_backoff, Duration::from_millis(1500));
        }
        _ => panic!("Expected RetryImmediately on attempt 1"),
    }

    // Attempt 2 fails: backoff = 1500 * 3 / 2 = 2250ms
    match budget.record_failure() {
        RetryDecision::RetryImmediately { remaining_attempts } => {
            assert_eq!(remaining_attempts, 3);
            assert_eq!(budget.current_backoff, Duration::from_millis(2250));
        }
        _ => panic!("Expected RetryImmediately on attempt 2"),
    }

    // Attempt 3 fails: backoff = 2250 * 3 / 2 = 3375ms
    match budget.record_failure() {
        RetryDecision::RetryImmediately { remaining_attempts } => {
            assert_eq!(remaining_attempts, 2);
            assert_eq!(budget.current_backoff, Duration::from_millis(3375));
        }
        _ => panic!("Expected RetryImmediately on attempt 3"),
    }

    // Attempt 4 fails: backoff = 3375 * 3 / 2 = 5062ms
    match budget.record_failure() {
        RetryDecision::RetryImmediately { remaining_attempts } => {
            assert_eq!(remaining_attempts, 1);
            assert_eq!(budget.current_backoff, Duration::from_millis(5062));
        }
        _ => panic!("Expected RetryImmediately on attempt 4"),
    }

    // Attempt 5 fails: backoff = 5062 * 3 / 2 = 7593ms. Cycle EXHAUSTED!
    match budget.record_failure() {
        RetryDecision::Exhausted { backoff_delay } => {
            assert_eq!(backoff_delay, Duration::from_millis(7593));
            assert_eq!(budget.current_attempts, 0); // Reset for next cycle
        }
        _ => panic!("Expected Exhausted on attempt 5"),
    }

    // Verify capping at 30,000ms
    for _ in 0..10 {
        budget.record_failure();
    }
    assert_eq!(
        budget.current_backoff,
        Duration::from_millis(30000),
        "Backoff must not exceed max_backoff cap (30,000ms)"
    );

    // Successful connection resets budget
    budget.reset_success();
    assert_eq!(budget.current_attempts, 0);
    assert_eq!(budget.current_backoff, Duration::from_millis(1000));
}

/// Locality detector validation (direct listeners, RFC 1918, loopback, and null global).
#[test]
fn test_p32_locality_detector_rules() {
    let listener: SocketAddr = "203.0.113.50:7144".parse().unwrap();
    let detector = LocalityDetector::new(vec![listener], true);

    // 1. Direct match on local listener endpoint
    let h1 = make_candidate(1, listener, HostFlags1::default(), 0, 0);
    assert!(detector.is_site_local(&h1));

    // 2. RFC 1918 private IPv4 addresses
    let h_192 = make_candidate(
        2,
        "192.168.1.1:7144".parse().unwrap(),
        HostFlags1::default(),
        0,
        0,
    );
    assert!(detector.is_site_local(&h_192));

    let h_10 = make_candidate(
        3,
        "10.250.0.1:7144".parse().unwrap(),
        HostFlags1::default(),
        0,
        0,
    );
    assert!(detector.is_site_local(&h_10));

    let h_172 = make_candidate(
        4,
        "172.20.10.5:7144".parse().unwrap(),
        HostFlags1::default(),
        0,
        0,
    );
    assert!(detector.is_site_local(&h_172));

    // 3. Loopback
    let h_loop = make_candidate(
        5,
        "127.0.0.1:7144".parse().unwrap(),
        HostFlags1::default(),
        0,
        0,
    );
    assert!(detector.is_site_local(&h_loop));

    // 4. Public IP not matching listener
    let h_public = make_candidate(
        6,
        "198.51.100.1:7144".parse().unwrap(),
        HostFlags1::default(),
        0,
        0,
    );
    assert!(!detector.is_site_local(&h_public));

    // 5. Host without global_endpoint (null global): treated as site-local (Station parity)
    let mut h_null_global = make_candidate(
        7,
        "10.0.0.1:7144".parse().unwrap(),
        HostFlags1::default(),
        0,
        0,
    );
    h_null_global.global_endpoint = None;
    assert!(detector.is_site_local(&h_null_global));
}
