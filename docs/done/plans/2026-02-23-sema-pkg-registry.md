# Sema Pkg Registry — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a self-hostable package registry for Sema that also powers the central registry at pkg.sema-lang.com.

**Architecture:** Single Axum binary with pluggable backends (SQLite/Postgres for DB, filesystem/S3 for blobs). Server-rendered HTML templates (Askama) with Alpine.js for interactivity. REST API for CLI clients. Auth via username/password + GitHub OAuth.

**Tech Stack:** Rust, Axum 0.8, SQLx (SQLite + Postgres), Askama, Alpine.js, argon2, tower-http, tokio.

---

### Task 1: Scaffold the Rust project

**Files:**
- Create: `pkg/Cargo.toml`
- Create: `pkg/src/main.rs`
- Create: `pkg/src/config.rs`
- Create: `pkg/.env.example`
- Create: `pkg/Dockerfile`
- Create: `pkg/.gitignore`

**Step 1: Create Cargo.toml**

```toml
[package]
name = "sema-pkg"
version = "0.1.0"
edition = "2021"
license = "MIT"
description = "Self-hostable package registry for Sema"

[dependencies]
axum = { version = "0.8", features = ["multipart"] }
tokio = { version = "1", features = ["full"] }
tower-http = { version = "0.6", features = ["fs", "cors", "trace"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "postgres", "migrate"] }
askama = "0.12"
askama_axum = "0.4"
argon2 = "0.5"
sha2 = "0.10"
rand = "0.8"
base64 = "0.22"
dotenvy = "0.15"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
thiserror = "2"
semver = { version = "1", features = ["serde"] }
time = { version = "0.3", features = ["serde"] }
```

**Step 2: Create config.rs**

Reads configuration from environment variables with sensible defaults for self-hosting:

```rust
use std::env;

pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub blob_dir: String,
    pub base_url: String,
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    pub session_secret: String,
    pub max_tarball_bytes: usize,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3000),
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://data/registry.db?mode=rwc".into()),
            blob_dir: env::var("BLOB_DIR").unwrap_or_else(|_| "data/blobs".into()),
            base_url: env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:3000".into()),
            github_client_id: env::var("GITHUB_CLIENT_ID").ok(),
            github_client_secret: env::var("GITHUB_CLIENT_SECRET").ok(),
            session_secret: env::var("SESSION_SECRET")
                .unwrap_or_else(|_| "change-me-in-production".into()),
            max_tarball_bytes: env::var("MAX_TARBALL_BYTES")
                .ok().and_then(|v| v.parse().ok())
                .unwrap_or(50 * 1024 * 1024), // 50 MB
        }
    }
}
```

**Step 3: Create main.rs**

Minimal Axum server that boots, loads config, connects to DB, and serves a health check:

```rust
mod config;

use axum::{Router, routing::get};
use config::Config;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = Config::from_env();
    let addr = format!("{}:{}", config.host, config.port);

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }));

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("sema-pkg listening on {}", addr);
    axum::serve(listener, app).await.unwrap();
}
```

**Step 4: Create .env.example**

```env
DATABASE_URL=sqlite://data/registry.db?mode=rwc
BLOB_DIR=data/blobs
BASE_URL=http://localhost:3000
# SESSION_SECRET=generate-a-random-string
# GITHUB_CLIENT_ID=
# GITHUB_CLIENT_SECRET=
```

**Step 5: Create Dockerfile**

```dockerfile
FROM rust:1.84-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/sema-pkg /usr/local/bin/
EXPOSE 3000
ENV DATABASE_URL=sqlite://data/registry.db?mode=rwc
ENV BLOB_DIR=data/blobs
CMD ["sema-pkg"]
```

**Step 6: Create .gitignore**

```
/target
/data
.env
```

**Step 7: Verify it builds and runs**

Run: `cd pkg && cargo build`
Expected: compiles without errors

Run: `cd pkg && cargo run`
Expected: "sema-pkg listening on 0.0.0.0:3000"

**Step 8: Commit**

```bash
git add pkg/Cargo.toml pkg/src/ pkg/.env.example pkg/Dockerfile pkg/.gitignore
git commit -m "feat(pkg): scaffold sema-pkg registry project"
```

---

### Task 2: Database schema and migrations

**Files:**
- Create: `pkg/migrations/001_initial.sql`
- Create: `pkg/src/db.rs`
- Modify: `pkg/src/main.rs`

**Step 1: Write the initial migration**

Create `pkg/migrations/001_initial.sql`:

```sql
-- Users
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT, -- NULL if GitHub-only auth
    github_id INTEGER UNIQUE,
    homepage TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Web sessions
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id),
    expires_at TIMESTAMP NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- CLI API tokens
CREATE TABLE IF NOT EXISTS api_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes TEXT NOT NULL DEFAULT 'publish',
    last_used_at TIMESTAMP,
    revoked_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Packages
CREATE TABLE IF NOT EXISTS packages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    repository_url TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Package versions
CREATE TABLE IF NOT EXISTS package_versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    package_id INTEGER NOT NULL REFERENCES packages(id),
    version TEXT NOT NULL,
    checksum_sha256 TEXT NOT NULL,
    blob_key TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    yanked INTEGER NOT NULL DEFAULT 0,
    sema_version_req TEXT,
    published_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(package_id, version)
);

-- Dependencies
CREATE TABLE IF NOT EXISTS dependencies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    version_id INTEGER NOT NULL REFERENCES package_versions(id),
    dependency_name TEXT NOT NULL,
    version_req TEXT NOT NULL
);

-- Package owners
CREATE TABLE IF NOT EXISTS owners (
    package_id INTEGER NOT NULL REFERENCES packages(id),
    user_id INTEGER NOT NULL REFERENCES users(id),
    PRIMARY KEY (package_id, user_id)
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_versions_package ON package_versions(package_id);
CREATE INDEX IF NOT EXISTS idx_deps_version ON dependencies(version_id);
CREATE INDEX IF NOT EXISTS idx_tokens_user ON api_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
```

**Step 2: Create db.rs**

Database connection setup and migration runner:

```rust
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

pub type Db = SqlitePool;

pub async fn connect(database_url: &str) -> Db {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
        .expect("Failed to connect to database");

    // Enable WAL mode for better concurrent reads
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&pool)
        .await
        .expect("Failed to set WAL mode");

    // Run migrations
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    pool
}
```

**Step 3: Wire DB into main.rs**

Add `mod db;` and pass the pool into Axum state:

```rust
mod config;
mod db;

use axum::{Router, routing::get, extract::State};
use config::Config;
use std::sync::Arc;

pub struct AppState {
    pub db: db::Db,
    pub config: Config,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env();
    let db = db::connect(&config.database_url).await;

    // Ensure blob directory exists
    std::fs::create_dir_all(&config.blob_dir).expect("Failed to create blob dir");

    let state = Arc::new(AppState { db, config });
    let addr = format!("{}:{}", state.config.host, state.config.port);

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("sema-pkg listening on {}", addr);
    axum::serve(listener, app).await.unwrap();
}
```

**Step 4: Verify migrations run**

Run: `cd pkg && cargo run`
Expected: starts without errors, creates `data/registry.db` with tables

**Step 5: Commit**

```bash
git add pkg/migrations/ pkg/src/db.rs pkg/src/main.rs
git commit -m "feat(pkg): add database schema and migrations"
```

---

### Task 3: Auth — registration, login, sessions

**Files:**
- Create: `pkg/src/auth.rs`
- Create: `pkg/src/api/mod.rs`
- Create: `pkg/src/api/auth.rs`
- Modify: `pkg/src/main.rs`

**Step 1: Create auth.rs with password hashing and session helpers**

- `hash_password(password: &str) -> String` — argon2 hash
- `verify_password(password: &str, hash: &str) -> bool`
- `generate_session_id() -> String` — random 32 bytes, base64url
- `create_session(db, user_id) -> session_id` — insert into sessions table with 7-day expiry
- `get_session_user(db, session_id) -> Option<User>` — look up valid session, return user
- Axum extractor `AuthUser` that reads session cookie and resolves to a User or rejects

**Step 2: Create api/auth.rs with handlers**

- `POST /api/v1/auth/register` — accepts `{username, email, password}`, creates user, returns session cookie
- `POST /api/v1/auth/login` — accepts `{username, password}`, validates, returns session cookie
- `POST /api/v1/auth/logout` — clears session cookie, deletes session from DB
- All return JSON responses with appropriate status codes

Validation rules:
- Username: 2-39 chars, alphanumeric + hyphens, no leading/trailing hyphens
- Password: minimum 8 chars
- Email: basic format check

**Step 3: Wire routes into main.rs**

```rust
mod auth;
mod api;

// In router:
.route("/api/v1/auth/register", post(api::auth::register))
.route("/api/v1/auth/login", post(api::auth::login))
.route("/api/v1/auth/logout", post(api::auth::logout))
```

**Step 4: Test manually with curl**

```bash
curl -X POST http://localhost:3000/api/v1/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"username":"testuser","email":"test@example.com","password":"password123"}'
# Expected: 201 with Set-Cookie header

curl -X POST http://localhost:3000/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"username":"testuser","password":"password123"}'
# Expected: 200 with Set-Cookie header
```

**Step 5: Commit**

```bash
git add pkg/src/auth.rs pkg/src/api/
git commit -m "feat(pkg): add user registration, login, and session auth"
```

---

### Task 4: API tokens for CLI auth

**Files:**
- Create: `pkg/src/api/tokens.rs`
- Modify: `pkg/src/auth.rs` — add token verification extractor
- Modify: `pkg/src/api/mod.rs`
- Modify: `pkg/src/main.rs`

**Step 1: Create api/tokens.rs**

- `POST /api/v1/tokens` — session required, accepts `{name}`, generates `sema_pat_<base64url 32 bytes>`, stores argon2 hash, returns token plaintext (shown once)
- `GET /api/v1/tokens` — session required, returns list of tokens (id, name, scopes, created_at, last_used_at — never the token itself)
- `DELETE /api/v1/tokens/{id}` — session required, sets revoked_at

**Step 2: Add Bearer token extractor to auth.rs**

`TokenUser` extractor that:
- Reads `Authorization: Bearer sema_pat_...` header
- Hashes the token, looks up in api_tokens table where revoked_at IS NULL
- Updates last_used_at
- Returns the associated User and token scopes

**Step 3: Wire routes**

```rust
.route("/api/v1/tokens", post(api::tokens::create).get(api::tokens::list))
.route("/api/v1/tokens/{id}", delete(api::tokens::revoke))
```

**Step 4: Test with curl**

```bash
# Create token (needs session cookie from login)
curl -X POST http://localhost:3000/api/v1/tokens \
  -H 'Content-Type: application/json' \
  -H 'Cookie: session=...' \
  -d '{"name":"test-token"}'
# Expected: 201 with {"token": "sema_pat_...", "id": 1, "name": "test-token"}
```

**Step 5: Commit**

```bash
git add pkg/src/api/tokens.rs pkg/src/auth.rs pkg/src/api/mod.rs pkg/src/main.rs
git commit -m "feat(pkg): add API token management for CLI auth"
```

---

### Task 5: Package publish endpoint

**Files:**
- Create: `pkg/src/api/packages.rs`
- Create: `pkg/src/blob.rs`
- Modify: `pkg/src/api/mod.rs`
- Modify: `pkg/src/main.rs`

**Step 1: Create blob.rs**

Filesystem blob storage:
- `store(blob_dir: &str, data: &[u8]) -> (blob_key, sha256_hex, size)`
  - Computes SHA-256 of data
  - Stores at `{blob_dir}/sha256/{first2}/{hash}.tar.gz`
  - Returns the key, checksum, and size
- `read(blob_dir: &str, blob_key: &str) -> Vec<u8>`
- `exists(blob_dir: &str, blob_key: &str) -> bool`

**Step 2: Create api/packages.rs — publish handler**

`PUT /api/v1/packages/{name}/{version}` — requires Bearer token with `publish` scope

Accepts multipart upload with:
- `tarball` field — the package tarball (must contain `sema.toml` at root with `[package]` and optionally `[deps]`)
- `metadata` field — JSON with `{description, repository_url}` (overrides or supplements what's in `sema.toml`)

Handler logic:
1. Validate package name format (e.g. `github.com/user/repo` or short names)
2. Parse and validate semver version
3. Check tarball size against max_tarball_bytes
4. If package doesn't exist, create it and set publishing user as owner
5. If package exists, verify user is an owner
6. Check version doesn't already exist (immutable)
7. Store tarball blob
8. Insert package_version row
9. Insert dependency rows
10. Return 201 with version metadata + checksum

**Step 3: Wire route**

```rust
.route("/api/v1/packages/{name}/{version}", put(api::packages::publish))
```

**Step 4: Test with curl**

```bash
# Publish a package
curl -X PUT http://localhost:3000/api/v1/packages/test-pkg/0.1.0 \
  -H 'Authorization: Bearer sema_pat_...' \
  -F 'tarball=@test.tar.gz' \
  -F 'metadata={"description":"A test package","dependencies":[]}'
# Expected: 201 with version metadata
```

**Step 5: Commit**

```bash
git add pkg/src/blob.rs pkg/src/api/packages.rs
git commit -m "feat(pkg): add package publish endpoint with blob storage"
```

---

### Task 6: Package read endpoints (metadata + download)

**Files:**
- Modify: `pkg/src/api/packages.rs`

**Step 1: Add read handlers**

- `GET /api/v1/packages?q=&page=&per_page=` — search packages by name/description, paginated. Returns `{packages: [...], total, page}`.
- `GET /api/v1/packages/{name}` — package summary: name, description, repo_url, owners, latest version, created_at
- `GET /api/v1/packages/{name}/versions` — all versions with metadata, deps, checksum, yanked flag
- `GET /api/v1/packages/{name}/{version}/download` — stream tarball from blob storage. Set `Content-Type: application/gzip`, `Content-Disposition: attachment`.

Search implementation: SQLite `LIKE '%query%'` on name and description. Good enough for thousands of packages. Add FTS later if needed.

**Step 2: Add yank/unyank**

- `POST /api/v1/packages/{name}/{version}/yank` — Bearer token, owner only, sets yanked=1
- `POST /api/v1/packages/{name}/{version}/unyank` — same, sets yanked=0

**Step 3: Wire routes**

```rust
.route("/api/v1/packages", get(api::packages::search))
.route("/api/v1/packages/{name}", get(api::packages::show))
.route("/api/v1/packages/{name}/versions", get(api::packages::versions))
.route("/api/v1/packages/{name}/{version}/download", get(api::packages::download))
.route("/api/v1/packages/{name}/{version}/yank", post(api::packages::yank))
.route("/api/v1/packages/{name}/{version}/unyank", post(api::packages::unyank))
```

**Step 4: Test full publish → download cycle**

```bash
# Search
curl http://localhost:3000/api/v1/packages?q=test
# Expected: JSON with matching packages

# Download
curl -o pkg.tar.gz http://localhost:3000/api/v1/packages/test-pkg/0.1.0/download
# Expected: downloads the tarball
```

**Step 5: Commit**

```bash
git add pkg/src/api/packages.rs pkg/src/main.rs
git commit -m "feat(pkg): add package search, metadata, download, and yank endpoints"
```

---

### Task 7: Ownership management

**Files:**
- Modify: `pkg/src/api/packages.rs` (or create `pkg/src/api/owners.rs`)

**Step 1: Add owner endpoints**

- `GET /api/v1/packages/{name}/owners` — list owners (public)
- `POST /api/v1/packages/{name}/owners` — add owner, accepts `{username}`, requires Bearer token + existing ownership
- `DELETE /api/v1/packages/{name}/owners/{username}` — remove owner, requires Bearer token + ownership. Cannot remove last owner.

**Step 2: Wire routes and test**

**Step 3: Commit**

```bash
git commit -m "feat(pkg): add package ownership management"
```

---

### Task 8: Web UI — templates and layout

**Files:**
- Create: `pkg/src/web/mod.rs`
- Create: `pkg/templates/base.html` — shared layout (header, footer, fonts, shared.css, Alpine.js CDN)
- Create: `pkg/templates/index.html` — homepage (search + featured + recent)
- Create: `pkg/templates/search.html` — search results
- Create: `pkg/templates/package.html` — package detail with tabs
- Create: `pkg/templates/login.html` — sign in / create account
- Create: `pkg/templates/account.html` — profile, packages, tokens
- Create: `pkg/static/style.css` — adapted from `pkg/prototypes/shared.css`
- Modify: `pkg/src/main.rs`

**Step 1: Create base.html layout**

Askama base template with blocks for title, content. Includes:
- Shared CSS (served from `/static/style.css`)
- Google Fonts (Cormorant + JetBrains Mono)
- Alpine.js via CDN
- Header with logo, nav, conditional sign-in/account link based on auth state

**Step 2: Convert each prototype HTML to Askama template**

Each template extends `base.html` and replaces hardcoded data with template variables. For example `search.html`:

```html
{% extends "base.html" %}
{% block title %}Search — Sema Pkg{% endblock %}
{% block content %}
<div class="page-content">
  <div>Results for "<span style="color:var(--gold)">{{ query }}</span>" — {{ total }} packages</div>
  {% for pkg in packages %}
  <a href="/packages/{{ pkg.name }}" class="pkg-list-item">
    <div class="pkg-list-left">
      <span class="pkg-list-name">{{ pkg.name }}</span>
      <span class="pkg-list-version">{{ pkg.latest_version }}</span>
      <div class="pkg-list-desc">{{ pkg.description }}</div>
    </div>
  </a>
  {% endfor %}
</div>
{% endblock %}
```

**Step 3: Create web/mod.rs with page handlers**

Each handler queries the DB and renders the template:
- `GET /` — homepage: featured packages + recently updated
- `GET /search?q=` — search results
- `GET /packages/{name}` — package detail
- `GET /login` — login/signup page
- `GET /account` — account page (session required, redirects to /login otherwise)

**Step 4: Wire routes and static file serving**

```rust
// Static files
.nest_service("/static", tower_http::services::ServeDir::new("static"))

// Web pages
.route("/", get(web::index))
.route("/search", get(web::search))
.route("/packages/{name}", get(web::package_detail))
.route("/login", get(web::login))
.route("/account", get(web::account))
```

**Step 5: Add Alpine.js interactivity**

- Tab switching on package detail page (x-data, x-show, @click)
- Token creation/revocation on account page (fetch to API, update UI)
- Login/signup tab switching
- Search form submission

**Step 6: Verify by browsing**

Open `http://localhost:3000` — should render the homepage with real data from DB.

**Step 7: Commit**

```bash
git add pkg/templates/ pkg/static/ pkg/src/web/
git commit -m "feat(pkg): add server-rendered web UI with Askama templates"
```

---

### Task 9: GitHub OAuth (optional, skip if no credentials)

**Files:**
- Create: `pkg/src/auth/github.rs`
- Modify: `pkg/src/main.rs`

**Step 1: Implement OAuth PKCE flow**

- `GET /auth/github` — generate state + PKCE verifier, store in cookie, redirect to GitHub authorize URL
- `GET /auth/github/callback` — exchange code for access token, fetch GitHub user info, find or create user, create session

If user with matching `github_id` exists, log them in. Otherwise create a new user using GitHub username and email.

**Step 2: Add reqwest dependency for GitHub API calls**

Add `reqwest = { version = "0.12", features = ["json"] }` to Cargo.toml.

**Step 3: Wire routes**

```rust
.route("/auth/github", get(auth::github::start))
.route("/auth/github/callback", get(auth::github::callback))
```

**Step 4: Test**

Only testable with real GitHub OAuth app credentials. Skip in CI. Document setup in README.

**Step 5: Commit**

```bash
git commit -m "feat(pkg): add GitHub OAuth login"
```

---

### Task 10: Docker and README

**Files:**
- Modify: `pkg/Dockerfile` — finalize with multi-stage build, copy static + templates + migrations
- Create: `pkg/docker-compose.yml` — for local dev (mounts data volume)
- Create: `pkg/README.md`

**Step 1: Finalize Dockerfile**

```dockerfile
FROM rust:1.84-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY migrations/ migrations/
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /build/target/release/sema-pkg /usr/local/bin/
COPY templates/ templates/
COPY static/ static/
COPY migrations/ migrations/
EXPOSE 3000
VOLUME ["/app/data"]
CMD ["sema-pkg"]
```

**Step 2: Create docker-compose.yml**

```yaml
services:
  registry:
    build: .
    ports:
      - "3000:3000"
    volumes:
      - ./data:/app/data
    environment:
      DATABASE_URL: sqlite://data/registry.db?mode=rwc
      BLOB_DIR: data/blobs
      BASE_URL: http://localhost:3000
```

**Step 3: Write README.md**

Cover:
- What it is (one paragraph)
- Quick start: `cargo run` or `docker compose up`
- Configuration reference (env vars table)
- API endpoints reference (table)
- Self-hosting guide
- GitHub OAuth setup (optional)

**Step 4: Verify Docker build**

```bash
cd pkg && docker compose up --build
# Expected: builds and starts, accessible at localhost:3000
```

**Step 5: Commit**

```bash
git add pkg/Dockerfile pkg/docker-compose.yml pkg/README.md
git commit -m "feat(pkg): add Docker support and README"
```

---

## Implementation order and dependency graph

```
Task 1 (scaffold)
  └─> Task 2 (database)
        └─> Task 3 (auth: register/login/sessions)
              ├─> Task 4 (API tokens)
              │     └─> Task 5 (publish endpoint)
              │           └─> Task 6 (read endpoints)
              │                 └─> Task 7 (ownership)
              └─> Task 9 (GitHub OAuth) — independent, can be done anytime after Task 3
        └─> Task 8 (web UI) — can start after Task 6, needs read data
Task 10 (Docker + README) — last
```

Tasks 5-7 are sequential (publish before read before ownership).
Task 8 (web UI) can be parallelized with Tasks 5-7 using stub data initially.
Task 9 (GitHub OAuth) is independent and optional.

---

## V2 Features (post-MVP)

### V2-1: GitHub-linked packages (Packagist-style)

Allow users to register a package by pasting a GitHub repository URL instead of publishing via CLI. The registry pulls package metadata and tarballs from GitHub automatically.

**How it works:**

1. User pastes a GitHub repo URL in the web UI (e.g. `github.com/helgesverre/sema-http`)
2. Registry validates the repo exists and contains a `sema.toml` manifest at the root:
   ```toml
   [package]
   name = "sema-http"
   version = "0.1.0"
   description = "HTTP client and server primitives"
   license = "MIT"
   entrypoint = "package.sema"

   [deps]
   "github.com/someuser/sema-json" = "v1.0.0"
   ```
3. Registry reads the manifest, creates the package, and imports existing tags/releases as versions
4. A GitHub webhook is registered on the repo — when new tags are pushed, the registry auto-publishes:
   - Fetches the tag's tarball via GitHub API (`/repos/{owner}/{repo}/tarball/{tag}`)
   - Extracts and validates `sema.toml` from the archive
   - Stores blob and creates the version record

**Schema changes:**

- Add `source` column to `packages`: `'upload'` (CLI) or `'github'` (linked)
- Add `github_repo` column to `packages`: `owner/repo` string
- Add `webhook_secret` column to `packages`: per-package secret for verifying webhook payloads
- New table `github_sync_log`: id, package_id, tag, status, error, synced_at

**New endpoints:**

- `POST /api/v1/packages/link` — session required, accepts `{repository_url}`, validates repo, registers webhook, imports existing tags
- `POST /api/v1/webhooks/github` — receives push events, verifies signature, triggers version sync
- `POST /api/v1/packages/{name}/sync` — session required (owner), manually re-sync from GitHub

**Web UI additions:**

- "Add Package" page with a repo URL input field and a "Link Repository" button
- Package detail shows source badge: "CLI" or "GitHub" with link to repo
- Sync status/log visible to owners on the package page

**Considerations:**

- Requires a GitHub App or OAuth app with repo read permissions
- Rate limiting on GitHub API (use conditional requests with ETags)
- Handle private repos: user must grant access, registry stores an installation token
- Dual-source packages: a package linked to GitHub cannot also be published via CLI (pick one source)
- Tag-to-version mapping: strip leading `v` from tags, validate as semver, skip non-semver tags

### V2-2: Postgres backend

Add `sqlx::PgPool` as an alternative database backend. Use a runtime enum or trait object to switch between SQLite and Postgres based on `DATABASE_URL` scheme. Adapt migrations for Postgres syntax where needed (e.g. `SERIAL` vs `AUTOINCREMENT`).

### V2-3: S3 blob storage

Add an S3-compatible blob backend behind the existing `blob` module trait. Configure via `BLOB_BACKEND=s3` + `S3_ENDPOINT`, `S3_BUCKET`, `S3_ACCESS_KEY`, `S3_SECRET_KEY`. For downloads, generate pre-signed URLs and redirect instead of streaming through Axum.

### V2-4: Download counts

Track per-version download counts. Increment on each `/download` hit (debounced per IP to avoid inflating). Surface on package detail page and search results. Add `GET /api/v1/packages/{name}/downloads` for stats.

---

## Notes

- **No sema-lang dependency** — this project stands alone in `pkg/`
- **SQLite first** — Postgres support can be added later by adding SQLx postgres feature and adapting migrations
- **S3 blob backend** — not in MVP, add when needed (trait is easy to add to blob.rs)
- **Package name format** — currently `github.com/user/repo` paths. The registry may also support short names. The route `{name}` uses `*name` catch-all or URL encoding.
- **Manifest file** — `sema.toml` with `[package]` (name, version, entrypoint) and `[deps]` sections. Default entrypoint is `package.sema`.
- **Askama templates** — compile-time checked, type-safe, zero overhead
- **Alpine.js** — loaded from CDN, no build step needed
