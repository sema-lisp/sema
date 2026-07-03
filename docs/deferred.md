# Deferred items

Things that came out of the May 2026 quality sweep (Wave 6 audit) but were intentionally not fixed because they're too risky, too design-dependent, or have a cheap workaround. Each entry says *why* it's deferred so a future pass can decide whether to revisit.

## MCP-1 — Named/aliased MCP servers

**Found 2026-07-01, during the MCP client PR (#59).** Every `mcp/connect` and `sema mcp login/logout` repeats the full server config (`:url`/`:command`). A convenience layer would let you declare a server once — a `name → {:url …}`/`{:command …}` mapping (in a script or a small config file) — and refer to it by name (`(mcp/connect "asana")`, `sema mcp login asana`). Pairs naturally with the token store, which already keys by canonical URL. **Deferred because** it's a pure ergonomics feature with a design choice (script-level form vs. a config file), orthogonal to the client's correctness, and best done after the base client lands.

## MCP-2 — `sema mcp list`

**Found 2026-07-01 (PR #59).** No CLI command surfaces which remote servers have cached credentials or their token status. A `sema mcp list` would show authenticated/known servers (and, ideally, which script or config declared each — which depends on MCP-1). **Deferred because** it's additive tooling; the "which script declared it" part needs the alias registry from MCP-1 first.

## MCP-3 — Fully-offline agent replay (cassette `tools/list` + `connect` skip)

**Found 2026-07-01 (PR #59, M5 cassettes).** MCP `tools/call` results record/replay through the shared cassette, so agent tool *calls* replay offline. But `mcp/connect` (and its `initialize`/`tools/list`) still runs live on replay, so a fully server-less agent-session replay isn't possible yet — you still need the stdio server or the HTTP endpoint reachable to establish the connection and enumerate tools. Extending the cassette to record `tools/list` and short-circuit `connect` on replay would close this. **Deferred because** the common case (deterministic *call* replay for CI) is covered; connect/list recording is a larger seam (identity keying for the handshake, and for remote servers the OAuth/discovery legs) that isn't needed for the value M5 delivers.

Also noted from the PR #59 merge review as low-priority, not-yet-done: capping the device-flow `slow_down` interval growth (the `+5` itself is RFC 8628-correct), and auto-reconnecting a Streamable-HTTP session on a mid-session `404` (currently surfaced as a `reconnect required` error rather than transparently re-initializing).

## ASYNC-3 — `async/all` early-reject strands a span-owning `IoHandle` (teardown abort)

**Found 2026-07-02, while landing the ASYNC-1 fix.** `async/all` (the `AllOf`
scheduler target) short-circuits to `Complete` on the **first** rejecting task
(`RunGoal::status`, `crates/sema-vm/src/scheduler.rs`), without cancelling the still
in-flight siblings. A sibling parked on `Blocked(AwaitIo)` that is still *reachable*
(its promise held by a Sema variable) is kept by the terminal-only reap
(`reap_leftover_tasks`), so its span-owning `IoHandle` (e.g. an `llm/complete` offload)
can survive to thread/process teardown — where its detached `LlmSpan` calls `span.end()`
against a destructed OTel thread-local and aborts the process (the adversarial-#7 hazard
the timeout path already guards via `cancel_promise_task`). The ASYNC-1 budget fix is the
first thing to trigger this deterministically (a budget overrun *rejects* a task), but any
task rejection with a slow reachable sibling under an active OTel exporter hits it. **Deferred
because** the proper fix is a distinct async-cancellation-semantics decision — on an
`async/all`/`async/race` short-circuit, transitively cancel + abort the abandoned in-flight
siblings (extending `cancel_await_tree` to the combinator's promise set) — orthogonal to
ASYNC-1's dynamic-scope capture. **Workaround:** the shipped `async_budget_gates_concurrent_fanout`
gate sizes the cap to trip only once *all* siblings have charged (so none is left in-flight);
real code that awaits every promise, or runs without an OTel exporter, is unaffected.

## ASYNC-2 — Stepping across the scheduler boundary into sibling async tasks

**Found 2026-06-23; residual of the async-breakpoints fix.** Breakpoints inside async tasks now fully work under both the native DAP and the WASM playground: a breakpoint in an `(async …)` / `(async/spawn …)` body stops, `Continue` resumes, inspection (stack/scopes/variables) targets the paused **task's** VM frames, and Step Over/Out follow the task's own call depth (gate tests: `crates/sema/tests/dap_async_breakpoint_test.rs`, `crates/sema/tests/wasm_async_debug_test.rs`, `playground/tests/async-debugger.spec.ts`). The one remaining gap: stepping (Step Into/Over/Out) does **not** follow control *across* the scheduler boundary into sibling tasks or back to the main VM — while a task is paused, siblings stay parked and a step stays within the current task slice. **Deferred because** cross-task stepping is a distinct design problem (the stepper would have to model the cooperative scheduler's task graph, not just one VM's frame depth), it's an enhancement rather than the reported bug, and the STOP+CONTINUE+inspect slice already covers the common debugging need. Revisit if async stepping across tasks becomes a real workflow ask.

---

Verified 2026-06-09: U6 ("did you mean" hints — shipped via `suggest_similar` in sema-core, attached in both backends) and U9 (REPL completeness check — replaced by the lexer-based `SemaValidator` in `crates/sema/src/repl/validator.rs`) were removed because they have since been fixed. Remaining entries re-verified as still open.

Verified 2026-07-01: **LEX-1** (scientific/exponential number literals — `1e19`, `2e-5`, `1E10` now parse), **VM-1** (VM stack traces on runtime errors — the VM now captures the call stack at error time and serializes it as `:stack-trace`), and **N7** (`sort` on heterogeneous types — comparator-free `sort` now raises a type error on mixed types and compares ints/floats numerically, `crates/sema-stdlib/src/list.rs`) were removed because they are fixed. Remaining entries re-verified as still open.

Fixed 2026-07-02: **ASYNC-1** (dynamic-scope flags vs deferred async tasks) — `llm/with-cache`/`llm/with-budget`/per-call `:tags` are now captured per task and swapped in/out at each scheduler step (a third per-task context beside the otel + usage-scope swaps), with the active budget frame shared by `Rc` so a concurrent `with-budget` fan-out charges one aggregate. See ADR #67, `docs/plans/2026-07-02-async-1-dynamic-scope-per-task.md`; gates `async_cache_miss_is_counted` + `async_budget_gates_concurrent_fanout` in `crates/sema/tests/complete_async_test.rs`. (The follow-up teardown gap it surfaced is now tracked as ASYNC-3 above.)

---

## D5 — Typed `try`/`catch` form

**Today:** `(try expr (catch e ...))` catches *every* error type, including `:unbound`, `:arity`, `:type-error` — the kind of errors that usually mean a typo. The docs (`website/docs/language/special-forms.md` near "Re-throw errors you don't intend to handle") explicitly warn about this.

**The bug shape:** silent bug-masking. A typo inside `try` is swallowed and the catch block runs as if the operation failed for "real" reasons.

**Proposed fix (not done):** add `(catch [:user :type-error] e ...)` syntax that filters by the `:type` field, mirroring Clojure's `catch ExceptionType` or Common Lisp's `handler-case`. Optionally lint-warn on the un-filtered form.

**Why deferred:** non-trivial language design. Affects reader (new pattern in catch clause), special-form lowering in both backends, and prelude macros that use `try`. Needs an ADR before code.

**Workaround today:** users can do `(try ... (catch e (if (= (:type e) :user) (handle e) (throw e))))` to re-raise unexpected errors. That's a documented pattern in special-forms.md.

---

## N5 — `server.rs` response-helper `.unwrap()`s

**Today:** `crates/sema-stdlib/src/server.rs` lines ~1028-1099 (as of 2026-06-09) unwrap on `as_map_rc()` / `__stream_handler` / `__ws_handler` after a single-marker `is_*_response` check. A user who constructs a partially-formed response map (sets `__file_path` flag but forgets `__stream_handler`) panics the HTTP server thread.

**Proposed fix:** convert each unwrap to `.ok_or_else(|| SemaError::eval("..."))?` and propagate via `Result<ServerResponse, SemaError>` — sending a `ServerResponse::Error` over the oneshot instead of panicking.

**Why deferred:** the helper functions return `()` today; restructuring to propagate errors via the existing `oneshot::Sender<ServerResponse>` requires a new `ServerResponse::Error` variant and changes to the axum-side handler. Medium-effort refactor with non-trivial blast radius.

**Workaround today:** users normally build response maps with `http/ok`, `http/file`, etc. — those constructors always produce well-formed maps. The bug only triggers if a user builds a map by hand with the wrong `__*` markers. Low-likelihood in practice.

---

## L2 — Code-lens execution + `sema/evalResult` notification untested e2e

**Today:** `crates/sema-lsp/tests/e2e/test_code_lens.py` only verifies the lens command name; it never calls `workspace/executeCommand` with `sema.runTopLevel` and never listens for the `sema/evalResult` custom notification described in `website/docs/lsp.md:138-150`. A regression in the eval subprocess path or the notification payload would slip through.

**Proposed fix:** add a python e2e test that:
1. Sends `workspace/executeCommand { command: "sema.runTopLevel", arguments: [{uri, formIndex: 0}] }`
2. Waits on the client's incoming-notification queue for `method == "sema/evalResult"` with a small timeout
3. Asserts payload includes `ok`, `value`, `elapsedMs` fields

Pattern can mirror the diagnostic-waiting in `test_diagnostics.py`.

**Why deferred:** medium effort — the python test harness needs to handle async notifications cleanly, and the test depends on a subprocess `sema eval` running and returning. Not flaky-prone, just a lift to write right.

**Workaround today:** the unit-level path (the lens command itself) is tested. Integration regressions would surface during manual testing of the editor extension.

---

## VFS — clones on every read

**Today (updated 2026-06-09):** `vfs_read` returns `Option<Vec<u8>>`, cloning file contents on each call — the function now lives in `crates/sema-core/src/vfs.rs:15` (the embedded-binary VFS). The originally-cited `crates/sema-notebook/src/vfs.rs` has since become a different thing (disk-backed path-sandboxed shim) and is no longer relevant to this entry.

**Proposed fix:** return `Cow<'_, [u8]>` so cached reads can be borrowed, or back the VFS with `Arc<HashMap>` so the file table can hand out cheap reference-counted handles.

**Why deferred:** identified in PR #14 review (severity: medium). VFS read isn't a current hotspot — the notebook is interactive, not a high-throughput file server. Revisit if the notebook starts serving real bundles.

---

## CORE-2 — recursive-closure Rc cycle (memory leak) — **FIXED (2026-07-02)**

**Was:** a self-referential closure formed an `Rc` cycle that reference counting couldn't
reclaim: a local/returned recursive closure captures its own name as an `UpvalueCell`
whose `Closed(Value)` holds the closure (shape U — measured 260 B leaked per churn
iteration). The design work found two more live shapes: every top-level define forms an
env⇄closure cycle that pins the whole global env at interpreter/notebook teardown
(shape E, ~168 KB per drop), and the `__vm-*` delegates strongly captured the very env
they were registered into (shape D, ~166 KB per drop with zero user code). The attempted
`Weak` captured-env fix had been dropped — it broke the "module exports a fn calling a
private helper" pattern (`vm_module_test`).

**Fix:** a synchronous Bacon–Rajan cycle collector over the existing `Rc` heap —
**ADR #66**, design/measurements/milestones in `docs/plans/2026-07-02-core2-gc.md`,
GC section in `docs/vm-status.md`. Creation-time candidate registry (VM closures, home
envs, the cold data constructors `delay`/promise/`channel`/`defmulti`), trial deletion
over a transient side map, reclamation by *severing* the one mutable cell every Sema
cycle must pass through. No headers, no `Value`/`Rc` changes, `Value::drop` untouched.
Shape D was fixed by refactor (delegates capture `Weak` — invariant I2 in AGENTS.md).
Perf gate passed (plan §6 M4): storm +0.91%, upvalue-counter +1.41%, fold −0.01%,
318 ns per reclaimed churn cycle. Oracles: `crates/sema/tests/leak_test.rs` (un-ignored),
the `gc_stress_test.rs` suite, the agent-turn FakeProvider test in `llm_fake_test.rs`,
and the notebook `reset_returns_old_kernel_memory` test.

---

## WASM-4 — `register_wasm_io` is a single ~1093-line function

**Today:** `crates/sema-wasm/src/lib.rs` registers all WASM I/O builtins in one ~1093-line function. Large WASM functions carry a known V8 Turboshaft miscompilation/crash risk on ARM64 (see the chromium-wasm-crash note in MEMORY).

**Proposed fix:** split into smaller per-area registration functions (pure refactor, no behavior change).

**Why deferred (decided 2026-06-18):** latent risk only; the crash has not been observed since. Revisit if it recurs in the playground. Large diff on a hot path, not worth the churn now.

---

## C1 follow-up — caught-HOF-callback errors lack a stack trace

**Today:** after the C1 fix (HOF callbacks routed into the running VM), one residual symptom of wrapping a VM closure as a `NativeFn` remains: a VM error caught from inside a HOF callback lacks a `:stack-trace`. (The sibling `(type (fn …))` → `:native-fn` artifact was fixed 2026-06-19 via the `NativeFn::is_closure` marker — see VM-2 above, now resolved.)

**Why deferred (decided 2026-06-18):** cosmetic / low-impact; it stems from the closure-as-NativeFn boundary, not from upvalue timing (which C1 fixed). Tied to VM-1 (stack traces). Revisit if it bites real usage.

---

## LC — Living Code LLM layers (`ask` / `heal!` / `evolve` / `observe!` / `become!`) — killed for good

**What it was:** layers 3–6 of the Living Code design (`docs/design/living-code.md`) — LLM-driven introspection (`ask`, `ask/code`, `ask/patch!`), auto-repair (`heal!`), genetic programming (`evolve`), and runtime self-modification (`observe!`, `become!`, `history`, `rollback!`, `freeze!`). Shipped on the tree-walker (PR #30, commits `248ebd8`/`fb0d7e6`/`69f1514`), then silently dropped when the tree-walker was retired in 1.18.0 — never ported to the VM, unbound at runtime, undiscovered for two releases.

**Why killed (not deferred):** (1) non-deterministic by construction — `evolve`/`heal!` emit a fresh LLM sample each run, so there is no regression test you can write, which is *exactly* how it rotted unnoticed; (2) `become!` (LLM rewrites a running function in place) carries a safety surface — doctest gates, sandboxes, rate limits, audit logs, freeze switches, rollback history — larger than the feature itself, a permanent tax on every VM/env change; (3) zero demand — no issue, no playground example, no website doc referenced it, and nobody noticed its disappearance.

**Salvage — also parked:** the whole feature is parked, nothing implemented. Only layer 0 (runtime docstrings `doc`/`meta`) was seriously considered, and a feasibility pass confirmed it's *clean* to build — the `Function` struct already carries serialized compile-time metadata (`source_file`, `local_scopes`), so a `doc` field rides the same path and the `.semac` string table (no source-text drag, binary path inherits it free). But with doctests + the LLM layers gone, `doc`/`meta` alone wasn't worth the standing maintenance (a `.semac` format-version bump + ~10 `Function` construction sites to carry forever), so it was **cut for maintainability** (2026-06-20) and parked as a clean plan to revisit later: `docs/plans/archive/2026-06-20-docstrings-and-introspection.md`. **Doctests (layer 1)** were dropped earlier as YAGNI. **Layer 2** (`read-source`/`source-of`/`;;@directives`) was scaffolding for the dead LLM layers — not salvaged either.

**Artifacts retired 2026-06-20:** PR #30 closed; `docs/plans/2026-02-24-living-code-phase4.md` archived; `docs/design/living-code.md` banner-marked RETIRED.

---

## P6 — `partition` / `frequencies` / `list/group-by` double-clone (perf, won't-do)

`crates/sema-stdlib/src/list.rs`. These clone each element twice (once for the callback args, once when pushing into the output bucket). Could be cut to one clone by consuming `items.iter().cloned()`.

**Why won't-do:** moved here from `docs/wip.md` on 2026-06-20. The earlier P1 work established that `Rc::clone` is too cheap to measure on these HOF-dispatch-bound paths; the same applies here. Revisit only if a profile actually fingers `partition`/`group-by` as a hotspot.

## P7 — `CALL_NATIVE` clones `Rc<NativeFn>` per call (perf, spiked → discarded)

`crates/sema-vm/src/vm.rs`, CALL_NATIVE handler: `let native = self.native_fns[native_id].clone();` — one `Rc` bump per native call, purely to release the borrow on `self.native_fns` so `self.stack` can also be borrowed.

**Spiked and discarded 2026-06-20.** Implemented the raw-pointer alternative (`Rc::as_ptr` + a minimal `unsafe` deref; the safety invariant holds — `native_fns` is built once at VM construction and never mutated during dispatch). It compiled, passed all tests + clippy, and was correct. But benchmarking before/after on `higher-order-fold`, `hashmap-bench`, and `string-pipeline` showed the delta entirely within noise (means < 1σ apart; the "winner" sign even flipped across workloads). A single non-atomic `Rc` bump on a single-threaded VM is free in practice. Adding `unsafe` to the hottest dispatch path — plus the standing burden of re-auditing the "never mutate `native_fns`" invariant on every future edit — for zero measured gain makes the codebase strictly worse. Not doing it. The only lever here is the `unsafe` one (a safe borrow-restructure is blocked by the re-entrant-HOF `&mut self` path), so this stays closed unless the call shape changes materially.

---

## TOOL-1 — Migrate the Makefile to a justfile

**Deferred (revisit later) — 2026-06-22.** The build/dev/deploy automation lives in a
single growing `Makefile`. The intent is to move it to a [`justfile`](https://github.com/casey/just),
whose tooling is better suited to a task runner (real argument passing, no `.PHONY`
bookkeeping, no tab/space footguns, per-recipe shebangs, `just --list` discovery, dotenv
support). Not urgent — the Makefile works — so parked until there's appetite for the
one-time port plus updating CI and the docs that reference `make` targets. When it
happens, mirror the current targets (`build`/`release`/`test`/`lint`/`examples`/
`smoke-bytecode`/`deploy`/`deploy-all`/…) one-for-one first, then improve.

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
  suite; wire `SEMA_LLM_CASSETTE_MODE=replay` into `make test` so the suite runs green with
  no API keys. (The keyless oracle today is the scripted `FakeProvider`; cassettes would add
  real-response replay on top.)
- **Open questions** carried from the plan: a `NullProvider` inner so pure-replay needs zero
  credentials; tape versioning/migration when `ChatRequest`/`ChatResponse` shapes change (the
  `"v":1` field is the hook); tapes beside tests (`tests/tapes/`) vs. a top-level `cassettes/`
  (leaning beside-tests); one-tape-per-test vs. shared (leaning per-test).

---

## LLM-1 — LLM bulletproofing remnants (from the archived plan)

**Deferred (revisit later) — 2026-06-22.** The bulletproofing plan
(`docs/plans/archive/2026-06-21-llm-bulletproofing.md`) shipped Phases 0–3, 4.1, 4.2, 4.4,
5, and 6.3. What's left:

- ~~**4.3 — streaming through the dispatch layer**~~ ✅ **DONE 2026-06-23.** `llm/stream`
  now applies rate-limit + fallback at stream-open and an opt-in budget pre-gate
  (`:on-stream :pre-gate`); mid-stream failure surfaces + keeps the partial (no failover —
  the spike proved a retry would duplicate). Cache stays off for streams (cassettes cover
  deterministic replay). Verified live.
- **6.1 — `llm/generate-object`**: schema-validated structured output with a bounded repair
  loop (today only `llm/extract` does schema+reask). Reuse `validate_extraction` +
  `format_reask_prompt`.
- **6.2 — batch budget pre-flight**: budgets are post-call caps, so a concurrent
  `llm/batch`/`llm/pmap` fan-out can overshoot before the cap fires. Add a pre-dispatch
  token-estimate gate.
- **6.5 — agent eval harness**: a `deftest`/`eval` surface that scores an agent against a
  fixture task + cassette in CI. Explicitly deferred by owner; reuses FakeProvider/cassettes.

(Cassette CI corpus — plan's 6.4 — is tracked separately as CASS-1.)

---

## PG-1 — Playground → downloadable native binary

**Deferred (revisit later) — 2026-06-23.** Captured 2026-06-19 as a curiosity and
archived to `docs/plans/archive/2026-06-19-playground-binary-export.md`. The
playground runs the WASM build, but `sema build` isn't compilation — it's
concatenation (`[stock runtime] + [VFS archive] + [trailer]`), so the browser
could produce a byte-identical runnable native binary with **no compiler**: pick a
target, fetch the stock runtime (ideally mirrored same-origin on sema.run), append
the archive built from the editor contents, write the `SEMAEXEC` trailer, download.

**Feasibility high, effort low (~half a day)** — mostly UI + hosting the runtime
mirror. Preferred first step: factor archive-writing into a lib and expose a
`sema-wasm` binding returning `Uint8Array` (avoids format drift vs. reimplementing
the format in JS). Pointers: `crates/sema/src/archive.rs` (format),
`crates/sema/src/cross_compile.rs` (`SUPPORTED_TARGETS`, runtime download/cache),
`crates/sema/src/main.rs` `Commands::Build` + `pkg.rs`.

**Why deferred:** not scheduled — no demand pull, just an attractive proof-of-concept.
Resume from the plan's "Smallest proof-of-concept" section.

---

## DOCS-SEARCH-1 — Domain-specialized tuning of the `docs_search` MCP tool

**Found 2026-06-25, after shipping `docs_search`.** The shipped tool is a generic-ish lexical BM25 ranker (recall@5 ≈ 0.93 on a keyword-ish oracle) but degrades on **vague, intent-only queries** where the user's words don't overlap the docs' words (~6/18 such queries missed: save→`file/write`, "each item"→`map`, scramble→`hash/sha256`). **Desired:** exploit that this engine is single-purpose over a fixed corpus known at build time — move expensive work (including a build-time LLM) offline and bake it, keeping the query path offline/deterministic and scratch-gate-safe. Highest-leverage levers: build-time document expansion (doc2query intent phrases/synonyms baked per entry), a popularity prior (we already computed per-symbol call-frequency), and a hybrid BM25 + pure-Rust static-embedding ranker — all measured against a baked gold-query eval harness. **Deferred because** the current tool is good enough to ship and the tuning is a multi-phase investment best done when conceptual-query quality demonstrably matters. Full plan: `docs/plans/2026-06-25-docs-search-tuning.md`.

---

## A note on the truly long-term language design items

These are not deferred — they're design questions that need a deliberate decision before any code lands. They're tracked in `docs/wip.md` (the "Wave 6c" cluster), not here.

---

## WF-1 — Larger dynamic-workflow work

**Deferred larger dynamic-workflow ideas** that should not be folded into a quick-fix pass. Source discussion: the GitHub issue comment on dynamic workflows — https://github.com/HelgeSverre/sema/issues/41#issuecomment-4815472955. (The core `defworkflow`/`phase`/`step`/`checkpoint`/`parallel`/`pipeline` runtime shipped in 1.28.0; the items below are the next-tier extensions.)

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
