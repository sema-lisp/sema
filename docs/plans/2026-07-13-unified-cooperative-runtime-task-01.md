# Task 01: Runtime Contracts, Characterization, and Inventory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish trustworthy RED/GREEN runtime oracles, a complete migration
ledger, and non-hanging legacy-source guards before production runtime code
changes.

**Architecture:** This task changes tests and planning evidence only. It corrects
the provisional characterization commit `52293e61`, distinguishes observation
from ownership, moves the perpetual-spinner oracle behind a process watchdog,
and records every current async path with its future cancellation class and
task-context policy.

**Tech Stack:** Rust integration tests, Sema source snippets, shell source scans,
Markdown evidence, Cargo, Jake.

## Execution contract

- **Status:** Not accepted; this plan supersedes the provisional Task 01 work in
  `52293e61` and is the next executable layer.
- **Dependencies:** Architecture commit `8acca1de` and the master specification.
- **Immutable inputs:** The approved observation/ownership, multiple-root,
  cancellation, fairness, resource, host, and final-profiling contracts.
- **Exact start state:** Clean worktree; branch contains `52293e61` and
  `8acca1de`; latest commit subject is
  `docs: expand unified runtime implementation plan`; production code still
  matches the state at `8acca1de`.
- **Parallel work:** Inventory discovery and independent oracle review may run in
  parallel. One implementer owns all test/scanner edits so test names and RED/
  GREEN evidence cannot diverge; the reviewer does not edit implementation.

## Global constraints

- Read `AGENTS.md` and
  `docs/plans/2026-07-13-unified-cooperative-runtime.md` before editing.
- Work only in the dedicated `codex/unified-async-runtime` worktree.
- Do not change production Rust, Sema prelude behavior, generated assets, or
  public documentation in this task.
- `async/all`, `async/race`, and `async/timeout` observe supplied promises and
  MUST NOT cancel them.
- Owned-loser tests belong to Task 04, which introduces `async/race-owned` and
  `async/with-timeout`.
- Every in-process infinite-work test is forbidden. A host process or
  deterministic bounded-drive harness must own the timeout.
- RED tests remain enabled. Do not weaken, ignore, or convert them to string-only
  implementation assertions.
- Scratch files under `/tmp` are not evidence. Commit evidence under
  `docs/plans/evidence/unified-cooperative-runtime/`.
- No profiling or benchmarking is performed in this task.

---

## Files and responsibilities

**Modify**

- `crates/sema/tests/vm_async_test.rs` — language-observable finite
  characterization tests.
- `crates/sema/tests/runtime_conformance_test.rs` — source-boundary and baseline
  manifest checks.
- `docs/internals/async-runtime-inventory.md` — executable migration ledger.
- `docs/plans/2026-07-13-unified-cooperative-runtime.md` — keep Task 01 status
  accurate if implementation reveals another superseded oracle.

**Create**

- `crates/sema/tests/unified_runtime_watchdog_test.rs` — subprocess-owned
  fairness and hang oracles.
- `scripts/check-unified-runtime-legacy.sh` — exact legacy-symbol inventory
  scanner.
- `docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline` —
  sorted baseline output from the scanner.
- `docs/plans/evidence/unified-cooperative-runtime/task-01.md` — commands,
  results, RED/GREEN classification, and handoff.

## Interfaces

**Consumes**

- Existing helpers `eval`, `eval_vm_err`, and `Value` in
  `crates/sema/tests/vm_async_test.rs`.
- The real CLI binary through `env!("CARGO_BIN_EXE_sema")`.
- Approved semantics in the master specification.

**Produces**

- Enabled characterization tests whose names and outcomes appear below.
- `run_sema_with_timeout(source: &str, timeout: Duration) -> TimedRun`, used by
  Task 09 for hostile scheduler programs.
- A legacy scan whose output is stable, sorted, relative-path based, and
  diffable against `legacy-symbols.baseline`.
- An inventory table with one row per production symbol/path and these columns:

```text
Area | Path and symbol | Current mechanism | Target wait family |
Cancellation class | Context policy | Owning layer | Existing tests | Status
```

## Entry criteria

- [ ] Confirm `git status --short --branch` shows only expected local work.
- [ ] Confirm `git rev-parse --short HEAD` includes architecture commit
  `8acca1de` in its ancestry.
- [ ] Read the current Task 01 implementation report if it still exists at
  `/tmp/unified-runtime-task-01-implementer-report.md`; treat it as discovery,
  not evidence.
- [ ] Record the current names around the characterization section:

```bash
rg -n "awaited_child_mutation|cancels_owned|negative_before_rounding|tick_ceiling|ready_spinner|nested_aggregate" \
  crates/sema/tests/vm_async_test.rs
```

Expected: all provisional tests from `52293e61` are present.

## Task 1: Correct observational combinator oracles

**Files:**

- Modify: `crates/sema/tests/vm_async_test.rs`

- [ ] **Step 1: Replace the pre-settled race cancellation oracle**

Replace `race_with_settled_winner_cancels_owned_pending_loser` with this
language-observable contract:

```rust
#[test]
fn race_with_settled_winner_does_not_cancel_supplied_loser() {
    assert_eq!(
        eval(
            r#"
            (define loser (async (async/sleep 10) :loser-finished))
            (define winner (async/resolved :winner))
            (define result (async/race (list winner loser)))
            (list result (async/cancelled? loser) (await loser))
            "#,
        ),
        Value::list(vec![
            Value::keyword("winner"),
            Value::bool(false),
            Value::keyword("loser-finished"),
        ]),
    );
}
```

- [ ] **Step 2: Replace the scheduled race sibling-cancellation oracle**

Replace `async_race_cancels_losing_siblings` with:

```rust
#[test]
fn async_race_does_not_cancel_supplied_loser() {
    assert_eq!(
        eval(
            r#"
            (define slow (async (async/sleep 10) :slow-finished))
            (define fast (async :fast))
            (define result (async/race (list slow fast)))
            (list result (async/cancelled? slow) (await slow))
            "#,
        ),
        Value::list(vec![
            Value::keyword("fast"),
            Value::bool(false),
            Value::keyword("slow-finished"),
        ]),
    );
}
```

- [ ] **Step 3: Replace the `async/all` sibling-cancellation oracle**

Replace `async_all_reject_cancels_pending_sibling` with:

```rust
#[test]
fn async_all_failure_does_not_cancel_supplied_sibling() {
    assert_eq!(
        eval(
            r#"
            (define slow (async (async/sleep 10) :slow-finished))
            (define boom (async (error "boom")))
            (try (async/all (list boom slow)) (catch e nil))
            (list (async/cancelled? slow) (await slow))
            "#,
        ),
        Value::list(vec![
            Value::bool(false),
            Value::keyword("slow-finished"),
        ]),
    );
}
```

- [ ] **Step 4: Rename the over-cancellation guard to describe observation**

Keep `combinator_short_circuit_spares_unrelated_task` if it still adds coverage,
but update its comment so it does not claim that supplied promise sets are
owned. It proves unrelated work survives; the three tests above prove supplied
work survives.

- [ ] **Step 5: Run the corrected tests individually**

```bash
cargo test -p sema-lang --test vm_async_test \
  race_with_settled_winner_does_not_cancel_supplied_loser -- --exact --nocapture
cargo test -p sema-lang --test vm_async_test \
  async_race_does_not_cancel_supplied_loser -- --exact --nocapture
cargo test -p sema-lang --test vm_async_test \
  async_all_failure_does_not_cancel_supplied_sibling -- --exact --nocapture
```

Expected on the legacy scheduler: RED because the current scheduler cancels
supplied siblings. Expected after Task 04: PASS.

## Task 2: Preserve finite characterization tests

**Files:**

- Modify: `crates/sema/tests/vm_async_test.rs`

- [ ] **Step 1: Keep the captured-cell test exact**

`awaited_child_mutation_is_visible_to_parent` must continue to expect `42`. Do
not replace it with an implementation-state assertion.

- [ ] **Step 2: Keep duration validation split by failure class**

Retain the tests for negative-before-rounding, non-finite input, and finite
overflow. Add the same negative-before-rounding oracle for observational
timeout so the shared parser contract is explicit:

```rust
#[test]
fn timeout_rejects_duration_negative_before_rounding() {
    let err = eval_vm_err("(async/timeout -0.4 (async/resolved :ready))");
    assert!(
        err.contains("non-negative"),
        "expected non-negative duration error, got: {err}"
    );
}
```

- [ ] **Step 3: Keep the capacity test panic-sensitive**

Retain `channel_rejects_unrepresentable_capacity_without_panicking`. The helper
must return the language error; do not wrap the evaluation in `catch_unwind`,
which would turn a Rust panic into an accepted result.

- [ ] **Step 4: Keep the finite tick-ceiling test separate from fairness**

Retain `scheduler_workload_beyond_tick_ceiling_completes` as a finite workload.
It may remain in-process because its Sema recursion terminates independently of
timer fairness.

- [ ] **Step 5: Keep the nested callback oracle exact**

Retain `nested_aggregate_callback_can_spawn_await_and_resume_parent` and its
expected `(5 23 203)` result. Task 03 will make it GREEN without nested
scheduler re-entry.

## Task 3: Put perpetual fairness behind a host watchdog

**Files:**

- Modify: `crates/sema/tests/vm_async_test.rs`
- Create: `crates/sema/tests/unified_runtime_watchdog_test.rs`

- [ ] **Step 1: Remove the in-process perpetual spinner test**

Delete `ready_spinner_does_not_starve_due_timer` from
`vm_async_test.rs`. Its replacement below exercises the same public behavior
without allowing an unfair scheduler to hang the test process.

- [ ] **Step 2: Add the reusable watchdog harness**

Create `crates/sema/tests/unified_runtime_watchdog_test.rs` with this harness:

```rust
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct TimedRun {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

fn run_sema_with_timeout(source: &str, timeout: Duration) -> TimedRun {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", "-e", source])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sema watchdog child");

    let started = Instant::now();
    let mut timed_out = false;
    loop {
        if child.try_wait().expect("poll sema watchdog child").is_some() {
            break;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            child.kill().expect("kill hung sema watchdog child");
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    let output = child
        .wait_with_output()
        .expect("collect sema watchdog output");
    TimedRun {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        timed_out,
    }
}
```

- [ ] **Step 3: Add the ready-storm/timer test**

Append:

```rust
#[test]
fn ready_spinner_does_not_starve_due_timer() {
    let run = run_sema_with_timeout(
        r#"
        (define spinner
          (async
            (let loop ()
              (async/sleep 0)
              (loop))))
        (define timer (async (async/sleep 1) :timer-fired))
        (define winner (async/race (list spinner timer)))
        (define cancelled-before-explicit-stop (async/cancelled? spinner))
        (async/cancel spinner)
        (println (list winner cancelled-before-explicit-stop))
        "#,
        Duration::from_secs(10),
    );

    assert!(!run.timed_out, "scheduler hung; stderr:\n{}", run.stderr);
    assert!(
        run.status.success(),
        "scheduler failed; stdout:\n{}\nstderr:\n{}",
        run.stdout,
        run.stderr
    );
    assert!(
        run.stdout.contains("(:timer-fired #f)"),
        "expected timer win without implicit race cancellation; stdout:\n{}",
        run.stdout
    );
}
```

- [ ] **Step 4: Run the watchdog test alone**

```bash
cargo test -p sema-lang --test unified_runtime_watchdog_test \
  ready_spinner_does_not_starve_due_timer -- --exact --nocapture
```

Expected on the legacy scheduler: RED by nonzero tick-ceiling failure or wrong
implicit cancellation, but it MUST finish within the host timeout. Expected
after Tasks 03–04: PASS.

## Task 4: Replace the inventory with an executable ledger

**Files:**

- Modify: `docs/internals/async-runtime-inventory.md`

- [ ] **Step 1: Add ledger rules at the top**

State that a row may be marked complete only when its target path, cancellation
class, context policy, tests, and removal status are all evidenced. Use these
status values exactly:

```text
LEGACY | ADAPTER | MIGRATED | SYNCHRONOUS-PROOF | REMOVED
```

- [ ] **Step 2: Inventory core and VM symbols**

Create one row per relevant symbol in:

```text
crates/sema-core/src/async_signal.rs
crates/sema-core/src/io_backend.rs
crates/sema-core/src/context.rs
crates/sema-core/src/value.rs
crates/sema-core/src/cycle.rs
crates/sema-core/src/mcp_cassette.rs
crates/sema-vm/src/scheduler.rs
crates/sema-vm/src/vm.rs
crates/sema-vm/src/debug.rs
crates/sema-eval/src/eval.rs
crates/sema-eval/src/debug_session.rs
crates/sema-eval/src/prelude.rs
crates/sema-io/src/lib.rs
```

Do not collapse an entire file into one row when it contains different wait or
context policies.

- [ ] **Step 3: Inventory standard-library waits and resources**

For each async branch, blocking call, resource close/drop path, and callback in
the stdlib modules named by the master specification, record:

- target wait family;
- `INTERRUPTIBLE`, `QUARANTINED-BOUNDED`, or `PROHIBITED` cancellation class;
- the concrete abort/close/kill/deadline mechanism;
- the Task 05 test file that proves it.

- [ ] **Step 4: Inventory task-local and integration state**

Record every relevant `thread_local!`, task capture/install callback, agent/stream
slab, workflow scope, MCP cassette/hook, output hook, OTel span stack, usage,
budget, cache, retry, and provider state. Classify each field as:

```text
INTERPRETER-SHARED | ROOT-SHARED | TASK-SNAPSHOT | TASK-PRIVATE |
SCOPE-SHARED | RESOURCE-OWNED | HOST-ADAPTER-ONLY
```

- [ ] **Step 5: Inventory every host and shipped asset**

Include CLI, REPL, embedding, DAP, LSP, notebook, workflow, MCP server,
`sema-wasm`, playground worker/client, vendored web runtime assets, and every
host-owned interpreter in tests.

- [ ] **Step 6: Prove inventory coverage with discovery commands**

Run and paste the sorted results into Task 01 evidence:

```bash
rg -n "IoHandle|IoPoll|YieldReason|Scheduler(Target|RunResult)|run_until_reentrant|call_run_scheduler|set_yield_signal|take_resume_value|in_async_context|io_block_on|block_on|thread_local!" \
  crates playground -g '*.rs' -g '*.js' -g '*.ts' | sort
rg -n "async/(spawn|await|run|all|race|timeout|sleep|cancel)|channel/(send|recv)|call_callback|eval_callback" \
  crates playground -g '*.rs' -g '*.sema' -g '*.js' -g '*.ts' | sort
```

Every production match must map to at least one ledger row.

## Task 5: Install the legacy-symbol baseline guard

**Files:**

- Create: `scripts/check-unified-runtime-legacy.sh`
- Create: `docs/plans/evidence/unified-cooperative-runtime/legacy-symbols.baseline`
- Modify: `crates/sema/tests/runtime_conformance_test.rs`

- [ ] **Step 1: Write the scanner**

The script must use `rg`, normalize paths relative to the repository root,
sort uniquely, exclude `target`, `.git`, plan/evidence prose, and generated
assets, and scan the exact legacy tokens listed under “Static removal and
boundary guards” in the master specification. It exits nonzero when current
output differs from the committed baseline and prints a unified diff.

- [ ] **Step 2: Generate the initial baseline**

```bash
chmod +x scripts/check-unified-runtime-legacy.sh
scripts/check-unified-runtime-legacy.sh --write-baseline
scripts/check-unified-runtime-legacy.sh
```

Expected: the first command writes a nonempty sorted baseline; the second exits
zero with no diff.

- [ ] **Step 3: Add a conformance test for the script**

Add a test that invokes the script from `CARGO_MANIFEST_DIR/../..` and asserts
successful status. Include stdout/stderr in the assertion message. Do not copy
the scanner logic into Rust.

- [ ] **Step 4: Run the conformance target**

```bash
cargo test -p sema-lang --test runtime_conformance_test -- --nocapture
```

Expected: PASS. Later tasks update the baseline only when a reviewed legacy
match disappears; Task 08 requires the production baseline to become empty.

## Task 6: Record durable characterization evidence

**Files:**

- Create: `docs/plans/evidence/unified-cooperative-runtime/task-01.md`

- [ ] **Step 1: Run every new test individually**

Record command, status, elapsed time, and the exact language-level mismatch for
each test. Classify it `RED-EXPECTED` or `GREEN-BASELINE`.

- [ ] **Step 2: Run the complete affected targets**

```bash
cargo test -p sema-lang --test vm_async_test -- --nocapture
cargo test -p sema-lang --test unified_runtime_watchdog_test -- --nocapture
cargo test -p sema-lang --test runtime_conformance_test -- --nocapture
```

Expected: characterization targets may be RED only for enumerated approved
runtime defects. The conformance target must be GREEN. No hang is acceptable.

- [ ] **Step 3: Run formatting and diff checks**

```bash
cargo fmt --all -- --check
git diff --check
```

If workspace formatting is RED before this task, record the exact unrelated
paths and run `rustfmt --edition 2021 --check` on both modified Rust test files.

- [ ] **Step 4: Write the handoff table**

The evidence file must list every RED test and the task expected to turn it
GREEN:

```text
captured mutation and nested callback -> Task 03
observational all/race and timeout semantics -> Task 04
duration and capacity validation -> Task 04
finite yields and fairness watchdog -> Task 03
legacy source matches -> Tasks 02–08
```

## Task 7: Independent review and commit

- [ ] **Step 1: Dispatch an independent test-oracle review**

The reviewer checks only:

- no test asserts implicit ownership of a supplied promise;
- every perpetual workload has an external watchdog;
- RED tests fail for the intended public behavior;
- the inventory covers every discovery match;
- the baseline scanner cannot silently exclude a production directory.

Write the review to
`docs/plans/reviews/unified-cooperative-runtime/task-01.md`.

- [ ] **Step 2: Fix every finding and rerun affected commands**

Add a regression oracle before fixing any discovered test-harness bug. The
implementer may not close their own review finding.

- [ ] **Step 3: Commit the accepted layer**

```bash
git add \
  crates/sema/tests/vm_async_test.rs \
  crates/sema/tests/unified_runtime_watchdog_test.rs \
  crates/sema/tests/runtime_conformance_test.rs \
  scripts/check-unified-runtime-legacy.sh \
  docs/internals/async-runtime-inventory.md \
  docs/plans/evidence/unified-cooperative-runtime \
  docs/plans/reviews/unified-cooperative-runtime/task-01.md
git commit -m "test(runtime): lock unified runtime contracts"
```

## Completion criteria

- The invalid `async/all` and `async/race` cancellation tests are gone.
- Observational loser/sibling tests are enabled and RED for the intended legacy
  behavior.
- No in-process test can spin forever after the tick ceiling is removed.
- The inventory maps every current discovery match to a target layer and policy.
- The source baseline guard is GREEN and reviewable.
- Task 01 evidence and independent review are committed.
- No production behavior changed.
