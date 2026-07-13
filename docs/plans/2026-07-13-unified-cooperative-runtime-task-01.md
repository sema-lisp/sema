# Task 1: Scheduler Characterization Tests

## Context

The approved architecture is in
`docs/plans/2026-07-13-unified-cooperative-runtime.md`. This task establishes
RED regression tests for defects the hard-cut rewrite must eliminate. Do not
change production code in this task.

## Requirements

Add focused tests, primarily in `crates/sema/tests/vm_async_test.rs`, for as many
of these independently reproducible current defects as possible:

1. A child task mutating a captured lexical variable is observed by its parent
   after the child is awaited. The expected result is the mutated value.
2. `async/race` with an already-settled winner cancels an owned pending loser;
   the fast path must match the scheduled path.
3. Duration inputs that are negative before rounding (for example `-0.4`) are
   rejected, and NaN/infinite/overflowing values are rejected cleanly.
4. A channel capacity too large to represent or allocate returns a Sema error
   without panicking or aborting. Choose a value that exercises capacity
   validation without risking an actual enormous allocation.
5. A workload exceeding the existing scheduler tick ceiling completes.
6. A perpetual ready/yield workload does not starve an already-due timer or
   an external completion. Keep wall-clock bounds conservative and CI-safe.
7. Nested aggregate/callback workloads preserve the parent task and allow
   callbacks to suspend, spawn, await, and resume without re-entrant corruption.

Some cases may already pass due to recent partial fixes. Keep passing tests if
they assert an important invariant, but the task is not complete unless it
captures at least two genuine current failures. If a candidate is unsafe or
cannot be expressed through the public test API, document why in the report
and replace it with another scheduler edge case from the approved plan.

## Test quality

- Assert language-observable behavior, not implementation details.
- Give each defect its own named test and a short comment explaining the
  invariant, not the history.
- Avoid sleeps when virtual time or deterministic ordering can prove behavior.
- Any real-time test must have a hard, short timeout and no flaky tight bound.
- Run each new test individually and record whether it is RED or already green.
- Run the complete `vm_async_test` target once after adding the suite. RED is
  expected; record the exact failing tests.
- Do not weaken or ignore tests to make the target green.

## Deliverables

- Test-only commit.
- Detailed report at
  `/tmp/unified-runtime-task-01-implementer-report.md`, including commands and
  relevant failure output.
- No production changes.
