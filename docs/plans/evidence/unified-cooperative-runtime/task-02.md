# Task 02: Core runtime data model

Date: 2026-07-14

Worktree: `sema/.worktrees/unified-async-runtime`

Start commit: `3509bb25`

Implementation range verified: `3509bb25..3e858c54` (16 commits, from
`9cc1ac83 refactor(runtime): add checked runtime identities` through
`3e858c54 feat(runtime): add legacy task ID bridge`). This evidence commit is
intentionally outside that implementation range. Independent review and the
final review document remain pending and were not performed here.

## Implemented contracts

- Checked scalar and runtime-scoped identities, non-wrapping allocators, and
  independent origin/cancellation/lifetime relationships.
- Returned, failed, and cancelled settlements plus lossless structured
  cancellation and timeout conditions.
- Send-only external completion envelopes, typed decoding, prepared resource
  ownership, capability-bound registration, opaque executor submissions, and
  exactly-one terminal delivery attempts for admitted work.
- Native return/call/suspend vocabulary, consuming continuations, wait and
  resume variants, fallible exact-multiplicity tracing, and the private
  runtime-aware `NativeFn` path while retaining the legacy ABI. Final review
  finding `UR-T02-R201` added `NativeFn::with_payload_result`, whose typed
  function-pointer callback holds only a `Weak` handle while the public payload
  field owns the single strong, registered-tracer-visible payload edge.
- Typed task-local extensions and optional `EvalContext` task-context handles.
- A named, fallible `LegacyRuntimeBridge` for the still-raw task-ID seams.
  Production scheduling, I/O behavior, and legacy callback signatures remain
  unchanged.

The implementation is spread across the commits named above. The hardening
commits `633e59ab`, `8182032e`, `fe6b2dba`, `dc4e26cb`, `ced12412`,
`b831626e`, `34d5a3d3`, and `9f1a2a9d` pin allocator, condition, executor,
native-context, and concrete task-local invariants.

## Required gates

Elapsed values are wall clock where `/usr/bin/time` was retained; cached
focused filters completed in less than one second each.

| Command | Result | Elapsed |
| --- | --- | ---: |
| `cargo test -p sema-core` | PASS: 314 unit, 23 integration/property, and 1 doc test passed; 1 doc test ignored. | 0.82s |
| `cargo test -p sema-lang --test runtime_conformance_test` | PASS: 8 passed. | 0.62s |
| `cargo fmt --all -- --check` | Initial expected baseline-only failure in `stream_file_async_test.rs` and `sema-stdlib/src/async_ops.rs`; after `cargo fmt` and scanner-coordinate refresh, PASS. | 1.11s final |
| `cargo clippy -p sema-core --all-targets -- -D warnings` | PASS. | 0.34s |
| `jake docs-check` | PASS: 1 selected docs test passed. | 8.80s |
| `git diff --check` | PASS. | 0.01s |

`cargo fmt` changed only wrapping/indentation in the two known committed files;
no expression, literal, assertion, or control flow changed. Because the legacy
and inventory snapshots contain exact `path:line:text` records, formatting
removed one scanner match (the formerly single-line duration error) and shifted
reviewed `async_ops.rs` coordinates by two lines. The baseline and mapping were
refreshed without changing semantic row assignments.

## Focused verification

| Area / command filter | Result |
| --- | --- |
| `cargo test -p sema-core ids` / `relationships` | PASS: ID unit/public tests and the relationship public test. |
| `settlement` / `condition` | PASS: settlement public test; 1 condition unit and 3 condition public tests. |
| `completion` / `resource` / `executor` | PASS: 2 completion integration tests, 2 resource unit tests, 30 executor unit tests plus 1 executor integration test. |
| `native` / `cycle` | PASS: 8 native tests and 34 cycle tests. |
| `task_context` / `context` | PASS: 7 task-context tests and 21 context tests. |
| runtime conformance `legacy` / `scanner` / `inventory` filters | PASS; the complete target's 8 tests pass. |
| `cargo check -p sema-vm` | PASS. | 
| `scripts/check-unified-runtime-legacy.sh --check` | PASS: 970 exact matches. |
| `scripts/check-unified-runtime-inventory.sh --check` | PASS: 1,256 exact mapped matches; no `UNREVIEWED` row. |

At `3509bb25`, both snapshots had 971 legacy and 1,257 inventory records.
Task 02 additions and removals net to 970 and 1,256 at this evidence point.
The final one-record reduction is formatting-only as described above; reviewed
survivors retain their prior semantic map rows.

## Intentional RED baseline rerun

`cargo test -p sema-lang --test vm_async_test -- --nocapture` completed in
4.83s wall clock (3.21s test time): **111 passed, exactly 7 failed**. These are
the unchanged Task 01 REDs and are not claimed green:

1. `async_all_failure_does_not_cancel_supplied_sibling` — supplied sibling is
   cancelled; awaiting it reports `async/await: task was cancelled`.
2. `async_race_does_not_cancel_supplied_loser` — supplied loser is cancelled;
   awaiting it reports the same cancellation error.
3. `awaited_child_mutation_is_visible_to_parent` — parent sees `0`, expected
   captured-cell mutation `42`.
4. `channel_rejects_unrepresentable_capacity_without_panicking` — Rust panic
   `capacity overflow` instead of a language capacity error.
5. `scheduler_workload_beyond_tick_ceiling_completes` — finite 1,000,001-yield
   workload reports `async scheduler: exceeded maximum ticks`.
6. `sleep_rejects_duration_negative_before_rounding` — `-0.4` rounds to an
   accepted duration and returns `nil`.
7. `timeout_rejects_duration_negative_before_rounding` — `-0.4` rounds to an
   accepted duration and returns `:ready`.

Task 01 treats fairness as affected, so the full watchdog target was rerun.
`cargo test -p sema-lang --test unified_runtime_watchdog_test -- --nocapture`
completed in 5.89s wall clock (4.23s test time): 3 harness tests passed, 1
helper was ignored, and only
`ready_spinner_does_not_starve_due_timer` remained RED with the same finite
tick-ceiling error before the ten-second host timeout.

## Panic and platform qualifications

The executor panic-to-`WorkerPanic` tests execute only under
`cfg(panic = "unwind")`, which is this verification build. Under
`cfg(panic = "abort")`, Rust cannot unwind through `catch_unwind`; a worker,
future poll, destructor, decoder, or terminal-delivery panic terminates the
process. This run therefore verifies the unwind implementation and compilation
of the abort branches, not out-of-process abort behavior.

This host is macOS. Native Windows watchdog cancellation/drain behavior and a
WASM host execution were not run. Per Task 01 evidence, Task 07 still requires
a native Windows CI run of the complete watchdog target; cross-compilation is
not a substitute. No platform-specific production adapter is introduced by
Task 02.

## Remaining deferrals

Tasks 03–08 still own queues, timers and drive turns, wait registration and
rollback, retention/reaping, execution of native call continuations, output
routing, migration of ambient `EvalContext`/TLS fields, production `sema-io`,
the WASM local host, language predicates, and deletion of every legacy bridge.
The seven VM REDs remain assigned to Tasks 03/04, and watchdog fairness remains
Task 03. Independent Task 02 review with stable `UR-T02-R###` IDs and its final
review document remain deliberately outstanding.
