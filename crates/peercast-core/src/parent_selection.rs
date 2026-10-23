//! Servent parent selection policy, scoring, locality detection, and retry coordination (REQ-042, REQ-043).
//!
//! # Architectural Invariants (REQ-043)
//! 1. **Local Policy, Not Wire Order**: Parent selection is a local node policy, not a universal
//!    protocol specification. Station uses a weighted randomized formula prioritizing site-local
//!    nodes (+8000) and factoring receiving status, relay availability, hops, and relay count.
//!    PeerCast-YT uses a greedy hop-minimization policy (`pickHits`).
//! 2. **Pre-scoring Eligibility**: Ineligible nodes (self-loop matching local session ID, matching
//!    local contact socket, port 0, unspecified IP, stale freshness, or active failure cooldown)
//!    MUST be rejected BEFORE scoring. Score can never override structural eligibility.
//! 3. **Locality Determination**: Station matches candidate contact addresses against local output
//!    listener endpoints (`IsSiteLocal`), with fallback for RFC 1918 / loopback / link-local addresses.
//! 4. **Bounded Retry Budget & Backoff**: Reconnect attempts are bounded per selection cycle
//!    (`max_attempts_per_cycle`). Failures trigger backoff delays (starting at 1000ms, multiplied by
//!    3/2 with integer arithmetic, capped at 30,000ms per REQ-062).
//! 5. **Deterministic Testing**: Seeded PRNG ([`DeterministicRng`]) and fixed sequences ([`FixedSequenceRng`])
//!    ensure 100% reproducible parent selection fixtures without timing jitter.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use crate::cache::CacheClock;
use crate::channel_id::ChannelId;
use crate::host::Host;
use crate::host_registry::{
    is_candidate_eligible, EndpointCooldownTable, EndpointIdentity, HostRegistry,
};

/// Trait providing pseudo-random floating-point numbers in [0.0, 1.0) for parent selection.
pub trait RngSource: Send + Sync {
    /// Returns the next random `f64` in the half-open range `[0.0, 1.0)`.
    fn next_f64(&mut self) -> f64;
}

/// A fast, deterministic 64-bit xorshift pseudo-random generator.
#[derive(Debug, Clone)]
pub struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    /// Creates a new generator seeded with a 64-bit value.
    pub fn new(seed: u64) -> Self {
        let s = if seed == 0 {
            0x5465_7374_5365_6564 // Fallback non-zero seed ("TestSeed")
        } else {
            seed
        };
        Self { state: s }
    }

    /// Generates the next pseudo-random 64-bit unsigned integer.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
}

impl RngSource for DeterministicRng {
    fn next_f64(&mut self) -> f64 {
        // Shift top 53 bits to generate standard IEEE 754 float in [0.0, 1.0)
        let val = self.next_u64() >> 11;
        (val as f64) / ((1u64 << 53) as f64)
    }
}

/// A deterministic RNG returning a predefined sequence of values, cycling if exhausted.
#[derive(Debug, Clone)]
pub struct FixedSequenceRng {
    values: Vec<f64>,
    index: usize,
}

impl FixedSequenceRng {
    /// Creates a new fixed sequence generator.
    pub fn new(values: Vec<f64>) -> Self {
        Self { values, index: 0 }
    }
}

impl RngSource for FixedSequenceRng {
    fn next_f64(&mut self) -> f64 {
        if self.values.is_empty() {
            return 0.5;
        }
        let val = self.values[self.index % self.values.len()];
        self.index = self.index.wrapping_add(1);
        val
    }
}

/// A system-seeded pseudo-random generator using current time entropy.
#[derive(Debug, Clone)]
pub struct SystemRng {
    inner: DeterministicRng,
}

impl Default for SystemRng {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemRng {
    /// Creates a new system-seeded PRNG.
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x1234_5678_9ABC_DEF0);
        Self {
            inner: DeterministicRng::new(seed ^ 0x9E37_79B9_7F4A_7C15),
        }
    }
}

impl RngSource for SystemRng {
    fn next_f64(&mut self) -> f64 {
        self.inner.next_f64()
    }
}

/// Detects whether a candidate endpoint is local/site-local to this node (REQ-043).
#[derive(Debug, Clone, Default)]
pub struct LocalityDetector {
    /// Explicitly bound local listener addresses.
    local_listeners: Vec<SocketAddr>,
    /// Whether to treat RFC 1918 / loopback / link-local addresses as site-local.
    allow_private_ranges: bool,
}

impl LocalityDetector {
    /// Creates a new locality detector with the given local listener addresses.
    pub fn new(local_listeners: Vec<SocketAddr>, allow_private_ranges: bool) -> Self {
        Self {
            local_listeners,
            allow_private_ranges,
        }
    }

    /// Determines if the host candidate is considered site-local.
    ///
    /// Parity with Station `PCPSourceStream.IsSiteLocal`:
    /// - If `global_endpoint` is None, Station treats the node as site-local (`true`).
    /// - If `global_endpoint` matches any local listener, it is site-local (`true`).
    /// - If private ranges are enabled, checks standard RFC 1918 / loopback IP classifications.
    pub fn is_site_local(&self, host: &Host) -> bool {
        let ep = match host.global_endpoint {
            Some(ep) => ep,
            None => return true, // Matches Station: null globalEndPoint => site-local
        };

        // 1. Check direct listener equality
        for listener in &self.local_listeners {
            if &ep == listener {
                return true;
            }
        }

        // 2. Check private / loopback IP address ranges if configured
        if self.allow_private_ranges {
            match ep.ip() {
                IpAddr::V4(v4) => {
                    if v4.is_loopback() || v4.is_link_local() {
                        return true;
                    }
                    let octets = v4.octets();
                    // 10.0.0.0/8
                    if octets[0] == 10 {
                        return true;
                    }
                    // 172.16.0.0/12
                    if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                        return true;
                    }
                    // 192.168.0.0/16
                    if octets[0] == 192 && octets[1] == 168 {
                        return true;
                    }
                }
                IpAddr::V6(v6) => {
                    if v6.is_loopback() {
                        return true;
                    }
                    let segments = v6.segments();
                    // fc00::/7 (Unique Local Address)
                    if (segments[0] & 0xFE00) == 0xFC00 {
                        return true;
                    }
                    // fe80::/10 (Link-Local)
                    if (segments[0] & 0xFFC0) == 0xFE80 {
                        return true;
                    }
                }
            }
        }

        false
    }
}

/// Pluggable parent selection policy for evaluating eligible peer nodes (REQ-043).
pub trait ParentSelectionPolicy: Send + Sync {
    /// Diagnostic policy name.
    fn name(&self) -> &'static str;

    /// Computes a priority score for an eligible host candidate. Higher scores are prioritized.
    fn score_candidate(
        &self,
        candidate: &Host,
        is_site_local: bool,
        rng: &mut dyn RngSource,
    ) -> f64;
}

/// Station-compatible weighted randomized parent selection policy (REQ-043).
///
/// Implements the exact scoring algorithm from PeerCastStation `PCPSourceStream.SelectSourceHost`:
/// ```text
/// score = (IsSiteLocal ? 8000 : 0) + rnd.NextDouble() * (
///     (IsReceiving ? 4000 : 0) +
///     (!IsRelayFull ? 2000 : 0) +
///     (max(10 - hops, 0) * 100) +
///     (relayCount * 10)
/// )
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct StationWeightedRandomPolicy;

impl ParentSelectionPolicy for StationWeightedRandomPolicy {
    fn name(&self) -> &'static str {
        "StationWeightedRandomPolicy"
    }

    fn score_candidate(
        &self,
        candidate: &Host,
        is_site_local: bool,
        rng: &mut dyn RngSource,
    ) -> f64 {
        // Station additive boost for site-local nodes
        let local_boost = if is_site_local { 8000.0 } else { 0.0 };

        // Node is actively receiving the content
        let recv_weight = if candidate.flags.is_receiving() {
            4000.0
        } else {
            0.0
        };

        // Node is accepting relays (flg1 RELAY bit set means NOT relay full in Station)
        let relay_weight = if candidate.flags.is_relay() {
            2000.0
        } else {
            0.0
        };

        // Proximity to root tracker: 10 - hops (capped at 0) * 100
        let hop_weight = 10u32.saturating_sub(candidate.hops) as f64 * 100.0;

        // Preference for established relays: relay_count * 10
        let relay_count_weight = candidate.relay_count as f64 * 10.0;

        let base_weight = recv_weight + relay_weight + hop_weight + relay_count_weight;
        let random_factor = rng.next_f64();

        local_boost + (random_factor * base_weight)
    }
}

/// PeerCast-YT compatible greedy hop minimization policy (`pickHits`, REQ-043).
///
/// Demonstrates local policy diversity: minimizes upstream hops among eligible candidates,
/// with site-local tie-breaking.
#[derive(Debug, Clone, Copy, Default)]
pub struct PeerCastYtGreedyHopPolicy;

impl ParentSelectionPolicy for PeerCastYtGreedyHopPolicy {
    fn name(&self) -> &'static str {
        "PeerCastYtGreedyHopPolicy"
    }

    fn score_candidate(
        &self,
        candidate: &Host,
        is_site_local: bool,
        _rng: &mut dyn RngSource,
    ) -> f64 {
        // Minimizing hops: 255 - hops gives highest score to fewest hops
        let hop_score = (255u32.saturating_sub(candidate.hops) as f64) * 1000.0;
        let local_score = if is_site_local { 100.0 } else { 0.0 };
        let recv_score = if candidate.flags.is_receiving() { 10.0 } else { 0.0 };
        hop_score + local_score + recv_score
    }
}

/// Manages attempt counts and backoff delays for parent connection retries (REQ-043, REQ-062).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectBudget {
    /// Maximum connection attempts permitted per selection cycle before forcing backoff.
    pub max_attempts_per_cycle: usize,
    /// Number of attempts executed in current cycle.
    pub current_attempts: usize,
    /// Base backoff duration (default 1,000ms per Station reference policy REQ-062).
    pub base_backoff: Duration,
    /// Maximum capped backoff duration (default 30,000ms per REQ-062).
    pub max_backoff: Duration,
    /// Current dynamic backoff delay.
    pub current_backoff: Duration,
}

impl Default for ReconnectBudget {
    fn default() -> Self {
        Self {
            max_attempts_per_cycle: 5,
            current_attempts: 0,
            base_backoff: Duration::from_millis(1000),
            max_backoff: Duration::from_millis(30000),
            current_backoff: Duration::from_millis(1000),
        }
    }
}

impl ReconnectBudget {
    /// Creates a new reconnect budget with configured limits.
    pub fn new(
        max_attempts_per_cycle: usize,
        base_backoff: Duration,
        max_backoff: Duration,
    ) -> Self {
        Self {
            max_attempts_per_cycle: max_attempts_per_cycle.max(1),
            current_attempts: 0,
            base_backoff,
            max_backoff,
            current_backoff: base_backoff,
        }
    }

    /// Records a failed connection attempt.
    ///
    /// Multiplies backoff duration by 3/2 using integer arithmetic (Station REQ-062),
    /// capped at `max_backoff`.
    pub fn record_failure(&mut self) -> RetryDecision {
        self.current_attempts = self.current_attempts.saturating_add(1);

        // Advance Station 3/2 backoff: delay = (delay * 3) / 2
        let current_ms = self.current_backoff.as_millis() as u64;
        let next_ms = (current_ms.saturating_mul(3) / 2).min(self.max_backoff.as_millis() as u64);
        self.current_backoff = Duration::from_millis(next_ms);

        if self.current_attempts >= self.max_attempts_per_cycle {
            let backoff = self.current_backoff;
            self.current_attempts = 0; // Reset attempt counter for next cycle after backoff
            RetryDecision::Exhausted {
                backoff_delay: backoff,
            }
        } else {
            RetryDecision::RetryImmediately {
                remaining_attempts: self.max_attempts_per_cycle - self.current_attempts,
            }
        }
    }

    /// Resets the attempt counter and backoff timer upon successful connection.
    pub fn reset_success(&mut self) {
        self.current_attempts = 0;
        self.current_backoff = self.base_backoff;
    }
}

/// Outcome of recording a connection attempt failure against the retry budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    /// Remaining attempts exist within current cycle; may try next eligible candidate immediately.
    RetryImmediately { remaining_attempts: usize },
    /// Selection cycle attempts exhausted; must wait for the specified backoff delay before next cycle.
    Exhausted { backoff_delay: Duration },
}

/// Servent parent selection coordinator (REQ-042, REQ-043).
pub struct ParentSelector<P: ParentSelectionPolicy = StationWeightedRandomPolicy> {
    policy: P,
    locality: LocalityDetector,
    self_session_id: ChannelId,
    self_endpoint: Option<SocketAddr>,
    budget: ReconnectBudget,
}

impl Default for ParentSelector<StationWeightedRandomPolicy> {
    fn default() -> Self {
        Self::new(
            StationWeightedRandomPolicy,
            LocalityDetector::default(),
            ChannelId::from_bytes([0u8; 16]),
            None,
            ReconnectBudget::default(),
        )
    }
}

impl<P: ParentSelectionPolicy> ParentSelector<P> {
    /// Constructs a new parent selector with specific policy and configuration.
    pub fn new(
        policy: P,
        locality: LocalityDetector,
        self_session_id: ChannelId,
        self_endpoint: Option<SocketAddr>,
        budget: ReconnectBudget,
    ) -> Self {
        Self {
            policy,
            locality,
            self_session_id,
            self_endpoint,
            budget,
        }
    }

    /// Policy name.
    #[inline]
    pub fn policy_name(&self) -> &'static str {
        self.policy.name()
    }

    /// Access the retry budget.
    #[inline]
    pub fn budget(&self) -> &ReconnectBudget {
        &self.budget
    }

    /// Mutable access to the retry budget.
    #[inline]
    pub fn budget_mut(&mut self) -> &mut ReconnectBudget {
        &mut self.budget
    }

    /// Selects the best eligible parent host for the specified channel.
    ///
    /// # Process
    /// 1. Queries all fresh hosts for `channel_id` from the [`HostRegistry`].
    /// 2. Filters out ineligible hosts using [`is_candidate_eligible`] (self session ID,
    ///    self address, loopback/invalid port, active failure cooldown).
    /// 3. Computes policy scores using [`ParentSelectionPolicy::score_candidate`].
    /// 4. Returns the candidate with the highest score, or `None` if no candidates qualify.
    pub fn select_best_parent<C1: CacheClock, C2: CacheClock>(
        &self,
        channel_id: &ChannelId,
        registry: &HostRegistry<C1>,
        cooldown: &EndpointCooldownTable<C2>,
        rng: &mut dyn RngSource,
    ) -> Option<Host> {
        let candidates = self.rank_candidates(channel_id, registry, cooldown, rng);
        candidates.into_iter().next().map(|(host, _)| host)
    }

    /// Ranks all eligible candidate hosts in descending order of their policy score.
    pub fn rank_candidates<C1: CacheClock, C2: CacheClock>(
        &self,
        channel_id: &ChannelId,
        registry: &HostRegistry<C1>,
        cooldown: &EndpointCooldownTable<C2>,
        rng: &mut dyn RngSource,
    ) -> Vec<(Host, f64)> {
        let fresh_hosts = registry.fresh_hosts_for_channel(channel_id);

        let mut scored = Vec::new();
        for host in fresh_hosts {
            // Structural eligibility & failure cooldown filter (REQ-042, REQ-043)
            if !is_candidate_eligible(
                &host,
                registry,
                cooldown,
                &self.self_session_id,
                self.self_endpoint.as_ref(),
            ) {
                continue;
            }

            let is_local = self.locality.is_site_local(&host);
            let score = self.policy.score_candidate(&host, is_local, rng);
            scored.push((host, score));
        }

        // Sort descending by score (highest priority first)
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });

        scored
    }

    /// Records a failed attempt against an endpoint, updating the cooldown table and budget.
    pub fn record_attempt_failure<C: CacheClock>(
        &mut self,
        endpoint: EndpointIdentity,
        cooldown: &mut EndpointCooldownTable<C>,
        reason: Option<String>,
    ) -> RetryDecision {
        cooldown.record_failure(endpoint, reason);
        self.budget.record_failure()
    }

    /// Resets the retry budget upon successful connection.
    pub fn record_attempt_success(&mut self) {
        self.budget.reset_success();
    }
}
