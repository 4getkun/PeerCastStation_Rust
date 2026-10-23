use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::protocol::{rpc_code, RpcErrorObject};
use crate::server::{ChannelSummary, PluginSummary, ServentSettings, StatusSnapshot};

/// Errors produced by the typed RPC client / adapter (REQ-064, REQ-081, REQ-084).
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ClientRpcError {
    #[error("Transport network error: {0}")]
    Transport(String),

    #[error("JSON-RPC error [{code}]: {message}")]
    Rpc {
        code: i32,
        message: String,
        data: Option<Value>,
    },

    #[error("Unauthorized: method requires administrative authorization")]
    Unauthorized,

    #[error("Method '{method}' is unsupported/deferred in current milestone (capability: {capability})")]
    UnsupportedMethod { method: String, capability: String },

    #[error("Failed to deserialize response payload: {0}")]
    Deserialization(String),

    #[error("Operation was cancelled before completion")]
    Cancelled,
}

impl From<RpcErrorObject> for ClientRpcError {
    fn from(err: RpcErrorObject) -> Self {
        if err.code == rpc_code::UNAUTHORIZED {
            ClientRpcError::Unauthorized
        } else if err.code == rpc_code::UNSUPPORTED_METHOD {
            let cap = err
                .data
                .as_ref()
                .and_then(|d| d.get("capability"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let method = err
                .data
                .as_ref()
                .and_then(|d| d.get("method"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            ClientRpcError::UnsupportedMethod {
                method,
                capability: cap,
            }
        } else {
            ClientRpcError::Rpc {
                code: err.code,
                message: err.message,
                data: err.data,
            }
        }
    }
}

/// Abstract transport boundary for JSON-RPC communication (REQ-081).
pub trait RpcTransport: Send + Sync {
    fn send_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> impl std::future::Future<Output = Result<Value, ClientRpcError>> + Send;
}

/// Type alias for mock call logs (method, params).
pub type RpcCallLog = Vec<(String, Option<Value>)>;

/// Type alias for mock canned response map.
pub type MockResponseMap = HashMap<String, Result<Value, ClientRpcError>>;

/// Mock transport for offline testing of controllers without a running daemon (REQ-081, REQ-083).
#[derive(Debug, Default, Clone)]
pub struct MockRpcTransport {
    responses: Arc<Mutex<MockResponseMap>>,
    call_log: Arc<Mutex<RpcCallLog>>,
}

impl MockRpcTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-registers a successful response for a method.
    pub async fn register_success(&self, method: &str, result: Value) {
        let mut map = self.responses.lock().await;
        map.insert(method.to_string(), Ok(result));
    }

    /// Pre-registers an RPC protocol error response for a method.
    pub async fn register_error(&self, method: &str, error: RpcErrorObject) {
        let mut map = self.responses.lock().await;
        map.insert(method.to_string(), Err(ClientRpcError::from(error)));
    }

    /// Pre-registers a client error (e.g. transport, unauthorized) for a method.
    pub async fn register_client_error(&self, method: &str, error: ClientRpcError) {
        let mut map = self.responses.lock().await;
        map.insert(method.to_string(), Err(error));
    }

    /// Returns recorded list of called methods and parameters.
    pub async fn calls(&self) -> Vec<(String, Option<Value>)> {
        self.call_log.lock().await.clone()
    }
}

impl RpcTransport for MockRpcTransport {
    async fn send_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, ClientRpcError> {
        self.call_log
            .lock()
            .await
            .push((method.to_string(), params.clone()));

        let responses = self.responses.lock().await;
        if let Some(res) = responses.get(method) {
            match res {
                Ok(val) => Ok(val.clone()),
                Err(err) => Err(err.clone()),
            }
        } else {
            Err(ClientRpcError::Rpc {
                code: rpc_code::METHOD_NOT_FOUND,
                message: format!("Mock has no registered handler for method '{}'", method),
                data: None,
            })
        }
    }
}

/// View model representing channel details in the presentation layer (REQ-081).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelViewModel {
    pub channel_id: String,
    pub name: String,
    pub status: String,
    pub display_bitrate: String,
    pub raw_bitrate: u32,
    pub mime_type: String,
    pub total_relays: u32,
    pub total_directs: u32,
}

impl From<ChannelSummary> for ChannelViewModel {
    fn from(s: ChannelSummary) -> Self {
        Self {
            channel_id: s.channel_id,
            name: s.name,
            status: s.status,
            display_bitrate: format!("{} kbps", s.bitrate),
            raw_bitrate: s.bitrate,
            mime_type: s.mime_type,
            total_relays: s.total_relays,
            total_directs: s.total_directs,
        }
    }
}

/// View model representing version and identity info (REQ-081).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfoViewModel {
    pub agent_name: String,
    pub api_version: String,
    pub jsonrpc_version: String,
}

/// View model representing overall server status (REQ-081).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusViewModel {
    pub uptime_formatted: String,
    pub uptime_seconds: u64,
    pub port_status_label: String,
    pub is_firewalled: Option<bool>,
    pub local_endpoint: Option<String>,
    pub global_endpoint: Option<String>,
}

impl From<StatusSnapshot> for StatusViewModel {
    fn from(s: StatusSnapshot) -> Self {
        let hrs = s.uptime / 3600;
        let mins = (s.uptime % 3600) / 60;
        let secs = s.uptime % 60;
        let uptime_formatted = format!("{:02}:{:02}:{:02}", hrs, mins, secs);

        let port_status_label = match s.is_firewalled {
            Some(false) => "ポート開放中 (Open)".to_string(),
            Some(true) => "ポート未開放 (Firewalled)".to_string(),
            None => "ポート状態確認中 (Unknown)".to_string(),
        };

        let local_endpoint = s
            .local_relay_end_point
            .as_ref()
            .map(|(ip, port)| format!("{}:{}", ip, port));

        let global_endpoint = s
            .global_relay_end_point
            .as_ref()
            .map(|(ip, port)| format!("{}:{}", ip, port));

        Self {
            uptime_formatted,
            uptime_seconds: s.uptime,
            port_status_label,
            is_firewalled: s.is_firewalled,
            local_endpoint,
            global_endpoint,
        }
    }
}

/// Typed API client adapter mediating between presentation controllers and RPC transport (REQ-081, REQ-084).
///
/// Prevents components from constructing RPC method strings manually and enforces mutation retry policies.
#[derive(Debug, Clone)]
pub struct PeerCastRpcClient<T: RpcTransport> {
    transport: T,
    id_counter: Arc<AtomicU64>,
}

impl<T: RpcTransport> PeerCastRpcClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            id_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Allocates an atomically incrementing JSON-RPC request ID (REQ-081).
    pub fn next_request_id(&self) -> u64 {
        self.id_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Queries core servent version info (REQ-064).
    pub async fn get_version_info(&self) -> Result<VersionInfoViewModel, ClientRpcError> {
        let val = self.transport.send_request("getVersionInfo", None).await?;
        let agent = val
            .get("agentName")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        let api_ver = val
            .get("apiVersion")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        let jsonrpc = val
            .get("jsonrpc")
            .and_then(|v| v.as_str())
            .unwrap_or("2.0")
            .to_string();

        Ok(VersionInfoViewModel {
            agent_name: agent,
            api_version: api_ver,
            jsonrpc_version: jsonrpc,
        })
    }

    /// Queries current status snapshot and maps to presentation view model (REQ-064, REQ-081).
    pub async fn get_status(&self) -> Result<StatusViewModel, ClientRpcError> {
        let val = self.transport.send_request("getStatus", None).await?;
        let snapshot: StatusSnapshot = serde_json::from_value(val)
            .map_err(|e| ClientRpcError::Deserialization(e.to_string()))?;
        Ok(StatusViewModel::from(snapshot))
    }

    /// Queries active channels list and converts to view models (REQ-064, REQ-081).
    pub async fn get_channels(&self) -> Result<Vec<ChannelViewModel>, ClientRpcError> {
        let val = self.transport.send_request("getChannels", None).await?;
        let summaries: Vec<ChannelSummary> = serde_json::from_value(val)
            .map_err(|e| ClientRpcError::Deserialization(e.to_string()))?;
        Ok(summaries.into_iter().map(ChannelViewModel::from).collect())
    }

    /// Queries current servent settings (REQ-064).
    pub async fn get_settings(&self) -> Result<ServentSettings, ClientRpcError> {
        let val = self.transport.send_request("getSettings", None).await?;
        serde_json::from_value(val).map_err(|e| ClientRpcError::Deserialization(e.to_string()))
    }

    /// Applies configuration settings mutation (REQ-064, REQ-084).
    ///
    /// Guarded as a mutation: callers must NOT blindly retry on uncertain network disconnects.
    pub async fn set_settings(&self, settings: &ServentSettings) -> Result<bool, ClientRpcError> {
        let params = json!(settings);
        let val = self
            .transport
            .send_request("setSettings", Some(params))
            .await?;
        val.as_bool()
            .ok_or_else(|| ClientRpcError::Deserialization("setSettings result must be a boolean".into()))
    }

    /// Queries active authentication token if any.
    pub async fn get_auth_token(&self) -> Result<Option<String>, ClientRpcError> {
        let val = self.transport.send_request("getAuthToken", None).await?;
        Ok(val.as_str().map(|s| s.to_string()))
    }

    /// Queries loaded plugins list.
    pub async fn get_plugins(&self) -> Result<Vec<PluginSummary>, ClientRpcError> {
        let val = self.transport.send_request("getPlugins", None).await?;
        serde_json::from_value(val).map_err(|e| ClientRpcError::Deserialization(e.to_string()))
    }

    /// Attempts to call broadcastChannel (demonstrating explicit capability reporting for deferred methods).
    pub async fn broadcast_channel(&self, info: Value) -> Result<Value, ClientRpcError> {
        // Enforce mutation safety: no silent success stubbing!
        self.transport
            .send_request("broadcastChannel", Some(info))
            .await
    }

    /// Returns whether a given method is a mutating operation that must not be automatically retried (REQ-084).
    pub fn is_mutating_method(method: &str) -> bool {
        matches!(
            method,
            "setSettings"
                | "setChannelInfo"
                | "stopChannel"
                | "bumpChannel"
                | "stopChannelConnection"
                | "restartChannelConnection"
                | "addYellowPage"
                | "removeYellowPage"
                | "addListener"
                | "removeListener"
                | "broadcastChannel"
                | "updateAndRestart"
        )
    }
}
