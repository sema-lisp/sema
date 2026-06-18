# Plan: Retire the tree-walker (VM becomes the only evaluator)

> ## ⏩ SESSION HANDOFF — start here next time
> **Decision (locked 2026-06-18):** retire the tree-walker; do not maintain two evaluators.
> Every TW-backed feature must become VM-native first.
>
> **Progress (2026-06-18, Block 1 nearly done — all green, committed per milestone):**
> - ✅ **M1 — Closure home-globals.** `vm::Closure.globals: Option<Rc<Env>>`; the dispatch
>   loop captures the frame's home globals per activation; global opcodes resolve against it.
>   Commit `dc86709`.
> - ✅ **M2 — VM macro expansion.** `apply_macro_vm` (compile+run transformer on the VM, no
>   cache, rooted at caller_env); `expand_macros_in`/`__vm-macroexpand` repointed;
>   `expand_for_vm_in` defmacro + `load_prelude` register via `register_defmacro` (TW-free).
>   Commit `39dea23`.
> - ✅ **M3 — VM eval bridge.** `__vm-eval` macro-expands + compiles + runs on the VM (async
>   inside `(eval ...)` proves it); `apply` already VM-native via call_callback. Commit `97d0394`.
> - ✅ **M4 — import on the VM.** Added `vm::Closure.functions` (home function table); the
>   dispatch loop restores `self.functions` per frame (fixes the cross-table MakeClosure bug);
>   `eval_import` runs module bodies on the VM rooted at the isolated module_env. Adversarial
>   tests (name collision, interleaved fns+map, async-in-module) pass. Commit `2ea190e`.
> - ✅ **pre-M5 cleanup:** `__vm-define-record-type` + `__vm-defmacro-form` now call the pure
>   destructures directly (no eval_callback). `__vm-force` was already VM-native (delay lowers
>   to a lambda → call_callback). Commits `b72e966`, `b16cf68`.
>
> - ✅ **M5 — eval bridge complete.** `__vm-deftool`/`__vm-defagent` build the Tool/Agent value
>   directly (via `special_forms::register_tool`/`register_agent`); sema-llm callback →
>   `eval_value_vm`; `__vm-load`/`__vm-import` call the drivers directly (off `eval_step`).
>   Commits `5b907bb`, `6c3d45f`.
> - ✅ **M6 — VM is the sole evaluator (all paths).** All `Interpreter` eval entry points run on
>   the VM (`run_exprs_on_vm`; deliberate change: all run in the global env / persist defines).
>   `sema::Interpreter` builder fixed (registers delegates+prelude); `sema::Interpreter` +
>   sema-wasm `preload_module` run on the VM. Commits `ecacecd`, `f3f2397`.
> - ✅ **M7 — tests on the VM.** dual_eval/integration/http/server all run on the VM; fallout
>   fixed: real bugs fixed (`(module (export …))` restriction on VM `f8377e1`; nested-import
>   leak via `current_vm_globals` in `ecacecd`; VM stack-overflow recursion hint); TW-only
>   error-UX tests `#[ignore]`d as a deferred-parity acceptance suite; obsolete TW-contrast
>   tests retired; new `embedding_api_test`.
> - ✅ **M8 (surface) — `--tw` retired** to a hidden no-op; eval-mode functions always use the VM.
>   Commit `0290a4a`.
> - ✅ **M9 — docs** reframed to a single VM evaluator (CLAUDE.md, website/docs, deferred.md).
>   Commit `cfe6cfe`.
>
> **STATUS: the tree-walker is FUNCTIONALLY RETIRED — the bytecode VM is the sole evaluator for
> every path (CLI, REPL, embedding API, wasm, tests, notebook, mcp). Full suite green; lint
> clean. `eval_value` has no live callers (the TW core is unreachable dead code).**
>
> **Remaining (cleanup, tracked in `docs/deferred.md`):**
> - **TW-3:** physically delete the dead TW source (`eval_value`/`eval_step`/trampoline/
>   `apply_lambda`/`run_trampoline`/eval-based `apply_macro`/`eval_string`; pure-eval special
>   forms + `try_eval_special`, keeping the drivers). Relocate `SPECIAL_FORM_NAMES` out of
>   `special_forms.rs` first (REPL + all of sema-lsp import it). Flip `debug_evaluate` (DAP) +
>   the `force`/thunk fallback off `eval_callback`. Needs a per-special-form VM-native-vs-TW
>   audit (e.g. `defmulti`/`defmethod`).
> - **TW-1:** VM stack-trace parity (8 `#[ignore]`d acceptance tests ready).
> - **TW-2:** `(type lambda)` → `:native-fn` (wants a `sema-core` NativeFn marker).
> - **Perf:** the hot dispatch loop now does 2 Rc clones per frame activation (frame_globals +
>   self.functions) — verify with `make bench-vm`; optimize if regressed.
> - **CORE-2 is NOT solved by this** — recursive-closure Rc cycle persists on the VM (GC effort).

**Status:** scoping / not started. **Phase-1a feasibility gate: PASSED** (spike 2026-06-18). **Size:** large, multi-phase (not a single PR).

## Phase-1a spike result (2026-06-18) — macro expansion on the VM is feasible
A macro is `{params, body}` with no captured env, and `apply_macro` is exactly
`((fn (params… . rest) body…) 'arg…)` (args passed as data). Ran that shape on the VM
for the hard cases — all correct: list/cons transformers, quasiquote + `unquote-splicing`
in the body, transformer bodies calling global helpers, and auto-gensym (`tmp#` →
consistent `tmp__0`). Real `defmacro`+use is already end-to-end green on the VM. So the
make-or-break subsystem is validated: **no standalone "mini tree-walker" is needed** — the
VM can apply macro transformers directly.

**Goal:** the bytecode VM is the sole evaluator. Remove the tree-walking interpreter,
the `--tw` flag, the dual-backend test harness, and all "two backends" framing in
the docs.

## The reality check (why this is bigger than "delete sema-eval")

The VM is **not** independent of the tree-walker today. It borrows it for four
subsystems — all of which must get VM-native (or standalone) replacements *before*
the tree-walker can be deleted:

1. **Macro expansion** — `Interpreter::expand_for_vm` (`crates/sema-eval/src/eval.rs:196`)
   evaluates `defmacro` bodies **via the tree-walker** (`eval_value`) to register macros,
   then expands macro calls, before handing forms to `sema_vm::compile_program`. The VM
   has no macro evaluator of its own. **This is the critical-path blocker.**
2. **Module `load` / `import`** — `__vm-load` / `__vm-import` delegate back to the
   tree-walker's `eval_load` / `eval_import` through the eval callback
   (`eval.rs:899-935`).
3. **The stdlib eval/call bridge** — `set_eval_callback(eval_value)` /
   `set_call_callback(call_value)` (`eval.rs:82-83`) point at tree-walker functions.
   Stdlib HOFs, the `eval`/`apply` runtime path, and async callbacks call through these.
   (C1 routes HOF callbacks into a running VM when present, but the registered targets
   and the no-VM fallback are still the tree-walker.)
4. **Prelude loading** — `load_prelude` (`eval.rs:890`) evaluates the Sema prelude at
   startup via the tree-walker.

So the work is: **make the VM self-sufficient in (1)–(4), migrate every consumer and
test off the tree-walker, then delete it.**

## Full coupling inventory (grounded in the tree at this commit)

### Code
- `crates/sema-eval` — hosts the tree-walker (`eval.rs` `eval_value`/trampoline,
  `special_forms.rs`) **and** shared machinery we must keep or re-home: the
  `Interpreter` struct, the module system (`EvalContext` cache, `eval_load`/`eval_import`),
  prelude loading, macro registration, and the eval/call callback wiring.
- CLI: `--tw` flag (`crates/sema/src/main.rs:155,330`) + `eval_with_mode*` dispatch;
  REPL `use_vm` mode (`crates/sema/src/repl/commands.rs`).
- Public/embedding API: `Interpreter::eval`, `eval_str`, `eval_string`, `eval_in_global`
  are tree-walker entry points (`eval.rs:117-137`). Used by embedders (see
  `website/docs/embedding.md`, `embedding-js.md`).
- Consumers still on the tree-walker:
  - **sema-wasm** — the playground "Tree" engine toggle calls `eval_string` (TW)
    (`lib.rs:1590,1640,1743`); other paths use `eval_str_compiled` (VM). Removing TW
    removes the playground's Tree/VM switch.
  - **sema-dap** — `Interpreter::new()` then debug executes on the VM; verify no TW eval.
  - Already VM-only: **sema-notebook** (`eval_str_compiled`), **sema-mcp**
    (`eval_str_compiled` / bytecode).

### Tests (the big migration)
- `crates/sema/tests/common/mod.rs`: `eval_tw` (= `eval_str`, TW) and `eval_vm`
  (= `eval_str_compiled`, VM); `dual_eval_tests!` / `dual_eval_error_tests!` generate a
  `_tw` and a `_vm` case each.
- Dual-eval suites (~1,487 cases across 9 files): collections 264, core 180, data 67,
  ergonomic 25, io 86, map 147, stdlib 196, `dual_eval_test` 455, types 67.
- **`integration_test.rs` (~1,025), `http_test.rs`, `server_test.rs`** — their local
  `eval()` helper uses `eval_str` → **tree-walker**. These all run on the TW today.

### Docs
- 22 `--tw` mentions across `cli.md`, `dap.md`, `embedding-js.md`,
  `language/macros-modules.md`, `language/special-forms.md`, and several `internals/*`.
- Tree-walker is described as a backend (and `internals/evaluator.md` is *about* it) in
  `internals/architecture.md`, `evaluator.md`, `bytecode-vm.md`, `performance.md`,
  `lisp-comparison.md`. CLAUDE.md also documents the dual-eval testing model.

## Phased plan (dependency-ordered)

**Phase 0 — Architecture decision.** Where do macro expansion, module load/import,
prelude loading, and the `Interpreter` API live once the TW is gone? Likely: a slimmed
`sema-eval` that owns macro expansion + module system + `Interpreter`, implemented on the
VM; or fold these into `sema-vm`. Decide crate boundaries first.

**Phase 1 — Make the VM self-sufficient (the hard engineering).**
- 1a. **Macro expansion without the TW.** *(Approach validated by the spike.)* Replace
  the `eval_value` call in `apply_macro` with a VM run of the transformer
  `((fn (params… . rest) body…) 'arg…)` (or compile the body with params bound), and
  replace `eval_defmacro`'s TW registration with a pure destructure (it does no real
  evaluation today). Cache the compiled transformer per macro (compile once, run per call
  site). Recurse on the result for nested/recursive expansion (as `expand_macros_in`
  already does). Preserve `macroexpand`, auto-gensym, and prelude macros.
- 1b. **`load`/`import` on the VM** — replace the `__vm-load`/`__vm-import`→TW delegation
  with VM-native module evaluation (the load-on-VM groundwork exists; finish import).
- 1c. **eval/call bridge → VM** — repoint `set_eval_callback`/`set_call_callback` (and the
  `eval`/`apply` runtime) at VM-based implementations; keep C1's in-VM HOF routing.
- 1d. **Prelude on the VM** — load the prelude via the VM at startup.

**Phase 2 — Switch consumers to the VM.** Embedding API (`Interpreter::eval*` → VM),
sema-wasm (drop the Tree toggle; all eval → VM), confirm dap/lsp.

**Phase 3 — Migrate tests.** Collapse `dual_eval_*` to single-backend VM tests (rework the
macros to emit one VM case, keep the literal expected values from the eval-tw work as the
oracle — we lose the TW/VM differential, so literal expectations become the correctness
anchor). Switch `integration_test`/`http`/`server` `eval()` to the VM; fix fallout
(genuinely TW-only behaviors, if any, surface here).

**Phase 4 — Delete the tree-walker.** Remove `eval_value`/trampoline + TW `special_forms`
paths, `--tw`, REPL `use_vm`, `eval_tw`. **CORRECTION (Opus review):** this does **NOT**
close CORE-2. It removes the TW's whole-`Env`-capture cycle, but the VM has its *own*
Rc cycle — a local/returned recursive closure captures its own name as an upvalue cell
whose `Closed(Value)` holds the closure (`resolve.rs:280-297`; see
`2026-02-16-compilation-strategy-investigation.md:1014-1016`). CORE-2 remains open and its
real fix is cycle collection / a GC (see `docs/deferred.md`). Also: relocate the TW-free
`SPECIAL_FORM_NAMES` constant out of `special_forms.rs` before deleting it (the REPL and
all of `sema-lsp` import it), and gate deletion on a grep that `eval_value` has no
remaining callers.

**Phase 5 — Docs.** Remove all `--tw`; rewrite/retire `internals/evaluator.md`; reframe
architecture/performance/lisp-comparison/bytecode-vm and CLAUDE.md to a single-evaluator
story.

## Risks / open questions
- ~~**Macro expansion is the make-or-break.**~~ **RESOLVED by the 2026-06-18 spike** — the
  VM applies macro transformers directly (incl. quasiquote/splicing, helpers, gensym). No
  standalone macro interpreter needed.
- **Loss of differential testing.** Dual-eval's value was catching TW↔VM divergence.
  Going VM-only, literal expected values (already started for foundational ops) and
  fuzzing become the correctness strategy.
- **Embedding-API break.** `Interpreter::eval` semantics change backend; document as a
  breaking change for embedders; bump major-ish.
- **Behaviors that only ever worked on the TW** (e.g. the CORE-2 returned-mutual-recursion
  corner) will be gone — acceptable, but inventory during Phase 3.

## Recommendation
The Phase-1a feasibility gate has **passed** — the make-or-break (macro expansion) works on
the VM. The remaining work is large but mostly mechanical. Proceed as separate PRs in
dependency order: 1b/1c/1d (load/import, eval-call bridge, prelude on the VM) → 1a
(wire VM macro expansion) → 2 (consumers) → 3 (tests) → 4 (delete TW) →
5 (docs). Land 1a–1d behind the existing dual-eval suite so parity is proven before any
deletion.

## Roadmap (milestones)

Decision (2026-06-18): retire the TW regardless — we will not maintain two evaluators.
Every TW-backed feature must become VM-native first. Ordered, PR-sized milestones; each
lands **behind the dual-eval suite** (parity is the safety net) until M7 collapses it.
Effort/risk are relative (S/M/L · Low/Med/High).

**Block 1 — make the VM self-sufficient (removes the TW round-trips):**
- **M1 · Closure home-globals** — add `globals: Rc<Env>` to `Closure`; `GetGlobal/SetGlobal`
  resolve against the closure's home env; update `MakeClosure` + scheduler task VMs. *The
  foundational enabler for import isolation.* Gate: full dual-eval + `vm_module_test` green.
  **L · High** (touches hot VM dispatch).
- **M2 · VM macro expansion (1a + 1d)** — expand `defmacro` via the VM, no bytecode cache
  (gensym hygiene), rooted at `caller_env`, cache (if any) thread-local in `sema-eval`.
  Absorbs the prelude (macro-only). Gate: dual-eval macro tests + absolute prelude-oracle
  tests, on a build where the macro path no longer calls `eval_value`. **M · Med.**
- **M3 · eval/call bridge → VM (1c)** — repoint `__vm-eval` + `debug_evaluate` + sema-llm's
  callback; VM call-callback for NativeFn/Keyword/MultiMethod. Gate: `server_test` router
  (HOF closures) + meta `eval`/`apply` green without TW. **M · Med.**
- **M4 · import on the VM (1b)** — depends on M1: `__vm-import`/`__vm-load` resolve+compile+
  run module bodies on the VM rooted at module globals, copying exports. Gate: `vm_module_test`
  isolation + module suite green; no `__vm-*` routes through `eval_value`. **M · Med.**
- **M5 · Phase-1 completion gate** — verify `set_eval_callback`/`set_call_callback` point at
  VM impls and `eval_value` has no callers on any live path. Checkpoint, not new code.
  **S · Low.** *Everything below is blocked on M5.*

**Block 2 — migrate, delete, document:**
- **M6 · Public API + consumers (2)** — fix `sema::Interpreter` builder (delegates+prelude);
  respect child-env vs global-env define-persistence; route eval entry points to the VM;
  drop the sema-wasm Tree toggle; confirm dap/lsp. Breaking change for embedders (note it).
  **M · Med.**
- **M7 · Test migration (3)** *(parallelizable — ultracode by file)* — flip integration/http/
  server `eval()` to the VM; collapse `dual_eval_tests!`→`vm_eval_tests!` (keep the literal
  oracle); upgrade legacy error tests; audit VM-vs-TW error wording. Gate: full suite green
  VM-only. **L · Med.**
- **M8 · Delete the TW (4)** — relocate `SPECIAL_FORM_NAMES` out of `special_forms.rs` first;
  grep-gate no `eval_value` callers; delete `eval_value`/trampoline + TW `special_forms`;
  remove `--tw` + REPL `use_vm` + `eval_tw`. Gate: workspace builds + full suite green.
  **M · Low** (mechanical once gated).
- **M9 · Docs (5)** *(parallelizable)* — remove 22 `--tw` mentions; rewrite/retire
  `internals/evaluator.md`; reframe architecture/performance/bytecode-vm/lisp-comparison +
  CLAUDE.md to a single evaluator. **S–M · Low.**

**Orthogonal:** CORE-2 (recursive-closure Rc cycle) is **not** addressed by this roadmap —
it persists on the VM and is a separate GC/cycle-collector effort (`docs/deferred.md`).
