# Task 03.2 Report — Deterministic Fair Ready Queues

## Outcome

Implemented `ReadyScheduler` as active-root round-robin rotation with per-root
FIFO task queues. Private root/task membership sets make duplicate root entries
and duplicate task wakeups idempotent. Removing a settled root deletes its ready
tasks without changing the relative order of the remaining roots.

## TDD evidence

### RED

After adding the four exact-sequence tests, ran:

```text
cargo test -p sema-vm runtime::tests::ready
error[E0583]: file not found for module `ready`
error: could not compile `sema-vm` (lib) due to 1 previous error
```

The failure was the expected missing production queue implementation.

### GREEN

After implementing only the ready queue and membership invariants, ran:

```text
cargo test -p sema-vm runtime::tests::ready
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 383 filtered out

cargo clippy -p sema-vm --all-targets -- -D warnings
Finished `dev` profile ...
```

`cargo fmt --all` also completed successfully before the focused test and
clippy run.

## Files

- `crates/sema-vm/src/runtime/ready.rs` — fair queue, duplicate protection, and
  test-build queue/set agreement assertions.
- `crates/sema-vm/src/runtime/mod.rs` — registers and exports the queue.
- `crates/sema-vm/src/runtime/tests.rs` — four exact sequence/idempotence tests.
- `.superpowers/sdd/task-2-report.md` — this evidence report.

## Commit

Commit subject: `feat(runtime): add fair ready queues` (the commit containing
this report).

## Self-review

**Verdict: Approve.** The diff is limited to Task 03.2. Queue and membership
collections are private; test-only assertions compare root rotation, root queue
keys, task queues, and both membership sets after every mutating operation.
Task identity is globally deduplicated even across roots. Root removal uses
stable `VecDeque::retain`, preserving the remaining rotation. No runtime, timer,
wait, completion, VM-drive, or later-task behavior was introduced.

No remaining concerns identified in this task's scope.
