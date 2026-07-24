use std::time::Duration;

use crate::types::LlmError;

/// Default HTTP request timeout for LLM providers (2 minutes).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Install rustls's ring `CryptoProvider` once per process. reqwest is built with
/// `rustls-no-provider` (the default `rustls` feature pins aws-lc-rs, whose C build
/// dominates cold compiles); with no provider installed, ANY `reqwest::Client`
/// construction panics. Every crate that builds a client must call its guard first.
pub fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Err(_) just means another provider is already installed — that's fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// A `reqwest::ClientBuilder` that honors the standard proxy environment
/// variables but does NOT run reqwest's automatic *system* proxy detection.
///
/// reqwest's default auto-detection reads the OS proxy configuration; on macOS
/// (and Windows) that is a synchronous `SCDynamicStore`/registry lookup that
/// blocks the calling thread for ~2 s the first time a client is built in a
/// process. In the cooperative unified runtime that first `Client` build runs on
/// the VM thread inside a drive quantum (e.g. the first `http/get` or the first
/// `llm/*` call), so it stalls the whole scheduler — and everything parked on it,
/// including a sibling task's `async/sleep`+`async/cancel` — for the duration.
/// That is a "no blocking work on the VM thread inside a quantum" violation and
/// it makes prompt cancellation impossible until the lookup finishes.
///
/// `.no_proxy()` disables that lookup; the portable `*_PROXY`/`NO_PROXY`
/// environment proxies (what CI, containers, and most proxied users actually
/// rely on) are re-added explicitly — reqwest reads those instantly on every
/// platform. Specific `HTTPS_PROXY`/`HTTP_PROXY` are added before `ALL_PROXY` so
/// they take precedence, matching reqwest's documented ordering.
///
/// Also installs the rustls crypto provider first (see
/// [`ensure_crypto_provider`]) so the returned builder can be `.build()`-ed
/// without panicking under `rustls-no-provider`.
pub fn proxy_env_client_builder() -> reqwest::ClientBuilder {
    ensure_crypto_provider();
    let mut builder = reqwest::Client::builder().no_proxy();
    let no_proxy = reqwest::NoProxy::from_env();
    let pick = |names: &[&str]| -> Option<String> {
        names
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
    };
    // Order matters: reqwest uses the first proxy whose scheme matches the
    // request, so the scheme-specific entries must precede the catch-all.
    if let Some(url) = pick(&["HTTPS_PROXY", "https_proxy"]) {
        if let Ok(proxy) = reqwest::Proxy::https(url.as_str()) {
            builder = builder.proxy(proxy.no_proxy(no_proxy.clone()));
        }
    }
    if let Some(url) = pick(&["HTTP_PROXY", "http_proxy"]) {
        if let Ok(proxy) = reqwest::Proxy::http(url.as_str()) {
            builder = builder.proxy(proxy.no_proxy(no_proxy.clone()));
        }
    }
    if let Some(url) = pick(&["ALL_PROXY", "all_proxy"]) {
        if let Ok(proxy) = reqwest::Proxy::all(url.as_str()) {
            builder = builder.proxy(proxy.no_proxy(no_proxy.clone()));
        }
    }
    builder
}

/// Create a new HTTP client with the given optional timeout.
/// Falls back to [`DEFAULT_TIMEOUT`] if `None`.
pub fn create_client(timeout: Option<Duration>) -> Result<reqwest::Client, LlmError> {
    let mut builder = proxy_env_client_builder();
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
