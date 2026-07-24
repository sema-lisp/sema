# Post-Migration Doc Reconciliation + Remaining-Slices Roadmap

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring every migration ledger (`docs/deferred.md`, the P8 release-readiness report, migration-progress) into agreement with the post-purge code, archive completed plan docs, and sequence the remaining engineering slices (P6-1 host API → P6-3 wasm, SRV-1, Step-G) so future sessions start from a trustworthy map.

**Architecture:** Phase 0 (Tasks 1–5) is executable now: pure documentation edits, each gated by `jake docs-check` + the runtime conformance suite. Phase 1+ (the Roadmap section) sequences the remaining engineering slices; each slice already has an authoritative design section and MUST get its own implementation plan at execution time — this document deliberately does not duplicate those designs.

**Tech Stack:** Markdown edits, `git mv`, `rg` verification, `cargo nextest run`, `jake docs-check`.

## Global Constraints

- Branch: `codex/unified-async-runtime`, worktree `/Users/helge/code/sema/sema/.worktrees/unified-async-runtime`. All commands run from the worktree root.
- Test runner is **nextest**: `cargo nextest run -p sema-lang --test runtime_conformance_test` (per AGENTS.md).
- Comments/docs describe the code **as it stands** (AGENTS.md style rule) — resolved entries keep a short RESOLVED stamp with date + commit, not change-narration prose.
- Never `git stash`; never edit generated files.
- Do not touch shipped runtime code in Phase 0 — doc-only commits (plus `git mv` of plan files).
- Line numbers below were verified 2026-07-16 at commit `1caba181`; re-verify with the quoted grep anchors before editing (the file may have shifted).

---

### Task 1: Sweep `docs/deferred.md` — mark resolved entries RESOLVED, rewrite the stale LEGACY-SCHEDULER section

**Files:**
- Modify: `docs/deferred.md` (section `## Unified runtime migration — deferred`, currently lines 561–790)

**Interfaces:**
- Produces: a `deferred.md` whose every claim matches post-purge source. Later tasks and future agents treat it as ground truth.

Verified ground truth to encode (re-check before editing):
- `scheduler.rs`, `LegacyPromise`, `LegacyChannel`, `IN_ASYNC_CONTEXT`, `init_scheduler`, `DebugCoopResume`, `COOP_TASK_STOP`: **0 hits** in non-test source (P5 purge, commit `a1862f67`).
- `AwaitIo`/`IoHandle`/`poll_io_waits`/`io_park`/`notify_io_complete`: deleted (P2 "AwaitIo funeral", commit `04257fcd`); remaining hits are comments only.
- `YieldReason` has a **single variant `Sleep(u64)`** (`crates/sema-core/src/async_signal.rs:22-25`), carried via `VmExecResult::AsyncYield` — the ctx-less `async/sleep` value ABI is the sole survivor of the TLS yield bridge.
- `event_select_yields_to_sibling_in_async_context` (`crates/sema/tests/vm_async_test.rs:1509`) is **not** `#[ignore]`d and passes (Step F landed, commit `1cabd457`/`e6b7004b`).
- C2 eager cancellation landed (commit `d385494e`); request-time delivery in `crates/sema-vm/src/runtime/state.rs` (~line 851).
- Still genuinely open: SRV-1, Step-G nested-`eval` (`vm_integration_test.rs:1775` `#[ignore]`), multimethod-async dispatch, ASYNC-2 (cross-sibling stepping), P6-1/P6-3.

- [ ] **Step 1: Fix the section header context (lines 563–569).** It still says the legacy scheduler "remains only in code paths not yet migrated" and "Two async tests are `#[ignore]`d". Replace that paragraph with:

```markdown
**Context (updated 2026-07-16, post-P5 purge).** Every eval entry point drives
the unified cooperative `Runtime` — the sole async engine for CLI, MCP,
notebook, REPL, DAP, wasm, and tests. The legacy thread-local scheduler is
DELETED (P5, commit a1862f67); `scripts/check-unified-runtime-legacy.sh
--check` enforces zero reintroduction. One async test remains `#[ignore]`d
pending Step G (below).
```

- [ ] **Step 2: Mark the `event_select` bullet (lines 616–624) resolved.** Prepend to that bullet:

```markdown
- **RESOLVED (2026-07-16, Step F / F2 conversion — commits e6b7004b, 1cabd457).**
  `event_select_yields_to_sibling_in_async_context` is un-ignored and green:
  `event/select` now suspends via `WaitKind::External` and yields to siblings
  before parking. Historical description follows.
```
  (Keep the original text below the stamp as history; convert its present-tense claims — "is `#[ignore]`d", "still on the legacy AwaitIo bridge" — to past tense.)

- [ ] **Step 3: Mark `### F2-RESIDUAL` (line 683) RESOLVED.** Retitle to `### F2-RESIDUAL — external I/O on the AwaitIo bridge (RESOLVED 2026-07-16)` and prepend:

```markdown
**RESOLVED 2026-07-16.** All three sub-gaps closed and the AwaitIo bridge is
deleted (P2 "AwaitIo funeral", commit 04257fcd):
- **F2-RESIDUAL-1** — `ResourceGate` runtime primitive (`WaitKind::ResourceSlot`,
  FIFO acquire-queue) + the shared `checkout_external` helper; all six checkout
  modules (proc, sqlite, kv, serial, pty, stream) converted (commits e4399de3,
  0485e486, d385494e).
- **F2-RESIDUAL-2** — no streaming primitive was needed: `ws` restructured onto
  checkout + async-tier `recv` (commit 869366cd, per the P2 plan amendment).
- **F2-RESIDUAL-3** — the executor async tier is a real reactor
  (`ProcessIoExecutor`, tokio spawn + AbortHandle drop-on-cancel, P0 commit
  e530fc06); sema-llm's `interruptible_async` path runs on it.
The historical description below is retained for the record.
```
  (Then edit the final paragraph of the section, lines 712–714 — "…stay — they are the runtime's I/O-offload transport…" — to past tense: they were the transport until P2 deleted them.)

- [ ] **Step 4: Mark `### ASYNC-TIMEOUT-CANCEL-1` (line 716) RESOLVED.** Retitle with `(RESOLVED 2026-07-16)` and prepend:

```markdown
**RESOLVED 2026-07-16 (decision C2, commit d385494e).** Cancellation recorded on
an External/IO-parked task now runs the wait teardown at request time
(deregister → abort hook once → cancelled settlement), so a sibling
`async/timeout` promptly aborts the child's in-flight executor job; the
drive-scan drain is a backstop only. The UCR-3 rendezvous-cancel value-drop was
fixed in the same pass.
```

- [ ] **Step 5: Rewrite `### LEGACY-SCHEDULER retained` (lines 729–741).** Retitle to `### LEGACY-SCHEDULER — purged (RESOLVED 2026-07-16, P5)` and replace the first paragraph (the purge is no longer "BLOCKED"; scheduler.rs / LegacyPromise / LegacyChannel / IN_ASYNC_CONTEXT are deleted) with:

```markdown
**RESOLVED 2026-07-16 (P5 purge, commit a1862f67).** `scheduler.rs`,
`LegacyPromise`/`LegacyChannel`, `IN_ASYNC_CONTEXT`, `SchedulerTarget`/
`SchedulerRunResult`/`DebugCoopResume`, `COOP_TASK_STOP`, and the scheduler
callback seams are deleted; `scripts/check-unified-runtime-legacy.sh --check`
(zero-tolerance, no globs) guards against reintroduction. The sole surviving
piece of the old TLS yield transport is `YieldReason` — now a single variant
`Sleep(u64)` (`crates/sema-core/src/async_signal.rs:22`) carried via
`VmExecResult::AsyncYield` — which is **live, not dead**: it is the ctx-less
value ABI for `async/sleep`. Retiring it needs a ctx-full sleep native; a
follow-up, not a correctness gap.
```
  Keep the existing "Inventory reconciliation — RESOLVED" paragraph (743–759) unchanged — it is accurate.

- [ ] **Step 6: Verify no stale claims remain.** Run:

```bash
rg -n "remains the transport|still on the legacy|remains only in code paths|is BLOCKED" docs/deferred.md
```
Expected: no hits in the unified-runtime section (lines >561).

- [ ] **Step 7: Gate + commit.**

```bash
jake docs-check
git add docs/deferred.md
git commit -m "docs(deferred): reconcile unified-runtime ledger with post-purge code"
```

---

### Task 2: Complete the P8 release-readiness "honest ledger"

**Files:**
- Modify: `docs/plans/evidence/unified-cooperative-runtime/release-readiness.md` (§5 "Deferred / not-landed", lines 83–101)

**Interfaces:**
- Consumes: the resolved-entry ground truth from Task 1.
- Produces: a §5 that lists *every* open item, matching `deferred.md`.

- [ ] **Step 1: Append three bullets to §5:**

```markdown
- **Step-G callback re-entry (nested `eval` of an async form)** — one migration
  `#[ignore]` remains: `vm_eval_is_vm_native_runs_async`
  (`crates/sema/tests/vm_integration_test.rs:1775`); needs the parent-VM
  parking machinery (`NativeOutcome::Call` for `eval`). See `docs/deferred.md`
  §Unified runtime migration.
- **Multimethod dispatch of a suspending method** — a characterized
  pre-existing Step-G-class limitation (dispatch re-enters the evaluator
  synchronously); documented in `docs/deferred.md`, not introduced by this
  migration.
- **ASYNC-2 (cross-sibling debugger stepping)** — stepping does not follow
  control across the scheduler boundary into sibling tasks (P3-B3 residual;
  STOP/CONTINUE/inspect/within-task stepping is complete). Deliberately out of
  scope per the plan; tracked in `docs/deferred.md` §ASYNC-2.
```

- [ ] **Step 2: Also note the six-round P7b campaign outcome** (the 5 async/run barrier bugs + D1 apply fix landed after this report was written): append one line to §3:

```markdown
Post-report hardening (P7b rounds 3–6): five `async/run` barrier ordering bugs
found and fixed to convergence (final rule: barriers order by TaskId = spawn
order; commits 7dcb8966..a48dacef), plus D1 (`apply` of a suspending lambda
runs cooperatively, commit caf24f4f). Each carries a regression test in
`crates/sema/tests/vm_async_test.rs`.
```

- [ ] **Step 3: Gate + commit.**

```bash
jake docs-check
git add docs/plans/evidence/unified-cooperative-runtime/release-readiness.md
git commit -m "docs(runtime): complete the P8 honest ledger (Step-G, multimethod, ASYNC-2, P7b rounds)"
```

---

### Task 3: Correct the "likely dead" mischaracterization in migration-progress

**Files:**
- Modify: `docs/plans/evidence/unified-cooperative-runtime/migration-progress.md:43-44`

- [ ] **Step 1: Replace** `YieldReason::Sleep cleanup (likely dead)` **with:**

```markdown
YieldReason::Sleep retirement (LIVE, not dead — it is async/sleep's ctx-less
value ABI; retiring it needs a ctx-full sleep native)
```

- [ ] **Step 2: Commit.**

```bash
git add docs/plans/evidence/unified-cooperative-runtime/migration-progress.md
git commit -m "docs(runtime): YieldReason::Sleep is live (async/sleep value ABI), not dead"
```

---

### Task 4: Archive completed plan docs (repo convention: completed plans → `docs/plans/archive/`)

**Files:**
- Move: `docs/plans/2026-07-13-unified-cooperative-runtime.md` and `docs/plans/2026-07-13-unified-cooperative-runtime-task-{01..06,08..11}.md` → `docs/plans/archive/`
- Modify: `docs/plans/2026-07-15-remaining-work-plan.md` (status header), `docs/plans/2026-07-13-unified-cooperative-runtime-task-07.md` (status header; originally stayed in place as the live P6 remainder — since archived 2026-07-17 now that P6-1 and P6-3 have both landed, see `docs/plans/archive/2026-07-13-unified-cooperative-runtime-task-07.md`)

**Interfaces:**
- Produces: `docs/plans/` contains only live plans: `2026-07-15-remaining-work-plan.md` (annotated), and this file. `task-07`, `2026-07-16-wasm-promise-driven-roots.md`, `2026-07-16-p6-1-host-api.md`, `2026-07-16-runtime-fast-path-recovery.md`, and `2026-07-17-runtime-fast-path-0c.md` are now all EXECUTED/LANDED and archived (`docs/plans/archive/`).

- [ ] **Step 1: Find inbound references before moving** (evidence docs and deferred.md link to these files):

```bash
rg -ln "2026-07-13-unified-cooperative-runtime" docs/ crates/ scripts/ --glob '!docs/plans/2026-07-13-*'
```

- [ ] **Step 2: Move with `git mv`** the master plan + task files 01–06 and 08–11 into `docs/plans/archive/`, then fix every reference found in Step 1 to the `archive/` path.

- [ ] **Step 3: Annotate the two live plans.** At the top of `2026-07-15-remaining-work-plan.md`, replace the `**Status:**` line with:

```markdown
**Status (2026-07-16): EXECUTED — P-hotfix, P0–P5, P7, P8 complete; C1/C2/C3/C4
decided and landed. Open remainder: P6-1 (host API), P6-3 (wasm Promise roots),
SRV-1, Step-G. See docs/plans/2026-07-16-post-migration-doc-reconciliation-and-p6-roadmap.md
for sequencing.**
```
  Add the equivalent one-line status to `task-07`'s header (DAP host done in P3; common host API + wasm + services remain).

- [ ] **Step 4: Gate + commit.**

```bash
jake docs-check && cargo nextest run -p sema-lang --test runtime_conformance_test
git add -A docs/plans
git commit -m "docs(plans): archive completed unified-runtime plans; annotate live remainders"
```
(The conformance run guards against a moved file breaking a path the inventory/docs tooling reads.)

---

### Task 5: Final verification sweep of the branch

**Files:** none (verification only)

- [ ] **Step 1: Run the full CI-equivalent suite** (AGENTS.md release rule — plain test runs are not enough):

```bash
cargo nextest run --workspace && jake examples && jake smoke-bytecode && jake lint && jake docs-check \
  && ./scripts/check-unified-runtime-legacy.sh --check \
  && ./scripts/check-unified-runtime-inventory.sh --check
```
Expected: all green. If the inventory gate is red from doc-only commits, something referenced a moved plan file — fix the reference, do not regenerate the map.

- [ ] **Step 2: Disk hygiene** (the volume ran at 96–98% during the campaign):

```bash
jake target-sizes && jake sweep-preview days=3
```
Run `jake sweep days=3` from the workspace root if the preview shows meaningful reclaim.

---

## Roadmap — remaining engineering slices (each gets its own plan at execution time)

These are **not** specified here; each has an authoritative design already. Sequencing and entry gates only.

### Slice A — P6-1 common host API — DONE
- **Design:** `docs/plans/archive/2026-07-16-p6-1-host-api.md` (all 6 tasks landed) + `docs/deferred.md` §P6-3/P6-1 blocker 1.
- **Scope:** public `Interpreter::submit_str/submit_value/drive/cancel_root/command_handle/shutdown`, `RootOptions`, root-tagged `OutputEvent`, `RuntimeCommandHandle` as the only `Send` surface. Landed on top of the existing private machinery (`drive_vm_on_runtime`, `Runtime::submit_root`, `ShutdownOptions/Report`); CLI Ctrl-C and the notebook engine are the proving consumers.
- **Note:** `check_interrupt` TLS retirement belonged to P6-3 (wasm SAB-cancel was its only consumer) — also done (the SAB path is deleted).

### Slice B — P6-3 wasm Promise-driven roots — DONE
- **Design:** `docs/plans/archive/2026-07-16-wasm-promise-driven-roots.md` (design record + acceptance gate, both kept verbatim). Landed 2026-07-17: `evalPromise` is the live default seam, the replay/Atomics machinery is deleted, playground programs execute once, concurrent evaluations are individually cancellable via `cancel_root`. Two narrowly-scoped survivors documented in `docs/deferred.md`'s P6-3 entry (debugger HTTP marker flow; synchronous-entry-point sleep busy-poll).
- **Oracle:** `playground/tests/unified-runtime.spec.ts` real-browser acceptance gate is green (transcript committed at `docs/plans/evidence/unified-cooperative-runtime/p63-browser-gate-transcript.txt`); `scripts/test-packaged-sema-web.sh` + wasm asset regen pass per AGENTS.md.

### Benchmark vs pinned baseline `3f111e83` — DONE
- Outcome: primes/spawn-storm now beat the pre-unification baseline (0.7×); the remaining PERF-RESIDUAL-1 rows were closed by Slices 0b/0c (channel rendezvous 7.4×→~1.4×, O(1) cancel-waiting/ready-remaining, depth-bounded value drop). See `docs/plans/evidence/unified-cooperative-runtime/benchmark-vs-baseline.md` close-out section.

### Windows CI leg — committed, advisory
- `ci.yml`'s `test-windows` job (`continue-on-error: true`) runs `cargo nextest run --workspace` on `windows-latest`. Advisory until first green, per its own header comment; not yet promoted to required.

### Slice C — SRV-1 concurrent `http/serve` (independent; remains open)
- **Design:** remaining-work plan §7.4; handler-task-per-connection, ~500–700 lines.
- **Oracle:** the four `#[ignore]`d wall-clock acceptance tests in `crates/sema/tests/http_serve_concurrent_test.rs` (137/186/231/265) — strict TDD: un-ignore, watch fail, implement.
- **Risk to prove out first:** accept-loop deadlock-freedom while idle-External-parked.

### Slice D — Step-G callback re-entry (nested `eval` + multimethod dispatch) — remains open
- **Design:** `docs/deferred.md` §Unified runtime migration (both sub-entries). One machinery serves both: dispatch/nested-eval returns `NativeOutcome::Call` instead of synchronous `call_callback` re-entry.
- **Oracle:** un-ignore `vm_integration_test.rs:1775`; add a multimethod-suspending-method test.

### Remaining, before merging to main
- **Stale-comment sweep:** ~40 comment lines still name `in_async_context()`/`IoHandle`; clean opportunistically in files touched by Slices C–D (per the AGENTS.md comment rule), not as a standalone churn commit.
- **ASYNC-2** stays deferred by design — do not schedule.

### Merge recommendation
After Phase 0 + the benchmark: **merge the runtime to main**. Slices A and B are done; C (SRV-1) and D (Step-G) are additive follow-ups with working fallbacks and committed TDD gates; holding a 190+-commit branch open for them adds rebase risk with no safety benefit.
