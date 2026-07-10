//! OAuth 2.1 protocol HTTP: Dynamic Client Registration, the authorization-URL
//! construction, and the token-endpoint exchanges (auth-code, refresh, device).
//!
//! These are the plain form-encoded / JSON requests of RFC 6749 / 7591 / 8628,
//! driven directly with our reqwest client. The `resource` parameter (RFC 8707,
//! the canonical MCP server URI) is a hard MUST on both the authorization and
//! token requests and is threaded through every builder here — `oauth2`'s client
//! won't add it, so we always set it explicitly.

use serde::Deserialize;
use url::Url;

use super::store::ClientInfo;

/// A token-endpoint success response (subset we consume).
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
}

/// Build the authorization-request URL (RFC 6749 §4.1.1 + PKCE + RFC 8707).
/// Pure so it is unit-testable without a browser.
pub fn build_authorize_url(
    authorization_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
    scopes: &[String],
    resource: &str,
) -> Result<String, String> {
    let mut url = Url::parse(authorization_endpoint)
        .map_err(|e| format!("invalid authorization_endpoint: {e}"))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", client_id);
        q.append_pair("redirect_uri", redirect_uri);
        q.append_pair("code_challenge", code_challenge);
        q.append_pair("code_challenge_method", "S256");
        q.append_pair("state", state);
        // RFC 8707: bind the token audience to this MCP server.
        q.append_pair("resource", resource);
        if !scopes.is_empty() {
            q.append_pair("scope", &scopes.join(" "));
        }
    }
    Ok(url.to_string())
}

/// Dynamic Client Registration (RFC 7591): register a public client with the
/// loopback redirect URI and return its issued `client_id`.
pub async fn register_client(
    client: &reqwest::Client,
    registration_endpoint: &str,
    redirect_uri: &str,
    client_name: &str,
) -> Result<ClientInfo, String> {
    let body = serde_json::json!({
        "client_name": client_name,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let resp = client
        .post(registration_endpoint)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("dynamic client registration request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let detail = resp.text().await.unwrap_or_default();
        return Err(format!(
            "dynamic client registration failed: HTTP {status} {}",
            detail.trim()
        ));
    }

    #[derive(Deserialize)]
    struct RegistrationResponse {
        client_id: String,
        #[serde(default)]
        client_secret: Option<String>,
    }
    let reg: RegistrationResponse = resp
        .json()
        .await
        .map_err(|e| format!("dynamic client registration response did not decode: {e}"))?;
    Ok(ClientInfo {
        client_id: reg.client_id,
        client_secret: reg.client_secret,
    })
}

/// Exchange an authorization `code` (+ PKCE verifier) for tokens.
#[allow(clippy::too_many_arguments)]
pub async fn exchange_code(
    client: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
    resource: &str,
) -> Result<TokenResponse, String> {
    let form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", code_verifier),
        ("client_id", client_id),
        ("resource", resource),
    ];
    post_token(client, token_endpoint, form, client_secret).await
}

/// Exchange a refresh token for a fresh access token (and possibly a rotated
/// refresh token, which the caller MUST persist).
pub async fn refresh_tokens(
    client: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
    resource: &str,
) -> Result<TokenResponse, String> {
    let form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
        ("resource", resource),
    ];
    post_token(client, token_endpoint, form, client_secret).await
}

/// POST a form to the token endpoint and decode the response, mapping an OAuth
/// error object (`{"error": "...", "error_description": "..."}`) to an `Err`
/// that preserves the error code so callers can branch (`invalid_grant`, …).
pub async fn post_token(
    client: &reqwest::Client,
    token_endpoint: &str,
    form: Vec<(&str, &str)>,
    client_secret: Option<&str>,
) -> Result<TokenResponse, String> {
    let mut builder = client
        .post(token_endpoint)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded");
    // Confidential clients authenticate with HTTP Basic; public clients send
    // only client_id in the form (already present).
    if let Some(secret) = client_secret {
        let client_id = form
            .iter()
            .find(|(k, _)| *k == "client_id")
            .map(|(_, v)| *v)
            .unwrap_or("");
        builder = builder.basic_auth(client_id, Some(secret));
    }
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form.iter().map(|(k, v)| (*k, *v)))
        .finish();
    let resp = builder
        .body(body)
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("token response body error: {e}"))?;

    if !status.is_success() {
        // Prefer the structured OAuth error code when present.
        #[derive(Deserialize)]
        struct OAuthError {
            error: String,
            #[serde(default)]
            error_description: Option<String>,
        }
        if let Ok(err) = serde_json::from_str::<OAuthError>(&text) {
            return Err(match err.error_description {
                Some(desc) => format!("{}: {desc}", err.error),
                None => err.error,
            });
        }
        // Not OAuth-error-shaped JSON: the body is unknown-shaped and possibly
        // sensitive (a stack trace, an internal error page, …), so it never
        // goes into the error string — only the status and a byte count.
        return Err(format!(
            "token endpoint HTTP {}: unrecognized error body ({} bytes)",
            status.as_u16(),
            text.len()
        ));
    }

    serde_json::from_str::<TokenResponse>(&text)
        .map_err(|e| format!("token response did not decode: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_has_all_required_params() {
        let url = build_authorize_url(
            "https://auth.example.com/authorize",
            "client-123",
            "http://127.0.0.1:5599/callback",
            "CHALLENGE",
            "STATE",
            &["files:read".to_string(), "files:write".to_string()],
            "https://mcp.example.com/mcp",
        )
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(pairs["response_type"], "code");
        assert_eq!(pairs["client_id"], "client-123");
        assert_eq!(pairs["redirect_uri"], "http://127.0.0.1:5599/callback");
        assert_eq!(pairs["code_challenge"], "CHALLENGE");
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["state"], "STATE");
        // RFC 8707 resource MUST be present.
        assert_eq!(pairs["resource"], "https://mcp.example.com/mcp");
        assert_eq!(pairs["scope"], "files:read files:write");
    }

    #[test]
    fn authorize_url_omits_empty_scope() {
        let url = build_authorize_url(
            "https://a/authorize",
            "c",
            "http://127.0.0.1:1/callback",
            "ch",
            "st",
            &[],
            "https://mcp/mcp",
        )
        .unwrap();
        assert!(
            !url.contains("scope="),
            "no scope param when none requested"
        );
    }

    /// A non-OAuth-error-shaped `500` body (an internal error page, a stack
    /// trace, …) must never be embedded verbatim in the returned error string
    /// — only the HTTP status and a byte count. `device.rs::poll_for_token`
    /// pattern-matches `err.starts_with("authorization_pending" | "slow_down")`
    /// against the OAuth-error-shaped branch ABOVE this one; this test only
    /// touches the fallback, so that contract is untouched (proved by the
    /// `sema-mcp` device/flow test suite continuing to pass).
    #[tokio::test]
    async fn post_token_error_redacts_unrecognized_body() {
        let server =
            tiny_http::Server::http("127.0.0.1:0").expect("bind mock token-endpoint listener");
        let port = server
            .server_addr()
            .to_ip()
            .expect("loopback address")
            .port();
        let handler = std::thread::spawn(move || {
            let request = server.recv().expect("receive token request");
            let response = tiny_http::Response::from_string("secret-blob").with_status_code(500);
            let _ = request.respond(response);
        });

        let client = reqwest::Client::new();
        let err = post_token(
            &client,
            &format!("http://127.0.0.1:{port}/token"),
            vec![("grant_type", "authorization_code")],
            None,
        )
        .await
        .expect_err("a non-OAuth-shaped 500 body must still be an error");

        handler.join().expect("mock server thread panicked");

        assert!(err.contains("500"), "status must be preserved: {err}");
        assert!(
            !err.contains("secret-blob"),
            "raw body must never be embedded: {err}"
        );
    }
}
