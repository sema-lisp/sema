# Task 05: Interruptible I/O and Bounded Resource Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route every standard-library resource through the unified runtime with
an explicit interruptible or quarantined-bounded cancellation contract, and
eliminate blocking/polling branches from runtime tasks.

**Architecture:** `sema-io` remains the one process-wide native executor, but it
executes send-only `IoJob`s and reports tagged `ExternalCompletion`s. The
interpreter runtime owns waits, decoders, cancellation, and cleanup. Resource
builtins return `NativeOutcome` regardless of whether called at a root or nested
inside a task; synchronous host APIs obtain blocking behavior by driving roots,
not by calling `io_block_on` from a VM task.

**Tech Stack:** Rust, Tokio, `sema-io`, `sema-stdlib`, OS process/PTY handles,
fake servers, deterministic fault injection.

## Execution contract

- **Status:** Ready only after Task 04 is accepted and committed.
- **Dependencies:** Stable observation/ownership APIs, cleanup scopes, external
  completion identities, and zero language-contract RED tests.
- **Immutable inputs:** Master resource classes, worker boundary, shutdown,
  native blocking wrapper, and complete standard-library resource scope.
- **Exact start state:** Clean worktree; latest commit subject is
  `feat(runtime): add explicit async ownership semantics`; Task 01–04 gates are
  GREEN, and the Task 01 inventory assigns every known resource discovery match
  to Task 05 or a later named owner. Task 1 creates and reviews the exhaustive
  Task 05 resource matrix before production edits.
- **Parallel work:** After the executor seam and matrix review, process/PTY/
  watcher/stream, HTTP/WS/server, and file/database/bounded-library migrations
  may proceed in disjoint modules. One owner integrates core/IO/runtime seams,
  snapshots, inventory, and conformance guards. Cross-category review waits for
  the full resource suite.

## Global constraints

- Tasks 01–04 must be accepted and all language-contract tests GREEN.
- Every operation is classified before migration. “Best effort,” “usually
  finite,” and dropping a result receiver are not cancellation contracts.
- An interruptible operation has an idempotent concrete cancel hook. A
  quarantined operation has a pre-dispatch hard deadline or fixed maximum work
  count and is reaped by `CleanupRegistry`.
- There is no constructible unbounded non-interruptible operation.
- No Task 05-owned runtime path calls `io_block_on`, `thread::sleep`, synchronous
  channel recv, or a polling loop that occupies the interpreter thread. Exact
  LLM/MCP/task-context adapters owned by Task 06 may remain only when listed by
  file and symbol in the inventory and scanner allowlist; Task 06 must delete
  them before its layer is accepted.
- A late completion contains only send-safe payload and is rejected by full
  runtime/wait/generation/operation identity.
- Existing top-level return values and errors remain compatible unless the
  master specification explicitly changes them.
- Network-dependent tests are supplemental; local fake servers and processes
  are the required CI oracle.
- No profiling or benchmarking in this layer.

---

## Files and responsibilities

**Create**

- `crates/sema-io/src/job.rs` — send-only job and completion sink interfaces.
- `crates/sema-io/src/pool.rs` — one-pool execution and shutdown accounting.
- `crates/sema-io/src/fault.rs` — test-only completion/cancel fault injection.
- `crates/sema/tests/resource_contract_test.rs` — cancellation-class matrix.
- `crates/sema/tests/resource_shutdown_test.rs` — process-level leak/watchdog
  tests.
- `docs/plans/evidence/unified-cooperative-runtime/task-05-resource-matrix.md` —
  one row per builtin/resource operation.
- `docs/plans/evidence/unified-cooperative-runtime/task-05.md` — test evidence.
- `docs/plans/reviews/unified-cooperative-runtime/task-05.md` — independent
  resource review.

**Modify**

- `crates/sema-core/src/io_backend.rs` — replace block-on-oriented seam with job
  submission and lifecycle reporting.
- `crates/sema-io/src/lib.rs` and `crates/sema-io/tests/tokio_pin_test.rs` —
  export jobs, preserve one-pool identity, verify abort/quarantine accounting.
- `crates/sema-vm/src/runtime/{wait.rs,drive.rs,cleanup.rs}` — register before
  dispatch, consume completions, and reap resources during shutdown.
- `crates/sema-stdlib/src/{archive,diff,event,fs_watch,git,http,io,kv,pdf,proc,pty,secret,serial,server,sqlite,stream,system,terminal,ws}.rs` — migrate all
  resource calls.
- `crates/sema-stdlib/src/{crypto,csv_ops,markup}.rs` — classify and migrate
  bounded CPU/library work reached from async tasks.
- Resource integration tests listed in Task 6 below.
- `docs/internals/async-runtime-inventory.md`, runtime conformance test, and
  legacy baseline — prohibit old offload/blocking seams after migration.

## Exact executor interface

```rust
pub struct CompletionSink {
    sender: Sender<ExternalCompletion>,
    runtime_id: RuntimeId,
    wait_id: WaitId,
    generation: WaitGeneration,
    operation_id: OperationId,
    kind: CompletionKind,
}

impl CompletionSink {
    pub fn complete(self, result: Result<SendPayload, ExternalFailure>) {
        let Self {
            sender,
            runtime_id,
            wait_id,
            generation,
            operation_id,
            kind,
        } = self;
        let _ = sender.send(ExternalCompletion {
            runtime_id,
            wait_id,
            generation,
            operation_id,
            kind,
            result,
        });
    }
}

pub trait IoJob: Send + 'static {
    fn run(self: Box<Self>, sink: CompletionSink);
}

/// Constructed and consumed only on the interpreter thread.
pub struct PreparedExternalOperation {
    pub operation_id: OperationId,
    pub completion_kind: CompletionKind,
    pub decoder: Box<dyn CompletionDecoder>,
    pub resource: ResourceClass,
    pub job: Box<dyn IoJob>,
}

/// Immediate pool-admission receipt; never owns runtime cleanup state.
pub struct RunningJob {
    operation_id: OperationId,
    executor_job_id: u64,
}

pub enum SubmitError {
    LeaseShuttingDown,
    QueueClosed,
    AdmissionRejected,
}

pub trait IoExecutor: Send + Sync {
    fn attach_runtime(&self, runtime_id: RuntimeId) -> ExecutorLease;
    fn snapshot(&self) -> ExecutorSnapshot;
}

pub struct ExecutorLease { /* private runtime id and shared-pool handle */ }

impl ExecutorLease {
    pub fn submit(
        &self,
        job: Box<dyn IoJob>,
        sink: CompletionSink,
    ) -> Result<RunningJob, SubmitError>;
    pub fn snapshot(&self) -> ExecutorSnapshot;
    pub fn shutdown(&self, deadline: Instant) -> ExecutorShutdown;
}
```

`PreparedExternalOperation` is a runtime-side bundle, not a worker message. The
runtime destructures it, installs the decoder and `ResourceClass` (including the
cancel hook) in `WaitRegistry`/`CleanupRegistry`, creates a `CompletionSink` with
the runtime-selected `CompletionKind`, and only then sends the `Box<dyn IoJob>`
to the executor. `IoJob::run` reports through its sink and returns `()`; worker
code cannot select the completion kind, return a resource/cancel hook, or move
VM-side cleanup state across the thread boundary. `RunningJob` is only an
immediate admission receipt.

Each interpreter runtime owns one `ExecutorLease` over the process-wide pool.
Lease shutdown rejects new jobs for that runtime, cancels/drains only its jobs,
and unregisters the lease without stopping jobs belonging to another
interpreter. The shared pool may stop workers after the final lease closes or an
explicit process shutdown. `Runtime::start_external` allocates and installs the
wait/resource registration before it calls the lease's `submit`, preventing a
completion-before-registration race. If `submit` returns `Err`, the runtime
deterministically unregisters that wait, invokes or transfers cleanup exactly
once, and records the rejected operation; no task remains parked. The executor
never decodes a Sema value.
Panic is caught at the job boundary and delivered as
`ExternalFailure::WorkerPanic`; it does not silently drop the wait.

`ExecutorSnapshot` reports queued, running-interruptible,
running-quarantined, completed, cancelled, panicked, and late-delivery counts.
Task/review evidence records snapshots before cancellation and after reaping.

## Required resource classification

The committed matrix has exact builtin names, source path, class, cancel/bound
mechanism, decoder, shutdown action, and tests. The following categories are
mandatory; the implementer expands them to one row per registered builtin:

| Source modules | Required class and mechanism |
| --- | --- |
| `http`, `ws`, `server` | Interruptible: abort request/accept/connection future and close owned socket/response body. |
| `proc`, `pty`, `git` | Interruptible: kill process/process-group or close PTY master, then asynchronously wait/reap child. |
| `fs_watch`, `event`, `serial`, `terminal` | Interruptible: deregister watcher/subscription/file descriptor and close wake source. |
| `stream` | Interruptible for open/read/write waits: cancel registered OS future; close operation remains idempotent. |
| `sqlite`, `kv` | Interruptible where library interrupt handle exists; otherwise hard busy/deadline limit plus quarantine. |
| `io` file operations | Interruptible async filesystem request where supported; finite-work quarantine only after size/entry count is captured and capped before dispatch. |
| `archive`, `pdf`, `diff`, `secret`, `crypto`, `csv_ops`, `markup` | Finite-work quarantine with validated byte/page/entry/item maximum fixed before dispatch. |
| `system` sleeps/process queries | Runtime timer or interruptible OS job; no worker-thread sleep. |

If an existing builtin cannot satisfy its row, make it return a structured
unsupported-in-cooperative-runtime condition when invoked from any root, record
the exact builtin and rationale as a blocking Task 05 finding, and do not claim
the layer complete. Adding a hidden synchronous fallback is forbidden.

## Task 1: Inventory every operation before editing it

**Files:** inventory and `task-05-resource-matrix.md`

- [ ] **Step 1: Generate discovery input**

```bash
rg -n 'io_block_on|io_spawn|io_spawn_blocking|io_offload_blocking|IoPoll|IoHandle|in_async_context|spawn_blocking|thread::sleep|Command::new|Tcp(Stream|Listener)|UdpSocket|WebSocket|rusqlite|notify::' \
  crates/sema-core crates/sema-io crates/sema-stdlib crates/sema-mcp crates/sema-llm
```

- [ ] **Step 2: Create one matrix row per matched operation**

No `misc`, module-wide wildcard, or “same as above” rows. Each match is linked to
one row or explicitly marked non-runtime test/infrastructure with evidence.

- [ ] **Step 3: Review classification before production edits**

Reviewer rejects finite-work claims that do not identify the input cap and the
code that enforces it before dispatch.

## Task 2: Replace the executor seam test-first

**Files:** core I/O backend, `sema-io` job/pool/fault modules, Tokio tests

- [ ] **Step 1: Write failing seam tests**

First use fake jobs/executors to pin the interface itself:

- `completion_sink_carries_runtime_selected_kind` — a worker can submit a result
  but cannot choose or overwrite the private kind;
- `io_job_returns_unit_and_only_send_work_crosses` — the fake job is `Send`, its
  `run` result is `()`, and a deliberately non-`Send` runtime cancel hook remains
  in the prepared operation/registry;
- `completion_before_submit_returns_is_safe` — registration exists before the
  executor runs the job inline and returns its receipt;
- `submit_rejection_rolls_back_wait_and_resource` — every `SubmitError` leaves
  zero waits/resources and runs cleanup once;
- `running_job_receipt_does_not_own_resource` — executor accounting cannot take
  over the runtime cancel hook.

Then cover cancellation before job starts, cancellation during job, cancel
twice, job panic, wrong runtime/generation/operation/kind, duplicate completion,
quarantine completion after observer cancellation, and shutdown with one job in
each state.

- [ ] **Step 2: Implement prepared operations, `IoJob`, admission, and counters**

Preserve the process-wide pool identity and blocking-tier admission headroom.
Keep `CompletionSink` fields private and expose one result-delivery method so a
job cannot forge identity or kind. Store the runtime `ResourceClass` before
submission and implement rejection rollback before migrating any builtin.
Remove public `io_block_on` once all callers in this layer are migrated; until
then its inventory row must list the exact remaining caller and deletion step.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-io
cargo test -p sema-vm runtime::tests::external
cargo test -p sema-vm runtime::tests::cleanup
```

Expected: race/fault tests pass and executor snapshot returns to zero live jobs.

## Task 3: Migrate processes, PTYs, watchers, and streams

**Files:** `proc.rs`, `pty.rs`, `git.rs`, `fs_watch.rs`, `event.rs`, `serial.rs`,
`terminal.rs`, `stream.rs`, resource tests

- [ ] **Step 1: Add failing cancellation tests per resource**

Use local child processes and pipes. Assert the OS child is gone/reaped, the PTY
fd is closed, watchers stop emitting, blocked reads/writes wake with
cancellation, close is idempotent, and no sibling/root is cancelled.

- [ ] **Step 2: Implement concrete interrupt hooks**

On Unix, process cancellation signals the owned process group with `SIGTERM`,
waits the configured grace timer, then uses `SIGKILL` and `waitpid`. On Windows,
the child is assigned to an owned Job Object and cancellation uses
`TerminateJobObject` before waiting on the process handle. Register handles
before exposing the promise.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test proc_pty_async_test
cargo test -p sema-lang --test git_async_test
cargo test -p sema-lang --test stream_file_async_test
cargo test -p sema-lang --test true_cancel_test
```

Expected: all pass and child/fd counters return to baseline.

## Task 4: Migrate HTTP, WebSocket, and server lifetimes

**Files:** `http.rs`, `ws.rs`, `server.rs`, resource tests

- [ ] **Step 1: Add local fake-server failure matrix**

Cover connect stall, partial headers, partial body, streaming body, peer close,
client cancellation, server-handler cancellation, listener shutdown,
disconnect during callback, late response, and two concurrent roots.

- [ ] **Step 2: Implement abort plus close**

Dropping a receiver is insufficient. The hook aborts the future and closes any
owned transport. Server shutdown cancels/reaps handler tasks through an owned
scope.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test http_concurrent_test
cargo test -p sema-lang --test server_async_test
cargo test -p sema-lang --test integration_test -- ws_
```

Expected: fake-server matrix passes without network access or lingering ports.

## Task 5: Migrate files, databases, and bounded library work

**Files:** `io.rs`, `sqlite.rs`, `kv.rs`, `archive.rs`, `pdf.rs`, `diff.rs`,
`secret.rs`, `crypto.rs`, `csv_ops.rs`, `markup.rs`, resource tests

- [ ] **Step 1: Write cap and cleanup tests**

For every finite-work row test just below cap, at cap, above cap, cancellation
after dispatch, worker panic, and cleanup registry reaping. For databases test
busy lock, query interruption, transaction rollback, and dropped connection.

- [ ] **Step 2: Enforce bounds before submission**

Capture file metadata/input byte count/page count/archive entry count or item
count, reject cap violations, and store the fixed bound in the job descriptor.
Code that discovers unbounded expansion while running must abort with a named
bound-exceeded failure.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test file_async_test
cargo test -p sema-lang --test archive_pdf_patch_async_test
cargo test -p sema-lang --test db_async_test
cargo test -p sema-lang --test kv_async_test
```

Expected: all pass; quarantine snapshot is empty after the declared bound.

## Task 6: Run the complete resource integration set

- [ ] **Step 1: Run exact existing targets**

```bash
cargo test -p sema-lang --test io_pool_identity_test
cargo test -p sema-lang --test shell_concurrent_test
cargo test -p sema-lang --test file_async_test
cargo test -p sema-lang --test stream_file_async_test
cargo test -p sema-lang --test git_async_test
cargo test -p sema-lang --test proc_pty_async_test
cargo test -p sema-lang --test http_concurrent_test
cargo test -p sema-lang --test server_async_test
cargo test -p sema-lang --test db_async_test
cargo test -p sema-lang --test kv_async_test
cargo test -p sema-lang --test archive_pdf_patch_async_test
cargo test -p sema-lang --test true_cancel_test
cargo test -p sema-lang --test resource_contract_test
cargo test -p sema-lang --test resource_shutdown_test
```

Expected: all targets pass. Evidence records test counts and final executor,
runtime wait, cleanup, process, descriptor, and port counts.

## Task 7: Static removal gates, review, and commit

- [ ] **Step 1: Strengthen conformance guards**

Outside `sema-io` implementation and test fixtures, fail on `io_block_on`, raw
Tokio runtime construction, `in_async_context` behavior branches, legacy
`IoPoll`/`IoHandle`, and process/resource dispatch without a matrix entry. The
only temporary exceptions are exact file-and-symbol Task 06 adapters recorded
in the inventory with deletion owners; broad crate/directory exclusions are
forbidden.

- [ ] **Step 2: Run layer gates**

```bash
cargo test -p sema-core -p sema-io -p sema-vm -p sema-stdlib
cargo test -p sema-lang --test runtime_conformance_test
cargo test -p sema-lang --test resource_contract_test
cargo test -p sema-lang --test resource_shutdown_test
cargo fmt --all -- --check
cargo clippy -p sema-core -p sema-io -p sema-vm -p sema-stdlib \
  --all-targets -- -D warnings
scripts/check-unified-runtime-legacy.sh > /tmp/runtime-legacy.actual
diff -u docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline /tmp/runtime-legacy.actual
git diff --check
```

Expected: all GREEN and static scans have no unexplained production match.

- [ ] **Step 3: Assign independent resource review**

Finding IDs use `UR-T05-R###`. Reviewer selects at least one operation from each
matrix category and verifies dispatch, cancellation, late delivery, decode,
close, and shutdown in code and tests. Reviewer also attempts to construct an
unbounded job and to block the interpreter thread.

- [ ] **Step 4: Fix every finding test-first and repeat full resource set**

Resource cleanup findings cannot be downgraded because the normal test process
eventually exits.

- [ ] **Step 5: Commit the accepted layer**

```bash
git add crates/sema-core crates/sema-io crates/sema-vm crates/sema-stdlib \
  crates/sema/tests docs/internals/async-runtime-inventory.md \
  docs/plans/evidence/unified-cooperative-runtime \
  docs/plans/reviews/unified-cooperative-runtime
git commit -m "refactor(runtime): migrate interruptible resources"
```

## Completion criteria

- Every resource operation has an exact reviewed matrix row.
- Every runtime operation is interruptible or proven bounded before dispatch.
- No VM task blocks on the I/O executor or chooses behavior via ambient async
  context.
- Completion registration is race-safe and late/duplicate delivery is harmless.
- Process, PTY, socket, watcher, stream, database, and quarantine cleanup is
  observable and returns to baseline.
- Static guards reject reintroduction of old seams and ad hoc runtimes.
- Independent review and durable evidence are clean.
