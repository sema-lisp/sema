# async/spawn snapshots open upvalues at spawn time — locals assigned after spawn read stale values in the task

**Status:** open (pre-existing; found by the CORE-2 M3 GC stress campaign, 2026-07-02)
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
