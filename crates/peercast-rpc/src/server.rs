use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::protocol::{RpcErrorObject, RpcId, RpcRequest, RpcResponse};
use peercast_core::channel_id::ChannelId;

/// Maximum allowed JSON-RPC HTTP request body size (REQ-064, Station parity 64 KiB).
pub const DEFAULT_MAX_RPC_REQUEST_BYTES: usize = 64 * 1024;

// Station RPC compatibility identifiers are independent of the Rust package version.
const DEFAULT_AGENT_NAME: &str = concat!("PeerCastStation/", env!("CARGO_PKG_VERSION"));
const API_VERSION: &str = "1.0.0";

/// Provides cached or refreshed Yellow Page directory channels to RPC callers.
pub trait YellowPageDirectory: std::fmt::Debug + Send + Sync {
    fn channels(
        &self,
        refresh: bool,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + '_>>;
}

/// Client access level for authorization enforcement (Station `OutputStreamType` parity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessLevel {
    /// Playback / public access: allowed for viewers and embedded players (REQ-065).
    Playback,
    /// Administrative access: allowed for management console and configuration mutations.
    Admin,
}

/// Access level required by an RPC method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodAccessRequired {
    PlaybackOrAdmin,
    AdminOnly,
}

/// Servent configuration settings managed via JSON-RPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServentSettings {
    pub max_relays: u32,
    pub max_relays_per_channel: u32,
    pub max_directs: u32,
    pub max_directs_per_channel: u32,
    pub max_upstream_rate: u32,
    pub max_upstream_rate_ipv6: u32,
}

impl Default for ServentSettings {
    fn default() -> Self {
        Self {
            max_relays: 16,
            max_relays_per_channel: 4,
            max_directs: 8,
            max_directs_per_channel: 2,
            max_upstream_rate: 10000,
            max_upstream_rate_ipv6: 10000,
        }
    }
}

/// Channel summary exposed via `getChannels`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSummary {
    pub channel_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub bitrate: u32,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub total_relays: u32,
    #[serde(default)]
    pub total_directs: u32,
}

impl ChannelSummary {
    pub fn new(
        channel_id: ChannelId,
        name: impl Into<String>,
        status: impl Into<String>,
        bitrate: u32,
        mime_type: impl Into<String>,
        total_relays: u32,
        total_directs: u32,
    ) -> Self {
        Self {
            channel_id: channel_id.to_hex_upper(),
            name: name.into(),
            status: status.into(),
            bitrate,
            mime_type: mime_type.into(),
            total_relays,
            total_directs,
        }
    }
}

/// Status snapshot exposed via `getStatus`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusSnapshot {
    pub uptime: u64,
    pub is_firewalled: Option<bool>,
    pub global_relay_end_point: Option<(String, u16)>,
    pub global_direct_end_point: Option<(String, u16)>,
    pub local_relay_end_point: Option<(String, u16)>,
    pub local_direct_end_point: Option<(String, u16)>,
}

impl Default for StatusSnapshot {
    fn default() -> Self {
        Self {
            uptime: 0,
            is_firewalled: None,
            global_relay_end_point: None,
            global_direct_end_point: None,
            local_relay_end_point: Some(("127.0.0.1".to_string(), 7144)),
            local_direct_end_point: Some(("127.0.0.1".to_string(), 7144)),
        }
    }
}

/// Plugin assembly version metadata (matches Station APIHost.cs:192).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PluginAssemblyInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub copyright: String,
}

/// Plugin description exposed via `getPlugins`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginSummary {
    pub name: String,
    #[serde(default)]
    pub is_usable: bool,
    #[serde(default)]
    pub assembly: PluginAssemblyInfo,
}

/// Core RPC state container accessed by the dispatcher.
#[derive(Debug, Clone)]
pub struct RpcState {
    pub agent_name: String,
    pub api_version: String,
    pub auth_token: Option<String>,
    pub settings: ServentSettings,
    pub status: StatusSnapshot,
    pub channels: Vec<ChannelSummary>,
    pub plugins: Vec<PluginSummary>,
    pub user_config: HashMap<String, HashMap<String, Value>>,
}

impl Default for RpcState {
    fn default() -> Self {
        Self {
            agent_name: DEFAULT_AGENT_NAME.to_string(),
            api_version: API_VERSION.to_string(),
            auth_token: None,
            settings: ServentSettings::default(),
            status: StatusSnapshot::default(),
            channels: Vec::new(),
            plugins: vec![PluginSummary {
                name: "PeerCastCore".to_string(),
                is_usable: true,
                assembly: PluginAssemblyInfo {
                    name: "peercast-core".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    copyright: "GPL-3.0-or-later".to_string(),
                },
            }],
            user_config: HashMap::new(),
        }
    }
}

/// JSON-RPC 2.0 Dispatcher enforcing access control, schemas, and error conventions (REQ-064, REQ-065).
#[derive(Debug, Clone)]
pub struct RpcDispatcher {
    state: Arc<RwLock<RpcState>>,
    max_request_bytes: usize,
    yellow_page_directory: Option<Arc<dyn YellowPageDirectory>>,
    logging: Option<Arc<dyn LogProvider>>,
}

/// Application-owned logging backend; authorization remains in the dispatcher.
pub trait LogProvider: std::fmt::Debug + Send + Sync {
    fn settings(&self) -> Value;
    fn read(&self, from: usize, max_lines: usize) -> Value;
    fn set_level(&self, level: u64) -> Result<(), String>;
    fn clear(&self);
}

impl Default for RpcDispatcher {
    fn default() -> Self {
        Self::new(RpcState::default())
    }
}

impl RpcDispatcher {
    pub fn new(state: RpcState) -> Self {
        Self {
            state: Arc::new(RwLock::new(state)),
            max_request_bytes: DEFAULT_MAX_RPC_REQUEST_BYTES,
            yellow_page_directory: None,
            logging: None,
        }
    }

    pub fn with_max_request_bytes(mut self, max: usize) -> Self {
        self.max_request_bytes = max;
        self
    }

    pub fn with_yellow_page_directory(mut self, provider: Arc<dyn YellowPageDirectory>) -> Self {
        self.yellow_page_directory = Some(provider);
        self
    }

    pub fn with_logging(mut self, provider: Arc<dyn LogProvider>) -> Self {
        self.logging = Some(provider);
        self
    }

    pub async fn version_info(&self) -> Value {
        let st = self.state.read().await;
        json!({
            "agentName": st.agent_name,
            "apiVersion": st.api_version,
            "jsonrpc": "2.0"
        })
    }

    pub fn state(&self) -> &Arc<RwLock<RpcState>> {
        &self.state
    }

    /// Processes an incoming raw JSON-RPC HTTP request string (REQ-064).
    ///
    /// Returns:
    /// - `Some(response_json)`: Serialized JSON-RPC response object or batch array.
    /// - `None`: If the request consisted solely of notifications (HTTP 204 No Content).
    pub async fn handle_request_str(&self, raw_body: &str, access: AccessLevel) -> Option<String> {
        // Enforce request size limit
        if raw_body.len() > self.max_request_bytes {
            let resp = RpcResponse::error(
                None,
                RpcErrorObject::invalid_request(format!(
                    "Request body size {} exceeds limit {}",
                    raw_body.len(),
                    self.max_request_bytes
                )),
            );
            return serde_json::to_string(&resp).ok();
        }

        // Parse JSON
        let parsed_json: Value = match serde_json::from_str(raw_body) {
            Ok(v) => v,
            Err(e) => {
                let resp = RpcResponse::error(
                    None,
                    RpcErrorObject::parse_error(format!("Invalid JSON: {}", e)),
                );
                return serde_json::to_string(&resp).ok();
            }
        };

        match parsed_json {
            // Batch request
            Value::Array(arr) => {
                if arr.is_empty() {
                    let resp = RpcResponse::error(
                        None,
                        RpcErrorObject::invalid_request("Batch request array cannot be empty"),
                    );
                    return serde_json::to_string(&resp).ok();
                }

                let mut responses = Vec::new();
                for item in arr {
                    if let Some(resp) = self.process_single_value(item, access).await {
                        responses.push(resp);
                    }
                }

                if responses.is_empty() {
                    // All items were notifications
                    None
                } else {
                    serde_json::to_string(&responses).ok()
                }
            }

            // Single request
            Value::Object(_) => {
                let resp = self.process_single_value(parsed_json, access).await;
                resp.and_then(|r| serde_json::to_string(&r).ok())
            }

            // Other primitives are invalid JSON-RPC requests
            _ => {
                let resp = RpcResponse::error(
                    None,
                    RpcErrorObject::invalid_request(
                        "Top-level JSON value must be an object or array",
                    ),
                );
                serde_json::to_string(&resp).ok()
            }
        }
    }

    async fn process_single_value(&self, val: Value, access: AccessLevel) -> Option<RpcResponse> {
        let req: RpcRequest = match serde_json::from_value(val) {
            Ok(r) => r,
            Err(e) => {
                return Some(RpcResponse::error(
                    None,
                    RpcErrorObject::invalid_request(format!("Invalid Request structure: {}", e)),
                ));
            }
        };

        if req.jsonrpc != "2.0" {
            return Some(RpcResponse::error(
                req.id,
                RpcErrorObject::invalid_request("jsonrpc must be exactly '2.0'"),
            ));
        }

        let is_notification = req.is_notification();
        let resp = self
            .dispatch_method(&req.method, req.params, req.id.clone(), access)
            .await;

        if is_notification {
            None
        } else {
            Some(resp)
        }
    }

    async fn dispatch_method(
        &self,
        method: &str,
        params: Option<Value>,
        id: Option<RpcId>,
        access: AccessLevel,
    ) -> RpcResponse {
        let access_required = Self::method_access_requirement(method);

        // Enforce access control
        if access_required == MethodAccessRequired::AdminOnly && access != AccessLevel::Admin {
            return RpcResponse::error(
                id,
                RpcErrorObject::unauthorized(format!(
                    "Method '{}' requires administrative authorization",
                    method
                )),
            );
        }

        match method {
            // --- Core Supported Methods (REQ-064) ---
            "getVersionInfo" => RpcResponse::success(id, self.version_info().await),

            "getStatus" => {
                let st = self.state.read().await;
                RpcResponse::success(
                    id,
                    json!({
                        "uptime": st.status.uptime,
                        "isFirewalled": st.status.is_firewalled,
                        "globalRelayEndPoint": st.status.global_relay_end_point.as_ref().map(|(ip, p)| json!([ip, p])),
                        "globalDirectEndPoint": st.status.global_direct_end_point.as_ref().map(|(ip, p)| json!([ip, p])),
                        "localRelayEndPoint": st.status.local_relay_end_point.as_ref().map(|(ip, p)| json!([ip, p])),
                        "localDirectEndPoint": st.status.local_direct_end_point.as_ref().map(|(ip, p)| json!([ip, p])),
                    }),
                )
            }

            "getChannels" => {
                let st = self.state.read().await;
                RpcResponse::success(id, json!(st.channels))
            }

            "getSettings" => {
                let st = self.state.read().await;
                RpcResponse::success(id, json!(st.settings))
            }

            "setSettings" => {
                let mut st = self.state.write().await;
                let params_val = match params {
                    Some(Value::Object(map)) => Value::Object(map),
                    Some(Value::Array(mut arr)) if !arr.is_empty() => arr.remove(0),
                    _ => {
                        return RpcResponse::error(
                            id,
                            RpcErrorObject::invalid_params(
                                "setSettings requires a settings object parameter",
                            ),
                        );
                    }
                };

                // Apply update fields if present
                if let Some(val) = params_val.get("maxRelays").and_then(|v| v.as_u64()) {
                    st.settings.max_relays = val as u32;
                }
                if let Some(val) = params_val
                    .get("maxRelaysPerChannel")
                    .and_then(|v| v.as_u64())
                {
                    st.settings.max_relays_per_channel = val as u32;
                }
                if let Some(val) = params_val.get("maxDirects").and_then(|v| v.as_u64()) {
                    st.settings.max_directs = val as u32;
                }
                if let Some(val) = params_val
                    .get("maxDirectsPerChannel")
                    .and_then(|v| v.as_u64())
                {
                    st.settings.max_directs_per_channel = val as u32;
                }
                if let Some(val) = params_val.get("maxUpstreamRate").and_then(|v| v.as_u64()) {
                    st.settings.max_upstream_rate = val as u32;
                }
                if let Some(val) = params_val
                    .get("maxUpstreamRateIPv6")
                    .and_then(|v| v.as_u64())
                {
                    st.settings.max_upstream_rate_ipv6 = val as u32;
                }

                RpcResponse::success(id, json!(true))
            }

            "getAuthToken" => {
                let st = self.state.read().await;
                RpcResponse::success(id, json!(st.auth_token))
            }

            "getPlugins" => {
                let st = self.state.read().await;
                RpcResponse::success(id, json!(st.plugins))
            }

            // --- Explicitly Deferred Methods (REQ-064: NO success stubs!) ---
            "broadcastChannel" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("broadcastChannel", "broadcast_ingest"),
            ),
            "checkBandwidth" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("checkBandwidth", "diagnostics"),
            ),
            "checkPorts" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("checkPorts", "diagnostics"),
            ),
            "checkUpdate" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("checkUpdate", "updater"),
            ),
            "updateAndRestart" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("updateAndRestart", "updater"),
            ),
            "getUpdateStatus" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getUpdateStatus", "updater"),
            ),
            "getNewVersions" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getNewVersions", "updater"),
            ),
            "getExternalIPAddresses" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getExternalIPAddresses", "network_discovery"),
            ),
            "getLogSettings" | "getLog" | "setLogSettings" | "clearLog" => {
                let Some(logging) = &self.logging else {
                    return RpcResponse::error(
                        id,
                        RpcErrorObject::unsupported_method(method, "logging"),
                    );
                };
                match method {
                    "getLogSettings" => RpcResponse::success(id, logging.settings()),
                    "clearLog" => {
                        logging.clear();
                        RpcResponse::success(id, Value::Null)
                    }
                    "setLogSettings" => {
                        let settings = match params.as_ref() {
                            Some(Value::Object(map)) => map.get("settings").or(params.as_ref()),
                            Some(Value::Array(items)) => items.first(),
                            _ => None,
                        };
                        let Some(level) = settings
                            .and_then(|s| s.get("level"))
                            .and_then(Value::as_u64)
                        else {
                            return RpcResponse::error(
                                id,
                                RpcErrorObject::invalid_params(
                                    "settings.level must be an integer from 0 to 5",
                                ),
                            );
                        };
                        match logging.set_level(level) {
                            Ok(()) => RpcResponse::success(id, Value::Null),
                            Err(error) => {
                                RpcResponse::error(id, RpcErrorObject::invalid_params(error))
                            }
                        }
                    }
                    _ => {
                        let number =
                            |name: &str, index: usize, default: usize| -> Result<usize, String> {
                                let value = match params.as_ref() {
                                    None | Some(Value::Null) => None,
                                    Some(Value::Object(map)) => map.get(name),
                                    Some(Value::Array(items)) => items.get(index),
                                    _ => return Err("getLog requires an object or array".into()),
                                };
                                match value {
                                    None | Some(Value::Null) => Ok(default),
                                    Some(value) => value
                                        .as_u64()
                                        .and_then(|n| usize::try_from(n).ok())
                                        .ok_or_else(|| {
                                            format!("{name} must be a non-negative integer")
                                        }),
                                }
                            };
                        match (number("from", 0, 0), number("maxLines", 1, 100)) {
                            (Ok(from), Ok(max_lines)) => {
                                RpcResponse::success(id, logging.read(from, max_lines.min(1000)))
                            }
                            (Err(error), _) | (_, Err(error)) => {
                                RpcResponse::error(id, RpcErrorObject::invalid_params(error))
                            }
                        }
                    }
                }
            }
            "getChannelStatus" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getChannelStatus", "channel_detail"),
            ),
            "getChannelInfo" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getChannelInfo", "channel_detail"),
            ),
            "setChannelInfo" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("setChannelInfo", "channel_detail"),
            ),
            "stopChannel" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("stopChannel", "channel_lifecycle"),
            ),
            "bumpChannel" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("bumpChannel", "channel_lifecycle"),
            ),
            "getChannelConnections" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getChannelConnections", "channel_detail"),
            ),
            "stopChannelConnection" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("stopChannelConnection", "channel_detail"),
            ),
            "restartChannelConnection" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("restartChannelConnection", "channel_detail"),
            ),
            "getChannelRelayTree" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getChannelRelayTree", "relay_tree"),
            ),
            "getContentReaders" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getContentReaders", "format_registry"),
            ),
            "getSourceStreams" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getSourceStreams", "source_registry"),
            ),
            "getYellowPageProtocols" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getYellowPageProtocols", "yp_client"),
            ),
            "getYPChannels" | "updateYPChannels" => {
                let refresh = method == "updateYPChannels";
                match &self.yellow_page_directory {
                    Some(provider) => match provider.channels(refresh).await {
                        Ok(channels) => RpcResponse::success(id, channels),
                        Err(error) => RpcResponse::error(id, RpcErrorObject::internal_error(error)),
                    },
                    None => RpcResponse::error(
                        id,
                        RpcErrorObject::unsupported_method(method, "yp_client"),
                    ),
                }
            }
            "getYellowPages" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getYellowPages", "yp_client"),
            ),
            "addYellowPage" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("addYellowPage", "yp_client"),
            ),
            "removeYellowPage" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("removeYellowPage", "yp_client"),
            ),
            "stopAnnounce" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("stopAnnounce", "yp_client"),
            ),
            "restartAnnounce" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("restartAnnounce", "yp_client"),
            ),
            "getListeners" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getListeners", "listener_manager"),
            ),
            "addListener" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("addListener", "listener_manager"),
            ),
            "resetListenerAuthenticationKey" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method(
                    "resetListenerAuthenticationKey",
                    "listener_manager",
                ),
            ),
            "removeListener" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("removeListener", "listener_manager"),
            ),
            "setListenerAccepts" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("setListenerAccepts", "listener_manager"),
            ),
            "setListenerAuthorizationRequired" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method(
                    "setListenerAuthorizationRequired",
                    "listener_manager",
                ),
            ),
            "getBroadcastHistory" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getBroadcastHistory", "broadcast_history"),
            ),
            "addBroadcastHistory" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("addBroadcastHistory", "broadcast_history"),
            ),
            "getNotificationMessages" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getNotificationMessages", "notifications"),
            ),
            "setUserConfig" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("setUserConfig", "user_config"),
            ),
            "getUserConfig" => RpcResponse::error(
                id,
                RpcErrorObject::unsupported_method("getUserConfig", "user_config"),
            ),

            // --- Unknown Method ---
            other => RpcResponse::error(id, RpcErrorObject::method_not_found(other)),
        }
    }

    /// Maps method names to required access category (Station `APIHost.cs` parity).
    fn method_access_requirement(method: &str) -> MethodAccessRequired {
        match method {
            "getVersionInfo" | "getStatus" | "getChannels" | "getAuthToken"
            | "getChannelStatus" | "getChannelInfo" => MethodAccessRequired::PlaybackOrAdmin,
            _ => MethodAccessRequired::AdminOnly,
        }
    }
}
