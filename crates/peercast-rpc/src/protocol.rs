use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 Request ID (can be number, string, or null).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcId {
    Number(i64),
    String(String),
}

/// JSON-RPC 2.0 Error Object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcErrorObject {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(code: i32, message: impl Into<String>, data: Value) -> Self {
        Self {
            code,
            message: message.into(),
            data: Some(data),
        }
    }

    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self::new(rpc_code::PARSE_ERROR, msg)
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self::new(rpc_code::INVALID_REQUEST, msg)
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            rpc_code::METHOD_NOT_FOUND,
            format!("Method '{}' not found", method),
        )
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(rpc_code::INVALID_PARAMS, msg)
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self::new(rpc_code::INTERNAL_ERROR, msg)
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self::new(rpc_code::UNAUTHORIZED, msg)
    }

    pub fn unsupported_method(method: &str, capability: &str) -> Self {
        let data = serde_json::json!({
            "method": method,
            "status": "deferred",
            "capability": capability
        });
        Self::with_data(
            rpc_code::UNSUPPORTED_METHOD,
            format!(
                "Method '{}' is not supported in the current milestone",
                method
            ),
            data,
        )
    }
}

/// Standard and extended JSON-RPC 2.0 error codes (REQ-064).
pub mod rpc_code {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;

    /// PeerCastStation specific error codes
    pub const UNAUTHORIZED: i32 = -32001;
    pub const UNSUPPORTED_METHOD: i32 = -32004;
}

/// JSON-RPC 2.0 Request object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RpcId>,
}

impl RpcRequest {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// JSON-RPC 2.0 Response object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<RpcId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcErrorObject>,
}

impl RpcResponse {
    pub fn success(id: Option<RpcId>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<RpcId>, error: RpcErrorObject) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}
