# P6-3: WASM Promise-driven roots — design + acceptance gate

Status: **DESIGN / NOT LANDED** (hard-fallback). The shipped replay+Atomics
mechanism in `crates/sema-wasm/src/lib.rs` is unchanged. This document is the
concrete design seam and the acceptance gate for the eventual landing, plus a
record of exactly what blocked real-browser verification in the fallback
session so the next attempt starts from a known state.

This is the WASM slice of Task 07
(`docs/plans/2026-07-13-unified-cooperative-runtime-task-07.md`, Task 5) and the
inventory rows **H10B** (WASM evaluation replay) and **H10C** (WASM synchronous
worker waits) in `docs/internals/async-runtime-inventory.md`.

## 1. What ships today (the working mechanism — do not break blindly)

`crates/sema-wasm/src/lib.rs` has two async mechanisms:

1. **HTTP = replay-with-cache.** An `http/get`/`http/post` (registered at
   `lib.rs:1038` / `:2486`) does not perform the fetch; it *throws* an
   `HTTP_AWAIT_MARKER` (`:192`) `SemaError::Eval` whose message JSON-encodes the
   request (`http_await_marker`, `:224`). Three async entry points catch it and
   loop up to `MAX_REPLAYS = 50` (`:193`):
   - `evalAsync` / `evalVMAsync` — `:1870`, `:1957`
   - `runEntryAsync` — `:2822`

   Each iteration clears output, re-runs the *whole program* via
   `eval_str_in_global`, and on a marker throw performs the real `fetch()`
   (`perform_fetch_from_marker(...).await`), inserts the response into
   `HTTP_CACHE`, and re-runs from the top. The cache makes the previously-issued
   request resolve synchronously on the replay. **Every non-idempotent side
   effect before the fetch re-executes on every replay** (the known defect).

2. **Sleep.** Main thread: an instant virtual clock (ctx-less yield-signal
   bridge). Worker (`playground/src/sema-worker.js`): a real `Atomics.wait` on a
   `SharedArrayBuffer` slot, installed by `installAtomicsSleep` (`:3094`), which
   calls `sema_core::set_blocking_sleep_callback(worker_atomics_sleep)` (`:48`).
   SAB slot 0 doubles as the sleep cell and the cancel flag polled by
   `worker_check_interrupt` (`:61`) via `set_interrupt_callback` (`:3100`).

## 2. Target design (Promise-driven roots)

### 2.1 The Promise seam

`eval(code)` (and its aliases `evalAsync`/`evalVMAsync`/`runEntryAsync`) returns
a JavaScript `Promise`. Internally:

- One call = **one root submitted once** to the interpreter's unified `Runtime`
  (`Runtime::submit_root`, `crates/sema-vm/src/runtime/state.rs:695`). The
  program body executes exactly once; there is no replay loop.
- A process-global `RootId -> { resolve, reject }` table (the "Promise table")
  lives in a `thread_local!` `RefCell<HashMap<RootId, PromiseSettlers>>`. On
  submit, a `js_sys::Promise` is constructed with an executor that stashes its
  `resolve`/`reject` functions in the table keyed by the returned `RootId`.
- The wasm crate never `.await`s inside `eval`. `eval` submits, kicks the
  macrotask driver, and returns the `Promise` immediately.

### 2.2 The macrotask driver (`crates/sema-wasm/src/driver.rs`, new)

The browser is single-threaded with no reactor, so the runtime's `drive` is
pumped across **macrotask** turns:

- `schedule_drive()` posts a macrotask via `MessageChannel` (preferred — a true
  macrotask that yields to rendering/input) or `setTimeout(0)` fallback. It is
  idempotent: a `DRIVE_SCHEDULED` flag coalesces multiple wake requests into one
  pending turn.
- Each turn calls `Runtime::drive(&DriveBudget)` with a **bounded** budget
  (instruction/time quantum, reuse `WASM_DEBUG_INSTRUCTION_BUDGET`), then drains
  `Runtime::poll_result` for every settled root and calls the matching
  `resolve`/`reject` from the Promise table, removing the entry.
- If `DriveState` reports remaining runnable work OR any root is still pending,
  it calls `schedule_drive()` again for the next macrotask. When the runtime is
  idle with no pending external waits, it stops scheduling (no busy loop).

This yields to the event loop between turns, so the page stays paintable and
input-responsive — the property H10C's synchronous `Atomics.wait` violated.

### 2.3 External tier = JS-callback completions

HTTP and timers become `WaitKind::External` operations whose async tier is
completed by JS callbacks, not by a thread pool (there is none in the browser).
The seam mirrors `PreparedExternalOperation`
(`crates/sema-stdlib/src/runtime_offload.rs`) but the "executor" is the browser:

- `http/get` builds an `ExternalCompletion` slot: it registers the wait with the
  runtime (obtaining a `CompletionSender`/`RuntimeCommandHandle`), returns
  `NativeOutcome::Suspend(External)`, and kicks off `fetch(url, opts)` in JS.
  When the promise resolves, a small JS shim calls an exported
  `resolve_external_wait(rootId_or_completionId, payloadJson)` which enqueues the
  decoded response as an `ExternalCompletion` through
  `RuntimeCommandHandle::complete(...)` and calls `schedule_drive()`. The next
  macrotask turn delivers the completion to the suspended continuation. **The
  program body never re-runs**; the `http/get` call site resumes with the real
  response value.
- `async/sleep` / timers register an External wait and call
  `setTimeout(cb, ms)`; the `cb` completes the wait the same way and schedules a
  drive turn. This replaces both the virtual clock and the `Atomics.wait`
  worker path — one mechanism, main thread, no SAB.

Only send-safe data crosses the JS-callback boundary: an opaque completion id
plus a serialized payload (JSON string / bytes), never a `Value` or VM state —
satisfying the Task 07 "only send-safe commands cross from browser callbacks"
constraint. Decoding JSON→`Value` happens inside the External wait's decoder on
the VM turn, exactly as the native `runtime_offload` decoder does.

### 2.4 Cancel routing

Playground "Stop" routes through `RuntimeCommandHandle::cancel_root(rootId,
reason)` instead of setting the SAB cancel flag. Because the driver is
macrotask-based on the main thread (or a worker that pumps the same runtime),
cancel is delivered as a runtime command and observed at the next drive turn /
suspension point. `installAtomicsSleep`, `worker_atomics_sleep`,
`worker_check_interrupt`, `set_blocking_sleep_callback`, and
`set_interrupt_callback` are all deleted; the SAB disappears from the worker
protocol.

### 2.5 Output

Root-tagged output sink (`crates/sema-wasm/src/output.rs`, new): each
`OutputEvent` carries its `RootId` so concurrent roots' `println` streams stay
attributable when the playground runs two evaluations at once.

## 3. Deletion inventory (what landing removes)

Rust (`crates/sema-wasm/src/lib.rs`):
- `HTTP_AWAIT_MARKER` (`:192`), `MAX_REPLAYS` (`:193`)
- `http_await_marker` (`:224`), `is_http_await_marker` (`:259`),
  `parse_http_marker` (`:267`), the `HTTP_CACHE` + `clear_http_cache` replay
  cache, `perform_fetch_from_marker`'s replay coupling
- the three replay loops (`:1870`, `:1957`, `:2822`)
- `SLEEP_I32` (`:33`), `worker_atomics_sleep` (`:48`),
  `worker_check_interrupt` (`:61`), `installAtomicsSleep` (`:3094`), and the
  `set_blocking_sleep_callback` / `set_interrupt_callback` installs (`:3097`,
  `:3100`)

JS (`playground/src/sema-worker.js`, `worker-client.js`, `app.js`):
- SAB (`sab`) allocation, `installAtomicsSleep`, `Atomics.store` cancel flag
- replaced by: root-id-tagged eval/output/completion/cancel messages, a
  `fetch().then(resolve_external_wait)` shim, and `setTimeout`-based timers

Inventory rows **H10B** and **H10C** in
`docs/internals/async-runtime-inventory.md` move from `LEGACY` to removed; the
`runtime-match-map.tsv` H10B/H10C rows are re-reconciled via
`scripts/check-unified-runtime-inventory.sh --write-mapping`.

## 4. Trace obligation

The External-HTTP completion and the timer completion carry a payload into a
suspended continuation. Any new continuation/wait type that holds a `Value`
(e.g. a decoded response bound into the resume) needs a `Trace` impl and an
edge-count test, per the CORE-2 GC invariant (I2). The JS-callback boundary must
carry only serialized bytes/JSON + an opaque id (no `Value`), so the `Value`
only materializes inside the decoder on the VM turn — that decoder's output
value is traced by the existing External-wait machinery, but a new resume record
must be audited for a `Trace` impl before landing.

## 5. Acceptance gate (the only valid oracle: a real browser)

Landing is permitted ONLY with a real-browser transcript proving, against a
`wasm-pack`-built bundle served to a headless Chromium:

- **(a) HTTP once, no replay.** An `http/get` inside evaluated Sema returns real
  response data via the returned `Promise`, and a side effect placed *before*
  the `http/get` in the same program executes **exactly once** (assert a
  `println`/counter fires a single time — the direct refutation of replay).
- **(b) Timer-driven sleep.** `async/sleep` (and a timer-based async op)
  completes via `setTimeout`, with the page remaining responsive between turns
  (a drive-turn/macrotask boundary is observable).

Plus: two concurrent `eval` calls stay pending and settle fairly with distinct
root ids; Stop cancels one exact root while the other completes; the source scan
finds no `HTTP_AWAIT_MARKER` / `MAX_REPLAYS` / `installAtomicsSleep` /
`Atomics.wait`.

**Step 4 scoping note (source-scan clause):** the old replay/Atomics machinery
is deliberately NOT deleted until step 5 — it still exists in
`crates/sema-wasm/src/lib.rs` (the three replay loops, `HTTP_AWAIT_MARKER`,
`MAX_REPLAYS`, `installAtomicsSleep`) and in `playground/src/sema-worker.js`'s
dormant `legacySab` fallback branch. A literal whole-repo grep for those
markers would therefore fail today by design, not by defect. Step 4 scopes the
source-scan clause to what it can honestly assert without step 5's deletion:
the NEW promise-driven path's own code (`crates/sema-wasm/src/driver.rs`)
contains zero occurrences of any of the four markers, and in the shipped
`dist/sema-worker.js`/`dist/app.js`, `HTTP_AWAIT_MARKER`/`MAX_REPLAYS` never
appear at all, no literal `Atomics.wait(` call exists, and the one legacy call
that does still exist (`installAtomicsSleep(`) is gated behind
`if (msg.legacySab)` — a flag nothing in the shipped bundle ever sets. The
full-repo "these strings are gone entirely" scan is step 5's job, once the
replay/Atomics machinery is actually deleted. See the file-level comment in
`playground/tests/unified-runtime.spec.ts` for the same scoping, kept in sync.

The harness for this gate is `playground/tests/unified-runtime.spec.ts` — as of
step 4 (`.superpowers/sdd/p63-step4-report.md`) all six tests are un-`fixme`d
and green against a real `wasm-pack`-built bundle in headless Chromium; a
passing transcript is committed at
`docs/plans/evidence/unified-cooperative-runtime/p63-browser-gate-transcript.txt`.

## 6. Why this session hard-fell-back (what blocked verification)

Per the ironclad landing rule, a working shipped mechanism may not be replaced
without real-browser verification. Verification could not be established this
session because the *implementation* the gate would verify does not exist and
cannot be safely completed-and-verified in one build-bearing session:

1. **The dependency layer is absent on this branch.** Task 07's common host API
   that the WASM driver builds on — `Interpreter::submit_str` / `submit_value` /
   `drive` / `cancel_root` / `command_handle`, `RuntimeCommandHandle`,
   `RootOptions`, root-tagged `OutputEvent` — is **not implemented**
   (`crates/sema-eval/src/host.rs`, `crates/sema/src/host_driver.rs`,
   `crates/sema-wasm/src/driver.rs`, `crates/sema-wasm/src/output.rs` do not
   exist; only `Runtime::submit_root/drive/poll_result/cancel_root` and
   `Interpreter::drive_vm_on_runtime` are present). The WASM Promise driver
   would have to build directly on the low-level `Runtime` primitives and invent
   the multi-root submit/drive/cancel seam that Task 2 was supposed to provide.
2. **Scope vs. single-oracle risk.** A correct landing is a from-scratch
   `driver.rs` + `output.rs` + Promise table, a rewrite of the three async entry
   points, deletion of the replay/Atomics machinery, AND a rewrite of the
   playground worker protocol (`sema-worker.js`/`worker-client.js`/`app.js`) off
   the SAB. The only valid oracle is real-browser behavior; a half-working
   rewrite that regresses the live playground is strictly worse than the shipped
   replay path. Producing a *verified-correct* rewrite of all of that in one
   session is not achievable to the required standard.

Toolchain availability was NOT the blocker: `wasm-pack`, `wasm-bindgen`,
`playwright`, the `wasm32-unknown-unknown` target, and a Playwright webServer
harness (`playground/playwright.config.ts`, `jake test.playground-e2e`) are all
present and are the intended vehicle for the gate. The blocker is implementation
completeness against a real-browser-only oracle, and the risk of shipping an
unverified replacement of working behavior.

### Recommended landing order (next attempt)

1. Land Task 2 (native common host API) first so `Interpreter::submit_str/drive/
   cancel_root/command_handle` exist and are covered by
   `host_runtime_contract_test.rs`.
2. Build `driver.rs`/`output.rs` + Promise table on that API behind a new
   `eval()` that returns a `Promise`, keeping the old paths until green.
3. Rewrite the playground worker protocol root-aware, off the SAB.
4. Un-`fixme` `playground/tests/unified-runtime.spec.ts`, run
   `jake pg.build && jake test.playground-e2e` headless, capture the transcript.
5. Only then delete the replay/Atomics machinery and re-reconcile H10B/H10C.
</content>
</invoke>
