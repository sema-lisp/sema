# Task 01: Runtime contracts, characterization, and inventory

Date: 2026-07-14

Worktree: `sema/.worktrees/unified-async-runtime`

Start commit: `b19e5cad` (`docs: expand unified runtime implementation plan`)

## Scope and entry criteria

- `8acca1de` is an ancestor of the start commit.
- The provisional report at `/tmp/unified-runtime-task-01-implementer-report.md`
  was used only as discovery input.
- The initial worktree difference was the controller-owned, untracked
  `.superpowers/` directory.
- This layer changes tests, a test helper, source guards, and planning evidence.
  It does not change production behavior.
- The perpetual fairness program was removed from the in-process test crate and
  placed behind a ten-second subprocess watchdog.
- Execution-plan correction: the watchdog must be reusable by sibling
  integration-test crates, so its public harness lives in
  `crates/sema/tests/common/watchdog.rs` and is exported by `common/mod.rs`.

## Individual characterization results

All commands use `cargo test -p sema-lang`. Elapsed values below are Cargo's
reported test execution times; build time is not included.

| Test and command suffix | Result | Elapsed | Public observation |
| --- | --- | ---: | --- |
| `--test vm_async_test race_with_settled_winner_does_not_cancel_supplied_loser -- --exact --nocapture` | `GREEN-BASELINE` | 0.03s | Returned `(:winner #f :loser-finished)`. The legacy pre-settled fast path already observes without cancelling. |
| `--test vm_async_test async_race_does_not_cancel_supplied_loser -- --exact --nocapture` | `RED-EXPECTED` | 0.01s | Awaiting the supplied loser produced `async/await: task was cancelled`; expected `(:fast #f :slow-finished)`. |
| `--test vm_async_test async_all_failure_does_not_cancel_supplied_sibling -- --exact --nocapture` | `RED-EXPECTED` | 0.01s | Awaiting the supplied sibling produced `async/await: task was cancelled`; expected `(#f :slow-finished)`. |
| `--test vm_async_test awaited_child_mutation_is_visible_to_parent -- --exact --nocapture` | `RED-EXPECTED` | 0.01s | Parent observed `0`; expected the child's captured-cell mutation `42`. |
| `--test vm_async_test sleep_rejects_duration_negative_before_rounding -- --exact --nocapture` | `RED-EXPECTED` | 0.01s | `async/sleep -0.4` returned `nil`; expected a language error containing `non-negative`. |
| `--test vm_async_test timeout_rejects_duration_negative_before_rounding -- --exact --nocapture` | `RED-EXPECTED` | 0.01s | `async/timeout -0.4` returned `:ready`; expected a language error containing `non-negative`. |
| `--test vm_async_test sleep_rejects_non_finite_durations_cleanly -- --exact --nocapture` | `GREEN-BASELINE` | 0.02s | NaN and both infinities are rejected as finite-number errors without a Rust panic. |
| `--test vm_async_test sleep_rejects_overflowing_finite_duration_cleanly -- --exact --nocapture` | `GREEN-BASELINE` | 0.01s | A finite duration outside the supported range is rejected cleanly. |
| `--test vm_async_test channel_rejects_unrepresentable_capacity_without_panicking -- --exact --nocapture` | `RED-EXPECTED` | 0.01s | `channel/create 9223372036854775807` panicked in `RawVec` with `capacity overflow`; expected a language capacity error. |
| `--test vm_async_test scheduler_workload_beyond_tick_ceiling_completes -- --exact --nocapture` | `RED-EXPECTED` | 2.97s | A finite 1,000,001-yield program failed with `async scheduler: exceeded maximum ticks`; expected `:complete`. |
| `--test vm_async_test nested_aggregate_callback_can_spawn_await_and_resume_parent -- --exact --nocapture` | `GREEN-BASELINE` | 0.01s | The nested callback returned the exact `(5 23 203)` result. |
| `--test unified_runtime_watchdog_test ready_spinner_does_not_starve_due_timer -- --exact --nocapture` | `RED-EXPECTED` | 4.07s | The child exited nonzero with the tick-ceiling error at `async/race`; it did not reach the ten-second host timeout. Expected printed `(:timer-fired #f)`. |
| `--test runtime_conformance_test unified_runtime_legacy_symbols_match_baseline -- --exact --nocapture` | `GREEN-BASELINE` | 0.04s | The scanner matched the committed baseline. |

The pre-settled race result differs from the plan's provisional RED prediction.
That is a baseline capability, not a weakened oracle: the language-level result
already satisfies the approved observation contract.

## Complete affected targets

| Command | Result | Elapsed | Classification |
| --- | --- | ---: | --- |
| `cargo test -p sema-lang --test vm_async_test -- --nocapture` | 111 passed, 7 failed | 3.07s | RED only for the two scheduled observation defects, captured mutation, sleep/timeout negative validation, channel capacity panic, and finite tick ceiling. |
| `cargo test -p sema-lang --test unified_runtime_watchdog_test -- --nocapture` | 0 passed, 1 failed | 4.12s | RED for the approved fairness/tick-ceiling defect; subprocess completed before its host timeout. |
| `cargo test -p sema-lang --test runtime_conformance_test -- --nocapture` | 2 passed, 0 failed | 0.12s | GREEN. |

No affected target hung.

## Inventory and source guard

The two required discovery commands returned 868 and 1,074 sorted matches.
Their verbatim sorted output, including the exact commands, is committed in
[`task-01-discovery.txt`](task-01-discovery.txt). Every production path in the
union maps to a ledger row in
[`async-runtime-inventory.md`](../../../internals/async-runtime-inventory.md).
Tests, examples, documentation, generated assets, and host-owned test
interpreters map to verification rows V01–V10.

`scripts/check-unified-runtime-legacy.sh` scans every `crates/*/src` and
`playground/src` Rust/JavaScript/TypeScript source for the master plan's legacy
runtime tokens. Its only source exclusions are the exact generated paths
`crates/sema/src/web/assets/**` and `playground/src/examples.js`; it cannot
exclude an unlisted production crate directory.
It rejects an empty scan, prints current matches on stdout, emits a unified diff
on mismatch, and uses repository-relative, C-locale sorted unique output.

Verification:

| Command | Result |
| --- | --- |
| `scripts/check-unified-runtime-legacy.sh --write-baseline` | Wrote a nonempty baseline. |
| `scripts/check-unified-runtime-legacy.sh --check` | PASS; current scan equals baseline. |
| `wc -l docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline` | 956 lines. |
| `LC_ALL=C sort -c -u docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline` | PASS. |

## Formatting and diff checks

| Command | Result |
| --- | --- |
| `rustfmt --edition 2021 --check crates/sema/tests/common/mod.rs crates/sema/tests/common/watchdog.rs crates/sema/tests/runtime_conformance_test.rs crates/sema/tests/unified_runtime_watchdog_test.rs crates/sema/tests/vm_async_test.rs` | PASS. |
| `git diff --check` | PASS. |
| `cargo fmt --all -- --check` | Baseline RED only in untouched `crates/sema/tests/stream_file_async_test.rs` and `crates/sema-stdlib/src/async_ops.rs`. Task 01 files are clean under the targeted `rustfmt` check. |

## RED handoff

| RED contract | Owner that turns it GREEN |
| --- | --- |
| Captured mutation | Task 03 |
| Nested callback architecture (baseline behavior is already GREEN) | Task 03 |
| Observational `async/all`, scheduled `async/race`, and timeout semantics | Task 04 |
| Duration and capacity validation | Task 04 |
| Finite yields and fairness watchdog | Task 03 |
| Legacy source matches | Tasks 02–08 |

Independent acceptance review is controller-owned and remains pending. This
evidence does not claim that review.
