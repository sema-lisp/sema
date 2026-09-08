# Build-time investigation report

For the September 2026 follow-up on optimized local builds, see
[Build profile measurements](build-profiles-2026-09-08.md).

**Date:** 2026-07-17 · **Machine:** Apple Silicon macOS (Darwin 24.6), workspace at
`/Users/helge/code/sema` with the shared sccache config from `../.cargo/config.toml`
(incremental disabled, `rustc-wrapper = sccache`, 20G workspace-local cache).

Scope: the painful scenario is **agent edit→compile→test loops, often in parallel
worktrees**, which are slow and have caused disk exhaustion (real ENOSPC failures on
2026-07-16). Release/PGO builds run in CI and are out of scope.

All measurements are single runs (N=1) on a machine with light concurrent activity —
treat them as ±20%, not benchmarks. Build experiments ran in a scratch
`CARGO_TARGET_DIR` (and a temporary `jake wt-new` worktree, removed with `jake wt-rm`
afterwards) so the shared `target/` was never touched. Temporary marker-comment edits
to `sema-core`/`sema-stdlib` were reverted; the tree was left as found.

## Workload evidence (mined from Claude Code transcripts, last 14 days)

- Command volume: `cargo test` ≈ 3,665 occurrences, `cargo build` ≈ 824,
  `cargo nextest` ≈ 626, `cargo clippy` ≈ 575, `cargo check` ≈ 563. **The loop is
  test-dominated**, so `cargo test` cost matters more than `cargo build` cost.
- Observed dev-build durations: median 5.1s, p90 15.6s, max 55.7s, plus repeated
  cold builds in the 3m30s–5m range.
- Real `ENOSPC: no space left on device` failures on 2026-07-16 in the
  `unified-async-runtime` worktree (whose `target/` measured **38 GB** during this
  investigation, all artifacts <3 days old — `jake sweep` correctly refused to
  touch any of it).
- Parallelism peak: 71 session files (incl. subagents) active on 2026-07-15.

## Baseline measurements

Dev profile, warm sccache unless noted. "Real edit" = an actual content change
(appended comment), which — unlike `touch` — makes every downstream crate a genuine
recompile, exactly like an agent edit.

| # | Scenario | Wall time | Notes |
|---|----------|-----------|-------|
| S1 | Clean build, fresh target, cache-cold keys | **85s** | 572 units, 645s serial CPU (~7.6× parallelism); Rust hit rate 0% (cache at 20G cap had evicted these keys) |
| S1b | Same build, target wiped, identical keys | **28.5s** | Rust hit rate 100% — this is the sccache best case: link + build scripts + cargo overhead |
| S1c | Same source built from a **different worktree path** | **57s** | Rust hit rate **75.6%** (108 misses: workspace members + path-sensitive units). Cross-worktree caching is partial, not full |
| S2 | `touch` sema-core → `cargo build` | 6.4s | 100% hits; ~pure relink floor |
| S2c | **Real edit** to sema-core → `cargo build` | **18.0s** | 15 genuine recompiles (core + all dependents), 0% hits by construction |
| S3c | Real edit to sema-stdlib → `cargo build` | **11.2s** | |
| S2i | Real edit to sema-core, **incremental on, no sccache** | **8.1s** | 2.2× faster than S2c |
| S3i | Real edit to sema-stdlib, incremental | **5.6s** | 2× faster than S3c |
| S4a | Build 6 sample integration-test binaries (deps warm) | 53s | Binaries are 102–105 MB **each** |
| S4b | `touch` sema-core → relink those 6 test binaries | 14.3s | ≈ **2.4s pure link per binary** |

Disk per worktree: dev `target/` is 2.2–2.3 GB for the binary build alone; with
incremental caches 3.5 GB; each integration-test binary adds ~100 MB.

## Findings, ranked by impact

### 1. `crates/sema/tests` is 86 separate ~100 MB binaries — the dominant cost of the loop, and the ENOSPC engine

`crates/sema/tests/*.rs` = 86 files (47.8k lines), and cargo builds each as an
independent binary linking the entire workspace. Measured: ~100 MB and ~2.4s of link
time per binary. Extrapolated to a full `cargo test` after any core edit:

- **Link time: ~3.5 minutes** (86 × 2.4s) *before any test runs*, every iteration.
- **Disk: ~8.6 GB of test executables per worktree**, rewritten on every relink.
  Two or three parallel worktrees running `cargo test` ≈ 25+ GB of churn — this is
  the 2026-07-10 / 2026-07-16 ENOSPC story, not the dependency graph.

The transcripts show `cargo test` is the most-run command by 4×, so this multiplies
against everything else. Consolidating the 86 files into ~6–10 grouped harnesses
(e.g. `async_tests.rs` with `mod` includes per current file, `mcp_tests.rs`, …)
would cut link time to seconds and per-worktree disk by ~7–8 GB. Caveats: files
relying on per-process isolation (env vars, cwd, global state) need care;
`cargo-nextest` (already in use) runs tests process-per-test, which actually
*removes* most isolation concerns after consolidation.

### 2. sccache does not help the edit loop at all — and it blocks the thing that would

Measured behavior of the current policy (`incremental = false` + sccache):

- **Same-path, unchanged source: perfect** (100% hits, 28.5s full rebuild).
- **Cross-worktree: partial** — 75.6% hits; the ~108 misses are precisely the
  crates a fresh worktree must compile anyway (workspace members and
  path/`OUT_DIR`-sensitive units). A new worktree's first build is ~57s, not ~28s.
- **Edited code: zero help by design.** A changed crate and all its dependents are
  content-new, i.e. guaranteed misses. The edit loop runs at full non-incremental
  recompile cost (18s per core edit).
- **Incremental is not just disabled, it is broken under the wrapper**: sccache
  hard-fails on `-C incremental` (`process didn't exit successfully`, exit 1). An
  earlier experiment that "measured" 0.4s incremental rebuilds was actually a
  failing build — worth knowing if anyone re-tests this.
- **The 20G cache cap is at capacity and evicting**: the first clean-build
  measurement got 0% Rust hits for keys that were in the cache a day earlier.
  Under eviction pressure the "shared cache across worktrees" premise quietly
  degrades toward cold builds.

With incremental on (wrapper off), the measured edit loop is **2–2.2× faster**
(8.1s vs 18.0s core; 5.6s vs 11.2s stdlib), costing ~+1.3 GB per worktree in
incremental caches. See Recommendations for the policy options — this is a
workspace-level decision (`../.cargo/config.toml` + `../CLAUDE.md`), not something
to flip inside this repo.

### 3. Default `cargo build` compiles all 17 members — including `sema-wasm` natively

There is no `default-members` in `Cargo.toml`, so `jake build` / `cargo build`
compiles `sema-wasm` for the host target, pulling `js-sys` (17.6s), `web-sys`,
`wasm-bindgen` + macros (~6s each) into every clean/native build for zero benefit
(the real wasm build uses `--target wasm32-unknown-unknown` via its own recipe).
Adding `default-members` that exclude `sema-wasm` removes ~25–30s of serial unit
time from cold builds for free.

### 4. Dependency heavies on the cold path (~1/3 of serial compile time is avoidable or upstream-fixable)

From the `cargo build --timings` unit data (645s serial total):

| Cost | Unit(s) | Pulled in by | Note |
|------|---------|--------------|------|
| 43.0s | `aws-lc-sys` build script | `sema-otel → opentelemetry-otlp → reqwest → rustls → aws-lc-rs` | Single slowest unit in the graph. A ring-based TLS provider (or making the OTLP exporter an off-by-default feature) would eliminate it; `reqwest`'s default rustls provider is aws-lc |
| 26.2s | `lopdf` ×2 (18.7s + 7.5s) | `pdf-extract 0.10` pins lopdf 0.38 while the workspace uses 0.41 | Compiled **twice**; goes away when pdf-extract updates (or by replacing/vendoring the extraction path) |
| 21.4s | `lsp-types` | `tower-lsp` | Inherent to the LSP feature |
| ~25s | `image`, `moxcms`, `pxfm` | `libsui` (executable embedding for `sema build`) | libsui pulls `image` unconditionally (PE icon support); also pins old `object` 0.36 + `zerocopy` 0.7 (both duplicated). Upstream issue/PR candidate |
| ~30s | `tonic`, `h2`, `hyper-util`, `opentelemetry_sdk`, `opentelemetry-*` | `sema-otel` OTLP exporter | Feature-gating the exporter would drop the whole gRPC stack from default dev builds |
| 10.6s | `reedline` | REPL | Inherent |

Duplicate versions currently compiled: `rand` 0.8/0.9/0.10, `getrandom` ×3,
`hashbrown` 0.14/0.16/0.17, `bitflags` 1/2, `lopdf` ×2, `object` ×2, `zerocopy` ×2,
`derive_more` ×2, `nix` ×2, `itertools` ×2, digest/cipher stacks ×2. Most are forced
by third-party pins (pdf-extract, libsui) — a few tens of seconds of serial time,
worth revisiting when upstreams update but not urgent.

`cargo machete` findings: `rand` in `sema-mcp` appears unused
(`random_hex_token` uses `uuid`) — removable after a `cargo check -p sema-mcp`;
`sema-otel` in `sema-workflow` is flagged but doc comments claim it as a deliberate
dependency — verify before touching; the fuzz-crate hits are intentional.

### 5. Codegen health: fine — no monomorphization problem

`cargo llvm-lines` on the two biggest crates shows healthy profiles: sema-stdlib
788k lines / 27.9k functions with the largest single function at 0.2%; sema-vm's
largest is the expected VM dispatch (`run_inner`, 2 × ~10.8k lines / 5% each).
No generic-bloat fix will move the needle; nothing to do here.

`cargo bloat` (dev): the 143.6 MB `sema` binary is only 40.1 MB `.text`
(sema_stdlib 5.1 MB, sema_lsp 3.3 MB, sema_llm 2.0 MB; the rest is the long tail
of 290+ crates) — the other ~100 MB is symbol/debug-map overhead, which is also
roughly what each of the 86 test binaries carries. `debug = "line-tables-only"`
is already in place; consolidation (finding 1), not stripping, is the fix.

## Recommendations (in order)

1. **Consolidate the 86 integration-test files into ~6–10 grouped harness binaries.**
   Biggest single win: `cargo test` link cost drops from ~3.5 min to well under 30s
   per iteration, and per-worktree disk drops by ~7–8 GB. Mechanical but large;
   do it as a dedicated change, watching for tests that assume process isolation.
2. **Add `default-members` excluding `sema-wasm`** (and consider `sema-docs`) to
   `Cargo.toml`. One-line change, saves the native wasm-bindgen stack on every
   clean build. Wasm builds are unaffected (own target/recipe).
3. **Revisit the incremental-vs-sccache policy** (workspace-level decision,
   `../.cargo/config.toml`): the measured data says sccache's value is
   fresh-worktree bootstrap (57–85s → warm subsets) while costing 2–2.2× on every
   real edit — and the edit loop is the stated pain. A reasonable middle ground:
   incremental **on** + wrapper **off** for interactive/agent work, with disk
   growth controlled by scheduled `jake sweep days=1` (incremental caches measured
   at ~1.3 GB/worktree — small next to the 8.6 GB of test binaries). If the
   wrapper stays, raise `SCCACHE_CACHE_SIZE` above 20G — the cache is at cap and
   evicting, which is why a clean build measured 0% hits.
4. **Kill the `aws-lc-sys` 43s build script** — investigate a ring-based rustls
   provider for `reqwest`, or make `sema-otel`'s OTLP exporter (tonic + friends)
   a non-default feature so dev builds skip the gRPC stack entirely.
5. **Dependency hygiene, low priority:** drop unused `rand` from `sema-mcp`;
   verify `sema-otel` in `sema-workflow`; upstream nudges for `pdf-extract`
   (lopdf 0.41) and `libsui` (object 0.37, zerocopy 0.8, optional `image`).
6. **Disk ops:** the ENOSPC events came from per-worktree test-binary bloat plus a
   38 GB active worktree target that `jake sweep` (correctly) won't touch because
   the artifacts are fresh. Recommendation 1 shrinks the per-worktree footprint at
   the source; during multi-worktree crunches run `jake sweep days=1` and prefer
   `jake wt-rm` promptly when a worktree is done.

### Not worth pursuing (checked and cleared)

- Monomorphization/generics cleanup (`cargo llvm-lines` is healthy).
- A faster linker (modern Xcode ld is fine; link cost is binary *count*, not speed).
- `cargo-machete`-style dead-dependency purges beyond the two items above — the
  dependency list is clean.
- Changing debug-info settings — `line-tables-only` is already the right call.

## Tools added / used

| Tool | Status | Role |
|------|--------|------|
| `cargo build --timings` | built-in | Unit timeline + critical path (used for S1; report saved under `target/cargo-timings/`) |
| `cargo-llvm-lines` | **installed this session** (`cargo install cargo-llvm-lines`) | Monomorphization audit |
| `cargo-machete` | already installed | Unused-dependency audit |
| `cargo-bloat` | already installed | Binary size by crate |
| `hyperfine` | already installed | Whole-command timing (use for before/after once changes land) |
| `sccache --zero-stats` / `--show-stats` | already installed | Hit-rate measurement per scenario |

Future/CI (separate track, not started): CodSpeed or Iai-Callgrind on a Linux
runner for runtime-performance regression gating.

## Changes applied (2026-07-17, branch `build-time-fixes`)

All six recommendations were applied and verified (full workspace tests + `jake
lint` + `jake examples` + `jake smoke-bytecode` + `jake docs-check` green; live
HTTPS request exercised end-to-end):

1. **Test consolidation: 86 → 42 binaries.** 53 files moved to
   `crates/sema/tests/suites/` as modules of eight `*_suite.rs` harnesses (eval,
   vm, async, llm, mcp, server, workflow, misc). 34 files stay standalone: the 24
   `sema_otel::testing::install()` files (process-global by design),
   `integration_test`, `leak_test` (its teardown oracles assert process-wide
   state — it failed inside vm_suite and was moved back out), the workflow-mcp
   e2e files, `git_async_test` (cwd mutation), and `embedding_bench`/`http_test`/
   `llm_test` (jake recipes target them by name). Test parity proven:
   `cargo test -- --list` yields **4,338 tests before and after** with identical
   leaf names. Convention documented in AGENTS.md: new test files go into a
   suite unless they need process isolation.
2. **Workspace policy: incremental ON, sccache wrapper OFF**
   (`../.cargo/config.toml`, workspace CLAUDE.md updated). Measured edit loop:
   core edit 18.0s → 8.1s, stdlib edit 11.2s → 5.6s. For one-off cached clean
   builds: `CARGO_INCREMENTAL=0 RUSTC_WRAPPER=sccache cargo build`.
3. **`default-members`** now excludes `sema-wasm` — native builds no longer
   compile js-sys/web-sys/wasm-bindgen (~25s serial). `cargo build -p sema-wasm`
   still works.
4. **TLS: aws-lc-sys eliminated** (was the single slowest unit, 43s build
   script). reqwest runs `rustls-no-provider` + the pure-Rust ring provider.
   Ring must be *installed* at runtime: every `reqwest::Client` (and
   tokio-tungstenite connect) construction site calls its crate's
   `ensure_crypto_provider()` guard (sema-llm http.rs, sema-stdlib http.rs,
   sema-mcp lib.rs, sema-otel imp.rs, sema main.rs + test helpers). Verified by
   a live `(http/get "https://example.com")`. **Gotcha:** a new construction
   site without the guard panics `No provider set` at runtime — rule recorded
   in AGENTS.md.
5. **OTLP gRPC behind a non-default feature.** `sema-otel/grpc` (→ sema-lang
   `otel-grpc`) gates the tonic/h2 stack (~30s serial). Release artifacts enable
   it via `features = ["otel-grpc"]` in dist-workspace.toml; a grpc-less build
   warns once and falls back to http/protobuf (docs updated in
   `website/docs/llm/observability.md`). Plain `cargo install sema-lang` needs
   `--features otel-grpc` for gRPC.
6. **Dependency diet:** `pdf-extract` 0.10 → 0.12 + `lopdf` 0.42
   `default-features = false` (kills the duplicate lopdf compile and the
   jiff/chrono/time/rayon transitives, ~30s serial); workspace unified on
   `zip 8` minimal (`deflate`+`deflate64` only — zstd/bzip2/AES archives now
   error at runtime, accepted trade-off); `libsui` 0.15 → 0.16 (Mach-O/ELF
   fixes); unused `rand` dropped from sema-mcp; unused `sema-otel` dropped from
   sema-workflow (now truly sema-core + serde leaf).

### Measured results (same machine, post-change, cargo clean first)

| Scenario | Before | After | Δ |
|---|---|---|---|
| Clean dev build | 85s (cold keys, warm sccache) | **33.6s** (fully cold, no cache at all) | 2.5× |
| Total compile work (user time) | ~645s serial | **195s** | 3.3× |
| Real edit to sema-core → `cargo build` | 18.0s | **3.6s** | 5× |
| Real edit → rebuild+relink ALL test binaries | ~3.5min (86 × 2.4s links) | **23.2s** | ~9× |
| Full `cargo test --no-run` from clean | n/a (measured 9.9G build) | 48.8s | |
| `target/` with full test build | 9.9G | 11G | see note |

Note on disk: the static footprint is a wash — the ~4.3G saved by 44 fewer test
binaries is offset by incremental caches (which the old policy banned). The disk
win is dynamic: each edit→test iteration now rewrites ~4.4G of binaries instead of
~8.6G, and the 5–9× faster loop means worktrees live (and are `jake wt-rm`'d)
sooner. Sweep discipline (`jake sweep days=1` under pressure) still matters.

### Follow-up round (same day)

- **`pio` dev-dependency removed** (the RP2040 PIO reference assembler pulled
  `pio-proc`, a 6MB lalrpop proc-macro stack, into every test build). Its
  reference encodings are frozen into `crates/sema/tests/fixtures/pio_golden.json`,
  generated once by `tools/pio-golden` — a tiny crate deliberately excluded from
  the workspace. All 72 cross-validation tests pass against the fixture; the
  oracle stays the real `pio` crate, just evaluated at regeneration time instead
  of every build.
- **`sema build` Windows executables are now branded**: the canonical
  `sema-mark-rounded` icon (self-tiled, so it reads on light and dark themes —
  PE icons cannot adapt to theme) is embedded via `libsui::set_icon`
  (512px master → 256→16px ICO set), and a
  `VERSIONINFO` resource (ProductName/FileDescription/FileVersion) is added via
  `editpe` — the PE editor libsui already uses internally, so zero new compile
  cost. This turns libsui's previously-dead `image` dependency into a feature.
  Verified against a real v1.30.0 Windows runtime: icon set, version strings,
  and the `semaexec` payload all read back correctly with `pefile`.

Research verdicts for the rest (2026-07-17): keep `scraper` (nothing lighter
parses messy HTML + CSS selectors; alternatives sit on the same html5ever
stack), keep `indicatif` (one call site, tiny tree), keep `notify` (bump to v8
when convenient, not a compile win), upstream-PR opportunity: `libsui` gating
its unconditional `image` dep behind an `icon` feature.

## Reproducing the measurements

```bash
# Clean-build timing + unit report (use a scratch target to protect the shared one)
export CARGO_TARGET_DIR=/tmp/sema-bench-target
sccache --zero-stats && cargo build --timings && sccache --show-stats

# Real-edit loop (marker line makes downstream crates genuine recompiles; revert after)
echo '// __marker__' >> crates/sema-core/src/lib.rs
sccache --zero-stats && time cargo build && sccache --show-stats
sed -i '' '/__marker__/d' crates/sema-core/src/lib.rs

# Incremental comparison (wrapper must be off: sccache hard-fails on -C incremental)
CARGO_INCREMENTAL=1 RUSTC_WRAPPER='' cargo build

# Test-binary link cost / sizes
cargo test -p sema-lang --no-run --test integration_test --test eval_test
ls -lS "$CARGO_TARGET_DIR"/debug/deps | head

# Monomorphization / size
cargo llvm-lines -p sema-stdlib | head -20
cargo bloat -p sema-lang --crates -n 15
```
