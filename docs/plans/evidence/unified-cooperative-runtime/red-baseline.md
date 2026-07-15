# Unified cooperative runtime — RED baseline (post compile-restoration)

Authoritative state of the `codex/unified-async-runtime` worktree after the
`VmExecResult::QuantumExpired` compile-restoration slice.

- **Workspace compiles: YES.** `cargo check --workspace --tests` exits 0
  (`Finished \`dev\` profile [unoptimized + debuginfo] target(s)`).
- `cargo clippy --workspace --tests -- -D warnings` — clean (only the
  pre-existing `proc-macro-error2 v2.0.1` future-incompat note from a
  transitive dep, unrelated to this work).
- `cargo fmt --all -- --check` — clean.

## Compile fixes applied

`VmExecResult` gained the `QuantumExpired { .. }` variant in the runtime
rewrite; three host/debug match sites in non-quantum paths were not updated.
Each new arm mirrors the sema-vm convention for debug (non-`run_quantum`)
execution (`vm.rs:1305` uses `unreachable!("debug execution does not install a
runtime quantum")`).

| Crate | File:line | Fix |
| --- | --- | --- |
| `sema-wasm` | `crates/sema-wasm/src/lib.rs:2163` (`start_cooperative`) | Added `QuantumExpired` arm → `unreachable!("debug execution does not install a runtime quantum")` |
| `sema-wasm` | `crates/sema-wasm/src/lib.rs:~2217` & `~3147` (`run_cooperative`, ×2 identical blocks) | Same arm added to both |
| `sema-lang` (test) | `crates/sema/tests/wasm_async_debug_test.rs:272` | Added `QuantumExpired` arm → `unreachable!("debug stepping does not install a runtime quantum")` |

No runtime behavior changed — all three are host/wasm/legacy debug paths that
never install a bounded quantum.

## `cargo test --workspace --no-fail-fast` — failing tests

8 failing tests across 3 binaries (down from 11 after the Task 04
duration/capacity validation slice landed 3 GREEN). All other suites pass.

| Test | Binary | Classification | Justification (file:line) |
| --- | --- | --- | --- |
| `async_all_failure_does_not_cancel_supplied_sibling` | `vm_async_test` | EXPECTED-RED (Task 03/04) | evidence `task-02.md` §Intentional RED baseline (task prompt list) |
| `async_race_does_not_cancel_supplied_loser` | `vm_async_test` | EXPECTED-RED (Task 03/04) | evidence `task-02.md` §Intentional RED baseline |
| `awaited_child_mutation_is_visible_to_parent` | `vm_async_test` | EXPECTED-RED (Task 03/04) | evidence `task-02.md` §Intentional RED baseline |
| `scheduler_workload_beyond_tick_ceiling_completes` | `vm_async_test` | EXPECTED-RED (Task 03/04) | evidence `task-02.md:96` |

### Task 04 "Duration and capacity validation" — now GREEN

The three duration/capacity validation tests moved from EXPECTED-RED to GREEN in
the Task 04 slice (commit `98790706`). They exercise the shared native parsing
helpers, so both the legacy and runtime paths benefit.

| Test | Binary | Classification | Fix |
| --- | --- | --- | --- |
| `sleep_rejects_duration_negative_before_rounding` | `vm_async_test` | GREEN (Task 04) | `duration_ms` rejects `f < 0.0` before rounding (`crates/sema-stdlib/src/async_ops.rs`) |
| `timeout_rejects_duration_negative_before_rounding` | `vm_async_test` | GREEN (Task 04) | shared `duration_ms` negative-before-rounding guard |
| `channel_rejects_unrepresentable_capacity_without_panicking` | `vm_async_test` | GREEN (Task 04) | `channel/new` bounds capacity by `MAX_CHANNEL_CAPACITY` before `VecDeque::with_capacity` |

After this slice `cargo test -p sema-lang --test vm_async_test` reports
`114 passed; 4 failed` (down from 7 failed). The remaining 4 (`async_all_failure_does_not_cancel_supplied_sibling`,
`async_race_does_not_cancel_supplied_loser`, `awaited_child_mutation_is_visible_to_parent`,
`scheduler_workload_beyond_tick_ceiling_completes`) stay EXPECTED-RED, owned by
the runtime scheduling work.
| `ready_spinner_does_not_starve_due_timer` | `unified_runtime_watchdog_test` | EXPECTED-RED (Task 03 fairness) | evidence `task-02.md:107`; main plan `2026-07-13-unified-cooperative-runtime.md:971` ("watchdog fairness remains Task 03") |
| `no_adhoc_tokio_runtimes_outside_allowlist` | `runtime_conformance_test` | IN-PROGRESS TASK-03 DRIFT (see note) | not enumerated as RED; conformance target was 8/8 GREEN at Task 02 (`task-02.md:50`) |
| `unified_runtime_inventory_mapping_covers_exact_current_matches` | `runtime_conformance_test` | IN-PROGRESS TASK-03 DRIFT (see note) | not enumerated as RED; GREEN at Task 02 (`task-02.md:50`) |
| `unified_runtime_legacy_symbols_match_baseline` | `runtime_conformance_test` | IN-PROGRESS TASK-03 DRIFT (see note) | not enumerated as RED; GREEN at Task 02 (`task-02.md:50`) |

### Note on the 3 `runtime_conformance_test` failures

These are migration-inventory *guard* tests, and their oracles are the checked-in
baseline snapshots (`legacy-symbols.baseline`, `runtime-match-map.tsv`) plus the
ADR #69 tokio-runtime allowlist. At Task 02 the whole `runtime_conformance_test`
target was GREEN (8 passed, 0 failed — `task-02.md:50`). The in-progress Task-03
commits already on this branch (`4cf9213f` bounded timer/wait components,
`ee3a7aa9` wait/drive foundations) introduced the drift:

- `no_adhoc_tokio_runtimes_outside_allowlist` — the new `crates/sema-vm/src/runtime/tests.rs`
  contains 23 `Runtime::new(` call sites with no allowlist entries.
- `unified_runtime_inventory_mapping_covers_exact_current_matches` /
  `unified_runtime_legacy_symbols_match_baseline` — symbol line numbers in
  `crates/sema-core/src/async_signal.rs` shifted (e.g. `set_yield_signal` moved
  from :226 → :235), so the checked-in baseline/match-map snapshots no longer
  match.

**These are NOT caused by the `VmExecResult` compile-restoration edits** (which
only touch `sema-wasm` and one test file, and cannot move `async_signal.rs`
lines or add `Runtime::new` calls). They are genuine gate drift owned by
Task 03: reconciling these baseline snapshots and the ADR #69 allowlist is part
of finishing the runtime-module seam. Per the compile-restoration mandate ("Do
NOT alter test oracles"), the baselines were left untouched and the drift is
recorded here rather than silenced.

## Critical channel-cancel hang (found by adversarial verification, FIXED)

Three independent adversarial reviewers confirmed an infinite-hang bug in
`RuntimeState::cancel_waiting` (`crates/sema-vm/src/runtime/state.rs`). It had
dedicated branches for `promise_waits`, `promise_set_waits`, `protocol_waits`,
and `timers`, but **none for `channel_waits`**. A VM task parked on
`channel/send` / `channel/recv` is tracked only in `channel_waits`, with a key
minted by `WaitRuntime::issue_internal_wait` that is never inserted into
`WaitRuntime::active`. A sticky cancellation on such a task fell through to the
generic fallback, whose `waits.cancel(key)` returned `None` without waking the
task, yet the fallback still returned `Ok(true)` unconditionally — so the task
stayed `Waiting` forever and `cancel_waiting` re-selected it and spun.

Reachable both ways:
- `close_for_interpreter_drop` / `shutdown` run
  `while matches!(self.cancel_waiting(), Ok(true)) {}` → infinite loop / process
  hang on `Runtime` drop (e.g. a detached `(async/spawn (fn () (channel/recv …)))`).
- `(async/cancel <channel-parked-task>)` never settled; `async/cancelled?`
  stayed `#f`; a parent `async/await` hung. Also hit by `async/spawn-all` /
  `async/pool-map` owned fail-fast cancelling channel-parked workers.

**Fix:** added a `channel_waits` branch to `cancel_waiting` that deregisters the
task from the `ChannelRegistry` (`channels.cancel_wait`, dropping a cancelled
blocked sender's unsent value), removes the `channel_waits` entry, and wakes the
task so it settles Cancelled on its next visit — mirroring the `promise_waits`
branch. The generic fallback was hardened to return `Ok(false)` (no progress)
instead of `Ok(true)` when `waits.cancel` matched nothing and nothing was woken,
so any future off-`active` wait kind can never reintroduce the spin.

**Regression tests** (bounded, in `sema-eval` `mod runtime_eval_tests`; a wrong
fix fails a wall-clock assertion or CI timeout rather than hanging):
`runtime_cancel_channel_recv_parked_task_settles_cancelled`,
`runtime_drop_with_channel_parked_task_does_not_hang` (the hang proof),
`runtime_drop_with_channel_send_parked_task_does_not_hang` (sender path),
`runtime_owned_fail_fast_cancels_channel_parked_worker`.

## Task 03 Step 2 — eval flip: MEASURED → REVERTED (2026-07-15)

Flipped the primary eval entry points (`eval_in_global`/`eval_str_in_global`,
backing `eval`/`eval_str`) from the legacy `run_exprs_on_vm` (TLS `init_scheduler`
path) onto `run_exprs_via_runtime` (the interpreter's single persistent unified
runtime), measured the full workspace, and reverted.

### What routed through the runtime cleanly
- eval_test **1072/0** and integration_test **1055/0** — the two oracles held.
- integration_test only stayed green after fixing a re-entry-guard gap: the
  `eval`/`load`/`import` builtins re-enter the VM synchronously
  (`eval_value_vm`/`eval_module_body_vm` → `VM::execute` → `run`), which the
  runtime-quantum guard (`vm.rs:1662`) rejected with *"legacy native callback
  cannot re-enter a VM during an active runtime quantum"* → 18 integration_test
  failures (eval/load/import/module/macroexpand). Fix: suspend the quantum for
  the duration of `VM::execute` (`ctx.suspend_runtime_quantum()`), mirroring the
  existing `run_nested_closure_args` bridge — `execute` is the legacy synchronous
  run-to-completion entry the runtime never drives through (it uses
  `seed_main_frame` + `run_quantum`). Restored integration_test to 1055/0.
- Simple async flips fine: `(await (async …))`, sync stdlib I/O, timers
  (`async/sleep`), multimethods, modules, dynamic context — all green.

### Blocking gap categories (why reverted)
`run_exprs_via_runtime`'s synchronous drive loop (NullExecutor; idle only
services a timer deadline, errors on `inbox_wakeup_required`) cannot service
genuine async/concurrent I/O reached through the flipped `eval`/`eval_str`. Both
categories were GREEN at baseline; neither is fixable without the Task 04–06
executor / callback-ABI work.

| Category | N | Tests | Root cause | Unblocked by |
| --- | --- | --- | --- | --- |
| HOF-callback async | 1 | `embedding_api_test::embedding_async_all_and_channels` | `(foldl + 0 (async/all (map (fn (x) (async (* x x))) …)))` → "async yield outside of scheduler context": a stdlib HOF callback (`map`/`foldl`) that spawns/awaits `async` re-enters synchronously and the async yield escapes the cooperative scheduler | Task 04 `NativeOutcome::Call` callback-re-entry migration |
| Concurrent external blocking I/O | 3 | `mcp_async_test::{cross_connection_overlap_proves_no_serialization, scheduler_not_stalled_sibling_completes_before_slow_call, cancellation_tombstones_connection_and_interpreter_stays_healthy}` | `async/spawn`ed tasks making blocking `mcp/call`s do not truly overlap on the NullExecutor sync-drive path (observed `["a-timed-out-without-marker","b-done"]` — no in-flight interleave); legacy `init_scheduler` achieved real concurrency | Task 05/06 real executor (run blocking leaf calls off-thread while siblings progress) |

The 4 pre-existing `vm_async_test` RED did **not** resolve: they run through
`common::eval` → `eval_str_compiled`, deliberately NOT flipped (flipping
`eval_str_compiled` broke 14 more async tests that suspend on
channels/blocking-sleep/deadlock — the same executor gap, wider blast radius).

### Post-revert state (exact green baseline)
eval_test **1072/0**, integration_test **1055/0**, vm_async_test **114 passed;
4 failed** (documented RED), sema-eval **91/0**, sema-vm **482/0**,
embedding_api_test **14/0**, mcp_async_test **8/0**. Working tree clean at HEAD;
no code changed (this evidence + the task-03 Step 2 annotation are the only
edits). `cargo check --workspace --tests` exit 0.

### What stands between here and "eval fully on the runtime, legacy scheduler deletable"
1. **Callback-re-entry ABI (Task 04)** — replace the synchronous
   `eval_value_vm`/HOF-callback `VM::execute` re-entry with a suspend/resume
   `NativeOutcome::Call` continuation so an `async` inside a `map`/`foldl`
   callback yields cooperatively instead of escaping the scheduler. This also
   retires the temporary `suspend_runtime_quantum` bridge.
2. **Real executor (Task 05/06)** — run blocking leaf I/O (`mcp/call`, HTTP,
   external I/O) off-thread so `async/spawn`ed siblings truly overlap; the
   `run_exprs_via_runtime` drive loop must service `inbox_wakeup_required` idle
   (currently a hard error), not just timer deadlines.
3. Then flip `eval`/`eval_str` **and** `eval_str_compiled` together (they share
   `common::eval` and the CLI `-e` path), which should also resolve the 4
   `vm_async_test` RED (scheduling/fairness/cancellation semantics owned by the
   runtime).

## Task 03 Step 2 — eval flip RE-MEASURE post HOF-migration: MEASURED → REVERTED (2026-07-15)

Re-ran the SAME flip (`eval_in_global`/`eval_str_in_global` → `run_exprs_via_runtime`
+ the `VM::execute` `suspend_runtime_quantum()` bridge) AFTER the
map/filter/foldl/reduce/for-each/sort-by `NativeOutcome::Call` migration
(commits `51e0356a`, `65721842`), to test whether that migration closed gap (A).

### Result: the migration MOVED the gap, did not fully close it

| Gap | Status | Suite | Root cause |
| --- | --- | --- | --- |
| (A) HOF-callback async | **CLOSED** | embedding_api_test **14/0** (`embedding_async_all_and_channels ... ok`) | `NativeOutcome::Call` ABI now yields the callback's `async` cooperatively |
| (B) concurrent blocking I/O | **UNCHANGED (RED)** | mcp_async_test **5 passed / 3 failed** (`cross_connection_overlap_proves_no_serialization`, `scheduler_not_stalled_sibling_completes_before_slow_call`, `cancellation_tombstones_connection_and_interpreter_stays_healthy`) | NullExecutor sync-drive: `async/spawn`ed blocking `mcp/call`s don't overlap → needs real executor (Task 05/06) |
| (C) module-imported HOF open-upvalue escape | **NEW (RED)** | integration_test **1051 passed / 4 failed** (`test_hof_dispatch_open_upvalue_shallow_write_back`, `test_hof_dispatch_open_upvalue_deep_nesting_no_panic`, `test_imported_hof_wrapper_set_write_back`, `test_imported_hof_transitive_closure_no_slot_clobber`) | `Eval error: captured variable's stack slot is not on this VM (a closure with open upvalues escaped its owning VM)`. Imported module's `for-each`-based HOF dispatch, driven through the runtime, runs the handler callback on a VM that doesn't own the handler's open-upvalue cell. Callback-site CORRECTNESS gap in the new ABI (was 1055/0 pre-migration) |

Oracles/other suites under the flip: eval_test **1072/0**; vm_async_test **114/4**
(same 4 pre-existing RED — `eval_str_compiled` still unflipped); llm_fake_test
**29/0**, agent_async_test **7/0**, workflow_cookbook_test **6/0**,
stream_async_test **10/0**, http_concurrent_test **3/0** — all green.

### Post-revert state (exact green baseline)
No code changed. integration_test **1055/0**, embedding_api_test **14/0**,
mcp_async_test **8/0**, eval_test **1072/0**, vm_async_test 114/4 (documented RED).
`cargo check --workspace --tests` exit 0; fmt + clippy clean.

### Assessment
The flip blocker is no longer a single category. Gap (A) is retired by the HOF
migration. Two blockers remain: (B) the real executor / concurrent blocking I/O
(Task 05/06), and (C) an open-upvalue-escape correctness bug in the
module-imported HOF `NativeOutcome::Call` dispatch path under the runtime — a
Task-04-adjacent callback-re-entry fix, independent of the executor.

## Task 03 Step 2 — eval flip MILESTONE re-measure post real-executor: MEASURED → REVERTED (2026-07-15)

Re-ran the flip AFTER the real `ThreadPoolExecutor` landed in the interpreter's
persistent runtime (`build_runtime` in eval.rs now uses `ThreadPoolExecutor::new`,
not `NullExecutor`), `run_exprs_via_runtime` gained a full idle drive
(`block_on_inbox` for `inbox_wakeup_required`, timer-deadline sleep), and the
blocking `sleep`/`mcp/call` external-wait migration + open-upvalue ABI fix
(f297b9ef) landed — i.e. gaps A/B/C all claimed closed. The executor gap is no
longer the blocker; **two new/deeper blockers surfaced. REVERTED.**

Changes measured (then reverted): (1) `eval_in_global`/`eval_str_in_global` →
`run_exprs_via_runtime` (primary flip); (2) the `VM::execute`
`suspend_runtime_quantum()` bridge for `eval`/`load`/`import` synchronous
re-entry; (3) ALSO `eval_str_compiled` → runtime (full flip); (4) a
`suspend_runtime_quantum` bridge at the `agent/run` tool-dispatch site
(`sema-llm execute_tool_call`) — which did NOT fix the agent-loop breakage.

### PRIMARY flip alone (`eval`/`eval_str`) — held the oracles but broke the native agent loop
- eval_test **1072/0**, integration_test **1055/0** held. Fragile async suites
  green: embedding_api **14/0**, mcp_async **8/0**, llm_fake **29/0**,
  agent_async **7/0**, workflow_cookbook **6/0**, stream_async **10/0**,
  http_concurrent **3/0**, leak **7/0**, gc_stress **48/0**.
- `mcp_builtin_test` regressed **6/0 → 4/2** (previously green, NOT in any RED
  baseline):
  - `test_mcp_agent_tool_call_round_trips_arguments` — `agent/run` returns the
    raw tool output `"ping"` instead of the model's final `"all done"`; the
    multi-turn loop stops after one tool turn.
  - `test_mcp_tool_error_surfaces_to_agent` — the mcp tool error `kaboom` escapes
    `agent/run` as a hard `WithTrace` failure instead of being caught and fed back
    for the model to recover with `"recovered"`.
  - ROOT CAUSE: the native `agent/run` loop (sema-llm `run_tool_loop` /
    `execute_tool_call` → `call_callback` for the tool handler AND the provider
    `complete`) is synchronous run-to-completion code. Entered via
    `eval_str`→runtime, its tools/`mcp/call`/`complete` become runtime
    external-waits the native loop can't cooperatively drive → turn continuation
    and tool-error recovery break. This is the Task 04 callback-re-entry ABI
    surfacing at a NATIVE loop re-entry site (a different site than the stdlib-HOF
    gap A/C already migrated).

### FULL flip (ALSO `eval_str_compiled`, backing `common::eval` + CLI `-e`) — NON-VIABLE
- eval_test **SIGABRTs**: `deep_structure_str_no_abort` overflows its native
  stack ("fatal runtime error: stack overflow, aborting", signal 6). The
  runtime's per-quantum drive machinery uses more native stack than the legacy
  `vm.execute` entry, tipping a deliberately-deep-recursion oracle over. This
  aborts the whole eval_test binary — the correctness oracle is unusable.
- vm_async_test **114/4 → 106/12** (+8 new failures), spanning FOUR causes:
  1. **Missing runtime-side deadlock/all-blocked detection.** Top-level
     `(channel/recv (channel/new 1))`, full `channel/send`, and a two-task mutual
     wait park forever; the `run_exprs_via_runtime` drive loop hits an unhandled
     `DriveState::Idle{next_deadline:None, inbox_wakeup_required:false,
     legacy_io_wakeup_required:false}` and errors "root did not settle" instead of
     the legacy synchronous "empty"/"full"/"deadlock detected"
     (`channel_recv_empty_error`, `channel_send_full_error`,
     `deadlock_detected_two_tasks_waiting`).
  2. **Changed top-level synchronous channel-error semantics** — under a runtime
     quantum, `channel/recv`/`channel/send` always park/yield rather than raising
     the synchronous "empty"/"full" error. Making these pass would WEAKEN the
     oracle (change the documented top-level error contract).
  3. **Runtime cancel/pending correctness + ordering** — `async_pending_predicate`,
     `cancel_pending_task`, `cancelled_promise_classifies_correctly` return wrong
     values; `event_select_yields_to_sibling_in_async_context`,
     `retry_backoff_yields_lets_sibling_complete_first`,
     `blocking_sleep_hook_receives_clock_advances` diverge on ordering/hook use.
  4. **Drop-time thread-join deadlock** — `failed to join thread: Resource
     deadlock avoided (os error 11)` when the interpreter's real-executor runtime
     drops with parked tasks.

### DECISION: REVERTED to the exact green baseline (no code changed)
eval_test **1072/0**, integration_test **1055/0**, vm_async_test **114/4**
(documented RED), mcp_builtin_test **6/0**, embedding_api_test **14/0**,
mcp_async_test **8/0**. `cargo check --workspace --tests` exit 0. Breakage is
systemic (native agent-loop re-entry across the pervasive `eval_str` path;
eval_test SIGABRT; an oracle-weakening channel-semantics change) — per the flip
mandate's revert rule ("revert if breakage is large/systemic or an oracle would
have to be weakened").

### What now stands between here and "legacy scheduler deletable" (Task 08)
Gap (B)'s executor is IN PLACE; the remaining blockers are:
1. **Callback-re-entry ABI for NATIVE synchronous loops (Task 04, widened).** The
   `NativeOutcome::Call` migration must cover native `call_callback` loops
   (`agent/run`/`run_tool_loop`, `llm/map`, streaming), not only stdlib HOFs, so
   an agent tool doing `mcp/call`/HTTP through the runtime cooperatively drives
   its multi-turn loop and preserves tool-error recovery.
2. **Runtime-side deadlock/all-blocked detection + host drive-loop policy** that
   settles the root with a legacy-parity error ("empty"/"full"/"deadlock") when
   all tasks are permanently blocked — plus a decision on top-level synchronous
   channel-op semantics (park vs. immediate error) that does NOT weaken the
   vm_async_test oracles.
3. **Native-stack budget of the runtime drive** (deep-recursion parity with the
   legacy `vm.execute` entry) and a **bounded interpreter-drop join** for the real
   executor (no `Resource deadlock avoided`).
Until (1)–(3) land, `eval_str_compiled`/`common::eval` cannot flip, and even the
primary `eval`/`eval_str` flip breaks the native agent loop. The `*_via_runtime`
entry points remain available for incremental validation.
