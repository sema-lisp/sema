# Sema Runtime Performance: Design Spec

> Status: deferred implementation spec  
> Scope: full runtime changes needed to reach Janet-level performance on allocation-heavy and byte-oriented workloads (1brc benchmark as the primary oracle).

## 1. Goal

Close the optimized 1brc gap vs. Janet from ~1.6× slower to within 1.1× on the same hardware and dataset, without regressing the simple/idiomatic benchmark or existing semantics.

## 2. Success criteria

- `benchmarks/1brc/1brc.sema` optimized time ≤ 1.1× `benchmarks/1brc/1brc.janet` on macOS M2 Max / 10M rows.
- `benchmarks/1brc/simple/1brc.sema` time does not regress.
- `cargo test` and all integration tests pass.
- No observable semantic changes for existing Sema programs that do not use the new APIs.

## 3. Current architecture (as of Sema 1.19.2)

- `Value` is a NaN-boxed `u64`.
- Every heap object is stored as `Rc<T>`:
  - `String`, `Vec<Value>` (list/vector), `BTreeMap`, `HashMap`, `Lambda`, `Macro`, `NativeFn`, `Prompt`, `Message`, `Conversation`, `ToolDef`, `Agent`, `Thunk`, `Record`, `Bytevector`, `MultiMethod`, `Stream`, `F64Array`, `I64Array`, `AsyncPromise`, `Channel`.
- `Env` is `Rc<RefCell<hashbrown::HashMap<Spur, Value>>>`.
- Cloning a `Value` increments an `Rc` refcount.
- Strings are Rust `String` (UTF-8, character-indexed).
- Vectors are immutable; `bytevector-u8-set!` returns a new bytevector.
- Hashmap has a `with_hashmap_mut_if_unique` COW fast path.

## 4. Target architecture

```
Value(u64) ──NaN-boxed──► GC-managed object header + payload
Env = GcRef<HashMap<Spur, Value>>
+ MutableArray heap type for in-place mutation
+ ByteString / StringSlice heap type for O(1) byte slicing
```

## 5. Subsystems

### 5.1 Tracing GC

#### 5.1.1 Decision: non-moving mark-sweep

Use a non-moving mark-sweep GC as the first implementation. It is the smallest semantic leap from `Rc` and avoids object-moving complexity.

Rationale:
- `Rc` already provides acyclic shared ownership; replacing it with tracing keeps the same object lifetime semantics.
- Rust stack scanning is simplest with pinned/non-moving objects.
- No write barrier needed for a single-generation collector.

Deferred options (future specs):
- Generational GC.
- Copying nursery.
- Compaction.

#### 5.1.2 Core types

New module: `crates/sema-core/src/gc.rs`.

```rust
/// Opaque handle to a GC-managed object. Stored in the NaN-boxed payload.
pub struct GcRef<T: GcObject> {
    ptr: NonNull<GcHeader<T>>,
    _marker: PhantomData<T>,
}

pub struct GcHeader<T: GcObject> {
    mark: Cell<bool>,
    finalized: Cell<bool>,
    payload: T,
}

pub trait GcObject {
    /// Trace all outgoing Value references.
    fn trace(&self, tracer: &mut dyn FnMut(Value));
}
```

`Value` gains `GcRef<T>` constructors alongside the existing `Rc<T>` paths, and the old `Rc` paths are removed once migration is complete.

#### 5.1.3 Heap and collector

```rust
pub struct GcHeap {
    allocations: RefCell<Vec<NonNull<GcHeader<dyn GcObject>>>>,
    threshold: Cell<usize>,
    bytes_allocated: Cell<usize>,
}

impl GcHeap {
    pub fn alloc<T: GcObject + 'static>(&self, obj: T) -> GcRef<T>;
    pub fn collect(&self, roots: &RootSet);
}
```

Collection trigger: bytes allocated since last collection exceed `threshold`.

#### 5.1.4 Root set

Roots must be registered before each collection:
- VM stack (`crates/sema-vm/src/vm.rs`).
- Open upvalues.
- Globals `Env` chain.
- Thread-local channels, promises, and any pending async state.
- Any `Value` held by native Rust code across a collection point (via root handles).

Root API:

```rust
pub struct RootHandle {
    value: Cell<Value>,
}

impl GcHeap {
    pub fn root(&self, value: Value) -> RootHandle;
}
```

#### 5.1.5 Migration strategy

1. Introduce `gc.rs` and `GcRef<T>`.
2. Convert one heap type at a time (start with `String`, then `Vec<Value>`, then `HashMap`, etc.).
3. Keep `Env` as `Rc<RefCell<...>>` initially; migrate to `GcRef` after all contained `Value`s are GC-managed.
4. Run full test suite after each type migration.
5. Remove `Rc<T>` value constructors once migration is complete.

#### 5.1.6 Special cases

- `NativeFn` carries a `Rc<dyn Any>` payload for VM closures. This payload must either be GC-ignored (it does not contain `Value`s) or wrapped in a GC object.
- `StreamBox` contains a `Box<dyn SemaStream>`. Streams do not contain `Value`s except through methods; they can be treated as opaque native roots.
- `AsyncPromise` and `Channel` are shared between coroutines. They must be reachable from the scheduler's task table.

### 5.2 Mutable arrays and cells

#### 5.2.1 New heap types

```rust
pub struct MutableArray {
    items: RefCell<Vec<Value>>,
}

pub struct MutableCell {
    value: RefCell<Value>,
}
```

Both implement `GcObject::trace` to mark their contained `Value`s.

#### 5.2.2 Public APIs

Canonical names (slash-namespaced):

- `(mutable-array/new size? fill?)` → `MutableArray`
- `(mutable-array/push! arr val)` → `arr`
- `(mutable-array/pop! arr)` → removed value
- `(mutable-array/set! arr idx val)` → `arr`
- `(mutable-array/ref arr idx)` → value
- `(mutable-array/length arr)` → int

- `(mutable-cell/new val)` → `MutableCell`
- `(mutable-cell/get cell)` → value
- `(mutable-cell/set! cell val)` → `cell`

Legacy aliases:
- `vector/set!`, `vector/push!` may be provided as aliases but must not change the semantics of immutable `vector`.

#### 5.2.3 VM support

Either:
- Compile `mutable-array/set!` to a direct `CallNative` to a specialized primitive, or
- Add dedicated opcodes `OpSetMutableArray`, `OpGetMutableArray`, `OpPushMutableArray`.

Recommendation: use `CallNative` primitives first; add opcodes only if profiling shows call overhead matters.

#### 5.2.4 1brc impact

Replace per-row stats vector allocation:

```sema
(define stats (mutable-array/new 4))
(mutable-array/set! stats 0 temp) ; min
(mutable-array/set! stats 1 temp) ; max
(mutable-array/set! stats 2 temp) ; sum
(mutable-array/set! stats 3 1)    ; count
```

Then update in place each row:

```sema
(mutable-array/set! stats 0 (min temp (mutable-array/ref stats 0)))
(mutable-array/set! stats 1 (max temp (mutable-array/ref stats 1)))
(mutable-array/set! stats 2 (+ temp (mutable-array/ref stats 2)))
(mutable-array/set! stats 3 (+ 1 (mutable-array/ref stats 3)))
```

### 5.3 Byte strings, slices, and byte-oriented I/O

#### 5.3.1 New heap type: `ByteString`

```rust
pub struct ByteString {
    bytes: Box<[u8]>,
}
```

`ByteString` is immutable and owns its bytes. Slicing produces a `StringSlice` referencing the same `ByteString`.

```rust
pub struct StringSlice {
    owner: GcRef<ByteString>,
    start: usize,
    end: usize,
}
```

`StringSlice` is also immutable and O(1) to create.

#### 5.3.2 Public APIs

- `(bytes/read-file path)` → `ByteString`
- `(bytes/slice bs start end)` → `StringSlice`
- `(bytes/split bs sep)` → list of `StringSlice`s
- `(bytes/index-of bs needle)` → int or nil
- `(bytes/parse-int10 bs)` → int
- `(bytes/parse-float bs)` → float
- `(bytes/->string bs)` → Sema UTF-8 string
- `(bytes/->symbol bs)` → interned symbol
- `(bytes/length bs)` → int
- `(bytes/ref bs idx)` → int (byte value)

#### 5.3.3 I/O APIs

- `(file/read-bytes path)` already exists; ensure it returns a `ByteString` (or add a fast path).
- `(file/fold-chunks path size fn init)` → fold over fixed-size byte chunks.
- `(file/fold-lines2 path fn init)` → fold over lines as `StringSlice`s without allocating per-line strings.

#### 5.3.4 1brc impact

Enable byte-scan implementation:

```sema
(define data (bytes/read-file input-file))
(fold-lines-bytes data
  (fn (acc line)
    (let* ((semi (bytes/index-of line ";"))
           (name (bytes/slice line 0 semi))
           (temp-bs (bytes/slice line (+ semi 1) (bytes/length line)))
           (temp (bytes/parse-int10 temp-bs)) ; or parse-float
           ...)
      ...))
  acc)
```

This mirrors the optimized Janet/Racket/Gauche approach.

## 6. Implementation phases

The spec is intentionally decomposed into phases that can be paused or deferred independently.

### Phase 1: Mutable arrays/cells + byte-slice APIs

- Does not require GC.
- Adds new heap types that initially live under `Rc<T>`.
- Provides immediate 1brc wins.
- Keeps existing `Value`/`Rc` infrastructure intact.

### Phase 2: Tracing GC

- Replaces `Rc<T>` with `GcRef<T>` for all heap types.
- Mutable arrays and byte strings from Phase 1 become GC-managed.
- Largest and riskiest phase.

### Phase 3: Optimize 1brc benchmark

- Rewrite `benchmarks/1brc/1brc.sema` to use Phase 1 APIs.
- Update `benchmarks/1brc/simple/1brc.sema` if beneficial.
- Update docs in `website/docs/internals/performance.md` and `website/docs/internals/lisp-comparison.md`.

## 7. Files and modules to touch

- `crates/sema-core/src/value.rs` — value representation, constructors, `ValueView`.
- `crates/sema-core/src/gc.rs` — new GC module.
- `crates/sema-core/src/env.rs` (or value.rs where `Env` lives) — migrate to GC-managed.
- `crates/sema-core/src/lib.rs` — export GC types.
- `crates/sema-vm/src/vm.rs` — root registration, value handling, opcodes.
- `crates/sema-vm/src/opcodes.rs` — new opcodes if needed.
- `crates/sema-vm/src/lower.rs` / `compiler.rs` — compile new forms.
- `crates/sema-stdlib/src/string.rs` — byte-string APIs.
- `crates/sema-stdlib/src/list.rs` — mutable array APIs.
- `crates/sema-stdlib/src/bytevector.rs` — consider merging or aliasing with byte-string APIs.
- `crates/sema-stdlib/src/io.rs` — byte-oriented file APIs.
- `crates/sema-stdlib/src/map.rs` — migrate hashmap to GC.
- `benchmarks/1brc/1brc.sema` — optimized implementation.
- `website/docs/internals/performance.md` — update internals.
- `website/docs/internals/lisp-comparison.md` — update benchmark notes.

## 8. Testing strategy

- All existing tests must pass after each phase.
- Add `crates/sema/tests/gc_test.rs` with:
  - Simple allocation/collection smoke tests.
  - Liveness tests (objects reachable from roots survive).
  - Collection during nested calls.
- Add `crates/sema/tests/mutable_array_test.rs`:
  - Mutation semantics.
  - Sharing semantics (two refs see same mutations).
  - Interaction with closures and upvalues.
- Add `crates/sema/tests/bytes_test.rs`:
  - Slice identity and immutability.
  - Parse-int10 and parse-float edge cases.
- Add benchmark tracking:
  - `benchmarks/1brc/run-native-benchmarks.py` already records times; ensure Sema numbers are captured.

## 9. Risks and mitigations

| Risk | Mitigation |
|------|-----------|
| GC use-after-free | Conservative stack scanning, root handles, extensive tests |
| GC pauses hurt interactivity | Start non-moving; measure pause times on REPL workloads |
| Mutable types break immutability expectations | New type names, do not alter `vector`/`list` |
| Byte strings confuse string APIs | Keep `string` UTF-8; `bytes` is separate |
| WASM stack scanning | Provide explicit root API for WASM target |
| Large refactor destabilizes | Phase 1 first, one type migration at a time, feature flags where possible |

## 10. Open questions

1. Should `Env` become a first-class GC object, or remain a special root?
2. Should the GC be global per thread, per `EvalContext`, or per VM?
3. How are finalizers handled for streams and async resources?
4. Should `bytes/split` return a list or a vector of slices?
5. Should `MutableArray` support typed storage (f64/i64) for numeric arrays?

## 11. Related docs

- `website/docs/internals/bytecode-format.md` — any new opcodes must update the format spec and serializer.
- `website/docs/internals/performance.md` — current performance internals.
- `website/docs/internals/lisp-comparison.md` — benchmark methodology and results.
