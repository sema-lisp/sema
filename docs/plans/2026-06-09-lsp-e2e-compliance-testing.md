# In-Crate Rust E2E + LSP Spec-Compliance Suite for sema-lsp

**Date:** 2026-06-09
**Status:** Deferred — revisit after the IntelliJ-plugin integration test track lands. Filed now so the research isn't lost.
**Related:** `2026-05-15-lsp-audit-harness-design.md` (corpus/oracle-based correctness auditing — complementary: that design proves features are *correct* against real code; this plan proves the server is *protocol-compliant and robust* at the wire level, in Rust, in CI).

## Context

LSP testing today is three disconnected layers:

1. **Python pytest suite** (`crates/sema-lsp/tests/e2e/`, 17 files, ~46 tests, pytest-lsp): per-feature happy paths. Weaknesses: `asyncio.sleep(0.3)` races after every didOpen (`helpers.py:33`), a manual kill-timeout workaround for tower-lsp#399 (`conftest.py:57-66`), and **not run in CI** (only `make test-lsp`; ci.yml never invokes pytest).
2. **Rust smoke test** (`crates/sema-lsp/tests/lsp_e2e_test.rs`): spawns the built binary over stdio with hand-rolled JSON-RPC framing; one test covering capability advertisement + formatting + selectionRange round-trips.
3. **IntelliJ plugin integration tests** (active work, LSP4IJ + Starter framework): real-client coverage, but JetBrains-specific and slow.

There is **no official LSP conformance suite** to adopt: the spec's metaModel.json is for codegen, not wire validation, and strict metaModel-driven validation breaks on open enumerations (LSP issue #1847). Mature Rust servers (rust-analyzer, texlab, taplo) all roll fixture-based in-process tests; "compliance" in practice = hand-written lifecycle/capability tests + strict typed deserialization as a shape oracle.

Key architectural facts that shape the design:

- `BackendState` (`crates/sema-lsp/src/lib.rs:1689`) runs on a dedicated thread with ~25 synchronous `handle_*` methods returning plain `lsp-types` values — a perfect in-process test seam, currently private.
- The wire path has custom logic that in-process handler tests would miss: `normalize_lsp_input` (lib.rs:~4580, hand-rolled Content-Length re-framer) and backend-thread message ordering/coalescing (lib.rs:~4272-4330).
- `run_server()` hardcodes stdio (lib.rs:~4252); tower-lsp's `Server::new` accepts any `AsyncRead`/`AsyncWrite`, so a `serve(input, output)` refactor enables full in-process wire tests over `tokio::io::duplex`.
- Known compliance gaps to target: no `positionEncoding` negotiation (UTF-16 assumed, never tested end-to-end with astral chars), `$/cancelRequest` cannot interrupt in-flight backend work, and the 19 advertised capabilities have no honesty-matrix test.

## Architecture — three layers, all in-crate Rust

### Layer 1 — Feature correctness (bulk of tests): in-process `BackendState` tests

- Expose `BackendState` + `handle_*` behind a `test-support` cargo feature (or `#[doc(hidden)] pub mod test_support`).
- texlab-style fixture parser in `tests/support/`: multi-document fixtures with `%! name.sema` headers; annotation-only lines carry `|` (cursor), `^^^` (range), `!` (point) markers stripped from text. Proven format, diff-friendly, Lisp-friendly.
- Assertions: `insta` snapshots for rich payloads (hover markdown, semantic-token arrays, completion lists); direct assertions for positions. No tokio, no sleeps, sub-ms per test.
- Prior art: rust-analyzer ide-crate fixture tests, texlab `crates/test-utils/src/fixture.rs`.

### Layer 2 — Protocol/lifecycle wire tests: in-process duplex harness

- Refactor `run_server()` → `pub async fn serve(input: impl AsyncRead, output: impl AsyncWrite)`; `run_server` becomes a 3-line stdio wrapper.
- Harness: `tokio::io::duplex` + a small typed client (`send_request<R: lsp_types::request::Request>`, `wait_for_response`) — promote the framing helpers already in `lsp_e2e_test.rs`.
- Exercises the *real* production path (normalizer + LspService routing + backend thread + ordering) without subprocess flakiness. rust-analyzer does the equivalent with `Connection::memory()` in its slow-tests.
- Keep exactly **one** spawned-binary smoke test (trimmed `lsp_e2e_test.rs`) for what only a process verifies: stdio behavior, exit codes, the tower-lsp#399 hang regression.

### Layer 3 — Compliance suite (one table-driven test file)

- **Capability honesty matrix:** for every advertised capability, issue the matching request → assert no `-32601 MethodNotFound`; for a fixed list of *unadvertised* methods (codeAction, typeDefinition, implementation, onTypeFormatting, …) → assert exactly `-32601`. Derive the table from the parsed `ServerCapabilities` so it can't rot.
- **Lifecycle:** request before `initialize` → `-32002 ServerNotInitialized`; double `initialize` → InvalidRequest; requests after `shutdown` → InvalidRequest; `exit` semantics (code 0 after shutdown, 1 without — binary smoke test); `$/cancelRequest` on pending request → `-32800 RequestCancelled`.
- **Position encoding:** wire-level fixtures with astral chars/emoji (`(define 🎉x …)`) asserting hover/definition/rename ranges in UTF-16 code units; differential oracle = `read_many_with_spans_recover` spans through the conversion helpers. Server must negotiate `positionEncoding` or behave correctly with a utf-16-only client.
- **Shape oracle:** strict deserialization of every response through `lsp-types` structs (already a dep) — the pragmatic spec-shape check. Do NOT build a metaModel-driven validator (open-enum breakage, see #1847).
- **Robustness:** malformed frames into `normalize_lsp_input` (split/missing Content-Length, oversized body), unknown notifications (ignored per spec), out-of-range positions (no panic; null/empty response).

## Python suite: reuse vs retire

- **Retire** `crates/sema-lsp/tests/e2e/` once Layers 1-2 reach parity — not in CI, sleep-based races, real-client angle covered by the IntelliJ track.
- **Reuse as the migration checklist:** port each of the ~46 assertions into Layer-1 fixture tests. The conftest shutdown-timeout workaround becomes the Layer-2 binary smoke regression test.

## Phases

1. **Harness foundation** — `test-support` feature exposing `BackendState`; `run_server` → `serve(io)` refactor; fixture parser + duplex client in `tests/support/`; migrate `lsp_e2e_test.rs` onto the harness keeping one binary smoke test. *Exit: `cargo test -p sema-lsp` runs both harnesses in CI with zero sleeps.*
2. **Feature migration** — port all Python feature tests to Layer-1 fixtures with insta snapshots; small `.sema` fixture corpus (unicode, f-strings, regex literals, error-recovery cases). *Exit: Python suite deletable.*
3. **Compliance suite** — capability matrix, lifecycle ordering, cancel, unknown methods/notifications, strict typed deserialization.
4. **Encoding + robustness** — UTF-16 astral wire tests, reader-span differential checks, framing fuzz cases, out-of-range no-panic sweep.
5. **CI hardening + retirement** — delete Python e2e dir, update `make test-lsp`, verify ci.yml coverage, document fixture format.

## Tools

| Tool | Role | Why |
|---|---|---|
| `tokio::io::duplex` (existing dep) | in-process wire transport | kills subprocess flakiness; rust-analyzer-equivalent pattern |
| `insta` | snapshot assertions | `cargo insta review` workflow for multi-line payloads (alt: `expect-test`, lighter) |
| `lsp-types` (existing dep) | typed shape oracle | strict deserialization = practical spec-shape validation |
| `serde_json` | wildcard JSON matching | port rust-analyzer's `find_mismatch` `[..]` pattern for loose assertions |
| texlab fixture format | fixture syntax | proven `%!` / `\|` / `^` markers |

**Risk note:** ebkalderon/tower-lsp is dormant (the Python conftest already works around tower-lsp#399); maintained fork is `tower-lsp-community/tower-lsp-server`. This test architecture is transport-agnostic and survives a migration — a point in its favor.

## Sources

- rust-analyzer slow-tests: https://github.com/rust-lang/rust-analyzer/blob/master/crates/rust-analyzer/tests/slow-tests/support.rs
- texlab fixture format: https://github.com/latex-lsp/texlab/blob/master/crates/test-utils/src/fixture.rs
- tower-lsp (generic IO via Server::new): https://github.com/ebkalderon/tower-lsp · fork: https://github.com/tower-lsp-community/tower-lsp-server
- pytest-lsp / lsp-devtools (spec-warning machinery): https://lsp-devtools.readthedocs.io/
- LSP 3.17 metaModel: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/metaModel/metaModel.json
- metaModel strictness pitfall: https://github.com/microsoft/language-server-protocol/issues/1847
