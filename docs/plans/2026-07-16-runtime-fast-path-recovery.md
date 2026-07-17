# Slice 0b: Runtime Fast-Path Performance Recovery

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status (2026-07-17): EXECUTED — Tasks A–F complete, every task Opus-reviewed.
Outcome: primes 0.74×/spawn-storm 0.67× (beat baseline); pingpong 2.82×,
sleep-storm 1.65×, deep-await ~1.7×, cons-1m 1.38× accepted as PERF-RESIDUAL-1
(docs/deferred.md) per owner decision. See the close-out section of
docs/plans/evidence/unified-cooperative-runtime/benchmark-vs-baseline.md.**

**Goal:** Recover the universal-flip performance regression (see `docs/plans/evidence/unified-cooperative-runtime/benchmark-vs-baseline.md`): channel rendezvous 6.5–7.4×, HOF callback dispatch ~28k instructions/element (primes 2.1× instructions), dispatch-loop accounting ~13%, drive-loop clock reads ~47% of drive time on channel-heavy workloads. Exit bar: **≤1.10× vs baseline `3f111e83` on all six benchmarks** (wall, hyperfine `--warmup 3 --runs 10`, plus `/usr/bin/time -l` instruction counts for primes/pingpong as the low-noise oracle).

**Architecture:** Four independent, individually-benchmarked fixes to `crates/sema-vm/src/runtime/state.rs` + `crates/sema-vm/src/vm.rs`, ordered cheap-and-safe → structural. Each fix = one commit + full suite + benchmark delta recorded in the evidence report. No observable-semantics change without an explicit flagged decision.

**Measurement protocol (every task):** benchmarks live in the session scratchpad `bench/` dir; baseline binary = `/Users/helge/code/sema/.worktrees/bench-baseline/target/release/sema` — **verify `git -C .worktrees/bench-baseline log --oneline -1` shows `3f111e83` AND rebuild if the binary predates the checkout** (a stale-binary mixup already burned one investigation — binary identity is part of the oracle). Record: wall mean±σ both binaries + instructions retired for primes/pingpong.

## Global Constraints

- Full CI-equivalent suite green after every task: `cargo nextest run --workspace && jake examples && jake smoke-bytecode && jake lint`.
- Inventory gate re-stamped when state.rs/vm.rs line-drift turns it red.
- No change to: wait/wake ordering visible to `async/run` barriers, cancellation delivery points, work-item-budget fairness floors (`reserve_floor`/`reserved_roots`, state.rs:900), the `check_hof_yield` error surface for bare yielding natives in HOF callbacks (state.rs:2933-2951).
- Opus review per task (hot-path semantics).

---

### Task A: Batch the drive-loop clock reads

**Where:** `state.rs:919-925` (`expired_quarantine(state.clock.now())`) and `state.rs:934-943` (wall-clock budget check) — two unconditional `Instant::now()` per drive iteration; ~47% of sampled drive time on pingpong (`scratchpad/prof/pingpong-big.sample.txt`).
**Change:** read the clock once at drive() entry and then at most every 64 iterations (a wrapping counter); thread the cached `Instant` through both checks. Worst-case wall-budget overshoot = 64 tiny iterations (µs-scale) — bounded, document on `DriveBudget::wall_clock_limit`.
**Oracle:** pingpong instructions/wall drop measurably; `cargo nextest run -p sema-vm` (timer/quarantine/wall-budget tests) green; the watchdog tests in `crates/sema/tests/unified_runtime_watchdog_test.rs` still pass (they assert hang *detection*, which must survive coarser clocking).

### Task B: Batched instruction accounting in the dispatch loop

**Where:** `vm.rs:2204-2212` — per-opcode `Option` load + compare + increment.
**Change:** register-local countdown: at quantum entry compute `remaining = budget - executed` into a local; decrement the local per instruction; on hitting 0 sync back and return `QuantumExpired`; sync the local back to `self.instructions_executed` at every exit point (Return/Suspend/error/debug-stop) — the audited exit list is the review focus. Alternatively (simpler, review both): keep the counter but hoist `instruction_budget` into a local `Option<usize>` converted to a plain `usize` sentinel (`usize::MAX` = unlimited) so the hot check is one compare on a register.
**Oracle:** cons-1m gap shrinks toward ≤1.10×; quantum-expiry tests in sema-vm (`run_quantum`-based) green — especially exact-boundary tests if any assert precise instruction counts (they may need documented adjustment if the batch granularity changes observable expiry points — flag, don't silently change).

### Task C: HOF `NativeOutcome::Call` in-place dispatch for non-yielding callbacks

**Where:** `invoke_callable` (state.rs:2836-2918) — today: fresh `VM::new_for_task_with_native_fns` per element + `Box<ReturnOwner>` + ready-queue round-trip + ≥4 drive iterations per element ≈ 28k instructions/element.
**Change:** when the callable is a plain VM closure, run it **synchronously in place** on a reused scratch VM (or nested frame on the parked parent — design decision for the implementer to propose, reviewer to check): execute the callback quantum immediately inside `invoke_callable`; if it `Finished` → feed the continuation directly (loop: continuation may return the next `Call` — drive the whole HOF element loop without leaving the function); if it **suspends** → fall back to exactly today's park path (this fallback is mandatory and must preserve `check_hof_yield` semantics). Reuse one scratch VM cached on `RuntimeState` across elements (clear between uses) to kill the per-element allocation.
**Semantic flag:** callback elements no longer interleave with sibling tasks between elements *when they don't yield* — same-as-baseline behavior (baseline ran callbacks synchronously), but a change vs current runtime fairness. Bounded by the quantum instruction budget: the in-place loop must count callback instructions against the parent quantum's budget so a 2M-element filter still yields to siblings on budget expiry.
**Oracle:** primes instructions from 540M → ~270M (≤1.10× of 257M); `runtime_map_callback_awaits_spawned_child` and all cooperative-HOF async tests green (the yielding fallback); eval_test/vm_async_test full green.

### Task D: Matched channel rendezvous completes without extra drive hops

**Where:** `install_channel_wait` (state.rs:4406-4482) already knows at install time whether the peer is queued (`result != Waiting`); today the matched case still round-trips `PendingStage::ChannelWake` → `finish_protocol_wait` → `ApplyRuntimeResponse` → `resume_continuation` → `Apply` → `reinstall_parent_vm` → fresh `visit_ready` (~10 drive iterations/rendezvous, ledger in the profiling report).
**Change:** on immediate match, resume BOTH sides' continuations synchronously within the current work item (self gets its response; peer's wake → `finish_protocol_wait` + `resume_continuation` + `reinstall_parent_vm` inline), enqueueing only the final ready-to-run VMs. Target: ≤3 drive iterations per rendezvous.
**Semantic flag:** collapsing hops concentrates more progress into one work item — must still debit `budget.work_item_limit` per logical hop (count the collapsed stages) so channel-heavy pairs cannot starve sibling roots; `async/run` barrier re-evaluation happens per drive iteration either way. Reviewer checks the UCR-3 rendezvous-cancel case (state.rs:818-825 region) and the cancelled-mid-match window.
**Oracle:** pingpong ≤1.10× instructions vs baseline (1.23B); channel test battery (sema-vm channel tests, vm_async channel cases, `async_run_releases_over_channel_rendezvous_blocked_child`) green.

### Task E: TaskScopeSwap empty-scope fast path

**Where:** `TaskScopeSwap::install/restore` (state.rs:220-269) + the three `TASK_SCOPE_SEAMS` (state.rs:166-186) — malloc/free per quantum of every spawned task even when LLM/OTel/usage are unused (visible in the pingpong profile).
**Change:** each seam gains an is-empty predicate; a captured-empty scope installs/restores as a no-op (no boxing round-trip). Panic-safety `Drop` (state.rs:254-269) unchanged.
**Oracle:** spawn-storm/deep-await/sleep-storm deltas; the P-hotfix isolation tests (per-task OTel/usage attribution under forced interleave) green — they are the invariant this code exists for.

### Task F: Re-run the full six-benchmark suite + close out

Update `benchmark-vs-baseline.md` with a final table (before/after per fix), flip the verdict section, mark the orchestration-plan gate passed. If any row still >1.10×: document residual with a per-row explanation and an explicit accept/continue decision for Helge. `jake wt-rm name=bench-baseline` (from workspace root) to reclaim the baseline worktree. Ledger + CHANGELOG entries.

---

**Order:** A → B (independent, small) → C → D (structural, benefit compounds with A) → E → F. Sleep-storm (2.15×) is expected to be mostly cured by A+D (timer wakes ride the same drive loop); if not, a timer-batching task gets added with its own measurement.
