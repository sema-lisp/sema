# 1BRC VM Profile: Data-Heavy Workload Analysis

> 📎 **Historical snapshot (kept as a baseline).** Captured 2026-02-19, *before*
> the 1.19.x perf pass (PGO, fat LTO, inline string opcodes — see CHANGELOG
> 1.19.2, ~25–29% faster on 1BRC). Useful as a pre-optimization reference; the
> hotspot percentages below predate those changes.

> **Date:** 2026-02-19
> **Git SHA:** `50f17d2`
> **Machine:** Apple M2 Max, 32 GB, macOS 15.6 (arm64)
> **Profiler:** samply (Firefox Profiler format), `release-with-debug` profile
> **Runtime:** Bytecode VM (`--vm`)

## Benchmark

```bash
cargo run --release -- --no-llm --vm examples/benchmarks/1brc.sema -- benchmarks/data/bench-10m.txt
```

**Input:** 10M rows, 40 weather stations, semicolon-delimited (`station;temperature`).

**Workload characteristics:** This is a data-heavy benchmark — the inner loop reads lines, splits strings, parses floats, and accumulates results in a hashmap with `assoc` (which clones the entire map per update). Contrasts with tak/nqueens which are compute-heavy (arithmetic + recursion).

**Result:** **14.9s** (14,907 samples at 1ms interval)

## Top Hotspots

### By self time (where CPU cycles are actually spent)

| Area | Self % | ~Time | Symbol | Notes |
|------|-------:|------:|--------|-------|
| VM dispatch loop | 16.5% | 2.5s | `VM::run` | Opcode fetch + decode + branch |
| `drop(Value)` — monomorphization 1 | 12.0% | 1.8s | `drop_in_place<Value>::h20afa…` | Rc decrement + tag dispatch |
| `drop(Value)` — monomorphization 2 | 11.3% | 1.7s | `drop_in_place<Value>::ha14e…` | Same logic, different call site |
| `HashMap::clone` (Env capture) | 5.8% | 0.9s | `<HashMap as Clone>::clone` | Closure creation clones function table |
| `Rc::drop_slow` | 3.0% | 0.5s | `Rc::drop_slow` | Deallocation when refcount → 0 |
| `Env::get` | 2.3% | 0.3s | `Env::get` | Variable lookup via scope chain |
| `to_vec` (args cloning) | 2.1% | 0.3s | `<T as ConvertVec>::to_vec` | `call_value` clones args off stack |
| `Value::view` | 2.0% | 0.3s | `Value::view` | NaN-box decode + Rc bump for pattern match |
| `string/split` (TwoWaySearcher) | 1.7% | 0.3s | `TwoWaySearcher::next` | String splitting in inner loop |
| `Value::eq` + `Value::hash` | 2.5% | 0.4s | `PartialEq::eq`, `Hash::hash` | Hashmap key operations |
| `NativeFn` dispatch | 3.1% | 0.5s | `NativeFn::simple::{closure}` | Multiple stdlib function calls |
| `malloc` / `free` | 1.5% | 0.2s | `DYLD-STUB$$malloc/free` | Heap allocation pressure |

### By inclusive time (on-stack)

| Area | Incl. % | ~Time | Notes |
|------|--------:|------:|-------|
| `VM::run` | 100% | 14.9s | Entire execution |
| `io::register::{closure}` (file/fold-lines) | 100% | 14.9s | All time in fold-lines callback |
| `call_value` | 77% | 11.5s | Function call dispatch |
| `VM::make_closure::{closure}` | 77% | 11.4s | VM closure fallback (stdlib HOF interop) |
| `drop(Value)` (all variants) | ~35% | 5.2s | Combined Rc refcounting + deallocation |
| `VM::call_value` | 12% | 1.8s | VM-level call dispatch |
| `HashMap::clone` | 6.3% | 0.9s | Closure environment cloning |
| `to_vec` (all sites) | 7.1% | 1.1s | Stack-to-vec copies for native args |

## Hot Call Paths

The top call paths (sema-only frames, 10 most frequent):

```
22.5%  …fold-lines → call_value → VM::make_closure::{closure} → VM::run
11.3%  …fold-lines → drop(Value) → Rc::drop_slow → drop(Value)
 9.2%  …VM::run → drop(Value)
 6.3%  …VM::run → call_value → NativeFn → HashMap::clone
 6.3%  …VM::run → NativeFn::{closure}
 6.1%  …VM::run → to_vec (args allocation)
 5.2%  …fold-lines → drop(Value) → Rc::drop_slow
 4.8%  …fold-lines → call_value → VM::make_closure
 2.3%  …VM::run → Env::get
 2.3%  …fold-lines → call_value → drop(Value)
```

## Interpretation

**Unlike tak/nqueens, 1BRC is dominated by data structure churn, not compute.**

The inner loop (`file/fold-lines`) processes 10M lines. Each iteration:

1. Calls `string/split` — allocates a list of strings
2. Calls `string->float` — parses a float
3. Calls `get` — hashmap lookup
4. Calls `assoc` — **clones the entire hashmap** + inserts new entry
5. Calls `vector` — allocates a 4-element vector `(vector temp temp temp 1)`
6. Returns the new accumulator — **drops the old hashmap**

Steps 4 and 6 are the killer: `assoc` on a hashmap is O(n) because it creates a full copy. With 40 stations, that's 40 entries × 10M iterations = 400M key-value clones. Each clone bumps and then drops Rc refcounts on both keys (strings) and values (vectors).

The `make_closure::{closure}` path at 77% inclusive shows that the fold callback lambda goes through the **NativeFn fallback path** (stdlib HOF interop creates a fresh mini-VM per call), not the fast same-VM call path. This adds overhead per iteration.

## Candidate Optimizations

Ranked by estimated impact on this workload:

### 1. Mutable hashmap accumulator (language-level)

**Impact: ~40-50% of total time**

Replace `assoc` (immutable, clones entire map) with `hashmap/set!` (mutate in place). This eliminates the O(n) clone + O(n) drop per iteration. The 1BRC script could be rewritten:

```scheme
;; Instead of:
(assoc acc name (vector ...))

;; Use:
(hashmap/set! acc name (vector ...))
acc
```

This is a benchmark-level fix, not a VM optimization — but it's the single biggest win.

### 2. Fold callback via same-VM path

**Impact: ~10-15%**

Currently `file/fold-lines` calls the user callback through `call_callback` → `NativeFn` wrapper → fresh `VM::new()` → `vm.run()`. If the callback could be detected as a VM closure and executed on the same VM (similar to `call_value`'s payload check), it would avoid per-call VM creation overhead for 10M iterations.

### 3. Pass native args as stack slice, not `to_vec`

**Impact: ~5-7%**

In `call_value` (line 850): `let args: Vec<Value> = self.stack[args_start..].to_vec()`. This clones all args into a fresh `Vec` on every native function call. Passing a `&[Value]` slice from the stack would eliminate 10M+ small vec allocations.

### 4. Reduce `drop(Value)` overhead

**Impact: ~5-10%**

The existing `drop` impl does a tag-dispatch match on every drop. For the common case (immediates: ints, bools, nil), the drop is a no-op but still branches through the tag check. A fast-path check `if !is_boxed(self.0) { return; }` followed by `if tag <= TAG_KEYWORD { return; }` could short-circuit sooner. (Already partially implemented but the branch pattern could be tuned.)

### 5. `HashMap::clone` in `make_closure`

**Impact: ~4-6%**

`make_closure` at line 1199 clones `self.functions` (an `Rc<Vec<Rc<Function>>>`) and `self.globals` for the NativeFn fallback wrapper. The `Rc::ptr_eq` check avoids cloning when they're the same, but the `globals` clone always happens. Consider sharing globals via `Rc` without cloning.

### 6. Dispatch loop improvements

**Impact: ~3-5%**

Already covered in the [VM Performance Plan](../plans/2026-02-17-vm-performance.md) and [Roadmap](../plans/2026-02-17-vm-performance-roadmap.md). Cached frame locals, raw pointer bytecode reads, and `u8` match dispatch apply here too but have less relative impact than on tak since dispatch is only 16.5% self-time (vs ~30%+ on tak).

## Comparison with Compute-Heavy Benchmarks

| Bottleneck | 1BRC (data-heavy) | tak (compute-heavy) |
|---|---|---|
| drop / Rc refcount | **~35%** | ~10% |
| VM dispatch | 16.5% | **~30%** |
| HashMap::clone | 5.8% | 0% |
| Arithmetic | <1% | **~25%** |
| String ops | ~4% | 0% |
| Env::get | 2.3% | **~15%** |

**Takeaway:** Optimizations targeting dispatch and arithmetic (Phases 1-4 in the roadmap) primarily help compute benchmarks. For data-heavy workloads, reducing allocation churn and Rc traffic is more impactful.
