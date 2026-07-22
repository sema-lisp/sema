# Unified Runtime Remaining Work

**Status:** Planned 2026-07-21 (not started)

## Context

Tasks 1–7 of `2026-07-19-unified-runtime-terminal-inventory.md` landed, including
the restricted VM bridge removal (`c0e9a2c9`, see
`2026-07-19-restricted-vm-bridge-removal.md`, Complete). An independent re-audit
(captured in `c1d81fe7`) found 26 missed nonterminal rows — resource rows
`R02 R03 R05 R06 R08B R08C R09A R09B R10 R13 R14 R16A R16B R17A R19 R21` and
context rows `C04 C05 C06 C07A C07D C08 C09 C10 C12 C14` — plus a workflow-engine
correctness cluster (global `WORKFLOW` TLS, weak run identity, unbounded journal
I/O on the VM thread). This plan sequences the remaining work: Part A rebuilds
workflow correctness in three reviewed commits; Part B closes Task 8 (resource
bounds/cancellation) and Task 9 (context isolation/active I/O) row by row.

Ground rules, restated from the terminal-inventory ledger:

- A row reaches `MIGRATED`/`REMOVED`/`SYNCHRONOUS-PROOF` ONLY when its
  implementation, cancellation/ownership contract, regression test, and source
  guard all agree. **Do not bulk-stamp the ledger** — each commit below flips
  only the rows whose four parts it completes, and updates
  `docs/internals/async-runtime-inventory.md` +
  `docs/plans/evidence/unified-cooperative-runtime/runtime-match-map.tsv` for
  exactly those rows.
- Where a row is genuinely best-effort synchronous CPU work, the commit performs
  an explicit **narrow-and-split** (a capped synchronous split row plus, where
  warranted, a bounded worker-job split row) — never a fake async wrap.
- No thread sleep, generic `block_on`, or synchronous evaluator re-entry inside
  an active quantum; retained host adapters get exact comment-stripped
  allowlist entries in `scripts/unified-runtime-host-adapters.tsv` plus mutation
  fixtures under `scripts/fixtures/unified-runtime-source-policy/`.
- CORE-2 invariant I2: anything task-held that can transitively reach a `Value`
  must be traced (`TaskContextHandle` extensions are traced; the
  `TASK_SCOPE_SEAMS` swap table is deliberately untraced and must stay
  `Value`-free).

Audit basis (verified against HEAD `c1d81fe7`): the row-by-row findings cited
inline below. Key already-landed work this plan builds on rather than re-plans:
stdin coordinated-owner reads (`0e67e854`), git process-group kill + reap
evidence (`28b0f49e`), watcher interpreter ownership (`0fc50a4d`), MCP sandbox
authority isolation (`ee24c700`), LLM task-local dynamic scope (`eec95fb4`),
quarantined file transforms for archive/patch/pdf (`13c18279`), and the
`io_block_on` active-quantum assert in `crates/sema-core/src/io_backend.rs:146`.

---

## Phase A — Workflow correctness (three reviewed commits)

The workflow engine currently keeps its live run in a **global thread-local**
(`WORKFLOW` in `crates/sema-workflow/src/context.rs:39-43`). Under the
cooperative runtime that TLS stays installed across suspensions, so while a
child task is parked inside workflow A, *any* task on the thread — the parent,
an unrelated root, another interpreter — that calls `workflow/checkpoint`
resolves `context::current()` to workflow A and writes into A's journal.
`cur_agent_id` is one shared slot (`context.rs:86-88`, documented "best-effort"
in `crates/sema-stdlib/src/workflow.rs:449-451`), so concurrent steps
cross-attribute tool calls. Run ids are `wf_<unix_secs>_<pid>`
(`context.rs:630-641`) — two runs in one second in one process collide into one
dir whose `events.jsonl` then violates the frozen first-line/seq invariant.
Journal writes, memo round-trips, and full-value rendering
(`Journal::write`/`write_memo` in `crates/sema-workflow/src/journal.rs:77-139`;
`capped_render` = full `pretty_print` then truncate, `workflow.rs:83-97`;
`value_digest`/`memo_store` full-JSON encode, `context.rs:297-433`) all run on
the VM thread inside the quantum.

Design choice (deliberate): workflow scope becomes a **traced
`TaskLocalValue` extension on `TaskContextHandle`**
(`crates/sema-core/src/runtime/task_context.rs`), modeled on
`DynamicTaskState` (`crates/sema-core/src/runtime/eval_task_context.rs:351,633`)
— NOT a fourth `TASK_SCOPE_SEAMS` entry. The seam table is untraced by design
(`crates/sema-vm/src/runtime/state.rs:256-264`) and `WorkflowCtx` holds live
`Value`s (checkpoint state bag, resume memos, MCP handles), so it must be
traced. This is also explicitly preferable to rewriting the workflow engine as
a scheduler feature.

### Commit A1 — task-local workflow scope (closes C12 impl; ledger flip after A3)

**Landed 2026-07-21.** The live run scope is now a traced `WorkflowTaskState`
extension on the owning `TaskContextHandle` (`impl Trace for WorkflowCtx`;
token-keyed scope stack with exact-token/out-of-LIFO removal; `cur_agent`
TASK-PRIVATE, `next_agent_id` run-shared); the stdlib workflow natives install/
read/remove scope on the task context via `current_for`/`cur_agent_for`/
`set_cur_agent_for` (dual-ABI), and `RunTeardown` closes MCP once with a `Drop`
backstop. The `WORKFLOW` thread-local survives only as the `!in_runtime_quantum()`
host-adapter fallback, pinned by a `WORKFLOW_TLS` row in
`scripts/unified-runtime-host-adapters.tsv` plus a quantum-guarded mutation
fixture. Regression: `crates/sema/tests/workflow_scope_isolation_test.rs` +
`gc_stress_test.rs`. **C12 stays `LEGACY`** — its ledger flip and
`runtime-match-map.tsv` re-map wait for A3 (bounded journal I/O) per the ground
rules; A1 completes only C12's implementation leg.

**Implementation**

1. `crates/sema-workflow/src/context.rs`:
   - `impl sema_core::runtime::Trace for WorkflowCtx` — edges for the `state`
     bag, `resume_memos`, and `mcp_handles` `Value`s.
   - New `WorkflowTaskState: TaskLocalValue` holding a scope **stack** of
     `(WorkflowScopeToken, Rc<WorkflowCtx>)` plus a per-task `cur_agent:
     Option<String>` attribution slot. `inherit()` clone-shares the stack
     (`Rc<WorkflowCtx>` is SCOPE-SHARED) and copies the spawner's `cur_agent`
     (children inherit the active workflow **and** step attribution).
   - `install_scope`/`WorkflowGuard` become token-based: install mints a fresh
     `WorkflowScopeToken`; removal removes **that exact token** from the stack
     (mirrors `DynamicTaskState`'s `ScopeId` removal), so out-of-LIFO teardown
     across interleaved tasks restores the exact outer scope.
   - Move `cur_agent_id` off `WorkflowCtx` (run-shared) onto `WorkflowTaskState`
     (TASK-PRIVATE). `next_agent_id` (the mint counter) stays run-shared so ids
     remain unique per run.
   - Scope resolution: `current_for(task_context: Option<&TaskContextHandle>)`
     reads the task extension first; the `WORKFLOW` TLS survives only as the
     HOST-ADAPTER-ONLY fallback, readable only when
     `!sema_core::in_runtime_quantum()`.
2. `crates/sema-stdlib/src/workflow.rs`:
   - `register_thunk_fn`'s `plan` closures receive the `NativeCallContext` on
     the runtime arm so `run_plan`/`step_plan`/`checkpoint_plan` and
     `finish_run`/`finish_step`/`finish_checkpoint` install/read/remove scope on
     the **task context**, not TLS.
   - `workflow/phase`, `workflow/tool-call`, `workflow/mcp-handle`, and the
     checkpoint read-arm move from `register_fn` to
     `NativeFn::simple_with_runtime` dual-ABI so their runtime arm reads the
     task-scoped state.
3. Cancellation/teardown: `ThunkContinuation::resume(ResumeInput::Cancelled)`
   already drives `finish_run` (`workflow.rs:294-318`); make `RunTeardown`'s MCP
   close **once-only** (an `Option`-take on the resolver+handles pair) and add a
   `Drop` backstop so a continuation dropped without resume (runtime teardown)
   still closes MCP handles exactly once and removes its exact scope token.

**Ownership/cancellation contract** — run ctx `SCOPE-SHARED` via inherited
`Rc`; active step attribution `TASK-PRIVATE`; TLS `HOST-ADAPTER-ONLY`.
Cancellation removes the exact scope token and closes MCP handles once;
unrelated roots and fresh interpreters observe "outside a workflow".

**Regression** — new `crates/sema/tests/workflow_scope_isolation_test.rs`
(naming per `task_scope_isolation_test.rs`):
- `parent_checkpoint_while_child_parked_in_workflow_errors_outside_run` (the
  codex example: child A parked inside workflow A ⇒ parent's
  `workflow/checkpoint` reports "outside a workflow/run");
- `unrelated_root_and_second_interpreter_see_no_workflow`;
- `concurrent_steps_keep_separate_agent_ids` (two parallel `workflow/step`
  leaves each emit `tool-call` events attributed to their own `agent_id`);
- `nested_runs_restore_exact_outer_scope_across_interleaved_teardown`;
- `spawned_child_inherits_workflow_and_step_attribution`;
- `cancelled_run_removes_scope_token_and_closes_mcp_handles_once` (stub
  resolver counts `close` calls);
- GC: a checkpoint value forming a cycle through the state bag is collected
  (extend `gc_stress_test.rs`).

**Source guard** — `WORKFLOW_TLS` token row in
`scripts/unified-runtime-host-adapters.tsv` pinning `WORKFLOW.with` /
TLS-reading `current()` to exact counts in
`crates/sema-workflow/src/context.rs`; mutation fixture: an
`in_runtime_quantum()`-guarded TLS read inside a quantum branch must fail
`scripts/check-unified-runtime-legacy.sh`.

### Commit A2 — safe run identity

**Implementation** (`crates/sema-workflow/src/context.rs`,
`crates/sema-workflow/src/journal.rs`)

1. Generated ids gain nanoseconds plus a process nonce:
   `wf_<unix_secs>_<subsec_nanos>_<pid>_<nonce>` (`AtomicU64` process counter)
   in `resolve_run_id` (`context.rs:630`).
2. Explicit ids (env `SEMA_WORKFLOW_RUN_ID` and any library caller) are
   validated **in the library** as exactly one safe path component (non-empty;
   no `/`, `\`, `..`, `.`-only, NUL/control chars) — today only the CLI
   validates (`crates/sema/src/main.rs:1310-1326`).
3. Fresh runs fail if the directory already exists: `Journal::open` switches
   from `fs::create_dir_all(run_dir)` (`journal.rs:63`) to
   `create_dir_all(parent)` + `fs::create_dir(run_dir)`; `AlreadyExists` maps to
   a clear `workflow/run` error. A generated-id collision retries with a fresh
   nonce (bounded attempts).
4. Resume requires an existing explicit run: `SEMA_WORKFLOW_RESUME=1` without a
   run id, or without an existing `<run_dir>/events.jsonl`, is an error in
   `set_workflow_scope` (`context.rs:529`) — not just in the CLI.
5. Concurrent resumes atomically claim different segments:
   `next_resume_segment` (`journal.rs:198-205`) stops being an exists-probe and
   claims via `OpenOptions::new().create_new(true)` in a loop, returning the
   opened file (claim == open, no TOCTOU).

**Ownership/cancellation** — no wait-family change; identity is resolved as
bounded pre-dispatch work before the run scope installs.

**Regression** — sema-workflow unit tests (id format, validation matrix,
collision retry, `create_new` claim race with two threads) plus
`crates/sema/tests/workflow_resume_test.rs` additions: two same-second runs in
one process land in distinct dirs; fresh run into a pre-existing dir fails;
library-level resume of a nonexistent run fails; two concurrent resumes claim
`events.resume-1`/`events.resume-2` distinctly.

**Source guard** — comment-stripped scan entry forbidding
`create_dir_all(<run_dir>)`-shaped reuse in `journal.rs` (allowlist the parent
call at exact count); fixture reintroducing an exists-probe segment claim fails
the policy test.

### Commit A3 — bounded workflow I/O (with A1+A2, flips C12)

**Landed.** Journal I/O is now a per-run bounded FIFO writer thread
(`crates/sema-workflow/src/writer.rs`) owned by `Journal`; the VM thread only
renders-and-`try_send`s. `Journal::write`/`write_memo`/`write_sidecar`/`write_result`
enqueue; `capped_render` is a byte-budgeted renderer (via `context::compact_capped`,
which aborts at the cap) that returns `pretty_print` verbatim for golden-sized values, and
`value_digest`/`memo_store` cap their encode the same way. Hard caps (`MEMO_MAX_COUNT`,
`MEMO_FILE_MAX_BYTES`, `JOURNAL_TOTAL_MAX_BYTES` writer-side odometer, `RENDERED_VALUE_MAX_BYTES`)
are captured before enqueue; overflow (queue-full or size-cap) drops + surfaces one
`journal.overflow` marker. Every terminal path (`finish_run` for a body that ran;
`end_run_before_body` for a pre-body `:mcp` gate) enqueues `run.ended` + `result.json` +
`Flush(ack)`; the runtime path parks on a `PreparedExternalOperation::interruptible_blocking`
flush-ack (Cluster W's `resolve_prepared` shape) before returning, the host path bounded-waits,
and a cancellation skips the barrier (`Journal::drop` `try_send`s `Stop` and detaches — no
`JoinHandle::join` on any VM/cancel path). Regression:
`crates/sema/tests/workflow_journal_writer_test.rs`; goldens stay byte-identical. Source
guard: `scripts/workflow-writer-fs-allowlist.tsv` pins `write_all`/`fs::write` to `writer.rs`
(journal.rs write-free, its `create_dir_all` count dropped 2→1), with a `workflow-journal-sync-write.rs`
mutation fixture. **C12 flipped to `MIGRATED`** in `docs/internals/async-runtime-inventory.md`;
the `runtime-match-map.tsv` re-map + `runtime_conformance_test` inventory reconciliation are
the deferred C7 work and remain red.

**Implementation**

1. **One bounded FIFO journal writer per run** (new
   `crates/sema-workflow/src/writer.rs`): a named OS thread owned by the
   `Journal`, fed by a bounded `std::sync::mpsc::SyncSender<WriterMsg>`
   (`Event(String)` / `Memo{key, json}` / `Sidecar{name, json}` /
   `Flush(AckSender)` / `Stop`). The VM thread only renders-and-`try_send`s.
   Queue-full degrades per the journal's existing best-effort contract: drop +
   overflow counter, surfaced as one `journal.overflow` event when space
   returns (append-only vocabulary addition). Golden-oracle runs are far below
   the queue bound, so `events.jsonl` goldens stay byte-identical.
2. **No filesystem writes or full-value rendering on the VM thread**: every
   `Journal::write`/`write_memo`/`write_sidecar`/`write_result` enqueues;
   rendering becomes budgeted — replace `cap_text(&pretty_print(v, 100))`
   (`workflow.rs:83-97`) with a byte-budgeted renderer that stops emitting at
   the cap (never materializes the full value), and cap
   `value_digest`/`memo_store` encoding the same way. The only VM-thread fs
   work left is the bounded run-dir claim + `events.jsonl` open from A2
   (pre-dispatch admission, same shape as archive preflight).
3. **Hard caps** (module consts, captured before enqueue): `MEMO_MAX_COUNT`
   per run, `MEMO_FILE_MAX_BYTES`, `JOURNAL_TOTAL_MAX_BYTES` (writer-side odometer;
   past it the writer records one overflow marker and drops), and rendered-value
   byte caps. An over-cap memo is *not* stored (the leaf re-runs on resume —
   identical semantics to the existing round-trip guard).
4. **Ordered terminal flush acknowledgement on normal completion**: `finish_run`
   enqueues `run.ended` + `result.json` + `Flush(ack)`; the runtime path parks
   on `WaitKind::External` (a `PreparedExternalOperation::interruptible_blocking`
   job that awaits the ack on the executor blocking tier) before returning the
   envelope, so when `workflow/run` returns normally the journal is complete on
   disk. The host (non-quantum) path uses a bounded `recv_timeout`.
5. **Cancellation/drop never joins or blocks on the writer**:
   `ResumeInput::Cancelled` teardown and `Journal::drop` `try_send(Stop)` and
   detach; the writer exits on `Stop`/disconnect. No `JoinHandle::join`
   anywhere on a VM/cancellation path.

**Ownership/cancellation contract** — writer is RESOURCE-OWNED by the run;
flush-ack wait is `INTERRUPTIBLE` (a cancelled run skips the ack and still
tears down scope/MCP once).

**Regression** — new `crates/sema/tests/workflow_journal_writer_test.rs`:
normal completion has a complete `events.jsonl` at return (flush-ack ordering);
cancelling a run parked on a stalled writer settles promptly and leaves a
runnable sibling; queue overflow drops-and-marks without blocking; over-cap
memo/value paths enforce all four caps; existing
`workflow_cookbook_test.rs`/`workflow_resume_test.rs` goldens stay
byte-identical.

**Source guard** — comment-stripped scan pinning `std::fs::`/`write_all` in
`sema-workflow` to `writer.rs` (exact counts; `journal.rs` keeps only the
open/claim calls); fixture restoring a synchronous `Journal::write` fs call
fails. After A3, flip **C12** in the ledger and re-map its
`runtime-match-map.tsv` payloads.

---

## Phase B — Task 8: resource bounds and cancellation (rows R02–R21)

Shared machinery to reuse (do not reinvent): the FIFO resource gate
(`crates/sema-vm/src/runtime/resource_gate.rs`), checkout offload
`checkout_external`/`CheckoutOp` with its `abort: Option<Box<dyn FnOnce()>>`
hook (`crates/sema-stdlib/src/runtime_offload.rs:687-726,830`), terminal-close
helpers (`prepare_terminal_gate`/`finish_terminal_gate`), quarantine offload
`quarantined_compute` (`crates/sema-stdlib/src/io.rs:1135`), the archive-style
pre-dispatch bounds pattern (`ArchiveBounds`/preflight,
`crates/sema-stdlib/src/archive.rs:42-105`), and the so-far-unused
`QuarantineBound::finite_work` descriptor
(`crates/sema-core/src/runtime/resource.rs:31`). The only real abort hook today
is proc/pty's `group_sigkill_abort` (`runtime_offload.rs:492`).

### Commit B1 — SQLite interrupt + result bounds (closes R16A, R16B)

**Landed.** `db/open`/`open-memory` set a bounded `busy_timeout` and capture the
connection's `Send` `InterruptHandle` beside the registry (`DB_INTERRUPTS`).
Every checkout op (exec/exec-batch/query/query-one/tables) now carries a real
`abort: Some(_)` (fire the interrupt + flag the op) plus a `reclaim` hook: the
generic checkout machinery gained an interrupt-then-reclaim mode
(`CheckoutOp.reclaim` + `CheckoutCancelHook` returning `PendingReap` and reaping
via the reclaim closure, `crates/sema-stdlib/src/runtime_offload.rs`; the gate
registry is untouched). On cancel the worker maps `SQLITE_INTERRUPT`, issues
`ROLLBACK` when `!conn.is_autocommit()`, and hands the connection back through a
shared cell so the reap reinstalls it `Available` instead of tombstoning; the
closed gate lets a fresh op re-create it. Result caps
(`DB_MAX_RESULT_ROWS`/`DB_MAX_RESULT_BYTES`, optional lower per-call override via
`set_db_result_caps_override`) are resolved pre-dispatch and enforced
incrementally inside `collect_query_rows`/`collect_tables` (reject at boundary+1
without buffering the whole result). Regression:
`crates/sema/tests/db_async_test.rs::{db_cancel_interrupts_long_query_and_reclaims_connection,
db_query_result_row_cap_rejects_at_boundary_plus_one, db_open_sets_busy_timeout}`
plus the `sqlite::tests` abort-presence/cap guards. **R16A and R16B flipped to
`MIGRATED`** in `docs/internals/async-runtime-inventory.md`; the
`runtime-match-map.tsv` re-map and the `runtime_conformance_test` inventory
reconciliation are the deferred C7 work and remain red.

Gap (verified): `abort: None` at `crates/sema-stdlib/src/sqlite.rs:237`; no
`get_interrupt_handle` anywhere; `collect_query_rows` (`sqlite.rs:246-265`) and
`collect_tables` (`sqlite.rs:318-327`) buffer unbounded; no `busy_timeout`.

- **Impl**: capture `Connection::get_interrupt_handle()` (rusqlite 0.40,
  `Send`) at open and store it beside the `DbSlot`; supply it as the
  `CheckoutOp.abort` for exec/query/exec-batch/tables. Extend the checkout
  cancel path with an *abort-then-reclaim* mode: on interrupt the worker op
  maps `SQLITE_INTERRUPT` to a cancelled outcome, issues `ROLLBACK` if
  `!conn.is_autocommit()`, and reinstalls the connection `Available` instead of
  tombstoning. Pre-dispatch result caps `DB_MAX_RESULT_ROWS`/`DB_MAX_RESULT_BYTES`
  (optional per-call override, hard ceiling) checked inside
  `collect_query_rows`/`collect_tables` incrementally. `db/open`/`open-memory`
  set `busy_timeout` and carry a bounded open deadline
  (`QuarantineBound::hard_deadline` stays the cleanup net, not the operation
  bound).
- **Contract**: `INTERRUPTIBLE` — interrupt, rollback, release; queued siblings
  wake via the gate; late results rejected.
- **Regression** (`crates/sema/tests/db_async_test.rs`):
  `db_cancel_interrupts_long_query_and_reclaims_connection` (recursive-CTE
  query; assert prompt settle, sibling progress, connection usable afterwards,
  rollback observed), `db_query_result_row_cap_rejects_at_boundary_plus_one`,
  `db_open_sets_busy_timeout`.
- **Guard**: unit test asserting every sqlite `CheckoutOp` carries
  `abort: Some(_)`; match-map rows re-pointed; ledger flips R16A/R16B.

### Commit B2 — KV persistence bounds + honest R09A disposition (closes R09A, R09B)

**Landed.** Stores are bounded before any blocking work dispatches. `KvBounds`
(`KV_MAX_STORE_BYTES` = 64 MiB, `KV_MAX_ITEMS` = 1_000_000) is captured on the VM
thread and carried by value onto the worker. `kv/open` preflights the backing
file's size — a metadata stat pre-dispatch (no allocation) on the runtime path,
plus `read_or_init_store` reading through a `Read::take(cap+1)` capped reader that
rejects an oversized (or growing/TOCTOU) file without allocating its whole
contents. `kv/set` splits admission: `check_value_bytes` rejects an over-cap value
pre-dispatch **without peeking the store** (so it never races the FIFO gate with a
spurious busy error), and `check_item_cap` rejects a *new* key past the item cap on
the exclusively-owned store (sync `with_store_mut`, or the checkout worker via a new
`admit` closure) **before** the mutation — so an over-cap set reinstalls the store
byte-for-byte intact. `flush_store` re-checks the serialized whole-store size as the
final gate before it ever touches disk. A test-only `set_kv_bounds_override`
(clamped to the hard ceilings, mirroring `set_db_result_caps_override`) lets the
regression suite drive over-cap paths without 64 MiB files. Regression:
`kv_async_test::{kv_open_rejects_oversized_store,kv_set_over_cap_value_fails_with_store_intact}`
(existing cancel/concurrency suites stay green); guard: `kv::tests::{runtime_bounds_are_finite_and_clamp_overrides,oversized_store_load_rejected_by_metadata_and_capped_read,set_admission_rejects_over_cap_value_and_item_count}`.
**R09A flipped to `MIGRATED (B2, narrowed)`** and **R09B to `MIGRATED (B2)`** in
`docs/internals/async-runtime-inventory.md`; the `runtime-match-map.tsv` re-map and
the `runtime_conformance_test` inventory reconciliation are the deferred C7 work and
remain red. **Honest R09A disposition:** the JSON backend is a plain
`std::fs::write` with no interrupt handle, so `abort` stays `None` (nothing to
interrupt) — a mid-op cancel keeps the best-effort tombstone-and-discard fallback,
which R09A's contract now states explicitly as its fallback arm rather than a faked
abort. Likewise, R09B claims the byte/item cap — not a wall-clock timer — as the
finite-work bound, since `fs::write` cannot honor a deadline.

Gap (verified): `read_or_init_store` unbounded `read_to_string` + parse,
`flush_store` whole-store write (`crates/sema-stdlib/src/kv.rs:170-179,458-463`);
no caps/deadline; `abort: None` at `kv.rs:241` (nothing to interrupt — plain
`fs::write`).

- **Impl**: `KvBounds` consts (`KV_MAX_STORE_BYTES`, `KV_MAX_ITEMS`) captured
  before dispatch: open preflights file size (metadata + capped read); set/flush
  checks item count + serialized size before enqueueing the write job; a
  bounded op deadline distinct from the cleanup expiry.
- **Contract**: R09B `QUARANTINED-BOUNDED`. R09A is **narrowed, not faked**:
  the JSON backend exposes no interrupt handle, so mid-op cancel = tombstone +
  discarded write, documented as the row's fallback arm; gate close/foreign
  close (already landed) stay the structural edges.
- **Regression** (`crates/sema/tests/kv_async_test.rs`): oversized store load
  rejected pre-dispatch without allocation; over-cap set/flush fails cleanly
  with the store intact; existing cancel tests keep passing.
- **Guard**: bounds-presence unit test + match-map/ledger flip for R09A/R09B
  with the narrowed contract text.

### Commit B3 — serial abort/wake, or split (closes R14)

**Landed 2026-07-21 — SPLIT arm (R14A/R14B).** A real `try_clone()`-based abort
was considered but not shipped: this environment has no serial hardware, so a
close/break wake of a blocked `read_line` cannot be validated on a tier-1
platform, and shipping an unverifiable abort is the wrong disposition. Instead
R14 is split honestly: **R14A** structural gate/open/close waits stay
`INTERRUPTIBLE` (already landed — queued-op FIFO removal, cancelled-open reject,
`serial/close` tombstone/close, mid-op tombstone). **R14B** the checkout ops are
`QUARANTINED-BOUNDED` by the port's read timeout: `serial/open` and every
checkout dispatch validate the timeout `Some(_)`, non-zero, and
`<= SERIAL_MAX_OP_TIMEOUT` (60 s) via `validate_op_timeout` /
`validate_available_timeout` before any blocking op dispatches, so an unbounded
blocking read is unrepresentable and a cancelled op's blocked worker is
guaranteed to free within the validated bound; `abort` stays `None` (nothing to
interrupt). A zero/oversized timeout is rejected up front (a minor tightening —
a zero read timeout is not a bounded op quantum; all shipped examples use
2000–5000 ms). Regression:
`crates/sema/tests/serial_async_test.rs::serial_dispatch_rejects_missing_or_oversized_timeout`
plus the existing no-hardware cancellation/missing-handle suite;
`serial::tests::{validate_op_timeout_matrix,serial_max_op_timeout_is_finite_and_covers_default,open_rejects_zero_and_oversized_timeout_before_device_open}`
guard the bound. **R14 flipped to `MIGRATED (B3)` as split rows R14A/R14B** in
`docs/internals/async-runtime-inventory.md`; the `runtime-match-map.tsv` re-map
and the `runtime_conformance_test` inventory reconciliation are the deferred C7
work and remain red.

Gap (verified): `abort: None` (`crates/sema-stdlib/src/serial.rs:228`); a
cancelled `read_line` parks the worker until the per-port timeout
(default 2000 ms, `serial.rs:257-264`) fires.

- **Impl**: attempt a real wake: capture a `SerialPort::try_clone()` handle at
  open (serialport 4.x) and use it as the `abort` hook (platform-appropriate
  close/break to wake the blocked read). If clone-abort proves unreliable on a
  tier-1 platform, **split**: R14A structural gate/open/close waits
  (INTERRUPTIBLE, already landed) and R14B checkout ops
  `QUARANTINED-BOUNDED by the port timeout` — with the timeout validated
  `Some(_) && <= SERIAL_MAX_OP_TIMEOUT` as a pre-dispatch bound at dispatch
  time, so an unbounded blocking read is unrepresentable.
- **Contract**: cancel settles the task immediately (sticky), the port slot
  tombstones, and worker occupancy is bounded by the validated timeout.
- **Regression** (`crates/sema/tests/serial_async_test.rs`):
  `serial_dispatch_rejects_missing_or_oversized_timeout`;
  cancelled-op-settles-within-timeout using a loopback/pty-backed port where
  available, else the existing no-hardware suite plus the timeout-validation
  unit tests.
- **Guard**: unit test that every serial `CheckoutOp` carries either an abort
  hook or a validated bounded timeout; ledger flip (possibly as split rows).

### Commit B4 — stream/file checkout narrowing + admission (closes R17A, R08B)

Gap (verified): `checkout_input`/`checkout_output` pass `abort: None`
(`crates/sema-stdlib/src/stream.rs:1014,1048`); a cancelled read/write leaves
the worker parked until the OS call returns (regular files: bounded chunk;
FIFOs/char devices: **unbounded**). `io.rs` whole-file value-ABI fallthroughs
do raw blocking reads (`crates/sema-stdlib/src/io.rs:2258,2290,2317,2350` —
host-path only). No file-type admission at `stream/open-*`.

- **Impl** (narrow-and-split; a portable abort for a blocked regular-file
  `read(2)` does not exist, and closing an fd out from under a reader is
  racy):
  1. **Admission**: `stream/open-input`/`open-output` (and the `io.rs`
     quantum offloads) stat the target; non-regular files (FIFO, char/block
     device, socket) are rejected for gate/quarantine offloads with an
     actionable error pointing at the coordinated stdin owner / `proc`
     APIs. This makes every checked-out blocking op chunk-bounded by
     construction.
  2. Declare the regular-file checkout ops' cancellation arm explicitly:
     tombstone + reap bounded by one capped chunk (`R17A` split: structural
     open/close/foreign-close INTERRUPTIBLE — already landed — plus
     bounded-chunk checkout quarantine).
  3. Guard the `io.rs` value-ABI raw blocking reads as HOST-ADAPTER-ONLY
     (they already sit on the `!in_runtime_quantum` arm; add the allowlist
     entries so reintroduction inside a quantum branch fails the scan).
- **Contract**: stream RESOURCE-OWNED (gate on the object — landed); every
  quantum-reachable blocking op has a pre-dispatch chunk/byte bound and a
  non-regular-file admission rejection.
- **Regression** (`crates/sema/tests/stream_file_async_test.rs`):
  `stream_open_input_rejects_fifo_in_quantum_with_guidance` (mkfifo),
  `cancelled_regular_file_read_settles_and_reaps_within_one_chunk`; keep the
  existing cap-boundary, foreign-close, and drop-releases-gate suites green.
- **Guard**: allowlist rows for the value-ABI blocking reads; fixture with a
  FIFO-admitting open fails; ledger flips R08B and R17A with the split wording.

### Commit B5 — stdin/TTY guard completion (closes R08C)

**Landed.** The raw-mode restore token is now a RESOURCE-OWNED `RawModeGuard`
parked in an INTERPRETER-SHARED `Rc<TtyRegistry>` (the `fs_watch` ownership
model): `io/tty-raw!` became a `with_ctx` native that mints a guard, registers a
weak interpreter-teardown hook (`restore_all`), and — when a task context is
installed (a runtime quantum) — attaches the guard to the owning task via a
`RawModeTaskGuard` task-context extension (traced, `Value`-free per CORE-2 I2;
children do not inherit terminal ownership). The terminal is restored **exactly
once** — whichever of an explicit `io/tty-restore!` (`restore_token`), a cancelled
task's `RawModeTaskGuard` drop, or interpreter teardown reaches the guard first
wins; the rest short-circuit on the guard's `restored` flag. The wasm
`read-line`/`read-stdin` blocking reads are annotated HOST-ADAPTER-ONLY, and a new
`RAW_STDIN_READ` token in `scripts/check-unified-runtime-legacy.sh` (active-runtime
scanner) fails any raw `std::io::stdin()` read placed inside an `in_runtime_quantum()`
branch, with pass/fail fixtures (`active-runtime-stdin-read.rs` /
`negated-runtime-stdin-read.rs`). Regression: deterministic
`sema-stdlib` `io::raw_mode_guard_tests::*` (registry teardown / task-guard drop /
cross-path idempotency, asserted through the observable `tty_restore_count` off a
real TTY — no pty is available in this environment) plus in-process
`stream_file_async_test::{interpreter_teardown_restores_raw_mode,cancelled_task_in_raw_mode_restores_termios}`
driving the real native. **R08C flipped to `MIGRATED (B5)`** in
`docs/internals/async-runtime-inventory.md`; the `runtime-match-map.tsv` re-map and
the `runtime_conformance_test` inventory reconciliation are the deferred C7 work and
remain red.

Landed (`0e67e854`): coordinated `StdinOwner` + lease/wake-socket cancellation
for line/byte/key/cursor/kitty reads, structural `WaitKind::Timer` parks, and a
thorough cancellation suite in `stream_file_async_test.rs`. Remaining
(verified): TTY raw mode is a manual `thread_local` token store
(`TTY_STORE`, `crates/sema-stdlib/src/io.rs:73-76`) with **no restore on
cancellation or interpreter teardown**; non-unix `read_line_value` raw-blocks
(`io.rs:33-38`).

- **Impl**: make the raw-mode token a resource-owned guard: move `TTY_STORE`
  into an interpreter-owned registry (the `fs_watch.rs` `Rc<Registry>` +
  `register_interpreter_teardown_hook` model, `fs_watch.rs:195-209`); restore
  termios on interpreter teardown, and register a cancellation cleanup so a
  task cancelled while holding raw mode restores the terminal exactly once
  (idempotent with an explicit later `io/tty-restore!`). Guard the non-unix raw
  stdin path as HOST-ADAPTER-ONLY.
- **Contract**: waiter deregistration (landed) + terminal-guard restore on
  cancel/teardown; TTY registry INTERPRETER-SHARED, guard RESOURCE-OWNED.
- **Regression**: `cancelled_task_in_raw_mode_restores_termios` and
  `interpreter_teardown_restores_raw_mode` (integration_test.rs or the stream
  suite; assert via a pty pair), plus the existing stdin-cancel suite.
- **Guard**: allowlist the two remaining host-path blocking stdin reads; a
  quantum-branch raw `std::io::stdin()` read fails the scan. Ledger flips R08C.

### Commit B6 — spinner lifecycle ownership (closes R19)

**Landed.** The `SPINNERS`/`SPINNER_COUNTER` thread-locals became an
interpreter-owned `Rc<SpinnerRegistry>` captured into the `term/spinner-*`
natives with a weak `register_interpreter_teardown_hook` (the `fs_watch`/B5
`TtyRegistry` model). Each render thread now parks on a `Condvar` wait-timeout
(`SpinnerStop::park_frame`) instead of a bare `thread::sleep` frame loop, so
`stop` sets a flag and wakes it immediately. `term/spinner-stop` is dual-ABI: in
a runtime quantum it signals stop on the VM thread and offloads only the bounded
join via `PreparedExternalOperation::interruptible_blocking`
(`build_spinner_join_suspend`, the `workflow/run` flush-ack shape), so a sibling
task runs while the thread winds down; the host (non-quantum) path joins inline,
bounded by one frame interval via the condvar wake. Interpreter teardown
(`SpinnerRegistry::stop_all`) stops+joins every live spinner (bounded by one
interval each) — no live render thread survives teardown. CORE-2 I2 holds: the
registry, handles, decoder, and cancel hook are all `Value`-free POD. Regression:
`integration_test::{spinner_stop_in_quantum_lets_sibling_run,interpreter_drop_stops_live_spinner_threads}`
(plus the existing `test_term_spinner_*` smoke tests) and
`sema-stdlib` `terminal::tests::{teardown_hook_stops_and_joins_live_spinner,stop_wakes_and_joins_render_thread_promptly}`.
Source guard: a new `SPINNER_FRAME_SLEEP` scan (`--check-spinner-park`, pinned to
zero via `scripts/spinner-park-allowlist.tsv`) fails any `thread::sleep`
reintroduced into `terminal.rs`, with pass/fail fixtures
(`spinner-condvar-park.rs` / `spinner-frame-sleep.rs`). **R19 flipped to
`MIGRATED (B6)`** in `docs/internals/async-runtime-inventory.md`; the
`runtime-match-map.tsv` re-map + the `runtime_conformance_test` inventory
reconciliation are the deferred C7 work and remain red.

Gap (verified, all four parts missing): `SPINNERS` thread-local
(`crates/sema-stdlib/src/terminal.rs:37-40`); per-spinner sleeping thread with
no `Drop`/teardown (an unstopped spinner runs to process exit);
`term/spinner-stop` does a blocking `t.join()` on the VM thread
(`terminal.rs:240-242`).

- **Impl**: interpreter-owned spinner registry (`Rc<SpinnerRegistry>` captured
  into the natives + `ensure_teardown_hook`, exactly the fs_watch model);
  spinner threads park on a `Condvar` wait-timeout instead of bare sleep so
  stop wakes immediately; `term/spinner-stop` in a quantum offloads the bounded
  join via `WaitKind::External` (host path keeps a bounded join ≤ one frame
  interval); interpreter teardown stops+joins every live spinner (bounded by
  one interval each).
- **Contract**: spinner RESOURCE-OWNED, registry INTERPRETER-SHARED, stop
  flag + wake + bounded join; teardown leaves no live spinner thread.
- **Regression** (integration_test.rs): `spinner_stop_in_quantum_lets_sibling_run`,
  `interpreter_drop_stops_live_spinner_threads` (probe via stop-flag/thread
  observation), existing smoke tests stay green.
- **Guard**: teardown-hook presence unit test; scan entry forbidding a bare
  `thread::sleep` frame loop without the condvar wake; ledger flips R19.

### Commit B7 — git output caps + watcher evidence (closes R06, R05)

**Landed 2026-07-22.** `drain_git_pipe` is now a capped incremental drain: a
pre-dispatch per-pipe byte cap (`GIT_MAX_OUTPUT_BYTES` = 64 MiB, lowered for tests
via `set_git_max_output_bytes_override` clamped to the ceiling) is resolved on the
VM thread and carried onto the offloaded job; the first pipe to exceed it wakes the
invocation future (a bounded `mpsc` signal) which kills the owned process group via
the existing `terminate_git_child` hook and rejects the task with a structured
over-cap error — never `read_to_end` of a hostile pipe. The INTERRUPTIBLE
group-kill + async-reap contract is otherwise unchanged. Regression:
`git_async_test::git_output_over_cap_kills_group_and_errors` (a fake `git` floods
stdout past a lowered cap and forks a process-group descendant — the group SIGKILL
reaps it before its delayed marker) plus `git::tests::{git_output_cap_is_finite_and_clamps_overrides,drain_git_pipe_caps_output_and_signals_over_cap}`;
the existing git-async cancellation suite stays green. **R06 flipped to
`MIGRATED (B7)`** and **R05 reclassified to `MIGRATED (B7, reclassified)`** — R05 is
already the shipped model (bounded non-blocking `fs/watch-events` drain +
interpreter-owned teardown from `0fc50a4d`), so its ledger row now states what
shipped rather than the obsolete "external watcher event wait" target (no
`fs_watch` code change). The `runtime-match-map.tsv` re-point (stale
`git.rs`/`fs_watch.rs` payloads) and the `runtime_conformance_test` inventory
reconciliation are the deferred **C7** work and remain red.

Landed: R06 group-kill + bounded-drain cancellation evidence (`28b0f49e`,
`git_async_test.rs:420-581`); R05 interpreter-owned watcher with teardown +
isolation tests (`0fc50a4d`). Remaining (verified): `drain_git_pipe` uses
uncapped `read_to_end` (`crates/sema-stdlib/src/git.rs:274-283`); both rows'
ledger/match-map entries are stale.

- **Impl**: pre-dispatch stdout/stderr byte caps for git offloads
  (`GIT_MAX_OUTPUT_BYTES`, capped incremental drain; over-cap kills the process
  group via the existing hook and reports a structured over-cap error).
- **Contract**: unchanged INTERRUPTIBLE group-kill + async reap, now with
  finite output admission.
- **Regression** (`git_async_test.rs`):
  `git_output_over_cap_kills_group_and_errors` (a command emitting > cap).
- **Guard**: match-map re-point for `git.rs` (stale `git_stdout_async` lines)
  and `fs_watch.rs`; R05 is **reclassified** to its shipped model (bounded
  non-blocking drain + interpreter-owned teardown — the "external watcher event
  wait" target text no longer matches reality and the ledger row must say what
  shipped, not the obsolete target). Ledger flips R05 and R06.

### Commit B8 — diff and secret/PII narrowing (closes R03, R13)

**Landed 2026-07-22 — narrow-and-split (R03A/R03B, R13A/R13B).** `diff/unified`
(super-linear LCS) and `secret/detect`/`secret/redact`/`pii/detect`/`redact/spans`
(regex + Shannon-entropy scan) became dual-ABI `register_runtime_fn` ops: inside a
runtime quantum they capture a per-input byte cap BEFORE dispatch (`DIFF_INPUT_BYTE_CAP`
= 64 MiB / `SECRET_INPUT_BYTE_CAP` = 16 MiB, each lowerable for tests via
`set_diff_input_byte_cap_override` / `set_secret_input_byte_cap_override` clamped to the
ceiling) and offload the compute through `quarantined_compute` over an owned `String`
snapshot (`Send`; a `Finding` is offsets + a `&'static str` kind, so `(text, findings)`
crosses the thread and the matched substrings are sliced back into a `Value` on the VM
thread — no `Value`/`Env` crosses, CORE-2 I2 holds). The rejected path reads `len()`
before any snapshot, so an over-cap input allocates nothing extra. `diff/stat`/`diff/hunks`/
`diff/parse`/`diff/apply` and `hash/digest` stay SYNCHRONOUS with pre-dispatch input-byte
(+ hunk-count for the patch consumers) caps enforced only inside a quantum — an explicit
`SYNCHRONOUS-PROOF` split, not a fake async wrap. The stale/false `secret.rs` module doc
(which claimed an `fs_offload`/`in_async_context()` offload that never existed) now
describes the real `quarantined_compute` mechanism. Regression:
`crates/sema/tests/quarantined_cpu_async_test.rs` (sibling-runs-first for `diff/unified`
and `secret/detect`; cap boundary + one-over rejection for `diff/unified`, `diff/stat`,
`secret/detect`, `hash/digest`; async==sync parity) plus `diff::tests`/`secret::tests`
cap-boundary/clamp unit tests. **R03 flipped to split rows R03A `MIGRATED (B8, split)` /
R03B `SYNCHRONOUS-PROOF (B8)` and R13 to R13A `MIGRATED (B8, split)` / R13B
`SYNCHRONOUS-PROOF (B8)`** in `docs/internals/async-runtime-inventory.md`; the
`runtime-match-map.tsv` re-point and the `runtime_conformance_test` inventory
reconciliation are the deferred **C7** work and remain red.

Gap (verified): the `diff/*` family is uncapped VM-thread sync
(`crates/sema-stdlib/src/diff.rs:387-570`; `diff/unified` is super-linear LCS);
`secret.rs` is fully sync/uncapped and its module doc **falsely claims** an
offload (`crates/sema-stdlib/src/secret.rs:10-19,232-237`).

- **Impl** (narrow-and-split):
  - `diff/unified` (non-linear): pre-dispatch input-byte cap + quantum offload
    via `quarantined_compute` (patch/apply-file pattern). `diff/stat`,
    `diff/hunks`, `diff/parse`, `diff/apply` (O(input)): stay synchronous with
    pre-dispatch input-byte/hunk caps — an explicit synchronous split row.
  - `secret/detect`, `secret/redact`, `pii/detect`, `redact/spans`:
    pre-dispatch input-byte cap + quantum offload via `quarantined_compute`
    (regex+entropy over a snapshot `String` is `Send`); `hash/digest` stays
    synchronous with an input cap. Fix the stale module docs to describe the
    real mechanism.
- **Contract**: offloaded arms `QUARANTINED-BOUNDED` (caps before dispatch;
  cleanup deadline is the net, not the bound); synchronous arms are capped
  bounded CPU (`SYNCHRONOUS-PROOF` split rows).
- **Regression**: extend `archive_pdf_patch_async_test.rs` (or new
  `quarantined_cpu_async_test.rs`): sibling-runs-first for `diff/unified` and
  `secret/detect` in a quantum; cap boundary + one-over rejection for each
  capped op without excess allocation.
- **Guard**: match-map re-point (R03/R13 payloads currently cite deleted
  comment lines); ledger flips R03 (split) and R13 (split).

### Commit B9 — csv/markup/crypto split + quarantine bound descriptors (closes R21, R02, R10)

**Landed 2026-07-22.** `csv/parse`(`-maps`) and `html/parse`/`select`/`text`/
`select-text` became dual-ABI offloaded ops: inside a runtime quantum each
captures a per-input byte cap BEFORE dispatch (`CSV_INPUT_BYTE_CAP` 64 MiB /
`MARKUP_INPUT_BYTE_CAP` 32 MiB, lowerable via
`set_csv_input_byte_cap_override`/`set_markup_input_byte_cap_override` clamped to
the ceiling) and offloads the parse through `quarantined_compute` over an owned
`String` snapshot (the selector string too); the worker returns `Send` cell
strings / normalized HTML / matched outer-HTML/text (and enforces incremental
row/cell or DOM-node caps), decoded into a `Value` on the VM thread — no
`Value`/`Env` crosses the boundary. `crypto.rs` (hashing/base64), `markdown/*`
(streaming), and `csv/encode` stay SYNCHRONOUS with a pre-dispatch input-byte (or,
for `csv/encode`, row-count) cap enforced only inside a quantum — an explicit
`SYNCHRONOUS-PROOF` split, not a fake async wrap. **R02 finalization:** the
archive offload now declares a TERMINAL `QuarantineBound::finite_work` descriptor
(via a new `io::quarantined_compute_bounded`) carrying the input-byte cap — its
caps are enforced incrementally on the worker, so the work is finite by
construction. **R10 split honestly:** R10A input-byte admission is terminal
(pre-dispatch reject); R10B's offloaded `lopdf`/`pdf-extract` parse keeps the
`hard_deadline` net (page/output caps are post-parse), so it is NOT terminally
bounded — subprocess parser isolation is deferred to `docs/deferred.md` (R10B).
Regression: `crates/sema/tests/quarantined_cpu_async_test.rs` (sibling-runs-first
+ cap boundary/one-over for `csv/parse` and `html/select`, plus async==sync
parity) with R02/R10's existing oversize-rejection tests still green
(`archive_pdf_patch_async_test`), a `finite_work` descriptor-presence unit test
(`archive::tests::archive_offload_declares_finite_work_bound`), and
`csv_ops`/`markup`/`crypto` cap-boundary/clamp unit tests. **R21 flipped to split
rows R21A `MIGRATED (B9, split)` / R21B `SYNCHRONOUS-PROOF (B9)`, R02 to
`MIGRATED (B9)`, and R10 to split rows R10A `MIGRATED (B9, split)` / R10B
`MIGRATED (B9, split; documented NON-terminal parser bound)`** in
`docs/internals/async-runtime-inventory.md`; the `runtime-match-map.tsv` re-point
and the `runtime_conformance_test` inventory reconciliation are the deferred **C7**
work and remain red.

Gap (verified): `csv_ops.rs`/`markup.rs`/`crypto.rs` are all uncapped VM-thread
sync; `html/*` parses a full DOM (`markup.rs:122-176`), `csv/parse`
materializes unbounded rows. R02/R10 are already offloaded+capped but encode
their caps only module-locally, and R10's page/output caps are post-parse
(`crates/sema-stdlib/src/pdf.rs:6-14,104-108`).

- **Impl** (narrow-and-split):
  - `csv/parse`(+`parse-maps`) and `html/parse`/`select`/`text`/`select-text`:
    pre-dispatch input-byte (+ row/cell / node) caps, quantum offload via
    `quarantined_compute`.
  - `crypto.rs` (O(input) hashing/base64 — no cost-parameter hashes exist),
    `markdown/*` (streaming), `csv/encode`: synchronous split rows with
    input-byte caps only. No async wrap.
  - R02/R10 finalization: declare the existing caps through
    `QuarantineBound::finite_work` (so the runtime descriptor carries the
    terminal unit cap, not just `hard_deadline`); **split R10** honestly —
    R10A input-byte admission (terminal) / R10B parser quarantine whose
    page/output caps remain post-parse checks under the hard cleanup deadline
    (subprocess parser isolation explicitly deferred to `docs/deferred.md`
    with rationale).
- **Contract**: as B8; R10B stays `QUARANTINED` with a documented non-terminal
  parser bound — the ledger row must say so rather than claim BOUNDED.
- **Regression**: quarantined-CPU suite: sibling progress + cap boundary tests
  for `csv/parse` and `html/select`; R02/R10 keep their existing oversize
  rejection tests, plus a `finite_work` descriptor presence unit test.
- **Guard**: match-map re-point (R02/R10 stale payloads); ledger flips R21
  (split), R02, and R10 (split).

---

## Phase C — Task 9: context isolation and active-I/O (rows C04–C14)

### Commit C1 — output fallback hook guard (closes C05)

Landed: root-tagged capture routes (`crates/sema-core/src/output_hook.rs:36-162`)
with weak-sink pruning tests. Gap (verified): the untagged fallback
`STDOUT_HOOK`/`STDERR_HOOK` (`output_hook.rs:12-27`) run arbitrary closures on
the VM thread inside quantums with no re-entrancy or blocking guard
(installers: sema-wasm, sema-dap, sema-mcp tools, debug_session).

- **Impl**: split the API names (`set_host_stdout_hook`) to mark the family
  HOST-ADAPTER-ONLY; add a per-thread re-entrancy latch in
  `write_stdout`/`write_stderr` (a hook that prints re-enters as a
  pass-through, never recursion); document + enforce the non-suspending
  contract (hooks are `Fn(&str)` and cannot structurally suspend; the latch
  closes the recursion hole).
- **Contract**: capture ROOT-SHARED root-tagged (landed); fallback hooks
  HOST-ADAPTER-ONLY, re-entrancy-safe, blocking prohibited by contract.
- **Regression** (`output_hook.rs` unit + integration): a re-entrant hook
  passes through without recursion; hook output during a quantum still
  delivers for DAP/MCP capture.
- **Guard**: allowlist rows for the four installer sites; ledger flips C05.

### Commit C2 — OTel file export off the VM thread (closes C06)

Gap (verified): `SEMA_OTEL_FILE` uses `with_simple_exporter`
(`crates/sema-otel/src/imp.rs:655-658`) — `JsonlFileExporter` does synchronous
`write_all`+`flush` per span end, on the VM thread at span-guard drop
(`imp.rs:774-783`, `file_exporter.rs:32-62`).

- **Impl**: give the file exporter the same bounded FIFO writer-thread shape as
  A3's journal writer (render on-thread bounded, enqueue, dedicated writer;
  drop/shutdown never joins from a quantum; provider shutdown does a bounded
  flush). Keep the OTLP batch path unchanged.
- **Contract**: span mutation stays synchronous; export becomes an off-thread
  bounded sink; a full queue drops per the exporter's existing best-effort
  trust model.
- **Regression**: `crates/sema-otel/tests/file_export.rs` gains a
  thread-placement assertion (writer thread id ≠ VM thread) and a
  flush-on-shutdown ordering test; the `otel_*` matrix stays green.
- **Guard**: scan entry pinning `std::fs`/`write_all` in `sema-otel` to the
  writer module; ledger flips C06.

### Commit C3 — LLM cache/cassette disk I/O off the quantum (closes C09)

Gap (verified): `load_cached` disk read runs in `complete_offload_prep` on the
VM thread pre-dispatch (`crates/sema-llm/src/builtins.rs:6862` via `:7359`);
`store_cached` writes on resume (`:6878-6880` via `:7521`); cassette
`Cassette::save`/load do append/seek/read/write on scope teardown in-quantum
(`crates/sema-llm/src/cassette.rs:202,316-351`). Pricing is compile-time
embedded (`pricing.rs:23,65`) — no disk work there; only the `CUSTOM_PRICING`
TLS remains (handled in C4).

- **Impl**: fold the cache read into the already-offloaded completion job
  (prep computes the key only; the worker checks disk before network, and a
  disk hit reports zero usage exactly like the memory hit); move `store_cached`
  and cassette save/load onto `io_offload_blocking`/an External job with the
  in-memory tape snapshot rendered bounded on-thread. Add
  `debug_assert!(!in_runtime_quantum())` at the raw fs sites.
- **Contract**: cache/cassette state stays INTERPRETER-SHARED/SCOPE-SHARED
  (landed via `LlmDynScope`); the disk legs become RESOURCE-OWNED offloaded
  jobs; a cache hit still charges zero usage.
- **Regression** (`llm_fake_test.rs` / `llm_cassette_test.rs`): disk-cache hit
  in a quantum leaves a runnable sibling (park-while-sibling shape from
  `llm_root_nonblocking_test.rs`); cassette record/replay correctness
  unchanged; a test-only fs seam asserts no quantum-thread fs call.
- **Guard**: the `debug_assert`s + scan entries for `std::fs` in
  `sema-llm` outside the offload modules; ledger flips C09.

### Commit C4 — LLM residual scope + accounting proof (closes C07A, C08, C10, C07D)

Landed (`eec95fb4` + Task 2): fallback/rate/retry/call-tag/last-usage state in
`LlmDynScope` (TASK-SNAPSHOT/TASK-PRIVATE) with isolation regressions
(`llm_root_nonblocking_test.rs`); pacing parks on `WaitKind::Timer`
(`builtins.rs:7676-7684`); retry backoff is a cancellable pool sleep inside the
interruptible External op; `io_block_on` centrally asserts
`!in_runtime_quantum()`. Remaining (verified): `CUSTOM_PRICING`
(`pricing.rs:92-93`) and `BUDGET_STACK` (`builtins.rs:59`) are still ambient
TLS mutable across suspensions; no cancelled-completion-no-charge test; C07D's
guard is one central assert, not per-site allowlisted, and sync-only provider
jobs have a concurrency bound but no per-job time bound.

- **Impl**: move `CUSTOM_PRICING` and the nested-budget save stack into
  `LlmDynScope` (one field + read/write lines each, per the established
  pattern); require a bounded per-job deadline for the sync-only provider
  offload (`request.timeout_ms` defaulted+clamped before dispatch); keep the
  sync-only path's cancellation explicitly narrowed as best-effort quarantine
  (no fake abort for a provider that only exposes a blocking API).
- **Contract**: config TASK-SNAPSHOT, cursors TASK-PRIVATE, budgets/usage
  SCOPE-SHARED by `Rc`; a discarded/cancelled completion never charges.
- **Regression**: `llm_root_nonblocking_test.rs` —
  `sibling_custom_pricing_change_does_not_reprice_suspended_task`,
  `interleaved_nested_budget_scopes_restore_their_own_frames`;
  `llm_fake_test.rs` — `cancelled_inflight_completion_charges_nothing`
  (FakeProvider, per AGENTS.md this is mandatory for accounting changes) and a
  sync-only-provider deadline test.
- **Guard**: `IO_BLOCK_ON` token rows in `unified-runtime-host-adapters.tsv`
  with exact per-file counts for every provider `io_block_on` site (openai,
  anthropic, gemini, ollama, embeddings, plus the host/CLI sites) + mutation
  fixture; ledger flips C07A, C07D (narrowed), C08, C10.

### Commit C5 — sandbox residual evidence (closes C04)

Landed (`ee24c700`): per-native captured sandboxes + `BrowserAuthority`
snapshot-before-thread-hop + interpreter-isolation regression
(`mcp_builtin_test.rs:240-283`). Remaining (verified): `HOST_SANDBOX`
(`crates/sema-mcp/src/builtins.rs:73-81`) is a retained ambient last-wins TLS
for host entry points; no guard/evidence for non-MCP `EvalContext.sandbox`
child-narrowing.

- **Impl**: none beyond documentation/annotation — `HOST_SANDBOX` is a
  deliberate host adapter; give it an explicit HOST-ADAPTER-ONLY comment and
  allowlist row rather than a rewrite.
- **Contract**: sandbox ROOT-SHARED, child same-or-narrower; MCP authority
  evaluator-specific (landed).
- **Regression**: add `spawned_child_sandbox_is_same_or_narrower` (integration)
  covering the non-MCP `EvalContext.sandbox` path across `async/spawn`.
- **Guard**: `HOST_SANDBOX` allowlist row + fixture; ledger flips C04.

### Commit C6 — registry ownership and teardown re-audit (closes C14)

Only after B1–B6: C14's contract depends on the R rows. Current state
(verified): `fs_watch` is the model (interpreter-owned, teardown hook); KV,
proc, PTY, serial, SQLite registries are per-thread TLS maps with **no
interpreter-shutdown cleanup** (proc/pty orphan live children; spinner fixed in
B6; TTY fixed in B5).

- **Impl**: register interpreter teardown hooks
  (`EvalContext::register_interpreter_teardown_hook`,
  `crates/sema-core/src/context.rs:392`) for the KV/proc/PTY/serial/SQLite
  registries: close Available slots, tombstone CheckedOut ones, kill+reap proc/
  PTY children via the existing group-kill/reap machinery, close gates so
  parked waiters fail fast.
- **Contract**: registry INTERPRETER-SHARED with bounded teardown; handles
  RESOURCE-OWNED; checked-out slots follow their row's cancel arm.
- **Regression**: `interpreter_drop_reaps_live_proc_and_pty_children`,
  `interpreter_drop_closes_kv_serial_sqlite_slots_and_gates`
  (integration/`proc_pty_async_test.rs`), plus two-interpreter isolation
  checks.
- **Guard**: teardown-hook presence unit tests per module; ledger flips C14.

### Commit C7 — evidence reconciliation and final gate

- Regenerate the discovery scans and `runtime-match-map.tsv`
  (`--write-mapping`), hand-classifying only genuinely new payloads; the audit
  found stale payloads for R02/R03/R10/R13, `fs_watch.rs`, and `git.rs`.
- Extend `scripts/unified-runtime-host-adapters.tsv` with the remaining
  retained-adapter families introduced above (WORKFLOW_TLS, IO_BLOCK_ON
  per-site, HOST_SANDBOX, output hooks, workflow/otel fs writers) and add the
  matching pass/fail fixtures to
  `scripts/fixtures/unified-runtime-source-policy/`.
- Update `docs/internals/async-runtime-inventory.md` row statuses — each row
  individually, citing its commit; update this plan and the terminal-inventory
  plan status; refresh
  `docs/plans/evidence/unified-cooperative-runtime/release-readiness.md`,
  `docs/deferred.md` (R10 parser isolation, serial hardware coverage), and
  CHANGELOG.
- Run the full final gate (below) plus the terminal-inventory plan's extended
  gate (`docs-check`, wasm32 check, `check-unified-runtime-legacy.sh`,
  `test-packaged-sema-web.sh`) before requesting independent review.

---

## Sequencing

A1 → A2 → A3 (each independently reviewable; A3 depends on A1's context
plumbing and A2's identity claim). Then Phase B in order B1–B9 (B1–B3 are
independent of each other; B4 before B5 only for shared stream-suite churn;
B7–B9 are independent). Then Phase C: C1–C5 in any order, C6 strictly after
B1–B6, C7 last. Every commit runs focused TDD (red test first where the gap is
behavioral) and updates only its own ledger rows.

## Verification gates

Per commit: the focused suites named in that commit, plus
`cargo clippy --all-targets -- -D warnings` for touched crates.

Branch gate (required before review; run from a clean worktree):

```bash
cargo nextest run --workspace
jake examples
jake smoke-bytecode
jake lint
scripts/check-unified-runtime-inventory.sh --check
```

Supplementary (Phase C7 / final): `jake docs-check`,
`cargo check --target wasm32-unknown-unknown -p sema-wasm`,
`scripts/check-unified-runtime-legacy.sh --check`,
`scripts/test-unified-runtime-source-policy.sh`,
`scripts/test-packaged-sema-web.sh`.
