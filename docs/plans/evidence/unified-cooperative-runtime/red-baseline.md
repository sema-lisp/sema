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

## Task 03 Step 2 — PRIMARY eval flip: LANDED — MILESTONE (2026-07-15)

All three blockers named in the prior revert are now closed on this branch
(native agent-loop cooperative re-entry `e9c1a2b6`, deadlock detection `5791e45a`,
stack-parity + bounded drop-join `3d91dee3`, executor sender-lifecycle
`ac78f7ac`). Re-applied the **PRIMARY flip only** and it is GREEN — **KEPT.**

**The unified cooperative runtime is now THE evaluator for `Interpreter::eval` /
`eval_str`.**

### The flip
`eval_in_global` / `eval_str_in_global` (backing `Interpreter::eval` / `eval_str`)
now call `run_exprs_via_runtime` instead of the legacy `run_exprs_on_vm`
(`crates/sema-eval/src/eval.rs:387,392`). `eval_str_compiled` (backing
`common::eval` / CLI `-e` / `vm_async_test`) deliberately stays on the legacy VM
path — that flip + legacy-scheduler deletion is Task 08.

### Two fixes needed to keep it green (both temporary bridges, deleted with Task 04)
1. **`VM::execute` quantum-suspend** (`crates/sema-vm/src/vm.rs:1345`). `execute`
   now wraps its `run` in `ctx.suspend_runtime_quantum()`. Nested SYNCHRONOUS
   module-body eval for `import`/`load`/eval-callback (`eval_module_body_vm` /
   `eval_value_vm`), fired from a native inside a root VM holding a quantum, was
   rejected by the legacy-VM-entry guard → **18** import/load/module/eval-special-
   form integration_test failures. At the top level (no active quantum) it is a
   no-op. Fixed → integration_test 1055/0.
2. **`suspend_runtime_quantum` must suspend BOTH quantum flags**
   (`crates/sema-core/src/context.rs`). `enter_runtime_quantum` sets the per-ctx
   `runtime_quantum_active` flag AND the thread-local `IN_RUNTIME_QUANTUM` mirror
   (read by ctx-less yielding natives via `in_runtime_quantum` — `mcp/call`,
   `async/*`). The suspend guard only cleared the ctx flag, so a nested
   synchronous re-entry (a `defworkflow` body thunk run via `call_function`) still
   saw `in_runtime_quantum() == true`, made `mcp/call` surface a runtime yield
   into a synchronous run, and crashed with "async yield outside of scheduler
   context" → regressed `workflow_mcp_e2e_test` (2) + `workflow_mcp_interactive_test`
   (2). Now the guard saves/restores both. Fixed → both suites 5/0; sema-core
   319/0 confirms no legacy-path regression.

### Verified (final KEPT state — verbatim `test result:` lines)
- eval_test **1072 passed; 0 failed**
- integration_test **1055 passed; 0 failed** (5 ignored)
- mcp_builtin_test **6/0**, mcp_runtime_test **2/0**, mcp_async_test **8/0**
- embedding_api_test **14/0**, llm_fake_test **29/0**, agent_async_test **7/0**
- workflow_cookbook_test **6/0**, workflow_mcp_e2e_test **5/0**,
  workflow_mcp_interactive_test **5/0**, stream_async_test **10/0**,
  http_concurrent_test **3/0**
- leak_test **7/0**, gc_stress_test **48/0** (drop safety — no hang/leak/abort)
- sema-eval **117/0**, sema-vm **486/0**, sema-core **319/0**
- `cargo check --workspace --tests` exit 0; clippy `--workspace --tests -D
  warnings` clean (only the pre-existing proc-macro-error2 note); fmt clean.

### UNCHANGED baseline RED (all on the legacy/unflipped path — NOT touched by the flip)
- vm_async_test **114 passed; 4 failed** — the same 4 `eval_str_compiled` cases
  (`async_all_failure_does_not_cancel_supplied_sibling`,
  `async_race_does_not_cancel_supplied_loser`,
  `awaited_child_mutation_is_visible_to_parent`,
  `scheduler_workload_beyond_tick_ceiling_completes`).
- runtime_conformance_test **5/3** + unified_runtime_watchdog_test **3/1** —
  pre-existing Task-03 gate drift (documented above).
- sema-lsp lib `builtin_doc_coverage` **212/1** — pre-existing, confirmed RED on
  the pristine baseline (verified by stashing the flip); unrelated to eval.

### What remains to flip `eval_str_compiled` too + delete the legacy scheduler (Task 08)
The residual synchronous native re-entry loops still rely on the legacy
thread-local scheduler (`init_scheduler` in `run_exprs_on_vm`). To finish:
(a) migrate those remaining loops onto the runtime; (b) delete
`init_scheduler`/the legacy `SCHEDULER` TLS and route `run_exprs_on_vm` (i.e.
`eval_str_compiled`) through `run_exprs_via_runtime`; (c) re-baseline the 4
`vm_async_test` RED + `runtime_conformance`/`watchdog` drift against the unified
runtime. The two temporary bridges above are deleted at that point.

## Task 03/08 — FULL flip (`eval_str_compiled`) re-measure post stack/deadlock/drop fixes: MEASURED → REVERTED (2026-07-15)

Re-ran the FULL flip — routing `eval_str_compiled` (backs `common::eval` in the
test suite + CLI `-e`; the last legacy-VM eval entry point) through
`run_exprs_via_runtime` — now that the three blockers from the previous full-flip
revert are claimed closed on this branch: native agent-loop cooperative re-entry
(`e9c1a2b6`), runtime-side deadlock detection (`5791e45a`), stack-parity +
bounded drop-join (`3d91dee3`), executor sender-lifecycle (`ac78f7ac`). The
one-line change (`crates/sema-eval/src/eval.rs:416`
`run_exprs_on_vm(&exprs, &self.global_env)` → `run_exprs_via_runtime(&exprs)`),
plus the already-landed `VM::execute` quantum-suspend bridge, was measured then
reverted.

### Progress since the prior full-flip revert, but STILL NON-VIABLE
The deadlock-detection + drop-join fixes closed causes 1, 2 (partially) and 4
from the prior measure: `channel_recv_empty_error`, `channel_send_full_error`,
`deadlock_detected_two_tasks_waiting` now PASS, and the drop-time
`Resource deadlock avoided` join is gone. vm_async_test moved **106/12 → 109/9**,
and — new this round — **all 4 pre-existing baseline RED RESOLVE** through the
runtime (its scheduling/cancellation/fairness semantics are correct):
`async_all_failure_does_not_cancel_supplied_sibling`,
`async_race_does_not_cancel_supplied_loser`,
`awaited_child_mutation_is_visible_to_parent`,
`scheduler_workload_beyond_tick_ceiling_completes`. Two blockers remain and force
the revert:

**Blocker 1 — eval_test SIGABRTs (unusable oracle).** `deep_structure_str_no_abort`
(`(string-length (str (foldl (fn (acc _) (list acc)) (list 1) (range 5000))))`)
overflows its native stack ("fatal runtime error: stack overflow, aborting",
signal 6) and aborts the whole binary. The stack-parity fix (`3d91dee3`) guards
**VM-frame** recursion via `MAX_FRAMES` (its gates in `mod runtime_eval_tests`
only exercise VM-frame recursion), but this oracle overflows in **native**
recursion — the `str` builtin formatting a 5000-deep nested list — which no VM
guard covers. The runtime drive machinery (`drive`→`poll`→`run_quantum`→native
`str`→recursive format) sits on more native stack than the legacy `vm.execute`
entry, so the deep native format that legacy handles gracefully aborts under the
runtime on the default (small) test-thread stack.

**Blocker 2 — vm_async_test 109/9 (9 new failures), two root-cause families.**
The runtime resolves the 4 baseline RED but breaks 9 others; making them pass on
this path would require deeper runtime work (family A) or weakening oracles.

| Family | N | Tests | Root cause |
| --- | --- | --- | --- |
| A. Spawned-task parking / pending / cancel + error-message parity | 6 | `async_pending_predicate`, `cancel_pending_task`, `cancelled_promise_classifies_correctly`, `channel_close_with_blocked_sender_reports_lost_value`, `native_callback_passed_directly_raises_clear_error`, `async_context_preserved_after_nested_run` | A spawned task that blocks (`channel/recv` on empty, `async/sleep`) is settled/classified differently than the legacy scheduler when observed synchronously by the parent within the same root: `async/pending?`/`cancelled?` mis-report, a channel-parked task cancelled before it runs settles Failed ("channel/recv: channel is empty") instead of Cancelled, and error messages diverge (got "task rejected: … channel is empty" instead of the lambda-wrap hint; the pending-send "lost value 2" text is absent). Runtime callback-re-entry / parked-task-observation parity, Task-04-adjacent. |
| B. Timer virtual-clock hook + cooperative-yield ordering | 3 | `blocking_sleep_hook_receives_clock_advances`, `event_select_yields_to_sibling_in_async_context`, `retry_backoff_yields_lets_sibling_complete_first` | The runtime services timers on the real clock (`std::thread::sleep` / `block_on_inbox`) and does not invoke the virtual-clock `set_blocking_sleep_callback` hook (hook never fires). `event/select` and `retry` backoff sleep block instead of yielding cooperatively, so a shorter-sleeping sibling that must wake first does not (ordering `[slow,fast]`/`[select-done,sibling-ran]` vs expected `[fast,slow]`/`[sibling-ran,select-done]`). Legacy-scheduler virtual-clock + cooperative-yield behavior not yet mirrored on the runtime timer path. |

### DECISION: REVERTED to the exact green baseline (no code changed)
Per the flip mandate's revert rule (breakage large/systemic, or an oracle would
have to be weakened): eval_test SIGABRT is systemic (aborts the correctness
oracle) and blocker-2 family B's channel/select ordering would need
virtual-clock+yield parity the runtime timer path lacks. `git checkout --
crates/sema-eval/src/eval.rs` restored HEAD (`44ffed3a`); working tree clean, no
code changed. This evidence section is the only edit.

### Verbatim `test result:` lines under the flip (measured, then reverted)
- eval_test: **SIGABRT** — `thread 'deep_structure_str_no_abort' has overflowed
  its stack` / `fatal runtime error: stack overflow, aborting` (binary aborts;
  no `test result:` line — the abort kills the run mid-suite).
- vm_async_test: `test result: FAILED. 109 passed; 9 failed; 0 ignored` (baseline
  is `114 passed; 4 failed`; the 4 RED resolved, 9 new failures per families above).

### Post-revert state (exact green baseline, HEAD 44ffed3a)
Working tree clean. `eval_str_compiled` back on `run_exprs_on_vm`
(`crates/sema-eval/src/eval.rs:416`). Baseline unchanged: vm_async_test
`114 passed; 4 failed` (documented RED, verified above via `git stash`).

### What EXACTLY remains to delete the legacy scheduler (Task 08)
The executor (Blocker B of the earlier round) and the primary `eval`/`eval_str`
flip are DONE. To flip `eval_str_compiled` and delete `init_scheduler` + the
`SCHEDULER` TLS:
1. **Native-stack budget parity for deep NATIVE recursion.** The runtime drive
   must not consume materially more native stack per Sema level than
   `vm.execute`, OR deep native-recursive builtins (`str`/display of deeply
   nested structures) need a guard/iterative rewrite so a legacy-graceful program
   never SIGABRTs on the runtime. Extend the `runtime_eval_tests` parity gates to
   cover native-format recursion, not only VM-frame recursion.
2. **Spawned-task observation/cancel/error-message parity (family A).** A
   parent observing a just-spawned blocked child synchronously
   (`async/pending?`/`cancelled?`, cancel-before-run, channel-close-under-blocked-
   sender lost-value, native-callback lambda-wrap hint) must match the legacy
   scheduler's classification and error text through the runtime callback-re-entry
   path — Task-04-adjacent.
3. **Virtual-clock + cooperative-yield timer parity (family B).** The runtime
   timer path must invoke the `set_blocking_sleep_callback` virtual-clock hook and
   let `event/select` / `retry` backoff yield to shorter-sleeping siblings, so
   ordering + blocking-sleep-hook oracles hold without weakening them.
Once (1)–(3) land, route `run_exprs_on_vm` through `run_exprs_via_runtime`, delete
`init_scheduler`/`SCHEDULER` TLS, and re-baseline the (now-resolved) 4
`vm_async_test` cases GREEN.

## Task 03/08 — full-flip parity slice: family A + retry FIXED, flip DEFERRED (2026-07-15)

Closed the bulk of the `eval_str_compiled` full-flip parity gap. Under a temporary
full flip (`eval_str_compiled` → `run_exprs_via_runtime`) the measured `vm_async`
regression went **109/9 → 116/2** and the 4 documented baseline RED all resolved
through the runtime. Blocker 1 (the `deep_structure_str` SIGABRT) is already gone
(fixed by `eb7ee47d` deep-collection teardown flatten — `eval_test` 1072/0 under
the flip). A **NEW** blocker surfaced (native agent-loop concurrency), so the flip
is **DEFERRED** and the parity fixes are **KEPT** (they harden the runtime path the
PRIMARY `eval`/`eval_str` flip already uses; gated by new `runtime_eval_tests`).

### Family A — spawned-task observation/cancel/error parity: ALL 6 FIXED
| Fix | Site | What changed |
| --- | --- | --- |
| Freshly-spawned task is Pending until spawner suspends | `runtime/state.rs` `spawn_detached` | Resume the spawner AHEAD of the child in the ready queue (cooperative parity), so `async/pending?` / `async/cancel`+`async/cancelled?` observe a not-yet-run child. Cascade-fixed `cancel_pending_task`, `cancelled_promise_classifies_correctly`, `channel_close_with_blocked_sender_reports_lost_value` (the cancelled/closed classification now precedes the child's first run, so the deadlock detector never misfires). |
| 0 ms timeout / ready-vs-timer ordering | `runtime/state.rs` `fire_timer` | Virtual-clock cooperative rule: never fire a timer while any task is Ready OR a pending settlement is queued. Fixes the `timeout_zero_lets_ready_work_complete` regression the spawn reorder exposed. |
| Yielding native passed directly to a HOF | `runtime/state.rs` `invoke_callable` (native branch) + `stdlib/list.rs` `check_hof_yield` | Dispatch native callbacks with the runtime-quantum flag active; a leftover PARK yield (channel/promise/sleep) → the lambda-wrap hint, while a driveable `NativeYield` (agent tools that offload I/O) is driven via its stashed `NativeOutcome` — so `native_callback_passed_directly_raises_clear_error` passes WITHOUT regressing `mcp_builtin`/`agent/run`. |
| `async/run` inside async | `stdlib/async_ops.rs` | Under a runtime quantum `async/run` is a cooperative `Sleep(0)` yield (no legacy scheduler to invoke), preserving async context — fixes `async_context_preserved_after_nested_run`. |

### Family B — virtual-clock hook + cooperative-yield ordering: 1 FIXED, 2 RESIDUAL
| Test | Status | Note |
| --- | --- | --- |
| `retry_backoff_yields_lets_sibling_complete_first` | **FIXED** | Prelude `retry` now takes the cooperative async loop under `(__runtime-quantum?)` too (was `(__async-context?)`-only), backing off via `async/sleep` timers the `fire_timer` guard orders correctly. |
| `event_select_yields_to_sibling_in_async_context` | **RESIDUAL** | `event/select`'s async path yields `AwaitIo` (IoHandle polling), which the runtime's VM-yield dispatch does not host yet; under the runtime it takes the blocking sync poll-loop, so it returns the correct value but does not cooperatively yield → ordering diverges. Needs runtime `AwaitIo` support (a new wait kind polled each drive turn, GC-sensitive: the handle may hold `Value`s). |
| `blocking_sleep_hook_receives_clock_advances` | **RESIDUAL** | Requires a VIRTUAL runtime clock: the hook, when installed, must advance logical time WITHOUT real sleep and make `pop_due` fire — the runtime's `MonotonicClock` is real, so `deltas.sum() == 30` (total virtual time) is unreachable. Needs an injectable/advanceable `RuntimeClock`. |

### The NEW flip blocker (why DEFERRED, not KEPT)
`agent_async_test` uses `common::eval` → `eval_str_compiled`. It is **7/0 on the
legacy path** but **3/4 under the full flip**: the native `agent/run` tool loop
(`run_tool_loop` / `__agent-drive`) does not drive cooperatively when entered
through `eval_str_compiled`→runtime, so agents don't overlap, a sibling ticker
freezes during rounds, and explicit cancellation can't interrupt the blocking
loop. This is the "callback-re-entry ABI for NATIVE synchronous loops (Task 04,
widened)" the prior revert already named — NOT a `vm_async` family-A/B item, and
out of scope for this parity slice. Keeping the flip would regress a green suite,
so per the revert rule the flip is reverted (`eval_str_compiled` back on
`run_exprs_on_vm`, `crates/sema-eval/src/eval.rs`).

### DECISION: fixes LANDED on the runtime path, `eval_str_compiled` flip DEFERRED
The family-A + retry fixes are exercised on the always-runtime `eval_str_via_runtime`
gate and by the KEPT primary `eval`/`eval_str` flip. Eight new `runtime_eval_tests`
prove them: `runtime_freshly_spawned_task_is_pending`,
`runtime_cancel_pending_channel_task_classifies_cancelled`,
`runtime_cancelled_promise_classifies_correctly`,
`runtime_yielding_native_as_hof_callback_raises_lambda_wrap_hint`,
`runtime_async_run_yields_and_preserves_context`,
`runtime_timeout_zero_lets_ready_work_complete`,
`runtime_retry_backoff_yields_to_shorter_sleeping_sibling`,
`runtime_top_level_async_side_effect_drains_at_exit`.

### Verified (flip REVERTED — final state, verbatim `test result:` lines)
- vm_async_test **114 passed; 4 failed** (the SAME documented RED:
  `async_all_failure_does_not_cancel_supplied_sibling`,
  `async_race_does_not_cancel_supplied_loser`,
  `awaited_child_mutation_is_visible_to_parent`,
  `scheduler_workload_beyond_tick_ceiling_completes`)
- eval_test **1072 passed; 0 failed**; integration_test **1055 passed; 0 failed**
- sema-eval **126 passed; 0 failed** (118 + 8 new gates); sema-vm **486/0**;
  sema-core **319/0**; sema-stdlib **196/0**
- mcp_builtin **6/0**, mcp_runtime **2/0**, mcp_async **8/0**, embedding_api **14/0**,
  llm_fake **29/0**, agent_async **7/0**, workflow_cookbook **6/0**,
  workflow_mcp_e2e **5/0**, workflow_mcp_interactive **5/0**, stream_async **10/0**,
  http_concurrent **3/0**, leak **7/0**, gc_stress **48/0**
- `cargo check --workspace --tests` exit 0; clippy `--workspace --tests -D warnings`
  clean (only the pre-existing proc-macro-error2 note); fmt clean.

### Precise residual to flip `eval_str_compiled` + delete the legacy scheduler
1. **Native agent-loop cooperative re-entry (Task 04, widened).** `run_tool_loop` /
   `__agent-drive` must drive its provider/tool rounds cooperatively when entered
   through the runtime, so agents overlap and cancellation interrupts the loop
   (`agent_async_test` 7/0 under the flip).
2. **Runtime `AwaitIo` support** for `event/select` / `io/read-key-timeout`
   cooperative yielding (family-B `event_select`).
3. **Injectable/virtual `RuntimeClock`** so the `set_blocking_sleep_callback`
   hook advances logical time deterministically (family-B `blocking_sleep_hook`).
Family A + `retry` are DONE. Once (1)–(3) land, route `run_exprs_on_vm` through
`run_exprs_via_runtime`, delete `init_scheduler`/`SCHEDULER` TLS, and re-baseline
the 4 `vm_async_test` RED GREEN.
