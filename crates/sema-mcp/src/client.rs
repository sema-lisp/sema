//! Minimal MCP *client* over the stdio transport.
//!
//! Sema is primarily an MCP *server* (see `server.rs`); this is the reverse
//! direction — spawn an external MCP server as a child process and speak
//! JSON-RPC to it over stdin/stdout so Sema code can consume its tools. It
//! mirrors `server.rs`'s line-delimited JSON-RPC loop, inverted.
//!
//! Scope is deliberately the stdio transport only (spike milestone M1/M2). The
//! Streamable-HTTP transport and the OAuth 2.1 login flow for authenticated
//! remote servers are separate, larger milestones (see
//! `docs/plans/2026-06-21-mcp-client-spike.md`). For stdio there is no auth
//! handshake: a server that needs a credential reads it from the environment
//! Sema hands the child, so pass tokens through the `:env` map on `mcp/connect`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::protocol::{JsonRpcRequest, JsonRpcResponse, Tool};

/// How long to wait for a single response line before giving up, so a wedged
/// server surfaces as an error instead of hanging the calling Sema thread
/// forever. Generous because a `tools/call` can legitimately be slow.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// The MCP protocol revision this client advertises during `initialize`.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Spec for launching a stdio MCP server. `env` entries are added to (not
/// replacing) the inherited environment — the natural place to pass a token a
/// server expects (e.g. `GITHUB_TOKEN`).
#[derive(Debug, Clone, Default)]
pub struct McpClientConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: Option<HashMap<String, String>>,
    pub cwd: Option<PathBuf>,
}

impl McpClientConfig {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            env: None,
            cwd: None,
        }
    }
}

/// A live connection to a stdio MCP server. Dropping it kills the child.
pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: i64,
    timeout: Duration,
}

impl McpClient {
    /// Spawn the server process and wire up its stdio. Does not perform the
    /// `initialize` handshake — call [`McpClient::initialize`] next.
    pub async fn connect(config: McpClientConfig) -> Result<Self, String> {
        let mut command = Command::new(&config.command);
        command.args(&config.args);
        if let Some(env) = config.env {
            for (key, value) in env {
                command.env(key, value);
            }
        }
        if let Some(cwd) = config.cwd {
            command.current_dir(cwd);
        }
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        // Let the server's stderr flow to ours so its diagnostics are visible.
        command.stderr(std::process::Stdio::inherit());

        let mut child = command.spawn().map_err(|err| {
            format!(
                "failed to spawn MCP server process `{}`: {err}",
                config.command
            )
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to open MCP server stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to open MCP server stdout".to_string())?;

        Ok(Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
            timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    /// Perform the MCP `initialize` handshake and send the mandatory
    /// `notifications/initialized` follow-up. Per the spec a server need not
    /// answer any other request until it has received that notification, so
    /// skipping it (as a naive client would) hangs against conformant servers.
    pub async fn initialize(&mut self) -> Result<serde_json::Value, String> {
        let result = self
            .request(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "sema-mcp-client",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
            )
            .await?;
        self.notify("notifications/initialized", None).await?;
        Ok(result)
    }

    pub async fn list_tools(&mut self) -> Result<Vec<Tool>, String> {
        let result = self.request("tools/list", None).await?;
        let tools = result
            .get("tools")
            .ok_or_else(|| "tools/list result did not include a tools array".to_string())?;
        serde_json::from_value(tools.clone())
            .map_err(|err| format!("failed to decode tools/list response: {err}"))
    }

    /// Call a tool and return the raw `tools/call` result object. The result is
    /// left as untyped JSON on purpose so every content shape a server may emit
    /// (text, image, resource links, `structuredContent`, …) passes through to
    /// Sema losslessly rather than being narrowed to a fixed enum.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.request(
            "tools/call",
            Some(serde_json::json!({ "name": name, "arguments": arguments })),
        )
        .await
    }

    pub async fn close(&mut self) -> Result<(), String> {
        self.child
            .start_kill()
            .map_err(|err| format!("failed to terminate MCP server process: {err}"))?;
        self.child
            .wait()
            .await
            .map_err(|err| format!("failed to wait for MCP server process: {err}"))?;
        Ok(())
    }

    /// Send a request and return its result, correlating on the JSON-RPC id.
    async fn request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "MCP request ID overflow".to_string())?;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
            id: Some(serde_json::Value::Number(serde_json::Number::from(id))),
        };
        let line = serde_json::to_string(&request)
            .map_err(|err| format!("failed to encode MCP request: {err}"))?;
        self.write_line(&line).await?;
        self.read_response(id, method).await
    }

    /// Send a JSON-RPC notification (no id, no reply). Notifications must omit
    /// the `id` field entirely, so this is built inline rather than reusing the
    /// request struct (whose `id` always serializes).
    async fn notify(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let mut message = serde_json::json!({ "jsonrpc": "2.0", "method": method });
        if let Some(params) = params {
            message["params"] = params;
        }
        let line = serde_json::to_string(&message)
            .map_err(|err| format!("failed to encode MCP notification: {err}"))?;
        self.write_line(&line).await
    }

    async fn write_line(&mut self, line: &str) -> Result<(), String> {
        self.stdin
            .write_all(format!("{line}\n").as_bytes())
            .await
            .map_err(|err| format!("failed to write MCP message: {err}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|err| format!("failed to flush MCP message: {err}"))
    }

    /// Read lines until the response whose id matches `expected_id` arrives,
    /// skipping any interleaved notifications or unrelated messages the server
    /// emits (logging, progress, …) — a single blind `read_line` would mistake
    /// the first such notification for the response.
    async fn read_response(
        &mut self,
        expected_id: i64,
        method: &str,
    ) -> Result<serde_json::Value, String> {
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout(self.timeout, self.reader.read_line(&mut line))
                .await
                .map_err(|_| {
                    format!(
                        "MCP server did not respond to `{method}` within {}s",
                        self.timeout.as_secs()
                    )
                })?
                .map_err(|err| format!("failed to read MCP response: {err}"))?;

            if read == 0 {
                return Err("MCP server closed the connection before responding".to_string());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let response: JsonRpcResponse = match serde_json::from_str(trimmed) {
                Ok(response) => response,
                // Not a response shape (e.g. a server->client notification/request);
                // ignore it and keep waiting for our correlated reply.
                Err(_) => continue,
            };

            let matches_id = response
                .id
                .as_ref()
                .and_then(|id| id.as_i64())
                .map(|id| id == expected_id)
                .unwrap_or(false);
            if !matches_id {
                continue;
            }

            if let Some(error) = response.error {
                return Err(format!("MCP RPC error {}: {}", error.code, error.message));
            }
            return response
                .result
                .ok_or_else(|| "MCP response did not include a result".to_string());
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Best-effort: don't leak the child if the handle is dropped without
        // an explicit `close`.
        let _ = self.child.start_kill();
    }
}
