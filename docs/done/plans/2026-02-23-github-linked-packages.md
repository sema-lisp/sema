# GitHub-Linked Packages — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Allow users to link a GitHub repository to the Sema package registry, automatically importing semver tags as package versions and receiving webhook notifications for new releases.

**Architecture:** Extends the existing sema-pkg Axum server. GitHub OAuth tokens are stored encrypted in a new `oauth_connections` table. Linked packages auto-import by fetching tarballs from GitHub's archive API per semver tag. A webhook endpoint receives push events to auto-publish new tags. Packages are locked to one source (`upload` or `github`) at creation time.

**Tech Stack:** Rust, Axum 0.8, SQLx (SQLite), reqwest, hmac+sha2 (webhook verification), aes-gcm (token encryption).

---

### Task 1: Database migration for GitHub-linked packages

**Files:**
- Create: `pkg/migrations/002_github_packages.sql`

**Step 1: Write the migration**

```sql
-- OAuth connections (stores encrypted GitHub access tokens)
CREATE TABLE IF NOT EXISTS oauth_connections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    provider TEXT NOT NULL DEFAULT 'github',
    provider_user_id TEXT NOT NULL,
    provider_login TEXT,
    access_token_enc BLOB NOT NULL,
    scopes TEXT,
    revoked_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_oauth_provider_user
ON oauth_connections(provider, provider_user_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_oauth_user_provider
ON oauth_connections(user_id, provider);

-- Add source tracking to packages
ALTER TABLE packages ADD COLUMN source TEXT NOT NULL DEFAULT 'upload';
ALTER TABLE packages ADD COLUMN github_repo TEXT;
ALTER TABLE packages ADD COLUMN webhook_secret TEXT;

-- Sync log for GitHub-linked packages
CREATE TABLE IF NOT EXISTS github_sync_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    package_id INTEGER NOT NULL REFERENCES packages(id),
    tag TEXT NOT NULL,
    status TEXT NOT NULL,
    error TEXT,
    synced_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_sync_log_package ON github_sync_log(package_id);
```

**Step 2: Verify migration runs**

Run: `cd pkg && cargo build`
Expected: compiles (sqlx migrate macro picks up the new file)

Run: `cd pkg && DATABASE_URL=sqlite://data/test-migrate.db?mode=rwc cargo run &` then kill it
Expected: server starts, migration runs without error

**Step 3: Commit**

```bash
git add pkg/migrations/002_github_packages.sql
git commit -m "feat(pkg): add migration for GitHub-linked packages and oauth connections"
```

---

### Task 2: Token encryption utilities

**Files:**
- Create: `pkg/src/crypto.rs`
- Modify: `pkg/Cargo.toml` — add `aes-gcm = "0.10"` dependency
- Modify: `pkg/src/lib.rs` — add `pub mod crypto;`
- Modify: `pkg/src/config.rs` — add `oauth_token_key` field

**Step 1: Add dependency**

Add to `[dependencies]` in `pkg/Cargo.toml`:

```toml
aes-gcm = "0.10"
```

**Step 2: Add config field**

Add to `Config` struct in `pkg/src/config.rs`:

```rust
pub oauth_token_key: String,
```

And in `from_env()`:

```rust
oauth_token_key: env::var("OAUTH_TOKEN_KEY")
    .unwrap_or_else(|_| "change-me-32-bytes-in-production!".into()),
```

**Step 3: Create crypto.rs**

Symmetric encrypt/decrypt using AES-256-GCM with the config key. The key is derived by SHA-256 hashing the config string to get exactly 32 bytes:

```rust
use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, AeadCore,
};
use sha2::{Digest, Sha256};

fn derive_key(secret: &str) -> [u8; 32] {
    let hash = Sha256::digest(secret.as_bytes());
    hash.into()
}

pub fn encrypt(plaintext: &str, secret: &str) -> Vec<u8> {
    let key = derive_key(secret);
    let cipher = Aes256Gcm::new_from_slice(&key).expect("invalid key length");
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher.encrypt(&nonce, plaintext.as_bytes()).expect("encryption failed");
    // Prepend nonce (12 bytes) to ciphertext
    let mut result = nonce.to_vec();
    result.extend_from_slice(&ciphertext);
    result
}

pub fn decrypt(data: &[u8], secret: &str) -> Option<String> {
    if data.len() < 12 {
        return None;
    }
    let key = derive_key(secret);
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let nonce = aes_gcm::Nonce::from_slice(&data[..12]);
    let plaintext = cipher.decrypt(nonce, &data[12..]).ok()?;
    String::from_utf8(plaintext).ok()
}
```

**Step 4: Add module to lib.rs**

Add `pub mod crypto;` to `pkg/src/lib.rs`.

**Step 5: Verify it compiles**

Run: `cd pkg && cargo build`
Expected: compiles without errors

**Step 6: Commit**

```bash
git add pkg/src/crypto.rs pkg/Cargo.toml pkg/src/lib.rs pkg/src/config.rs
git commit -m "feat(pkg): add AES-256-GCM encryption for OAuth token storage"
```

---

### Task 3: GitHub OAuth "connect" mode

**Files:**
- Modify: `pkg/src/github.rs` — add `mode=connect` support, store access token

**Step 1: Update the OAuth start handler**

The `start` handler needs to accept an optional `mode` and `return_to` query parameter. Store them in the OAuth state cookie alongside the CSRF state string. When `mode=connect`, the user must already be logged in.

Add to the `start` function:
- Read `mode` query param (default: `login`)
- Read `return_to` query param (default: `/account`)
- Encode `state|mode|return_to` into the cookie value

Update the OAuth scope to include `admin:repo_hook,read:user,user:email` (needed for webhook registration):

```rust
let scopes = "read:user,user:email,public_repo,admin:repo_hook";
let url = format!(
    "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}/auth/github/callback&scope={}&state={}",
    client_id, state.config.base_url, scopes, oauth_state,
);
```

The cookie value encodes all three pieces:

```rust
let cookie_value = format!("{}|{}|{}", oauth_state, mode, return_to);
let cookie = format!(
    "github_oauth_state={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600",
    cookie_value
);
```

**Step 2: Update the callback handler**

Parse the cookie value to extract `state`, `mode`, and `return_to`:

```rust
let parts: Vec<&str> = stored_cookie.splitn(3, '|').collect();
let stored_state = parts.get(0).unwrap_or(&"");
let mode = parts.get(1).unwrap_or(&"login");
let return_to = parts.get(2).unwrap_or(&"/account");
```

After obtaining the access token from GitHub, branch on `mode`:

- **`mode=login`** (existing behavior): find or create user, create session, redirect to `/account`
- **`mode=connect`**: require existing session cookie → resolve current user → store token in `oauth_connections` table → set `users.github_id` if not already set → redirect to `return_to`

For the connect branch:

```rust
if *mode == "connect" {
    // Must have a valid session
    let session_cookie = cookie_header.split(';')
        .filter_map(|c| c.trim().strip_prefix("session="))
        .next()
        .unwrap_or("");
    let current_user = get_session_user(&state.db, session_cookie).await;
    let current_user = match current_user {
        Some(u) => u,
        None => return (StatusCode::UNAUTHORIZED, "Must be logged in to connect GitHub").into_response(),
    };

    // Check if this github_id is already linked to a different user
    let existing = sqlx::query("SELECT user_id FROM oauth_connections WHERE provider = 'github' AND provider_user_id = ?")
        .bind(gh_user.id.to_string())
        .fetch_optional(&state.db).await.ok().flatten();
    if let Some(row) = existing {
        let linked_user_id: i64 = row.get("user_id");
        if linked_user_id != current_user.id {
            return (StatusCode::CONFLICT, "This GitHub account is linked to another user").into_response();
        }
    }

    // Encrypt and store token
    let token_enc = crate::crypto::encrypt(&token_body.access_token, &state.config.oauth_token_key);

    // Upsert oauth_connections
    sqlx::query(
        "INSERT INTO oauth_connections (user_id, provider, provider_user_id, provider_login, access_token_enc, scopes, updated_at)
         VALUES (?, 'github', ?, ?, ?, ?, datetime('now'))
         ON CONFLICT(user_id, provider) DO UPDATE SET
           provider_user_id = excluded.provider_user_id,
           provider_login = excluded.provider_login,
           access_token_enc = excluded.access_token_enc,
           scopes = excluded.scopes,
           revoked_at = NULL,
           updated_at = datetime('now')"
    )
    .bind(current_user.id)
    .bind(gh_user.id.to_string())
    .bind(&gh_user.login)
    .bind(&token_enc)
    .bind("read:user,user:email,public_repo,admin:repo_hook")
    .execute(&state.db).await.ok();

    // Also set github_id on users table if not set
    sqlx::query("UPDATE users SET github_id = ? WHERE id = ? AND github_id IS NULL")
        .bind(gh_user.id)
        .bind(current_user.id)
        .execute(&state.db).await.ok();

    // Redirect back
    let clear_state = "github_oauth_state=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0".to_string();
    return (
        AppendHeaders([(header::SET_COOKIE, clear_state)]),
        Redirect::to(return_to),
    ).into_response();
}
```

Also update the `login` mode to store the token in `oauth_connections` (so users who login via GitHub also get their token stored for linking later):

After `find_or_create_user`, store the token:

```rust
let token_enc = crate::crypto::encrypt(&token_body.access_token, &state.config.oauth_token_key);
sqlx::query(
    "INSERT INTO oauth_connections (user_id, provider, provider_user_id, provider_login, access_token_enc, scopes, updated_at)
     VALUES (?, 'github', ?, ?, ?, ?, datetime('now'))
     ON CONFLICT(user_id, provider) DO UPDATE SET
       access_token_enc = excluded.access_token_enc,
       revoked_at = NULL,
       updated_at = datetime('now')"
)
.bind(user_id)
.bind(gh_user.id.to_string())
.bind(&gh_user.login)
.bind(&token_enc)
.bind("read:user,user:email,public_repo,admin:repo_hook")
.execute(&state.db).await.ok();
```

**Step 3: Add query param structs**

```rust
#[derive(Deserialize)]
pub struct StartParams {
    #[serde(default = "default_login")]
    pub mode: String,
    #[serde(default = "default_account")]
    pub return_to: String,
}

fn default_login() -> String { "login".into() }
fn default_account() -> String { "/account".into() }
```

Update `start` signature: `Query(params): Query<StartParams>`

**Step 4: Verify it compiles**

Run: `cd pkg && cargo build`
Expected: compiles without errors

**Step 5: Commit**

```bash
git add pkg/src/github.rs
git commit -m "feat(pkg): GitHub OAuth connect mode with encrypted token storage"
```

---

### Task 4: GitHub sync logic (core module)

**Files:**
- Create: `pkg/src/github_sync.rs`
- Modify: `pkg/src/lib.rs` — add `pub mod github_sync;`

This module contains the shared sync logic used by both the link endpoint (initial import) and the webhook endpoint (incremental updates). It does NOT define Axum handlers — those go in separate files.

**Step 1: Create github_sync.rs**

```rust
use crate::{blob, crypto, db::Db};
use sqlx::Row;

/// Fetch the decrypted GitHub access token for a user.
pub async fn get_github_token(db: &Db, user_id: i64, token_key: &str) -> Option<String> {
    let row = sqlx::query(
        "SELECT access_token_enc FROM oauth_connections WHERE user_id = ? AND provider = 'github' AND revoked_at IS NULL"
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .ok()??;

    let enc: Vec<u8> = row.get("access_token_enc");
    crypto::decrypt(&enc, token_key)
}

/// Mark a user's GitHub connection as revoked (e.g. after a 401).
pub async fn mark_token_revoked(db: &Db, user_id: i64) {
    let _ = sqlx::query(
        "UPDATE oauth_connections SET revoked_at = datetime('now') WHERE user_id = ? AND provider = 'github'"
    )
    .bind(user_id)
    .execute(db)
    .await;
}

/// Validate that a GitHub repo exists and contains sema.toml. Returns the parsed sema.toml content.
pub async fn validate_repo(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
) -> Result<RepoManifest, String> {
    // Check repo exists
    let resp = client
        .get(format!("https://api.github.com/repos/{owner}/{repo}"))
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "sema-pkg")
        .send()
        .await
        .map_err(|e| format!("Failed to reach GitHub: {e}"))?;

    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("GitHub token is invalid or revoked".into());
    }
    if !resp.status().is_success() {
        return Err(format!("Repository {owner}/{repo} not found or not accessible"));
    }

    // Fetch sema.toml from default branch
    let toml_resp = client
        .get(format!("https://api.github.com/repos/{owner}/{repo}/contents/sema.toml"))
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "sema-pkg")
        .header("Accept", "application/vnd.github.raw+json")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch sema.toml: {e}"))?;

    if !toml_resp.status().is_success() {
        return Err(format!("No sema.toml found in {owner}/{repo}"));
    }

    let toml_content = toml_resp.text().await.map_err(|e| format!("Failed to read sema.toml: {e}"))?;
    parse_manifest(&toml_content)
}

#[derive(Debug, Clone)]
pub struct RepoManifest {
    pub name: String,
    pub description: String,
    pub repository_url: Option<String>,
    pub sema_version_req: Option<String>,
}

fn parse_manifest(content: &str) -> Result<RepoManifest, String> {
    let doc: toml::Value = toml::from_str(content).map_err(|e| format!("Invalid sema.toml: {e}"))?;
    let pkg = doc.get("package").ok_or("sema.toml missing [package] section")?;
    let name = pkg.get("name").and_then(|v| v.as_str())
        .ok_or("sema.toml [package] missing 'name'")?;
    let description = pkg.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let repository_url = pkg.get("repository").and_then(|v| v.as_str()).map(|s| s.to_string());
    let sema_version_req = pkg.get("sema_version_req").and_then(|v| v.as_str()).map(|s| s.to_string());
    Ok(RepoManifest { name: name.to_string(), description, repository_url, sema_version_req })
}

/// List semver tags from a GitHub repo. Strips leading 'v' prefix.
/// Returns (tag_name, semver_version) pairs sorted newest-first.
pub async fn list_semver_tags(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
) -> Result<Vec<(String, semver::Version)>, String> {
    let mut tags = Vec::new();
    let mut page = 1u32;

    loop {
        let resp = client
            .get(format!("https://api.github.com/repos/{owner}/{repo}/tags?per_page=100&page={page}"))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", "sema-pkg")
            .send()
            .await
            .map_err(|e| format!("Failed to list tags: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Failed to list tags ({})", resp.status()));
        }

        let items: Vec<serde_json::Value> = resp.json().await.map_err(|e| format!("Invalid response: {e}"))?;
        if items.is_empty() {
            break;
        }

        for item in &items {
            if let Some(tag_name) = item.get("name").and_then(|n| n.as_str()) {
                let version_str = tag_name.strip_prefix('v').unwrap_or(tag_name);
                if let Ok(ver) = semver::Version::parse(version_str) {
                    tags.push((tag_name.to_string(), ver));
                }
            }
        }

        if items.len() < 100 {
            break;
        }
        page += 1;
    }

    // Sort newest first
    tags.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(tags)
}

/// Sync a single tag: download tarball from GitHub, store as blob, create version record.
/// Returns Ok(true) if version was created, Ok(false) if it already existed.
pub async fn sync_tag(
    db: &Db,
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    tag_name: &str,
    version: &semver::Version,
    package_id: i64,
    blob_dir: &str,
    sema_version_req: Option<&str>,
) -> Result<bool, String> {
    let version_str = version.to_string();

    // Check if version already exists
    let exists = sqlx::query(
        "SELECT COUNT(*) as cnt FROM package_versions WHERE package_id = ? AND version = ?"
    )
    .bind(package_id)
    .bind(&version_str)
    .fetch_one(db)
    .await
    .ok()
    .map(|r| r.get::<i32, _>("cnt"))
    .unwrap_or(0);

    if exists > 0 {
        return Ok(false);
    }

    // Download tarball
    let tarball_url = format!("https://api.github.com/repos/{owner}/{repo}/tarball/{tag_name}");
    let resp = client
        .get(&tarball_url)
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "sema-pkg")
        .send()
        .await
        .map_err(|e| format!("Failed to download tarball for {tag_name}: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Failed to download tarball for {tag_name} ({})", resp.status()));
    }

    let tarball = resp.bytes().await.map_err(|e| format!("Failed to read tarball: {e}"))?;

    // Store blob
    let (blob_key, checksum, size) = blob::store(blob_dir, &tarball).await;

    // Insert version
    sqlx::query(
        "INSERT INTO package_versions (package_id, version, checksum_sha256, blob_key, size_bytes, sema_version_req) VALUES (?, ?, ?, ?, ?, ?)"
    )
    .bind(package_id)
    .bind(&version_str)
    .bind(&checksum)
    .bind(&blob_key)
    .bind(size as i64)
    .bind(sema_version_req)
    .execute(db)
    .await
    .map_err(|e| format!("Failed to insert version: {e}"))?;

    // Log sync
    sqlx::query(
        "INSERT INTO github_sync_log (package_id, tag, status) VALUES (?, ?, 'ok')"
    )
    .bind(package_id)
    .bind(tag_name)
    .execute(db)
    .await
    .ok();

    Ok(true)
}

/// Register a webhook on a GitHub repository.
pub async fn register_webhook(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    webhook_url: &str,
    webhook_secret: &str,
) -> Result<(), String> {
    let body = serde_json::json!({
        "name": "web",
        "active": true,
        "events": ["push"],
        "config": {
            "url": webhook_url,
            "content_type": "json",
            "secret": webhook_secret,
            "insecure_ssl": "0"
        }
    });

    let resp = client
        .post(format!("https://api.github.com/repos/{owner}/{repo}/hooks"))
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "sema-pkg")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Failed to register webhook: {e}"))?;

    if resp.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
        // Webhook may already exist — not an error
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let errors = body.get("errors").and_then(|e| e.as_array());
        if let Some(errors) = errors {
            let already_exists = errors.iter().any(|e| {
                e.get("message").and_then(|m| m.as_str()).map(|m| m.contains("already exists")).unwrap_or(false)
            });
            if already_exists {
                return Ok(());
            }
        }
        return Err(format!("Failed to register webhook: {}", body));
    }

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Failed to register webhook ({status})"));
    }

    Ok(())
}

/// Parse an "owner/repo" string from a GitHub URL.
/// Accepts: "github.com/owner/repo", "https://github.com/owner/repo", "https://github.com/owner/repo.git", "owner/repo"
pub fn parse_github_url(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let url = url.strip_suffix(".git").unwrap_or(url);
    let url = url.strip_prefix("https://").unwrap_or(url);
    let url = url.strip_prefix("http://").unwrap_or(url);
    let url = url.strip_prefix("github.com/").unwrap_or(url);

    let parts: Vec<&str> = url.splitn(3, '/').collect();
    if parts.len() >= 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

/// Generate a random webhook secret.
pub fn generate_webhook_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
```

**Step 2: Add toml dependency to Cargo.toml**

Add to `[dependencies]` in `pkg/Cargo.toml`:

```toml
toml = "0.8"
hmac = "0.12"
```

**Step 3: Add module to lib.rs**

Add `pub mod github_sync;` to `pkg/src/lib.rs`.

**Step 4: Verify it compiles**

Run: `cd pkg && cargo build`
Expected: compiles without errors

**Step 5: Commit**

```bash
git add pkg/src/github_sync.rs pkg/src/lib.rs pkg/Cargo.toml
git commit -m "feat(pkg): add GitHub sync core logic (tag import, webhook registration, URL parsing)"
```

---

### Task 5: Link package API endpoint

**Files:**
- Create: `pkg/src/api/github.rs`
- Modify: `pkg/src/api/mod.rs` — add `pub mod github;`
- Modify: `pkg/src/lib.rs` — add route

**Step 1: Create api/github.rs**

`POST /api/v1/packages/link` — requires session cookie (not Bearer token, since this is a web action). Accepts JSON `{ "repository_url": "github.com/owner/repo" }`.

```rust
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;

use crate::{auth::AuthUser, github_sync, AppState};

#[derive(Deserialize)]
pub struct LinkRequest {
    pub repository_url: String,
}

pub async fn link(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Json(body): Json<LinkRequest>,
) -> impl IntoResponse {
    // Parse the GitHub URL
    let (owner, repo) = match github_sync::parse_github_url(&body.repository_url) {
        Some(pair) => pair,
        None => return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid GitHub URL. Expected format: github.com/owner/repo"})),
        ).into_response(),
    };

    // Get the user's GitHub token
    let token = match github_sync::get_github_token(&state.db, user.id, &state.config.oauth_token_key).await {
        Some(t) => t,
        None => return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "GitHub not connected",
                "connect_url": format!("/auth/github?mode=connect&return_to=/account")
            })),
        ).into_response(),
    };

    let client = reqwest::Client::new();

    // Validate repo exists and has sema.toml
    let manifest = match github_sync::validate_repo(&client, &token, &owner, &repo).await {
        Ok(m) => m,
        Err(e) => {
            if e.contains("invalid or revoked") {
                github_sync::mark_token_revoked(&state.db, user.id).await;
            }
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e}))).into_response();
        }
    };

    // Check if package name is already taken
    let existing = sqlx::query("SELECT id, source FROM packages WHERE name = ?")
        .bind(&manifest.name)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    if let Some(row) = existing {
        let source: String = row.get("source");
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!("Package '{}' already exists (source: {})", manifest.name, source)
            })),
        ).into_response();
    }

    // Generate webhook secret
    let webhook_secret = github_sync::generate_webhook_secret();
    let github_repo = format!("{owner}/{repo}");

    // Create package with source=github
    let pkg_result = sqlx::query(
        "INSERT INTO packages (name, description, repository_url, source, github_repo, webhook_secret) VALUES (?, ?, ?, 'github', ?, ?)"
    )
    .bind(&manifest.name)
    .bind(&manifest.description)
    .bind(format!("https://github.com/{github_repo}"))
    .bind(&github_repo)
    .bind(&webhook_secret)
    .execute(&state.db)
    .await;

    let package_id = match pkg_result {
        Ok(r) => r.last_insert_rowid(),
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to create package: {e}")})),
        ).into_response(),
    };

    // Add user as owner
    let _ = sqlx::query("INSERT INTO owners (package_id, user_id) VALUES (?, ?)")
        .bind(package_id)
        .bind(user.id)
        .execute(&state.db)
        .await;

    // Register webhook
    let webhook_url = format!("{}/api/v1/webhooks/github", state.config.base_url);
    if let Err(e) = github_sync::register_webhook(&client, &token, &owner, &repo, &webhook_url, &webhook_secret).await {
        tracing::warn!("Failed to register webhook for {github_repo}: {e}");
        // Non-fatal: package is still created, can use manual sync
    }

    // Import existing semver tags
    let tags = github_sync::list_semver_tags(&client, &token, &owner, &repo).await.unwrap_or_default();
    let mut imported = 0u32;
    let mut errors = Vec::new();

    for (tag_name, version) in &tags {
        match github_sync::sync_tag(
            &state.db, &client, &token, &owner, &repo,
            tag_name, version, package_id, &state.config.blob_dir,
            manifest.sema_version_req.as_deref(),
        ).await {
            Ok(true) => imported += 1,
            Ok(false) => {} // already existed
            Err(e) => {
                // Log error in sync log
                let _ = sqlx::query(
                    "INSERT INTO github_sync_log (package_id, tag, status, error) VALUES (?, ?, 'error', ?)"
                )
                .bind(package_id)
                .bind(tag_name)
                .bind(&e)
                .execute(&state.db)
                .await;
                errors.push(format!("{tag_name}: {e}"));
            }
        }
    }

    (StatusCode::CREATED, Json(serde_json::json!({
        "ok": true,
        "package": manifest.name,
        "source": "github",
        "github_repo": github_repo,
        "tags_found": tags.len(),
        "versions_imported": imported,
        "errors": errors,
    }))).into_response()
}
```

**Step 2: Add manual sync endpoint**

In the same `pkg/src/api/github.rs`:

```rust
pub async fn sync(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl IntoResponse {
    // Find the package and verify ownership
    let pkg_row = sqlx::query(
        "SELECT p.id, p.source, p.github_repo, p.sema_version_req FROM packages p JOIN owners o ON o.package_id = p.id WHERE p.name = ? AND o.user_id = ?"
    )
    .bind(&name)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let pkg_row = match pkg_row {
        Some(r) => r,
        None => return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Package not found or you are not an owner"})),
        ).into_response(),
    };

    let source: String = pkg_row.get("source");
    if source != "github" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Package is not GitHub-linked"})),
        ).into_response();
    }

    let package_id: i64 = pkg_row.get("id");
    let github_repo: String = pkg_row.get("github_repo");

    let (owner, repo) = match github_sync::parse_github_url(&github_repo) {
        Some(pair) => pair,
        None => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Invalid github_repo in database"})),
        ).into_response(),
    };

    let token = match github_sync::get_github_token(&state.db, user.id, &state.config.oauth_token_key).await {
        Some(t) => t,
        None => return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "GitHub not connected. Reconnect at /auth/github?mode=connect"})),
        ).into_response(),
    };

    let client = reqwest::Client::new();
    let tags = match github_sync::list_semver_tags(&client, &token, &owner, &repo).await {
        Ok(t) => t,
        Err(e) => {
            if e.contains("invalid or revoked") || e.contains("401") {
                github_sync::mark_token_revoked(&state.db, user.id).await;
            }
            return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": e}))).into_response();
        }
    };

    let mut imported = 0u32;
    for (tag_name, version) in &tags {
        match github_sync::sync_tag(
            &state.db, &client, &token, &owner, &repo,
            tag_name, version, package_id, &state.config.blob_dir, None,
        ).await {
            Ok(true) => imported += 1,
            Ok(false) => {}
            Err(e) => {
                let _ = sqlx::query(
                    "INSERT INTO github_sync_log (package_id, tag, status, error) VALUES (?, ?, 'error', ?)"
                )
                .bind(package_id).bind(tag_name).bind(&e)
                .execute(&state.db).await;
            }
        }
    }

    Json(serde_json::json!({
        "ok": true,
        "tags_found": tags.len(),
        "versions_imported": imported,
    })).into_response()
}
```

**Step 3: Update api/mod.rs**

Add `pub mod github;` to `pkg/src/api/mod.rs`.

**Step 4: Wire routes in lib.rs**

Add to `build_router` in `pkg/src/lib.rs`:

```rust
.route("/api/v1/packages/link", post(api::github::link))
.route("/api/v1/packages/{name}/sync", post(api::github::sync))
```

**Step 5: Verify it compiles**

Run: `cd pkg && cargo build`
Expected: compiles without errors

**Step 6: Commit**

```bash
git add pkg/src/api/github.rs pkg/src/api/mod.rs pkg/src/lib.rs
git commit -m "feat(pkg): add link and sync API endpoints for GitHub-linked packages"
```

---

### Task 6: Webhook endpoint

**Files:**
- Modify: `pkg/src/api/github.rs` — add webhook handler
- Modify: `pkg/src/lib.rs` — add route

**Step 1: Add webhook handler**

`POST /api/v1/webhooks/github` — no auth required (uses HMAC signature verification instead).

Add to `pkg/src/api/github.rs`:

```rust
use axum::body::Bytes;
use hmac::{Hmac, Mac};

type HmacSha256 = Hmac<sha2::Sha256>;

pub async fn webhook(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // Get the signature header
    let signature = match headers.get("x-hub-signature-256").and_then(|v| v.to_str().ok()) {
        Some(s) => s.to_string(),
        None => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Missing signature"}))).into_response(),
    };

    // Parse the event type
    let event = headers.get("x-github-event").and_then(|v| v.to_str().ok()).unwrap_or("");
    if event == "ping" {
        return Json(serde_json::json!({"ok": true, "event": "ping"})).into_response();
    }
    if event != "push" {
        return Json(serde_json::json!({"ok": true, "event": event, "skipped": true})).into_response();
    }

    // Parse the push payload to get the ref and repo
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid JSON"}))).into_response(),
    };

    let git_ref = payload.get("ref").and_then(|r| r.as_str()).unwrap_or("");
    // Only process tag pushes
    let tag_name = match git_ref.strip_prefix("refs/tags/") {
        Some(t) => t,
        None => return Json(serde_json::json!({"ok": true, "skipped": "not a tag push"})).into_response(),
    };

    // Parse as semver (strip v prefix)
    let version_str = tag_name.strip_prefix('v').unwrap_or(tag_name);
    let version = match semver::Version::parse(version_str) {
        Ok(v) => v,
        Err(_) => return Json(serde_json::json!({"ok": true, "skipped": "not a semver tag"})).into_response(),
    };

    // Get the repo full_name from the payload
    let repo_full_name = payload.get("repository")
        .and_then(|r| r.get("full_name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");

    if repo_full_name.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Missing repository info"}))).into_response();
    }

    // Find the package by github_repo
    let pkg_row = sqlx::query(
        "SELECT id, github_repo, webhook_secret FROM packages WHERE github_repo = ? AND source = 'github'"
    )
    .bind(repo_full_name)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let pkg_row = match pkg_row {
        Some(r) => r,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "No linked package for this repo"}))).into_response(),
    };

    let package_id: i64 = pkg_row.get("id");
    let webhook_secret: String = pkg_row.get("webhook_secret");

    // Verify HMAC signature
    let expected_sig = format!("sha256={}", compute_hmac(&webhook_secret, &body));
    if !constant_time_eq(signature.as_bytes(), expected_sig.as_bytes()) {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": "Invalid signature"}))).into_response();
    }

    // Get the owner's GitHub token (use first owner)
    let owner_row = sqlx::query("SELECT user_id FROM owners WHERE package_id = ? LIMIT 1")
        .bind(package_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

    let owner_user_id: i64 = match owner_row {
        Some(r) => r.get("user_id"),
        None => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "No owner found"}))).into_response(),
    };

    let token = match github_sync::get_github_token(&state.db, owner_user_id, &state.config.oauth_token_key).await {
        Some(t) => t,
        None => {
            let _ = sqlx::query(
                "INSERT INTO github_sync_log (package_id, tag, status, error) VALUES (?, ?, 'error', 'Owner GitHub token missing or revoked')"
            ).bind(package_id).bind(tag_name).execute(&state.db).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "Owner GitHub token not available"}))).into_response();
        }
    };

    let (owner, repo) = match github_sync::parse_github_url(repo_full_name) {
        Some(pair) => pair,
        None => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid repo name"}))).into_response(),
    };

    let client = reqwest::Client::new();
    match github_sync::sync_tag(
        &state.db, &client, &token, &owner, &repo,
        tag_name, &version, package_id, &state.config.blob_dir, None,
    ).await {
        Ok(true) => {
            tracing::info!("Webhook: synced {repo_full_name} tag {tag_name} as {version}");
            Json(serde_json::json!({"ok": true, "version": version.to_string(), "imported": true})).into_response()
        }
        Ok(false) => {
            Json(serde_json::json!({"ok": true, "version": version.to_string(), "imported": false, "reason": "already exists"})).into_response()
        }
        Err(e) => {
            let _ = sqlx::query(
                "INSERT INTO github_sync_log (package_id, tag, status, error) VALUES (?, ?, 'error', ?)"
            ).bind(package_id).bind(tag_name).bind(&e).execute(&state.db).await;
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response()
        }
    }
}

fn compute_hmac(secret: &str, data: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key size");
    mac.update(data);
    hex::encode(mac.finalize().into_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
```

**Step 2: Wire route in lib.rs**

Add to `build_router`:

```rust
.route("/api/v1/webhooks/github", post(api::github::webhook))
```

**Step 3: Verify it compiles**

Run: `cd pkg && cargo build`
Expected: compiles without errors

**Step 4: Commit**

```bash
git add pkg/src/api/github.rs pkg/src/lib.rs
git commit -m "feat(pkg): add GitHub webhook endpoint for auto-publishing tags"
```

---

### Task 7: Enforce source locking on publish

**Files:**
- Modify: `pkg/src/api/packages.rs` — check `source` column before allowing CLI publish

**Step 1: Add source check to publish handler**

In `pkg/src/api/packages.rs`, in the `publish` function, after finding an existing package (the `if let Some(row) = existing` branch), add a source check:

```rust
let source: String = row.get("source");
if source == "github" {
    return (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "This package is GitHub-linked and cannot be published via CLI. Push a new semver tag to the linked repository instead."
        })),
    ).into_response();
}
```

This requires adding `source` to the SELECT query for the existing package check. Change:

```sql
SELECT id FROM packages WHERE name = ?
```

to:

```sql
SELECT id, source FROM packages WHERE name = ?
```

Also, when creating a new package via CLI publish, explicitly set `source = 'upload'`:

Change the INSERT to:

```sql
INSERT INTO packages (name, description, repository_url, source) VALUES (?, ?, ?, 'upload')
```

**Step 2: Verify it compiles**

Run: `cd pkg && cargo build`
Expected: compiles without errors

**Step 3: Commit**

```bash
git add pkg/src/api/packages.rs
git commit -m "feat(pkg): enforce source locking — GitHub-linked packages reject CLI publish"
```

---

### Task 8: Web UI — GitHub connection status on account page

**Files:**
- Modify: `pkg/src/web/mod.rs` — add GitHub connection status to account template data
- Modify: `pkg/templates/account.html` — add GitHub integration section

**Step 1: Update AccountTemplate**

In `pkg/src/web/mod.rs`, add to the `AccountTemplate` struct:

```rust
pub github_connected: bool,
pub github_login: Option<String>,
```

In the `account` handler, query for the connection:

```rust
let github_row = sqlx::query(
    "SELECT provider_login FROM oauth_connections WHERE user_id = ? AND provider = 'github' AND revoked_at IS NULL"
)
.bind(user.id)
.fetch_optional(&state.db)
.await
.ok()
.flatten();

let github_connected = github_row.is_some();
let github_login = github_row.map(|r| r.get::<String, _>("provider_login"));
```

Pass these to the template.

**Step 2: Update account.html**

Add a "GitHub Integration" section to the account nav and content. Between the Profile section and My Packages section:

```html
<a href="#github">GitHub</a>
```

And in the content area:

```html
<div class="account-section" id="github">
  <h2>GitHub Integration</h2>
  {% if github_connected %}
  <p style="font-size:0.78rem; color:var(--text);">
    Connected as <strong style="color:var(--gold);">{% if let Some(ref login) = github_login %}{{ login }}{% endif %}</strong>
  </p>
  <div style="margin-top:0.75rem; display:flex; gap:0.5rem;">
    <a href="/auth/github?mode=connect&return_to=/account" class="btn btn-secondary" style="font-size:0.7rem; padding:0.4rem 0.8rem;">Reconnect</a>
  </div>
  {% else %}
  <p style="font-size:0.78rem; color:var(--text-dim); margin-bottom:0.75rem;">
    Connect your GitHub account to link repositories and auto-publish packages from tags.
  </p>
  <a href="/auth/github?mode=connect&return_to=/account" class="btn btn-primary" style="font-size:0.7rem; padding:0.45rem 0.8rem;">Connect GitHub</a>
  {% endif %}
</div>
```

**Step 3: Verify it compiles**

Run: `cd pkg && cargo build`
Expected: compiles without errors

**Step 4: Commit**

```bash
git add pkg/src/web/mod.rs pkg/templates/account.html
git commit -m "feat(pkg): show GitHub connection status on account page"
```

---

### Task 9: Web UI — Link Repository page

**Files:**
- Create: `pkg/templates/link.html`
- Modify: `pkg/src/web/mod.rs` — add link page handler + template
- Modify: `pkg/src/lib.rs` — add route

**Step 1: Create link.html template**

```html
{% extends "base.html" %}
{% block title %}Link Repository — Sema Pkg{% endblock %}
{% block content %}
<div class="page-content" x-data="{ url: '', loading: false, result: null, error: null }">
  <h1 style="font-family:var(--serif); font-size:1.6rem; font-weight:300; color:var(--text-bright); margin-bottom:0.5rem;">Link GitHub Repository</h1>
  <p style="font-size:0.82rem; color:var(--text-dim); margin-bottom:1.5rem;">
    Import a Sema package from a GitHub repository. The repo must contain a <code style="font-family:var(--mono); color:var(--gold); font-size:0.82em;">sema.toml</code> at the root. Semver tags (e.g. <code style="font-family:var(--mono); color:var(--gold); font-size:0.82em;">v1.0.0</code>) will be imported as versions, and a webhook will be registered for automatic updates.
  </p>

  {% if !github_connected %}
  <div style="padding:1rem; background:var(--bg-code); border:1px solid var(--border); border-radius:6px; margin-bottom:1.5rem;">
    <p style="font-size:0.82rem; color:var(--text-bright); margin-bottom:0.5rem;">Connect GitHub to continue</p>
    <p style="font-size:0.75rem; color:var(--text-dim); margin-bottom:0.75rem;">We need permission to read your repositories and register webhooks.</p>
    <a href="/auth/github?mode=connect&return_to=/link" class="btn btn-primary" style="font-size:0.7rem; padding:0.45rem 0.8rem;">Connect GitHub</a>
  </div>
  {% else %}
  <p style="font-size:0.72rem; color:var(--text-dim); margin-bottom:1rem;">
    Connected as <strong style="color:var(--gold);">{% if let Some(ref login) = github_login %}{{ login }}{% endif %}</strong> · <a href="/auth/github?mode=connect&return_to=/link" style="color:var(--gold);">Reconnect</a>
  </p>

  <div class="form-group" style="max-width:32rem;">
    <label class="form-label">GitHub Repository URL</label>
    <div style="display:flex; gap:0.5rem; align-items:center;">
      <input type="text" class="form-input" placeholder="github.com/user/my-sema-package" x-model="url" :disabled="loading">
      <button class="btn btn-primary" style="font-size:0.7rem; padding:0.45rem 0.8rem; white-space:nowrap;" :disabled="loading || !url.trim()" @click="
        loading = true; error = null; result = null;
        try {
          const res = await fetch('/api/v1/packages/link', {method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({repository_url: url})});
          const data = await res.json();
          if (res.ok) { result = data; } else { error = data.error || 'Unknown error'; }
        } catch(e) { error = e.message; }
        loading = false;
      " x-text="loading ? 'Linking…' : 'Link Repository'"></button>
    </div>
  </div>

  <div x-show="error" style="margin-top:1rem; padding:0.75rem; background:rgba(255,82,82,0.08); border:1px solid rgba(255,82,82,0.2); border-radius:4px;">
    <p style="font-size:0.78rem; color:#ff5252;" x-text="error"></p>
  </div>

  <div x-show="result" style="margin-top:1rem; padding:0.75rem; background:var(--gold-glow); border:1px solid var(--gold-dim); border-radius:4px;">
    <p style="font-size:0.82rem; color:var(--text-bright);">✓ Linked <strong x-text="result?.package"></strong></p>
    <p style="font-size:0.72rem; color:var(--text-dim); margin-top:0.25rem;">
      <span x-text="result?.tags_found"></span> tags found, <span x-text="result?.versions_imported"></span> versions imported
    </p>
    <a :href="'/packages/' + result?.package" class="btn btn-secondary" style="font-size:0.7rem; padding:0.35rem 0.7rem; margin-top:0.5rem;">View Package →</a>
  </div>
  {% endif %}
</div>
{% endblock %}
```

**Step 2: Add template and handler to web/mod.rs**

Template struct:

```rust
#[derive(Template)]
#[template(path = "link.html")]
pub struct LinkTemplate {
    pub username: Option<String>,
    pub github_connected: bool,
    pub github_login: Option<String>,
}
```

Handler:

```rust
pub async fn link_page(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let cookie = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let session_id = cookie.split(';')
        .filter_map(|c| c.trim().strip_prefix("session="))
        .next();

    let session_id = match session_id {
        Some(s) => s,
        None => return Redirect::to("/login").into_response(),
    };

    let user = match get_session_user(&state.db, session_id).await {
        Some(u) => u,
        None => return Redirect::to("/login").into_response(),
    };

    let github_row = sqlx::query(
        "SELECT provider_login FROM oauth_connections WHERE user_id = ? AND provider = 'github' AND revoked_at IS NULL"
    )
    .bind(user.id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let github_connected = github_row.is_some();
    let github_login = github_row.map(|r| r.get::<String, _>("provider_login"));

    render(LinkTemplate {
        username: Some(user.username),
        github_connected,
        github_login,
    }).into_response()
}
```

**Step 3: Wire route in lib.rs**

```rust
.route("/link", get(web::link_page))
```

**Step 4: Verify it compiles**

Run: `cd pkg && cargo build`
Expected: compiles without errors

**Step 5: Commit**

```bash
git add pkg/templates/link.html pkg/src/web/mod.rs pkg/src/lib.rs
git commit -m "feat(pkg): add Link Repository web page"
```

---

### Task 10: Web UI — Source badge on package detail

**Files:**
- Modify: `pkg/src/web/mod.rs` — add `source` and `github_repo` to PackageTemplate
- Modify: `pkg/templates/package.html` — display source badge and sync button

**Step 1: Update PackageTemplate**

Add to the `PackageTemplate` struct in `pkg/src/web/mod.rs`:

```rust
pub source: String,
pub github_repo: Option<String>,
```

In `package_detail`, add to the SELECT query:

```sql
SELECT id, name, description, repository_url, source, github_repo FROM packages WHERE name = ?
```

And populate:

```rust
let source: String = pkg.get("source");
let github_repo: Option<String> = pkg.get("github_repo");
```

Pass them to the template.

**Step 2: Update package.html**

After the package name heading, add a source badge:

```html
{% if source == "github" %}
<span class="badge" style="background:rgba(255,255,255,0.06); color:var(--text-dim); font-size:0.6rem;">GitHub</span>
{% endif %}
```

In the sidebar, if github-linked, show the repo link and a sync indicator:

```html
{% if source == "github" %}
{% if let Some(ref gh) = github_repo %}
<section>
  <div class="section-label">Source</div>
  <a href="https://github.com/{{ gh }}" class="sidebar-link">{{ gh }}</a>
  <div style="font-size:0.65rem; color:var(--text-dim); margin-top:0.25rem;">Auto-published from GitHub tags</div>
</section>
{% endif %}
{% endif %}
```

**Step 3: Verify it compiles**

Run: `cd pkg && cargo build`
Expected: compiles without errors

**Step 4: Commit**

```bash
git add pkg/src/web/mod.rs pkg/templates/package.html
git commit -m "feat(pkg): show source badge and GitHub repo on package detail page"
```

---

### Task 11: Add "Link Repository" to navigation

**Files:**
- Modify: `pkg/templates/base.html` — add link to nav for logged-in users
- Modify: `pkg/templates/account.html` — add link in packages section

**Step 1: Update base.html nav**

In the nav section, add a "Link Repo" link for logged-in users, before "Account":

```html
{% if username.is_some() %}
<a href="/link" class="header-link">Link Repo</a>
<a href="/account" class="header-link">Account</a>
{% else %}
```

**Step 2: Update account.html**

In the "My Packages" section, add a link to the link page after the package list (or instead of the empty state message):

Replace the empty state message:

```html
<div class="empty-state">
  You haven't published any packages yet.
  <a href="/link" style="color:var(--gold); margin-left:0.25rem;">Link a GitHub repository →</a>
</div>
```

**Step 3: Verify it compiles**

Run: `cd pkg && cargo build`
Expected: compiles without errors

**Step 4: Commit**

```bash
git add pkg/templates/base.html pkg/templates/account.html
git commit -m "feat(pkg): add Link Repository navigation links"
```

---

### Task 12: Update .env.example and README

**Files:**
- Modify: `pkg/.env.example`
- Modify: `pkg/README.md` (if it exists, otherwise create)

**Step 1: Update .env.example**

Add the new env var:

```env
# OAUTH_TOKEN_KEY=generate-a-random-32-char-string
```

**Step 2: Update README**

Add a section about GitHub-linked packages:

- Explain the feature
- Document the OAuth scopes needed
- Document the webhook URL that must be reachable
- Document the `OAUTH_TOKEN_KEY` env var
- Document the API endpoints: `POST /api/v1/packages/link`, `POST /api/v1/packages/{name}/sync`, `POST /api/v1/webhooks/github`

**Step 3: Commit**

```bash
git add pkg/.env.example pkg/README.md
git commit -m "docs(pkg): document GitHub-linked packages feature"
```

---

## Implementation order and dependency graph

```
Task 1 (migration)
  └─> Task 2 (crypto)
        └─> Task 3 (OAuth connect mode)
              └─> Task 4 (sync core logic)
                    └─> Task 5 (link endpoint)
                    └─> Task 6 (webhook endpoint)
                    └─> Task 7 (source locking)
              └─> Task 8 (account page GitHub status)
                    └─> Task 9 (link page)
                    └─> Task 10 (package detail badge)
                    └─> Task 11 (navigation)
Task 12 (docs) — last
```

Tasks 5, 6, 7 are independent of each other (all depend on Task 4).
Tasks 8–11 are sequential (each builds on template changes from the previous).
Task 12 is standalone.

---

## Testing checklist (manual, after all tasks)

1. **Password user flow**: Sign up with password → go to `/link` → see "Connect GitHub" → click → OAuth flow → redirected back to `/link` → paste repo URL → see imported versions
2. **GitHub login flow**: Login with GitHub → token stored automatically → go to `/link` → see "Connected as X" → link a repo
3. **Webhook**: Push a new semver tag to the linked repo → verify new version appears in the registry within seconds
4. **Source locking**: Try `sema pkg publish` on a GitHub-linked package → get rejected
5. **Non-semver tags**: Push a non-semver tag (e.g. `latest`) → webhook receives it → skips silently
6. **Token revocation**: Revoke the GitHub token on github.com → try manual sync → get "reconnect" prompt
