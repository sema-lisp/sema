# Dynamic Workflows — De-risking Addendum & Spike Specs

**Status:** De-risking addendum (2026-06-23) to `docs/plans/2026-06-21-dynamic-workflows-scoping.md`.
Code-grounded. Every claim cites a `file:symbol` I actually read at the stated commit state.
Audience: the repo owner, building from this for a full day.

---

> ## ⚑ STATUS UPDATE 2026-06-23 (later same day) — the Spike-0 concurrency BLOCKER is RESOLVED on `main`
>
> This addendum was written against a tree where **every LLM call was `runtime.block_on`** and
> `set_yield_signal` appeared **zero times** in `sema-llm`, so the gate's central verdict was
> "in-process async cannot interleave the actual LLM workload; concurrent LLM I/O degrades to
> sequential; subprocess fan-out has zero existing primitive." **That premise is now false.** The
> concurrent-I/O work landed (branch `feat/async-awaitio`, commit range `f233943..d254685`,
> **CHANGELOG 1.27.0**). Verified in-repo (file:line):
>
> - **A cooperative `AwaitIo` yield now lets blocking leaves interleave.** New `YieldReason::AwaitIo(Rc<IoHandle>)`
>   (`sema-core/src/async_signal.rs:119`), scheduler wake-arm + poll (`sema-vm/src/scheduler.rs:176,236`).
>   `set_yield_signal(AwaitIo(..))` now appears — gated on `sema_core::in_async_context()` — in
>   `sema-stdlib/src/http.rs:126,240` (`http/*`), `sema-stdlib/src/system.rs:199,241` (`shell`), and
>   `sema-llm/src/builtins.rs:2842,5346` (`llm/embed`, and `llm/complete`/`classify`/`extract` via
>   `do_complete`). **Top-level (non-async) calls are byte-identically synchronous** — the yield only
>   fires inside a scheduler task. Verified live (CHANGELOG): 4× `llm/complete` ~3.4× faster than serial,
>   4× `llm/embed` ~13.6×, 5× `shell` 514 ms vs 2571 ms.
> - **`async/pool-map` ships as a prelude macro** (`sema-eval/src/prelude.rs:134`): bounded-concurrency
>   fan-out, `≤n` in flight, results in **input order** — built from exactly the semaphore-channel +
>   `async/all` recipe Spike 0 Track A / Spike 3 specified (token released on BOTH success and error
>   paths, no deadlock). **This IS the `workflow/foreach`/`parallel` substrate. It is built and shipped,
>   not a spike to run.**
> - **True cancellation ships** (`docs/plans/2026-06-23-concurrent-complete-and-true-cancel.md`,
>   Slice B): `async/cancel`/`async/timeout` abort real work — `cancel_await_tree`
>   (`scheduler.rs:323`, transitive across `async/await`), `IoHandle` abort seam
>   (`async_signal.rs:39` `with_abort`), `http` connection torn down, `shell` subprocess **SIGKILLed as
>   a process group** (`system.rs:104-151` `kill_on_drop`+`process_group(0)`). LLM tier is honest
>   best-effort (the `spawn_blocking` worker can't be interrupted mid-call; the result is discarded).
> - **Per-task OTel isolation ships** (concurrent LLM spans no longer cross-contaminate).
>
> **What this means for the spikes below.** Spike 0's GATE is effectively **passed by shipped code**:
> the in-process async substrate runs ordered, bounded, cancellable fan-out AND now overlaps blocking
> leaves (http/shell/single-shot LLM) — no new value type was needed (`Value::AsyncPromise` is the
> handle). The four obsolete verdicts (R1 row, the "do NOT route concurrent LLM I/O through async/spawn"
> recommendation, and the three §3.2 Plan-correction bullets) are marked **SUPERSEDED** inline below;
> the history is kept for the reasoning trail.
>
> **What is STILL true and STILL needs building** (orthogonal to concurrency — do not treat as solved):
> Spike 1 (sequential runtime + frozen JSONL journal), Spike 2 (cassette-backed demo + the
> `compute_cache_key` anti-collision proof), Spike 4 (canonical `Value→bytes` encoder for resume).
> Two known caveats survive: **(a)** in-process **multi-round `agent/run`** overlap is the deferred
> "major rewrite" — the monolithic `run_tool_loop` native frame cannot yield mid-loop
> (`async-agent-parallelization.md` §2; the round-loop holds `Rc` otel guards across `do_complete`
> inside a Rust `for`). Single-shot `llm/complete`/`classify`/`extract`/`embed` DO overlap; a full
> tool-using agent run does not yet. **(b) ASYNC-1** (`docs/deferred.md`): `llm/with-cache`/`with-budget`
> are dynamically-scoped thread-locals that an async task reads *when it executes*, which the scheduler
> can defer past the thunk's return — so budgets/cache flags may **not** reliably apply to deferred
> concurrent tasks. Relevant the moment a workflow attaches a budget to a concurrent fan-out.
>
> The "Remaining blockers to start implementation" section at the very bottom is the actionable summary.

---

## TL;DR — what grounding the plan in real code changed

Grounding flipped four things the scoping doc treated as cheap or solved.
**(1)** Spike 0's own "zero-Rust, ~1 day" pure-Sema `foreach` sketch is *not buildable as written*: there is no `make-vector`/`vector-set!`/`atom`/`box` in the stdlib (only `vector` literal at `list.rs:15` and `assoc` at `map.rs:48`), so the index-result-vector and the shared peak-counter it relies on do not exist — and they are also *unnecessary*, because `async/all` (`async_ops.rs:239`) already returns results in **input order**.
**(2)** Spike 3's `workflow/typed-step` sketch uses `(loop … (recur …))`, which **does not exist** — there is no `loop`/`recur` special form or prelude macro (only `dotimes`/`for-range` at `prelude.rs:81,91`; `try`/`catch` at `lower.rs:264`).
**(3)** Spike 4's canonical-encoder reject-list is wrong: `impl Hash for Value` (`value.rs:1781`) *does* handle `Record`, `F64Array`, `I64Array`; the silent `_ => {}` (`value.rs:1854`) only swallows `Map`/`HashMap`/closures/agentic types — so the encoder must *encode* records and typed arrays, not reject them, or resume silently breaks for any numeric/record input.
**(4)** The plan's two named dependencies are partly phantom: `docs/plans/2026-06-21-llm-cassettes.md` **does not exist** (the cassette layer ships at `crates/sema-llm/src/cassette.rs` regardless), and `docs/internals/` **does not exist** — `bytecode-format.md` lives under `website/docs/internals/`, so the journal spec belongs there. The GATE conclusion stands but narrows hard: the in-process async substrate is real and needs no new value type, yet it cannot run the one workload the feature exists for (concurrent `agent/run`), because every LLM call is `runtime.block_on` (`anthropic.rs:429`) and `set_yield_signal` appears zero times in `sema-llm`.

---

## Grounded verdicts per risk

| Risk area | Verdict | Decisive actual-code fact | Remaining unknown |
|---|---|---|---|
| **R1 — Parallel scheduling on single-thread `Rc`** | ~~Feasible for *structure*; **blocked** for the actual LLM workload~~ **SUPERSEDED 2026-06-23 → RESOLVED** | ~~`set_yield_signal` count in `sema-llm` = 0~~ — **now non-zero**: `YieldReason::AwaitIo` (`async_signal.rs:119`) makes `http/*`/`shell`/`llm/embed`/`llm/complete`/`classify`/`extract` yield while their work runs on a background runtime (`http.rs:240`, `system.rs:199`, `builtins.rs:2842,5346`), gated on `in_async_context()`. `async/pool-map` (`prelude.rs:134`) ships the ordered bounded substrate. Concurrent LLM I/O verified ~3.4× (CHANGELOG 1.27.0). | The one remaining gap is **multi-round `agent/run`** overlap (the `run_tool_loop` native frame can't yield mid-loop — deferred "major rewrite", `async-agent-parallelization.md` §2). Single-shot LLM leaves DO overlap. |
| **R1b — Subprocess pool** | Feasible but **net-new Rust** | Only process builtin is blocking `shell` = `Command::output()` (`system.rs:53`); grep finds no `Stdio`/`.spawn()`/`Child`/`kill` in stdlib | Unix process-group/`setsid` reaping + Windows `TerminateProcess`; the `--json` envelope `value` is a STRING (`main.rs` `print_eval_json`) needing a parse round-trip |
| **R2 — Resume needs canonical serialization** | Feasible; **must build a new encoder** | `Value::Hash` swallows `Map`/`HashMap`/closures via `_ => {}` (`value.rs:1854`); `serialize_value` writes string-table *indices* (`serialize.rs:108`) → process-unstable; `value_to_json` collapses string/keyword/symbol keys and errors on NaN (`json.rs`) | Float/JSON low-bit drift in *downstream* inputs (normalization spec); whether `Record`/typed-array inputs are common enough to matter (they ARE Hash-handled, so encoder must too) |
| **R3 — Determinism softer than pitch** | Confirmed; honest framing required | Cassette keys on LLM request text only (`compute_cache_key`, `builtins.rs:4475`), not Sema step inputs; orchestrator determinism is *assumed*, never enforced | No test enforces "record twice ⇒ byte-identical tape"; unsorted dir walks in `inventory` would silently drift keys |
| **R4 — Structured agent output absent** | Confirmed, but partial result already exists | 2-arg `agent/run` returns `Value::string`; 3-arg returns `{:response :messages}` (`builtins.rs:~2354`) — no `:status`/typed findings | Repair loop (`llm/extract`, `builtins.rs:1743`) is bound to `llm/complete`+`json_mode`, treats JSON *parse* failure as hard error (escapes retry); `validate_extraction` is shallow + private (`builtins.rs:4384`) |
| **Dynamic-context mechanism (defworkflow/phase/checkpoint/budget)** | **Feasible-low-risk**; copy a shipped pattern | `ConversationGuard`+`Drop` (`sema-otel/src/imp.rs:152`) is the panic-safe RAII template; `otel/with-session` (`sema-stdlib/src/otel.rs:203`) + `with-session` macro (`prelude.rs:103`) are the macro-over-thunk template | Whether to copy the panic-safe Drop guard or the non-panic-safe budget save/restore (`builtins.rs:339/362`) — recommend Drop |
| **Frozen JSONL journal + CLI + OTel tie-in** | Feasible; one real correction | `JsonlFileExporter` (`file_exporter.rs:25`) is the verbatim writer template, but its `span_to_json` (`file_exporter.rs:119`) **drops span events** → cannot be the journal sink; OTel switch is `SEMA_OTEL_FILE`/`OTEL_EXPORTER_OTLP_ENDPOINT`, **not** `SEMA_OTEL=1` | `serde_json` has no `preserve_order` (`Cargo.toml:50`) ⇒ map-encoded events sort keys alphabetically; needs fixed `#[derive(Serialize)]` enum + a clock/run-id test seam |
| **Cassette key collision across tool-loop rounds** | Real, must de-risk before trusting multi-round replay | `compute_cache_key` hashes only model+temp+system+role+`content.to_text()`, no separators, ignores tools/`tool_calls` (`builtins.rs:4475`); `Tape::lookup` is first-match non-consuming (`cassette.rs:204`) | Whether a *re-asked* agent that emits identical visible text twice collides — Spike 2's scripted distinct-text rounds do NOT exercise this |

---

## Spike specifications

### Spike 0 — Scheduler GATE (most detailed; do this first)

**Goal.** Decide and *prove* the concurrency substrate. The plan (§3.2) offers two candidates and calls reuse of the async scheduler "mostly wiring." Grounding says: the structural primitive is free and needs **no new value type** (`Value::AsyncPromise` is the handle — `task_id` + `async/cancel`/`async/cancelled?`/`async/all`, `async_ops.rs:213,231,239`), but the workload the feature targets (`agent/run` fan-out) **cannot interleave** on it. The gate must produce a *decision*, not a build.

**Explicit recommendation — in-process-async vs subprocess.**

> **SUPERSEDED 2026-06-23 (see STATUS UPDATE banner).** The core "do NOT route real concurrent LLM I/O
> through `async/spawn`" recommendation below is **obsolete**: the `AwaitIo` yield (`async_signal.rs:119`)
> now makes `http/*`/`shell`/`llm/embed`/single-shot `llm/complete`/`classify`/`extract` interleave on
> `async/spawn`, and `set_yield_signal` *does* now appear in `sema-llm` (`builtins.rs:2842,5346`). The
> scheduler now advances `virtual_now` by **real elapsed** while an `AwaitIo` is in flight (bounded by
> the nearest sleeper/timeout deadline) — it models real wall-clock overlap. Subprocess fan-out is also
> **no longer net-new Rust**: `shell` yields and is SIGKILL-cancellable today (`system.rs:104-199`).
> The *narrow* part of the original recommendation that survives: **multi-round `agent/run`** (the
> `run_tool_loop` native frame, not single-shot LLM leaves) still cannot interleave — keep that out of
> the parallel feature's v1 (use single-shot `llm/complete` composition or subprocess `.sema` workers
> instead). The original text is preserved below for the reasoning trail.

~~Ship `workflow/foreach`/`parallel` on the **in-process async substrate** for *sequencing, input-ordering, bounded structure, and cancellation* — these are real and free. **Do not** route real concurrent LLM I/O through `async/spawn`. The reason, code-grounded: `agent/run` → `run_tool_loop` → `provider.complete` = `self.runtime.block_on(self.complete_async(req))` (`anthropic.rs:429`; identical at `openai.rs`/`gemini.rs`/`ollama.rs`), and `set_yield_signal` never appears in `sema-llm` — so the first LLM leaf blocks the single OS thread for the whole HTTP round-trip and starves every sibling task. The scheduler interleaves *only* at the four `YieldReason` points (`async_signal.rs:18` `AwaitPromise`/`ChannelRecv`/`ChannelSend`/`Sleep`), and `async/sleep` advances a **virtual clock** (`scheduler.rs:79` `virtual_now`, `:58` `wake_at`) — so it does not even model real wall-clock overlap. Real concurrent LLM I/O is a **separate follow-on**: cheapest is extending `batch_complete`'s `join_all` (`anthropic.rs:441`) with per-item indices so order survives; the general (arbitrary-step) path needs a new `YieldReason::ProcessWait` + scheduler poll of child exit, and is blocked structurally by the thread-local `PROVIDER_REGISTRY` (`builtins.rs:26`) + `Rc`-non-`Send` `Value`/`Env`. **Subprocess fan-out is the right model for genuinely heavy/isolated leaves, but it is net-new Rust** (no `spawn`/`Stdio`/`kill` exists today — `system.rs:53` is blocking `Command::output()`), so it is out of the gate's build scope; the gate's job is to *choose it or defer it*.~~

**Concrete steps.**

*Track A — in-process structural proof (pure Sema, but NOT zero-Rust — see step A0).*
- **A0 (precondition, ~0.5d Rust).** Add one mutable cell primitive, because the stdlib has none (grep: no `make-vector`/`vector-set!`/`atom`/`box`/`swap!`). Smallest option: a Rust `atom`/`box` (`atom`, `deref`, `reset!`) in `crates/sema-stdlib/src/`. *Or* avoid it entirely for ORDER by relying on `async/all`'s ordered return (verified below) and only needing the cell for the BOUND counter.
- **A1.** Write `workflow/foreach` in `fuzz/spike0-foreach.sema`: a semaphore `(channel/new N)` pre-filled with N tokens via N `channel/send`; each worker thunk does `(channel/recv sem)` (acquire, yields when empty — `async_ops.rs:501`), runs the leaf, then `(channel/send sem :tok)` on **both** success and error paths (unwind-safe: a worker that fails to return its token deadlocks the pool — `scheduler.rs:581` surfaces "all tasks blocked"). Spawn all thunks, `(async/all promises)`.
- **A2 — ORDER.** Do **not** build a result vector. `async/all` iterates `promises` in spawn order and pushes `Resolved` values in that order (`async_ops.rs:266`), so results are already input-ordered. Leaf `slow-echo` does `(async/sleep (* (- 8 i) 5))` then returns `i` — completion order reversed, result order preserved.
- **A3 — BOUND.** This is the only place that needs a mutable cell. Increment on acquire / decrement on release, track peak. **Caveat to honor:** the acquire→increment window spans a yield (`channel/recv` yields), so a naive peak counter measures *counter state*, not *true in-flight*, and reads racy. Make the assertion structural instead: assert the semaphore channel's capacity is N and that no `channel/send sem` ever sees the buffer exceed N (the cap is enforced by `async_ops.rs:467`'s `buf.len() >= capacity` yield). If you keep a counter, assert `peak <= 3`, not `== 3`.
- **A4 — CANCEL.** Spawn workers that `(async/sleep 1000)`, then `(map async/cancel ps)`. `async/cancel` transitions only tasks with a real `task_id` via `call_cancel_callback` (`async_ops.rs:215`); the scheduler honors `task.cancelled` at the top of the per-task loop (`scheduler.rs:162`). **Fix the oracle:** assert each promise is `async/cancelled?` *directly* (do not `(filter async/pending? …)` first — that filter is vacuously satisfiable once cancel moved them out of Pending), and drive the scheduler (`async/all` in a `try`) so cancellation is actually observed.

*Track B — blocking-leaf trap evidence (proves WHY subprocess needs new Rust).*
- **B1.** Add a yielding control leaf `(async/sleep …)` and a blocking leaf `(shell "sh" "-c" "sleep 0.05; echo i")` (`system.rs:53`). Run `foreach` over each, timed with `sys/elapsed` (ns — MEMORY: not `time/now-ms`).
- **B2 — confound to avoid (O4).** `sh sleep` blocks the OS thread *by construction*, so "wall-clock == sum" would print even if the trap were fixed. To make it load-bearing, compare against the `async/sleep` leaf: the virtual clock (`scheduler.rs:213` advances `wake_at`) compresses its wall-clock to ~instant while preserving N-at-a-time interleaving; the `shell` leaf does not. The evidence is the *contrast* (async leaf fast + interleaved vs shell leaf serial), not the shell timing alone.

**Real files to touch.**
- `fuzz/spike0-foreach.sema` (new, throwaway).
- `crates/sema-stdlib/src/list.rs` or a new `atom.rs` (new `atom`/`reset!`/`deref` if BOUND needs a cell) + register in `lib.rs`.
- `crates/sema/tests/vm_async_test.rs` (new `#[test] spike0_foreach_order_cancel`, model on existing async harness; pin ORDER with `=> common::eval_tw("'(0 1 2 3 4 5 6 7)")`).
- `docs/plans/2026-06-21-dynamic-workflows-scoping.md` (record the §5 gate verdict; correct §3.2 option (2) — see Plan corrections).

**Code sketch (hardest part: unwind-safe semaphore + ordered collect with no result vector).**
```sema
;; fuzz/spike0-foreach.sema — relies on async/all input-ordering (async_ops.rs:266),
;; so NO mutable result vector is needed. Token MUST be returned on the error path too.
(defn workflow/foreach (worker items n)
  (let ((sem (channel/new n)))
    (dotimes (_ n) (channel/send sem :tok))           ; pre-fill exactly n tokens
    (let ((promises
            (map (fn (item)
                   (async/spawn
                     (fn ()
                       (channel/recv sem)              ; ACQUIRE (yields when empty)
                       (let ((r (try {:status :ok :value (worker item)}
                                     (catch e {:status :failed :error e}))))
                         (channel/send sem :tok)       ; RELEASE on BOTH paths
                         r))))                          ; resolve to a MAP, never reject
                 items)))
      (async/all promises))))                          ; results in INPUT order

(defn slow-echo (i) (async/sleep (* (- 8 i) 5)) i)
;; ORDER oracle: completion reversed, result order preserved
(println (workflow/foreach slow-echo (list 0 1 2 3 4 5 6 7) 3))
```
**Runnable acceptance oracle.** `make release`, then `./target/release/sema fuzz/spike0-foreach.sema` exits 0 and the ORDER print is `(0 1 2 3 4 5 6 7)`; CI form `cargo test -p sema --test vm_async_test spike0_foreach` GREEN, no network. CANCEL: `(every? async/cancelled? ps)` after `(map async/cancel ps)` is `#t`. Track B prints the *contrast* (async leaf wall-clock ≪ shell leaf wall-clock ≈ sum), documented as the gate evidence that blocking leaves cannot parallelize on this path.
**GATE passes** iff Track A proves order+bound(≤N)+cancel with no new value type AND Track B shows the async/shell contrast. If A's bound or interleaving fails, scope the feature to sequential-only or commit to `proc/pool-map`/`YieldReason::ProcessWait` *before* any further build.
**Effort.** M, 2–3 days (A0 cell + A1–A4 + B + write-up). The optional `proc/pool-map` Rust primitive is a separate +2–3 days with real platform risk; explicitly out of the gate.
**Dependencies.** None blocking for Track A (all primitives shipped). Spike 0 GATES Spike 3. It is independent of Spike 1 (sequential).

---

### Spike 1 — Sequential runtime + frozen journal

**Goal.** `defworkflow` macro + `phase` + `checkpoint` + a frozen JSONL run-dir + a `{:status …}` envelope + `sema workflow run <file> --args <json>`. Sequential only; no parallel, resume, canonical hashing, subprocess, or daemon. Oracle = byte-identical golden `events.jsonl` under a fixed-clock/run-id seam.

**Concrete steps.**
- New leaf crate `crates/sema-workflow` (deps `sema-core` + `sema-otel` + serde; **not** `sema-eval` — preserves the CLAUDE.md no-circular-dep invariant). Add to workspace members + the 12 `=X.Y.Z` pins.
- `journal.rs`: copy `JsonlFileExporter` verbatim (`file_exporter.rs:25`: `OpenOptions::new().append(true).create(true)`, `BufWriter`, one `serde_json` line + `\n`, flush-per-event, swallow write errors). Rust-side, bypasses the Sema sandbox — same trust model as the OTel exporter; document it.
- `event.rs`: `#[derive(Serialize)] #[serde(tag="event")] enum WorkflowEvent` (controls field names; avoids the `serde_json` no-`preserve_order` alphabetical surprise — `Cargo.toml:50`). Each variant carries `seq: u64` + `ts: String`. Freeze `agent.result.output` as an opaque **string/digest only** (agent/run returns a String — `builtins.rs`), so typed fields can be added later without a breaking change.
- `context.rs`: `WorkflowCtx` + thread-local `WORKFLOW` + `set_workflow_scope → WorkflowGuard` with `Drop` restoring `prev` — copy the panic-safe `ConversationGuard` shape (`sema-otel/src/imp.rs:152`), **not** the non-panic-safe budget save/restore (`builtins.rs:339/362`).
- `builtins.rs`: register `workflow/run`, `workflow/phase`, `checkpoint` via the `register_fn` + `list::call_function` idiom of `otel/with-session` (`sema-stdlib/src/otel.rs:203`).
- `prelude.rs`: add `defworkflow` + `phase` macros next to `with-session` (`prelude.rs:103`). `defworkflow`-as-macro keeps the VM untouched (`Defagent`/`Deftool` are special forms at `lower.rs`, so this is a deliberate, fine departure matching the `->`/`when-let` prelude family).
- CLI: `Commands::Workflow{ … }` (mirror `Notebook`, `main.rs:287`), `--args` parsed via `sema_core::json::json_to_value`; dispatch in the main match. `init_from_env` already ran (`main.rs:473`).
- **Test seam:** read `SEMA_WORKFLOW_FIXED_TS` and `SEMA_WORKFLOW_RUN_ID`; force `dur_ms=0` under fixed ts. Neither exists yet and the golden is impossible without them.
- **Decide & freeze the run-dir location: project-local `./.sema/runs/<run-id>/`** (cwd-relative, git-ignorable), NOT `sema_home()` (`home.rs:5` is user-global `~/.sema`). The plan implies project-local but never states it.
- Journal spec at **`website/docs/internals/workflow-journal.md`** (where `bytecode-format.md` lives — `docs/internals/` does not exist).

**Code sketch (hardest part: `workflow/phase` must emit `phase.ended` on the error path).**
```rust
// crates/sema-workflow/src/builtins.rs — Sema errors are Result-valued (lower.rs CoreExpr::Try),
// so the dominant failure mode is an Err short-circuit, NOT a Rust panic. Order the emit BEFORE
// returning Err; the Drop guard covers the (rare) panic case.
fn workflow_phase(args: &[Value]) -> Result<Value, SemaError> {
    let label = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    let ctx = current().ok_or_else(|| SemaError::eval("workflow/phase outside a run"))?;
    ctx.emit(WorkflowEvent::PhaseStarted { seq: ctx.next_seq(), ts: ctx.ts(), phase: label.into() });
    let result = sema_stdlib::list::call_function(&args[1], &[]);   // dispatches into the SAME VM
    let status = if result.is_ok() { "success" } else { "failed" };
    ctx.emit(WorkflowEvent::PhaseEnded { seq: ctx.next_seq(), ts: ctx.ts(),
                                         phase: label.into(), status: status.into(),
                                         dur_ms: ctx.dur_ms() });   // 0 under fixed-ts seam
    result   // propagate Ok OR Err AFTER ended is journaled
}
```
**Runnable acceptance oracle.** `SEMA_WORKFLOW_FIXED_TS=0 SEMA_WORKFLOW_RUN_ID=wf_test_0001 cargo run -p sema -- workflow run crates/sema/tests/fixtures/workflow/hello-wf.sema --args '{"name":"x"}' --run-dir /tmp/wfspike`, where the fixture emits run.started → phase(Inventory) → checkpoint → phase.ended → phase(Audit) → checkpoint → phase.ended → run.ended. Assert: `diff` against committed `hello-wf.events.jsonl` is empty; `jq -c 'select(.event=="run.ended")|.status'` == `"success"`; `seq` monotonic; run twice → byte-identical; `(checkpoint :files)` read inside Audit returns the Inventory value. Wire as one integration test via `env!("CARGO_BIN_EXE_sema")`. Negative oracle: delete one golden line → diff non-empty.
**Effort.** M, 4–5 days. Risk multiplier: quasiquote-splicing a literal `{…}` meta map in `defworkflow`; the try/catch-around-`phase` judgment.
**Dependencies.** None hard. Cassette NOT needed (no real LLM in the fixture). Canonical serialization NOT needed (checkpoint `value_digest` can be lossy here). OTel dual-emit optional/inert (no-op when disabled — `imp.rs:1263`). Spike 1 is independent of Spike 0.

---

### Spike 2 — Cassette-backed `audit-auth` demo (the offline-CI oracle)

**Goal.** Ship `audit-auth.sema` (inventory → audit → verify → report) over a fixture repo with N=3 known missing-auth findings, replayed deterministically and green in CI with no key, driven by the **already-shipped** cassette layer (`crates/sema-llm/src/cassette.rs`). Load-bearing de-risk: prove `compute_cache_key` (`builtins.rs:4475`) does NOT collide across tool-loop rounds.

**Concrete steps.**
- Depends on Spike 1's `defworkflow`/`phase`/`checkpoint`/`{:status}` (verified absent today — no `sema-workflow` crate). If Spike 1 isn't done, build its minimal slice first.
- Fixture repo `crates/sema/tests/fixtures/audit-auth/repo/` (4–6 PHP files, exactly 3 findings) + `expected-findings.json`. **Sort the dir listing** in `inventory` — unsorted iteration order leaks into prompt content and silently drifts cassette keys (O16).
- `audit-auth.sema` with at least one auditor agent making a **2-round** tool loop (round 1 tool call, round 2 final reply) so the multi-round cassette path is exercised.
- RECORD: harness in `crates/sema/tests/workflow_audit_auth_test.rs` reusing `crates/sema/tests/llm_cassette_test.rs` pattern (`reset_runtime_state()` + `register_test_provider(FakeProvider)`); script the FakeProvider (`sema-llm/src/fake.rs`) `.tool_call(...)` then `.reply(...)`; wrap in `(llm/with-cassette "<tape>" {:mode :record} …)`; commit the tape.
- REPLAY: re-run with `{:mode :replay}` and a FakeProvider built with `.error(...)` (the "must not be called on replay" trick) — every round must be served from the tape.

**Code sketch (hardest part: the anti-collision proof — and why Spike 2's happy path under-tests it).**
```rust
// compute_cache_key (builtins.rs:4475) hashes ONLY model+temp+system+role+content.to_text();
// to_text() returns "" for a tool-call-only assistant turn. Round 2's key differs from round 1
// ONLY because round 2 appends a tool_result whose content text is non-empty.
//   => Scripting DISTINCT round texts proves keys differ for a reason ORTHOGONAL to the bug.
//   => The real danger (a re-asked agent emitting IDENTICAL visible text twice) is NOT exercised.
// Add a SECOND assertion that targets the danger directly:
assert!(canonical(&replayed).contains("missing authorize"),   // round-2 distinct content survived
    "round-2 finding lost -> keys collided; extend compute_cache_key before building the runtime");
// And a dedicated unit test: two rounds whose ADDED content text is identical/empty must produce
// DISTINCT keys once compute_cache_key also hashes msg.tool_calls(id+name+args)+tool_call_id+json_mode.
```
**Runnable acceptance oracle.** `cargo test -p sema --test workflow_audit_auth_test` GREEN, no network/key, run twice byte-identical. Asserts: `:status :success`; `(count (:findings result)) == 3` == expected; canonical result == committed `result.golden.json`; the 2-round auditor's replayed finding is round-2's distinct content; deleting one tape line → `Err` whose Display contains `"cassette miss"` (`cassette_miss_error`, `builtins.rs:4867`). **Add the missing determinism gate (O16): record twice, assert tapes byte-identical** — without it "byte-deterministic" rests on an unverified orchestrator-determinism premise the `inventory` dir-walk likely violates.
**If the anti-collision assertion fails:** land the contained fix to `compute_cache_key` (hash `request.tools`, each `msg.tool_calls` id+name+args, `tool_call_id`, `json_mode`, `max_tokens`, all length-prefixed to fix the existing separator bug), bump `TapeEntry.v` (`cassette.rs:53`), re-record. Do this BEFORE building the runtime on top.
**Effort.** M, 3–4 days (+1d contingency for the key fix; +2d if Spike 1 slice must be built inline).
**Dependencies.** SHIPPED: cassette layer + FakeProvider. BLOCKING: Spike 1 runtime. NOT a dependency: canonical serialization (cassette keys on LLM request, not Sema step inputs — corrects plan §3.3).

---

### Spike 3 — Bounded parallel + structured step output

**Goal.** `workflow/foreach`/`parallel` over the existing async substrate + `:schema`-validated step output with bounded repair-retry, so `verify` branches on typed findings and a malformed result yields one repair then a clean `{:status :failed}`.

**Concrete steps.**
- Precondition: Spike 0's combinator + GATE conclusion ("in-process async sufficient for structure, no new Task type" — confirmed).
- `workflow/foreach`/`parallel` in `prelude.rs` (or a loadable module) over `async/spawn` + semaphore channel + `async/all`. **Order is free** (`async/all` input-ordered, `async_ops.rs:266`) — no mutable result vector. **Workers MUST catch their own error and resolve to a tagged map**, because `async/all` aborts the whole batch on the first `Rejected`/`Cancelled` (`async_ops.rs:257`).
- Expose `validate_extraction` (`builtins.rs:4384`, currently private `fn`) as a new builtin `llm/validate` returning `nil` on OK or `{:status :invalid :errors "…"}` — lets the step validate without a second JSON-coercion LLM call.
- `workflow/typed-step`: run worker → `(try (json/decode raw) …)` (**catch parse failure here** — `llm/extract` treats a parse error as a hard, non-retried error that escapes its loop, `builtins.rs:1812`) → `llm/validate` → on parse/validation failure re-run with a reask suffix, capped at `:retries` (default 1); on exhaustion return `{:status :failed :reason :schema-invalid}`, never throw.
- **Rewrite the sketch to avoid `loop`/`recur` (they don't exist — `prelude.rs` has only `dotimes`/`for-range`).** Use a recursive helper fn or a `for-range` over attempts.

**Code sketch (hardest part: bounded no-bare-throw step + the missing `loop`/`recur`).**
```sema
;; NO loop/recur in the language — use a recursive helper fn.
(defn typed-step-attempt (worker schema reask attempt retries)
  (let ((raw    (worker reask))
        (parsed (try (json/decode raw) (catch _ :PARSE-FAIL))))
    (cond
      (= parsed :PARSE-FAIL)
        (if (< attempt retries)
            (typed-step-attempt worker schema "Reply ONLY valid JSON." (+ attempt 1) retries)
            {:status :failed :reason :schema-invalid :error "unparseable JSON"})
      :else
        (let ((v (llm/validate parsed schema)))         ; nil = OK, else {:errors …}
          (if (nil? v)
              {:status :success :value parsed}
              (if (< attempt retries)
                  (typed-step-attempt worker schema (:errors v) (+ attempt 1) retries)
                  {:status :failed :reason :schema-invalid :error (:errors v)}))))))
```
**Runnable acceptance oracle.** `cargo test -p sema --test workflow_parallel_test` GREEN, no network, via FakeProvider + the `llm_fake_test.rs` harness. ORDER: `foreach` over `'(0..7)` with reversed-sleep leaves returns `'(0..7)`. BOUND: assert capacity is never exceeded (structural; `peak <= N`, not `== N` — the counter window races, O3). REPAIR-SUCCESS: FakeProvider `.reply("{not json")` then `.reply("{\"confirmed\":true}")` → `{:status :success}` with recorder call_count==2. REPAIR-FAIL: malformed twice → `{:status :failed :reason :schema-invalid}`, eval is `Ok` (no panic, no bare throw), call_count==2.
**Open scope boundary to state explicitly (O13/O14):** (a) `validate_extraction` is **shallow** — top-level keys only, `:type` in {string,number,boolean,list}, no nested/list-element typing — so the plan's §3.1 verify phase filtering a list-of-findings on `:confirmed` is *not expressible* without extending it; the verify phase must use a flat schema or the validator grows. (b) A `typed-step` inside a `foreach` worker re-introduces the blocking-leaf trap (its `agent/run` is `block_on`), so BOUND-interleaving and typed-step **cannot both hold for the composed parallel-typed-agent feature** — test in isolation, document the composition gap.
**Effort.** M, 4–6 days (pure-Sema combinators + tests ~2d; `llm/validate` + parse-error catch ~1d; the no-bare-throw + unwind-safe-token tax +1–2d).
**Dependencies.** Spike 0 (substrate), Spike 1 (journal + envelope). Cassette shipped, not blocking. Independent of Spike 4's canonical serializer. Hard scope boundary: real concurrent LLM I/O stays out.

---

### Spike 4 — Resume via content-keyed memoization

**Goal.** `workflow/resume <run-id>` re-runs the deterministic skeleton, short-circuits steps whose content key is already journaled with a result, re-runs steps with a `start` but no `result` (conservative-resume). The one new primitive: a canonical, cross-process-stable `Value→bytes` encoder.

**Concrete steps.**
- HARD-BLOCKED on Spike 1 (resume reads `.sema/runs/<run-id>/events.jsonl` + the `WorkflowCtx`).
- New `crates/sema-core/src/canonical.rs`: `canonical_bytes(&Value, &mut Vec<u8>)` with a FROZEN tag set **separate** from `serialize.rs` VAL_* (which churns with the bytecode format and writes process-unstable string-table indices — `serialize.rs:108`). Rules: distinct tags for String/Symbol/Keyword (fixes `value_to_json` key collapse, `json.rs`); strings written **inline** via `resolve(spur)`, never an index; ints i64-LE; floats normalize `-0.0→+0.0` and **REJECT NaN/Inf** (a NaN is never `==` itself — `value.rs:1737`); Map and HashMap → **same tag**, entries sorted by **key bytes** (not `Value::Ord`, broken for nested maps); list/vector u64 length (no u16 cap — `serialize.rs:77` must not leak); depth cap 128 (`serialize.rs:73`).
- **CORRECT the reject-list (O8).** `impl Hash` (`value.rs:1781`) explicitly handles `Record` (`:1823`), `F64Array`, `I64Array` — these are legitimate step-input data and the encoder MUST encode them (`Record` via stable `type_tag`+`fields`; arrays via element bytes), or those steps can never be memoized and silently re-run. Only `Map`/`HashMap`/closures/`Agent`/`Thunk`/`Stream`/`Channel`/`AsyncPromise` are in the silent `_ => {}` arm; reject the closure/agentic/stream ones explicitly, **not** records or typed arrays.
- `step_key`: SHA-256 over a domain tag then EACH field **length-prefixed** (fixes the `compute_cache_key` separator bug). `agent_fingerprint` = canonical hash of (name, system, model, sorted tool names + schemas), not the `Rc` pointer.
- Wire `checkpoint`/leaf calls to consult the journal before executing; short-circuit on a journaled result, re-run on `start`-without-`result`.
- `hash/value` builtin in `crates/sema-stdlib/src/crypto.rs` (next to `hash/sha256`) so eval tests can pin golden keys.

**Code sketch (hardest part: the encoder match must cover the real `ValueView` set).**
```rust
// crates/sema-core/src/canonical.rs — covers Record + typed arrays (Hash handles them, value.rs:1823+),
// rejects ONLY closures/agentic/stream types. Map==HashMap (same tag), sorted by key bytes.
fn go(v: &Value, out: &mut Vec<u8>, depth: usize) -> Result<(), SemaError> {
    if depth > 128 { return Err(SemaError::eval("canonical: too deep / possible cycle")); }
    match v.view() {
        ValueView::Float(f) if f.is_nan() || f.is_infinite() =>
            return Err(SemaError::eval("canonical: NaN/Infinity cannot be a stable key")),
        ValueView::Symbol(sp)  => { out.push(tag::SYMBOL);  put_bytes(out, resolve(sp).as_bytes()); } // INLINE
        ValueView::Keyword(sp) => { out.push(tag::KEYWORD); put_bytes(out, resolve(sp).as_bytes()); }
        ValueView::Map(m)      => emit_map(out, sorted_pairs(m.iter(), depth)?),     // same tag
        ValueView::HashMap(m)  => emit_map(out, sorted_pairs(m.iter(), depth)?),     // as Map
        ValueView::Record(r)   => { out.push(tag::RECORD); put_bytes(out, resolve(r.type_tag).as_bytes());
                                    /* fields as a sorted map */ }
        ValueView::F64Array(a) => { out.push(tag::F64ARR); put_len(out, a.len());
                                    for x in a.iter() { let x = if *x==0.0 {0.0} else {*x};
                                        out.extend_from_slice(&x.to_bits().to_le_bytes()); } }
        // ... Nil/Bool/Int/Float/String/Char/List/Vector/Bytevector/I64Array ...
        other => return Err(SemaError::eval(format!(
            "canonical: {} is not stably hashable (closures/agents/streams excluded)", v.type_name()))),
    }
    Ok(())
}
```
**Runnable acceptance oracle (three).**
1. **Cross-process stability** (the property `serialize_value` lacks): a throwaway `key-probe` bin builds a fixed Value set in randomized intern order (nested maps inserted a-then-b vs b-then-a; a hashmap vs btreemap with identical contents; string-key `"a"` vs keyword `:a`; `-0.0`; a 70000-element list) and prints `canonical_sha256` per line. Run twice in fresh processes (decoy-intern first in run B); `diff` MUST be empty. CONTROL: the same set through `serialize_value` (`serialize.rs:87`) MUST differ across processes.
2. **Encoder semantics** via `eval_tests!`: `(= (hash/value {:a 1}) (hash/value (hash-map :a 1)))` ⇒ true; `(= (hash/value {:a 1}) (hash/value {"a" 1}))` ⇒ false; `(= (hash/value {:a 1 :b 2}) (hash/value {:b 2 :a 1}))` ⇒ true; NaN-in-map errors.
3. **Resume short-circuit** (`cargo test -p sema --test workflow_resume_test`): **fix the ambiguous oracle (O7)** — counting FakeRecorder calls cannot distinguish "memo-skipped" from "served from the still-on-disk cassette." Assert the step's *executor body never ran* via a side-effect counter incremented inside the Sema step (or that no new `agent.started` event was re-emitted), not just zero provider calls. Then: truncate `events.jsonl` after step 2's result → resume → steps 1+2 bodies don't run, step 3 does; and a `start`-without-`result` step re-runs.
**Resolve the code-version contradiction (O10).** The three findings prescribe whole-program `source_hash` (`serialize.rs:836`) / "too coarse" / per-defworkflow-form hash inconsistently. **Decide now: per-workflow form hash** (a comment elsewhere must not invalidate every step) and require Spike 1's `metadata.json` to record it — otherwise Spike 4 must re-open Spike 1's frozen contract.
**Flag the Map==HashMap read-back inconsistency (O9).** The encoder collides the two map reps intentionally, but `Value::PartialEq` treats them unequal (no cross arm). A step keyed "same" can hand back a representation the deterministic skeleton then `=`-compares as different downstream. Add a read-back-and-compare test; if it bites, normalize checkpoint values to one map rep on store.
**Effort.** L, 5–7 days (encoder + cross-process tests ~2d is the novel core; short-circuit wiring ~2d; the normalization spec for float/JSON downstream inputs is the risk multiplier).
**Dependencies.** HARD-BLOCKED on Spike 1 (+ its per-form code-version). DEPENDS on R4 for stable downstream keys (agent/run strings + low-bit float drift need a per-step normalization spec — R2 and R4 are more coupled than the plan presents). Cassette shipped, used as the "no agent re-called" verifier. Does NOT fix `Value::Hash` (separate path, leaves `value.rs:1781` untouched — the safe choice).

---

## Plan corrections (deduped)

- **§3.2 option (2) — ~~"reuse the async scheduler for in-process concurrent LLM calls is mostly wiring" is WRONG~~ SUPERSEDED 2026-06-23 → the scoping doc was directionally RIGHT.** The "yield-aware bridge" the original correction said was missing has now been *built* (the `AwaitIo` yield, `async_signal.rs:119`), and the wiring is done: `set_yield_signal(AwaitIo)` now fires for `llm/complete`/`classify`/`extract`/`embed` (`builtins.rs:2842,5346`), so in-process single-shot LLM calls *do* run concurrently on `async/spawn`/`async/pool-map`. The original correction was true *at the time* (it was a net-new bridge, not "mostly wiring") but the bridge has since shipped. The one residual: **multi-round** `agent/run` still can't yield mid-`run_tool_loop` (deferred). Original text struck through:
  ~~`sema-llm` has zero `set_yield_signal`; every call is `runtime.block_on` (`anthropic.rs:429`). In-process LLM "concurrency" degrades to **sequential**; making it concurrent needs a new yield-aware bridge, not wiring.~~
- **§3.2 — ~~"`llm/pmap`/`llm/batch` … generalizing that to arbitrary steps is mostly wiring" is FALSE~~ SUPERSEDED 2026-06-23.** Concurrent single-shot LLM fan-out now generalizes to arbitrary Sema items via `(async/pool-map llm/complete items n)` (`prelude.rs:134`) — each `llm/complete` is a first-class native (`builtins.rs:1521`) that yields `AwaitIo`, so this is no longer limited to the `Send`-struct `batch_complete` path. (Multi-round agent steps remain the exception — see the deferred agent-loop rewrite.) Original text struck through:
  ~~`llm/pmap` maps the Sema fn **sequentially** ("since Rc", `builtins.rs:~2394`); concurrency is only `provider.batch_complete`'s `join_all` over `Vec<ChatRequest>` (`anthropic.rs:441`) — plain `Send` structs, never arbitrary `Rc`-VM steps.~~
- **§3.2 — ~~"subprocess fan-out" has ZERO existing primitive~~ SUPERSEDED 2026-06-23.** `shell` now yields `AwaitIo` in async context and runs on a shared background runtime (`system.rs:199,241`), so subprocess leaves DO compose with the scheduler (a child wait no longer stalls the one thread), and a cancelled child is **SIGKILLed as a process group** (`system.rs:104-151`) — the `YieldReason::ProcessWait` the original correction asked for is subsumed by `AwaitIo`. Original text struck through:
  ~~Only `shell` = blocking `Command::output()` (`system.rs:53`); no `Stdio`/`spawn`/`Child`/`kill`. It is net-new Rust, and it does not compose with the async scheduler (a child wait stalls the one thread) without a new `YieldReason::ProcessWait`.~~
- **§3.3 / §4-R2 — canonical serialization is bigger than "stable key ordering."** `Value::Hash` silently drops `Map`/`HashMap`/closures (`value.rs:1854`), `Value::Eq` treats hashmap≠btreemap, `serialize_value` is process-unstable (`serialize.rs:108`), `value_to_json` collapses key types and errors on NaN. A dedicated encoder is required. (But note Hash *does* handle Record/F64Array/I64Array — the encoder must too.)
- **§3.3 — "this is exactly how `llm/with-cache` already works, generalized to steps."** `compute_cache_key` (`builtins.rs:4475`) hashes only model+temp+system+message-text on a Rust `ChatRequest`, no separators, ignores tools/tool_calls/json_mode; it is not a reusable canonical-Value hasher.
- **§3.3 — canonical serialization is NOT a cassette prerequisite.** The cassette keys on the LLM request (Rust-side SHA-256), not Sema step inputs. Spike 2 is unblocked by it; only Spike 4 needs it.
- **§3.3 — the named companion `docs/plans/2026-06-21-llm-cassettes.md` DOES NOT EXIST.** The cassette layer ships at `crates/sema-llm/src/cassette.rs` + `llm/with-cassette` (`builtins.rs:3716`) regardless; the "lockstep doc" reference is phantom.
- **§3.4 — "reuse the `llm/extract` validator + reask machinery" is not a drop-in for agent steps.** `llm/extract` (`builtins.rs:1743`) is hardwired to `llm/complete`+`json_mode`, treats a JSON **parse** failure as a hard error that escapes the retry loop (`builtins.rs:1812`); only validation failures are repaired. The reusable pieces are the **private** `validate_extraction` (`builtins.rs:4384`) and `format_reask_prompt` (`builtins.rs:4344`) — expose `validate_extraction` as `llm/validate`. The validator is **shallow** (top-level keys only) — it cannot express the §3.1 verify phase's nested finding shape.
- **§3.5 — "when `SEMA_OTEL=1` the same events also become OTel spans (shared sink)" is wrong twice.** The live switches are `SEMA_OTEL_FILE` / `OTEL_EXPORTER_OTLP_ENDPOINT` (`imp.rs:396`), not `SEMA_OTEL=1`; and the OTel JSONL exporter's `span_to_json` **drops span events** (`file_exporter.rs:119`), so it cannot serve as the journal. It is **dual-emit** (own `events.jsonl` + `sema_otel::add_event`/`vm_span`, no-op when disabled) — two files, two schemas.
- **§3.5 — journal spec location.** `docs/internals/` does not exist; `bytecode-format.md` lives at `website/docs/internals/`. Put the journal spec there to truly mirror the single-source-of-truth pattern.
- **§3.5 — events.jsonl determinism is not free.** `serde_json` has no `preserve_order` (`Cargo.toml:50`) → map-encoded events sort keys alphabetically. Use a fixed `#[derive(Serialize)] enum`, and add a clock/run-id test seam — neither exists.
- **§3.5 / R4 — `agent.result` can only carry a string today** (`agent/run` returns String 2-arg, `{:response :messages}` 3-arg). Freeze the event with `output`/`output_digest` only; add typed fields later.
- **§3.5 run-dir — project-local vs user-global is unstated.** `home.rs:5` `sema_home()` is `~/.sema`; the plan implies `./.sema/runs`. Decide project-local and freeze it.
- **R1 framing is overstated.** The cooperative scheduler + channels + `async/all` + `async/cancel` already give bounded, ordered, cancellable fan-out for *yielding* work with **no new value type** — `Value::AsyncPromise` is the handle. ~~The genuine unknown is narrower: concurrent **in-process LLM I/O**, blocked by `block_on`, not by a missing scheduling primitive.~~ **UPDATE 2026-06-23: that "genuine unknown" is now RESOLVED** — single-shot in-process LLM I/O interleaves via `AwaitIo` (`builtins.rs:5346`). The remaining unknown narrows once more, to **multi-round `agent/run`** only.
- **R1 — "no `Task`/`Future`/`AgentHandle`" omits that `AsyncPromise` is effectively the handle** (`task_id` + `async/cancel`/`async/await`/`async/all`/`async/cancelled?`/`async/pending?`). State explicitly: no new type needed for structure; maybe a small one for subprocess-PID reaping.

## Open risks the skeptic surfaced (honest, unresolved)

- **The GATE tests the easy 90% and relabels the hard 10% "follow-on."** Spike 0 Track A proves a substrate that *cannot run `agent/run` fan-out* — the feature's entire purpose. The honest gate verdict is "in-process async is sufficient for sequencing/ordering/cancellation, and the real gate (concurrent LLM I/O) is BLOCKED and untested here." Consider a **Track C** that demonstrates real LLM overlap via per-item-indexed `batch_complete`, since *that* is what R1 actually gates. (O6)
- **Spike 0's own sketch doesn't run as written.** No `make-vector`/`vector-set!`/`atom` (verified absent); the BOUND counter races across the acquire→increment yield window so "peak == 3" is an artifact, not a proof (assert `<= N` structurally); the CANCEL `(filter async/pending? …)` is vacuously satisfiable; Track B's `sh sleep` timing would print "sequential" even if the trap were fixed (needs the async-vs-shell contrast). (O1–O5)
- **Spike 2's anti-collision assertion can pass while the bug is present.** Scripted *distinct-text* rounds make keys differ for a reason orthogonal to the collision; the real danger (a re-asked agent emitting identical visible text twice) is untested. Add a dedicated unit test on identical-added-content rounds. (O11)
- **Spike 4's resume oracle is ambiguous.** Zero provider calls is satisfiable by the on-disk cassette alone; assert the *step body never executed*, not "no LLM call." (O7)
- **Map==HashMap collision contradicts `Value::Eq`.** Intentional key collision + `hashmap ≠ btreemap` equality can re-run *downstream* steps after a memo hit hands back the "wrong" representation. Untested read-back path. (O9)
- **code-version is specified three incompatible ways.** Whole-program vs "too coarse" vs per-form. Decide per-form now or Spike 4 re-opens Spike 1's frozen `metadata.json`. (O10)
- **`loop`/`recur` don't exist**, so Spike 3's typed-step sketch is non-buildable as written (use a recursive helper). (O12)
- **Composed feature (parallel typed agents) hits the blocking-leaf trap on every worker** — BOUND-interleaving and typed-step can't both hold for the real shape; the spikes test them apart and hide it. (O13)
- **The verify-phase validator is too shallow for its own example** (no nested/list-element typing). Either restrict verify to flat schemas (document it) or extend the validator (beyond the spike's effort). (O14)
- **Orchestrator determinism is assumed, never enforced.** An unsorted `inventory` dir walk leaks into prompt content and silently drifts cassette keys until a `lookup` miss. Add a "record twice ⇒ identical tape" gate. (O16)
- **Panic-safety is the wrong justification, though the conclusion is fine.** Sema errors are `Result`-valued (`lower.rs` `CoreExpr::Try`), so the dominant failure is an `Err` short-circuit the manual budget save/restore *already* handles; copy the Drop guard defensively, but the load-bearing case is ordering the `phase.ended` emit before the `Err` return. (O15)

## Recommended first move

**Build Spike 0 Track A only, and only far enough to settle the GATE — starting with the one missing primitive.** Concretely: add a minimal `atom`/`reset!`/`deref` (or decide BOUND can be proven structurally and skip it), write `workflow/foreach` in `fuzz/spike0-foreach.sema` relying on `async/all`'s verified input-ordering (`async_ops.rs:266`), and prove **ORDER + BOUND(≤N) + CANCEL** plus the **async-vs-`shell` wall-clock contrast** (Track B).

**Gate it must pass:** `./target/release/sema fuzz/spike0-foreach.sema` returns `(0 1 2 3 4 5 6 7)` in input order with the semaphore never exceeding N concurrent past-`recv` tasks, `async/cancel` makes outstanding promises `async/cancelled?`, and the `async/sleep` leaf runs ≪ the `shell` leaf (≈ sum of sleeps). If that holds, the structural substrate is proven with **no new value type** and the verdict is recorded; if BOUND/interleaving fails, the feature scopes to sequential-only or commits to a subprocess/`YieldReason::ProcessWait` build **before** anything else is written. This is the smallest thing that either unblocks Spike 3 or kills the parallel pitch — and it costs ~1 day, not the ~1 day the plan claimed *plus* the missing primitives it didn't account for.

> **POST-1.27.0 NOTE (2026-06-23).** The "Recommended first move" above (build Spike 0 Track A to settle
> the GATE) is now **largely moot** — `async/pool-map` (`prelude.rs:134`) *is* the proven substrate,
> shipped with the exact semaphore + `async/all`-input-order recipe Track A specified, and the
> async-vs-`shell` overlap contrast was verified live (CHANGELOG 1.27.0). The GATE is passed. The first
> move is now **Spike 1** (sequential runtime + frozen journal) — see the section below.

---

## Remaining blockers to start implementation (2026-06-23, post-1.27.0)

**Verdict: implementation can start NOW.** The Spike-0 concurrency GATE is settled by shipped code, so
the critical-path is the orthogonal-but-still-unbuilt work. There is **one HARD blocker** to the
*parallel* feature (a missing mutable-cell primitive, and only for one narrow use), and a short list of
caveats to design around. Nothing blocks **Spike 1** at all.

### (a) HARD blockers — must build/decide before the dependent spike

| # | Blocks | Blocker (code-grounded) | Cheapest path |
|---|---|---|---|
| **H1** | Spike 4 (resume) | **Canonical, cross-process-stable `Value→bytes` encoder does not exist.** `serialize_value` writes process-unstable string-table *indices* (`serialize.rs:108`); `Value::Hash` silently drops `Map`/`HashMap`/closures (`value.rs:1854`); `value_to_json` collapses string/keyword/symbol keys and errors on NaN. Resume short-circuiting is unsound without it. | Build `crates/sema-core/src/canonical.rs` + `hash/value` builtin exactly as Spike 4 specifies (inline-interned strings, sorted-by-key-bytes maps, NaN/Inf reject, Record/typed-array *encoded*). ~2d. **Only blocks Spike 4** — Spikes 1/2/3 do not need it. |
| **H2** | Spike 2 multi-round replay | **`compute_cache_key` (`builtins.rs:4475`) ignores `tools`/`tool_calls`/`tool_call_id`/`json_mode` and has no field separators** — a re-asked agent emitting identical visible text can collide tape entries. | Land the contained fix (hash those fields, length-prefixed), bump `TapeEntry.v` (`cassette.rs:53`), re-record. ~1d. Gate it with the dedicated identical-added-content unit test before building the runtime on top. |
| **H3** | The BOUND *counter* assertion only (NOT the substrate) | **No `atom`/`box`/`make-vector`/`vector-set!`/`swap!` mutable cell in the stdlib** (re-verified absent 2026-06-23). | **Largely avoidable:** `async/pool-map` already enforces BOUND *structurally* (capacity-N channel), and `async/all` gives ORDER for free — neither needs a cell. Add a minimal `atom`/`reset!`/`deref` (~0.5d) **only if** a workflow needs run-scoped shared mutable state (Mastra-style `state` bag) or a peak-counter assertion. Decide when designing `checkpoint`'s state-bag, not before. |

### (b) Caveats to design around — not blockers, but must be honored

| # | Caveat | Why it's not a blocker | Cheapest path to design around |
|---|---|---|---|
| **C1** | **ASYNC-1: dynamic-scope flags vs deferred async tasks** (`docs/deferred.md`). `llm/with-cache`/`with-budget`/`:tags` are thread-locals read *when the task executes*, which the scheduler can defer past the thunk's return → a budget attached to a concurrent fan-out may not gate it, and cache-miss accounting under-reports. | Caching *correctness* still works in-extent (same-prompt repeat is a hit); the gap is accounting/visibility and **budget-gating of concurrent tasks**. Sequential workflows (Spike 1) are unaffected. | For v1, **attach budgets at the sequential boundary**, not inside a `pool-map` worker — i.e. enforce the run-level budget *before/after* a fan-out, not per-concurrent-call. The real fix (snapshot dynamic scope onto each task at `async/spawn`, reinstall on run — like the shipped per-task OTel swap) is its own task; defer it. **Document the gap in the workflow `:budget` contract.** |
| **C2** | **Multi-round `agent/run` does not overlap.** The monolithic `run_tool_loop` native frame holds `messages`/round-counter/`Rc` otel guards across `do_complete` inside a Rust `for`, so it can't yield mid-loop (`async-agent-parallelization.md` §2 — deferred "major rewrite"). | Single-shot `llm/complete`/`classify`/`extract`/`embed` **do** overlap; subprocess `.sema` workers overlap. Only an in-process *tool-using multi-round agent* fanned out N-at-a-time serializes. | Scope the parallel feature's leaves to **single-shot LLM composition** (`async/pool-map llm/complete …`) or **subprocess workers** for v1; keep in-process multi-round `agent/run` fan-out out of scope and document it. (This is the surviving slice of the old Spike-0 recommendation.) |
| **C3** | **`agent/run` structured output is still string-only** (2-arg returns `Value::string`; 3-arg `{:response :messages}` — `builtins.rs:~2354`), and `validate_extraction` is **shallow** (top-level keys only, `builtins.rs:4384`). | Doesn't block the journal or the parallel substrate; only constrains the *typed-findings* shape Spike 3's `verify` phase can express. | Freeze `agent.result.output` as opaque string/digest in the journal (Spike 1 already specifies this), use **flat schemas** in the verify phase, and expose `validate_extraction` as `llm/validate`. Grow the validator only if a nested-finding schema becomes load-bearing. |
| **C4** | **No `loop`/`recur`** (only `dotimes`/`for-range` — `prelude.rs:81,91`). | Cosmetic — affects only the Spike 3 `typed-step` sketch as written. | Use a recursive helper fn or `for-range` over attempts (Spike 3 sketch already rewritten this way). |
| **C5** | **Determinism is assumed, not enforced** — an unsorted `inventory` dir walk leaks into prompt content and drifts cassette keys (O16). | Doesn't block building; it's a correctness gate to add. | Sort dir listings in `inventory`; add a "record twice ⇒ byte-identical tape" gate to Spike 2. |

### Can implementation start now, and on what?

**Yes — start with Spike 1 (sequential runtime + frozen JSONL journal).** It has **zero hard blockers**:
no concurrency (the journal is sequential), no canonical serialization (checkpoint digests can be lossy),
no cassette. It is the foundation Spikes 2/3/4 all depend on, and it freezes the ~8-event vocabulary the
dashboard scope (`2026-06-23-workflow-dashboard-scope.md`) waits on.

**Order:** Spike 1 → Spike 2 (build H2's `compute_cache_key` fix as its first step) → Spike 3 (now a
*thin* layer: wrap the shipped `async/pool-map` + `:schema` validation; honor C1/C2/C3) → Spike 4 (build
H1's canonical encoder first). The parallel substrate Spike 3 used to gate on is **already shipped**, so
Spike 3 collapses from "build bounded fan-out" to "compose the shipped `async/pool-map` with structured
step output and document the C2/C3 leaf-shape boundary."
