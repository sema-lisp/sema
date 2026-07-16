# Task 10: Six-Round Independent Review and Bug-Hunting Campaign Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Subject the correctness-complete runtime to six independent specialist
reviews, remediate every finding with regression coverage, and finish with a
clean full-diff/maintainability review before any profiling begins.

**Architecture:** Each round receives the master specification, horizontal task
plans/evidence, final inventory, source guards, baseline-to-candidate diff, and
test commands, but not another reviewer’s conclusions until its own report is
written. Findings use stable IDs. A remediation agent reproduces/tests/fixes;
an independent reviewer verifies closure. Later rounds review the remediated
candidate, not the original snapshot.

**Tech Stack:** Code review, static/source inspection, deterministic tests,
fault injection, browser/package inspection, Markdown finding ledgers.

## Execution contract

- **Status:** Ready only after Task 09 is accepted and committed.
- **Dependencies:** Green full/adversarial/fuzz-seed/leak/browser/package gates,
  stable seed corpus, final matrices/inventory, and clean Task 09 ledger.
- **Immutable inputs:** Master six review scopes, no-waiver/remediation rules,
  approved architecture/language/resource/context/host contracts.
- **Exact start state:** Clean worktree; latest commit subject is
  `test(runtime): add adversarial runtime campaign`; candidate SHA and baseline
  ancestry are recorded before Round 01.
- **Parallel work:** The six rounds are sequential. Within a round, read-only
  source inspection and test reproduction may be delegated, but one independent
  reviewer owns the report/verdict. Remediation and closure use different agents;
  no later round starts before prior blockers close and full gates rerun.

## Global constraints

- Tasks 01–09 must be accepted; all functional, adversarial, leak, fuzz, browser,
  shipped-package, and deletion gates must be GREEN.
- Six rounds are mandatory and sequential at their checkpoints. They may not be
  collapsed into one general review or performed by the implementation agent.
- A reviewer first writes evidence and findings without reading prior round
  conclusions, then may read them to detect coverage gaps/recurrence.
- Correctness, safety, security, leak, determinism, shutdown, and maintainability
  blockers cannot be waived. Other findings require explicit evidence-based
  disposition and independent acceptance.
- Every implementation defect is reduced to a failing regression before its fix.
- A finding is closed only by a reviewer other than the agent that implemented
  the fix. “Tests pass” without reproducing the original defect is insufficient.
- Any fix reruns its scoped gate and the complete correctness gate before the
  next review round.
- Review must not tune or benchmark performance. Obvious asymptotic pathologies
  are correctness/maintainability findings; measurement waits for Task 11.

---

## Files and responsibilities

**Create**

- `docs/plans/evidence/unified-cooperative-runtime/task-10-candidate.md` —
  baseline SHA, candidate SHA per round, environment, and green-gate hashes.
- `docs/plans/reviews/unified-cooperative-runtime/round-01-architecture.md`.
- `docs/plans/reviews/unified-cooperative-runtime/round-02-cancellation.md`.
- `docs/plans/reviews/unified-cooperative-runtime/round-03-fairness.md`.
- `docs/plans/reviews/unified-cooperative-runtime/round-04-context-orchestration.md`.
- `docs/plans/reviews/unified-cooperative-runtime/round-05-hosts-wasm-package.md`.
- `docs/plans/reviews/unified-cooperative-runtime/round-06-final-diff.md`.
- `docs/plans/reviews/unified-cooperative-runtime/task-10-findings.md` — unified
  finding/disposition/regression/verification ledger.
- `docs/plans/evidence/unified-cooperative-runtime/task-10.md` — commands/results.

**Modify only when a finding proves it necessary**

- Runtime production/tests/docs/scripts from Tasks 02–09.
- The relevant specialist report and unified finding ledger.
- Master specification only if the user explicitly approves a contract change;
  ambiguity alone is first resolved against approved semantics and tests.

## Common review packet and report format

Before Round 01, record:

```bash
git rev-parse 3f111e83
git rev-parse HEAD
git diff --stat 3f111e83...HEAD
git log --oneline 3f111e83..HEAD
scripts/check-unified-runtime-legacy.sh --check
```

If any production code landed after `3f111e83` but before Task 02 began, replace
the baseline with the last pre-rewrite production commit and explain it in the
candidate evidence. Task 11 consumes the same baseline.

Each report contains:

- reviewer identity and independent scope;
- reviewed candidate SHA and baseline range;
- files/interfaces/tests inspected;
- concrete invariants traced end to end;
- adversarial commands or reproductions executed;
- findings ordered by severity with stable ID, path/line evidence, consequence,
  minimal reproduction, and required acceptance test;
- coverage gaps and a final `clean` or `findings-open` verdict.

IDs are `UR-R01-###` through `UR-R06-###`. The unified ledger records owner,
regression test, fix commit, scoped/full gate result, closure reviewer, and
closure evidence.

## Task 1: Freeze and independently reproduce the green candidate

- [ ] **Step 1: Run the complete pre-review gate**

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
```

Expected: all GREEN. Record SHA, command, exit status, and durable log digest.

- [ ] **Step 2: Build a review coverage map**

Map every master-spec section, inventory category, resource/context/host matrix,
and Task 09 adversarial category to one or more review rounds. Every item has a
named primary round; cross-round overlap is encouraged.

## Task 2: Round 01 — architecture, state machines, types, and inventory

- [ ] **Step 1: Assign an architecture reviewer**

Review Runtime/Root/Task/Scope/Wait/Promise/Channel states; checked identities;
settlement sequencing; `Value`/continuation tracing; send boundary; interpreter
ownership; VM-only evaluation; root/global semantics; inventory completeness;
and zero-legacy guard coverage.

- [ ] **Step 2: Require end-to-end traces**

Trace successful root, failed root, detached task across roots, native callback
suspension, external completion, owned failure cleanup, and interpreter drop.
Attempt illegal state edges, ID reuse, send-boundary leakage, untraced cycles,
and a hidden second scheduler.

- [ ] **Step 3: Write `round-01-architecture.md` before reading prior reviews**

- [ ] **Step 4: Remediate, independently close, and run full gate**

No Round 02 starts with an open Round 01 blocker.

## Task 3: Round 02 — cancellation, ownership, resources, and shutdown

- [ ] **Step 1: Assign a lifecycle reviewer**

Review cancellation ancestry vs observation vs lifetime ownership; all public
observer/owner APIs; primary/suppressed outcomes; resource matrix truthfulness;
interrupt hooks; finite-work/deadline proofs; quarantine transfer; process/PTY/
socket/database/stream/server cleanup; late completion; and shutdown ordering.

- [ ] **Step 2: Inject cancellation at every lifecycle boundary**

Use Task 09 harness plus manual source tracing. Attempt double cancel/close,
cancel-vs-complete, lost process reap, stuck quarantine, producer cancellation by
observation, owned child escape, and shutdown result publication before cleanup.

- [ ] **Step 3: Write `round-02-cancellation.md`, remediate, close, full-gate**

No Round 03 starts with an open Round 02 blocker.

## Task 4: Round 03 — fairness, determinism, timers, channels, interleavings

- [ ] **Step 1: Assign a scheduler reviewer**

Review root rotation/per-root FIFO, quantum and drive bounds, completion/timer
drain caps, wake deduplication, fake/real clock boundary, settlement ordering,
channel FIFO/backpressure/close, origin barriers, debug pause, and watchdogs.

- [ ] **Step 2: Challenge hostile scheduling cases**

Reproduce more-than-million yields, perpetual-ready plus timer/completion,
pre-settled reverse races, same-turn settlements, multiple roots with uneven
load, zero-duration storms, cancelled channel waiters, and seed/model shrinkers.

- [ ] **Step 3: Write `round-03-fairness.md`, remediate, close, full-gate**

No Round 04 starts with an open Round 03 blocker.

## Task 5: Round 04 — context, sandbox, LLM, agents, workflow, MCP, tracing

- [ ] **Step 1: Assign a context/orchestration reviewer**

Review every context-matrix row and remaining TLS rationale; sandbox narrowing;
output ownership; tracing lineage; FakeProvider request/tool correlation; cache/
usage/budget accounting; retry/stream cursors; agent tool scopes; workflow
journals/resume; MCP handles/requests/cassettes; and cycle tracing.

- [ ] **Step 2: Force alternating scopes and failures**

Attempt sibling/root leakage, sandbox widening, wrong output/usage/span/request,
cache recharge, retry after cancel, stream chunk after cancel, agent owned-child
escape, workflow journal inconsistency, and MCP reconnect/shutdown races.

- [ ] **Step 3: Write `round-04-context-orchestration.md`, remediate, close,
  full-gate**

No Round 05 starts with an open Round 04 blocker.

## Task 6: Round 05 — native hosts, debugger/tooling, WASM, browser, package

- [ ] **Step 1: Assign a host/browser/package reviewer**

Review the host matrix and common API; native parking/wakeup/signals; CLI/REPL;
DAP/LSP; notebook/MCP/workflow services; exact request/root cancellation; output
tagging; reset/drop; WASM Promise table; macrotask drive; fetch/timers; debugger;
worker protocol; generated assets; and `.crate` embedding.

- [ ] **Step 2: Run browser/package attacks**

Attempt two simultaneous roots, stop wrong root, output crossing, side-effect
replay, microtask starvation, browser input/render loss, disconnect/shutdown,
missing/stray generated asset, source-tree dependency, and sync/Atomics fallback.

- [ ] **Step 3: Write `round-05-hosts-wasm-package.md`, remediate, close,
  full-gate**

No Round 06 starts with an open Round 05 blocker.

## Task 7: Round 06 — complete final diff, documentation, and maintainability

- [ ] **Step 1: Freeze the remediated candidate SHA**

Update candidate evidence and give the reviewer the full baseline diff plus all
round reports only after they complete their first-pass diff review.

- [ ] **Step 2: Assign a final generalist reviewer**

Review every changed production/test/doc/build/generated file; naming and API
coherence; duplicated state machines; oversized but poorly factored modules;
opaque callbacks; unsafe/unchecked identity conversion; comments describing old
behavior; public compatibility; error quality; test oracle strength; ignored/
flaky tests; scanner blind spots; inventory closure; docs/examples accuracy;
and package/release gates.

- [ ] **Step 3: Search explicitly for incomplete work**

```bash
rg -n -i 'TODO|FIXME|HACK|temporary|compat(ibility)? bridge|legacy runtime|follow.?up|implement later|unreachable!|todo!|unimplemented!' \
  crates scripts jake docs website examples playground
git diff --check 3f111e83...HEAD
```

Every match is inspected; historical/intentional matches get exact disposition.

- [ ] **Step 4: Write `round-06-final-diff.md`, remediate, and independently
  close every finding**

- [ ] **Step 5: Rerun the complete gate from Task 1**

Expected: all GREEN on the final remediated SHA.

## Task 8: Close the campaign and commit reports/fixes

- [ ] **Step 1: Audit the unified ledger**

Every finding ID appears once, has severity/disposition, and if fixed links a
regression, fix, scoped/full result, and independent closure. No correctness,
safety, leak, determinism, security, shutdown, or maintainability blocker is
open or waived.

- [ ] **Step 2: Record the Task 11 handoff**

Record final candidate SHA, confirmed benchmark baseline SHA, compiler/toolchain,
complete-gate result, and statement that no profiling has yet influenced the
implementation.

- [ ] **Step 3: Commit**

```bash
git add crates scripts jake docs website examples playground packages \
  Cargo.toml Cargo.lock
git commit -m "review(runtime): complete independent correctness campaign"
```

If a round produced no source change, its report/evidence is still committed.

## Completion criteria

- Six separate specialist/generalist rounds are complete on successively
  remediated candidates.
- Every master contract and migration surface maps to a review round.
- Every finding has stable evidence and independent disposition/closure.
- No correctness, safety, security, leak, determinism, shutdown, or
  maintainability blocker remains.
- The complete functional/adversarial/browser/package gate is GREEN on the
  final candidate SHA.
- Final candidate and pre-rewrite baseline SHAs are frozen for Task 11.
- No profiling or performance tuning has occurred.
