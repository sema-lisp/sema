# Self-tail-call optimization (issue #62)

Compile self-recursive tail calls without self-capture, eliminating the
named-let per-entry self-referential closure cycle (the CORE-2 / ADR #66 cycle
shape). A closure that references its own letrec-bound name **only** as the
operator of a tail call never needs to capture itself as an upvalue: the
running frame already holds its own `Rc<Closure>`.

## Baseline (verified in this worktree)

Pipeline: `lower.rs` (S-expr → CoreExpr) → `optimize.rs` (AST const-fold) →
`resolve.rs` (slots/upvalues) → `compiler.rs` (bytecode) → `vm.rs`.

- Named let `(let loop ((x 0)) … (loop …))` lowers to
  `Letrec{[(loop, Lambda{name:loop,…})], body:[Call{Var(loop), inits, tail}]}`
  (`lower.rs:722`). The lambda body is lowered with `tail=true`, so the
  self-call is `Call{tail:true}`.
- `resolve_letrec` (`resolve.rs:555`) gives `loop` a **Local slot** in the
  enclosing function. `resolve_lambda` resolves the body; the self-reference
  becomes `UpvalueDesc::ParentLocal(loop_slot)` + reference site
  `Upvalue{index}` (`resolve_upvalue`, `resolve.rs:188`).
- `make_closure` (`vm.rs:3104`) captures `loop_slot` as an **Open**
  `UpvalueCell`; the letrec store writes the new closure into that same slot; on
  frame exit `close_open_upvalues` snapshots it `Closed(closure)` → the
  `Rc<UpvalueCell> ⇄ Rc<Closure>` cycle.
- Zero-upvalue exemption already exists (`vm.rs:3288`,
  `let candidate = (n_upvalues > 0).then_some(&native_rc)`): a closure with no
  upvalues is not registered as a cycle candidate.

## Design

Analyze the **resolved** lambda (scoping/shadowing/tail already correct), and if
it qualifies, rewrite self-references to a new `VarResolution::SelfFn`, drop the
now-unused self upvalue, and emit a dedicated `SelfTailCall argc` opcode that
reuses `frame.closure` directly.

Doing the analysis post-resolution is sound by construction: the resolver has
already turned a shadowing binding into a `Local` (so it is *not* the self
upvalue), tracked tail position on `Call`, and produced the upvalue list. No
hand-rolled scope/shadow logic needed.

### Qualification (per Letrec binding `(loop@Local{slot}, Lambda)`)

Let `self_uv` = the index `i` with `lambda.upvalues[i] == ParentLocal(slot)`.
If no such upvalue exists, the loop name is never referenced from the body →
nothing to do. Otherwise the binding qualifies iff **every** occurrence of
`Upvalue{self_uv}` in the lambda body is the `func` of a `Call{tail:true}`, AND:

- never `set!` through it (`Set(Upvalue{self_uv}, …)`),
- never captured by a nested lambda
  (no nested `Lambda` whose `upvalues` contains `ParentUpvalue(self_uv)`),
- never used as a plain value (arg, returned, non-tail-call func).

All-or-nothing: a single disqualifying use keeps the real self-capture.

### Rewrite (only when qualified)

Produce a new `LambdaDef` with `upvalues`/`upvalue_names` minus element
`self_uv`, and body rewritten so that:

- each `Call{func: Var(Upvalue{self_uv}), tail:true}` → `func` becomes
  `Var(SelfFn)`;
- each `Var`/`Set` `Upvalue{i}` with `i > self_uv` → `i-1` (index shift);
- each **direct** nested `Lambda`'s upvalue descriptor `ParentUpvalue(i)` with
  `i > self_uv` → `i-1` (do not recurse into nested lambda *bodies* — their
  var-refs index their own upvalue lists).

Applies to any `Letrec` binding, covering named-let and explicit self-recursive
`letrec`. Mutual recursion is handled naturally: only the binding's *own* self
upvalue is touched; cross-references stay real upvalues.

### New opcode `SelfTailCall argc` (= 69)

No callee on the stack — only `argc` args. `stack_effect`:
`pops = argc, pushes = 0, exits_frame = true` (contrast `TailCall`: `argc+1`).
VM handler mirrors `TAIL_CALL` and calls `self_tail_call(argc)`:

```
closure = frame.closure.clone()   // already the correct self Rc
arity/has_rest/n_locals from closure.func
arity check (→ arity SemaError, routed through handle_exception)
src = stack.len() - argc          // args (no callee slot)
base = frame.base
close_open_upvalues(frame.open_upvalues, stack, base)  // other captures still need closing
copy_args_to_locals(stack, base, src, arity, argc, has_rest)
stack.resize(base + n_locals, nil)
frame.pc = 0; frame.open_upvalues = None
// closure/cache_base unchanged (same function)
```

Correct because `SelfTailCall` is emitted only inside the loop lambda's own
compiled `Function`, which executes only as that closure's frame — so
`frame.closure` is always the right self, per named-let entry.

## Touch-points

1. `opcodes.rs` — `Op::SelfTailCall`; `from_u8` (69); `stack_effect`
   (`pops:operand, pushes:0, exits_frame:true`); `_assert_all_ops_covered` arm;
   `mod op::SELF_TAIL_CALL`.
2. `core_expr.rs` — `VarResolution::SelfFn`.
3. `resolve.rs` — after each resolved Letrec binding, `try_self_tail_optimize`.
4. `compiler.rs` — `compile_call`: `Var(SelfFn)` + tail → args + `SelfTailCall`;
   `compile_var_load`/`store` reject `SelfFn` (safety net); add `SelfTailCall`
   to `patch_closure_func_ids` (u16 skip) and the `extract_ops` test helper.
5. `disasm.rs` — `op_name` arm (exhaustive; forced) + `Call|TailCall` decode arm.
6. `serialize.rs` — `advance_pc` (pc+3 group) + `stack_effect_operand` (read_u16_at(1)).
7. `vm.rs` — dispatch arm `op::SELF_TAIL_CALL` + `self_tail_call(argc)`.
8. `website/docs/internals/bytecode-format.md` — document opcode 69.

## Tests (TDD)

- resolve: named-let self-only → SelfFn + upvalue dropped; escaping loop name
  (passed to `map`, returned, `set!`, non-tail call, captured by inner lambda)
  → unchanged real upvalue.
- compile/disasm: named-let emits `SelfTailCall`, no `LoadUpvalue` of self.
- eval correctness: counters, accumulators, mutual recursion, rest-arg self
  call, wrong-arity self call → arity error, nested named-lets, shadowed loop
  name, loop name used as value.
- serialize round-trip of a chunk containing `SelfTailCall`.
- GC: 100-rep pure counter named-let births no cycle candidate / no leak
  (extend `gc_stress_test.rs`).
- `make bench-vm` closure + numeric suites; `gc_stress_test` stays green.
