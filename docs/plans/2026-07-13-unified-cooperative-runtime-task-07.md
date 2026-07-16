# Task 07: Unified Native, Service, Notebook, Debugger, and WASM Hosts Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route every Sema host through the same root submission, bounded drive,
cancellation, output, and shutdown contract, including true Promise-based WASM
evaluation with multiple simultaneous roots.

**Architecture:** `sema-eval::Interpreter` exposes root-oriented primitives and
sync wrappers. Native blocking wrappers drive all roots fairly and park only
outside the runtime when idle. Services keep root handles and react to host
wakeups. WASM schedules bounded drive turns through browser macrotasks; fetch
and timers submit completions into the same runtime. No host replays evaluation
or owns a private scheduler.

**Tech Stack:** Rust, CLI/REPL, DAP, LSP, notebook, MCP server, workflow host,
`wasm-bindgen`, browser `Promise`, Playwright.

## Execution contract

- **Status (2026-07-16): PARTIALLY LANDED — kept in place as the live P6 remainder.** The DAP
  host runs on the unified runtime (P3). The common host API (P6-1: `submit_str`/`drive`/
  `cancel_root`/`command_handle`, root-tagged `OutputEvent`) and the wasm/services hosts (P6-3,
  SRV-1) remain; see `docs/plans/2026-07-16-post-migration-doc-reconciliation-and-p6-roadmap.md`
  Slices A–C for sequencing. Original: Ready only after Task 06 is accepted and committed.
- **Dependencies:** Final runtime/language/resource/context/orchestration APIs and
  clean Task 01–06 review evidence.
- **Immutable inputs:** Master common host API, native blocking wrappers,
  debugger stop-the-world, WASM Promise/macrotask/no-replay contract, multiple
  roots, and root-tagged output.
- **Exact start state:** Clean worktree; latest commit subject is
  `refactor(runtime): make task context explicit`; Task 01–06 gates are GREEN
  and the Task 01 inventory assigns every known host entry/private loop to Task
  07. Task 1 creates and reviews the exhaustive host matrix before host edits.
- **Parallel work:** Common eval/native driver lands first. DAP/LSP and notebook/
  MCP/workflow hosts may then migrate in parallel. WASM driver work may use the
  stable common API in parallel with native service hosts; playground protocol
  starts after WASM Promise/root IDs land. One owner integrates shared host API,
  conformance guards, generated-source exclusions, and final host suite.

## Global constraints

- Tasks 01–06 must be accepted and their full gates GREEN.
- Multiple roots on one interpreter are a required public capability, not only
  an internal test helper.
- Roots share globals/runtime and have distinct result, cancellation, output,
  sandbox/initial context, tracing parent, and host metadata.
- Sync wrappers return when the requested root settles, not when detached tasks
  or unrelated roots settle; while waiting they drive all roots fairly.
- Only send-safe commands cross from signal/network/browser callbacks. They may
  enqueue cancel, wake, or external completion, never `Value` or VM state.
- WASM `eval()` returns a JavaScript `Promise`; `evalAsync()` may remain an alias.
- Browser drive uses a macrotask/event-loop mechanism. Promise-only microtask
  spinning, evaluation replay, synchronous XHR, `Atomics.wait`, and sync fallback
  are forbidden.
- Playground Stop cancels the exact `RootId`. Output is root-tagged.
- Shipped asset changes are generated and package-verified in Task 08; do not
  hand-edit generated WASM/JS bundles here.
- No profiling or benchmarking in this layer.

---

## Files and responsibilities

**Create**

- `crates/sema-eval/src/host.rs` — common submit/drive/cancel/shutdown API.
- `crates/sema/src/host_driver.rs` — native park/wakeup/signal integration.
- `crates/sema-wasm/src/driver.rs` — browser macrotask driver and Promise table.
- `crates/sema-wasm/src/output.rs` — root-tagged browser output sinks.
- `crates/sema/tests/host_runtime_contract_test.rs` — native host contract.
- `playground/tests/unified-runtime.spec.ts` — browser multi-root/fetch/stop and
  heartbeat contract run by the repository Playwright harness.
- `docs/plans/evidence/unified-cooperative-runtime/task-07-host-matrix.md` — one
  row per host entry point and old loop.
- `docs/plans/evidence/unified-cooperative-runtime/task-07.md` — verification.
- `docs/plans/reviews/unified-cooperative-runtime/task-07.md` — independent
  host review.

**Modify**

- `crates/sema-eval/src/{lib,eval,debug_session}.rs` — root API and sync wrappers.
- `crates/sema/src/{lib,main}.rs` and `crates/sema/src/repl/{mod,headless,commands}.rs` — embedding, file/eval CLI, REPL, interrupt handling.
- `crates/sema-dap/src/{lib,server}.rs` — debug root/drive integration.
- `crates/sema-lsp/src/{server,state}.rs` and
  `crates/sema-lsp/src/handlers/command.rs` — evaluation roots and cancellation.
- `crates/sema-notebook/src/{engine,server,bridge}.rs` — cell roots, concurrent
  evaluation, output routing, reset/shutdown.
- `crates/sema-mcp/src/{server,tools,notebook}.rs` — tool-call roots and request
  cancellation.
- `crates/sema/src/{workflow_mcp,workflow_view}.rs` — workflow/service root
  lifetime and shutdown.
- `crates/sema-wasm/src/lib.rs` — Promise API, shared VFS, fetch, debugger.
- `playground/src/{app,sema-worker,worker-client}.js` — root IDs, streamed output,
  exact stop, concurrent requests.
- Playground and notebook host tests; runtime inventory/conformance/baseline.

## Exact common host API

```rust
pub struct RootOptions {
    pub output: Rc<dyn OutputSink>,
    pub sandbox: Sandbox,
    pub initial_context: TaskContext,
    pub tracing_parent: Option<TraceParent>,
    pub metadata: BTreeMap<String, String>,
    pub completion_waker: Rc<dyn RootCompletionWaker>,
}

impl Interpreter {
    pub fn submit_value(
        &self,
        value: &Value,
        options: RootOptions,
    ) -> Result<RootHandle, SemaError>;
    pub fn submit_str(
        &self,
        source: &str,
        options: RootOptions,
    ) -> Result<RootHandle, SemaError>;
    pub fn drive(&self, budget: DriveBudget) -> DriveState;
    pub fn cancel_root(&self, root: RootId, reason: CancelReason) -> bool;
    pub fn command_handle(&self) -> RuntimeCommandHandle;
    pub fn shutdown(&self, options: ShutdownOptions) -> ShutdownReport;
}
```

`RuntimeCommandHandle` is `Clone + Send + Sync` and exposes only:

```rust
pub fn cancel_root(&self, root: RootId, reason: SendCancelReason) -> bool;
pub fn wake(&self) -> bool;
pub fn complete(&self, completion: ExternalCompletion) -> bool;
```

`eval`, `eval_str`, and `eval_str_compiled` remain native convenience wrappers.
They use `submit_*`, then alternate bounded `drive` calls with parking on the
runtime inbox or next timer deadline. They never create another runtime.

Every output event is:

```rust
pub struct OutputEvent {
    pub root_id: RootId,
    pub task_id: TaskId,
    pub sequence: NonZeroU64,
    pub stream: OutputStream,
    pub text: String,
}
```

## Task 1: Inventory and lock common host behavior

**Files:** host matrix, `host_runtime_contract_test.rs`

- [ ] **Step 1: Discover every host entry and drive loop**

```bash
rg -n 'Interpreter::|eval(_str|_async|Async|VM|Global)?\(|run_until|run_cooperative|shutdown_scheduler|thread::sleep|block_on|Atomics|HTTP_AWAIT_MARKER|MAX_REPLAYS' \
  crates/sema-eval crates/sema crates/sema-dap crates/sema-lsp \
  crates/sema-notebook crates/sema-mcp crates/sema-wasm playground/src
```

- [ ] **Step 2: Add one host-matrix row per entry/loop**

Columns: `host`, `entry path`, `root options`, `drive owner`, `park/wakeup`,
`cancel source`, `output sink`, `shutdown owner`, `tests`, `old loop deletion`.

- [ ] **Step 3: Write failing common contract tests**

Test two pending roots, shared globals, independent outputs/context/errors,
round-robin progress, cancellation from `RuntimeCommandHandle`, late command
after drop, sync wrapper servicing another root, detached survival, and shutdown.

## Task 2: Implement common API and native driver

**Files:** eval host/eval/lib files, `host_driver.rs`, sema public lib/main

- [ ] **Step 1: Implement root submission and sync wrapper from tests**

Parsing/expansion/compilation failure settles only the submitted root or returns
before submission; it does not poison the interpreter. Compilation uses the
shared global environment and VM function ownership established in Task 03.

- [ ] **Step 2: Implement park/wakeup and Ctrl-C**

The native driver parks only after `DriveState::Idle`, until inbox wake or the
next timer. Ctrl-C/signal handlers use `RuntimeCommandHandle` to cancel the
foreground root. A second interrupt may request host shutdown but never mutates
VM state directly.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test host_runtime_contract_test
cargo test -p sema-lang --test integration_test -- cli_
cargo test -p sema-lang --test integration_test -- repl_
cargo test -p sema-lang --lib
```

Expected: native host contract and embedding compatibility pass.

## Task 3: Migrate DAP and LSP

**Files:** DAP/LSP files and tests

- [ ] **Step 1: Add failing service tests**

DAP: breakpoint in root A stops all roots, inspect both roots, resume, pause,
cancel A without settling B, disconnect shutdown. LSP: two eval commands,
request cancellation, document change while one root waits, isolated output,
server shutdown.

- [ ] **Step 2: Replace private execution loops**

Each service owns one interpreter and one host driver. DAP debug state is the
runtime’s stop-the-world state. LSP request IDs map to root IDs.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-dap
cargo test -p sema-lsp
jake test.lsp
```

Expected: unit and e2e tests pass with no private scheduler loop.

## Task 4: Migrate notebook, MCP server, and workflow service hosts

**Files:** notebook/MCP/workflow host files and tests

- [ ] **Step 1: Add root-lifetime tests**

Notebook: overlapping cells, root-tagged output, one-cell stop, shared global
definitions in scheduled order, reset with live roots, Run All policy. MCP:
overlapping tool evals, request cancellation/disconnect, sandbox per request,
shutdown. Workflow service: client disconnect, interactive wait cancellation,
server shutdown, durable journal completion.

- [ ] **Step 2: Implement handles and explicit shutdown**

Notebook cell/request IDs map to stable root IDs. Reset first cancels roots and
waits for runtime shutdown/cleanup before replacing the interpreter. Server
disconnect cancels only roots owned by that request unless service shutdown is
requested.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-notebook
cargo test -p sema-mcp
cargo test -p sema-lang --test otel_notebook_test
cargo test -p sema-lang --test workflow_mcp_cli_e2e_test
cargo test -p sema-lang --test workflow_mcp_interactive_test
cargo test -p sema-lang --test workflow_view_connect_test
jake example-notebook
jake example-notebooks-async
```

Expected: all pass and teardown snapshots have zero live roots/tasks/resources.

## Task 5: Replace WASM replay/blocking with Promise-driven roots

**Files:** WASM lib/driver/output, browser runtime tests

- [ ] **Step 1: Write failing browser contract tests**

Test that `eval()` returns a `Promise`; two calls stay pending and complete
fairly; definitions are shared; outputs carry distinct root IDs; fetch suspends
without replaying pre-fetch side effects; timers allow rendering/user input;
Stop cancels one exact root; another root continues; debugger stop/resume works;
shutdown rejects pending Promises with structured cancellation.

- [ ] **Step 2: Implement Promise table and macrotask driver**

Each eval submits once and stores `RootId -> {resolve, reject}`. Drive one bounded
turn, settle ready Promises, then schedule another turn with `setTimeout(0)`,
`MessageChannel`, or equivalent macrotask when work remains. Fetch/JS timers send
tagged external completions and schedule a drive turn.

- [ ] **Step 3: Delete replay and blocking paths**

Remove `HTTP_AWAIT_MARKER`, HTTP cache replay, `MAX_REPLAYS`, synchronous eval
fallbacks for async work, SharedArrayBuffer sleep state, `installAtomicsSleep`,
and `Atomics.wait/notify` dependencies. `evalAsync()` delegates to `eval()`.
Legacy `evalVM`/`evalGlobal` either return the same Promise or are removed with a
clear migration note in Task 08 docs.

- [ ] **Step 4: Run WASM tests/build**

```bash
cargo test -p sema-wasm
wasm-pack test crates/sema-wasm --headless --chrome
jake wasm.build
```

Expected: Rust/browser tests pass; source scan has no replay or Atomics wait.

## Task 6: Migrate playground and worker protocol

**Files:** playground JS and Playwright tests

- [ ] **Step 1: Make protocol root-aware**

Eval response, output, completion, cancel, error, and debugger messages carry
`rootId`. The worker can hold multiple pending roots and drive them fairly.
Main-thread and worker modes use the same Promise API.

- [ ] **Step 2: Add Playwright scenarios**

Run two evaluations concurrently with interleaved output; stop one during
sleep/fetch/CPU loop; verify the other completes; verify UI remains paintable;
verify side effect before fetch occurs once; verify repeated run/stop leaves no
pending worker request.

- [ ] **Step 3: Run**

```bash
jake pg.build
jake test.playground-e2e
```

Expected: build and browser suite pass. Do not deploy.

## Task 7: Static guards, packaged-source boundary, review, and commit

- [ ] **Step 1: Strengthen conformance guards**

Fail production source on removed scheduler loops, WASM replay markers/cache,
Atomics wait/sleep installation, sync XHR, Promise microtask drive recursion,
and output buffers not keyed/tagged by root.

- [ ] **Step 2: Run layer gates**

The repository must have a Windows-native CI job (for example, a
`windows-latest` matrix leg in `.github/workflows/verify.yml`) that checks out
the commit and runs this exact command in a native Windows process:

```powershell
cargo test -p sema-lang --test unified_runtime_watchdog_test -- --nocapture
```

That job must execute and pass the `cfg(windows)` inherited-writer,
immediate-marker, and multi-chunk head/tail marker regressions before Task 07
acceptance. Cross-compiling or `cargo check --target ...windows...` does not
exercise `CancelSynchronousIo` and is not acceptance evidence. Task 07 may not
be accepted without the successful native Windows workflow URL/run ID and test
output in Task 07 evidence.

```bash
cargo test -p sema-eval -p sema-lang -p sema-dap -p sema-lsp \
  -p sema-notebook -p sema-mcp -p sema-wasm
cargo test -p sema-lang --test host_runtime_contract_test
cargo test -p sema-lang --test runtime_conformance_test
jake pg.build
jake test.playground-e2e
cargo fmt --all -- --check
cargo clippy -p sema-eval -p sema-lang -p sema-dap -p sema-lsp \
  -p sema-notebook -p sema-mcp -p sema-wasm --all-targets -- -D warnings
scripts/check-unified-runtime-legacy.sh > /tmp/runtime-legacy.actual
diff -u docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline /tmp/runtime-legacy.actual
git diff --check
```

Expected: all GREEN; generated asset/package verification remains assigned to
Task 08 and is not falsely claimed here.

- [ ] **Step 3: Assign independent host review**

Finding IDs use `UR-T07-R###`. Reviewer traces one root through every host,
verifies cancellation identity and output ownership, tests shutdown during each
wait kind, and inspects browser task scheduling to prove a macrotask boundary.

- [ ] **Step 4: Fix findings test-first and repeat all host/browser gates**

Host-specific fixes must preserve the common contract test; a private exception
requires changing the master specification and all hosts, not a local bypass.

- [ ] **Step 5: Commit the accepted layer**

```bash
git add crates/sema-eval crates/sema crates/sema-dap crates/sema-lsp \
  crates/sema-notebook crates/sema-mcp crates/sema-wasm playground \
  docs/internals/async-runtime-inventory.md \
  docs/plans/evidence/unified-cooperative-runtime \
  docs/plans/reviews/unified-cooperative-runtime
git commit -m "refactor(runtime): unify native and browser hosts"
```

## Completion criteria

- Every host entry appears in the matrix and uses common root/drive APIs.
- Multiple live roots share globals and retain independent result/cancel/output
  and context across native, service, notebook, and browser hosts.
- Native wrappers park only when idle and service unrelated roots fairly.
- DAP stop is interpreter-wide; service request cancellation is root-specific.
- WASM `eval()` is Promise-based, single-execution, macrotask-driven, and
  interruptible without replay, Atomics wait, or sync fallback.
- Playground messages and Stop actions carry exact root identity.
- Shutdown settles/reaps all roots, tasks, and host resources.
- Independent review and durable evidence are clean.
- A Windows-native CI run executes the complete watchdog target successfully,
  including all three `cfg(windows)` drain regressions; cross-compilation alone
  cannot satisfy this criterion.
