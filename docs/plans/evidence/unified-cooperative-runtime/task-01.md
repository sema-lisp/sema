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
- Discovery/evidence chronology is after Task 01 test/harness edits and before
  production runtime behavior changes.
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

## Independent-review amendment results

| Test/command | RED first | Corrected result |
| --- | --- | --- |
| `runtime_conformance_test unified_runtime_scanner_detects_raw_blocking_recv_fixture` | Exit 101: scanner rejected unsupported `--scan-path`; after adding the option it still omitted the filename until `--with-filename` was required. | PASS, 0.02s; exact fixture `path:line:text` observed. |
| `runtime_conformance_test unified_runtime_inventory_mapping_covers_exact_current_matches` | Exit 101: inventory checker did not exist. | PASS, 0.25s; checker covers 1,257 exact production matches. |
| `unified_runtime_watchdog_test noisy_child_is_drained_without_hanging_and_capture_is_bounded` | Exit 101 after 5.00s: pipe backpressure misclassified the noisy child as hung. | PASS, 1.32s; both streams drain concurrently and retained diagnostics are capped at 64 KiB. |
| `unified_runtime_watchdog_test inherited_pipe_writer_does_not_extend_parent_watchdog` | Exit 101 compile error: `run_command_with_timeout` was absent. | PASS, 0.01s; Unix process-group cleanup terminates the inherited writer and joins drains. The oracle accepts absent or zombie state rather than requiring prompt orphan reaping. |
| `vm_async_test async_all_surfaces_first_settled_rejection` | Review found an invalid implicit sibling-cancellation claim. | PASS, 0.02s; the test now asserts only first-settlement error selection. |
| `embed_timeout_reap_test` | Review found timeout observation used as a cancellation owner. | 3 passed in 0.32s using explicit `async/cancel`; normal control uses plain await. |
| `true_cancel_test` | Review found timeout observation used to prove resource abort. | 6 passed in 10.95s using explicit `async/cancel`; process/LLM abort and normal controls remain exact. |
| Agent/stream/MCP cancellation companions | Review found timeout observation used to prove slab/resource cleanup. | Three agent cases, two stream cases, two breaker cases, chat-tools, and MCP all passed individually with explicit cancellation. |

Fix rationale: `async/all`, `async/race`, and `async/timeout` only observe
supplied promises, so cleanup/abort ownership must be expressed by
`async/cancel`. The watchdog drains while polling to avoid pipe deadlock and, on
Unix, owns a process group so drain joins cannot be held forever by descendants.
The inventory uses match-level evidence because path-family tables cannot prove
that mixed cancellation/context policies received distinct migration rows.

## Second acceptance amendment results

The second amendment addresses checker trust, semantic map assignments,
completion liveness, and inherited-pipe behavior without changing production
runtime behavior.

| Contract | RED/limitation first | Corrected result |
| --- | --- | --- |
| Inventory fixture validation | A nonexistent `R99` was accepted when that text appeared outside the ledger table's ID column. | `unified_runtime_inventory_checker_rejects_invalid_fixture_states` passes all valid, empty/missing, stale, duplicate, malformed, `UNREVIEWED`, and nonexistent-row cases; row membership is anchored to the first table column. |
| Mapping regeneration | The writer reclassified every match through path/text heuristics, so a source edit could silently receive the wrong semantic row. | `unified_runtime_inventory_writer_preserves_reviews_and_marks_only_new_matches` proves surviving assignments are preserved, vanished payloads are removed, and new payloads remain `UNREVIEWED`. |
| Partial discovery failure | A failing first discovery `rg` could be masked by a later successful scan. | `unified_runtime_inventory_checker_rejects_partial_discovery_scan_failure` passes for both `--check` and `--write-mapping`; neither writes a map after failure. The missing-binary regression also requires the named diagnostic. |
| Escaped inherited writer | The Unix `setsid` helper held both pipes open and the blocking drain join returned after about 2.00s instead of the direct parent's prompt exit. | `escaped_session_pipe_writers_do_not_block_drain_join` passes in about 0.02s. Unix uses nonblocking reads; Windows uses a dedicated blocking reader and repeatedly targets that reader thread with `CancelSynchronousIo` during shutdown. Unix process-group cleanup remains best effort. |
| Windows inherited writer | Native Windows execution is unavailable in this macOS worktree. | A `cfg(windows)` regression now spawns a two-second PowerShell writer through a promptly exiting helper. The exact watchdog and Windows test code pass an isolated `x86_64-pc-windows-gnu` `cargo check`; Task 07 must run the complete watchdog command in Windows-native CI and retain its run evidence. |
| Completion liveness | Passing `CompletionSink` into a job let it omit delivery, duplicate it, or panic before consuming the sink. | Master and Tasks 02/03/05 use `ExecutorJob`; the executor privately owns one terminal sink delivery, maps panic to `WorkerPanic`, handles queued cancellation, and accounts for closed-inbox/late delivery. |

The full Sema Windows cross-check was attempted with:

```bash
cargo check --target x86_64-pc-windows-gnu -p sema-lang \
  --test unified_runtime_watchdog_test
```

It stops in the unrelated `aws-lc-sys` build script before compiling Sema
because `x86_64-w64-mingw32-gcc` is not installed. To compile-lock the changed
platform branch, a temporary dependency-only crate included the exact
`common/watchdog.rs` plus the exact Windows helper/test code and passed:

```text
CARGO_BIN_EXE_sema=sema cargo check --tests --target x86_64-pc-windows-gnu
Finished `dev` profile
```

The temporary crate was removed and is not evidence or a committed artifact.

The audited 1,257-row match map is assigned by logical operation/source span,
not path/text heuristics. Representative corrections include core abortable
I/O (`async_signal.rs:72` → F01A), prelude agent orchestration
(`prelude.rs:753` → F34C), LLM async dispatch (`builtins.rs:2050` → C07B),
`async/run` (`async_ops.rs:181` → R01D), channel operations
(`async_ops.rs:524` → R01C), server receive (`server.rs:1184` → R15A), system
shell-await/sleep (`system.rs:290` → R18A and `system.rs:419` → R18C), and
WebSocket receive (`ws.rs:328` → R20A). `--write-mapping` now preserves reviewed
assignments, removes vanished payloads, marks only new payloads `UNREVIEWED`, and
contains no semantic classifier. Its post-audit SHA-256 remained
`ba893a85b70635a0eb5071a6ef59115d81895ab44365ba23213a68064a90b204`
before and after regeneration.

## Complete affected targets

| Command | Result | Elapsed | Classification |
| --- | --- | ---: | --- |
| `cargo test -p sema-lang --test vm_async_test -- --nocapture` | 111 passed, 7 failed | 3.07s | RED only for the two scheduled observation defects, captured mutation, sleep/timeout negative validation, channel capacity panic, and finite tick ceiling. |
| `cargo test -p sema-lang --test unified_runtime_watchdog_test -- --nocapture` | 0 passed, 1 failed | 4.12s | RED for the approved fairness/tick-ceiling defect; subprocess completed before its host timeout. |
| `cargo test -p sema-lang --test runtime_conformance_test -- --nocapture` | 2 passed, 0 failed | 0.12s | GREEN. |

No affected target hung.

Amendment full-target regression sweep:

| Command target | Result | Elapsed |
| --- | --- | ---: |
| `vm_async_test` | 111 passed, 7 approved RED | 3.08s |
| `unified_runtime_watchdog_test` | 2 passed, 1 approved fairness RED | 4.10s |
| `runtime_conformance_test` | 4 passed | 0.24s |
| `true_cancel_test` | 6 passed | 10.96s |
| `embed_timeout_reap_test` | 3 passed | 0.31s |
| `agent_async_test` | 7 passed | 2.66s |
| `stream_async_test` | 10 passed | 0.60s |
| `agent_async_breaker_test` | 10 passed | 2.26s |
| `llm_chat_tools_async_test` | 7 passed | 1.23s |
| `mcp_async_test` | 8 passed | 0.44s |

The seven VM RED cases remain exactly: scheduled supplied-promise survival for
`all`/`race`, captured-cell mutation, channel capacity panic, negative
sleep/timeout validation, and the finite tick ceiling. The watchdog RED remains
exactly the ready-storm/timer fairness case; both harness self-regressions pass.

Second-amendment affected targets:

| Command target | Result | Elapsed |
| --- | --- | ---: |
| `runtime_conformance_test` | 8 passed | 0.17s |
| `unified_runtime_watchdog_test` | 3 passed, 1 approved fairness RED, 1 helper ignored | 4.08s |

The three native watchdog passes are noisy stdout/stderr draining, ordinary
Unix process-group cleanup, and escaped-session no-EOF drain liveness. The
Windows inherited-writer regression is target-gated and compile-locked here;
native Windows execution remains Task 07's explicit Windows-native
full-watchdog command and evidence gate.

Post-commit robustness correction: the ordinary Unix descendant oracle polls
`ps -o state= -p PID` and accepts either absence or a state beginning with `Z`.
It still rejects a running descendant, but no longer assumes PID 1 reaps an
orphaned zombie within one second. The full watchdog target rerun produced 3
harness passes, the same single approved fairness RED, and 1 ignored helper in
4.07s.

## Inventory and source guard

The original broad discovery commands returned 868 and 1,074 sorted matches;
their verbatim output remains in
[`task-01-discovery.txt`](task-01-discovery.txt) as historical discovery captured
after Task 01 harness edits and before production behavior changes.

The executable coverage gate scopes both discovery scans to `crates/*/src` and
`playground/src`, then unions them with the legacy production scan. The current
counts are 807 mechanism matches, 302 language/callback matches, 971 legacy
matches, and 1,257 sorted unique union records. Every exact `path:line:text`
record has a stable row ID in
[`runtime-match-map.tsv`](runtime-match-map.tsv), and
`scripts/check-unified-runtime-inventory.sh --check` rejects scan failures,
malformed/duplicate/stale/missing or `UNREVIEWED` mappings, or missing ledger
rows. Ledger membership comes only from the first Markdown-table column. Its
fixture suite also proves that an early failed scan cannot be hidden by a later
successful scan.

`scripts/check-unified-runtime-legacy.sh` scans every `crates/*/src` and
`playground/src` Rust/JavaScript/TypeScript source for the master plan's legacy
runtime tokens, including raw synchronous `.recv()`. Its only source exclusions are the exact generated paths
`crates/sema/src/web/assets/**` and `playground/src/examples.js`; it cannot
exclude an unlisted production crate directory.
It rejects an empty scan, prints current matches on stdout, emits a unified diff
on mismatch, and uses repository-relative, C-locale sorted unique output.

Verification:

| Command | Result |
| --- | --- |
| `scripts/check-unified-runtime-legacy.sh --write-baseline` | Wrote a nonempty baseline. |
| `scripts/check-unified-runtime-legacy.sh --check` | PASS; current scan equals baseline. |
| `wc -l docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline` | 971 lines. |
| `LC_ALL=C sort -c -u docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline` | PASS. |
| `scripts/check-unified-runtime-inventory.sh --check` | PASS; 1,257 exact production matches mapped. |
| `scripts/check-unified-runtime-inventory.sh --write-mapping` plus before/after SHA-256 | PASS; audited map preserved byte-for-byte. |

## Formatting and diff checks

| Command | Result |
| --- | --- |
| `rustfmt --edition 2021 --check` on all 11 modified Rust test/helper files | PASS. |
| `git diff --check` | PASS. |
| `cargo fmt --all -- --check` | Baseline RED only in untouched `crates/sema/tests/stream_file_async_test.rs` and `crates/sema-stdlib/src/async_ops.rs`. Task 01 files are clean under the targeted `rustfmt` check. |
| `jake docs-check` | PASS; 1 selected docs test passed. |

Second-amendment verification:

| Command | Result |
| --- | --- |
| `bash -n scripts/check-unified-runtime-inventory.sh` | PASS. |
| `rustfmt --edition 2021 --check` on the three modified Rust test/helper files | PASS. |
| `scripts/check-unified-runtime-inventory.sh --check` | PASS; 1,257 exact matches. |
| mapping payload `LC_ALL=C sort -c -u` | PASS. |
| `scripts/check-unified-runtime-legacy.sh --check` | PASS. |
| `git diff --check` | PASS. |
| `jake docs-check` | PASS; 1 selected docs test passed. |
| `cargo fmt --all -- --check` | Same baseline RED only in untouched `crates/sema/tests/stream_file_async_test.rs` and `crates/sema-stdlib/src/async_ops.rs`; no amended file appears. |

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

## Third acceptance amendment

Windows drain shutdown now relies only on its reader-start handshake, the stop
flag checked before every blocking read, and repeated `CancelSynchronousIo` on
the exact reader thread. It removes the byte-count/10 ms quiet heuristic and
uses a 64 KiB read buffer so one read can retain the complete capture window;
excess is still drained and discarded while the direct child runs. Windows-only
tests cover immediate markers and multi-chunk head/tail markers. They are
compile-ready, but native execution remains assigned to Windows CI because this
macOS host cannot execute the Windows cancellation path.

The synchronized architecture text now has one canonical executor seam across
the master and Tasks 02/03/05, explicit runtime-owned IDs and callback borrow
discipline, counted handle retention/reaping, attach/shutdown ordering, a
cfg-neutral coalescing completion wake boundary, Task 07 local-host separation,
named legacy bridges, and Task 05 blocking-call feasibility spikes. The legacy
baseline is an exact reviewed snapshot/change detector; it reports additions and
removals for review but does not independently prohibit all additions.

| Command | Third-amendment result |
| --- | --- |
| targeted `rustfmt --edition 2021 --check` | PASS on `watchdog.rs`, `runtime_conformance_test.rs`, and `unified_runtime_watchdog_test.rs`. |
| `cargo test -p sema-lang --test runtime_conformance_test -- --nocapture` | PASS: 8 passed. |
| `cargo test -p sema-lang --test unified_runtime_watchdog_test -- --nocapture` | Expected exit 101: 3 passed, 1 helper ignored, only fairness RED; completed in 4.23s without hanging. |
| `scripts/check-unified-runtime-inventory.sh --check` | PASS: 1,257 exact matches. |
| temporary mapping `--write-mapping` and `cmp` | PASS: reviewed map preserved byte-for-byte, including R17C stream-copy and C07C LLM callback remappings. |
| `scripts/check-unified-runtime-legacy.sh --check` | PASS. |
| `jake docs-check` | PASS. |
| `git diff --check` | PASS. |
| `cargo fmt --all -- --check` | Expected baseline-only failure in untouched `stream_file_async_test.rs` and `async_ops.rs`; no Task 01-owned Rust file differed. |

The exact intentional RED set is unchanged: supplied `async/all` sibling and
scheduled `async/race` loser observation, captured mutation, negative
sleep/timeout validation, unrepresentable channel capacity, finite tick ceiling,
and watchdog fairness. Independent acceptance remains pending.

## Important acceptance findings correction

Task 07 acceptance now requires native Windows evidence from a repository CI
job (for example, `windows-latest` in `.github/workflows/verify.yml`) running:

```powershell
cargo test -p sema-lang --test unified_runtime_watchdog_test -- --nocapture
```

The run must execute and pass the `cfg(windows)` inherited-writer,
immediate-marker, and multi-chunk head/tail marker regressions. A Windows target
cross-check does not execute `CancelSynchronousIo`, cannot replace this gate,
and cannot support Task 07 acceptance. Task 07 evidence must retain the native
workflow URL/run ID and output.

Task 02 now keeps `CompletionSink`, its registered-wait constructor, and its
consuming completion method `pub(in crate::runtime)` in `sema-core`. A checked
factory wraps it in opaque `ExecutorSubmission`; the separate `sema-io` crate
queues that owner and invokes a public sealed driver implemented by the
core-controlled adapter. Worker jobs cannot access delivery. Rejection's
`into_rollback` consumes the opaque submission, destroys the sink privately,
and returns only job/start-token/rejection ownership. This preserves exact
rollback while preventing a rejected caller from delivering.

Validation after the correction: targeted `rg` inspection found no public
`CompletionSink` constructor/completion method and no old three-argument
executor submission contract in the synchronized master and Tasks 02/03/05;
the only `sink: CompletionSink` occurrence is the private field in
`ExecutorSubmission`. `git diff --check` passed. `jake docs-check` passed with 1
selected docs test.

## Executor submission construction seal

Task 02 restricts `ExecutorSubmission::for_registered_wait` to
`pub(in crate::runtime)`. Task 03 runtime registration constructs the opaque
submission, and `sema-io` receives only that submission plus access to its
sealed driver. `SubmissionRejected::into_rollback` destroys the private sink
inside `sema-core` and returns only the job, start token, and rejection kind.

## Independent acceptance

The controller-owned final review covered the complete Task 01 range from
`b19e5cad` through `e3d3cae4`. It found no remaining Critical, Important, or
Minor issues and accepted Task 01. The exact intentional RED set remains assigned
to Tasks 03 and 04. Native Windows watchdog execution remains a binding Task 07
CI gate and cannot be replaced by cross-compilation evidence.
