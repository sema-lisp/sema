//! CLI entry points for MCP client authentication (`sema mcp login/logout`).
//!
//! `login` runs the OAuth flow eagerly (before any `mcp/connect`) so a token is
//! cached and later connects are silent; `logout` clears the stored credentials
//! for a server. Both use the default credential store (keychain or `0600` file).

use std::time::Duration;

use crate::oauth;

const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to create runtime: {e}"))
}

/// Authenticate to a remote MCP server and cache the resulting token. Uses the
/// browser loopback flow by default; `use_device` selects the RFC 8628 device
/// flow for headless boxes.
pub fn mcp_login(url: &str, use_device: bool, client_id: Option<&str>) -> Result<(), String> {
    let rt = runtime()?;
    rt.block_on(async {
        let http = reqwest::Client::new();
        let store = oauth::store::default_store();
        let config = oauth::login::LoginConfig {
            mcp_url: url,
            resource_metadata_url: None,
            requested_scope: None,
            preconfigured_client_id: client_id,
        };
        let existing = store.load(url).and_then(|c| c.client_info);

        let creds = if use_device {
            oauth::device::device_login(&http, &config, existing, &|device| {
                eprintln!(
                    "\nTo authorize, visit:\n  {}\nand enter the code: {}\n",
                    device.verification_uri, device.user_code
                );
                if let Some(complete) = &device.verification_uri_complete {
                    eprintln!("(or open directly: {complete})\n");
                }
                eprintln!("Waiting for approval…");
            })
            .await?
        } else {
            let driver = oauth::loopback::LoopbackDriver::new(LOGIN_TIMEOUT)?;
            eprintln!("Opening your browser to authorize {url} …");
            oauth::login::login(&http, &config, existing, &driver).await?
        };

        store.save(&creds)?;
        eprintln!("Authenticated to {url}. Token cached.");
        Ok(())
    })
}

/// Store a pre-issued access token directly, skipping discovery/DCR/OAuth
/// entirely — the headless/CI escape hatch (plan §5: "accepts a … pre-issued
/// token"). `expires_in` (seconds, relative to now) becomes an absolute
/// `expires_at`; `None` stores a non-expiring token. Never echoes `token`.
pub fn mcp_login_token(url: &str, token: &str, expires_in: Option<u64>) -> Result<(), String> {
    let creds = oauth::store::StoredCredentials {
        server_url: url.to_string(),
        tokens: oauth::store::TokenSet::from_response(
            token.to_string(),
            None,
            expires_in,
            None,
            oauth::store::now_unix(),
        ),
        client_info: None,
    };
    oauth::store::default_store().save(&creds)?;
    eprintln!("Authenticated to {url}. Token cached.");
    Ok(())
}

/// Remove any cached credentials for a remote MCP server.
pub fn mcp_logout(url: &str) -> Result<(), String> {
    oauth::store::default_store().delete(url)?;
    eprintln!("Cleared cached credentials for {url}.");
    Ok(())
}
