# Restricted VM Bridge Removal

**Status:** Approved for implementation 2026-07-19

## Goal

Remove active-runtime synchronous evaluator re-entry without breaking procedural
macros or debugger expression evaluation. Delete `QuantumSuspendGuard`, the
`CURRENT_VM` raw-pointer stack, nested foreign-VM callback execution, and
task-time `STDLIB_CTX` fallback after their callers have explicit context and
owner plumbing.

## Contract

A restricted compiler/debugger evaluation may execute ordinary non-suspending
Sema code, including user helpers, higher-order functions, and multimethods. It
must:

- keep the runtime-quantum flag active and invoke runtime native ABIs;
- share one hard instruction budget across every VM/callback transition;
- bound zero-bytecode continuation transitions separately;
- observe cancellation and an optional deadline between transitions;
- accept only terminal return or error outcomes;
- reject `Suspend` and `Runtime` before installing a wait or runtime request;
- never drive the scheduler, start a prepared external operation, or sleep.

Procedural transformers report `macro transformer cannot suspend during
expansion`. Debugger expressions report `debug evaluation cannot suspend`.
Conditional-breakpoint errors remain fail-open. Failed `setVariable`
expressions do not mutate the stopped frame.

## Task 1: Make native call context self-contained

Replace the borrowed `&mut TaskContext` in `NativeCallContext` with a cloned
`TaskContextHandle`. Add `call_env: Option<Rc<Env>>`.

- VM native dispatch supplies its active globals as `call_env`.
- Runtime callback dispatch propagates the parked parent VM's globals.
- Host calls use `None` and retain explicit weak fallback environments.
- Task-local access borrows the handle only for the individual operation; no
  `RefMut` may span a native call.

Update all context constructors and direct task-context readers. Prove two
interpreters with colliding local IDs keep task context and call environments
isolated.

## Task 2: Remove ambient current-environment readers

Replace `current_vm_globals()` at load/import, runtime eval, GC pinning, and
other delegates with `NativeCallContext.call_env` plus their existing weak
fallback.

Tests cover nested module imports, `load` definitions, runtime eval, and GC pins
landing in the exact caller/module environment.

## Task 3: Make escaping ownership explicit

Refactor escaping-value traversal to accept an explicit owner VM instead of
reading `CURRENT_VM`.

- Runtime callback/spawn paths use the parked parent VM already held by
  `ReturnOwner`.
- VM native dispatch passes `self` directly.
- Host compatibility calls snapshot against their explicit caller before a
  fresh-VM call, then synchronize tracked upvalues back afterward.

Keep `snapshot_escaping_call_with_owner`,
`snapshot_native_escaping_args_with_owner`, and tracked-upvalue synchronization.

## Task 4: Add the bounded restricted driver

Add a `sema-vm` restricted driver accepting an operation name, instruction and
transition limits, optional deadline, cancellation view, task-context handle,
compiled program, and globals.

Drive VM closures, native callables, keywords, multimethod dispatch, and
continuation-produced calls inline. Return before installing any suspend or
runtime request.

Tests cover:

- ordinary helper/HOF/multimethod calls;
- attempted timer, channel, spawn, provider/I/O, and prepared external waits;
- no external job admission on rejection;
- instruction, transition, deadline, and cancellation limits;
- failure and continuation trace cleanup.

## Task 5: Route procedural macros through the restricted driver

Host compilation retains the existing synchronous path. Expansion inside an
active task uses the restricted driver with the current task context.

Cover runtime-loaded modules, runtime eval, force-time compilation, nested
macro output, helper/HOF/multimethod transformer calls, suspension rejection,
infinite transformers, cancellation, and host compatibility.

## Task 6: Route debugger evaluation through the restricted driver

Compile debugger scratch expressions directly; do not call `eval_callback`.
Snapshot reachable open upvalues against the paused owner and synchronize them
after successful restricted execution.

Cover conditional truth/false, fail-open parse/eval/budget/suspension errors,
locals/globals/upvalues, helper/HOF/multimethod evaluation, prompt suspension
errors, and mutation atomicity for `setVariable`.

## Task 7: Delete bridges and guard host adapters

After Tasks 1-6 are green, delete:

- `CURRENT_VM` and `CurrentVmGuard`;
- `try_run_on_current_vm*` and `run_nested_closure_args`;
- `current_vm_globals` and current-VM snapshot helpers;
- active-runtime fresh-VM closure branches;
- every `suspend_runtime_quantum` call and `QuantumSuspendGuard`.

Keep `eval_callback` and `call_callback*` only as explicit host compatibility
adapters guarded by `!in_runtime_quantum()`. Give `with_stdlib_ctx` an exact
host-only allowlist. Add comment-stripped source guards for every deleted or
restricted symbol.

## Verification

Run focused TDD at each task boundary, then:

```bash
cargo nextest run -p sema-core
cargo nextest run -p sema-vm
cargo nextest run -p sema-eval
cargo nextest run -p sema-lang --test vm_async_test
cargo nextest run -p sema-lang --test dap_async_breakpoint_test
cargo nextest run -p sema-dap
cargo clippy -p sema-core -p sema-vm -p sema-eval -p sema-dap --all-targets -- -D warnings
scripts/check-unified-runtime-legacy.sh --check
scripts/check-unified-runtime-inventory.sh --check
```

Finish with the repository-wide release gate from
`2026-07-19-unified-runtime-terminal-inventory.md`.
