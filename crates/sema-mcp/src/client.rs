//! MCP *client* transports.
//!
//! Sema is primarily an MCP *server* (see `server.rs`); this is the reverse
//! direction — connect to an external MCP server and speak JSON-RPC to it so
//! Sema code can consume its tools. Two transports live behind one [`McpClient`]:
//!
//! - **stdio** ([`McpClient::connect`]): spawn the server as a child process and
//!   exchange line-delimited JSON-RPC over stdin/stdout. No auth handshake — a
//!   server that needs a credential reads it from the environment Sema hands the
//!   child, so pass tokens through the `:env` map on `mcp/connect`.
//! - **Streamable HTTP** ([`McpClient::connect_http`]): POST each JSON-RPC message
//!   to a remote server's single MCP endpoint; the response is either a single
//!   JSON object or an SSE stream. Session continuity rides the `Mcp-Session-Id`
//!   header and the negotiated `MCP-Protocol-Version` header. A caller-supplied
//!   `:headers` map carries a static bearer token; the full OAuth 2.1 login flow
//!   for servers that require it is a later milestone (see
//!   `docs/plans/2026-06-21-mcp-client-spike.md`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::protocol::{JsonRpcRequest, JsonRpcResponse, Tool};

/// How long to wait for a single response before giving up, so a wedged server
/// surfaces as an error instead of hanging the calling Sema thread forever.
/// Generous because a `tools/call` can legitimately be slow.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// The MCP protocol revision this client advertises during `initialize` — the
/// latest revision we implement (a client SHOULD offer the newest it supports).
const PROTOCOL_VERSION: &str = "2025-11-25";

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

/// Spec for connecting to a remote MCP server over Streamable HTTP. `headers`
/// are attached to every request — the place to pass a caller-supplied bearer
/// token (`{"Authorization" "Bearer …"}`) until the OAuth flow lands.
#[derive(Debug, Clone, Default)]
pub struct McpHttpConfig {
    pub url: String,
    pub headers: HashMap<String, String>,
}

impl McpHttpConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: HashMap::new(),
        }
    }
}

/// A live connection to an MCP server. Dropping it terminates the underlying
/// stdio child (HTTP sessions are best-effort DELETE'd via [`McpClient::close`]).
pub struct McpClient {
    transport: Transport,
    next_id: i64,
    timeout: Duration,
}

enum Transport {
    Stdio(StdioTransport),
    Http(HttpTransport),
    LegacySse(LegacySseTransport),
}

impl McpClient {
    /// Spawn a stdio server and wire up its stdio. Does not perform the
    /// `initialize` handshake — call [`McpClient::initialize`] next.
    pub async fn connect(config: McpClientConfig) -> Result<Self, String> {
        let transport = StdioTransport::spawn(config).await?;
        Ok(Self {
            transport: Transport::Stdio(transport),
            next_id: 1,
            timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    /// Prepare a Streamable-HTTP connection. Does not perform the `initialize`
    /// handshake — call [`McpClient::initialize`] next.
    pub async fn connect_http(config: McpHttpConfig) -> Result<Self, String> {
        let transport = HttpTransport::new(config)?;
        Ok(Self {
            transport: Transport::Http(transport),
            next_id: 1,
            timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    /// Connect over the deprecated 2024-11-05 HTTP+SSE transport: open the SSE
    /// stream and read the `endpoint` event that names the POST URL. Does not
    /// perform the handshake — call [`McpClient::initialize`] next.
    pub async fn connect_legacy_sse(config: McpHttpConfig) -> Result<Self, String> {
        let transport = LegacySseTransport::connect(config, DEFAULT_REQUEST_TIMEOUT).await?;
        Ok(Self {
            transport: Transport::LegacySse(transport),
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
        match &mut self.transport {
            Transport::Stdio(t) => t.shutdown().await,
            Transport::Http(t) => t.shutdown().await,
            Transport::LegacySse(_) => Ok(()),
        }
    }

    /// The `WWW-Authenticate` challenge from the most recent HTTP `401`/`403`, if
    /// the last request was refused for auth (Streamable HTTP or legacy SSE).
    pub fn http_challenge(&self) -> Option<String> {
        match &self.transport {
            Transport::Http(t) => t.last_challenge.clone(),
            Transport::LegacySse(t) => t.last_challenge.clone(),
            Transport::Stdio(_) => None,
        }
    }

    /// The HTTP status code of the most recent request when it was not a success
    /// (Streamable HTTP or legacy SSE) — used to detect a legacy server (`404`/
    /// `405`) or a mid-session auth challenge (`401`/`403`).
    pub fn http_last_status(&self) -> Option<u16> {
        match &self.transport {
            Transport::Http(t) => t.last_status,
            Transport::LegacySse(t) => t.last_status,
            Transport::Stdio(_) => None,
        }
    }

    /// Attach a bearer token to all subsequent HTTP requests (no-op for stdio).
    pub fn set_bearer_token(&mut self, token: &str) {
        if let Transport::Http(t) = &mut self.transport {
            t.set_bearer(token);
        }
    }

    /// Update the bearer token after a mid-session re-authorization and make the
    /// connection ready to retry. Streamable HTTP just swaps the header (requests
    /// are independent); the legacy transport must **reconnect** — its SSE stream
    /// was opened with the stale token — so it re-opens the stream with the new
    /// token and re-runs the handshake. No-op for stdio.
    pub async fn reauthorize_bearer(&mut self, token: &str) -> Result<(), String> {
        // Capture legacy reconnect params without holding a borrow across the
        // transport reassignment below.
        let legacy = match &self.transport {
            Transport::LegacySse(t) => Some((t.base_url.clone(), t.headers.clone())),
            _ => None,
        };
        match &mut self.transport {
            Transport::Http(t) => {
                t.set_bearer(token);
                return Ok(());
            }
            Transport::Stdio(_) => return Ok(()),
            Transport::LegacySse(_) => {}
        }
        let (url, mut headers) = legacy.expect("legacy transport");
        headers.insert("Authorization".to_string(), format!("Bearer {token}"));
        let fresh =
            LegacySseTransport::connect(McpHttpConfig { url, headers }, self.timeout).await?;
        self.transport = Transport::LegacySse(fresh);
        self.next_id = 1;
        self.initialize().await.map(|_| ())
    }

    /// The remote server URL (HTTP or legacy SSE), for keying the token store.
    pub fn http_url(&self) -> Option<String> {
        match &self.transport {
            Transport::Http(t) => Some(t.url.clone()),
            Transport::LegacySse(t) => Some(t.base_url.clone()),
            Transport::Stdio(_) => None,
        }
    }

    /// Send a request under a fresh JSON-RPC id and return its result.
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
        match &mut self.transport {
            Transport::Stdio(t) => t.request(id, method, params, self.timeout).await,
            Transport::Http(t) => t.request(id, method, params, self.timeout).await,
            Transport::LegacySse(t) => t.request(id, method, params, self.timeout).await,
        }
    }

    async fn notify(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        match &mut self.transport {
            Transport::Stdio(t) => t.notify(method, params).await,
            Transport::Http(t) => t.notify(method, params).await,
            Transport::LegacySse(t) => t.notify(method, params).await,
        }
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers shared by both transports
// ---------------------------------------------------------------------------

fn response_matches_id(response: &JsonRpcResponse, expected_id: i64) -> bool {
    response
        .id
        .as_ref()
        .and_then(|id| id.as_i64())
        .map(|id| id == expected_id)
        .unwrap_or(false)
}

/// Turn a correlated JSON-RPC response into its `result`, mapping an RPC-level
/// error into an `Err`.
fn extract_result(response: JsonRpcResponse) -> Result<serde_json::Value, String> {
    if let Some(error) = response.error {
        return Err(format!("MCP RPC error {}: {}", error.code, error.message));
    }
    response
        .result
        .ok_or_else(|| "MCP response did not include a result".to_string())
}

fn encode_request(
    id: i64,
    method: &str,
    params: Option<serde_json::Value>,
) -> Result<String, String> {
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: method.to_string(),
        params,
        id: Some(serde_json::Value::Number(serde_json::Number::from(id))),
    };
    serde_json::to_string(&request).map_err(|err| format!("failed to encode MCP request: {err}"))
}

/// Build a JSON-RPC notification (no `id`). Notifications must omit the `id`
/// field entirely, so this is built by hand rather than reusing the request
/// struct (whose `id` always serializes).
fn notification_value(method: &str, params: Option<serde_json::Value>) -> serde_json::Value {
    let mut message = serde_json::json!({ "jsonrpc": "2.0", "method": method });
    if let Some(params) = params {
        message["params"] = params;
    }
    message
}

// ---------------------------------------------------------------------------
// stdio transport
// ---------------------------------------------------------------------------

struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
}

impl StdioTransport {
    async fn spawn(config: McpClientConfig) -> Result<Self, String> {
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
        })
    }

    async fn request(
        &mut self,
        id: i64,
        method: &str,
        params: Option<serde_json::Value>,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let line = encode_request(id, method, params)?;
        self.write_line(&line).await?;
        self.read_response(id, method, timeout).await
    }

    async fn notify(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let line = serde_json::to_string(&notification_value(method, params))
            .map_err(|err| format!("failed to encode MCP notification: {err}"))?;
        self.write_line(&line).await
    }

    async fn shutdown(&mut self) -> Result<(), String> {
        self.child
            .start_kill()
            .map_err(|err| format!("failed to terminate MCP server process: {err}"))?;
        self.child
            .wait()
            .await
            .map_err(|err| format!("failed to wait for MCP server process: {err}"))?;
        Ok(())
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
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout(timeout, self.reader.read_line(&mut line))
                .await
                .map_err(|_| {
                    format!(
                        "MCP server did not respond to `{method}` within {}s",
                        timeout.as_secs()
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
                // Not JSON-RPC at all; ignore and keep waiting.
                Err(_) => continue,
            };

            // Skip server->client requests/notifications (they carry `method`) even
            // when their id collides with ours — only a real response ends the wait.
            if !response.is_response() || !response_matches_id(&response, expected_id) {
                continue;
            }
            return extract_result(response);
        }
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Best-effort: don't leak the child if the handle is dropped without
        // an explicit `close`.
        let _ = self.child.start_kill();
    }
}

// ---------------------------------------------------------------------------
// Streamable HTTP transport
// ---------------------------------------------------------------------------

struct HttpTransport {
    client: reqwest::Client,
    url: String,
    headers: HashMap<String, String>,
    /// Assigned by the server on the `initialize` response; echoed on every
    /// subsequent request so the server can associate them with the session.
    session_id: Option<String>,
    /// The protocol version negotiated in `InitializeResult`, sent back as the
    /// `MCP-Protocol-Version` header on all following requests.
    protocol_version: Option<String>,
    /// The `WWW-Authenticate` header from the most recent `401`, so the auth
    /// layer can discover where to log in and retry.
    last_challenge: Option<String>,
    /// The status code of the most recent non-success response, so a caller can
    /// detect a `404`/`405` that signals a legacy (HTTP+SSE) server.
    last_status: Option<u16>,
}

impl HttpTransport {
    fn new(config: McpHttpConfig) -> Result<Self, String> {
        if config.url.is_empty() {
            return Err("mcp/connect (http) requires a non-empty :url".to_string());
        }
        let client = reqwest::Client::builder()
            .build()
            .map_err(|err| format!("failed to build HTTP client: {err}"))?;
        Ok(Self {
            client,
            url: config.url,
            headers: config.headers,
            session_id: None,
            protocol_version: None,
            last_challenge: None,
            last_status: None,
        })
    }

    /// Attach a bearer token to every subsequent request.
    fn set_bearer(&mut self, token: &str) {
        self.headers
            .insert("Authorization".to_string(), format!("Bearer {token}"));
    }

    /// Attach the session / protocol-version / caller headers that apply once
    /// they are known. On the first `initialize` request `session_id` and
    /// `protocol_version` are still `None`, so nothing session-specific is sent.
    fn apply_headers(&self, mut builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(session_id) = &self.session_id {
            builder = builder.header("Mcp-Session-Id", session_id);
        }
        if let Some(version) = &self.protocol_version {
            builder = builder.header("MCP-Protocol-Version", version);
        }
        for (key, value) in &self.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }
        builder
    }

    fn capture_session(&mut self, response: &reqwest::Response) {
        if let Some(value) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            self.session_id = Some(value.to_string());
        }
    }

    async fn request(
        &mut self,
        id: i64,
        method: &str,
        params: Option<serde_json::Value>,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let body = encode_request(id, method, params)?;
        let builder = self
            .client
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .body(body);
        let builder = self.apply_headers(builder);

        let response = tokio::time::timeout(timeout, builder.send())
            .await
            .map_err(|_| {
                format!(
                    "MCP server did not respond to `{method}` within {}s",
                    timeout.as_secs()
                )
            })?
            .map_err(|err| format!("failed to send MCP request: {err}"))?;

        self.capture_session(&response);

        let status = response.status();
        self.last_status = (!status.is_success()).then_some(status.as_u16());
        // Record any `WWW-Authenticate` challenge (cleared on success) so the auth
        // layer can re-authorize and retry — a `401` (token missing/expired) or a
        // `403 insufficient_scope` (needs step-up). Reflects the current response.
        self.last_challenge = if status.is_success() {
            None
        } else {
            response
                .headers()
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        if status.as_u16() == 404 {
            // Session was terminated/expired; per the spec the client must start
            // a fresh session with a new `initialize`. Surface it so the caller
            // reconnects rather than silently retrying against a dead session.
            return Err(format!(
                "MCP session expired (HTTP 404) on `{method}`; reconnect required"
            ));
        }
        if status.as_u16() == 401 {
            return Err(format!(
                "MCP server requires authorization (HTTP 401) on `{method}`"
            ));
        }
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(format!(
                "MCP server returned HTTP {} on `{method}`: {}",
                status.as_u16(),
                detail.trim()
            ));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let result = if content_type.contains("text/event-stream") {
            self.read_sse_response(response, id, timeout).await?
        } else {
            let text = response
                .text()
                .await
                .map_err(|err| format!("failed to read MCP response body: {err}"))?;
            let response: JsonRpcResponse = serde_json::from_str(text.trim())
                .map_err(|err| format!("failed to decode MCP response: {err}"))?;
            if !response.is_response() || !response_matches_id(&response, id) {
                return Err(format!(
                    "MCP response id mismatch on `{method}` (expected {id})"
                ));
            }
            extract_result(response)?
        };

        // Record the negotiated protocol version so it rides subsequent requests.
        if method == "initialize" {
            if let Some(version) = result.get("protocolVersion").and_then(|v| v.as_str()) {
                self.protocol_version = Some(version.to_string());
            }
        }
        Ok(result)
    }

    async fn notify(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let body = serde_json::to_string(&notification_value(method, params))
            .map_err(|err| format!("failed to encode MCP notification: {err}"))?;
        let builder = self
            .client
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .body(body);
        let builder = self.apply_headers(builder);

        let response = builder
            .send()
            .await
            .map_err(|err| format!("failed to send MCP notification: {err}"))?;
        self.capture_session(&response);
        let status = response.status();
        // A notification is accepted with `202 Accepted` (no body); tolerate any
        // 2xx in case a server answers `200`.
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(format!(
                "MCP server rejected notification `{method}`: HTTP {} {}",
                status.as_u16(),
                detail.trim()
            ));
        }
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<(), String> {
        // Best-effort session teardown: DELETE with the session id. A server that
        // doesn't allow client-driven termination answers `405`, which is fine.
        if self.session_id.is_some() {
            let builder = self.client.delete(&self.url);
            let builder = self.apply_headers(builder);
            let _ = builder.send().await;
        }
        Ok(())
    }

    /// Read the POST-initiated SSE stream until the JSON-RPC response correlated
    /// to `expected_id` arrives, ignoring any interleaved server→client
    /// notifications/requests the server may emit before it.
    async fn read_sse_response(
        &mut self,
        response: reqwest::Response,
        expected_id: i64,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let mut stream = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        let mut data_lines: Vec<String> = Vec::new();

        loop {
            let next = tokio::time::timeout(timeout, stream.next())
                .await
                .map_err(|_| {
                    format!("MCP SSE stream stalled waiting for response id {expected_id}")
                })?;
            let Some(chunk) = next else {
                // Stream ended. Give any un-terminated trailing event one last
                // chance before declaring the response missing.
                if let Some(result) = try_take_sse_event(&mut data_lines, expected_id)? {
                    return Ok(result);
                }
                return Err("MCP SSE stream ended before the response arrived".to_string());
            };
            let chunk = chunk.map_err(|err| format!("failed to read MCP SSE stream: {err}"))?;
            buffer.extend_from_slice(&chunk);

            while let Some(line) = take_line(&mut buffer) {
                if line.is_empty() {
                    // Event boundary — try to correlate the accumulated data.
                    if let Some(result) = try_take_sse_event(&mut data_lines, expected_id)? {
                        return Ok(result);
                    }
                    continue;
                }
                if let Some(rest) = line.strip_prefix("data:") {
                    // A `data:` value may have a single leading space per the SSE grammar.
                    data_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                }
                // `id:` / `event:` / `retry:` / comment lines are ignored for now;
                // event-id resumption (`Last-Event-ID`) is a later refinement.
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Legacy HTTP+SSE transport (deprecated 2024-11-05 two-endpoint shape)
// ---------------------------------------------------------------------------

/// The deprecated two-endpoint transport: a long-lived `GET` SSE stream carries
/// every server→client message, and the client `POST`s requests to a separate
/// URL announced by the stream's first `endpoint` event. Kept for backwards
/// compatibility with `2024-11-05` servers.
struct LegacySseTransport {
    client: reqwest::Client,
    base_url: String,
    post_url: String,
    headers: HashMap<String, String>,
    #[allow(clippy::type_complexity)]
    stream: std::pin::Pin<Box<dyn futures::Stream<Item = Result<Vec<u8>, reqwest::Error>> + Send>>,
    buffer: Vec<u8>,
    /// Status + `WWW-Authenticate` of the most recent POST failure, so a mid-
    /// session `401`/`403` on a legacy server can be re-authorized like an HTTP
    /// one (parity — many servers still run this transport).
    last_status: Option<u16>,
    last_challenge: Option<String>,
}

impl LegacySseTransport {
    async fn connect(config: McpHttpConfig, timeout: Duration) -> Result<Self, String> {
        if config.url.is_empty() {
            return Err("mcp/connect (legacy sse) requires a non-empty :url".to_string());
        }
        let client = reqwest::Client::builder()
            .build()
            .map_err(|err| format!("failed to build HTTP client: {err}"))?;
        let mut builder = client
            .get(&config.url)
            .header("Accept", "text/event-stream");
        for (key, value) in &config.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }
        let response = tokio::time::timeout(timeout, builder.send())
            .await
            .map_err(|_| "legacy SSE connect timed out".to_string())?
            .map_err(|err| format!("legacy SSE GET failed: {err}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "legacy SSE GET returned HTTP {}",
                response.status().as_u16()
            ));
        }
        let stream = response.bytes_stream().map(|r| r.map(|b| b.to_vec()));
        let mut transport = Self {
            client,
            base_url: config.url.clone(),
            post_url: String::new(),
            headers: config.headers,
            stream: Box::pin(stream),
            buffer: Vec::new(),
            last_status: None,
            last_challenge: None,
        };

        // The first meaningful event names the POST endpoint.
        loop {
            let (event, data) = transport.next_event(timeout).await?;
            if event == "endpoint" {
                transport.post_url = resolve_url(&transport.base_url, data.trim())?;
                return Ok(transport);
            }
        }
    }

    async fn request(
        &mut self,
        id: i64,
        method: &str,
        params: Option<serde_json::Value>,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let body = encode_request(id, method, params)?;
        self.post(body).await?;
        // The response arrives on the shared SSE stream; skip unrelated messages.
        loop {
            let (_event, data) = self.next_event(timeout).await?;
            if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&data) {
                if response.is_response() && response_matches_id(&response, id) {
                    return extract_result(response);
                }
            }
        }
    }

    async fn notify(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let body = serde_json::to_string(&notification_value(method, params))
            .map_err(|err| format!("failed to encode MCP notification: {err}"))?;
        self.post(body).await
    }

    async fn post(&mut self, body: String) -> Result<(), String> {
        let mut builder = self
            .client
            .post(&self.post_url)
            .header("Content-Type", "application/json")
            .body(body);
        for (key, value) in &self.headers {
            builder = builder.header(key.as_str(), value.as_str());
        }
        let response = builder
            .send()
            .await
            .map_err(|err| format!("legacy SSE POST failed: {err}"))?;
        let status = response.status();
        self.last_status = (!status.is_success()).then_some(status.as_u16());
        self.last_challenge = if status.is_success() {
            None
        } else {
            response
                .headers()
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        if !status.is_success() {
            return Err(format!("legacy SSE POST returned HTTP {}", status.as_u16()));
        }
        Ok(())
    }

    /// Read the SSE stream until one complete event, returning its `event` type
    /// (defaulting to `message`) and joined `data`.
    async fn next_event(&mut self, timeout: Duration) -> Result<(String, String), String> {
        let mut event_type = String::new();
        let mut data_lines: Vec<String> = Vec::new();
        loop {
            while let Some(line) = take_line(&mut self.buffer) {
                if line.is_empty() {
                    if !data_lines.is_empty() || !event_type.is_empty() {
                        let event = if event_type.is_empty() {
                            "message".to_string()
                        } else {
                            std::mem::take(&mut event_type)
                        };
                        return Ok((event, data_lines.join("\n")));
                    }
                    continue;
                }
                if let Some(rest) = line.strip_prefix("event:") {
                    event_type = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("data:") {
                    data_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                }
            }
            let next = tokio::time::timeout(timeout, self.stream.next())
                .await
                .map_err(|_| "legacy SSE stream stalled".to_string())?;
            match next {
                Some(Ok(chunk)) => self.buffer.extend_from_slice(&chunk),
                Some(Err(err)) => return Err(format!("legacy SSE read error: {err}")),
                None => return Err("legacy SSE stream closed unexpectedly".to_string()),
            }
        }
    }
}

/// Resolve a possibly-relative `endpoint` value against the SSE URL's origin.
fn resolve_url(base: &str, target: &str) -> Result<String, String> {
    let base = url::Url::parse(base).map_err(|e| format!("invalid legacy SSE base URL: {e}"))?;
    base.join(target)
        .map(|u| u.to_string())
        .map_err(|e| format!("could not resolve legacy endpoint `{target}`: {e}"))
}

/// Pull the next complete line (through the `\n`) from a raw byte buffer,
/// returning it without the trailing CR/LF, decoded as UTF-8. `None` when no
/// complete line is buffered yet. Framing on the `\n` *byte* (which never
/// appears inside a multi-byte UTF-8 sequence) and decoding whole lines — never
/// arbitrary network chunks — keeps multi-byte characters intact across chunk
/// boundaries.
fn take_line(buffer: &mut Vec<u8>) -> Option<String> {
    let newline = buffer.iter().position(|&b| b == b'\n')?;
    let mut line: Vec<u8> = buffer.drain(..=newline).collect();
    line.pop(); // drop '\n'
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Some(String::from_utf8_lossy(&line).into_owned())
}

/// Interpret an accumulated SSE event's `data` as a JSON-RPC message. Returns
/// `Ok(Some(result))` when it is the correlated response, `Ok(None)` when it is
/// an unrelated message to skip, and `Err` for an RPC-level error on our id.
fn try_take_sse_event(
    data_lines: &mut Vec<String>,
    expected_id: i64,
) -> Result<Option<serde_json::Value>, String> {
    if data_lines.is_empty() {
        return Ok(None);
    }
    let data = std::mem::take(data_lines).join("\n");
    match serde_json::from_str::<JsonRpcResponse>(&data) {
        Ok(response) if response.is_response() && response_matches_id(&response, expected_id) => {
            extract_result(response).map(Some)
        }
        // A server->client notification/request, or a response for a different
        // id — not ours; keep reading.
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::take_line;

    #[test]
    fn take_line_reassembles_utf8_split_across_chunks() {
        // Feed the bytes one at a time (worst-case chunk boundaries). `take_line`
        // must yield nothing until the '\n', then decode the whole line intact —
        // the emoji (4 bytes) and accented chars must NOT become U+FFFD.
        let full = "data: {\"text\":\"😀 café 日本\"}\n".as_bytes();
        let mut buf: Vec<u8> = Vec::new();
        for &b in &full[..full.len() - 1] {
            buf.push(b);
            assert!(
                take_line(&mut buf).is_none(),
                "no complete line before the newline"
            );
        }
        buf.push(*full.last().unwrap());
        assert_eq!(
            take_line(&mut buf).as_deref(),
            Some("data: {\"text\":\"😀 café 日本\"}")
        );
        assert!(take_line(&mut buf).is_none());
    }

    #[test]
    fn take_line_strips_crlf_and_yields_lines_in_order() {
        let mut buf = b"first\r\nsecond\n".to_vec();
        assert_eq!(take_line(&mut buf).as_deref(), Some("first"));
        assert_eq!(take_line(&mut buf).as_deref(), Some("second"));
        assert!(take_line(&mut buf).is_none());
    }
}
