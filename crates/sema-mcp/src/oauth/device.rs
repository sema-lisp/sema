//! OAuth 2.0 Device Authorization Grant (RFC 8628) — the headless alternative to
//! the loopback browser flow. The user is shown a short `user_code` and a
//! `verification_uri` to visit on any device; meanwhile the client polls the
//! token endpoint until the user approves. No PKCE (there is no redirect to
//! intercept); a refresh token requires the `offline_access` scope on many
//! providers.

use std::time::{Duration, Instant};

use serde::Deserialize;

use super::flow::{self, TokenResponse};
use super::login::{discover, LoginConfig};
use super::store::{now_unix, ClientInfo, StoredCredentials, TokenSet};

pub const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The device-authorization response (RFC 8628 §3.2).
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    #[serde(default = "default_expires_in")]
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_expires_in() -> u64 {
    900
}
fn default_interval() -> u64 {
    5
}

/// Request a device + user code from the device-authorization endpoint.
pub async fn request_device_code(
    client: &reqwest::Client,
    device_endpoint: &str,
    client_id: &str,
    scopes: &[String],
    resource: &str,
) -> Result<DeviceAuthorization, String> {
    let scope = scopes.join(" ");
    let mut form = vec![("client_id", client_id), ("resource", resource)];
    if !scope.is_empty() {
        form.push(("scope", scope.as_str()));
    }
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form.iter().map(|(k, v)| (*k, *v)))
        .finish();
    let resp = client
        .post(device_endpoint)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("device authorization request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let detail = resp.text().await.unwrap_or_default();
        return Err(format!(
            "device authorization failed: HTTP {status} {}",
            detail.trim()
        ));
    }
    resp.json::<DeviceAuthorization>()
        .await
        .map_err(|e| format!("device authorization response did not decode: {e}"))
}

/// Poll the token endpoint until the user approves (or the code expires),
/// honoring `authorization_pending` / `slow_down` per RFC 8628 §3.5.
pub async fn poll_for_token(
    client: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    device_code: &str,
    resource: &str,
    mut interval: u64,
    expires_in: u64,
) -> Result<TokenResponse, String> {
    let deadline = Instant::now() + Duration::from_secs(expires_in);
    loop {
        let form = vec![
            ("grant_type", DEVICE_CODE_GRANT),
            ("device_code", device_code),
            ("client_id", client_id),
            ("resource", resource),
        ];
        match flow::post_token(client, token_endpoint, form, None).await {
            Ok(token) => return Ok(token),
            Err(err) if err.starts_with("authorization_pending") => {}
            Err(err) if err.starts_with("slow_down") => interval += 5,
            Err(err) => return Err(err),
        }
        if Instant::now() >= deadline {
            return Err("device authorization expired before it was approved".to_string());
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

/// Full headless login via the device grant. `display` shows the user the code +
/// verification URL (RFC 8628 §3.3).
pub async fn device_login(
    client: &reqwest::Client,
    config: &LoginConfig<'_>,
    existing_client_info: Option<ClientInfo>,
    display: &dyn Fn(&DeviceAuthorization),
) -> Result<StoredCredentials, String> {
    let discovered = discover(client, config).await?;
    let device_endpoint = discovered
        .authorization_server
        .device_authorization_endpoint
        .clone()
        .ok_or_else(|| {
            "authorization server does not advertise a device authorization endpoint".to_string()
        })?;

    let client_info = match (config.preconfigured_client_id, existing_client_info) {
        (Some(id), _) => ClientInfo {
            client_id: id.to_string(),
            client_secret: None,
        },
        (None, Some(info)) => info,
        (None, None) => {
            let reg = discovered
                .authorization_server
                .registration_endpoint
                .as_deref()
                .ok_or_else(|| {
                    "no client id and no dynamic registration endpoint; pass :auth {:client-id …}"
                        .to_string()
                })?;
            flow::register_client(client, reg, "http://127.0.0.1/callback", "Sema MCP client")
                .await?
        }
    };

    let scopes: Vec<String> = config
        .requested_scope
        .map(|s| s.split_whitespace().map(String::from).collect::<Vec<_>>())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| discovered.protected_resource.scopes_supported.clone());

    let device = request_device_code(
        client,
        &device_endpoint,
        &client_info.client_id,
        &scopes,
        &discovered.resource,
    )
    .await?;
    display(&device);

    let token = poll_for_token(
        client,
        &discovered.authorization_server.token_endpoint,
        &client_info.client_id,
        &device.device_code,
        &discovered.resource,
        device.interval,
        device.expires_in,
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
