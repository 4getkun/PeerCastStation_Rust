//! PeerCastStation JSON-RPC 2.0 management API and contract implementation (REQ-064, REQ-065).

pub mod protocol;
pub mod server;

pub use protocol::{
    rpc_code, RpcErrorObject, RpcId, RpcRequest, RpcResponse,
};
pub use server::{
    AccessLevel, ChannelSummary, MethodAccessRequired, PluginSummary, RpcDispatcher,
    RpcState, ServentSettings, StatusSnapshot, DEFAULT_MAX_RPC_REQUEST_BYTES,
};
