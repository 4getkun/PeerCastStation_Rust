use serde::Serialize;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSnapshot {
    pub id: u64,
    pub kind: String,
    pub endpoint: String,
    pub status: String,
    pub session_id: Option<String>,
    pub agent: Option<String>,
    pub port_open: Option<bool>,
    pub measurements_available: bool,
    pub received_bytes: u64,
    pub sent_bytes: u64,
    pub uptime_secs: u64,
}

struct ConnectionEntry {
    snapshot: ConnectionSnapshot,
    started: Instant,
}

#[derive(Default)]
pub struct ConnectionRegistry {
    next_id: AtomicU64,
    entries: Mutex<HashMap<u64, ConnectionEntry>>,
}

impl ConnectionRegistry {
    pub fn track(
        self: &Arc<Self>,
        kind: &str,
        endpoint: Option<SocketAddr>,
        status: &str,
    ) -> ConnectionLease {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let snapshot = ConnectionSnapshot {
            id,
            kind: kind.to_owned(),
            endpoint: endpoint.map_or_else(|| "不明".to_owned(), |value| value.to_string()),
            status: status.to_owned(),
            session_id: None,
            agent: None,
            port_open: None,
            measurements_available: true,
            received_bytes: 0,
            sent_bytes: 0,
            uptime_secs: 0,
        };
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                id,
                ConnectionEntry {
                    snapshot,
                    started: Instant::now(),
                },
            );
        ConnectionLease {
            id,
            registry: Arc::downgrade(self),
        }
    }

    pub fn snapshot(&self) -> Vec<ConnectionSnapshot> {
        let now = Instant::now();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut snapshot = entries
            .values_mut()
            .map(|entry| {
                entry.snapshot.uptime_secs = now.duration_since(entry.started).as_secs();
                entry.snapshot.clone()
            })
            .collect::<Vec<_>>();
        snapshot.sort_unstable_by_key(|entry| entry.id);
        snapshot
    }
}

pub struct ConnectionLease {
    id: u64,
    registry: Weak<ConnectionRegistry>,
}

impl ConnectionLease {
    fn update(&self, update: impl FnOnce(&mut ConnectionSnapshot)) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let mut entries = registry
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get_mut(&self.id) {
            update(&mut entry.snapshot);
        }
    }

    pub fn connected(
        &self,
        session_id: Option<String>,
        agent: Option<String>,
        port_open: Option<bool>,
    ) {
        self.update(|entry| {
            entry.session_id = session_id;
            entry.agent = agent;
            entry.port_open = port_open;
        });
    }

    pub fn received(&self, bytes: usize) {
        self.update(|entry| {
            entry.received_bytes = entry.received_bytes.saturating_add(bytes as u64);
        });
    }

    pub fn sent(&self, bytes: usize) {
        self.update(|entry| {
            entry.sent_bytes = entry.sent_bytes.saturating_add(bytes as u64);
        });
    }

    pub fn set_status(&self, status: &str) {
        let status = status.to_owned();
        self.update(|entry| entry.status = status);
    }
}

impl Drop for ConnectionLease {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.id);
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortSnapshot {
    pub relay_port: u16,
    pub ipv4: String,
    pub ipv6: String,
    pub observed_ipv4: Option<String>,
    pub observed_ipv6: Option<String>,
}

pub struct NetworkDiagnostics {
    externally_listening: bool,
    snapshot: Mutex<PortSnapshot>,
}

impl NetworkDiagnostics {
    pub fn new(listen_addr: SocketAddr) -> Self {
        let (ipv4, ipv6) = if listen_addr.port() == 0 || listen_addr.ip().is_loopback() {
            if listen_addr.is_ipv4() {
                ("closed", "unavailable")
            } else {
                ("unavailable", "closed")
            }
        } else if listen_addr.is_ipv4() {
            ("unknown", "unavailable")
        } else {
            ("unavailable", "unknown")
        };
        Self {
            externally_listening: listen_addr.port() != 0 && !listen_addr.ip().is_loopback(),
            snapshot: Mutex::new(PortSnapshot {
                relay_port: listen_addr.port(),
                ipv4: ipv4.to_owned(),
                ipv6: ipv6.to_owned(),
                observed_ipv4: None,
                observed_ipv6: None,
            }),
        }
    }

    pub fn observe(
        &self,
        observed_ip: Option<IpAddr>,
        verified_port: Option<u16>,
        family_is_v6: bool,
    ) {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(ip) = observed_ip {
            let observed = if family_is_v6 {
                &mut snapshot.observed_ipv6
            } else {
                &mut snapshot.observed_ipv4
            };
            *observed = Some(ip.to_string());
        }
        if let Some(port) = verified_port {
            let status = if family_is_v6 {
                &mut snapshot.ipv6
            } else {
                &mut snapshot.ipv4
            };
            if status != "unavailable" {
                *status = if port == 0 || !self.externally_listening {
                    "closed"
                } else {
                    "open"
                }
                .to_owned();
            }
        }
    }

    pub fn snapshot(&self) -> PortSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

pub(super) struct MeteredWriter<'a, W> {
    writer: &'a mut W,
    lease: &'a ConnectionLease,
}

impl<'a, W> MeteredWriter<'a, W> {
    pub fn new(writer: &'a mut W, lease: &'a ConnectionLease) -> Self {
        Self { writer, lease }
    }
}

impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for MeteredWriter<'_, W> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let result = std::pin::Pin::new(&mut *self.writer).poll_write(cx, buf);
        if let std::task::Poll::Ready(Ok(n)) = &result {
            self.lease.sent(*n);
        }
        result
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut *self.writer).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut *self.writer).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectionRegistry, NetworkDiagnostics};
    use std::net::SocketAddr;
    use std::sync::Arc;

    #[test]
    fn connection_lease_tracks_metadata_bytes_and_removal() {
        let registry = Arc::new(ConnectionRegistry::default());
        let lease = registry.track(
            "relay",
            Some("192.0.2.4:7144".parse().unwrap()),
            "connecting",
        );
        lease.connected(
            Some("session-1".to_owned()),
            Some("PeerCastStation".to_owned()),
            Some(true),
        );
        lease.received(12);
        lease.sent(7);
        lease.set_status("connected");

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].session_id.as_deref(), Some("session-1"));
        assert_eq!(snapshot[0].agent.as_deref(), Some("PeerCastStation"));
        assert_eq!(snapshot[0].port_open, Some(true));
        assert_eq!(snapshot[0].received_bytes, 12);
        assert_eq!(snapshot[0].sent_bytes, 7);
        assert_eq!(snapshot[0].status, "connected");
        drop(lease);
        assert!(registry.snapshot().is_empty());
    }

    #[test]
    fn network_diagnostics_distinguishes_unknown_closed_and_open() {
        let diagnostics = NetworkDiagnostics::new("0.0.0.0:7144".parse::<SocketAddr>().unwrap());
        let initial = diagnostics.snapshot();
        assert_eq!(initial.ipv4, "unknown");
        assert_eq!(initial.ipv6, "unavailable");

        diagnostics.observe(Some("198.51.100.8".parse().unwrap()), Some(0), false);
        let closed = diagnostics.snapshot();
        assert_eq!(closed.ipv4, "closed");
        assert_eq!(closed.observed_ipv4.as_deref(), Some("198.51.100.8"));

        diagnostics.observe(None, Some(7144), false);
        assert_eq!(diagnostics.snapshot().ipv4, "open");
    }

    #[test]
    fn loopback_listener_starts_closed_and_other_family_unavailable() {
        let diagnostics = NetworkDiagnostics::new("127.0.0.1:0".parse().unwrap());
        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.ipv4, "closed");
        assert_eq!(snapshot.ipv6, "unavailable");
        assert_eq!(snapshot.relay_port, 0);
        diagnostics.observe(Some("198.51.100.1".parse().unwrap()), Some(7144), false);
        assert_eq!(
            diagnostics.snapshot().ipv4,
            "closed",
            "an echoed port cannot expose a loopback listener"
        );
    }
}
