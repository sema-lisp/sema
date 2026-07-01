use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    // Omit `params` entirely when absent rather than sending `"params":null` —
    // some servers (e.g. Asana) reject an explicit null with `400`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    pub id: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: Option<serde_json::Value>,
    // Present only on server→client *requests*/notifications the client may
    // receive interleaved on a shared stream. Captured so the client can tell a
    // request (has `method`) from its own response and never mis-correlate a
    // colliding id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

impl JsonRpcResponse {
    /// True only for a genuine response to one of our requests: it must carry a
    /// `result` or `error` and must NOT be a server-initiated request/notification
    /// (which would carry `method`).
    pub fn is_response(&self) -> bool {
        self.method.is_none() && (self.result.is_some() || self.error.is_some())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    // MCP tool descriptions are optional on the wire; default to empty so a server
    // that omits one still decodes rather than failing the whole `tools/list`.
    #[serde(default)]
    pub description: String,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolContent {
    Text { text: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_omits_null_params() {
        // A request with no params must NOT serialize `"params":null` — some
        // servers (Asana) reject that with a 400.
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/list".to_string(),
            params: None,
            id: Some(serde_json::json!(2)),
        };
        let encoded = serde_json::to_string(&req).unwrap();
        assert!(
            !encoded.contains("params"),
            "params must be omitted when None: {encoded}"
        );
    }

    #[test]
    fn is_response_distinguishes_replies_from_server_requests() {
        let parse = |s: &str| serde_json::from_str::<JsonRpcResponse>(s).unwrap();
        // A server->client request/notification with a colliding id is NOT our response.
        assert!(!parse(r#"{"jsonrpc":"2.0","method":"ping","id":3}"#).is_response());
        assert!(!parse(r#"{"jsonrpc":"2.0","method":"notifications/progress"}"#).is_response());
        // A real result or error IS our response.
        assert!(parse(r#"{"jsonrpc":"2.0","result":{"ok":true},"id":3}"#).is_response());
        assert!(
            parse(r#"{"jsonrpc":"2.0","error":{"code":-1,"message":"x"},"id":3}"#).is_response()
        );
        // Neither result nor error → not a usable response.
        assert!(!parse(r#"{"jsonrpc":"2.0","id":3}"#).is_response());
    }

    #[test]
    fn request_keeps_present_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({ "name": "x" })),
            id: Some(serde_json::json!(1)),
        };
        let encoded = serde_json::to_string(&req).unwrap();
        assert!(encoded.contains("\"params\""), "got: {encoded}");
    }
}
