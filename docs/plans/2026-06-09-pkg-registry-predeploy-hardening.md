# Package Registry Pre-Deploy Hardening

**Date:** 2026-06-09
**Status:** Pending — gated on redeploying the registry. `pkg.sema-lang.com` is currently down (DNS resolves to Vercel, returns `DEPLOYMENT_NOT_FOUND`; the fly app is not serving), so none of this is operationally urgent. It becomes a blocker the moment the registry is (re)deployed.
**Supersedes:** `docs/archived/2026-02-23-pkg-hardening.md` Task 7. Group A (CLI-side tasks 1-6) of that plan shipped; its Task 7 was written against the pre-SeaORM sqlx codebase and is no longer executable as written. This plan contains only the items still live after the SeaORM rewrite, re-verified 2026-06-09.

## 1. Publish transaction must cover version + dependency inserts (MUST-FIX, correctness bug)

The transaction in `pkg/src/api/packages.rs:140-191` covers only package + owner creation. The version insert (`packages.rs:224`) and dependency inserts (`packages.rs:238-246`) run directly on `&state.db` outside any transaction — and dep-insert errors are silently swallowed:

```rust
let _ = new_dep.insert(&state.db).await;
```

Failure mode: a version row commits while some or all dependency rows are missing → clients resolve the package with silently dropped deps. The `UNIQUE(package_id, version)` constraint (`pkg/migrations/001_initial.sql:52`) prevents duplicates but does nothing for this.

Fix: one SeaORM transaction around package + owner + version + all dependency inserts; propagate dep-insert errors (no `let _`). On any failure, nothing commits.

## 2. Tarball gzip magic-byte validation (cheap, same PR)

No `1f 8b` check anywhere in `pkg/src/` — any authenticated publish can store arbitrary bytes as a "tarball". Mitigated client-side (CLI's safe extractor fails cleanly on non-gzip; locked installs verify sha256), so severity is pollution, not exploitation.

Fix: reject uploads whose first two bytes aren't `1f 8b` before blob store. NOTE: registry integration tests currently publish `b"fake tarball data"` (`pkg/tests/integration_test.rs:224` and ~10 more sites) — fixtures must switch to minimal real gzip bytes.

## 3. Dependency-count cap + version_req validation (cheap, same PR)

- `pkg/src/config.rs` has `max_tarball_bytes` but no `max_dependencies` — deps are inserted with no count cap. Add a configurable cap (default e.g. 64).
- `version_req` strings are stored unvalidated. Validate with the `semver` crate (already a workspace dep since the Group A work) at publish time; reject invalid requirements.

## 4. blob::store panics on IO error (found during review)

`pkg/src/blob.rs:18,23` uses `.expect()` — a full disk or permission error panics the request handler. Convert to proper error returns surfaced as 500s.

## Explicitly dropped from the old plan

- **Blob delete on failed publish** — moot: blobs are content-addressed (`sha256.tar.gz`), so a failed publish leaves at worst one orphaned file that a retry reuses. If disk growth ever matters, the right shape is a GC sweep (also covering yanked versions), not per-failure cleanup. Parked as a someday item.

## Done When

- Publish is atomic: induced dep-insert failure leaves no version row (integration test)
- Non-gzip upload rejected with 4xx (integration test, fixtures updated to real gzip)
- Publish with > max_dependencies or invalid `version_req` rejected with 4xx
- No `.expect()` on IO paths in `blob.rs`
- All existing `pkg/tests/` pass
