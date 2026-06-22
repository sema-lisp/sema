# Unify the sema-llm eval callback onto the core callback

**Status:** proposed · **Date:** 2026-06-22 · **Owner:** unassigned

## Problem

`sema-llm` needs to evaluate user code (tool handlers, Lisp-defined provider
`:complete` functions, streaming/event callbacks, `with-*` thunks). Like
`sema-stdlib`, it can't depend on `sema-eval` (that would close the
`sema-eval → sema-llm` cycle), so it calls back into the evaluator through a
registered callback.

The problem is that `sema-llm` carries a **second, redundant** callback mechanism
distinct from the one `sema-core` already provides and that `sema-stdlib` (and
`sema-llm` itself, in 3 places) already use:

| Mechanism | Where | Type | Dispatches to |
| --- | --- | --- | --- |
| Core (canonical) | `sema-core/context.rs` `call_fn`/`eval_fn` | `fn` pointer | `sema_eval::call_value` → VM (`run_nested_closure`) |
| Bespoke (to remove) | `sema-llm/builtins.rs` `EVAL_FN` | `Box<dyn Fn>` | `full_eval` → `simple_eval` fallback |

The bespoke path is:

- **`EVAL_FN`** — a `thread_local! RefCell<Option<Box<dyn Fn>>>` (heavier than the
  core `fn` pointer), set by `sema_llm::builtins::set_eval_callback`.
- **`full_eval`** — reads `EVAL_FN`, or falls back to `simple_eval`.
- **`simple_eval`** — a degraded mini-evaluator handling only symbols and calls
  (no `let`/`if`/`cond`); used silently if `EVAL_FN` was never registered.
- **`call_value_fn`** — a hand-rolled re-implementation of function application:
  it manually creates a child `Env`, binds params + rest, sets the self-reference,
  and evals the body via `full_eval`.

`call_value_fn` is used at **15 call sites**; the shared `sema_core::call_callback`
at only **3**.

### Why this matters (beyond dedup)

`call_value_fn` binds lambda params into a plain `Env` and evals the body directly,
**bypassing the VM's closure machinery**. The canonical `sema_core::call_callback`
→ `sema_eval::call_value` routes closures through `run_nested_closure` /
`CURRENT_VM` (see `crates/sema-vm/src/vm.rs`), which is what makes `set!` through a
callback, captured upvalues, and async/yield inside a callback behave correctly.
This is the documented dual-path hazard (`vm_closure_dual_path`): anything
yield-aware or mutation-aware must go through the in-VM path. So tool handlers,
streaming callbacks, and Lisp-provider functions invoked via `call_value_fn` may
silently diverge from the same code invoked via a stdlib HOF.

Consolidating is therefore **correctness work, not just cleanup**: it removes the
divergent path and the silent `simple_eval` degradation (a partially-wired
interpreter would mis-evaluate instead of erroring).

## Target

One callback. Every "call a user function value" in `sema-llm` goes through
`sema_core::call_callback`; every "evaluate an expression" (none remain after the
migration — see below) would go through `sema_core::eval_callback`. Delete
`EVAL_FN`, `set_eval_callback`, `full_eval`, `simple_eval`, and `call_value_fn`,
plus the now-dead `sema_llm::builtins::set_eval_callback(...)` registration calls.

### Precondition (already true — verify in step 0)

`sema_core::call_callback` dispatches through `ctx.call_fn`, registered as
`sema_eval::call_value` in `crates/sema/src/lib.rs:112`,
`crates/sema-eval/src/eval.rs:61,81`, and `crates/sema-wasm/src/lib.rs`. Every code
path that registers `sema-llm` builtins also registers `call_fn`, so after the
migration there is no path where llm callbacks run without the canonical evaluator
available.

## The 15 call sites (all `call_value_fn(ctx, f, args)`)

| Line (approx) | Context | Args |
| --- | --- | --- |
| 501 | Lisp-defined provider `:complete` fn | `[request_map]` |
| 1598, 1621 | `llm/stream` on-chunk callbacks | `[string chunk]` |
| 2304 | per-item function (batch/pmap) | `[item]` |
| 3395, 3482, 3507, 3821 | `with-budget` / `with-rate-limit` / `with-cache` / fallback thunks | `[]` |
| 4897, 4950 | agent event callbacks (`:on-tool-call`, …) | `[event_map]` |
| 5013 | **tool handler dispatch** (agent loop) | `sema_args` |

(5121/5144 are internal to `full_eval`/`simple_eval` and disappear with them.)

## Plan (TDD, one commit per step, suite green throughout)

**Step 0 — Characterize current behavior (no code change).**
Add focused regression tests that pin the *correct* expected behavior of each
callback kind, so the migration is differential:
- `llm_fake_test.rs`: a `deftool` handler that uses `let`/`if`/`cond` and returns a
  computed value; an agent run (FakeProvider scripted tool call) asserting the tool
  result the runtime built. A Lisp-defined provider whose `:complete` uses non-trivial
  forms.
- `vm_async_test.rs`: a tool handler / `with-*` thunk that performs an async op
  (channel/sleep) — pins that async works through the callback.
- A `set!`-through-callback case: a tool handler that mutates an outer `atom`/binding
  via `set!`, asserting the mutation is observed (this is the dual-path bug class).
Run; confirm green on the **current** code (these encode the behavior we must keep;
the async/`set!` ones may already expose divergence — if any fail now, note them as
bugs the migration fixes).
Commit.

**Step 1 — Route `call_value_fn` through the core callback.**
Replace the body of `call_value_fn` with a thin shim: `sema_core::call_callback(ctx,
func, args)`. Leave the 15 call sites untouched for now. This isolates the behavior
change to one function. Run Step-0 tests + `cargo test -p sema --test llm_fake_test`
+ `vm_async_test` + `otel_agent_test`. Fix fallout. Commit.

**Step 2 — Inline and delete the shim.**
Replace each of the 15 `call_value_fn(ctx, f, args)` call sites with
`sema_core::call_callback(ctx, f, args)`; delete `call_value_fn`. Run the suite.
Commit.

**Step 3 — Delete the dead eval indirection.**
Remove `EVAL_FN`, `full_eval`, `simple_eval`, `sema_llm::builtins::set_eval_callback`,
and the `EVAL_FN` line in `reset_runtime_state`. Remove the now-dead
`sema_llm::builtins::set_eval_callback(...)` calls in `crates/sema-eval/src/eval.rs`
(lines ~69, 87) and `crates/sema/src/lib.rs` (~120). Confirm nothing else references
them (`grep`). `cargo build` + `cargo clippy --all-targets -- -D warnings`. Commit.

**Step 4 — Full verification.**
`cargo test --workspace && make examples && make smoke-bytecode && make lint`.
Run an end-to-end live agent smoke (one cheap real provider, e.g. `claude-haiku`)
with a multi-step tool loop to confirm tool dispatch + `set!`/async inside a handler
behave correctly through the unified path. Commit (if any test files added).

## Acceptance gate

- `cargo test --workspace` green, including new Step-0 regression tests.
- `grep -rn "EVAL_FN\|call_value_fn\|full_eval\|simple_eval" crates/sema-llm` returns
  nothing.
- `grep -rn "sema_llm::builtins::set_eval_callback" crates/` returns nothing.
- Clippy clean; wasm still builds (the core `call_fn` is registered in
  `sema-wasm` too).
- The `set!`-through-tool-handler and async-in-tool-handler tests pass — proving the
  llm path now shares the VM closure semantics.

## Risks / watch-outs

- **In-VM vs fallback dispatch.** `call_value` has both an in-VM path (when invoked
  from a running VM via `CURRENT_VM`) and a fallback. Tool handlers/streaming run
  *inside* a live agent/run on the VM, so the in-VM path should engage — verify the
  streaming-callback case specifically (it runs mid-export of a provider stream).
- **Behavior change is the point.** If a Step-0 test of `set!`/async fails on current
  code but passes after Step 1, that's a *fix*; document it in the commit + CHANGELOG.
- **CLAUDE.md mandate:** any agent-loop/retry/provider change needs a FakeProvider
  test — Steps 0/4 satisfy this. Don't skip the live smoke.
- **Scope discipline:** do not refactor `sema-stdlib`'s callback usage in the same
  pass; it already uses the core path correctly.

## Rollback

Each step is an independent commit; revert the last green commit if Step 4 surfaces a
regression that can't be fixed forward. The Step-0 tests stay regardless (they're
valuable even if the migration is deferred).

## Not in scope

- The `sema-eval → sema-stdlib`/`sema-llm` crate cycle itself stays. The callback
  inversion is the correct, zero-overhead solution; this plan only removes the
  *duplicate* mechanism inside `sema-llm`, not the inversion pattern.
