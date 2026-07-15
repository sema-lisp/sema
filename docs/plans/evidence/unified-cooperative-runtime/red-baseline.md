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
