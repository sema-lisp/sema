# Cranelift native code generation (`sema-codegen`)

Status: phase 1 landed (JIT, immediate-only numeric subset). Phases 2 and 3 open.

## Problem

The bytecode VM spends most of a numeric loop's time on dispatch, not on
arithmetic. Each `(+ a b)` costs an opcode fetch, a bounds-checked stack access,
a tag test, and a `Value` construction, for one hardware `add`. Loop-heavy and
recursive numeric code pays that overhead on every operation.

## Why Cranelift

- **Pure Rust.** It joins the workspace as an ordinary Cargo dependency: no C++
  toolchain, no LLVM, and it cross-compiles with everything else.
- **NaN boxing maps onto registers.** A `Value` is a `u64`, so a compiled
  function takes and returns plain `I64`s and every type test is one `band` plus
  one `icmp`.
- **No tracing GC.** Sema refcounts with `Rc`, so there are no stack maps and no
  GC safepoints to emit — the two things that make custom backends hard for Go,
  Java, and OCaml.
- **One backend for JIT and AOT.** `cranelift-jit` emits into memory for the
  REPL and script runner; `cranelift-object` emits `.o` files for a future
  native `sema build`. Both consume the same IR.

## Design

### Where it attaches

```
Sema source → reader → lower → resolve → compile → bytecode Chunk
                                                        │
                                            ┌───────────┴───────────┐
                                            ▼                       ▼
                                      bytecode VM            sema-codegen
                                    (sole evaluator)      (decode → CLIF → JIT)
                                            ▲                       │
                                            └── bail ───────────────┘
```

The VM stays the sole evaluator. `sema-codegen` compiles from **bytecode**, not
from the AST, so it attaches at one place and works for `.semac` files too.

The seam is `sema_vm::jit`, which owns the calling convention and the
per-`Function` compile state; `sema-codegen` implements its `JitBackend` trait.
The dependency runs `sema-codegen → sema-vm`, so the VM never learns about
Cranelift.

Two call sites hook it, both non-tail: `call_vm_closure` (`Call`, `CallGlobal`)
and `call_vm_closure_direct` (`CallSelf`). When compiled code handles the call,
no VM frame is pushed — the result replaces the arguments on the stack and the
dispatch loop resumes in the caller's frame. Tail calls are not hooked; a
self-tail-call becomes a loop *inside* the compiled function, so a compiled loop
is still entered exactly once.

### The immediate-only rule

This is the property everything else rests on.

A `Value` is *immediate* when its 8 bytes hold the whole value: floats, nil,
booleans, small ints, chars, symbols, keywords (tags `0..=6`). Every other tag
stores an `Rc` pointer whose refcount the holder owns.

Compiled code handles values as raw `u64` and performs **no refcount work at
all**. That is sound only because two guards keep every `Rc` away from it:

1. **Entry guard** (`sema_vm::jit::try_execute`) — every argument must satisfy
   `Value::is_immediate`. One list, string, or bignum argument and the call goes
   to the VM.
2. **Producer guard** (the opcode whitelist in `decode.rs`, plus per-operation
   bails in `translate.rs`) — compiled code may only produce immediates.
   Constants baked into the code are checked to be immediate; every operation
   that could allocate bails instead.

With both in force, every bit pattern crossing the boundary is a value whose
`Clone` and `Drop` are no-ops, so rebuilding a `Value` from the returned bits
transfers no ownership. `sema_core::value::immediate_tests` pins the
classification against the tag constants in both directions.

### Bailing out

Compiled code returns `JIT_BAIL` (`NAN_TAG_MASK`: a boxed value with tag 63 and
an empty payload — no `Value` constructor uses tag 63) to mean "a guard failed,
run this call yourself". The VM then executes the call normally, from the start.

Re-running from the start is correct because the whitelisted subset admits no
global store, no mutation, no upvalue capture, no call other than self-recursion,
no `throw`, and no I/O. A compiled call abandoned halfway has changed nothing
observable.

Operations that bail:

| Case | Why |
|---|---|
| Fixnum result outside the 45-bit range | the VM promotes to a bignum, which allocates |
| Integer `/` that is not exact | the VM produces an exact rational, which allocates |
| Division or `modulo` by zero | raising is the VM's job |
| A non-numeric operand | `+` on strings, `<` on strings, structural `=` |
| Float `modulo` | CLIF has no float remainder instruction |
| Non-tail self-recursion past `JIT_MAX_DEPTH` | see below |

### Value model in CLIF

| operation | CLIF |
|---|---|
| `is_int(v)` | `(v & NAN_TAG_MASK) == NAN_INT_SMALL_PATTERN` |
| `is_float(v)` | `(v & NAN_BOX_MASK) != NAN_BOX_MASK` |
| unbox int | `((v & NAN_PAYLOAD_MASK) << 19) >> 19` (arithmetic) |
| box int | range-check via `(x << 19) >> 19 == x`, then `\|` the tag pattern |
| box float | `bitcast`, then `select` the canonical NaN when the result is NaN |
| truthiness | `v != NIL && v != FALSE` |

Two details that are easy to get wrong and are pinned by tests:

- **NaN canonicalization is mandatory.** Hardware's default NaN is
  `0xFFF8_0000_0000_0000`, which is bit-identical to `Value::NIL`. Without the
  `select`, `(/ 0.0 0.0)` would silently return nil.
- **Ordering follows the VM, not IEEE.** The VM builds `Ge(a,b)` as `!(a < b)`
  and `Le(a,b)` as `!(b < a)`, so both answer `#t` when an operand is NaN.
  Compiled code reproduces the same swap-and-invert rather than using
  `FloatCC::GreaterThanOrEqual`.

Mixed int/float arithmetic promotes to `f64`. A 45-bit int converts to `f64`
exactly, so mixed comparison stays faithful to the VM's exact `cmp_int_float`
without needing it.

### Multiplication overflow

A 45-bit by 45-bit product can exceed `i64`, and the low half alone can alias
back into the fixnum range (`2^64 + 5` reads as `5`). Compiled `*` therefore
checks `smulhi(x, y) == lo >> 63` before applying the 45-bit range check.

### Control flow

`decode.rs` decodes the chunk, rejects any opcode outside the whitelist, finds
jump targets, and propagates the operand-stack depth from the entry point to
every reachable instruction. A merge point reached at two different depths is a
rejection, not a translation — the whitelisted control flow never produces one,
and the check keeps a future compiler change from silently breaking the
translator.

Each jump target becomes a CLIF block whose parameters carry the operand stack.
Locals become CLIF variables, so SSA construction is Cranelift's problem.

A self-tail-call rebinds the parameter variables, nils the remaining local slots
(matching `VM::self_tail_call`'s frame resize), and jumps to the block at pc 0 —
so a tail-recursive loop compiles to an actual machine loop with no call at all.

### Recursion depth

Non-tail self-calls recurse on the machine stack, which the VM's frame vector
cannot police, so unbounded recursion would be a segfault rather than a Sema
error. Compiled code therefore keeps an explicit depth counter — a `u64` owned
by the backend, its address baked into the code — and bails at `JIT_MAX_DEPTH`,
which is set to the VM's own `MAX_FRAMES`. The VM then re-runs the call and
raises its usual "stack overflow" error. The counter is per backend and the
backend is per thread, so no synchronization is involved.

### Calling convention

```
extern "C" fn(a0: u64, ..., aN: u64) -> u64
```

Arity is capped at `MAX_JIT_ARITY` (6) so every argument is register-passed on
both System V and AAPCS. Functions with a rest parameter are never compiled —
building the rest list would allocate.

### Warm-up

Compilation is attempted after `SEMA_JIT_THRESHOLD` VM calls (default 32), and
the verdict is cached in a `JitSlot` on the `Function` — one `Cell` read per
call, not a hash lookup. `JitSlot` is not serialized: a `.semac` file carries
bytecode, and each process decides for itself what to compile.

A call counter alone misses the hottest shape there is. A loop runs its
iterations inside a single VM frame, so a function that spends a second looping
is still only one call and would never reach a 32-call threshold — leaving
exactly the code the generator exists for on the VM. A function whose chunk
contains a self-tail-call or a backward branch is therefore compiled on its
first call.

The generator is opt-in: `sema --jit`, or `SEMA_JIT=1` when the process
arguments must stay unchanged (a shebang script reading `sys/args`). The scan walks instructions through `serialize::advance_pc` rather
than searching for opcode bytes, since an operand byte can hold any value.

When no backend is installed the whole path costs one thread-local `bool` load.

## Measured effect

`cargo build --release`, x86-64 Linux, `sema -e` versus `sema --jit -e`:

| program | VM | JIT | |
|---|---|---|---|
| `examples/benchmarks/tak.sema`, 500 iterations | 2827ms | 178ms | 15.9x |
| `fib-naive(30)` (scheme-algorithms) | 215ms | 20ms | 10.8x |
| Ackermann `A(3,8)` (scheme-algorithms) | 458ms | 50ms | 9.2x |
| `fib 32` (non-tail recursion) | 0.59s | 0.07s | 8.4x |
| Collatz step counts to 300k | 6.13s | 0.61s | 10.0x |
| Escape-time inner loop, 200k points | 13.92s | 2.03s | 6.9x |
| Float iteration, 3M steps | 0.25s | 0.03s | 8.3x |
| Tail-recursive sum, 5M | 0.39s | 0.06s | 6.5x |
| Sum of squares mod 1000, 1M | 0.18s | 0.03s | 6.0x |

Code that is not numeric is untouched, as intended: `deriv.sema` (symbolic
differentiation over lists) runs 1674ms versus 1716ms, and a 500-element
mergesort stays at 3ms. Neither is compilable, so both keep running on the VM.

### The cost of bailing late

A bail discards the compiled call and re-runs it in the VM from the start, so a
long loop that bails on its *last* iteration pays for the whole compiled run and
then the whole VM run:

| program | VM | JIT | |
|---|---|---|---|
| tail-recursive sum, 20M (result exceeds the fixnum range) | 2.30s | 2.24s | 1.0x |

The compiled loop runs 20M iterations, overflows the fixnum range near the end,
bails, and the VM redoes all of it. The answer is right; the work is doubled.
This is the model's worst case, and it is the price of not needing
deoptimization state. Narrowing it means on-stack replacement — resuming the VM
mid-loop from the compiled frame's state — which is a much larger piece of work
than phase 1.

## Testing

Three layers, all in-process:

- `sema-core` unit tests pin the immediate/heap classification against the tag
  constants, and pin that hardware's default NaN collides with `Value::NIL`.
- `crates/sema-codegen/tests/equivalence_test.rs` runs each program twice in one
  process — VM alone, then with the JIT compiling eagerly — and demands
  identical results, including type. It also asserts that compilation actually
  happened, so the suite cannot pass by compiling nothing.
- `crates/sema-codegen/tests/differential_test.rs` generates ~580 random
  programs over the subset, with operands on the fixnum boundary, zero divisors,
  NaN, the infinities, and non-numbers, and demands the same agreement. Seeded,
  so a failure reproduces.

Bugs these caught during development, all of which would have shipped:

1. A float NaN result decoding as `nil` (missing canonicalization).
2. Cranelift branch arguments passed to both edges of a conditional.
3. `>=` and `<=` disagreeing with the VM for NaN operands.

## Deliberately not done

- **AOT object emission.** `cranelift-object` plus the system linker would make
  `sema build` produce a true native binary instead of a runner plus `.semac`.
  `decode.rs` and `translate.rs` are reusable as-is; what is missing is a
  runtime static library and linker driving.
- **Runtime trampolines.** Calling back into stdlib natives (`string/split`,
  `json/encode`) from compiled code would widen the subset a long way, but it
  reintroduces `Rc` ownership across the boundary, which is exactly what phase 1
  avoids. It needs its own ownership design, not an incremental patch.
- **Calls to other functions.** Same reason: a general call returns an owned
  `Value` that may be heap-backed.
- **WebAssembly.** Browsers cannot map executable pages, so `sema-wasm` keeps
  using the VM. This is not a gap to close.
- **Tail calls into compiled code.** `tail_call_vm_closure` would have to pop
  the caller's frame and return, which the current hook shape does not express.
  Self-tail-calls already compile to loops, so the missing case is mutual tail
  recursion, which needs `return_call` anyway.

## Files

| Path | Role |
|---|---|
| `crates/sema-vm/src/jit.rs` | the seam: ABI, `JitBackend`, `JitSlot`, warm-up, stats |
| `crates/sema-codegen/src/decode.rs` | bytecode decode, opcode whitelist, stack-depth analysis |
| `crates/sema-codegen/src/translate.rs` | bytecode → CLIF |
| `crates/sema-codegen/src/backend.rs` | Cranelift module, code memory, depth counter |
| `crates/sema-core/src/value.rs` | `is_immediate` and the NaN-box constants the backend needs |
