# Task 09: Integrated Adversarial, Model, Fuzz, Stress, and Leak Verification Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Attack the complete runtime across layer boundaries with deterministic
fault injection, reference models, seeded randomized scenarios, fuzzing, hostile
process watchdogs, and repeated leak/shutdown stress until all discovered defects
have minimized regression tests.

**Architecture:** Tests operate through public host/runtime APIs plus narrowly
scoped test instrumentation. A small reference model predicts task/promise/wait/
scope/channel state and settlement order. Scenario generators vary host actions,
completion order, cancellation boundaries, and resource faults; they do not
replace required round-robin scheduling. Infinite or perpetual workloads always
run under an external watchdog.

**Tech Stack:** Rust tests, `proptest`, fake clock/executor/provider/server,
libFuzzer, grammar fuzzing, shell subprocess watchdogs, Playwright.

## Execution contract

- **Status:** Ready only after Task 08 is accepted and committed.
- **Dependencies:** Final production paths, empty legacy guard, current docs and
  shipped assets, accepted per-layer evidence/reviews.
- **Immutable inputs:** Master adversarial matrix, remediation loop, process-
  watchdog rule, deterministic fairness/ownership/context/resource contracts.
- **Exact start state:** Clean worktree; latest commit subject is
  `docs(runtime): remove legacy paths and document final runtime`; no expected
  RED test or runtime migration adapter remains.
- **Parallel work:** Model/harness, watchdog scenarios, resource fixtures,
  context/orchestration scenarios, and bounded fuzz target may be authored in
  parallel in disjoint files. One owner controls shared harness/snapshot APIs,
  seed corpus, finding ledger, and production fixes. A defect fix serializes its
  reproduction/implementation/review before campaigns resume.

## Global constraints

- Tasks 01–08 must be accepted, deletion guard empty, docs/assets/package gates
  GREEN, and no expected RED test remain.
- This layer adds tests, test instrumentation, scripts, and defect fixes only.
  It does not redesign semantics without first updating the master specification.
- Every failure gets a stable ID, minimized deterministic reproduction, failing
  regression, production fix, complete affected-suite run, and independent
  verification.
- Random failures print seed plus serialized action trace before assertion/panic.
  Promoted regression seeds are committed.
- No in-process infinite loop, socket stall, server, child, or browser test may
  rely on the test runner’s eventual timeout.
- Timing assertions use conservative upper bounds only to detect hangs; ordering
  assertions use fake time/events wherever possible.
- This is correctness and leak testing, not profiling or performance comparison.

---

## Files and responsibilities

**Create**

- `crates/sema/tests/runtime_model_test.rs` — reference-state-machine properties.
- `crates/sema/tests/runtime_adversarial_test.rs` — deterministic boundary matrix.
- `crates/sema/tests/runtime_leak_test.rs` — cycles, churn, shutdown plateaus.
- `crates/sema/tests/runtime_stress_test.rs` — committed seeded scenario runner.
- `crates/sema/tests/common/runtime_harness.rs` — fake clock/executor/server,
  action trace, snapshots, and bounded-drive helpers.
- `crates/sema-eval/fuzz/fuzz_targets/fuzz_runtime.rs` — byte-to-action runtime
  scenario fuzz target.
- `scripts/test-unified-runtime-stress.sh` — debug/release subprocess campaign.
- `scripts/test-unified-runtime-watchdogs.sh` — perpetual/hostile workload suite.
- `docs/plans/evidence/unified-cooperative-runtime/runtime-seeds.txt` — committed
  deterministic seed corpus, one decimal `u64` per line.
- `docs/plans/evidence/unified-cooperative-runtime/task-09-findings.md` — stable
  finding ledger and regression mapping.
- `docs/plans/evidence/unified-cooperative-runtime/task-09.md` — commands/results.
- `docs/plans/reviews/unified-cooperative-runtime/task-09.md` — independent
  adversarial review.

**Modify**

- `Cargo.toml`/`crates/sema/Cargo.toml` — `proptest` dev dependency only.
- `crates/sema-eval/fuzz/Cargo.toml` — register `fuzz_runtime`.
- `crates/sema-vm/src/runtime/*` — test-only snapshot/fault seams; production
  defect fixes require regressions.
- `crates/sema-io/src/fault.rs` and FakeProvider/test servers — deterministic
  injected boundary control.
- `fuzz/grammar-fuzz.sema` — generate bounded concurrency/channel/ownership
  programs with computable or model-derived outcomes.
- `jake/fuzz.jake` and `jake/test.jake` — named runtime fuzz/stress gates.
- Any production file required by a proven defect fix.

## Exact test instrumentation

Under `#[cfg(test)]` or a non-shipping test-support feature, expose:

```rust
pub struct RuntimeSnapshot {
    pub roots_by_state: BTreeMap<&'static str, usize>,
    pub tasks_by_state: BTreeMap<&'static str, usize>,
    pub waits: usize,
    pub timers: usize,
    pub observations: usize,
    pub owned_scopes: usize,
    pub cleanup_entries: usize,
    pub channels: usize,
    pub late_completions: u64,
    pub next_settlement_sequence: SettlementSeq,
}

pub enum ScenarioAction {
    SubmitRoot(RootProgram),
    Drive(DriveBudget),
    AdvanceClock(u64),
    Deliver(CompletionToken, DeliveryMutation),
    CancelRoot(RootSlot),
    CancelPromise(PromiseSlot),
    DropHandle(HandleSlot),
    DebugPause,
    DebugResume,
    Disconnect(ResourceSlot),
    Shutdown,
}
```

`DeliveryMutation` includes exact, duplicate, stale generation, wrong runtime,
wrong operation, wrong payload kind, and reordered. Snapshots expose counts and
IDs only; they do not permit mutation or leak runtime `Value`s across threads.

The reference model implements the approved transition/ownership rules without
executing Sema. After every action, compare observable root/promise/channel
outcomes, settlement ordering, and live-count invariants. It need not model VM
values beyond symbolic tokens.

## Task 1: Build the deterministic harness and reference model

**Files:** common harness, model test, dev dependencies

- [ ] **Step 1: Write model self-tests**

Hand-author action traces for return, fail, cancel, observation timeout,
observational race, owned race cleanup, channel block/close, detached survival,
root cancellation, and shutdown. Assert expected symbolic state after every
action.

- [ ] **Step 2: Implement harness adapters**

All driving is bounded. Fake external operations allocate real wait/generation/
operation identities and route completions through the production inbox.

- [ ] **Step 3: Add property generation**

Generate 1–4 roots, 0–32 tasks, bounded channel capacity 1–8, 0–64 actions, and
valid/invalid completion delivery. Use `proptest` shrinking; emit the final
action trace in a copy-paste Rust literal.

- [ ] **Step 4: Run**

```bash
PROPTEST_CASES=10000 cargo test -p sema-lang --test runtime_model_test
```

Expected: model self-tests and 10,000 generated cases pass.

## Task 2: Exhaust cancellation and completion boundaries

**Files:** adversarial test, harness, runtime wait/cleanup fixes if proven

- [ ] **Step 1: Add one named test per boundary**

Cancel immediately before/after native call, quantum expiry, wait allocation,
registration, dispatch, wake enqueue, decoder, task settlement, sequence
assignment, observer wake, scope primary selection, child cancellation,
quarantine transfer, reaping, and root result publication.

- [ ] **Step 2: Cross each boundary with completion faults**

For every wait kind inject duplicate, stale, late, reordered, wrong-runtime,
wrong-operation, wrong-kind, and wrong-generation completions. Assert no second
resume, no unrelated wake, no outcome change, and correct late counter.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test runtime_adversarial_test -- cancellation_boundary
cargo test -p sema-lang --test runtime_adversarial_test -- completion_fault
```

Expected: all named boundary tests pass with zero live wait/cleanup entries.

## Task 3: Attack roots, fairness, races, IDs, and churn

**Files:** adversarial/stress tests, watchdog script

- [ ] **Step 1: Enumerate root/order cases**

Start/mutate globals/output/return/fail/cancel roots A/B/C in all pairwise order
classes. Assert last-scheduled global write, isolated output/context/result, and
round-robin progress. Include already-settled/duplicate promise races and same-
turn settlements ordered by `SettlementSeq`.

- [ ] **Step 2: Exercise exhaustion seams**

Inject ID/generation counters at `MAX-1`, allocate through exhaustion, and prove
structured failure without wrap or alias. Repeat for settlement sequence.

- [ ] **Step 3: Run finite extreme churn**

Test more than 1,000,000 finite yields, 100,000 task create/settle/reap cycles,
and repeated handle drop/re-observation. These run as subprocess cases with a
60-second external watchdog and print final counts.

- [ ] **Step 4: Run perpetual fairness watchdogs**

Keep ready CPU work active beside timer, channel, and external completion. The
parent process requires each finite event within its conservative deadline, then
cancels/shuts down the child. The perpetual workload never runs in the test
process itself.

```bash
scripts/test-unified-runtime-watchdogs.sh
```

Expected: all child scenarios report their marker and clean shutdown before the
watchdog; no tick ceiling is involved.

## Task 4: Attack channels, resources, callbacks, and shutdown

**Files:** adversarial/leak tests and fake resources

- [ ] **Step 1: Generate channel action sequences**

Model FIFO, capacity/backpressure, close, cancelled sender/receiver, multiple
producers/consumers/roots, close-vs-send/recv, and values participating in cycles.

- [ ] **Step 2: Inject real-resource failures locally**

Slow consumer, disconnect, partial read/write, broken pipe, killed process,
process ignoring first termination, PTY close, listener cancellation, database
lock/interrupt, server handler cancellation, and shutdown during each phase.

- [ ] **Step 3: Exercise nested continuation chains**

Native → Sema callback → spawn → await → sleep → I/O → callback → fail/cancel,
with parent captured-cell mutation and task context assertions before/after each
suspension.

- [ ] **Step 4: Run**

```bash
cargo test -p sema-lang --test runtime_adversarial_test -- channel
cargo test -p sema-lang --test runtime_adversarial_test -- resource
cargo test -p sema-lang --test runtime_adversarial_test -- nested_callback
```

Expected: all pass; OS/runtime snapshots return to baseline.

## Task 5: Attack task context and orchestration

**Files:** adversarial tests, FakeProvider, workflow/MCP fake transports

- [ ] **Step 1: Randomize suspension in every dynamic scope**

Alternate siblings/roots inside user, hidden, parameter, file/module, sandbox,
output, trace, LLM config, usage/budget, retry/stream cursor, workflow step, MCP
request, and debugger scopes. Attempt sandbox widening and output/usage/trace
misattribution explicitly.

- [ ] **Step 2: Inject orchestration failures**

Provider error, chunk interruption, retry-timeout race, cache hit/miss, budget
exhaustion, tool failure, MCP queue/disconnect/reconnect/cassette replay miss,
workflow leaf failure/cancellation/resume, and parent shutdown.

- [ ] **Step 3: Run**

```bash
cargo test -p sema-lang --test runtime_adversarial_test -- task_context
cargo test -p sema-lang --test runtime_adversarial_test -- orchestration
cargo test -p sema-lang --test llm_runtime_test
cargo test -p sema-lang --test orchestration_runtime_test
```

Expected: no leakage or accounting/lineage mismatch and zero owned work after
cleanup.

## Task 6: Cycle, interpreter-drop, native/browser stress

**Files:** leak/stress tests and scripts

- [ ] **Step 1: Build cycle shapes**

Promise↔closure, task-context extension↔promise, channel-buffer↔closure,
continuation↔value, captured cell↔closure, module cache↔task, and combinations
held across suspension/cancellation. Drop all external roots and force GC.

- [ ] **Step 2: Repeat interpreter shutdown**

Create/drop at least 1,000 interpreters containing live roots, detached tasks,
timers, channel waits, external jobs, owned scopes, debug stop, and cycles.
Assert stable candidate/live resource counts after warm-up rather than timing.

- [ ] **Step 3: Run debug and release seed corpus**

```bash
scripts/test-unified-runtime-stress.sh --profile debug \
  --seeds docs/plans/evidence/unified-cooperative-runtime/runtime-seeds.txt
scripts/test-unified-runtime-stress.sh --profile release \
  --seeds docs/plans/evidence/unified-cooperative-runtime/runtime-seeds.txt
```

- [ ] **Step 4: Run browser heartbeat/input/cancel stress**

```bash
jake test.playground-e2e
jake test.web-e2e
```

Expected: all stress scenarios pass; browser heartbeat/input events occur while
roots are live; no retained-count upward staircase remains.

## Task 7: Fuzz runtime action and grammar surfaces

- [ ] **Step 1: Add `fuzz_runtime` target**

Decode bytes into bounded `ScenarioAction`s, execute with fake time/resources,
and assert model/snapshot invariants after every action. Cap roots/tasks/actions/
payload size in the target so each input terminates.

- [ ] **Step 2: Extend grammar fuzzer**

Generate bounded owned/observational async forms, channels, cancellation, and
captured mutation only where the oracle can compute an outcome. Emit seed and
program for mismatches.

- [ ] **Step 3: Run fixed campaigns**

```bash
cargo +nightly fuzz run fuzz_runtime --fuzz-dir crates/sema-eval/fuzz -- \
  -max_total_time=300 -timeout=10
scripts/grammar-fuzz.sh check -n 50000 -d 6 -s 2026071301
```

Expected: no crash, hang, model mismatch, or invariant failure. Promote any
finding input/seed before fixing it.

## Task 8: Remediation loop, full gates, review, and commit

- [ ] **Step 1: Maintain the finding ledger**

IDs use `UR-T09-F###`. Each row records seed/action trace, minimized repro,
regression test, fix commit, affected/full commands, independent verifier, and
status. No correctness/leak/nondeterminism finding is waived.

- [ ] **Step 2: Run the full correctness gate after the ledger is clean**

```bash
cargo test --workspace
jake examples
jake smoke-bytecode
jake lint
jake docs-check
scripts/check-unified-runtime-legacy.sh --check
scripts/test-unified-runtime-watchdogs.sh
scripts/test-unified-runtime-stress.sh --profile debug \
  --seeds docs/plans/evidence/unified-cooperative-runtime/runtime-seeds.txt
scripts/test-unified-runtime-stress.sh --profile release \
  --seeds docs/plans/evidence/unified-cooperative-runtime/runtime-seeds.txt
jake test.playground-e2e
jake test.web-e2e
scripts/test-packaged-sema-web.sh
git diff --check
```

Expected: all GREEN.

- [ ] **Step 3: Assign independent adversarial review**

Finding IDs use `UR-T09-R###`. Reviewer audits model independence, generator
coverage/shrinking, watchdog process ownership, snapshot completeness, fixed
seed durability, and every remediated finding’s regression and fix.

- [ ] **Step 4: Fix review findings and repeat the full gate**

- [ ] **Step 5: Commit the accepted layer**

```bash
git status --short
# Stage every exact Task 09 path listed in evidence, including production fixes
# in crates not anticipated when this plan was written. Do not use `git add -A`.
git add <reviewed Task 09 paths from the evidence manifest>
git diff --cached --name-only
git diff --exit-code
test -z "$(git ls-files --others --exclude-standard)"
git commit -m "test(runtime): add adversarial runtime campaign"
```

The staged-path list must cover every Task 09 change and no unrelated user work.
The clean-diff and untracked-file checks are hard gates for Task 10's clean start.

## Completion criteria

- The complete required adversarial matrix has named deterministic coverage.
- Model/property runs, seed corpus, fuzz campaigns, watchdogs, and debug/release
  stress pass.
- More than one million yields complete without a scheduler tick ceiling.
- All perpetual work is externally watchdog-owned.
- Completion faults, cancellation boundaries, context/orchestration leakage,
  resource failure, cycles, and shutdown return to invariant state.
- Every discovered defect has a minimized committed regression and independent
  verification.
- No profiling or performance tuning occurred.
