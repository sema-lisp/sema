# Breakpoints don't fire inside async task code

**Status:** STOP+CONTINUE FIXED in BOTH the native DAP (Slice 1) and the WASM
playground (Slice 2), 2026-06-23 — a breakpoint inside an async task now stops and
`Continue` resumes in both. Remaining follow-ups: stepping (Step Into/Over/Out)
*across* the scheduler boundary into sibling tasks, and full stack/variable
inspection targeting the paused **task's** VM at an async stop (today inspection at a
cooperative async stop targets the main VM's `async/all`/`await` frame). See
`docs/plans/2026-06-23-async-debugger.md` and the gate tests
`crates/sema/tests/dap_async_breakpoint_test.rs` (native) +
`crates/sema/tests/wasm_async_debug_test.rs` (cooperative) +
`playground/tests/async-debugger.spec.ts` (e2e).

## Symptom

In the playground debugger (and the native DAP / VS Code), a breakpoint set on a line
that executes **only inside an async task** — i.e. anything inside an `(async …)` /
`(async/spawn …)` thunk, or code reached through `async/map` / `async/pool-map` /
`async/all` / channel workers — is **silently ignored**. Execution runs straight
through it. Breakpoints in *synchronous* top-level code (before/after the async block)
work fine. Stepping has the same gap.

## Root cause (confirmed)

Breakpoint and step checking live entirely in the VM's debug-aware execution loop,
gated on a `DebugState` argument: `VM::run_inner(ctx, Some(debug))`
(`crates/sema-vm/src/vm.rs:875`). Only the main debug driver calls it with
`Some(debug)`.

The cooperative scheduler runs **every** async task step through the *non-debug*
path: `Scheduler::run_until_reentrant` → `task.vm.run_async(ctx)` /
`task.vm.execute_async(closure, ctx)` (`crates/sema-vm/src/scheduler.rs:894-896`), and
both of those call `self.run_inner(ctx, None)` (`vm.rs:785, 815`). `None` ⇒ the
breakpoint/step machinery is skipped. The scheduler has **no `DebugState` plumbing at
all** (`grep DebugState scheduler.rs` → nothing), and per-task VMs
(`VM::new_for_task`, `vm.rs:403`) carry no debug state.

So: synchronous code is debugged on the main VM with `Some(debug)`; the moment control
enters the scheduler (any `await`/`async/*`), tasks execute in non-debug mode and
breakpoints can't trip. This is architectural — the cooperative scheduler + per-task
VMs and the debugger were built independently — and affects **both** the WASM
playground and the native DAP identically (it is not WASM-specific).

## Repro

In the playground (or `sema dap`): set a breakpoint on the `(+ 1 2)` line below.

```sema
(define p (async/spawn (fn ()
  (+ 1 2))))      ;; <- breakpoint here NEVER trips
(await p)
```

vs. the synchronous version, where the breakpoint DOES trip:

```sema
(define (work) (+ 1 2))   ;; <- breakpoint here trips fine
(work)
```

## Workaround

None at the debugger level. To inspect async task state today, fall back to
`println`/logging inside the task, or temporarily de-async the code path under test.

## Why deferred

The fix (see the plan) is a debugger×scheduler integration with a real stop/resume
design decision (a breakpoint hit mid-task must pause the scheduler and round-trip a
`DebugCommand` through the playground's step-driven JS bridge), not a localized patch.
It is worth its own slice and was deferred to keep the async-concurrency release
focused.
