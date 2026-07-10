---
outline: [2, 3]
---

# Bytecode File Format (`.semac`)

::: tip Versioned build artifact
The `.semac` format is stable and used in production — `sema compile`, `sema disasm`, and `sema build` all rely on it, and a verifier guarantees untrusted files can be loaded safely. It is **versioned** (currently `5`): the header records the format version, and the loader requires an exact match, so a `.semac` is a build artifact tied to the Sema version that produced it rather than a long-term portable interchange format. When a format change bumps the version, recompile from source. See [Versioning Strategy](#versioning-strategy).
:::

## Overview

Sema supports compiling source files to bytecode files (`.semac`) for faster loading and distribution without source. The compilation pipeline is:

```
Source (.sema) → Reader → Lower → Optimize → Resolve → Compile → Serialize → .semac file
```

Loading a `.semac` file skips parsing, lowering, resolution, and compilation — the VM directly deserializes and executes the pre-compiled bytecode.

### CLI Interface

```bash
# Compile a source file to bytecode
sema compile script.sema                   # → script.semac
sema compile -o output.semac script.sema   # explicit output path

# Run a bytecode file (auto-detected via magic number)
sema script.semac

# Validate a bytecode file
sema compile --check script.semac

# Disassemble a bytecode file
sema disasm script.semac
sema disasm --json script.semac            # structured JSON output
```

### Design Goals

1. **Fast loading** — skip parsing and compilation; the primary benefit (like Lua's `luac`)
2. **Source protection** — distribute without revealing source code
3. **Debuggability** — optional debug sections for source maps, local names, breakpoints
4. **Forward compatibility** — version field allows graceful rejection of incompatible bytecode
5. **Simplicity** — flat section-based format, no complex container (no ELF, no zip)

### Non-Goals

- **Portability** — bytecode files are tied to the Sema version that produced them (like Lua). Always keep source files.
- **AOT native compilation** — Sema's dynamic nature (eval, macros, LLM primitives) makes this impractical
- **Streaming** — the entire file is read into memory; no mmap or lazy loading

## File Layout

A `.semac` file consists of a fixed **header**, followed by a sequence of **sections**. Each section has a type tag, length, and payload.

```
┌──────────────────────────────────────┐
│           File Header (24 bytes)     │
├──────────────────────────────────────┤
│  Section: String Table    (required) │
├──────────────────────────────────────┤
│  Section: Function Table  (required) │
├──────────────────────────────────────┤
│  Section: Main Chunk      (required) │
├──────────────────────────────────────┤
│  Section: Source Map      (optional) │
├──────────────────────────────────────┤
│  Section: Debug Symbols   (optional) │
├──────────────────────────────────────┤
│  Section: Breakpoints     (optional) │
├──────────────────────────────────────┤
│  ... future sections ...             │
└──────────────────────────────────────┘
```

All multi-byte integers are **little-endian**. All strings are **UTF-8**.

## File Header

| Offset | Size | Field | Description |
|--------|------|-------|-------------|
| 0 | 4 | `magic` | `\x00SEM` (`0x00`, `0x53`, `0x45`, `0x4D`) |
| 4 | 2 | `format_version` | Bytecode format version (currently `5`) |
| 6 | 2 | `flags` | Bit flags (see below) |
| 8 | 2 | `sema_major` | Sema version major that produced this file |
| 10 | 2 | `sema_minor` | Sema version minor |
| 12 | 2 | `sema_patch` | Sema version patch |
| 14 | 2 | `n_sections` | Number of sections in the file |
| 16 | 4 | `source_hash` | CRC-32 of the original source file (0 if unknown) |
| 20 | 4 | `reserved` | Reserved for future use (must be 0) |

**Total: 24 bytes**

### Magic Number

The magic bytes `\x00SEM` serve two purposes:
1. **File type identification** — the CLI uses this to auto-detect bytecode vs source (source files never start with a null byte)
2. **Corruption detection** — if the magic doesn't match, reject the file immediately

### Flags (Bit Field)

| Bit | Name | Description |
|-----|------|-------------|
| 0 | `HAS_DEBUG` | File contains debug sections (Source Map, Debug Symbols) |
| 1 | `HAS_SOURCE_MAP` | File contains a Source Map section |
| 2 | `HAS_BREAKPOINTS` | File contains a Breakpoints section |
| 3–15 | — | Reserved (must be 0) |

The current serializer always writes `flags = 0` — debug sections (and a `--strip` flag to omit them) are not yet implemented.

## Section Format

Each section begins with a section header:

| Offset | Size | Field | Description |
|--------|------|-------|-------------|
| 0 | 2 | `section_type` | Section type tag (see table) |
| 2 | 4 | `section_length` | Byte length of section payload (excluding this header) |

**Section header: 6 bytes**, followed by `section_length` bytes of payload.

### Section Types

| Type ID | Name | Required | Description |
|---------|------|----------|-------------|
| `0x01` | String Table | ✅ | All interned strings (Spur remapping) |
| `0x02` | Function Table | ✅ | Compiled function templates |
| `0x03` | Main Chunk | ✅ | Top-level bytecode |
| `0x10` | Source Map | — | Source file name + PC-to-line mapping |
| `0x11` | Debug Symbols | — | Local variable names per function |
| `0x12` | Breakpoints | — | Reserved for breakpoint table |
| `0x13` | Debug Scopes | — | Reserved for lexical scope ranges |

The three required sections are always written, in the order above, so `n_sections` in the header is currently always `3`. The `0x10`–`0x13` debug sections are **reserved tags only** — the current serializer never emits them and defines no constants for them yet; they are documented here so a future writer and any third-party reader agree on the IDs. Unknown section types are **skipped** (forward compatibility), so a reader that ignores them stays compatible.

## String Table (Section `0x01`)

The string table contains all unique strings referenced by the bytecode, including:
- Symbol names (global identifiers, function names)
- Keyword names
- String constants in the constant pool
- Source file paths (in debug sections)

```
┌────────────────────────────┐
│  count: u32                │  Number of strings
├────────────────────────────┤
│  String Entry 0            │
│    len: u32                │  Byte length of UTF-8 data
│    data: [u8; len]         │  UTF-8 bytes (no null terminator)
├────────────────────────────┤
│  String Entry 1            │
│    ...                     │
└────────────────────────────┘
```

On load, each string is interned into the process-local `lasso::Rodeo` (a thread-local interner), producing a fresh `Spur`. The loader builds a **remap table** (`Vec<Spur>`) mapping file-local string indices to process-local Spurs.

String index `0` is reserved and must be the empty string `""`.

## Main Chunk (Section `0x03`)

The main chunk contains the top-level bytecode and its constant pool.

```
┌────────────────────────────────┐
│  code_len: u32                 │
│  code: [u8; code_len]          │  Raw bytecode
├────────────────────────────────┤
│  n_consts: u16                 │
│  constants: [SerializedValue]  │  Constant pool entries
├────────────────────────────────┤
│  n_spans: u32                  │
│  spans: [(u32 pc, u32 line,    │  PC → source location
│           u32 col, u32         │
│           end_line, u32        │
│           end_col)]            │
├────────────────────────────────┤
│  max_stack: u16                │
│  n_locals: u16                 │
│  n_global_cache_slots: u16     │  Inline cache slots for global lookups
├────────────────────────────────┤
│  n_exceptions: u16             │
│  exceptions: [ExceptionEntry]  │  Exception table
└────────────────────────────────┘
```

### Exception Entry (16 bytes each)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 4 | `try_start` (PC) |
| 4 | 4 | `try_end` (PC) |
| 8 | 4 | `handler_pc` |
| 12 | 2 | `stack_depth` |
| 14 | 2 | `catch_slot` |

## Function Table (Section `0x02`)

```
┌────────────────────────────────┐
│  count: u32                    │  Number of functions
├────────────────────────────────┤
│  Function Entry 0              │
│    name: u32                   │  String table index (0xFFFFFFFF = anonymous)
│    arity: u16                  │
│    has_rest: u8                │  0 or 1
│    n_upvalue_descs: u16        │
│    upvalue_descs: [UpvalueDesc]│
│    n_upvalue_names: u16        │
│    upvalue_names: [u32 name]   │  Lexical names aligned with upvalue_descs
│    chunk: [Chunk data]         │  Same format as Main Chunk
│    n_local_names: u16          │
│    local_names: [(u16 slot,    │  Local variable debug info
│                   u32 name)]   │  (name = string table index)
│    n_local_scopes: u16         │
│    local_scopes: [(u16 slot,   │  Block-scope ranges (debug metadata)
│                    u32 start,  │  half-open [start_pc, end_pc) per
│                    u32 end)]   │  block-introduced local
├────────────────────────────────┤
│  Function Entry 1              │
│    ...                         │
└────────────────────────────────┘
```

### Local Scopes (10 bytes each)

`local_scopes` records the half-open bytecode PC range `[start_pc, end_pc)` over
which each block-introduced local (from `let` / `let*` / `letrec` / `do`) is
live. The debugger uses these ranges to hide locals that are not yet bound or
already out of scope at the current PC. This is debug-only metadata — it is never
read during execution. Functions whose `local_scopes` is empty (e.g. those with
only parameters, or older `.semac` files) cause the debugger to show all locals.

| Offset | Size | Field |
|--------|------|-------|
| 0 | 2 | `slot` — local variable slot |
| 2 | 4 | `start_pc` — PC where the binding comes into scope |
| 6 | 4 | `end_pc` — PC where the binding goes out of scope (exclusive) |

### Upvalue Descriptor (3 bytes each)

| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | `kind`: 0 = ParentLocal, 1 = ParentUpvalue |
| 1 | 2 | `index`: slot/upvalue index in parent |

::: warning Bytecode inline encoding differs
The upvalue descriptors in the **function table** (above) use a compact 3-byte encoding (`u8` kind + `u16` index). However, the `MakeClosure` opcode in the **bytecode stream** uses a 4-byte encoding per upvalue: `u16` is_local + `u16` index. This wider encoding is used for alignment in the runtime bytecode.
:::

## Serialized Values (Constant Pool)

Each constant is serialized as a **type tag** (1 byte) followed by type-specific payload.

| Tag | Type | Payload |
|-----|------|---------|
| `0x00` | Nil | — (0 bytes) |
| `0x01` | Bool | 1 byte: `0x00` = false, `0x01` = true |
| `0x02` | Int | 8 bytes: i64 little-endian |
| `0x03` | Float | 8 bytes: f64 little-endian (IEEE 754) |
| `0x04` | String | 4 bytes: string table index (u32) |
| `0x05` | Symbol | 4 bytes: string table index (u32) |
| `0x06` | Keyword | 4 bytes: string table index (u32) |
| `0x07` | Char | 4 bytes: Unicode code point (u32) |
| `0x08` | List | 2 bytes: count (u16), then `count` recursive SerializedValues |
| `0x09` | Vector | 2 bytes: count (u16), then `count` recursive SerializedValues |
| `0x0A` | Map | 2 bytes: n_pairs (u16), then `n_pairs × 2` recursive SerializedValues (key, value alternating) |
| `0x0B` | HashMap | Same as Map (`0x0A`) — tag distinguishes sorted vs hash map |
| `0x0C` | Bytevector | 4 bytes: length (u32), then `length` raw bytes |
| `0x0D` | BigInt | 4 bytes: byte-length (u32), then that many bytes of two's-complement little-endian magnitude (`num-bigint`'s `to_signed_bytes_le`) |
| `0x0E` | Rational | Two `0x0D`-style parts (numerator, then denominator): each a 4-byte byte-length (u32) followed by that many bytes of two's-complement little-endian magnitude |
| `0x0F` | Complex | Two nested SerializedValues (real part, then imaginary part); each component is itself an Int/BigInt/Rational/Float SerializedValue |

### Values That Cannot Appear in Bytecode

The following `ValueView` variants are **runtime-only** and must never appear in a `.semac` constant pool:

- `Lambda` / `Macro` — closures are constructed at runtime via `MakeClosure`
- `NativeFn` — registered by the runtime, not serializable
- `Prompt` / `Message` / `Conversation` — constructed via `__vm-prompt` / `__vm-message`
- `ToolDef` / `Agent` — constructed via `__vm-deftool` / `__vm-defagent`
- `Thunk` — created by `delay`
- `Record` — constructed by `define-record-type`
- `AsyncPromise` (tag 28) — created by `async/spawn`, runtime-only
- `Channel` (tag 29) — created by `channel/new`, runtime-only

If the serializer encounters any of these in a constant pool, it should emit a compile error.

## Spur Remapping

Sema uses `lasso::Spur` (process-local interned string handles) for symbols, keywords, and global variable names. These handles are **not stable** across processes.

### In the bytecode stream

Global variable opcodes (`LoadGlobal`, `StoreGlobal`, `DefineGlobal`, `CallGlobal`) encode Spur values as `u32`. `LoadGlobal` additionally carries a `u16` inline-cache slot operand, and `CallGlobal` carries `u16 argc` + `u16` cache slot — these are copied through unchanged; only the `u32` Spur operand is remapped. On serialization:

1. The serializer collects all Spurs referenced in the bytecode (globals, function names, local names)
2. Each Spur's string is added to the string table, getting a file-local index
3. The bytecode is **rewritten**: Spur-encoded u32 operands are replaced with string table indices

On deserialization:

1. The string table is loaded and each string is interned → new process-local Spurs
2. A remap table maps file-local indices to process-local Spurs
3. The bytecode is walked: `LoadGlobal`/`StoreGlobal`/`DefineGlobal`/`CallGlobal` operands are rewritten with the new Spur u32 values

This is the same approach Lua uses for upvalue names, and Guile uses for its symbol table.

## Source Map (Section `0x10`)

::: info Future Feature
This section is defined but not yet implemented.
:::

The source map links bytecode PCs back to source file locations, enabling error messages with file/line info when running from `.semac` files.

```
┌────────────────────────────────┐
│  source_file: u32              │  String table index of source file path
│  source_hash: [u8; 32]        │  SHA-256 of the original source
├────────────────────────────────┤
│  n_entries: u32                │
│  entries: [SourceMapEntry]     │  Sorted by PC, delta-encoded
└────────────────────────────────┘
```

### Source Map Entry (delta-encoded, variable-length)

For compact representation, entries are delta-encoded from the previous entry:

| Field | Encoding | Description |
|-------|----------|-------------|
| `delta_pc` | LEB128 u32 | PC offset from previous entry |
| `delta_line` | LEB128 i32 | Line offset from previous entry |
| `delta_col` | LEB128 i32 | Column offset from previous entry |

The first entry uses absolute values (delta from 0).

## Debug Symbols (Section `0x11`)

::: info Future Feature
This section is defined but not yet implemented.
:::

Debug symbols provide local variable names and their scope ranges within each function, enabling meaningful debugger variable inspection.

```
┌────────────────────────────────┐
│  n_functions: u32              │  Must match Function Table count
├────────────────────────────────┤
│  Function 0 debug info         │
│    n_locals: u16               │
│    locals: [LocalDebugEntry]   │
├────────────────────────────────┤
│  Function 1 debug info         │
│    ...                         │
└────────────────────────────────┘
```

### Local Debug Entry

| Offset | Size | Field |
|--------|------|-------|
| 0 | 4 | `name` — string table index |
| 4 | 2 | `slot` — local variable slot |
| 6 | 4 | `scope_start` — PC where variable comes into scope |
| 10 | 4 | `scope_end` — PC where variable goes out of scope |

## Breakpoints Section (Section `0x12`)

::: info Future Feature
This section is reserved for debugger integration. Format TBD.
:::

The breakpoints section will support:
- **Persistent breakpoints** — set breakpoints by source location; they survive recompilation
- **Conditional breakpoints** — attach Sema expressions as conditions
- **Source-mapped breakpoints** — store breakpoints as `(file, line)` pairs, resolved to PCs on load

Planned entry format:

```
┌────────────────────────────────┐
│  n_breakpoints: u32            │
├────────────────────────────────┤
│  Breakpoint Entry              │
│    source_file: u32            │  String table index
│    line: u32                   │
│    col: u32                    │  0 = any column
│    condition_len: u16          │  0 = unconditional
│    condition: [u8]             │  Sema source expression (UTF-8)
│    flags: u8                   │  0x01 = enabled, 0x02 = one-shot
└────────────────────────────────┘
```

## Debug Scopes Section (Section `0x13`)

::: info Future Feature
This section is reserved for lexical scope tracking. Format TBD.
:::

Debug scopes will map PC ranges to lexical scopes, enabling:
- Accurate "step over" / "step into" behavior
- Proper variable shadowing display in debuggers
- Scope-aware watch expressions

## Validation

When loading a `.semac` file, the loader performs these checks:

1. **Magic number** — must be `\x00SEM`
2. **Format version** — must exactly match the version this Sema build supports
3. **Reserved header field** — must be zero
4. **Section completeness** — all three required sections must be present (and string index 0 must be `""`)
5. **String table bounds** — all string table indices in the file must be in range
6. **Function table bounds** — all `func_id` references in `MakeClosure` must be valid
7. **Constant pool types** — no runtime-only value types in the constant pool
8. **Bytecode well-formedness** — chunks must be non-empty, opcodes must be valid, operand sizes must be correct, constant/local/upvalue/`CallNative` native indices must be in bounds, and jump targets must land on instruction boundaries (the native table is process-local and unserialized, so its loaded length is `0` — any `CallNative` in a `.semac` is rejected)
9. **Stack-depth balance** — an abstract-interpretation pass over every chunk (main chunk and each function) proves the operand stack never underflows and never exceeds the maximum depth

If validation fails, the loader returns a `SemaError` with a descriptive message.

### Stack-Depth Verifier (ADR #56)

The VM's hot dispatch loop uses an unchecked stack pop (`pop_unchecked`) for speed, which is sound only if the bytecode is stack-balanced. In-process bytecode is balanced by construction; deserialized `.semac` bytecode is proven balanced by a verifier that runs inside `validate_bytecode` before `deserialize_from_bytes` returns.

The verifier abstract-interprets each chunk:

- Each opcode has a static stack effect (`Op::stack_effect()` — the single source of truth shared with the VM dispatch arms). Variable-arity opcodes (`Call`, `TailCall`, `SelfTailCall`, `CallSelf`, `CallGlobal`, `CallNative`, `MakeList`, `MakeVector`, `MakeMap`, `MakeHashMap`) compute their effect from the decoded operand count. `SelfTailCall` and `CallSelf` pop only their `argc` args (the callee is the running frame's own closure, not a stack value); `SelfTailCall` additionally exits the frame; the other calls pop the callee too.
- A worklist tracks the operand-stack depth on entry to every reachable instruction, following fallthrough and jump edges. Exception handlers are seeded as additional roots at their known entry depth (`stack_depth - n_locals + 1`).
- Join points must agree on depth exactly (strict-equality lattice, like the JVM/CLR verifiers). A disagreement, a reachable pop deeper than the current depth (underflow), a depth above the maximum (overflow), or control falling off the end of a chunk (including a reachable jump to end-of-code) are all rejected with a descriptive `SemaError`.

The verifier is **sound** — it never accepts an underflowing chunk. It is intentionally conservative: it may reject exotic-but-safe bytecode that a future optimizing compiler could emit, but accepts every program Sema's compiler produces. Once verification succeeds, `.semac` files from untrusted sources can be loaded without risking the unchecked-pop undefined behavior. The same walk also establishes the pc-bounds invariants: every reachable instruction decodes in bounds, every reachable control transfer lands on an in-chunk instruction, and exception handler pcs are bounds-checked.

The loader also enforces two hard limits while deserializing, both of which a re-implementation must respect to stay compatible: a chunk may declare a maximum stack depth of at most **65535** (`MAX_STACK_DEPTH`), and a constant-pool value may nest at most **128** levels deep (`MAX_VALUE_DEPTH`) — the latter bounds recursion in the value deserializer so a hostile file can't blow the native stack. Both are defined in `serialize.rs`.

## Opcodes

The complete, numbered opcode set lives in `crates/sema-vm/src/opcodes.rs` (the `Op` enum and its `Op::from_u8` mapping are the single source of truth). Most opcodes are single-byte; a handful carry inline operands (`u16`/`u32`/`i32`) as noted in the encoding descriptions above.

To keep the common path off the `CallGlobal` → hash-lookup → `NativeFn` route, a set of **inline stdlib intrinsics** are compiled directly to dedicated single-byte opcodes when the call site references the canonical global with the matching arity (and that global has not been redefined in the program). These include list/collection ops (`Car`, `Cdr`, `Cons`, `Append`, `Length`, `Get`, `Nth`, …), type predicates (`IsNull`, `IsString`, …), and **string ops**:

| Opcode | Source name(s) | Arity | Stack effect | Behavior |
|--------|----------------|-------|--------------|----------|
| `StringLength` (0x42) | `string-length` | 1 | pop 1, push 1 | push char count (`chars().count()`) of the string; type error if not a string |
| `StringRef` (0x43) | `string-ref` | 2 | pop 2, push 1 | push the char at the 0-based char index; errors on negative index, non-int index, non-string, or out-of-bounds index (matching the stdlib messages) |
| `StringAppend` (0x44) | `string-append` | 2 | pop 2, push 1 | push the concatenation of two values (non-strings coerced via `Display`); the N-ary `string-append` stays on the generic path |

String indexing is by **Unicode scalar (char)**, not byte, matching the stdlib semantics. These opcodes are additive within the existing encoding (single-byte, no new operand shapes), so they do not change the `format_version`.

### Self-tail-call (`SelfTailCall` 0x45)

`SelfTailCall argc` (`u16 argc`) is a tail call whose callee is the **current frame's own closure**, so — unlike `TailCall` — no callee value is pushed onto the stack; only the `argc` args are. The compiler emits it for a self-recursive named-let / `letrec` loop whose name is referenced only in tail-call position: the resolver elides the self upvalue (see `VarResolution::SelfFn` in `resolve.rs`), so the running frame reads its own closure instead of a captured cell — eliminating the per-entry self-reference cycle (issue #62 / ADR #66). It reuses the frame in place (rebind args, jump to entry, arity-checked). It is also emitted for a tail self-call inside an eligible top-level define's own lambda (see `CallSelf` below). Additive within the existing encoding (`u16` operand, no new operand shape), so it does not change `format_version`.

### Direct self-call (`CallSelf` 0x46)

`CallSelf argc` (`u16 argc`) is the non-tail counterpart of `SelfTailCall`: a call whose callee is the **current frame's own closure**, so no callee value is on the stack and no global lookup or callable dispatch happens — the VM pushes a frame for the running closure directly (arity-checked, rest params supported). The compiler emits it inside a top-level `(define (f …))` lambda for direct non-tail calls to `f` when nothing else in the program rebinds the name — a second `define` of `f` or a global `set!` of `f` at any depth opts the name out (the intrinsics' program-wide redefinition rule, extended to `set!`). Tail-position self-calls under the same eligibility emit `SelfTailCall`. Value (non-operator) references to `f` remain `LoadGlobal`, so identity (`eq?`) and escape semantics are unchanged. Additive within the existing encoding (`u16` operand, no new operand shape), so it does not change `format_version`.

### Take-local (`TakeLocal` 0x47)

`TakeLocal slot` (`u16 slot`) is a **moving** `LoadLocal`: it pushes `locals[slot]` and replaces the slot with nil, instead of cloning (refcount-bumping) the slot value. Same encoding and stack effect as `LoadLocal` (push 1, pop 0). The compiler emits it for the *statically last* use of a local slot that is never captured by an inner lambda and never a `set!` target, as proven by a conservative backward liveness analysis (`crates/sema-vm/src/takelocal.rs`); functions containing `try`, `do` loops, or self-frame-reuse calls opt out entirely. Dropping the dead slot reference lets uniquely-owned values hit the stdlib's `strong_count == 1` in-place fast paths (`assoc`/`dissoc`/`update` on maps, etc.) instead of deep-cloning.

Because the slot is proven dead, the nil left behind is unobservable by the program. The only surface that can still read the slot is the **debug inspector** (DAP variable views / `evaluate`): a variable whose last use has executed displays as `nil` for the remainder of its lexical scope. This is accepted debugger behavior, mirroring registerized locals in native debuggers. Additive within the existing encoding (`u16` operand, no new operand shape), so it does not change `format_version`.

### Mutable-array accessors (`MutArrGet` 0x48, `MutArrSet` 0x49)

Single-byte inline intrinsics for the `mutable-array` accessors, following the same emission rules as the other stdlib intrinsics (canonical global name, exact arity, name not redefined anywhere in the program):

| Opcode | Source form | Stack effect | Behavior |
|--------|-------------|--------------|----------|
| `MutArrGet` (0x48) | `(mutable-array/get arr idx)` | pop 2, push 1 | push `arr[idx]`; errors on non-array, negative/non-int index, or out-of-bounds index |
| `MutArrSet` (0x49) | `(mutable-array/set! arr idx val)` | pop 3, push 1 | `arr[idx] = val`, push the array itself (the Sema-level return value); errors on non-array, negative/non-int index, or out-of-bounds index |

The 3-arg (default) form of `mutable-array/get` and any wrong-arity call stay on the generic `CallGlobal` path — the native owns the default logic and the arity errors. Both opcodes share their implementation with the stdlib natives (`sema_core::mutable_ops`), so error messages are byte-identical across dispatch paths. Additive within the existing encoding (single-byte, no new operand shapes), so they do not change `format_version`.

## Example

Given this source file:

```sema
;; hello.sema
(define greeting "Hello, World!")
(println greeting)
```

The compiled `.semac` would contain:

**String Table**: `["", "greeting", "println", "Hello, World!"]`

**Main Chunk bytecode** (conceptual):
```
0000  CONST         0    ; "Hello, World!" (string constant)
0003  DEFINE_GLOBAL 1    ; greeting (string table index → Spur)
0008  LOAD_GLOBAL   2    ; println (+ u16 inline-cache slot)
0015  LOAD_GLOBAL   1    ; greeting (+ u16 inline-cache slot)
0022  CALL          1
0025  RETURN
```

**Function Table**: (empty — no inner functions)

## Reading a Real `.semac`, Byte by Byte

The layout above is easier to trust when you can see every byte of an actual file. Here is the smallest interesting program, compiled and dumped in full — no diagrams, the real 85 bytes:

```bash
$ echo '(+ 1 2)' > tiny.sema
$ sema compile tiny.sema -o tiny.semac
$ sema disasm tiny.semac
== <main> ==
0000  CONST            0    ; 3
0003  RETURN

$ xxd tiny.semac
00000000: 0053 454d 0400 0000 0100 1300 0100 0300  .SEM............
00000010: d9b7 a83a 0000 0000 0100 0800 0000 0100  ...:............
00000020: 0000 0000 0000 0200 0400 0000 0000 0000  ................
00000030: 0300 1f00 0000 0400 0000 0000 0012 0100  ................
00000040: 0203 0000 0000 0000 0000 0000 0000 0000  ................
00000050: 0000 0000 00                             .....
```

Notice the compiler already **constant-folded** `(+ 1 2)` into the literal `3` — the [Optimize pass](./bytecode-vm.md) ran before serialization, so the only instruction is a `CONST` that pushes a pooled `3`, then `RETURN`. Now every byte:

```
offset  bytes                      meaning
------  -----------------------    --------------------------------------------
HEADER (24 bytes)
 0x00   00 53 45 4D                magic  "\x00SEM"
 0x04   05 00                      format_version = 5
 0x06   00 00                      flags = 0
 0x08   01 00                      sema major = 1   ┐
 0x0A   13 00                      sema minor = 19  ├ compiled by Sema 1.19.1
 0x0C   01 00                      sema patch = 1   ┘
 0x0E   03 00                      n_sections = 3
 0x10   D9 B7 A8 3A                source_hash = 0x3AA8B7D9  (CRC-32 of source)
 0x14   00 00 00 00                reserved

SECTION 1 — String Table (type 0x01)
 0x18   01 00                      section type = 0x01
 0x1A   08 00 00 00                section length = 8 bytes
 0x1E   01 00 00 00                string count = 1
 0x22   00 00 00 00                string[0]: length 0  (the reserved empty string at index 0)

SECTION 2 — Function Table (type 0x02)
 0x26   02 00                      section type = 0x02
 0x28   04 00 00 00                section length = 4 bytes
 0x2C   00 00 00 00                function count = 0   (no lambdas in this program)

SECTION 3 — Main Chunk (type 0x03)
 0x30   03 00                      section type = 0x03
 0x32   1F 00 00 00                section length = 31 bytes
        ── chunk body ──
 0x36   04 00 00 00                code length = 4 bytes
 0x3A   00                           CONST           (opcode 0)
 0x3B   00 00                        └ operand: constant index 0
 0x3D   12                           RETURN          (opcode 18 = 0x12)
 0x3E   01 00                      constant count = 1
 0x40   02                         const[0] tag = VAL_INT (0x02)
 0x41   03 00 00 00 00 00 00 00      └ i64 value = 3
 0x49   00 00 00 00                span count = 0
 0x4D   00 00                      max_stack = 0
 0x4F   00 00                      n_locals = 0
 0x51   00 00                      n_global_cache_slots = 0
 0x53   00 00                      exception count = 0
```

That is the whole format with nothing hidden: a 24-byte header, three length-prefixed sections, and a chunk whose four instruction-bytes (`00 00 00 12`) are literally `CONST 0` / `RETURN`. A program that referenced a global would add `"println"` to the string table and `LOAD_GLOBAL`/`CALL` opcodes to the chunk (as in the conceptual example above); a program with a `lambda` would add an entry to the function table. Everything else is more of the same.

::: tip Want to build the instructions themselves first?
[Build a Bytecode VM (in Sema)](./build-a-bytecode-vm.md) constructs a working compiler and stack machine from scratch in ~80 lines, so the `CONST`/`RETURN` stream above reads as the natural output of a process you've already seen end to end.
:::

## Versioning Strategy

- `format_version` started at `1` and increments on any breaking change to the binary format. Version `2` added `n_global_cache_slots` and the inline-cache operands; version `3` added per-function upvalue names to the debug metadata; version `4` added per-function `local_scopes` (block-scope PC ranges) to the debug metadata; version `5` (current) added the `BigInt`/`Rational`/`Complex` constant tags (`0x0D`–`0x0F`) for the numeric tower.
- `sema_major`/`sema_minor`/`sema_patch` record the compiler version for diagnostics
- The loader requires an exact `format_version` match and refuses anything else with a clear error: `"unsupported bytecode format version 1 (expected 5). Recompile from source."`
- Within the same `format_version`, new section types can be added without breaking older loaders (unknown sections are skipped)

## Comparison with Other Languages

| Feature | Sema (`.semac`) | Lua (`luac.out`) | Python (`.pyc`) | Erlang (`.beam`) | Guile (`.go`) |
|---------|-----------------|------------------|-----------------|------------------|---------------|
| Format | Flat sections | Flat binary | Header + marshal | IFF chunks | ELF container |
| Portable | No (version-tied) | No (arch-tied) | No (version-tied) | Yes | Yes |
| Debug info | Optional sections | Optional (`-s` strips) | Included | Included | Included |
| Auto-detect | Magic `\x00SEM` | Magic `\033Lua` | Magic `\xNN\r\n` | Magic `FOR1` | ELF header |
| Cache invalidation | CRC-32 source hash | N/A | Timestamp or hash | N/A | N/A |
| Spur/symbol remap | String table + rewrite | Upvalue names | marshal interning | Atom table | Symbol table |
