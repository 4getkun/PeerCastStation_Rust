//! Runtime configuration for channels and protocol resource limits.

use std::collections::HashSet;
use std::net::SocketAddr;

use peercast_core::gossip::{DEFAULT_MAX_DEDUP_ENTRIES, DEFAULT_STORM_SUPPRESSION_WINDOW};
use peercast_core::index_txt::IndexParserLimits;
use peercast_format::safestart::DEFAULT_SAFE_START_TIMEOUT;
use peercast_http::{MAX_INGEST_CHUNK_SIZE, MAX_INGEST_HEADER_SIZE};
use peercast_pcp::probe::ProbeLimits;
use peercast_pcp::relay_client::MAX_CANDIDATE_HOSTS;
use peercast_pcp::yellow_page::DEFAULT_YP_UPDATE_INTERVAL;
use serde::{Deserialize, Serialize};

fn default_container_type() -> String {
    "RAW".to_string()
}

fn default_source() -> ChannelSource {
    ChannelSource::default()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RuntimeConfig {
    pub channels: Vec<ChannelConfig>,
    pub yellow_pages: Vec<YellowPageSettings>,
    pub reconnect: ReconnectSettings,
    pub streaming: StreamingSettings,
    pub gossip: GossipSettings,
    pub probe: ProbeSettings,
    pub ingest: IngestSettings,
    pub directory: DirectorySettings,
    pub relay: RelaySettings,
    pub port_mapping: PortMappingSettings,
}

impl RuntimeConfig {
    pub fn validate(&self) -> Result<(), String> {
        let positive = [
            (
                "reconnect.maxAttemptsPerCycle",
                self.reconnect.max_attempts_per_cycle as u64,
            ),
            ("reconnect.baseBackoffMs", self.reconnect.base_backoff_ms),
            ("reconnect.maxBackoffMs", self.reconnect.max_backoff_ms),
            (
                "reconnect.connectTimeoutMs",
                self.reconnect.connect_timeout_ms,
            ),
            ("reconnect.readTimeoutMs", self.reconnect.read_timeout_ms),
            (
                "streaming.safeStartTimeoutMs",
                self.streaming.safe_start_timeout_ms,
            ),
            (
                "streaming.subscriberQueuePackets",
                self.streaming.subscriber_queue_packets as u64,
            ),
            (
                "streaming.maxHeaderBytes",
                self.streaming.max_header_bytes as u64,
            ),
            ("gossip.maxEntries", self.gossip.max_entries as u64),
            ("gossip.windowSecs", self.gossip.window_secs),
            (
                "probe.maxConcurrentGlobal",
                self.probe.max_concurrent_global as u64,
            ),
            ("probe.connectTimeoutMs", self.probe.connect_timeout_ms),
            ("probe.exchangeTimeoutMs", self.probe.exchange_timeout_ms),
            ("ingest.maxHeaderBytes", self.ingest.max_header_bytes as u64),
            ("ingest.maxChunkBytes", self.ingest.max_chunk_bytes as u64),
            (
                "directory.maxTotalBytes",
                self.directory.max_total_bytes as u64,
            ),
            ("directory.maxLines", self.directory.max_lines as u64),
            ("directory.maxFieldLen", self.directory.max_field_len as u64),
            (
                "directory.requestTimeoutMs",
                self.directory.request_timeout_ms,
            ),
            ("directory.cacheTtlSecs", self.directory.cache_ttl_secs),
            (
                "relay.maxCandidateHosts",
                self.relay.max_candidate_hosts as u64,
            ),
        ];
        for (name, value) in positive {
            if value == 0 {
                return Err(format!("{name} must be greater than zero"));
            }
        }
        if self.reconnect.base_backoff_ms > self.reconnect.max_backoff_ms {
            return Err("reconnect.baseBackoffMs must be <= reconnect.maxBackoffMs".to_string());
        }
        let mapping = &self.port_mapping;
        if mapping.discovery_timeout_ms == 0
            || mapping.request_timeout_ms == 0
            || mapping.retry_base_secs == 0
            || mapping.max_mapping_entries == 0
            || mapping.lease_duration_secs <= mapping.renewal_margin_secs
            || mapping.renewal_margin_secs == 0
            || mapping.retry_base_secs > mapping.retry_max_secs
        {
            return Err("portMapping requires positive deadlines, a finite lease longer than renewalMarginSecs, and retryBaseSecs <= retryMaxSecs".into());
        }

        let mut channel_ids = HashSet::new();
        for (index, channel) in self.channels.iter().enumerate() {
            validate_id(
                &channel.channel_id,
                &format!("channels[{index}].channelId"),
                &mut channel_ids,
            )?;
            if let Some(broadcast_id) = &channel.broadcast_id {
                validate_id(
                    broadcast_id,
                    &format!("channels[{index}].broadcastId"),
                    &mut HashSet::new(),
                )?;
            }
            if let ChannelSource::Relay { endpoint } = &channel.source {
                validate_endpoint(endpoint, &format!("channels[{index}].source.endpoint"))?;
            }
        }
        for (index, yp) in self.yellow_pages.iter().enumerate() {
            if yp.update_interval_secs == 0
                || yp.retry_base_ms == 0
                || yp.retry_max_ms == 0
                || yp.io_timeout_ms == 0
            {
                return Err(format!(
                    "yellowPages[{index}] time values must be greater than zero"
                ));
            }
            if yp.retry_base_ms > yp.retry_max_ms {
                return Err(format!(
                    "yellowPages[{index}].retryBaseMs must be <= retryMaxMs"
                ));
            }
            validate_endpoint(&yp.endpoint, &format!("yellowPages[{index}].endpoint"))?;
            if let Some(uri) = &yp.channels_uri {
                let url = reqwest::Url::parse(uri)
                    .map_err(|_| format!("yellowPages[{index}].channelsUri is invalid"))?;
                if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                    return Err(format!(
                        "yellowPages[{index}].channelsUri must be HTTP or HTTPS"
                    ));
                }
            }
        }
        Ok(())
    }
}

fn validate_endpoint(endpoint: &SocketAddr, field: &str) -> Result<(), String> {
    if endpoint.port() == 0 {
        return Err(format!("{field} port must be non-zero"));
    }
    Ok(())
}

fn validate_id(id: &str, field: &str, seen: &mut HashSet<String>) -> Result<(), String> {
    let parsed = peercast_core::ChannelId::from_hex(id)
        .map_err(|_| format!("{field} must be exactly 32 hexadecimal characters"))?;
    if parsed.as_bytes().iter().all(|byte| *byte == 0) {
        return Err(format!("{field} must not be all zero"));
    }
    let canonical = parsed.to_hex_lower();
    if !seen.insert(canonical) {
        return Err(format!("duplicate channel ID in {field}"));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelConfig {
    pub channel_id: String,
    pub name: String,
    pub container_type: String,
    pub broadcast_id: Option<String>,
    pub metadata: ChannelMetadata,
    pub source: ChannelSource,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            channel_id: String::new(),
            name: String::new(),
            container_type: default_container_type(),
            broadcast_id: None,
            metadata: ChannelMetadata::default(),
            source: default_source(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ChannelMetadata {
    pub genre: String,
    pub description: String,
    pub comment: String,
    pub contact_url: String,
    pub bitrate: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ChannelSource {
    Relay {
        endpoint: SocketAddr,
    },
    HttpPush {
        bind_address: SocketAddr,
        auth_token: Option<String>,
    },
}

impl Default for ChannelSource {
    fn default() -> Self {
        Self::Relay {
            endpoint: "127.0.0.1:7144"
                .parse()
                .expect("valid default socket address"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct YellowPageSettings {
    pub name: String,
    pub channels_uri: Option<String>,
    pub endpoint: SocketAddr,
    pub update_interval_secs: u64,
    pub retry_base_ms: u64,
    pub retry_max_ms: u64,
    pub io_timeout_ms: u64,
    pub allow_public_announcements: bool,
    /// Disable PCP announcements for a listing-only directory.
    pub announcement_enabled: bool,
}

impl Default for YellowPageSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            channels_uri: None,
            endpoint: "127.0.0.1:7144"
                .parse()
                .expect("valid default socket address"),
            update_interval_secs: DEFAULT_YP_UPDATE_INTERVAL.as_secs(),
            retry_base_ms: 1000,
            retry_max_ms: 30000,
            io_timeout_ms: 10000,
            allow_public_announcements: false,
            announcement_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ReconnectSettings {
    pub max_attempts_per_cycle: usize,
    pub base_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
}

impl Default for ReconnectSettings {
    fn default() -> Self {
        Self {
            max_attempts_per_cycle: 5,
            base_backoff_ms: 1000,
            max_backoff_ms: 30000,
            connect_timeout_ms: 5000,
            read_timeout_ms: 10000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct StreamingSettings {
    pub safe_start_timeout_ms: u64,
    pub subscriber_queue_packets: usize,
    pub max_header_bytes: usize,
}

impl Default for StreamingSettings {
    fn default() -> Self {
        Self {
            safe_start_timeout_ms: DEFAULT_SAFE_START_TIMEOUT.as_millis() as u64,
            subscriber_queue_packets: 128,
            max_header_bytes: 128 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct GossipSettings {
    pub max_entries: usize,
    pub window_secs: u64,
}

impl Default for GossipSettings {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_MAX_DEDUP_ENTRIES,
            window_secs: DEFAULT_STORM_SUPPRESSION_WINDOW.as_secs(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct ProbeSettings {
    pub max_concurrent_global: usize,
    pub connect_timeout_ms: u64,
    pub exchange_timeout_ms: u64,
}

impl Default for ProbeSettings {
    fn default() -> Self {
        let limits = ProbeLimits::default();
        Self {
            max_concurrent_global: limits.max_concurrent_global,
            connect_timeout_ms: limits.connect_timeout.as_millis() as u64,
            exchange_timeout_ms: limits.exchange_timeout.as_millis() as u64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct IngestSettings {
    pub max_header_bytes: usize,
    pub max_chunk_bytes: usize,
}

impl Default for IngestSettings {
    fn default() -> Self {
        Self {
            max_header_bytes: MAX_INGEST_HEADER_SIZE,
            max_chunk_bytes: MAX_INGEST_CHUNK_SIZE,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct DirectorySettings {
    pub max_total_bytes: usize,
    pub max_lines: usize,
    pub max_field_len: usize,
    pub request_timeout_ms: u64,
    pub cache_ttl_secs: u64,
}

impl Default for DirectorySettings {
    fn default() -> Self {
        let limits = IndexParserLimits::default();
        Self {
            max_total_bytes: limits.max_total_bytes,
            max_lines: limits.max_lines,
            max_field_len: limits.max_field_len,
            request_timeout_ms: 5000,
            cache_ttl_secs: 180,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct RelaySettings {
    pub max_candidate_hosts: usize,
}

impl Default for RelaySettings {
    fn default() -> Self {
        Self {
            max_candidate_hosts: MAX_CANDIDATE_HOSTS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct PortMappingSettings {
    pub enabled: bool,
    pub discovery_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub lease_duration_secs: u32,
    pub renewal_margin_secs: u32,
    pub retry_base_secs: u64,
    pub retry_max_secs: u64,
    pub max_mapping_entries: u32,
}

impl Default for PortMappingSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            discovery_timeout_ms: 3000,
            request_timeout_ms: 3000,
            lease_duration_secs: 1800,
            renewal_margin_secs: 300,
            retry_base_secs: 5,
            retry_max_secs: 300,
            max_mapping_entries: 256,
        }
    }
}
