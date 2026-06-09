# CLI ↔ Registry Integration — Required Changes

> What needs to change in the `sema pkg` CLI and `sema-core` resolver after the registry (pkg.sema-lang.com) is implemented.

## Current State

The CLI is **git-only**: `sema pkg add github.com/user/repo@v1.0` clones the repo into `~/.sema/packages/github.com/user/repo` and checks out the ref. Resolution, listing, updating — all backed by git repos on disk.

The registry introduces a **tarball-based** distribution model with an HTTP API. Both models need to coexist.

---

## 1. New CLI commands

### `sema pkg login`

Authenticate with a registry to get/store an API token.

```
sema pkg login                          # login to pkg.sema-lang.com
sema pkg login https://my-registry.com  # login to a self-hosted registry
```

Flow:
1. Prompt for username + password (or open browser for OAuth)
2. `POST /api/v1/auth/login` → get session cookie
3. `POST /api/v1/tokens` with `{name: "cli-<hostname>"}` → get `sema_pat_...` token
4. Store token in `~/.sema/credentials.toml`:
   ```toml
   [registries]
   "pkg.sema-lang.com" = "sema_pat_..."
   "my-registry.com" = "sema_pat_..."
   ```

**Files:** `crates/sema/src/pkg.rs` (new `cmd_login`), `crates/sema-core/src/home.rs` (credentials path helper), `crates/sema/src/main.rs` (add `Login` to `PkgCommands`)

### `sema pkg logout`

Remove stored token for a registry.

**Files:** same as login

### `sema pkg publish`

Package the current directory into a tarball and upload to the registry.

```
sema pkg publish                         # publish to pkg.sema-lang.com
sema pkg publish --registry https://...  # publish to self-hosted
```

Flow:
1. Read `sema.toml`, validate `[package]` has `name` and `version`
2. Create tarball of current directory (respecting `.gitignore` or a `sema.ignore`)
3. Read token from `~/.sema/credentials.toml`
4. `PUT /api/v1/packages/{name}/{version}` with multipart tarball + metadata
5. Print success with checksum

**Files:** `crates/sema/src/pkg.rs` (new `cmd_publish`), `crates/sema/src/main.rs` (add `Publish` to `PkgCommands`)

### `sema pkg search`

Search the registry from the CLI.

```
sema pkg search http
```

Flow: `GET /api/v1/packages?q=http` → print results table

**Files:** `crates/sema/src/pkg.rs` (new `cmd_search`), `crates/sema/src/main.rs`

### `sema pkg yank` / `sema pkg unyank`

Yank a published version (prevent new installs, existing installs keep working).

```
sema pkg yank my-package@0.1.0
```

**Files:** `crates/sema/src/pkg.rs`, `crates/sema/src/main.rs`

---

## 2. Changes to `sema pkg add`

Currently `cmd_add` is hardcoded to `git clone` + `git checkout`. After the registry, it needs a **dual-source** strategy:

### Resolution order

1. **Registry first** — if the package name exists on the configured registry, download the tarball for the requested version
2. **Git fallback** — if the spec looks like a git URL (`github.com/...`), fall back to direct git clone (current behavior)

### How registry install works

```
sema pkg add http-helpers@1.0.0
```

1. `GET /api/v1/packages/http-helpers/versions` → find version `1.0.0`
2. `GET /api/v1/packages/http-helpers/1.0.0/download` → download tarball
3. Extract to `~/.sema/packages/http-helpers/` (note: **short name**, not `github.com/user/repo` path)
4. Write a marker file or metadata so `sema pkg list` and `sema pkg update` know this came from a registry, not git

### Impact on `PackageSpec`

Currently `PackageSpec` assumes the path doubles as a git clone URL (`clone_url()` → `https://{path}.git`). Registry packages use **short names** (e.g., `http-helpers`) without a host prefix. Changes needed:

- `PackageSpec` needs a `source` field: `Git { url }` or `Registry { name, registry_url }`
- `PackageSpec::parse()` needs to distinguish `github.com/user/repo@v1` (git) from `http-helpers@1.0.0` (registry)
- Heuristic: if the spec contains a `.` in the first segment (looks like a hostname), treat as git; otherwise treat as registry short name
- Or: add a `--git` flag to force git mode: `sema pkg add --git github.com/user/repo@main`

**Files:** `crates/sema-core/src/resolve.rs` (PackageSpec changes), `crates/sema/src/pkg.rs` (cmd_add dual path)

---

## 3. Changes to `sema pkg install`

Currently reads `[deps]` from `sema.toml` and calls `cmd_add` for each. This still works, but:

- Deps from the registry will use **semver version requirements** (e.g., `"^1.0"`), not git refs
- Git deps will keep using refs (e.g., `"main"`, `"v1.0.0"`)
- Need to distinguish between the two in `sema.toml`

### Proposed `sema.toml` format

```toml
[deps]
# Registry packages — short name = semver requirement
http-helpers = "^1.0.0"
json-schema = "~2.1"

# Git packages — quoted URL = git ref (current format)
"github.com/user/private-lib" = "main"
```

Heuristic: if the key contains `/`, it's a git dep. Otherwise it's a registry dep. This is backwards-compatible with the current format.

**Files:** `crates/sema/src/pkg.rs` (`cmd_install` needs to route to registry vs git)

---

## 4. Changes to package resolution (`resolve.rs`)

Currently `resolve_package_import_in` looks up packages by their full path (`github.com/user/repo`) under `~/.sema/packages/`. Registry packages will be stored by **short name** (`http-helpers`). The resolver already handles this — it just joins the spec to the packages dir — but:

- `is_package_import()` needs updating: currently requires `/` in the spec. Registry short names like `http-helpers` (no `/`) would not be recognized as package imports
- Either relax `is_package_import()` to also match known installed packages, or use a different signal (e.g., check if the name exists in `~/.sema/packages/` as a directory)
- `validate_package_spec()` rejects specs without `/` (empty segment check). Needs to allow single-segment names for registry packages

**Files:** `crates/sema-core/src/resolve.rs`

---

## 5. Changes to `sema pkg update`

Currently runs `git pull` on each package directory. Registry packages don't have a `.git` directory, so:

- Detect source type: check for `.git/` dir → git update; otherwise → registry update
- Registry update: `GET /api/v1/packages/{name}/versions` → check if newer version available within semver constraint → download and replace
- Or: `sema pkg update` only updates git packages; registry packages are pinned and updated explicitly with `sema pkg add name@new-version`

**Files:** `crates/sema/src/pkg.rs` (`cmd_update`)

---

## 6. Changes to `sema pkg list`

Currently uses `current_git_ref()` to show the checked-out ref. Registry packages need a different version display:

- Store version metadata in `~/.sema/packages/<name>/.sema-pkg.json` or similar:
  ```json
  {"source": "registry", "registry": "pkg.sema-lang.com", "version": "1.0.0", "checksum": "sha256:..."}
  ```
- `cmd_list` reads this metadata for registry packages, falls back to git ref for git packages

**Files:** `crates/sema/src/pkg.rs` (`cmd_list`)

---

## 7. Changes to `sema pkg remove`

Current implementation works for both — it just `rm -rf`s the directory and cleans up `sema.toml`. No changes needed beyond supporting both key formats in `remove_dep_from_toml` (short names and full paths).

---

## 8. HTTP client dependency

The CLI currently has **no HTTP client** — it only shells out to `git`. Registry operations need one:

- Add `reqwest` (or `ureq` for sync/smaller binary) to `sema` crate's `Cargo.toml`
- Or use `curl` via `Command` to avoid the dependency (ugly but simple)
- `reqwest` is already used in `sema-llm`, but that's a separate crate. The `sema` binary already transitively depends on `tokio` via `sema-llm`, so `reqwest` is reasonable.

**Files:** `crates/sema/Cargo.toml`

---

## 9. Registry configuration

Need a way to configure which registry to use:

- Default: `pkg.sema-lang.com`
- Override per-project in `sema.toml`:
  ```toml
  [package]
  registry = "https://my-company-registry.com"
  ```
- Override globally in `~/.sema/config.toml`:
  ```toml
  default-registry = "https://my-company-registry.com"
  ```
- Override per-command with `--registry` flag

**Files:** `crates/sema-core/src/home.rs` (config path), `crates/sema/src/pkg.rs` (registry URL resolution)

---

## Summary: implementation order

```
Phase 1 — Registry-aware install (read-only)
  1. Add HTTP client dep (reqwest/ureq)
  2. Add registry config + credentials storage
  3. Update PackageSpec to support short names + registry source
  4. Update is_package_import() and validate_package_spec() for short names
  5. Update cmd_add to try registry first, fall back to git
  6. Update cmd_install to route deps by type
  7. Update cmd_list to show registry package versions
  8. Update cmd_update to handle registry packages

Phase 2 — Publishing (write)
  9. Add sema pkg login / logout
  10. Add sema pkg publish
  11. Add sema pkg yank / unyank
  12. Add sema pkg search
```

Phase 1 is the critical path — it's what end users need first. Phase 2 is only needed by package authors.
