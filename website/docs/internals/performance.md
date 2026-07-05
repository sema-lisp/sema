# Performance Internals

Sema's evaluator is a bytecode VM (see [Bytecode VM](./bytecode-vm.md)). It reached that design through a long optimization journey. Early optimizations brought the [1 Billion Row Challenge](https://github.com/gunnarmorling/1brc) benchmark from **~25s to ~9.6s** on 10M rows using a "mini-eval" — a minimal evaluator inlined in the stdlib that bypassed the full trampoline. The mini-eval was later **removed** for architectural reasons (semantic drift from the real evaluator, and blocking the path to a bytecode VM). Fast-path optimizations in the (then) tree-walking evaluator partially recovered performance, bringing it to **~2,700ms on 1M rows** (vs ~960ms with the mini-eval). The bytecode VM now achieves **~1,100ms on 1M rows** and **~11,000ms on 10M rows** (re-measured 2026-07, see the note below), more than recovering the mini-eval's performance through compilation rather than inlining. (The tree-walking interpreter has since been retired entirely.) This page documents each optimization, its history, and measured impact.

All benchmarks were run on Apple Silicon (M-series), processing the 1BRC dataset (semicolon-delimited weather station readings, one per line).

## Benchmark Summary

| Stage              | 1M rows       | 10M rows       | Technique                          | Status                   |
| ------------------ | ------------- | -------------- | ---------------------------------- | ------------------------ |
| Baseline           | 2,501 ms      | ~25,000 ms     | Naive implementation               | —                        |
| + COW assoc        | 1,800 ms      | ~18,000 ms     | In-place map mutation              | ✅ Active                |
| + Env reuse        | 1,626 ms      | 16,059 ms      | Lambda env recycling (mini-eval)   | ❌ Removed               |
| + Mini-eval        | ~960 ms       | ~9,600 ms      | Inlined builtins, custom parser    | ❌ Removed               |
| + String interning | —             | —              | Spur-based dispatch                | ✅ Active                |
| + hashbrown        | —             | —              | Amortized O(1) accumulator         | ✅ Active                |
| **Post-removal**   | **~2,700 ms** | **~29,700 ms** | Callback architecture + fast paths | ⏸ Tree-walker (retired)  |
| **Bytecode VM**    | **~1,150 ms** | **~12,600 ms** | Bytecode VM (sole evaluator)       | ✅ Current               |

> **Note:** The mini-eval and its associated optimizations (env reuse, inlined builtins, custom number parser, SIMD split fast path) were removed to unblock the bytecode VM, which has since become Sema's sole evaluator. The bytecode VM provides a ~2.4× speedup over the (now-retired) tree-walker (~1,150ms vs ~2,700ms on 1M rows), more than recovering the mini-eval's performance through compilation. Fast-path optimizations (self-evaluating short-circuit, inline NativeFn dispatch, thread-local EvalContext, deferred cloning) partially recovered the tree-walker's performance before it was retired.

> **VM compute benchmarks** (Feb 2026, post-stdlib intrinsics): TAK 1,248ms, upvalue-counter 450ms, deriv 887ms. The deriv benchmark — dominated by `car`/`cdr`/`cons`/`pair?` — improved 22% from stdlib intrinsic opcodes. The 1BRC numbers above are I/O-bound and less affected by VM compute optimizations.

> **1BRC re-measured (Jul 2026)** — interleaved hyperfine A/B on the same machine
> and datasets, v1.28.1 vs the cycle-collector + true-async work (ADR #66/#68/#69):
> 1M rows 1,085ms → 1,101ms, 10M rows 10,861ms → 11,012ms — a **+1.4% delta,
> attributed to the Bacon–Rajan cycle collector's bookkeeping (documented tax
> envelope ≤1.6%)**, partially offset by the self-tail-call optimization (#62).
> The cooperative-async runtime work (yielding LLM/http/shell/file natives, the
> one-pool `sema-io` consolidation) measures **zero cost on synchronous programs**
> — the sync execution path is byte-identical by construction, enforced by a
> source-conformance test. In exchange, cycles are reclaimed automatically
> (long-lived async servers no longer accrete unreachable closures) and blocking
> I/O yields to the scheduler so sibling tasks interleave.

## Per-Instruction Inline Cache (Mar 2026)

The VM's global variable lookup was originally served by a 256-slot direct-mapped cache. Each `LoadGlobal`/`CallGlobal` hashed the variable name to a slot, leading to collisions on hot paths where multiple globals mapped to the same slot.

The per-instruction inline cache assigns a **dedicated cache slot to each `LoadGlobal`/`CallGlobal` instruction** at compile time. Cache entries are `(spur_bits, env_version, value)` tuples — the spur_bits guard provides cross-VM closure safety, and the env version counter invalidates stale entries on any global mutation.

**Impact** (Apple Silicon, release build, hyperfine --warmup 2 --runs 5):

| Benchmark          | Before (direct-mapped) | After (per-instruction) | Speedup    |
| ------------------ | ---------------------: | ----------------------: | ---------- |
| higher-order-fold  | 6,116 ms               | 2,617 ms                | **2.34×**  |
| deriv              | 2,356 ms               | 1,449 ms                | **1.63×**  |
| closure-storm      | 1,302 ms               | 1,145 ms                | **1.14×**  |
| tak                | 1,728 ms               | 1,749 ms                | ~1.0×      |
| mandelbrot         | 311 ms                 | 313 ms                  | ~1.0×      |
| upvalue-counter    | 574 ms                 | 575 ms                  | ~1.0×      |

The biggest wins are on **global-call-heavy** workloads: `higher-order-fold` calls stdlib HOFs (`map`, `filter`, `foldl`) in a tight loop — each call requires a global lookup. `deriv` similarly uses many global functions for symbolic differentiation. Benchmarks dominated by local computation (tak, mandelbrot, upvalue-counter) show no change, as expected.

## Micro-Benchmark Suite (Feb 2026)

All benchmarks run on Apple Silicon (M-series), 10 runs + 3 warmup, via `scripts/bench.sh`.

| Benchmark          | Tree-walker    | Bytecode VM    | VM speedup |
| ------------------ | -------------- | -------------- | ---------- |
| tak                | 21,222 ms      | 1,248 ms       | 17.0×      |
| nqueens            | 20,735 ms      | 2,028 ms ¹     | 10.2×      |
| deriv              | 3,473 ms       | 887 ms         | 3.9×       |
| upvalue-counter    | 5,762 ms       | 450 ms         | 12.8×      |
| closure-storm      | 2,373 ms       | 1,041 ms       | 2.3×       |
| higher-order-fold  | 2,292 ms       | 1,081 ms       | 2.1×       |
| hashmap-bench      | 8,612 ms       | 3,645 ms       | 2.4×       |
| bench-features     | 12,427 ms      | 1,144 ms       | 10.9×      |
| string-pipeline    | 1,551 ms       | 613 ms         | 2.5×       |
| mandelbrot         | 2,223 ms       | 212 ms         | 10.5×      |
| throw-catch        | 2,195 ms       | 197 ms         | 11.2×      |

¹ nqueens was previously broken on the VM due to a forward-reference bug in inner defines (fixed Mar 2026). The VM result above now reflects correct execution.

The VM achieves **2–17× speedups** across the board, with the largest gains on recursion-heavy benchmarks (tak, nqueens, bench-features, upvalue-counter) where call overhead dominates. Closure-heavy and string benchmarks show more modest ~2–3× gains.

## 1. Copy-on-Write Map Mutation

**Problem:** Every `(assoc map key val)` call cloned the entire `BTreeMap`, even when no other reference existed. For the 1BRC accumulator (~400 weather stations), this was O(400) per row × millions of rows.

**Solution:** Use `Rc::try_unwrap` to check if the reference count is 1. If so, take ownership and mutate in place. Otherwise, clone.

```rust
// crates/sema-stdlib/src/map.rs
match Rc::try_unwrap(m) {
    Ok(map) => map,       // refcount == 1: we own it, mutate in place
    Err(m) => m.as_ref().clone(),  // shared: must clone
}
```

The key insight is pairing this with `Env::take()` — by _removing_ the accumulator from the environment before passing it to `assoc`, the refcount drops to 1, enabling the in-place path. User code looks like:

```sema
(file/fold-lines "data.csv"
  (lambda (acc line)
    (let ((parts (string/split line ";")))
      (assoc acc (first parts) (second parts))))
  {})
```

The `fold-lines` implementation moves (not clones) `acc` into the lambda env on each iteration, keeping the refcount at 1.

**Impact:** ~30% of the total speedup. Eliminated the O(n) full-map clone, leaving only the O(log n) BTreeMap insert per row.

**Literature:**

- This is the same copy-on-write strategy used by Swift's value types. (Clojure's persistent data structures solve a related problem — avoiding full copies — but via structural sharing rather than refcount-based COW.)
- Phil Bagwell, ["Ideal Hash Trees"](https://lampwww.epfl.ch/papers/idealhashtrees.pdf) (2001) — the paper behind Clojure/Scala persistent collections
- Rust's `Rc::make_mut` provides the same semantics with less ceremony

## 2. Lambda Environment Reuse _(removed)_

> **Status:** This optimization was part of the mini-eval's hot path in `io.rs`. It was removed when the mini-eval was deleted. The current `file/fold-lines` uses `sema_core::call_callback`, which routes through the real evaluator — each call creates a fresh `Env` via the standard `apply_lambda` path.

**What it was:** For simple lambdas (known arity, no rest params), the mini-eval created the lambda environment _once_ and reused it across all iterations, overwriting bindings in place. Combined with a reusable `line_buf`, this eliminated per-iteration allocations for `Env`, string interning, and line buffers.

**Why it was removed:** The env reuse logic was tightly coupled to the mini-eval's direct lambda dispatch. The callback architecture routes through the real evaluator's `apply_lambda`, which always creates a fresh child `Env` — this is correct and avoids subtle bugs from env mutation leaking across calls.

**Impact when active:** ~15% speedup (2,501ms → 1,626ms combined with COW assoc).

**What remains:** The reusable `line_buf` (`String::with_capacity(64)` cleared each iteration) is still present in `file/fold-lines` — only the env reuse was lost.

## 3. Evaluator Callback Architecture _(replacing Mini-Eval)_

> **Status:** The mini-eval was deleted and replaced with a callback architecture. Stdlib now calls the real evaluator via `sema_core::call_callback`.

**What the mini-eval was:** `sema-stdlib` previously contained its own minimal evaluator (`sema_eval_value`) that handled common forms via direct recursive calls, inlining builtins like `+`, `=`, `assoc`, `string/split`, and `string/to-number` to skip `Env` lookup and `NativeFn` dispatch entirely.

**Why it was removed:**

1. **Semantic drift:** The mini-eval diverged from the real evaluator — new special forms, error handling, and features had to be duplicated or were silently missing.
2. **Blocking bytecode VM:** A bytecode compiler can't target two evaluators. Removing the mini-eval ensures a single evaluation path that the VM can replace.

**The callback architecture:** `sema-stdlib` cannot depend on `sema-eval` (circular dependency). Instead, `sema-eval` registers a thread-local callback (`set_call_callback`) at startup, and stdlib functions call `sema_core::call_callback` to invoke the real evaluator. A thread-local `EvalContext` (`with_stdlib_ctx`) is shared across calls to avoid per-call context allocation.

```rust
// crates/sema-stdlib/src/io.rs — file/fold-lines via callback
sema_core::with_stdlib_ctx(|ctx| {
    let mut line_buf = String::with_capacity(64);
    loop {
        line_buf.clear();
        let n = reader.read_line(&mut line_buf)?;
        if n == 0 { break; }
        // Calls the real evaluator (eval_value) via thread-local callback
        acc = sema_core::call_callback(ctx, &func, &[acc, Value::string(&line_buf)])?;
    }
    Ok(acc)
})
```

**Performance trade-off:** ~960ms → ~2,900ms on 1M rows (~3× regression). The overhead comes from the full trampoline evaluator: call stack management, span tracking, and `Trampoline` dispatch on every sub-expression.

**Fast-path optimizations that partially recovered performance:**

1. **Self-evaluating fast path:** `eval_value` short-circuits for integers, floats, strings, keywords, and symbols — skipping depth tracking and step limits for the most common forms.
2. **Inline NativeFn dispatch:** When the evaluator sees a `Value::NativeFn` in call position, it calls the function pointer directly without going through `call_callback` indirection.
3. **Thread-local shared EvalContext:** `with_stdlib_ctx` reuses a single `EvalContext` across all stdlib → evaluator callbacks, avoiding per-call allocation of `RefCell`/`Cell` fields.
4. **Deferred cloning:** `eval_value_inner` avoids cloning the expression and environment on the first trampoline iteration, only cloning if a tail call (`Trampoline::Eval`) is returned.

**Remaining gap:** The ~3× regression could not be fully closed within the tree-walking architecture. The bytecode VM — the reason the mini-eval was removed, and now Sema's sole evaluator — gets to ~1.2× of the mini-eval on 1M (~1,150ms vs ~960ms) and is ~2.4× faster than the tree-walker was on the same workload.

**Literature:**

- Inline caching, pioneered by Smalltalk-80 and refined in V8's hidden classes, solves the same dispatch overhead problem but at a different architectural level
- Most production Lisps (SBCL, Chez Scheme) compile to native code, making dispatch overhead negligible — Sema's callback overhead is inherent to tree-walking interpreters
- Lua 5.x's bytecode VM inlines common operations (`OP_ADD`, `OP_GETTABLE`) into the dispatch loop — this is the approach Sema's bytecode VM (`sema-vm`) takes

## 4. String Interning (lasso)

**Problem:** Symbol/keyword equality was O(n) string comparison. Environment lookups keyed by `String` required comparing the full string on each `BTreeMap` node visit. Special form dispatch compared against 30+ string literals on every list evaluation.

**Solution:** Replace `Rc<String>` in `Value::Symbol` and `Value::Keyword` with `Spur` — a `u32` handle from the [lasso](https://crates.io/crates/lasso) string interner. Environment bindings keyed by `Spur` for direct integer lookup.

```rust
// Before: O(n) string comparison
Value::Symbol(Rc<String>)
env: BTreeMap<String, Value>

// After: O(1) integer comparison
Value::Symbol(Spur)  // u32
env: BTreeMap<Spur, Value>
```

(`Env` bindings have since moved from `BTreeMap` to `hashbrown::HashMap`, still keyed by `Spur`.)

Special form dispatch uses pre-cached `Spur` constants:

```rust
// crates/sema-eval/src/special_forms.rs
struct SpecialFormSpurs {
    quote: Spur,
    if_: Spur,
    define: Spur,
    // ... 30 more
}

// Dispatch: integer comparison, no string resolution
if head_spur == sf.if_ {
    return Some(eval_if(args, env));
}
```

**Caveat:** The initial implementation was actually _slower_ (2,518ms vs 1,580ms baseline) because `resolve()` was allocating a new `String` on every symbol lookup. Fixed by adding `with_resolved(spur, |s| ...)` which provides a borrowed `&str` without allocation, and switching `Env` to use `Spur` keys directly.

**Impact:** 1,580ms → 1,400ms (11% faster) after fixing the allocation issue.

**Literature:**

- String interning is as old as Lisp itself — McCarthy's original LISP 1.5 (1962) interned atoms in the "object list" (oblist)
- Java interns all string literals and provides `String.intern()`. The JVM's `invokedynamic` uses interned method names for O(1) dispatch
- The [string-interner](https://crates.io/crates/string-interner) and [lasso](https://crates.io/crates/lasso) crates are the two main Rust options; lasso was chosen for its `Rodeo` thread-local interner which fits Sema's single-threaded architecture

## 5. hashbrown HashMap

**Problem:** The 1BRC accumulator uses a map keyed by weather station name (~400 entries). `BTreeMap` provides O(log n) lookup, but the accumulator is accessed on every row. With 10M rows, the log₂(400) ≈ 9 comparisons per lookup adds up.

**Solution:** Added a `Value::HashMap` variant backed by [hashbrown](https://crates.io/crates/hashbrown) (the same hash map used inside Rust's `std::collections::HashMap`, but exposed directly for `no_std` compatibility and raw API access).

```sema
;; User code: opt into HashMap for the accumulator
(file/fold-lines "data.csv"
  (lambda (acc line) ...)
  (hashmap/new))  ; amortized O(1) vs O(log n)

;; Convert back to sorted BTreeMap for output
(hashmap/to-map acc)
```

`BTreeMap` remains the default for `{}` map literals because deterministic ordering matters for equality, printing, and test assertions. `hashbrown` is opt-in for performance-critical paths.

**Impact:** 1,400ms → 1,340ms (4% faster). Modest because BTreeMap with 400 entries and short string keys is already fast.

**Literature:**

- hashbrown uses SwissTable, designed by Google for their C++ `absl::flat_hash_map`. See [CppCon 2017: Matt Kulukundis "Designing a Fast, Efficient, Cache-friendly Hash Table"](https://www.youtube.com/watch?v=ncHmEUmJZf4)
- Clojure's `{:key val}` maps use HAMTs (hash array mapped tries) which provide O(~1) lookup with structural sharing. Sema's approach is simpler: full COW on the `Rc<HashMap>` rather than structural sharing, which is viable because the refcount-1 fast path almost always hits

## 6. SIMD Byte Search (memchr) _(removed)_

> **Status:** The memchr-based two-part split fast path was part of the mini-eval's inlined `string/split` and was removed with it. The current `string/split` in `sema-stdlib/src/string.rs` uses Rust's standard `str::split()` followed by `map` and `collect`. The `memchr` crate remains a dependency of `sema-stdlib` but is no longer used in the split hot path.

**What it was:** A SIMD-accelerated (SSE2/AVX2/NEON) byte search via the [memchr](https://crates.io/crates/memchr) crate, combined with a two-part split fast path that avoided `Vec` allocation when splitting on a single-byte separator with exactly one occurrence (the common case in 1BRC: `"Berlin;12.3"` → `["Berlin", "12.3"]`).

**Impact when active:** Negligible for SIMD specifically (1BRC strings are 10–30 bytes), but the two-part fast path avoided iterator/Vec overhead.

**Literature:**

- memchr is maintained by Andrew Gallant (BurntSushi), author of ripgrep. It uses a [generic SIMD](http://0x80.pl/articles/simd-strfind.html) framework to dispatch to the best available instruction set at runtime

## 7. Custom Number Parser _(removed)_

> **Status:** This was part of the mini-eval's inlined `string/to-number` and was removed with it. The current `string/to-number` in `sema-stdlib/src/string.rs` uses Rust's standard `str::parse::<i64>()` with fallback to `str::parse::<f64>()`.

**What it was:** A hand-rolled decimal parser that handled only `[-]digits[.digits]`, using a precomputed powers-of-10 lookup table for 1–4 fractional digits. It returned `None` for complex cases (scientific notation, infinity, NaN), falling back to the standard parser.

**Impact when active:** Part of the combined mini-eval speedup. Difficult to isolate, but avoided the overhead of Rust's [dec2flt](https://doc.rust-lang.org/stable/src/core/num/dec2flt/mod.rs.html) algorithm.

**Literature:**

- Rust's float parser is based on the [Eisel-Lemire algorithm](https://nigeltao.github.io/blog/2020/eisel-lemire.html) (2020), which is fast for a general-purpose parser but still does more work than necessary for simple decimals
- Daniel Lemire's [fast_float](https://github.com/fastfloat/fast_float) C++ library (and its Rust port) takes a similar "fast path for common cases" approach

## 8. Enlarged I/O Buffer

**Problem:** `BufReader`'s default 8KB buffer means frequent syscalls for large files.

**Solution:** 256KB buffer for `file/fold-lines`.

```rust
let mut reader = std::io::BufReader::with_capacity(256 * 1024, file);
```

**Impact:** Minor. CPU was the bottleneck, not I/O. But it's a free win — larger buffers amortize syscall overhead and improve sequential read throughput on modern SSDs.

## 9. Bytecode VM Optimizations

The bytecode VM applies several optimizations beyond basic bytecode compilation. These are documented in detail in [Bytecode VM](./bytecode-vm.md); highlights below.

### Intrinsic Recognition

The compiler recognizes calls to known builtins and emits inline opcodes instead of function calls:

**Arithmetic & comparison** (phase 1):

| Source | Compiled to | What it replaces |
|--------|------------|-----------------|
| `(+ a b)` | `AddInt` | `CallGlobal("+", 2)` → hash lookup → NativeFn downcast → args Vec → function call |
| `(- a b)` | `SubInt` | Same overhead |
| `(* a b)` | `MulInt` | Same overhead |
| `(< a b)` | `LtInt` | Same overhead |
| `(> a b)` | `Gt` | Same overhead |
| `(not x)` | `Not` | Same overhead |

**Stdlib: list operations & type predicates** (phase 2, Feb 2026):

| Source | Compiled to | What it replaces |
|--------|------------|-----------------|
| `(car x)` / `(first x)` | `Car` | Same overhead — pop list, push first element |
| `(cdr x)` / `(rest x)` | `Cdr` | Same — pop list, push tail |
| `(cons h t)` | `Cons` | Same — pop head+tail, push new list |
| `(null? x)` | `IsNull` | Same — push `#t` if nil or empty list |
| `(pair? x)` | `IsPair` | Same — push `#t` if non-empty list |
| `(list? x)` | `IsList` | Same — push `#t` if list |
| `(number? x)` | `IsNumber` | Same — push `#t` if int or float |
| `(string? x)` | `IsString` | Same — push `#t` if string |
| `(symbol? x)` | `IsSymbol` | Same — push `#t` if symbol |
| `(length x)` | `Length` | Same — push collection length as int |
| `(append a b)` | `Append` | Same — concatenate two lists (2-arg only) |
| `(get m k)` | `Get` | Same — map lookup, nil default (2-arg only) |
| `(contains? m k)` | `ContainsQ` | Same — push `#t` if key exists in map |

This eliminates global hash lookup, `Rc` downcast, argument `Vec` allocation, and function pointer dispatch — the entire call overhead — for the most common operations. The `*Int` opcodes include NaN-boxed small-int fast paths that operate directly on raw `u64` bits, avoiding `Clone`/`Drop` overhead entirely.

All standard arithmetic and comparison operators are inlined. The `*Int` variants include NaN-boxed fast paths; the generic opcodes (`Div`, `Gt`, `Le`, `Ge`) handle int/float coercion correctly.

**Impact:** Phase 1: TAK 4,352ms → 1,250ms (-71%), upvalue-counter 1,232ms → 450ms (-63%). Phase 2: deriv 1,123ms → 879ms (-22%), closure-storm 1,135ms → 1,029ms (-9%). The deriv benchmark is dominated by `car`/`cdr`/`cons`/`pair?` — exactly the functions that became intrinsics.

### Constant Folding

An optimization pass (`optimize.rs`) runs on the CoreExpr IR between lowering and variable resolution. It folds compile-time-evaluable expressions:

- **Arithmetic:** `(+ 1 2)` → `3`, `(* 3 4)` → `12`
- **Comparisons:** `(< 1 2)` → `#t`, `(= 3 3)` → `#t`
- **Boolean:** `(not #t)` → `#f`
- **Control flow:** `(if #t a b)` → `a`, `(and #f x)` → `#f`, `(or #t x)` → `#t`
- **Dead code:** `(begin 42 x)` → `(begin x)` (pure constants before the last expression are eliminated)

**Impact:** Eliminates unnecessary instructions at compile time. Runtime impact on benchmarks is negligible (hot loops operate on variables), but reduces code size and improves startup for programs with constant subexpressions.

### Peephole: `(if (not X) ...)` → JumpIfTrue

The compiler pattern-matches `(if (not expr) then else)` and emits the condition with an inverted jump, eliminating both the `not` call and one opcode dispatch:

```
;; Before: CallGlobal("not") + JumpIfFalse
;; After:  JumpIfTrue (condition compiled directly)
```

### Fused CallGlobal

Non-tail calls to global functions use a single `CallGlobal` instruction that combines `LoadGlobal + Call`, using `call_vm_closure_direct` to set up the call frame without needing the function value on the stack.

### Per-Instruction Inline Cache

Each `LoadGlobal`/`CallGlobal` instruction gets a dedicated cache slot at compile time, eliminating hash collisions. See the [inline cache section](#per-instruction-inline-cache-mar-2026) above for benchmark results.

### Specialized Local Access

Slots 0–3 have dedicated zero-operand opcodes (`LoadLocal0`..`LoadLocal3`, `StoreLocal0`..`StoreLocal3`), saving 2 bytes per access to the most common local variable slots.

## Build Tuning: Fat LTO + PGO (v1.19.2)

Beyond the VM itself, the **distributed binaries** are optimized at build time:

- **Fat LTO** (`lto = "fat"` on the `release`/`dist` profiles): lets LLVM inline across crate boundaries — the dispatch loop in `sema-vm` calls `sema-core` value accessors (`view`, `as_int`, `type_name`, …) millions of times per benchmark, and thin LTO can't always inline those. Measured 3–9% across the suite, at the cost of ~2× longer release builds.
- **Profile-Guided Optimization (PGO):** the cargo-dist GitHub-release binaries and Homebrew bottle are built with PGO. The build instruments the binary, trains it on the full benchmark suite + a 1BRC sample, merges the profile with `llvm-profdata`, then rebuilds — letting LLVM lay out the `match op` dispatch hot blocks by _measured_ opcode frequency. It runs on native release targets via cargo-dist's `github-build-setup`; cross-compiled and Windows targets fall back to fat LTO, and the step is fail-safe (a PGO failure ships LTO, never breaks the release). Run it locally with `make build-pgo`. (`cargo install` builds get fat LTO but not PGO — PGO needs the training step.)

**Measured impact** (v1.19.2 PGO build vs the pre-optimization build, Apple Silicon, best-of-N):

| Benchmark         | Before | v1.19.2 PGO | Δ      |
| ----------------- | ------ | ----------- | ------ |
| 1BRC (10M rows)   | 11.18s | 8.23s       | −26%   |
| higher-order-fold | 552ms  | 334ms       | −39%   |
| tak               | 1793ms | 1209ms      | −33%   |
| mandelbrot        | 246ms  | 177ms       | −28%   |
| deriv             | 767ms  | 570ms       | −26%   |
| hashmap-bench     | 3976ms | 2967ms      | −25%   |
| closure-storm     | 1040ms | 836ms       | −20%   |
| bench-features    | 1373ms | 1098ms      | −20%   |
| string-pipeline   | 633ms  | 537ms       | −15%   |
| nqueens           | 2060ms | 1790ms      | −13%   |

The win is dominated by PGO; fat LTO contributes ~3–9% of it. (`cargo install` builds get the LTO portion but not PGO.)

## Rejected Optimizations

Not everything we tried worked:

| Approach                                  | Result       | Why                                                                                                                                                                                 |
| ----------------------------------------- | ------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **HashMap for Env**                       | Slower (later adopted) | At the time, hashing overhead exceeded BTreeMap's few integer comparisons on the very small maps (1–3 entries) typical of `let` scopes. The verdict was later reversed: `Env` bindings now use `hashbrown::HashMap<Spur, Value>`.               |
| **im-rc / rpds (persistent collections)** | Slower       | Structural sharing fights the COW optimization — the whole point is to _avoid_ sharing and mutate in place when refcount is 1.                                                      |
| **bumpalo / typed-arena**                 | Incompatible | Values need to escape the arena (returned from functions, stored in environments). Arena allocation only works for temporaries.                                                     |
| **compact_str / smol_str**                | Redundant    | Once symbols/keywords are interned as `Spur`, small-string optimization for them is pointless. String _values_ are still `Rc<String>` but they're not in the hot path for dispatch. |
| **`target-cpu=native`**                   | No-op (this workload) | Tested Jun 2026: the VM dispatch loop is branch-bound, not SIMD-bound, and the generic `aarch64` target already uses NEON on Apple Silicon. Zero measurable gain — and it breaks portable/distributable binaries, so it is not used.                                 |

> **Note:** "Full evaluator callback" was previously listed here as rejected (4x slower than mini-eval). It became the **tree-walker's architecture** — the ~2.7× overhead vs the mini-eval was accepted as the cost of architectural correctness. The bytecode VM, now Sema's sole evaluator, bypasses this overhead by compiling directly to bytecode.

## Architecture Diagram

The hot path for `file/fold-lines` under the callback architecture, as it ran on the (now-retired) tree-walking evaluator:

```
file/fold-lines
  ├── BufReader (256KB buffer, reused line_buf)
  └── Per-line loop:
        ├── read_line → reused buffer (no alloc)
        ├── call_callback → real evaluator (eval_value)
        │     ├── self-evaluating fast path (ints, floats, strings skip depth tracking)
        │     ├── NativeFn inline dispatch (direct call, no callback indirection)
        │     ├── apply_lambda → fresh Env per call (no env reuse)
        │     ├── string/split → std str::split (no SIMD fast path)
        │     ├── string/to-number → std parse::<i64> / parse::<f64>
        │     └── assoc → COW in-place mutation (Rc refcount == 1)
        ├── thread-local EvalContext (shared, not per-call)
        └── acc moved, not cloned → preserves refcount == 1
```

The bytecode VM bypasses that callback path entirely. Instead of `call_callback → eval_value → trampoline`, the VM compiles the lambda body to bytecode once and executes it in a tight instruction dispatch loop, eliminating trampoline overhead, per-call span tracking, and repeated AST traversal.
