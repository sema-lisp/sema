//! End-to-end OAuth login orchestration: discover → obtain a client_id →
//! authorization-code + PKCE → tokens. Ties together `discovery`, `flow`, and a
//! [`RedirectDriver`] (loopback browser or device/paste). The result is a
//! [`StoredCredentials`] the caller persists and attaches to MCP requests.

use super::discovery::{self, AuthorizationServerMetadata, ProtectedResourceMetadata};
use super::loopback::RedirectDriver;
use super::store::{now_unix, ClientInfo, StoredCredentials, TokenSet, TokenStore};
use super::{flow, new_pkce_session};

/// Seconds of clock skew to treat a token as already expired before its
/// nominal expiry, so we refresh proactively rather than mid-request.
const EXPIRY_SKEW_SECS: u64 = 60;

/// Inputs for a login attempt against one MCP server.
pub struct LoginConfig<'a> {
    /// The MCP server endpoint URL (`:url`).
    pub mcp_url: &'a str,
    /// The `resource_metadata` URL advertised by the server's `401`, if any.
    pub resource_metadata_url: Option<&'a str>,
    /// The `scope` advertised by the `401` (authoritative when present).
    pub requested_scope: Option<&'a str>,
    /// A pre-registered client_id the user configured (`:auth {:client-id …}`).
    pub preconfigured_client_id: Option<&'a str>,
}

/// The discovery outcome, cached so refresh/re-scope can skip re-probing.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub resource: String,
    pub protected_resource: ProtectedResourceMetadata,
    pub authorization_server: AuthorizationServerMetadata,
}

/// Whether two URLs share scheme + host + port (same origin). Used to bind the
/// RFC 8707 `resource` audience to the actual MCP server being connected to.
fn same_origin(a: &str, b: &str) -> bool {
    match (url::Url::parse(a), url::Url::parse(b)) {
        (Ok(a), Ok(b)) => {
            a.scheme() == b.scheme()
                && a.host_str() == b.host_str()
                && a.port_or_known_default() == b.port_or_known_default()
        }
        _ => false,
    }
}

/// Walk the metadata chain: PRM (RFC 9728) → AS metadata (RFC 8414/OIDC), and
/// enforce the PKCE-S256 gate.
pub async fn discover(
    client: &reqwest::Client,
    config: &LoginConfig<'_>,
) -> Result<Discovered, String> {
    let prm = discovery::fetch_protected_resource_metadata(
        client,
        config.mcp_url,
        config.resource_metadata_url,
    )
    .await?;
    // RFC 9728: the advertised `resource` must be the server we're connecting to.
    // Reject a PRM that binds our token's audience to a different origin (a
    // malicious/mis-hosted resource-metadata document).
    if !same_origin(config.mcp_url, &prm.resource) {
        return Err(format!(
            "protected resource metadata `resource` ({}) does not match the server being \
             connected to ({}); refusing to authorize",
            prm.resource, config.mcp_url
        ));
    }
    if prm.authorization_servers.is_empty() {
        return Err("protected resource metadata lists no authorization servers".to_string());
    }
    // Try each advertised authorization server in order; use the first that
    // resolves, matches its issuer (RFC 8414 §3.3 mix-up defense), and supports
    // PKCE-S256. Collect why each was skipped for a useful error if none work.
    let mut skipped: Vec<String> = Vec::new();
    let mut chosen: Option<AuthorizationServerMetadata> = None;
    for issuer in &prm.authorization_servers {
        let as_meta = match discovery::fetch_authorization_server_metadata(client, issuer).await {
            Ok(meta) => meta,
            Err(e) => {
                skipped.push(format!("{issuer}: {e}"));
                continue;
            }
        };
        if as_meta.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
            skipped.push(format!(
                "{issuer}: metadata issuer ({}) does not match",
                as_meta.issuer
            ));
            continue;
        }
        if !as_meta.supports_pkce_s256() {
            skipped.push(format!("{issuer}: no PKCE S256 support"));
            continue;
        }
        chosen = Some(as_meta);
        break;
    }
    let as_meta = chosen.ok_or_else(|| {
        format!(
            "no usable authorization server among {} advertised: {}",
            prm.authorization_servers.len(),
            skipped.join("; ")
        )
    })?;

    Ok(Discovered {
        resource: prm.resource.clone(),
        protected_resource: prm,
        authorization_server: as_meta,
    })
}

/// Resolve the client_id: a pre-configured one, an existing DCR registration, or
/// a fresh Dynamic Client Registration.
async fn resolve_client(
    client: &reqwest::Client,
    config: &LoginConfig<'_>,
    discovered: &Discovered,
    existing: Option<ClientInfo>,
    redirect_uri: &str,
) -> Result<ClientInfo, String> {
    if let Some(id) = config.preconfigured_client_id {
        return Ok(ClientInfo {
            client_id: id.to_string(),
            client_secret: None,
        });
    }
    if let Some(info) = existing {
        return Ok(info);
    }
    let registration_endpoint = discovered
        .authorization_server
        .registration_endpoint
        .as_deref()
        .ok_or_else(|| {
            "server requires a pre-registered client (no dynamic registration endpoint); \
             pass :auth {:client-id \"…\"}"
                .to_string()
        })?;
    flow::register_client(
        client,
        registration_endpoint,
        redirect_uri,
        "Sema MCP client",
    )
    .await
}

/// The scopes to request: the `401` challenge scope wins; else the PRM
/// `scopes_supported`; else none.
fn resolve_scopes(config: &LoginConfig<'_>, discovered: &Discovered) -> Vec<String> {
    if let Some(scope) = config.requested_scope {
        let scopes: Vec<String> = scope.split_whitespace().map(String::from).collect();
        if !scopes.is_empty() {
            return scopes;
        }
    }
    discovered.protected_resource.scopes_supported.clone()
}

/// Run the full login and return persistable credentials.
pub async fn login(
    client: &reqwest::Client,
    config: &LoginConfig<'_>,
    existing_client_info: Option<ClientInfo>,
    redirect: &dyn RedirectDriver,
) -> Result<StoredCredentials, String> {
    let discovered = discover(client, config).await?;
    let redirect_uri = redirect.redirect_uri();
    let client_info = resolve_client(
        client,
        config,
        &discovered,
        existing_client_info,
        &redirect_uri,
    )
    .await?;
    let scopes = resolve_scopes(config, &discovered);

    let pkce = new_pkce_session();
    let authorize_url = flow::build_authorize_url(
        &discovered.authorization_server.authorization_endpoint,
        &client_info.client_id,
        &redirect_uri,
        &pkce.challenge,
        &pkce.state,
        &scopes,
        &discovered.resource,
    )?;

    // Blocking browser leg (opens the browser + captures the loopback redirect).
    let code = redirect.drive(&authorize_url, &pkce.state)?;

    let token = flow::exchange_code(
        client,
        &discovered.authorization_server.token_endpoint,
        &client_info.client_id,
        client_info.client_secret.as_deref(),
        &code,
        &redirect_uri,
        &pkce.verifier,
        &discovered.resource,
    )
    .await?;

    Ok(StoredCredentials {
        server_url: config.mcp_url.to_string(),
        tokens: TokenSet::from_response(
            token.access_token,
            token.refresh_token,
            token.expires_in,
            token.scope,
            now_unix(),
        ),
        client_info: Some(client_info),
    })
}

/// Refresh an access token with the stored refresh token (re-discovering the
/// token endpoint). Preserves the prior refresh token / scope when the response
/// omits a rotated value.
pub async fn refresh(
    client: &reqwest::Client,
    config: &LoginConfig<'_>,
    creds: &StoredCredentials,
) -> Result<TokenSet, String> {
    let discovered = discover(client, config).await?;
    let client_info = creds
        .client_info
        .as_ref()
        .ok_or_else(|| "cannot refresh: no client registration stored".to_string())?;
    let refresh_token = creds
        .tokens
        .refresh_token
        .as_ref()
        .ok_or_else(|| "cannot refresh: no refresh token stored".to_string())?;
    let token = flow::refresh_tokens(
        client,
        &discovered.authorization_server.token_endpoint,
        &client_info.client_id,
        client_info.client_secret.as_deref(),
        refresh_token,
        &discovered.resource,
    )
    .await?;
    Ok(TokenSet::from_response(
        token.access_token,
        token
            .refresh_token
            .or_else(|| creds.tokens.refresh_token.clone()),
        token.expires_in,
        token.scope.or_else(|| creds.tokens.scope.clone()),
        now_unix(),
    ))
}

/// Obtain a valid access token for the server, using the store: a still-valid
/// cached token is returned as-is (silent reconnect); an expired one is
/// refreshed; otherwise a full browser login runs. New credentials are
/// persisted, reusing any existing client registration.
pub async fn ensure_access_token(
    client: &reqwest::Client,
    store: &dyn TokenStore,
    config: &LoginConfig<'_>,
    redirect: &dyn RedirectDriver,
) -> Result<String, String> {
    if let Some(mut creds) = store.load(config.mcp_url) {
        if !creds.tokens.is_expired(now_unix(), EXPIRY_SKEW_SECS) {
            return Ok(creds.tokens.access_token);
        }
        if creds.tokens.refresh_token.is_some() {
            if let Ok(tokens) = refresh(client, config, &creds).await {
                creds.tokens = tokens;
                store.save(&creds)?;
                return Ok(creds.tokens.access_token);
            }
            // Refresh failed (revoked/expired refresh token) — fall through to a
            // fresh login.
        }
    }
    let existing_client_info = store.load(config.mcp_url).and_then(|c| c.client_info);
    let creds = login(client, config, existing_client_info, redirect).await?;
    store.save(&creds)?;
    Ok(creds.tokens.access_token)
}

/// Order-preserving union of two space-separated scope strings.
pub fn union_scopes(a: Option<&str>, b: Option<&str>) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for source in [a, b].into_iter().flatten() {
        for token in source.split_whitespace() {
            if !out.iter().any(|s| s == token) {
                out.push(token.to_string());
            }
        }
    }
    (!out.is_empty()).then(|| out.join(" "))
}

/// React to a mid-session auth challenge on an already-connected server and
/// return a fresh access token to retry with, or `None` if the status isn't an
/// auth challenge we handle. Handles two cases:
///
/// - **401** (token missing/expired): refresh with the stored refresh token, or
///   fall back to a full login.
/// - **403 `insufficient_scope`**: step-up — re-authorize requesting the *union*
///   of the previously-granted scopes and the scopes the challenge demands.
///
/// New credentials are persisted. `redirect` is only used when a full
/// (re-)authorization is required.
pub async fn reauth_on_challenge(
    client: &reqwest::Client,
    store: &dyn TokenStore,
    url: &str,
    status: Option<u16>,
    challenge_header: Option<&str>,
    preconfigured_client_id: Option<&str>,
    redirect: &dyn RedirectDriver,
) -> Result<Option<String>, String> {
    let challenge = challenge_header
        .map(discovery::parse_www_authenticate)
        .unwrap_or_default();
    let existing = store.load(url);
    let client_info = existing.as_ref().and_then(|c| c.client_info.clone());

    match status {
        Some(401) => {
            // Try a refresh first, then fall back to a full login.
            if let Some(creds) = &existing {
                if creds.tokens.refresh_token.is_some() {
                    let config = LoginConfig {
                        mcp_url: url,
                        resource_metadata_url: challenge.resource_metadata.as_deref(),
                        requested_scope: creds.tokens.scope.as_deref(),
                        preconfigured_client_id,
                    };
                    if let Ok(tokens) = refresh(client, &config, creds).await {
                        let mut updated = creds.clone();
                        updated.tokens = tokens;
                        store.save(&updated)?;
                        return Ok(Some(updated.tokens.access_token));
                    }
                }
            }
            let config = LoginConfig {
                mcp_url: url,
                resource_metadata_url: challenge.resource_metadata.as_deref(),
                requested_scope: existing.as_ref().and_then(|c| c.tokens.scope.as_deref()),
                preconfigured_client_id,
            };
            let creds = login(client, &config, client_info, redirect).await?;
            store.save(&creds)?;
            Ok(Some(creds.tokens.access_token))
        }
        Some(403) if challenge.error.as_deref() == Some("insufficient_scope") => {
            let prior = existing.as_ref().and_then(|c| c.tokens.scope.clone());
            let union = union_scopes(prior.as_deref(), challenge.scope.as_deref());
            let config = LoginConfig {
                mcp_url: url,
                resource_metadata_url: challenge.resource_metadata.as_deref(),
                requested_scope: union.as_deref(),
                preconfigured_client_id,
            };
            let creds = login(client, &config, client_info, redirect).await?;
            store.save(&creds)?;
            Ok(Some(creds.tokens.access_token))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{same_origin, union_scopes};

    #[test]
    fn same_origin_binds_resource_to_the_server() {
        assert!(same_origin(
            "https://mcp.example.com/mcp",
            "https://mcp.example.com"
        ));
        assert!(same_origin(
            "https://mcp.example.com:443/mcp",
            "https://mcp.example.com"
        ));
        // Different host, scheme, or port → not the same origin (mix-up defense).
        assert!(!same_origin(
            "https://mcp.example.com/mcp",
            "https://evil.example.com"
        ));
        assert!(!same_origin(
            "https://mcp.example.com/mcp",
            "http://mcp.example.com"
        ));
        assert!(!same_origin(
            "https://mcp.example.com/mcp",
            "https://mcp.example.com:8443"
        ));
    }

    #[test]
    fn union_scopes_dedupes_and_preserves_order() {
        assert_eq!(
            union_scopes(Some("read"), Some("read write")).as_deref(),
            Some("read write")
        );
        assert_eq!(
            union_scopes(Some("a b"), Some("c b")).as_deref(),
            Some("a b c")
        );
        assert_eq!(union_scopes(None, Some("x")).as_deref(), Some("x"));
        assert_eq!(union_scopes(Some("y"), None).as_deref(), Some("y"));
        assert_eq!(union_scopes(None, None), None);
    }
}
