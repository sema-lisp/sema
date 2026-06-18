# C1 — Route stdlib HOF callbacks in-VM (fix `set!` lost through HOF callbacks)

Date: 2026-06-18
Status: in progress
Decision: OPEN.md C1 — "route HOF callbacks in-VM"

## Repro

```
$ sema --tw -e '(let ((c 0)) (map (fn (x) (set! c (+ c x))) (list 1 2 3)) c)'
6
$ sema      -e '(let ((c 0)) (map (fn (x) (set! c (+ c x))) (list 1 2 3)) c)'
0   ← wrong, should be 6
```

Any closure that captures a local and `set!`s it, when invoked through a stdlib
higher-order function (`map`, `filter`, `for-each`, `foldl`, `sort-by`, …),
loses the mutation on the VM backend. Globals and in-VM closure calls are fine.

## Root cause

The Lua-style open-upvalue runtime (`UpvalueState::{Open,Closed}`) keeps a
closure's captured local as an *open* cell that points at the parent frame's
live stack slot. In-VM `set!` writes through the open cell to the parent slot.

But the VM closes those open cells (`close_open_upvalues`) **before every
non-VM call** — the `CALL_NATIVE` site plus the `call_value` / `call_value_with`
native + callback sites in `vm.rs`. Closing snapshots the current value into the
cell and detaches it from the stack slot.

When a stdlib HOF then invokes the captured VM closure, it does so via
`call_callback` → tree-walker `call_value` → the closure's `NativeFn` *fallback*,
which spins up a **fresh** `VM` (Decision #50 dual-path). The fresh VM mutates
the now-*closed* snapshot cell; the parent VM's stack slot — the real `c` — is
never updated. After `map` returns, the parent reads the stale slot → 0.

## Fix: re-enter the *running* VM instead of spawning a fresh one

Keep one running VM per thread reachable from inside native calls, and have the
VM-closure fallback run the closure as a **nested frame on that same live VM**
when the closure belongs to it (same `functions` + `globals` Rc). Because the
parent frame's open upvalue cells are still Open and still point at the live
parent stack slots, `set!` inside the callback writes straight through to the
parent — exactly like an in-VM call.

Concretely:

1. **Thread-local current-VM stack** (`CURRENT_VM: RefCell<Vec<*mut VM>>`) in
   `vm.rs`. A RAII guard (`CurrentVmGuard`) pushes `self` before a native call
   and pops after. Strictly nested: the parent's `run_inner` is paused at the
   exact native call and does not touch `self` until the native returns, so the
   raw pointer is valid and unaliased for the nested call.

2. **`VM::run_nested_closure(closure, args, ctx)`** — pushes a frame for the
   closure on the current stack/frames and runs a *bounded* dispatch loop that
   returns when that frame (and only that frame) returns. Upvalues are NOT
   closed on entry; the parent's open cells stay connected.

3. **Fallback `func`** of a VM closure first tries `try_run_on_current_vm`: if a
   compatible running VM is registered on this thread, route the call there.
   Otherwise keep the existing fresh-VM / async-scheduler behaviour unchanged.

4. **Stop closing upvalues before re-entrant native calls** is NOT done globally
   — instead the args are copied to an owned `Vec` at the re-entrant native call
   sites so the `&self.stack` borrow is released before the native runs (required
   for the nested run to take `&mut *vm` soundly). The `close_open_upvalues`
   calls before native dispatch are removed for the routed path because routing
   keeps cells open; they remain correct for the fresh-VM path (a closed cell is
   a valid snapshot there).

## Safety

- The native call is fully synchronous and nested: control returns to
  `run_inner` only after the native (and any nested closure runs) complete. The
  parent does not read or write `self` during the call.
- Args are copied to an owned `Vec` before invoking natives that can re-enter,
  so there is no outstanding `&self.stack` borrow when the nested run mutates the
  stack. This removes the aliasing hazard at the cost of one small alloc per
  native call.
- The nested run is bounded to its own frame depth: it returns as soon as the
  frame it pushed is popped, leaving the parent frames untouched.

## Async / yield interaction

The fallback keeps the `in_async_context()` → `run_closure_as_inline_task`
branch. In-VM routing is only taken on the synchronous path. The nested run uses
the non-debug, non-async `run` path; if a routed closure performs an async yield
it surfaces the existing "async yield outside scheduler" behaviour, identical to
the previous fresh-VM fallback (which also used `vm.run`). The async scheduler
tests must stay green.

## Risks

- Re-entrancy via raw pointer: mitigated by the strict nesting + arg copy.
- Recursion depth: nested runs add Rust stack frames per HOF nesting level (same
  as the old fresh-VM path, which also recursed into `vm.run`). MAX_FRAMES still
  guards VM-level depth.
- Tree-walker behaviour unchanged.

## Verification

- New dual-eval regression test: the repro returns 6 on BOTH backends, plus
  `filter`/`for-each`/`sort-by` variants.
- `cargo test -p sema-vm`, `-p sema-stdlib`, dual_eval_test, vm_async_test, and
  clippy `-D warnings` all green.
