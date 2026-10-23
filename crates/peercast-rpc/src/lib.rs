//! PeerCastStation JSON-RPC 2.0 management API and contract implementation (REQ-064, REQ-065).

pub mod client;
pub mod controller;
pub mod protocol;
pub mod security;
pub mod server;

pub use client::{
    ChannelViewModel, ClientRpcError, MockRpcTransport, PeerCastRpcClient, RpcTransport,
    StatusViewModel, VersionInfoViewModel,
};
pub use controller::{
    ChannelsController, ControllerError, IndexController, IndexScreenData, MutationState,
    ScreenState, SettingsController,
};
pub use protocol::{
    rpc_code, RpcErrorObject, RpcId, RpcRequest, RpcResponse,
};
pub use security::{
    redact_secrets, validate_relay_destination, HttpRequestContext, HttpRpcResponse, ListenerKind,
    SecureRpcHandler, SecurityError, SecurityPolicy,
};
pub use server::{
    AccessLevel, ChannelSummary, MethodAccessRequired, PluginAssemblyInfo, PluginSummary,
    RpcDispatcher, RpcState, ServentSettings, StatusSnapshot, DEFAULT_MAX_RPC_REQUEST_BYTES,
};
