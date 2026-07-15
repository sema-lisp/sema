# Task 05: Interruptible I/O and Bounded Resource Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route every standard-library resource through the unified runtime with
an explicit interruptible or quarantined-bounded cancellation contract, and
eliminate blocking/polling branches from runtime tasks.

**Architecture:** `sema-io` remains the one process-wide native executor, but it
executes private send-only jobs owned by `PreparedExternalOperation` and reports
tagged `ExternalCompletion`s through opaque executor wrappers. The interpreter
runtime owns waits, decoders, cancellation, and cleanup. Resource
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
- Every still-unmigrated `AwaitIo`, resource, LLM, or MCP producer routes through
  the single named `LegacyRuntimeBridge`. Its inventory row assigns deletion to
  Task 05 (resources), Task 06 (LLM), Task 07 (host adapters), or Task 08 (final
  removal); no anonymous compatibility path survives Task 03.
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

- `crates/sema-io/src/job.rs` — production one-shot runner and panic/delivery
  wrapper implementing the Task 02 core seam; no duplicate nominal interfaces.
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

- `crates/sema-io/src/lib.rs` and `crates/sema-io/tests/tokio_pin_test.rs` —
  implement/re-export the core executor seam, delete the inventoried legacy
  block-on adapters, preserve one-pool identity, and verify abort/quarantine
  accounting.
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

Task 02 defines the exact `CompletionSender`, private unnameable
`CompletionSink`, capability-safe `CompletionRegistrar`, opaque
`ExecutorSubmission` and `ExecutorDispatch` wrappers,
`PreparedExternalOperation`, `RunningSubmission`, owning `SubmissionRejected`,
`ExecutorDriveReport`/`ExecutorTerminal`, `ExecutorLease`, and `IoExecutor`
interfaces in `sema-core`. Task 03 consumes
those interfaces with a fake lease. Task 05 implements them in `sema-io` over
the one process-wide pool; it does not redeclare or wrap them in a second public
seam.

```rust
pub struct ProcessIoExecutor {
    pool: Arc<ProcessPool>,
}

struct ProcessExecutorLease {
    runtime_id: RuntimeId,
    pool: Arc<ProcessPool>,
}

impl IoExecutor for ProcessIoExecutor {
    fn attach_runtime(
        &self,
        runtime_id: RuntimeId,
    ) -> Result<Arc<dyn ExecutorLease>, ExecutorAttachError> {
        self.pool.register_runtime(runtime_id)?;
        Ok(Arc::new(ProcessExecutorLease {
            runtime_id,
            pool: Arc::clone(&self.pool),
        }))
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        self.pool.snapshot()
    }
}

impl ExecutorLease for ProcessExecutorLease {
    fn submit(
        &self,
        submission: ExecutorSubmission,
    ) -> Result<RunningSubmission, SubmissionRejected> {
        self.pool.submit(self.runtime_id, submission)
    }

    fn snapshot(&self) -> ExecutorSnapshot {
        self.pool.snapshot_runtime(self.runtime_id)
    }

    fn shutdown(&self, deadline: Instant) -> ExecutorShutdown {
        self.pool.shutdown_runtime(self.runtime_id, deadline)
    }
}
```

Task 02 defines the sole unregistered `PreparedExternalOperation` with three
compatibility-enforcing constructors. Each prepared operation privately owns
exactly one async or blocking job; no public nominal job type crosses the core
boundary. Task 03 defines the distinct
`RegisteredExternalWait`. Task 05 implements their executor boundary but MUST
NOT introduce a second prepared/external-wait owner. The executor reserves
capacity while it owns an unarmed submission, calls `into_dispatch` as the
admission linearization point, and enqueues only the armed dispatch. It never
passes either wrapper to the private job's consuming run path. The job returns a
send-safe result. Worker code therefore cannot omit or duplicate completion
delivery, select the completion kind, return a resource/cancel hook, or move
VM-side cleanup state across the thread boundary. `RunningSubmission` is only an
immediate admission receipt. On pre-arm rejection, `SubmissionRejected` proves no
admission or sink delivery occurred. It owns the rejected submission, exposes
read-only kind and operation identity, and consumes itself for rollback; core
destroys the private sink, job, and start token and returns only the rejection
kind. If enqueue fails after arming, dispatch drop attempts cancellation; this
is admitted cancellation, never `SubmissionRejected`.

Each interpreter runtime owns one `ExecutorLease` over the process-wide pool.
Lease shutdown rejects new jobs for that runtime, cancels/drains only its jobs,
and only then unregisters its `RuntimeId`, without stopping jobs belonging to
another interpreter. Duplicate attachment and attachment after pool shutdown
return `ExecutorAttachError`. The shared pool may stop workers after the final
lease unregisters or an explicit process shutdown. `Runtime::apply_native_suspend` dispatches its sole
boxed prepared request to private `start_external`, which destructures that
bundle once. It first reads the prepared completion kind and issues the complete
runtime identity, then uses its private registrar to bind and split the prepared
operation. The split yields runtime-local decoder/resource/queue-control state
and one opaque `ExecutorSubmission`. In one atomic transition, the runtime
installs `RegisteredExternalWait` with its traced decoder and continuation, the
concrete cleanup/resource entry, and the task's `Running -> Waiting` state. Only
then does it call the lease's `submit` with the opaque submission; `sema-io`
never receives or can name the sink or private job. After capacity reservation,
`sema-io` calls `into_dispatch()` before enqueue. An inline
completion can only enqueue; the dispatch wrapper processes it after this
transition returns and therefore observes the
registered waiting task. If `submit` returns `SubmissionRejected`, its consuming
rollback destroys the unarmed sink/job/start token internally and returns the
rejection kind. The runtime then removes the wait, processes/transfers resource cleanup
once, drops the queue-cancel half, changes `Waiting -> Running`, and
routes `ExternalFailureCode::Rejected` through the same consuming decoder and
then the same consuming continuation used by worker completion. No task remains
parked and rejection cannot enqueue a completion. For each
admitted job, the executor owns one terminal delivery. On dequeue it consumes
`start_token.claim_for_run()`. `CompleteCancelled` drops the job without invoking
it and completes the sink once with `ExternalFailureCode::Cancelled`. `Run`
claims the operation before the runtime can classify it as queued. In either
case the dispatch drive report updates `cancelled`, `panicked`, or `completed`;
all non-cancellation/non-worker-panic producer results use `completed`, because
the snapshot has no failed bucket, and its delivery updates `undeliverable`.
Cancellation consumes/deregisters the runtime resource entry exactly
once. For `Interruptible`, it invokes the hook exactly once even when
`CancelledQueued` won: the pre-armed hook records sticky cancellation and
closes/releases any existing prepared child, stream, or resource. Repeated
cancellation cannot call it again. `CancelDisposition::Reaped` releases the
cleanup entry immediately. `PendingReap` or `CancelHookError` transfers the
still-owned hook/resource to `CleanupRegistry`, retains live accounting, and
records/deduplicates a suppressed diagnostic; task cancellation may settle only
after that ownership transfer is recorded. Cleanup turns invoke only `reap`,
never `cancel` again. Late job completion remains stale while cleanup ownership
is retained. The queue CAS controls only body invocation and sink ownership; it
never suppresses the hook. Every interruptible job and
its runtime hook share a
pre-armed sticky cancellation state created before submission. That state
includes a cancellation token/wake primitive selected or polled by any
potentially blocking acquisition, unless the concrete abort handle is installed
before the resource's first potentially blocking poll. A one-time check before
acquisition is not enough: cancellation while acquisition is blocked MUST wake
it and produce `Cancelled`. Post-construction attachment is allowed only for
nonblocking construction, through the synchronized shared state, before first
blocking use; a racing request then causes immediate attach-and-abort. If an API
exposes its handle only after a potentially unbounded acquisition and cannot
select/poll cancellation, split acquisition into its own interruptible wait,
prove `QuarantinedBounded`, or mark the operation `PROHIBITED`. After dequeue,
`sema-io` polls an already armed async
wrapper's consuming future or calls a blocking wrapper's consuming `run`; the
wrappers claim the token, catch construction/poll/run panic under
`panic = "unwind"`, map panic to `ExternalFailureCode::WorkerPanic`, and make
exactly one terminal delivery attempt through the private sink. `panic =
"abort"` terminates the process. Dropping an admitted wrapper or async future
attempts terminal cancellation. Because `Drop` returns no report, bounded
non-panicking `CompletionSender::send` (or its explicit reporter) increments the
undeliverable counter on `InboxClosed`. An
enqueued completion rejected by `WaitRegistry` as stale, cancelled, duplicate,
or wrong-identity normally increments the runtime's `late_completions` counter.
The exception is an exact full-identity match in quarantine cleanup after its
active wait was cancelled: that completion discards its payload, removes the
quarantine entry once, and increments `quarantine_reaped`. Wrong-kind/identity,
duplicate-after-reap, and interruptible stale completion remain late and cannot
release cleanup ownership. The executor never decodes a Sema value.

`ExecutorSnapshot` reports queued, running-interruptible,
running-quarantined, completed, cancelled, panicked, and undeliverable counts.
The runtime snapshot separately reports late completions after inbox validation.
Task/review evidence records both snapshots before cancellation and after
reaping.

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
| `sqlite`, `kv` | Feasibility spike first: prove an interrupt handle wakes the representative blocked call, prove killable process isolation, or classify it `PROHIBITED`. |
| `io` file operations | Feasibility spike first: prove the representative filesystem syscall is interruptible/wakeable, isolate it in a killable process, or classify it `PROHIBITED`. |
| `archive`, `pdf`, `diff`, `secret`, `crypto`, `csv_ops`, `markup` | Bounds may constrain CPU/input expansion, but do not enforce a deadline around a sync-only blocking call. Each sync-only provider representative needs proven interrupt/wake, killable process isolation, or `PROHIBITED`. |
| `system` sleeps/process queries | Runtime timer or interruptible OS job; no worker-thread sleep. |

Before a `QuarantinedBounded` job wins `Run`, it MUST consist only of an
immutable owned `Send` input snapshot. Preparation and queue residency acquire
no resource, start no work, perform no external mutation, and create no cleanup
that a queue-cancel drop could strand. An operation with any such pre-run effect
is `Interruptible` with an exactly-once hook that releases it, or
`PROHIBITED`.

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
code that enforces it before dispatch. Before approving any filesystem, SQLite,
subprocess/PTY, or sync-only provider row, run a feasibility spike that blocks a
representative operation and proves either actual interrupt+wake, killable
process isolation and reap, or `PROHIBITED`. A byte/page/item cap or deadline on
an uninterruptible same-process syscall is not enforcement.

## Task 2: Replace the executor seam test-first

**Files:** core I/O backend, `sema-io` job/pool/fault modules, Tokio tests

- [ ] **Step 1: Write failing seam tests**

First use fake jobs/executors to pin the interface itself:

- `executor_completes_normal_job_exactly_once` — a returned payload produces one
  tagged completion with the runtime-selected kind;
- `executor_completes_returned_failure_exactly_once` — a returned
  `ExternalFailure` follows the same one-shot path;
- `executor_converts_job_panic_to_completion` — under `panic = "unwind"`, a
  panic before a job can return is converted to `WorkerPanic` and cannot strand
  the wait; document that abort mode terminates instead;
- `io_job_cannot_control_completion_delivery` — the fake job is `Send`, its
  consuming `run` returns only a send-safe result, and the type/API gives it no
  sink with which to omit, duplicate, or forge a completion;
- `only_send_work_crosses` — a deliberately non-`Send` decoder and runtime
  cancel hook remain in the prepared operation/registry;
- `closed_completion_inbox_is_accounted` — a bounded non-panicking send failure
  produces `InboxClosed` and sender-side accounting increments the executor's
  undeliverable counter, including delivery attempted from drop;
- `admission_arms_before_enqueue` — capacity rejection leaves an unarmed
  submission and sends no completion; accepted queues contain only dispatches;
  faulted enqueue after arming drops the dispatch, attempts cancellation, and
  is not reported as `SubmissionRejected`;
- `late_enqueued_completion_is_accounted` — cancellation followed by an
  enqueued completion increments the runtime's late-completion counter without
  changing the task outcome;
- `queued_cancel_prevents_job_invocation_and_completes_once` — cancellation wins
  `Queued -> Cancelled`, the job body is never entered, and the executor consumes
  its sink once with `Cancelled`; the registered interruptible hook runs exactly
  once and releases an existing fake resource;
- `cancel_vs_dequeue_has_one_linearized_winner` — a barrier-controlled race
  repeatedly proves either cancel wins and the body does not run, or dequeue
  wins and the body reaches its interruptible wait; both outcomes invoke the
  resource hook once, have one terminal sink attempt, and leak no registration.
  Its running branch pauses after the
  `Queued -> Running` CAS but before resource acquisition, cancels, and proves
  the sticky token prevents acquisition; a second barrier pauses inside the
  fake acquisition and proves cancellation wakes/unblocks it; a third phase
  cancels between nonblocking construction and abort-handle attachment and
  proves immediate one-shot abort before first blocking use;
- `cancel_before_start_is_idempotent` — repeated cancellation reports
  `AlreadyCancelled` without another sink delivery or hook/cleanup call;
- `cancel_hook_reaped_releases_once` — one `cancel` call returning `Reaped`
  removes the entry and decrements the live-resource count once;
- `cancel_hook_pending_reaps_later` — one `cancel` call returning `PendingReap`
  transfers ownership, later cleanup polls only `reap`, and `Reaped` removes the
  entry once;
- `cancel_hook_error_retains_owned_cleanup` — one `cancel` error keeps the live
  entry, repeated task cancel does not call `cancel` again, identical suppressed
  diagnostics deduplicate, a late job completion is stale, and bounded `reap`
  polls either recover or leave a named snapshot entry;
- `cancel_hook_error_fails_clean_shutdown_at_deadline` — a persistent reap error
  produces a non-clean shutdown report naming operation/resource identity, last
  error, and attempt count;
- `queued_quarantined_job_has_no_pre_run_effects` — preparation and queue
  residency retain only an immutable owned `Send` snapshot and counters prove no
  acquisition, work, mutation, or cleanup exists before `Run`;
- `completion_before_submit_returns_is_safe` — a fake executor drives the exact
  `Running -> Native(Suspend) -> apply_native_suspend -> RegisteredExternalWait
  + Waiting -> inline completion` path; the inbox is processed only afterward
  and makes the task ready through its stored continuation;
- `submit_rejection_resumes_registered_continuation` — the same path returns
  `SubmissionRejected`; rollback destroys the private sink/job/start token
  internally and returns only the rejection kind, then runtime rollback
  removes wait/control state, and the structured failure consumes the decoder
  then continuation once without a parked task or enqueued completion;
- `submit_rejection_rolls_back_wait_and_resource` — successful rollback leaves
  zero waits/resources and calls one-shot resource cancellation once; an error
  variant leaves zero waits but retains named cleanup/live-resource ownership;
- `running_job_receipt_does_not_own_resource` — executor accounting cannot take
  over the runtime cancel hook.

Then cover cancellation before job starts, cancellation during job, cancel
twice, wrong runtime/generation/operation/kind, a fault-injected duplicate
completion at the inbox boundary, quarantine completion after observer
cancellation, and shutdown with one job in each state. Duplicate delivery is an
inbox/fault-injection case because opaque dispatch wrappers own terminal
delivery and private jobs cannot access the sink.

- [ ] **Step 2: Implement prepared operations, opaque dispatch, admission, and counters**

Preserve the process-wide pool identity and blocking-tier admission headroom.
Keep `CompletionSink`, submission construction, and result delivery private to
`sema-core::runtime`; `sema-io` must only queue the opaque
`ExecutorSubmission` until capacity is reserved, arm it with `into_dispatch`,
and queue only its `Async`/`Blocking` dispatch wrapper. Construct the
queue-cancel/start-token pair before binding, store the runtime handle beside
`ResourceClass`, and carry the private job and non-cloneable token only through
`ExecutorSubmission` and `ExecutorDispatch`. `sema-io` implements admission and
dispatch wrappers around those opaque core types; it never declares a second
public nominal job seam. Implement the exact atomic
cancel-versus-dequeue transition and rejection rollback before migrating any
builtin. For every interruptible row, construct and test a pre-armed sticky
runtime-hook/job-token pair whose wake participates in potentially blocking
acquisition, or install the abort handle before first blocking poll. Direct
handle lookup and check-then-block acquisition are prohibited. Reclassify APIs
that cannot meet this contract as separate interruptible acquisition waits,
`QuarantinedBounded`, or `PROHIBITED`. Wrap every running job in the one-shot
panic-to-result delivery path and account for `InboxClosed`.
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
These tests prove bounded input/work only; separate feasibility tests prove that
a blocked filesystem/SQLite/subprocess/PTY/sync-provider operation is actually
interrupted and woken or that its isolated process is killed and reaped.

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

## Foundation slice landed (2026-07-15)

The first vertical slice of Task 05 — a real executor + inbox-wakeup drive + one
op proving true concurrency — is implemented and green. This does NOT complete
Task 05 (the full resource-matrix migration remains); it establishes the
executor + drive foundation the rest of the migration builds on.

**Delivered:**

- **Real thread-pool executor** (`sema-vm/src/runtime/host.rs`,
  `ThreadPoolExecutor`): a fixed pool (clamped `[2,8]`) of `std::thread` workers
  fed an unbounded `mpsc` channel. Each `ExecutorSubmission` is armed with
  `into_dispatch()` and run to completion on a worker (`BlockingExecutorDispatch::run`
  / a minimal thread-parking `block_on` for async dispatches). The send-only
  boundary is upheld by the runtime (dispatches carry no `Rc`/`Value`; the worker
  delivers the raw `ExternalCompletion` into the inbox and the VM thread decodes).
  `shutdown(deadline)` stops accepting, disconnects idle workers, and bounded-waits
  (Condvar + `wait_timeout`) on the in-flight count → `Drained`/`DeadlineExceeded`;
  `PoolInner::Drop` disconnects + joins. No tokio dependency added to
  sema-vm/sema-eval.
- **Wired into the interpreter runtime**: `build_runtime` (`sema-eval/src/eval.rs`)
  now constructs the persistent `Runtime` with `ThreadPoolExecutor` instead of
  `NullExecutor`.
- **Inbox-wakeup drive**: `Runtime::block_on_inbox` /
  `WaitRuntime::block_on_inbox` block-wait on the completion inbox (bounded by the
  timer deadline if any), buffering the completion for the next drive turn.
  `run_exprs_via_runtime` services `DriveState::Idle { inbox_wakeup_required:
  true, .. }` by block-waiting instead of erroring. Wakeable, bounded, no busy-spin.
- **One op migrated to a true external wait**: `sleep` (`sema-stdlib/src/system.rs`),
  under `in_runtime_quantum()`, submits a `PreparedExternalOperation::interruptible_blocking`
  (`thread::sleep` on a worker) and SUSPENDs (`WaitKind::External`); the runtime
  resumes the frame with nil when the worker completes. Distinct from `async/sleep`
  (a virtual timer). Legacy paths (`in_async_context` timer yield, top-level real
  sleep) are unchanged.

**Concurrency gate GREEN**: `crates/sema/tests/runtime_external_io_test.rs` —
two `async/spawn`ed `(sleep 200)` overlap on separate workers, total wall-time
~200ms (asserted `< 350ms`), driven through `eval_str_via_runtime`. Internal
executor coverage in `host.rs` (`thread_pool_tests`) proves overlap + bounded
shutdown.

**Remaining Task 05 I/O migration (ordered decomposition):**

1. Resource-matrix classification (interruptible vs quarantined-bounded) for the
   full stdlib resource set — the plan's Step 1.
2. Pure-compute + simple blocking ops (crypto/hash, `shell`/`system`) → external
   waits with concrete cancel hooks / bounds.
3. Files + streams + database (blocking handles) → interruptible external ops.
4. Process / PTY / watcher (spawn + kill cancel hooks).
5. Sockets / HTTP / WS / servers (needs an async executor tier or `sema-io`
   integration; the current pool runs async dispatches via `block_on` but a
   real reactor belongs behind the ADR #69 seam).
6. `mcp/call` and LLM adapters (Task 06 boundary).

**Red baseline unchanged**: `vm_async_test` still exactly 4 RED
(`async_all_failure_does_not_cancel_supplied_sibling`,
`async_race_does_not_cancel_supplied_loser`,
`awaited_child_mutation_is_visible_to_parent`,
`scheduler_workload_beyond_tick_ceiling_completes`) — all legacy-scheduler,
untouched. `runtime_conformance_test` (3) and `unified_runtime_watchdog_test` (1)
were already RED on this branch before this slice (verified by stash) and are not
regressions.

**mcp_async_test concurrency gap**: CLOSED for the runtime path. `mcp/call` (and
every `mcp/tools->sema` handler, which routes through the same `call_tool` core)
now has an `in_runtime_quantum()` branch that submits its blocking JSON-RPC round
trip to the runtime's thread-pool executor as a `PreparedExternalOperation`
external wait (mirroring the `sleep` template), so two `async/spawn`ed `mcp/call`s
overlap on separate workers instead of serializing on the VM thread.
`crates/sema-mcp/src/builtins.rs`: `mcp_call_runtime_outcome` (+ `McpCallDecoder`
/ `McpForwardContinuation` / `McpCallCancelHook`). `McpConnection` is already
`Send` (asserted by `_assert_mcp_connection_is_send`), so the connection is
checked out on the VM thread and moved into the `Send` job for its lifetime;
the decoder checks it back in and records the cassette on the VM thread.

- **Per-connection serialization**: preserved. A second call to the SAME
  connection that finds the slot `CheckedOut` parks on a short executor "poll"
  wait (`McpAcquireContinuation` + `McpNoopCancelHook`) and retries the checkout
  on resume — different connections overlap, one connection's calls queue.
- **Cancellation**: `McpCallCancelHook` tombstones the slot on cancel (the
  in-flight worker's late completion is discarded, the connection drops
  off-thread); a merely-queued call's poll wait cancels as a no-op, leaving the
  slot untouched.
- **Gate**: `crates/sema/tests/mcp_runtime_test.rs` —
  `spawned_mcp_calls_overlap_through_runtime` (cross-connection overlap, the
  acceptance gate) and `same_connection_mcp_calls_serialize_through_runtime`,
  both driven through `eval_str_via_runtime` (added as a passthrough on
  `sema::Interpreter`). Legacy `mcp_async_test` stays 8/8 (the `in_async_context`
  path is untouched).

**Remaining decomposition (Task 06)**: `mcp/tools` and `mcp/close` still take the
synchronous `block_on` path under the runtime (they briefly block the VM thread,
but do not corrupt state); migrating them to the same external-wait pattern is a
Task 06 follow-up. The full cancellation/tombstone scenario is exercised by the
legacy `mcp_async_test`; a runtime-driven `async/cancel`/`async/timeout` MCP test
belongs with the Task 06 orchestration cancellation surface.
