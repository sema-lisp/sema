# VM-backed module loading (`load`/`import`) — design + why the naive approach failed

**Status (2026-06-16):**
- A naive "route both `load` and `import` through the VM" attempt was made, found
  broken by adversarial verification, and reverted (details below).
- **Option B LANDED** (commit `14c2c9f`): `(load ...)` runs on the VM; `(import ...)`
  stays tree-walked (correct isolation). Closes the async-in-`load`ed-files and
  perf gaps. Cache-invalidation + flag-leak fixes included. Tests in
  `crates/sema/tests/vm_module_test.rs`.
- **Option A (VM-backed `import`) remains future work** — needs the per-module
  globals VM change described below.

## Goal

Today `(load ...)` and `(import ...)` always execute the loaded module **body on
the tree-walker**, even when the VM is the active backend: the VM compiler lowers
them to `__vm-load`/`__vm-import` runtime shims (`crates/sema-vm/src/compiler.rs`
~1026/1034) that call back into the tree-walker's `eval_load`/`eval_import`
(`crates/sema-eval/src/special_forms.rs`), which run each top-level form with
`eval::eval_value`.

Consequences:
1. **VM-only features (async/await, channels) don't work in loaded/imported modules** — they run on the tree-walker, which has no async.
2. **Performance** — loaded code runs on the slower tree-walker.
3. **DAP breakpoints in loaded files never hit** — loaded code bypasses the VM debug loop.

We want loaded/imported module bodies to compile + run on the VM (tree-walker only under `--tw`).

## The naive approach (attempted, commit fe86de8 — REVERTED)

Keep all of `eval_load`/`eval_import`'s path/VFS/cache/cycle/export/sandbox logic,
add a `vm_backend` flag to `EvalContext`, and swap only the body-eval loop: on the
VM, compile each top-level form with `compile_program_with_spans` and run it on a
fresh VM whose `globals = Rc::new(env.clone())` (the shared global env for `load`,
an isolated child-of-root `module_env` for `import`). Macro expansion was
parameterized by the body env so module macros register where expansion looks.

This **passed a first test pass but adversarial verification found it is not
correct.** Reverted.

## Why it failed — critical/high findings (adversarial verify, all reproduced)

### 1. (CRITICAL) VM closures carry no per-module globals env
`Closure` (`crates/sema-vm/src/vm.rs`) holds only `func` + `upvalues`; a VM has a
single `self.globals`. A module's exported function compiled against `module_env`
references its private siblings via `GetGlobal`. When the importer later **calls**
that exported closure, it runs inside the *importer's* VM whose globals are the
importer root — so a non-exported helper/constant is `Unbound`.

Repro: `lib.sema` = `(define (private-helper x) (* x 10))` + `(define (public-api x) (private-helper x))`;
`(import "lib.sema" public-api) (public-api 5)` → VM: `Unbound: private-helper`; `--tw`: `50`.
This is the ubiquitous "public API backed by private helpers" pattern and is a
**regression** vs the tree-walker (which lexically captures `module_env` in closures).

### 2. (CRITICAL) async thunk referencing a module-local global fails
Same root cause + the scheduler builds task VMs with its fixed root globals
(`crates/sema-vm/src/scheduler.rs`). An `(async ...)` in an imported module that
touches a module-local define → `Unbound`. This is precisely the headline feature
the change set out to enable, so it must work.

### 3. (CRITICAL) `(module name (export ...))` export list dropped on the VM
`compile_module` (`compiler.rs`) prefixes name/exports with `_` and never calls
`ctx.set_module_exports`, so full `import` of a `(module … (export a) …)` file
exports **every** top-level binding (leaks privates). The tree-walker's
`eval_module` does call `set_module_exports`.

### 4. (HIGH, affects `load` too) inline-cache staleness via decoupled version cell
`Rc::new(env.clone())` shares `bindings` (`Rc<RefCell>`) but clones an independent
`version: Cell<u64>`. The VM's `LoadGlobal` inline cache keys on
`self.globals.version`. A loaded module that redefines a global bumps the *clone's*
version, not the outer VM's, so the outer VM serves a **stale cached value**.
Repro: `(begin (define shared 1) (define (peek) shared) (load <redefines shared>) (peek))`
→ VM returns the stale `1`; `--tw` returns the new value.

### 5. (HIGH) sticky `vm_backend` flag leaks across calls
Single-expr tree-walker entry points (`eval`, `eval_in_global`) never reset the
flag, so a tree-walker `eval` of `(load ...)` after any prior VM call wrongly runs
the loaded body on the VM. (CLI/REPL `--tw` are safe — they route through
`eval_string` which resets.)

### Also found
- `run_bytecode_bytes` (CLI + MCP `.semac`) and the DAP path would set the backend
  flag but never `init_scheduler`, so async in a loaded module from those paths
  errors (and the error text is misleading). (Pre-existing for top-level async in `.semac` too.)
- WASM debugger paths never set the backend flag.
- Loaded module bodies lose the `known_natives` `CallNative` intrinsic fast-path.

## Root cause (the real blocker)

The tree-walker captures the **defining `Env` lexically** in every closure, so a
function always resolves its module's globals regardless of who calls it. The VM
has a **single `self.globals`** and closures do **not** carry a per-module globals
env. Routing `import` (which requires module isolation) through the VM therefore
breaks any exported function that references module-private globals, and the
scheduler's single root-globals compounds it for async.

`load` (everything shares the root global env) does **not** hit the closure-globals
problem, but it does hit the inline-cache version-cell issue (#4).

## Viable paths forward

### Option A — full VM support for per-module globals (correct, large, risky)
Give VM closures a "home globals" `Rc<Env>` (store on `Closure` /
`VmClosurePayload`); on `CALL`, save/restore `self.globals` to the callee's home
globals so `GetGlobal` resolves against the defining module. Thread the same into
the scheduler's task VMs. This is a change to the **hot call path** and interacts
with the per-globals **inline cache** (caches are keyed on a single globals
version) — needs careful design (per-globals cache invalidation or cache keyed by
(globals identity, version)). High regression risk; warrants its own focused effort
with heavy benchmarking + dual-eval verification.

### Option B — `load`-only on the VM, keep `import` on the tree-walker (smaller, correct)
`load` shares the root global env, so no closure-globals problem. Implement only
`load` on the VM and fix the two real issues it has:
- inline cache: don't decouple the version cell — thread the actual `Rc<Env>`
  (the shim has it as `load_env`) instead of `Rc::new(env.clone())`, or
  `bump_version()` the outer env after the loaded body runs.
- flag leak: reset `vm_backend` deterministically at every top-level entry
  (incl. single-expr `eval`/`eval_in_global`), or pass the backend explicitly /
  save-restore around the dispatch.
Keep `import` on the tree-walker (correct isolation). Trade-off: async/channels work
in `load`ed files but **not** in `import`ed modules; `import` perf unchanged.

### Phase 2 (either option) — DAP breakpoints in loaded files
Separate from execution: the loaded body runs on a *separate* VM not attached to
the debug session. Hitting breakpoints requires sharing the active `&mut DebugState`
with the nested module VM (an `unsafe *mut` thread-local — the outer `&mut` is live
but parked; formally aliasing) **or** running loaded bytecode as frames in the
existing VM. Defer until execution is correct.

## Test gaps the first attempt missed (must cover next time)
- selective import where an exported fn calls a **non-exported helper** (and a module-local constant)
- `(module … (export …))` file: full import must NOT leak non-exported bindings
- async thunk inside an **imported** module referencing a module-local global
- global redefined by a `load` then read via a previously-cached global access (cache invalidation)
- single-expr `eval` of `(load …)` after a prior VM call (flag leak)
- async in a loaded module from the `.semac` (`run_bytecode_bytes`) and DAP paths

## Recommendation
Decide between Option A (full, correct `import` on the VM — significant VM work) and
Option B (correct `load`-only now, `import` deferred). The first attempt proved the
naive factor-out is unsafe; either real option needs the fixes above plus the same
adversarial test matrix re-run before landing.
