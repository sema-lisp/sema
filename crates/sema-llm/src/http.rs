use std::time::Duration;

use crate::types::LlmError;

/// Default HTTP request timeout for LLM providers (2 minutes).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Create a new HTTP client with the given optional timeout.
/// Falls back to [`DEFAULT_TIMEOUT`] if `None`.
pub fn create_client(timeout: Option<Duration>) -> Result<reqwest::Client, LlmError> {
    let mut builder = reqwest::Client::builder();
    if let Some(t) = timeout.or(Some(DEFAULT_TIMEOUT)) {
        builder = builder.timeout(t);
    }
    builder
        .build()
        .map_err(|e| LlmError::Config(format!("failed to create http client: {e}")))
}

/// Apply a per-request timeout (milliseconds) to a request builder when set. Lets a
/// per-call `:timeout` override the client default without rebuilding the client.
pub fn with_timeout(
    rb: reqwest::RequestBuilder,
    timeout_ms: Option<u64>,
) -> reqwest::RequestBuilder {
    match timeout_ms {
        Some(ms) => rb.timeout(Duration::from_millis(ms)),
        None => rb,
    }
}
