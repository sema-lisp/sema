# Deferred items

> **Resolved items live in [`deferred-resolved.md`](plans/archive/deferred-resolved.md).** This
> file is only work that could still be picked up; anything finished, killed, or
> decided-against was split out on 2026-07-27.


Things that came out of the May 2026 quality sweep (Wave 6 audit) but were intentionally not fixed because they're too risky, too design-dependent, or have a cheap workaround. Each entry says *why* it's deferred so a future pass can decide whether to revisit.

## MCP-1 — Named/aliased MCP servers

**Found 2026-07-01, during the MCP client PR (#59).** Every `mcp/connect` and `sema mcp login/logout` repeats the full server config (`:url`/`:command`). A convenience layer would let you declare a server once — a `name → {:url …}`/`{:command …}` mapping (in a script or a small config file) — and refer to it by name (`(mcp/connect "asana")`, `sema mcp login asana`). Pairs naturally with the token store, which already keys by canonical URL. **Deferred because** it's a pure ergonomics feature with a design choice (script-level form vs. a config file), orthogonal to the client's correctness, and best done after the base client lands. Note: `sema mcp list` exists (MCP-2, resolved 2026-07-29) but has no alias/declared-by column — the alias registry here would supply it.

## MCP-3 — Fully-offline agent replay (cassette `tools/list` + `connect` skip)

**Found 2026-07-01 (PR #59, M5 cassettes).** MCP `tools/call` results record/replay through the shared cassette, so agent tool *calls* replay offline. But `mcp/connect` (and its `initialize`/`tools/list`) still runs live on replay, so a fully server-less agent-session replay isn't possible yet — you still need the stdio server or the HTTP endpoint reachable to establish the connection and enumerate tools. Extending the cassette to record `tools/list` and short-circuit `connect` on replay would close this. **Deferred because** the common case (deterministic *call* replay for CI) is covered; connect/list recording is a larger seam (identity keying for the handshake, and for remote servers the OAuth/discovery legs) that isn't needed for the value M5 delivers.

Also noted from the PR #59 merge review as low-priority, not-yet-done: capping the device-flow `slow_down` interval growth (the `+5` itself is RFC 8628-correct), and auto-reconnecting a Streamable-HTTP session on a mid-session `404` (currently surfaced as a `reconnect required` error rather than transparently re-initializing).

---


## ERR-1 — Arity errors do not carry an `in: (f 1 2 3)` note

**Today:** an arity error prints the source snippet under `-->` and the
`at f (<file>:line:col)` frame, which already shows the call. The test
`test_arity_error_shows_call_form` (`crates/sema/tests/integration_test.rs`)
additionally expects a `note()` of the form `in: (f 1 2 3)` and is `#[ignore]`d.

**Why deferred:** the VM raises the arity error with a span but without the
source text; reconstructing the call form would need the reader's source map at
runtime. The snippet under `-->` gives the same information, so the note is
low value. Revisit if error reports move to a structured format where a note
is cheaper than a snippet.

---

## D5 — Typed `try`/`catch` form

**Today:** `(try expr (catch e ...))` catches *every* error type, including `:unbound`, `:arity`, `:type-error` — the kind of errors that usually mean a typo. The docs (`website/docs/language/special-forms.md` near "Re-throw errors you don't intend to handle") explicitly warn about this.

**The bug shape:** silent bug-masking. A typo inside `try` is swallowed and the catch block runs as if the operation failed for "real" reasons.

**Proposed fix (not done):** add `(catch [:user :type-error] e ...)` syntax that filters by the `:type` field, mirroring Clojure's `catch ExceptionType` or Common Lisp's `handler-case`. Optionally lint-warn on the un-filtered form.

**Why deferred:** non-trivial language design. Affects reader (new pattern in catch clause), special-form lowering in both backends, and prelude macros that use `try`. Needs an ADR before code.

**Workaround today:** users can do `(try ... (catch e (if (= (:type e) :user) (handle e) (throw e))))` to re-raise unexpected errors. That's a documented pattern in special-forms.md.

---

## TOOL-2 — Speed up CI drastically (it's painful)

**Deferred (revisit later) — 2026-06-22.** A release cycle takes painfully long: the
`verify` gate (full `cargo test --workspace` + examples + smoke-bytecode + lint +
docs-check) runs ~12–15 min on a **cold** cache, and it runs **per workflow** (CI on the
branch push, `publish.yml` verify on the tag, `publish-npm.yml` verify on the tag) — so a
release re-builds the world several times. Observed leads for a future push:

- **Caching is the big lever.** `Swatinem/rust-cache` keys per *job*, so each workflow's
  verify job has its own (often cold) cache; warm it / share it, or move to `sccache`
  with a shared backend. Cold-cache full builds are the dominant cost.
- **Split the gate for fast-fail.** Run `fmt` + `clippy` + `docs-check` as a quick job
  that fails in ~1 min; run the heavy `cargo test`/examples/smoke separately and in
  parallel (test sharding via `cargo-nextest --partition`).
- **Don't re-verify per registry.** crates.io and npm publishes each gate on `verify`
  today (kept separate because npm's OIDC whitelists the workflow *filename* — see
  `publish-npm.yml`). Find a way to share one verify result across both without breaking
  the OIDC filename match (e.g. a reusable verify that both `needs:`, gated so it runs
  once per SHA).
- **Faster runners.** GitHub's free runners are 2 vCPU. Managed drop-ins that work on a
  *personal* account (not just orgs): **Namespace** and **Ubicloud** (Blacksmith is
  org-only). ~2–3× wall-clock on a compile-heavy Rust suite.
- **cargo-dist Windows flakiness** (separate but related): the Windows build intermittently
  fails fetching from crates.io; mitigated by `.cargo/config.toml` (`[http] multiplexing
  = false`, `[net] retry = 10`) — keep an eye on whether that's enough.

---

## CASS-1 — Cassette tape corpus + replay-in-CI (cassettes M4)

**Deferred (revisit later) — 2026-06-22.** Cassette M1–M3 shipped in 1.23.0 (record/replay
for `complete`/`chat`/`extract`/agents/streaming/embeddings; `with-cassette` + `llm/cassette-*`
+ env vars). M4 — making the LLM/agentic suite run keyless in CI off committed tapes — is
unstarted. The implementation plan was archived to `docs/plans/archive/2026-06-21-llm-cassettes.md`.
Remaining work:

- **Record a tape corpus** for the playground `llm-tools` examples and the agentic test
  suite; wire `SEMA_LLM_CASSETTE_MODE=replay` into `jake test` so the suite runs green with
  no API keys. (The keyless oracle today is the scripted `FakeProvider`; cassettes would add
  real-response replay on top.)
- **Open questions** carried from the plan: a `NullProvider` inner so pure-replay needs zero
  credentials; tape versioning/migration when `ChatRequest`/`ChatResponse` shapes change (the
  `"v":1` field is the hook); tapes beside tests (`tests/tapes/`) vs. a top-level `cassettes/`
  (leaning beside-tests); one-tape-per-test vs. shared (leaning per-test).

---

## LLM-1 — LLM bulletproofing remnants (from the archived plan)

**Deferred (revisit later) — 2026-06-22; trimmed 2026-07-28.** The bulletproofing plan
(`docs/plans/archive/2026-06-21-llm-bulletproofing.md`) shipped Phases 0–3, 4.x, 5, 6.3,
and streaming-through-dispatch (4.3, 2026-06-23). `llm/generate-object` (6.1) and the
batch budget pre-flight (6.2) were decided against 2026-07-28 — see the resolved ledger.
What's left:

- **6.5 — agent eval harness**: a `deftest`/`eval` surface that scores an agent against a
  fixture task + cassette in CI. Explicitly deferred by owner; reuses FakeProvider/cassettes.

(Cassette CI corpus — plan's 6.4 — is tracked separately as CASS-1.)

---

## A note on the truly long-term language design items

These are not deferred — they're design questions that need a deliberate decision before any code lands. They were tracked in `docs/wip.md` (the "Wave 6c" cluster), which is now archived at `docs/plans/archive/wip.md`.

---

## WF-1 — Larger dynamic-workflow work

**Deferred larger dynamic-workflow ideas** that should not be folded into a quick-fix pass. Source discussion: the GitHub issue comment on dynamic workflows — https://github.com/sema-lisp/sema/issues/41#issuecomment-4815472955. (The core `defworkflow`/`phase`/`step`/`checkpoint`/`parallel`/`pipeline` runtime shipped in 1.28.0; the items below are the next-tier extensions.)

**Manager and subprocess agents**
- Add a `sema-workflowd`-style manager that owns run lifecycle, scheduling, budgets, retries, cancellation, subprocess supervision, and dashboard serving. Keep it deterministic — it supervises and journals work, it is not an LLM planning loop.
- Add subprocess agents with a JSONL protocol before sockets (inspectable, replayable, journal-first).
- Define `defsubagent` (or equivalent) metadata for command, protocol, timeout, sandbox, and compiled-executable agents.

**Run directory format**
- Snapshot the executed `workflow.sema` and `args.json` into each run directory.
- Add per-agent folders with `input.json`, `prompt.md`, `events.jsonl`, `stdout.log`, `stderr.log`, `result.json`, and a first-class `artifacts/` path for reports/patches/generated files.
- Treat the run directory as a stable public format that can be copied to another machine and replayed or inspected later.

**Resume and cache keys**
- Extend agent cache keys beyond the current workflow source/version, args fingerprint, phase, name, prompt, and schema representation to also include model, system prompt, tool set/version, agent source, and the relevant child sandbox.
- Decide whether checkpoint keys should include an explicit caller-provided input hash for values that depend on external state.
- Preserve backward-compatible behavior or provide migration notes when content-key fields change.

**Permissions**
- Keep `:permissions` as the workflow metadata key.
- Move beyond CLI sandbox strings toward a structured permission schema (e.g. read-only, test-agent, patch-agent, research-agent profiles); map workflow/agent permissions to child-process sandbox flags and `--allowed-paths`.
- Consider runtime-level enforcement for in-process workflow calls, not only CLI pre-run interpreter construction.

**Scheduler semantics**
- Make `parallel` a scheduler primitive with ordered results, independent completion order, bounded concurrency, and configurable fail-fast.
- Add task/agent handles with `await`, `await-all`, `cancel`, and `status`; make cancellation propagate downward to running child agents.
- Add `pipeline` as a streaming DAG/barrier-avoidance primitive once `parallel` semantics are settled.

**Dashboard operations**
- Project `events.jsonl` into the dashboard first; SQLite remains a secondary index.
- Add operator controls: pause/resume/cancel run, cancel/restart agent, inspect prompt/result/tool-transcript, export report.
- Prefer SSE over WebSockets for the first live local dashboard stream.

## Notebook: per-cell + per-session LLM cost tracking (status bar) — SHIPPED in 1.35.0

Shipped as #68 (see CHANGELOG 1.35.0). The scoping notes below are kept for
the record only.

Accumulate LLM spend for a notebook session and attribute it per cell / per
run, surfaced as a per-cell badge and a session-cumulative status bar. Scoped
2026-07-03 (see the GitHub issue for full context):

- **Cell boundary**: `NotebookEngine::eval_cell` (engine.rs:108) / `eval_cells`
  (:277); cells evaluate on the dedicated engine thread (bridge.rs), so
  sema-llm's thread-local accounting is stable across cells.
- **Mechanism**: reuse the per-leaf usage-scope seam (`open_usage_scope` /
  `LeafUsage`, sema-llm builtins.rs:127/187) — open a scope per cell eval. It is
  already async-correct: offload pollers fold into the Rc captured at dispatch
  (the ASYNC-1 guarantee), so spend from tasks/agents/streams started in a cell
  lands on that cell even though it settles in a poller.
- **Plumbing**: `EvalResult` (engine.rs:50) gains usage; `EvalResponse`
  (render.rs:164) serializes it; UI = ui/notebook.js + index.html (Alpine).
- **Semantics**: badge = last-run cost of the cell; status bar = session
  cumulative (parity with `(llm/session-usage)`); reset on kernel restart.
  Cache hits report zero (shows "re-runs are free"); cassette replays charge
  the recorded usage from the tape — decide whether to tag those visually.
- Headless `notebook run` should print the same summary line at the end.

Deferred: feature work, not async-runtime scope. Filed as a GitHub issue.

## Consciously-not-converted blocking natives

**Found 2026-07-10, during the scheduler-blocking-natives sweep.** Two more
blocking-on-the-VM-thread spots were found and deliberately left as-is (not
tracked as bugs to fix later — the audit checked them and closed them):

- **`serial/*`** (`crates/sema-stdlib/src/serial.rs`) — `serial/read-line` and
  `serial/send` block up to the configured port timeout. Hardware-niche
  (`Caps::SERIAL`-gated, a real physical/virtual serial port must be attached)
  and low-traffic by nature — a script driving a serial device is not the
  concurrent-fan-out shape this wave targets. Revisit only if someone actually
  reports a serial script wanting to run concurrently with other async work.
- **Cold `import`/`load` and `sema/check-file`'s first-load read** (`import`:
  `crates/sema-eval/src/special_forms.rs`; `sema/check-file`:
  `crates/sema-stdlib/src/reflect.rs`) — the first time a module is imported,
  loaded, or checked, its source is read from disk and compiled synchronously.
  Narrow window (one file read, amortized by the module cache on every later
  reference) and not offload-able the way a leaf builtin is: compilation must
  run on the VM thread regardless (it calls back into the compiler/macro
  expander), so there is no simple "do the blocking part off-thread, resume
  with a `Value`" shape here — offloading only the file read would still leave
  the (usually larger) compile step blocking. Not worth the complexity for a
  one-shot, per-module cost.

## Unified runtime terminal-inventory — residual deferrals (2026-07-23, C7 sign-off)

Recorded when the terminal-inventory ledger was signed off
(`docs/plans/2026-07-19-unified-runtime-terminal-inventory.md`, Tasks 8–9 + C7).
Both are honest **narrowed-terminal** dispositions, not gaps to silently close.

- **R10B — PDF parser is not terminally bounded (subprocess isolation deferred).**
  `pdf/*` offloads `lopdf`/`pdf-extract` parsing over an owned byte snapshot under
  the quarantine `hard_deadline` cleanup net, and input-byte admission (R10A) is a
  terminal pre-dispatch reject. But the page/output caps run *post-parse*: `lopdf`
  can allocate/decompress object streams before the caps apply, so the parse step
  is bounded only by the wall-clock cleanup deadline, not a terminal `finite_work`
  unit cap. A truly terminal bound needs subprocess parser isolation (parse in a
  killable child under an RLIMIT), which is out of scope for the cooperative-runtime
  wave. Ledger row R10B is `MIGRATED (B9, split; documented NON-terminal parser
  bound)` — it does not claim BOUNDED.
- **R14B — serial bounded-checkout cancellation is unverifiable without hardware.**
  Serial ports expose no portable read-interrupt, so a cancelled `serial/read-line`
  cannot be aborted; R14B instead validates the port read timeout (`Some(_)`,
  non-zero, `<= SERIAL_MAX_OP_TIMEOUT`) before every dispatch, so a blocked worker
  frees within the validated bound. The `cancelled-op-settles-within-timeout`
  regression can only run against a loopback/pty-backed port; this environment has
  no serial hardware, so that arm is covered by the timeout-validation unit tests
  plus the no-hardware cancellation suite. Revisit if serial hardware coverage
  becomes available in CI.
- **B4 `io.rs` whole-file value-ABI read scanner guard — deferred (not a clean
  scan).** R08B's contract is already structural: `stream/open-*` and the `io.rs`
  quantum offloads admit only regular files (`io::admit_regular_file`), and the
  whole-file value-ABI reads (`file/read`, `file/read-bytes`) on the
  `!in_runtime_quantum()` host arm are HOST-ADAPTER-ONLY. A source guard to fail a
  raw whole-file read *reintroduced on the VM thread inside a quantum* cannot use
  the existing `RAW_STDIN_READ`-style active-runtime scanner: the legitimate
  in-quantum reads (`crates/sema-stdlib/src/io.rs` `std::fs::read_to_string`/`read`
  at the offloaded arms) sit **inside `quarantined_compute` worker closures** that
  the brace-matched `if in_runtime_quantum() { … }` block scan cannot distinguish
  from a direct VM-thread read, so the rule would false-positive and regress the
  green source-policy. A precise guard needs closure-aware analysis or a refactor
  that hoists the read out of the quantum block; deferred as a follow-up. (Unlike
  stdin, a file read inside a quantum is legal when offloaded, so the
  zero-tolerance stdin model does not transfer.)

## WIN-1 — MCP HTTP cancel doesn't always sever the transport on Windows (reopened)

**Originally fixed 2026-07-29** (`run_transport_task` in
`crates/sema-mcp/src/builtins.rs`, commits `e336e615`/`f6128756`): moved the
drop of an in-flight HTTP request future, on cancel, onto a task spawned on
the shared multi-thread Tokio runtime instead of a plain blocking thread, so
the connection's hyper dispatcher task reliably gets scheduled to notice the
abandoned request and close the socket. Verified green on Windows CI at the
time (`runtime_mcp_close_wait_is_promptly_cancellable`,
`runtime_mcp_call_http_wait_is_promptly_cancellable`,
`runtime_mcp_connect_http_wait_is_promptly_cancellable`) and archived as
resolved in `docs/plans/archive/deferred-resolved.md`.

**Reopened 2026-08-11**: nightly run 30979939966 (2026-08-05) failed
`runtime_mcp_call_http_wait_is_promptly_cancellable` on Windows with the
pre-fix symptom — clean Sema-side teardown (`live_tasks=0 resource_gates=0
cancel_accepted=true`) but the peer observed no disconnect within a 30s
window. `run_transport_task` makes the sever reliable, not guaranteed:
hyper has no API to force-close a specific pooled connection from outside
(`hyperium/hyper#3533`), so the fix only ensures a wake is attempted on the
connection's driver task, not that the resulting close completes before an
external observer checks. Same category as the Jul 29 nightly flake in
`windows_inherited_pipe_writer_does_not_block_drain_join` (different
transport, same shape: Windows I/O teardown latency isn't as tightly bounded
as the Unix equivalent). **Deferred because** the durable fix — driving the
HTTP transport through `hyper::client::conn` directly (holding the
`Connection` future so cancel can abort it explicitly) instead of
`reqwest::Client`'s pooled connections — is a real transport rewrite that
needs Windows CI iteration to verify (no Windows dev machine available), and
the failure rate is low (1 nightly run out of 14 since the original fix
landed). Revisit if it recurs, or when Windows CI access improves.

