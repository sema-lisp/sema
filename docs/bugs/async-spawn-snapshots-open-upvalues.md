# async/spawn snapshots open upvalues at spawn time — locals assigned after spawn read stale values in the task

**Status:** FIXED (2026-07-10, #104) — see `## Fix` below. (Pre-existing; found by the CORE-2 M3 GC stress campaign, 2026-07-02.)
**Verified against:** installed `sema 1.28.1` and the `worktree-core2-gc-design` build
**Area:** `sema-vm` scheduler spawn path (`close_closure_upvalues_for_foreign_run`), not the collector

## Repro

```bash
sema -e '(define (demo) (define x nil) (define p (async/spawn (fn () (async/sleep 5) x))) (set! x 42) (await p)) (println (demo))'
# nil
```

Expected `42`: the task closure captures the local `x` by cell, the `set!`
runs before the task body ever reads `x` (the task only starts when `await`
drives the scheduler), so the task should observe the updated value — exactly
as the same shape does without a spawn:

```bash
# control: same capture + set!, no spawn — shares the cell correctly (C1 semantics)
sema -e '(define (demo) (define x nil) (define f (fn () x)) (set! x 42) (f)) (println (demo))'
# 42
```

## Cause

`Scheduler::spawn` calls `close_closure_upvalues_for_foreign_run(&closure)`
(`crates/sema-vm/src/scheduler.rs` → `crates/sema-vm/src/vm.rs:630`) because
the task runs on a dedicated task VM whose stack differs from the spawning
VM's — an `Open { frame_base, slot }` cell would dangle there. The close is
in place (`*cell.state.borrow_mut() = Closed(current stack value)`) and the
owning frame's `open_upvalues` entry for the slot is cleared, so the cell —
still `Rc`-shared with the task — is *decoupled from the parent's stack
slot* at spawn time. `StoreLocal` writes the stack slot unconditionally
(Lua-style open-upvalue model), so the parent's later `set! x 42` updates a
slot no cell watches anymore, and the task reads the cell's spawn-time
snapshot (`nil`).

So the close-at-spawn is load-bearing (C1: cells must not dangle across
foreign VM stacks) but over-eager as *semantics*: it turns capture-by-cell
into capture-by-value at the spawn boundary while the defining frame is
still live.

## Notes

- Globals are unaffected (they resolve through the env, not upvalues):
  spawn-then-`define`/`set!` of a global reads the updated value in the task.
- Found while writing GC stress workloads that mutate captured locals around
  `async/spawn`; behavior is identical with and without the collector.
- A fix has to keep parent writes flowing into the early-closed cell — e.g.
  keep the frame's `open_upvalues` entry alive with a "closed but tracked"
  state whose Store writes both slot and cell, or defer the real close to
  frame exit and give the task VM a way to read a foreign-but-live open cell.
  Both touch the C1 machinery; needs its own careful change, not a drive-by.

## Fix

Implemented the "closed-but-tracked" cell state (the first suggested direction),
in `crates/sema-vm/src/vm.rs`. A third `UpvalueState` variant is introduced:

```rust
enum UpvalueState {
    Open { frame_base, slot },   // reads/writes the owning VM's live stack slot
    Closed(Value),               // owns the value after the defining frame exits
    Tracked { frame_base, slot, value },  // detached-but-live (NEW)
}
```

`close_closure_upvalues_for_foreign_run` no longer fully `Closed`s a cell at the
foreign-run boundary. It snapshots the value off the owning VM's stack (as
before) but stores it as `Tracked` and — crucially — **leaves the owning frame's
`open_upvalues[slot]` entry in place** (the old code cleared it). A `Tracked`
cell owns its `value`, so it is safe to read on any VM stack (it no longer
indexes a foreign stack — the **C1 no-dangle invariant is preserved**, which is
the whole reason the close-at-spawn exists), while still being reachable from the
still-live defining frame.

Because the entry stays, the defining frame's later writes to that local keep
flowing into the cell:

- **`StoreLocal` (and `StoreLocal0..3`)** now call `propagate_local_store_to_tracked`,
  which mirrors the write into a `Tracked` cell for that slot in addition to the
  stack slot. It is a single branch (`open_upvalues == None`) for non-capturing
  frames — the hot path is unchanged — and only does real work for a captured
  slot whose cell is `Tracked` (i.e. only after a spawn). Note the specialized
  `StoreLocal0..3` opcodes had to be patched too; patching only the generic
  `StoreLocal` left the repro (`set! x` → `StoreLocal0`) still broken.
- **`LoadUpvalue`/`StoreUpvalue`** read/write a `Tracked` cell's owned `value`
  directly (no stack indexing), so a task VM — and any closure sharing the cell —
  observes and can mutate the current value.

When the defining frame exits, `close_open_upvalues` / `close_open_upvalues_above`
promote a `Tracked` cell to a real `Closed`, finalizing it with the cell's own
`value` (which already reflects the latest parent `StoreLocal` **and** task
`StoreUpvalue` writes, whereas the stack slot only saw the parent writes). After
that the cell no longer references any frame slot, so a task that outlives the
frame (promise returned then awaited later) reads the correct final value.

GC integration (CORE-2): `trace_upvalue_cell` now sinks a `Tracked` cell's owned
`value` as a strong edge (like `Closed`) so the collector keeps it reachable, and
`sever_upvalue_cell` returns it for deferred drop. A `Tracked` cell is kept black
by its live frame's `open_upvalues`, exactly as an `Open` cell is.

**Verification:** the repro now prints `42`; a task whose promise is awaited after
the defining frame returns reads the final value; globals still unaffected.
Regression tests live in `crates/sema/tests/vm_async_test.rs`
(`spawn_observes_set_of_captured_local_after_spawn` and siblings, plus the
`no_spawn_observes_set_of_captured_local` control). Full CI-equivalent suite
(`cargo test --workspace`, `jake examples`, `jake smoke-bytecode`, `jake lint`,
`jake docs-check`) is green — including the async, closure/upvalue, and
GC/CORE-2 stress tests.
