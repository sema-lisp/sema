# sema-pkg

Self-hostable package registry for the [Sema](https://sema-lang.com) programming language. Ships as a single binary with SQLite, serves both a web UI and a REST API for CLI clients.

## Quick Start

```bash
cd pkg

make dev          # start locally (cargo) on a fresh DB and seed it
make dev-docker   # build + start in Docker on a fresh DB and seed it
```

Both start the registry on [http://localhost:3000](http://localhost:3000) and load the
demo data (see [Local Development](#local-development) below). If port 3000 is busy they
bump to the next free port (and point the seed at the same one) — the chosen URL is
printed at startup. Override with `make dev PORT=4000`. `make dev` runs the server in the
foreground (Ctrl-C to stop); `make dev-docker` runs it detached and tails the logs
(`make down` to stop the container).

To run the server without seeding:

```bash
make run                 # locally, no reset/seed
docker compose up --build
```

Run `make help` to list all targets.

## Local Development

`make dev` / `make dev-docker` reset the database, start the server, and run `seed.sh`
once it is healthy. The seed creates a reproducible demo dataset:

- **Users:** `helge` (admin), `kari`, `magnus`, `spambot` (banned). Every seeded user has the password `123123123`. Admin login: `helge` / `123123123`, panel at `/admin`.
- **Packages:** `sema-http` (3 versions, 1 yanked), `sema-json` (2 versions), `sema-csv`, `sema-xml`, plus spam packages `free-robux` and `bitcoin-miner`.
- **Reports:** 3 open moderation reports.

`seed.sh` targets two things at once: it creates users, tokens, packages, and reports
through the **HTTP API**, and it promotes the first admin directly in **SQLite** (the API
has no way to create the first admin). `SEED_MODE` controls how that SQLite step runs:

- `SEED_MODE=local` (default) — edits the local `data/registry.db` file.
- `SEED_MODE=docker` — runs the SQL *inside* the `registry` container so it shares the
  server's filesystem (this is why the Docker image bundles `sqlite3`).

```bash
make seed                       # seed a registry that is already running (no reset)
make seed-stress                # seed + bulk synthetic data (local SQLite only)
bash seed.sh --wait             # wait for the server, then seed
SEED_MODE=docker bash seed.sh   # seed a running Docker registry
```

## Configuration

All configuration is via environment variables with sensible defaults:

| Variable | Default | Description |
|---|---|---|
| `HOST` | `0.0.0.0` | Bind address |
| `PORT` | `3000` | Listen port |
| `DATABASE_URL` | `sqlite://data/registry.db?mode=rwc` | Database URL. The engine is inferred from the scheme — `sqlite:`, `postgres:`, or `mysql:` (see [Database engines](#database-engines)). |
| `BLOB_DIR` | `data/blobs` | Directory for package tarballs |
| `BASE_URL` | `http://localhost:3000` | Public URL (used in links; enables `Secure` session cookies when `https://`) |
| `MAX_TARBALL_BYTES` | `52428800` (50 MB) | Max upload size |
| `MAX_DEPENDENCIES` | `64` | Max dependencies per published version |
| `GITHUB_CLIENT_ID` | — | GitHub OAuth app client ID (optional) |
| `GITHUB_CLIENT_SECRET` | — | GitHub OAuth app secret (optional) |
| `OAUTH_TOKEN_KEY` | — | 32-byte key encrypting stored GitHub tokens. **Required when GitHub OAuth is enabled** — the server refuses to boot if left at the insecure default. |

## Database engines

The registry runs on **SQLite**, **PostgreSQL**, or **MySQL** from the same
binary — the engine is chosen by the `DATABASE_URL` scheme:

```bash
DATABASE_URL="sqlite://data/registry.db?mode=rwc"          # default
DATABASE_URL="postgres://user:pass@host:5432/sema"         # PostgreSQL
DATABASE_URL="mysql://user:pass@host:3306/sema"            # MySQL
```

On startup the server infers the backend, applies SQLite-only tuning (WAL) where
relevant, and runs the schema migrations (`src/migration/`, SeaORM programmatic
migrations that emit correct DDL per engine). No manual schema step is needed.

All database access goes through the Data Access Layer in `src/dal/` (one module
per aggregate: `packages`, `versions`, `owners`, `deps`, `users`, `sessions`,
`tokens`, `reports`, `audit_log`, `oauth`, `downloads`, plus `admin` read models
and a `time` helper). Handlers never touch SQL directly, and the DAL avoids
engine-specific constructs — timestamps are generated in Rust, upserts use
SeaORM's `on_conflict`, and any raw SQL is standard and parameterized. This is
what keeps all three engines behaving identically; add new queries to the DAL,
not to handlers.

Run the suite against all three engines (Docker):

```bash
make test-all-drivers   # SQLite + PostgreSQL + MySQL via docker-compose.test.yml
```

## API Endpoints

### Auth

| Method | Path | Auth | Description |
|---|---|---|---|
| `POST` | `/api/v1/auth/register` | — | Create account `{username, email, password}` |
| `POST` | `/api/v1/auth/login` | — | Sign in `{username, password}` |
| `POST` | `/api/v1/auth/logout` | — | Clear session |

### Tokens

| Method | Path | Auth | Description |
|---|---|---|---|
| `POST` | `/api/v1/tokens` | Session | Create API token `{name}` |
| `GET` | `/api/v1/tokens` | Session | List your tokens |
| `DELETE` | `/api/v1/tokens/{id}` | Session | Revoke a token |

### Packages

| Method | Path | Auth | Description |
|---|---|---|---|
| `GET` | `/api/v1/search?q=&page=&per_page=` | — | Search packages |
| `GET` | `/api/v1/packages/{name}` | — | Package metadata + versions |
| `PUT` | `/api/v1/packages/{name}/{version}` | Bearer | Publish version (multipart: `tarball` + `metadata`) |
| `GET` | `/api/v1/packages/{name}/{version}/download` | — | Download tarball |
| `POST` | `/api/v1/packages/{name}/{version}/yank` | Bearer | Yank a version |

### Ownership

| Method | Path | Auth | Description |
|---|---|---|---|
| `GET` | `/api/v1/packages/{name}/owners` | — | List owners |
| `PUT` | `/api/v1/packages/{name}/owners` | Bearer | Add owner `{username}` |
| `DELETE` | `/api/v1/packages/{name}/owners` | Bearer | Remove owner `{username}` |

### GitHub-Linked Packages

| Method | Path | Auth | Description |
|---|---|---|---|
| `POST` | `/api/v1/packages/link` | Session | Link a GitHub repo `{repo_url}` |
| `POST` | `/api/v1/packages/{name}/sync` | Session | Manual re-sync from GitHub (owner only) |
| `POST` | `/api/v1/webhooks/github` | HMAC | Webhook receiver for tag events |

## GitHub-Linked Packages

Link a GitHub repository to automatically publish packages from semver tags.

### Prerequisites

- GitHub OAuth configured (`GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`)
- `OAUTH_TOKEN_KEY` set to a random 32-character string (used to encrypt stored GitHub tokens)

### How It Works

1. User connects their GitHub account via OAuth
2. User pastes a repo URL on the `/link` page
3. Registry validates the repo contains a `sema.toml`, then imports existing semver tags as versions
4. A webhook is registered on the repo — new semver tags are published automatically

### Tag-to-Version Mapping

Git tags are mapped to package versions: `v1.0.0` → `1.0.0`. Tags that don't match semver (e.g., `nightly`, `latest`) are skipped.

### Source Locking

A package is either **CLI-uploaded** or **GitHub-linked**, never both. Once a package is linked to a repo, it cannot be published via `sema publish`, and vice versa.

## Self-Hosting

1. Build: `cargo build --release`
2. Copy `target/release/sema-pkg`, `templates/`, and `static/` to your server (schema migrations are compiled into the binary and run automatically on startup)
3. Set `DATABASE_URL`, `BLOB_DIR`, `BASE_URL` (use an `https://` URL so session cookies are marked `Secure`), and — if using GitHub OAuth — a unique `OAUTH_TOKEN_KEY`
4. Run `sema-pkg` behind a reverse proxy (nginx/caddy) with TLS

Or use the Docker image:

```bash
docker compose up -d
```

Data is stored in `./data/` (SQLite DB + blob files). Back up this directory.

## GitHub OAuth (Optional)

1. Create a GitHub OAuth App at https://github.com/settings/developers
2. Set callback URL to `{BASE_URL}/auth/github/callback`
3. Set `GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET` environment variables
4. Restart the server — the GitHub login button will appear automatically

## License

[MIT](LICENSE.md)
