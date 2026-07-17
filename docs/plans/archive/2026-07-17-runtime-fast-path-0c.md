# Slice 0c: Scheduler Squeeze — profile-directed micro-optimization pass

**Status: EXECUTED** — Tasks 0c-1 through 0c-6 complete, plus 0c-7
(direct task-to-task rendezvous handoff, added after close-out) also landed.
See `docs/plans/evidence/unified-cooperative-runtime/benchmark-vs-baseline.md`
close-out section for final numbers.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Continuation of the 0b pass (see `2026-07-16-runtime-fast-path-recovery.md`); same Global Constraints, measurement protocol, and inventory/lint discipline apply verbatim.

**Goal:** Close the PERF-RESIDUAL-1 rows using symbolized-profile evidence (`scratchpad/prof/*-0c.sample.txt`, release-with-debug build). Squeeze order = measured leverage.

**Profile evidence (main-thread samples, pingpong-1M / deep-await-big):**
- SipHash on id-keyed std HashMaps: `sip::Hasher::write` 314 + `hash_one<&TaskId>` 99 + `<&RootId>` 91 + ReadyScheduler enqueue/dequeue 148 ≈ **~25% of pingpong**.
- `Runtime::cancel_waiting` 218 (pingpong) / 112 = **top sema fn in deep-await**: drive source 2 does a LINEAR SCAN of `state.tasks` per rotation (state.rs:1473) hunting cancelled waiters that don't exist in cancel-free programs.
- Clock residue: `Timespec::sub_timespec` 106 + `mach_absolute_time` 73; completion-inbox `try_recv` 64/iteration; `_tlv_get_addr` 69.
- cons-10m: allocation-bound (`nanov2_free` 143 + malloc ~190 + `Vec<Value>::resize`/`finish_grow` 51) — needs comparative baseline profile before touching.

### Task 0c-1: hashbrown/fast-hash for runtime id maps
Swap `std::collections::HashMap/HashSet` keyed by `TaskId`/`RootId`/`WaitKey`/`ChannelId`/`PromiseId` to `hashbrown` (already a workspace dep; its default hasher is fast) across `runtime/state.rs`, `ready.rs`, `channel.rs`, `wait.rs`, and any registry with id keys. Pure container swap — no logic change. Oracle: pingpong/deep-await wall+instructions drop; full suite green.

### Task 0c-2: O(1) cancel_waiting via dirty queue
Replace the per-rotation full-task scan with `pending_cancel_waits: VecDeque<TaskId>` on `RuntimeState`: every site that records a cancellation on a task (cancel_root fan-out, async/cancel, C2 request-time delivery, shutdown) pushes the id; `cancel_waiting` pops until it finds a still-valid candidate (re-validate: still Waiting + still cancelled + UCR-3 channel-wake skip preserved — re-push nothing that fails validation, the wake path re-enqueues if needed... careful: the UCR-3 skip case must RE-PUSH or be re-armed when the wake completes, else a cancelled-but-skipped waiter is never torn down; trace that path before coding). Shutdown keeps its drain loop (state.rs:4031/4131) — seed the queue with all waiting tasks at shutdown instead of scanning. Empty queue = `Ok(false)` in O(1). Oracle: deep-await/pingpong drop; ALL cancellation tests + watchdog + UCR-3 tests green.

### Task 0c-3: divan micro-benchmark suite for the scheduler
`crates/sema-vm/benches/runtime_micro.rs` (divan, dev-dependency; `bench = false` on lib per divan convention): benchmarks for (a) one matched rendezvous, (b) spawn→settle lifecycle, (c) timer arm+fire, (d) in-place HOF element dispatch, (e) one idle drive turn, (f) cancel_waiting on N parked tasks (locks in 0c-2). Drive through the public `Runtime`/test harness used by `runtime/tests.rs` (no Sema source parsing in the hot loop — build VMs/requests directly where feasible, else eval a pre-compiled chunk). Register a `jake bench.micro` recipe. Oracle: `cargo bench -p sema-vm` runs; numbers recorded in the evidence doc as the go-forward regression reference.

### Task 0c-4: clock + inbox residue
After 0c-1/2 land, re-profile pingpong. If `sub_timespec`/`mach_absolute_time`/`try_recv` still register: locate remaining per-iteration `clock.now()`/elapsed sites (candidates: run_parked_quantum wall accounting, timer wheel peek) and batch/skip; gate completion-inbox `try_recv` behind an atomic dirty flag set by completion senders. Only act on what the fresh profile shows.

### Task 0c-5: cons-1m comparative diagnosis
Profile baseline binary on cons-10m (`sample`, same protocol) and diff top-of-stack vs current. Identify the EXTRA allocation source (suspects: per-quantum VM stack `Vec<Value>::resize`, GC-registry bookkeeping per heap Value, drop path). Fix only if the diff names a runtime-attributable source; a pure allocator-shape parity finding closes the row as not-runtime-caused.

### Task 0c-6: close-out
Re-run the six-benchmark matrix + divan suite; update `benchmark-vs-baseline.md` final table + PERF-RESIDUAL-1 (resolve rows that reach ≤1.10×, keep honest residuals); ledger + CHANGELOG.

### Task 0c-7: direct task-to-task rendezvous handoff (added after close-out; owner go-ahead)
Close the last residual (pingpong 1.97×, ~12k instr/message): a channel op whose
match is immediately available must complete WITHOUT parking its own task.
Preferred design (b) — no new VM↔runtime seam: in the drive path where a task's
quantum returns the channel-suspend (visit_ready → run_parked_quantum →
quantum_to_action), consult the registry BEFORE boxing/parking the VM; on
immediate availability, write the response onto the still-unboxed VM's stack
(the same value-application `reinstall_parent_vm_now` performs) and loop
straight back into `run_quantum` on the same VM object — no `Box<VM>`, no
task-map churn, no extra drive items for the matched side. Peer delivery reuses
the Task-D inline machinery unchanged. Alternative design (a) — a VM-level
fast-resolve hook pre-suspend — only if (b) proves unworkable; justify.
Constraints: quantum instruction budget continues across the handoff loop
(bounds a send/recv-spinning pair exactly like Task C's in-place loop);
work-item credit debited per handoff; FIFO via registry queues untouched;
genuine-block path byte-identical; UCR-3/cancellation windows preserved
(cancellation re-checked per handoff iteration); `async/run` barrier
re-evaluation unaffected (handoffs happen within one work item, barriers
re-check next iteration as with Task C/D).
Oracle: pingpong ≤1.10× instructions vs ~400M baseline (stretch; report honest
number); full channel battery + barrier + cancellation + watchdog suites;
divan channel_rendezvous bench delta.
