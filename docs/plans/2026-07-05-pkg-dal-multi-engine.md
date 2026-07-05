# Package Registry — Multi-Engine Data Access Layer

**Date:** 2026-07-05
**Status:** Done (2026-07-05). All four stages shipped. Every handler is thin;
all DB access is behind `src/dal/` (verified: no `Entity::find` / raw
`Statement` / `ActiveModel` outside `src/dal`, `src/entity`, `src/migration`).
No engine-specific SQL remains. 85 tests green on SQLite; `make test-all-drivers`
is the cross-engine oracle.
**Goal:** Make `pkg/` (the registry) run cleanly on SQLite, PostgreSQL, and MySQL by
routing all database access through a well-defined, testable Data Access Layer
(DAL), and replacing SQLite-only migrations with engine-agnostic SeaORM
programmatic migrations.

## Why

The multi-engine *test harness* already exists (`docker-compose.test.yml`,
`make test-all-drivers`, tests read `DATABASE_URL`), but the code cannot run on
Postgres/MySQL:

- `db::connect()` is SQLite-only (`SqlitePoolOptions`, `PRAGMA journal_mode=WAL`,
  `sqlx::migrate!`).
- The 6 `migrations/*.sql` files are SQLite-dialect DDL (`INTEGER PRIMARY KEY
  AUTOINCREMENT`, `TIMESTAMP DEFAULT CURRENT_TIMESTAMP`).
- ~36 raw-SQL sites use SQLite-only functions (`datetime('now')`,
  `date('now','-N days')`) and `ON CONFLICT` upserts.
- Handlers reach into the DB directly (entity calls + raw SQL interleaved),
  so there is no single place to make portable or to test per-engine.

The registry is **not yet deployed**, so there is no production data or
migration history to preserve — we can switch migration frameworks freely
(sqlx `_sqlx_migrations` → SeaORM `seaql_migrations`).

## Design

### Timestamps are application-generated strings

Every time/date column is `String` in the entities and compared
lexicographically. We keep that contract and make it portable:

- All timestamp/date columns are **TEXT** in every engine.
- The DAL generates timestamps in Rust as canonical `YYYY-MM-DD HH:MM:SS`
  (UTC) via `dal::time::now()` / dates as `YYYY-MM-DD` via `dal::time::today()`,
  and binds them — no `datetime('now')`/`CURRENT_TIMESTAMP` DB defaults.
- Date-window filters (was `date('now','-30 days')`) compute the cutoff string
  in Rust and bind it: `download_date >= ?`.

This removes every engine-specific time function.

### Upserts via SeaORM

`INSERT ... ON CONFLICT DO UPDATE` becomes `Entity::insert(..).on_conflict(..)`,
which SeaORM lowers to each backend's dialect (`ON CONFLICT` for sqlite/pg,
`ON DUPLICATE KEY UPDATE` for mysql).

### DAL module (`src/dal/`)

One submodule per aggregate; every function takes `&impl ConnectionTrait` so it
composes with both a pooled connection and a transaction (needed for the atomic
publish path). Handlers become thin: parse/authorize → call DAL → shape
response. No raw SQL or entity queries remain in `api/`, `web/`, `github*.rs`.

Submodules: `time`, `users`, `sessions`, `tokens`, `packages`, `versions`,
`deps`, `owners`, `downloads`, `oauth`, `reports`, `audit`, `sync_log`.

Any remaining raw SQL inside the DAL uses only standard SQL (joins, `GROUP BY`,
`COUNT`, `COALESCE`, `LIKE`) with bound parameters — portable across all three.

### Migrations (`src/migration/`)

SeaORM `MigratorTrait` with one migration per current `.sql` file, reproducing
the exact schema (TEXT for time columns). `db::connect()` dispatches on the URL
scheme, applies the WAL pragma only for SQLite, then runs the migrator.

## Verification

- `cargo test` green on SQLite at every commit.
- `make test-all-drivers` (Docker: sqlite + postgres + mysql) is the
  cross-engine oracle in CI.

## Stages

1. SeaORM migrations + engine-aware `connect()`; delete `.sql` + `sqlx::migrate!`.
2. `dal::time` + `dal` skeleton; move the engine-specific queries (downloads,
   oauth upserts, all `datetime('now')` writes) into the DAL.
3. Move the remaining reads/writes per aggregate; make handlers thin.
4. Docs (README multi-engine section), CHANGELOG.
