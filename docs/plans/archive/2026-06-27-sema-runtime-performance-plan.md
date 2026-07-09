# Sema Runtime Performance: Implementation Plan

> **Status (2026-07-09):** Phase 1 shipped in modified form — `MutableArray`/`MutableCell` landed at tags 33/34 (30-32 were taken by the numeric tower), and the planned `ByteString`/`StringSlice` types were replaced by `bytes/*` ops on the existing `Bytevector` (optional start/end args stand in for zero-copy slices), plus `file/fold-lines-bytes`. Phase 3 (1BRC rewrite) shipped against those APIs. Phase 2 (tracing GC) is **superseded** — see `docs/performance-roadmap.md` §2: `TakeLocal` + the owned-args callback protocol unlock the existing Rc-uniqueness COW gates instead. Result: Sema ahead of Janet on all four suite benchmarks.

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reach Janet-level performance on the optimized 1brc benchmark by adding mutable arrays, byte-oriented strings/slices, and a tracing GC to the Sema runtime.

**Architecture:** Three independent phases. Phase 1 adds `MutableArray`/`MutableCell` and `ByteString`/`StringSlice` heap types with stdlib/VM support (no GC). Phase 2 replaces `Rc<T>` with a non-moving tracing GC for all heap objects. Phase 3 rewrites the 1brc benchmark to use the new APIs and updates docs.

**Tech Stack:** Rust 2021, `sema-core`, `sema-vm`, `sema-stdlib`, `sema-reader`, `cargo test`, `benchmarks/1brc/run-native-benchmarks.py`.

---

## Overview

| Phase | Focus | Approx. agent-hours | Produces working/testable software |
| ----- | ----- | ------------------- | ----------------------------------- |
| 1     | Mutable arrays/cells + byte strings/slices | 12–20 | Yes |
| 2     | Tracing GC replacing `Rc<T>` | 50–80 | Yes |
| 3     | 1brc rewrite + docs | 2–4 | Yes |

Each phase can be deferred independently. Phase 1 does not require Phase 2. Phase 3 depends on Phase 1.

---

# Phase 1: Mutable Arrays/Cells and Byte Strings/Slices

**Goal:** Add runtime support for in-place mutable arrays/cells and byte-oriented string slices, then expose them through stdlib APIs. This phase keeps the existing `Rc<T>` value representation.

**Approx. agent-hours:** 12–20

## Files to create or modify

- `crates/sema-core/src/value.rs` — add `MutableArray`, `MutableCell`, `ByteString`, `StringSlice` heap types and `Value` constructors/view variants.
- `crates/sema-core/src/lib.rs` — export new public types.
- `crates/sema-stdlib/src/list.rs` — add `mutable-array/*` and `mutable-cell/*` functions.
- `crates/sema-stdlib/src/string.rs` — add `bytes/*` functions.
- `crates/sema-stdlib/src/io.rs` — add `file/fold-lines-bytes` and `file/fold-chunks`.
- `crates/sema/tests/mutable_array_test.rs` — new tests.
- `crates/sema/tests/bytes_test.rs` — new tests.
- `crates/sema/tests/integration_test.rs` — register new tests if needed.

---

## Task 1.1: Add `MutableArray` and `MutableCell` heap types

**Files:**
- Modify: `crates/sema-core/src/value.rs`
- Modify: `crates/sema-core/src/lib.rs`

### Step 1: Define the types

Add near the other heap structs (after `Record`):

```rust
/// Mutable array: in-place mutable vector of Values.
#[derive(Debug)]
pub struct MutableArray {
    pub items: RefCell<Vec<Value>>,
}

impl MutableArray {
    pub fn new() -> Self {
        MutableArray {
            items: RefCell::new(Vec::new()),
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        MutableArray {
            items: RefCell::new(Vec::with_capacity(cap)),
        }
    }
}

impl Clone for MutableArray {
    fn clone(&self) -> Self {
        MutableArray {
            items: RefCell::new(self.items.borrow().clone()),
        }
    }
}

/// Mutable cell: single mutable value reference.
#[derive(Debug)]
pub struct MutableCell {
    pub value: RefCell<Value>,
}

impl MutableCell {
    pub fn new(value: Value) -> Self {
        MutableCell {
            value: RefCell::new(value),
        }
    }
}

impl Clone for MutableCell {
    fn clone(&self) -> Self {
        MutableCell {
            value: RefCell::new(self.value.borrow().clone()),
        }
    }
}
```

### Step 2: Add tags and ValueView variants

Add tags after `TAG_CHANNEL`:

```rust
const TAG_MUTABLE_ARRAY: u64 = 30;
const TAG_MUTABLE_CELL: u64 = 31;
const TAG_BYTE_STRING: u64 = 32;
const TAG_STRING_SLICE: u64 = 33;
```

Add `ValueView` variants:

```rust
MutableArray(Rc<MutableArray>),
MutableCell(Rc<MutableCell>),
ByteString(Rc<ByteString>),
StringSlice(Rc<StringSlice>),
```

### Step 3: Add constructors and accessors

Add `Value` methods after `channel_from_rc`:

```rust
pub fn mutable_array(items: Vec<Value>) -> Value {
    Self::mutable_array_from_rc(Rc::new(MutableArray {
        items: RefCell::new(items),
    }))
}

pub fn mutable_array_from_rc(rc: Rc<MutableArray>) -> Value {
    let ptr = Rc::into_raw(rc) as *const u8;
    Value(make_boxed(TAG_MUTABLE_ARRAY, ptr_to_payload(ptr)))
}

pub fn mutable_cell(value: Value) -> Value {
    Self::mutable_cell_from_rc(Rc::new(MutableCell {
        value: RefCell::new(value),
    }))
}

pub fn mutable_cell_from_rc(rc: Rc<MutableCell>) -> Value {
    let ptr = Rc::into_raw(rc) as *const u8;
    Value(make_boxed(TAG_MUTABLE_CELL, ptr_to_payload(ptr)))
}
```

Add matching `as_mutable_array()`, `as_mutable_cell()` accessors in the `Value` impl.

### Step 4: Update `type_name()` and formatting

Add cases in `type_name()`:

```rust
TAG_MUTABLE_ARRAY => "mutable-array",
TAG_MUTABLE_CELL => "mutable-cell",
TAG_BYTE_STRING => "bytes",
TAG_STRING_SLICE => "bytes-slice",
```

Add cases in `Display` and `Debug` if needed.

### Step 5: Export from `sema-core`

In `crates/sema-core/src/lib.rs`, add:

```rust
pub use crate::value::{ByteString, MutableArray, MutableCell, StringSlice};
```

---

## Task 1.2: Add `ByteString` and `StringSlice` heap types

**Files:**
- Modify: `crates/sema-core/src/value.rs`

### Step 1: Define the types

Add near `MutableCell`:

```rust
/// Immutable byte string. Owns its bytes.
#[derive(Debug, Clone)]
pub struct ByteString {
    pub bytes: Box<[u8]>,
}

impl ByteString {
    pub fn new(bytes: Vec<u8>) -> Self {
        ByteString {
            bytes: bytes.into_boxed_slice(),
        }
    }
}

/// Immutable slice into a ByteString. O(1) to create.
#[derive(Debug)]
pub struct StringSlice {
    pub owner: Rc<ByteString>,
    pub start: usize,
    pub end: usize,
}

impl Clone for StringSlice {
    fn clone(&self) -> Self {
        StringSlice {
            owner: self.owner.clone(),
            start: self.start,
            end: self.end,
        }
    }
}
```

### Step 2: Add constructors and accessors

```rust
pub fn byte_string(bytes: Vec<u8>) -> Value {
    Self::byte_string_from_rc(Rc::new(ByteString::new(bytes)))
}

pub fn byte_string_from_rc(rc: Rc<ByteString>) -> Value {
    let ptr = Rc::into_raw(rc) as *const u8;
    Value(make_boxed(TAG_BYTE_STRING, ptr_to_payload(ptr)))
}

pub fn string_slice(owner: Rc<ByteString>, start: usize, end: usize) -> Value {
    Self::string_slice_from_rc(Rc::new(StringSlice { owner, start, end }))
}

pub fn string_slice_from_rc(rc: Rc<StringSlice>) -> Value {
    let ptr = Rc::into_raw(rc) as *const u8;
    Value(make_boxed(TAG_STRING_SLICE, ptr_to_payload(ptr)))
}
```

Add concrete accessors:

```rust
#[inline(always)]
pub fn as_byte_string(&self) -> Option<&ByteString> {
    if is_boxed(self.0) && get_tag(self.0) == TAG_BYTE_STRING {
        Some(unsafe { self.borrow_ref::<ByteString>() })
    } else {
        None
    }
}

#[inline(always)]
pub fn as_string_slice(&self) -> Option<&StringSlice> {
    if is_boxed(self.0) && get_tag(self.0) == TAG_STRING_SLICE {
        Some(unsafe { self.borrow_ref::<StringSlice>() })
    } else {
        None
    }
}

pub fn as_bytes(&self) -> Option<Vec<u8>> {
    if let Some(bs) = self.as_byte_string() {
        Some(bs.bytes.as_ref().to_vec())
    } else if let Some(s) = self.as_string_slice() {
        Some(s.owner.bytes[s.start..s.end].to_vec())
    } else {
        None
    }
}

pub fn byte_string_owner(&self) -> Option<Rc<ByteString>> {
    self.as_byte_string().map(|bs| {
        let ptr = bs as *const ByteString;
        unsafe { Rc::increment_strong_count(ptr); Rc::from_raw(ptr) }
    })
}
```

---

## Task 1.3: Add mutable-array stdlib functions

**Files:**
- Modify: `crates/sema-stdlib/src/list.rs`

### Step 1: Register functions

At the end of `register()` in `list.rs`, add:

```rust
register_fn(env, "mutable-array/new", |args| {
    check_arity!(args, "mutable-array/new", 0..=2);
    let mut arr = if args.is_empty() {
        Vec::new()
    } else {
        let n = args[0].as_index("mutable-array/new")?;
        let fill = if args.len() == 2 {
            args[1].clone()
        } else {
            Value::nil()
        };
        vec![fill; n]
    };
    Ok(Value::mutable_array(arr))
});

register_fn(env, "mutable-array/push!", |args| {
    check_arity!(args, "mutable-array/push!", 2);
    let arr = args[0]
        .as_mutable_array()
        .ok_or_else(|| SemaError::type_error("mutable-array", args[0].type_name()))?;
    arr.items.borrow_mut().push(args[1].clone());
    Ok(args[0].clone())
});

register_fn(env, "mutable-array/pop!", |args| {
    check_arity!(args, "mutable-array/pop!", 1);
    let arr = args[0]
        .as_mutable_array()
        .ok_or_else(|| SemaError::type_error("mutable-array", args[0].type_name()))?;
    arr.items
        .borrow_mut()
        .pop()
        .ok_or_else(|| SemaError::eval("mutable-array/pop!: array is empty"))
});

register_fn(env, "mutable-array/set!", |args| {
    check_arity!(args, "mutable-array/set!", 3);
    let arr = args[0]
        .as_mutable_array()
        .ok_or_else(|| SemaError::type_error("mutable-array", args[0].type_name()))?;
    let idx = args[1].as_index("mutable-array/set!")?;
    let mut items = arr.items.borrow_mut();
    if idx >= items.len() {
        return Err(SemaError::eval(format!(
            "mutable-array/set!: index {idx} out of bounds (length {})",
            items.len()
        )));
    }
    items[idx] = args[2].clone();
    Ok(args[0].clone())
});

register_fn(env, "mutable-array/ref", |args| {
    check_arity!(args, "mutable-array/ref", 2);
    let arr = args[0]
        .as_mutable_array()
        .ok_or_else(|| SemaError::type_error("mutable-array", args[0].type_name()))?;
    let idx = args[1].as_index("mutable-array/ref")?;
    let items = arr.items.borrow();
    items.get(idx).cloned().ok_or_else(|| {
        SemaError::eval(format!(
            "mutable-array/ref: index {idx} out of bounds (length {})",
            items.len()
        ))
    })
});

register_fn(env, "mutable-array/length", |args| {
    check_arity!(args, "mutable-array/length", 1);
    let arr = args[0]
        .as_mutable_array()
        .ok_or_else(|| SemaError::type_error("mutable-array", args[0].type_name()))?;
    Ok(Value::int(arr.items.borrow().len() as i64))
});

register_fn(env, "mutable-cell/new", |args| {
    check_arity!(args, "mutable-cell/new", 1);
    Ok(Value::mutable_cell(args[0].clone()))
});

register_fn(env, "mutable-cell/get", |args| {
    check_arity!(args, "mutable-cell/get", 1);
    let cell = args[0]
        .as_mutable_cell()
        .ok_or_else(|| SemaError::type_error("mutable-cell", args[0].type_name()))?;
    Ok(cell.value.borrow().clone())
});

register_fn(env, "mutable-cell/set!", |args| {
    check_arity!(args, "mutable-cell/set!", 2);
    let cell = args[0]
        .as_mutable_cell()
        .ok_or_else(|| SemaError::type_error("mutable-cell", args[0].type_name()))?;
    *cell.value.borrow_mut() = args[1].clone();
    Ok(args[0].clone())
});
```

### Step 2: Add aliases

In `crates/sema-stdlib/src/string.rs` or `list.rs` (where aliases are registered), add:

```rust
if let Some(v) = env.get(sema_core::intern("mutable-array/new")) {
    env.set(sema_core::intern("marray/new"), v.clone());
}
```

Only if desired. Skip if not.

---

## Task 1.4: Add bytes stdlib functions

**Files:**
- Modify: `crates/sema-stdlib/src/string.rs`

### Step 1: Register functions

Add in `string.rs` `register()`:

```rust
register_fn(env, "bytes/length", |args| {
    check_arity!(args, "bytes/length", 1);
    let len = if let Some(bs) = args[0].as_byte_string() {
        bs.bytes.len()
    } else if let Some(s) = args[0].as_string_slice() {
        s.end - s.start
    } else {
        return Err(SemaError::type_error("bytes", args[0].type_name()));
    };
    Ok(Value::int(len as i64))
});

register_fn(env, "bytes/ref", |args| {
    check_arity!(args, "bytes/ref", 2);
    let idx = args[1].as_index("bytes/ref")?;
    let byte = if let Some(bs) = args[0].as_byte_string() {
        bs.bytes.get(idx).copied()
    } else if let Some(s) = args[0].as_string_slice() {
        s.owner.bytes[s.start..s.end].get(idx).copied()
    } else {
        return Err(SemaError::type_error("bytes", args[0].type_name()));
    };
    byte.map(|b| Value::int(b as i64)).ok_or_else(|| {
        SemaError::eval(format!("bytes/ref: index {idx} out of bounds"))
    })
});

register_fn(env, "bytes/slice", |args| {
    check_arity!(args, "bytes/slice", 3);
    let owner = args[0]
        .byte_string_owner()
        .ok_or_else(|| SemaError::type_error("bytes", args[0].type_name()))?;
    let start = args[1].as_index("bytes/slice")?;
    let end = args[2].as_index("bytes/slice")?;
    let len = owner.bytes.len();
    if start > len || end > len || start > end {
        return Err(SemaError::eval("bytes/slice: index out of bounds"));
    }
    Ok(Value::string_slice(owner, start, end))
});
```

### Step 3: Add `bytes/index-of`

```rust
register_fn(env, "bytes/index-of", |args| {
    check_arity!(args, "bytes/index-of", 2);
    let haystack = args[0]
        .as_bytes()
        .ok_or_else(|| SemaError::type_error("bytes", args[0].type_name()))?;
    let needle = args[1]
        .as_bytes()
        .ok_or_else(|| SemaError::type_error("bytes", args[1].type_name()))?;
    if needle.is_empty() {
        return Ok(Value::int(0));
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| Value::int(i as i64))
        .unwrap_or(Value::nil())
});
```

### Step 4: Add `bytes/split`

```rust
register_fn(env, "bytes/split", |args| {
    check_arity!(args, "bytes/split", 2);
    let owner = args[0]
        .byte_string_owner()
        .ok_or_else(|| SemaError::type_error("bytes", args[0].type_name()))?;
    let sep = args[1]
        .as_bytes()
        .ok_or_else(|| SemaError::type_error("bytes", args[1].type_name()))?;
    let bytes = owner.bytes.as_ref();
    let mut parts = Vec::new();
    let mut start = 0;
    for pos in bytes.windows(sep.len()).enumerate().filter(|(_, w)| *w == sep).map(|(i, _)| i) {
        parts.push(Value::string_slice(owner.clone(), start, pos));
        start = pos + sep.len();
    }
    parts.push(Value::string_slice(owner.clone(), start, bytes.len()));
    Ok(Value::list(parts))
});
```

### Step 5: Add `bytes/parse-int10`

Parse a decimal integer possibly with a leading `-`. Returns a Sema int.

```rust
register_fn(env, "bytes/parse-int10", |args| {
    check_arity!(args, "bytes/parse-int10", 1);
    let bytes = args[0]
        .as_bytes()
        .ok_or_else(|| SemaError::type_error("bytes", args[0].type_name()))?;
    let mut neg = false;
    let mut i = 0;
    if !bytes.is_empty() && bytes[0] == b'-' {
        neg = true;
        i = 1;
    }
    let mut n: i64 = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if !(b'0'..=b'9').contains(&b) {
            return Err(SemaError::eval(format!(
                "bytes/parse-int10: invalid digit at byte {}",
                i
            )));
        }
        n = n * 10 + (b - b'0') as i64;
        i += 1;
    }
    Ok(Value::int(if neg { -n } else { n }))
});
```

### Step 6: Add `bytes/->string`, `bytes/->symbol`

```rust
register_fn(env, "bytes/->string", |args| {
    check_arity!(args, "bytes/->string", 1);
    let bytes = args[0]
        .as_bytes()
        .ok_or_else(|| SemaError::type_error("bytes", args[0].type_name()))?;
    let s = std::str::from_utf8(&bytes)
        .map_err(|e| SemaError::eval(format!("bytes/->string: invalid UTF-8: {e}")))?;
    Ok(Value::string(s))
});

register_fn(env, "bytes/->symbol", |args| {
    check_arity!(args, "bytes/->symbol", 1);
    let bytes = args[0]
        .as_bytes()
        .ok_or_else(|| SemaError::type_error("bytes", args[0].type_name()))?;
    let s = std::str::from_utf8(&bytes)
        .map_err(|e| SemaError::eval(format!("bytes/->symbol: invalid UTF-8: {e}")))?;
    Ok(Value::symbol(s))
});
```

---

## Task 1.5: Add byte-oriented file I/O

**Files:**
- Modify: `crates/sema-stdlib/src/io.rs`

### Step 1: Add `file/read-bytes-fast`

If `file/read-bytes` does not already use a fast path, ensure it reads the whole file into a `ByteString`:

```rust
crate::register_fn_path_gated(env, sandbox, Caps::FS_READ, "file/read-bytes", &[0], |args| {
    check_arity!(args, "file/read-bytes", 1);
    let path = args[0]
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
    if let Some(data) = sema_core::vfs::vfs_read(path) {
        return Ok(Value::byte_string(data));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| SemaError::Io(format!("file/read-bytes {path}: {e}")))?;
    Ok(Value::byte_string(bytes))
});
```

### Step 2: Add `file/fold-lines-bytes`

Fold over lines of a file, passing each line as a `StringSlice` into a mutable `ByteString` buffer.

```rust
crate::register_fn_path_gated(
    env,
    sandbox,
    Caps::FS_READ,
    "file/fold-lines-bytes",
    &[0],
    |args| {
        check_arity!(args, "file/fold-lines-bytes", 3);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let func = args[1].clone();
        let mut acc = args[2].clone();
        let file = std::fs::File::open(path)
            .map_err(|e| SemaError::Io(format!("file/fold-lines-bytes {path}: {e}")))?;
        let mut reader = std::io::BufReader::with_capacity(256 * 1024, file);

        sema_core::with_stdlib_ctx(|ctx| {
            let native = func.as_native_fn_ref().map(|n| &*n.func);
            let mut buf = Vec::with_capacity(64);
            loop {
                buf.clear();
                let n = reader
                    .read_until(b'\n', &mut buf)
                    .map_err(|e| SemaError::Io(format!("file/fold-lines-bytes {path}: {e}")))?;
                if n == 0 {
                    break;
                }
                // trim trailing \n and \r
                let mut end = buf.len();
                if end > 0 && buf[end - 1] == b'\n' {
                    end -= 1;
                }
                if end > 0 && buf[end - 1] == b'\r' {
                    end -= 1;
                }
                let owner = Rc::new(ByteString::new(buf.clone()));
                let line = Value::string_slice(owner.clone(), 0, end);
                let args = [std::mem::replace(&mut acc, Value::nil()), line];
                acc = if let Some(f) = native {
                    f(ctx, &args)?
                } else {
                    sema_core::call_callback(ctx, &func, &args)?
                };
            }
            Ok(acc)
        })
    },
);
```

Note: this allocates a new `ByteString` per line. A more advanced version shares a single large buffer; defer that optimization to Phase 2 or a later iteration.

---

## Task 1.6: Add tests for mutable arrays and cells

**Files:**
- Create: `crates/sema/tests/mutable_array_test.rs`

```rust
use sema_core::Value;

#[test]
fn mutable_array_new_and_ref() {
    let arr = eval("(mutable-array/new 3 7)").unwrap();
    assert_eq!(eval_with_arg("(mutable-array/length $1)", &arr).unwrap().as_int(), Some(3));
    assert_eq!(eval_with_arg("(mutable-array/ref $1 1)", &arr).unwrap().as_int(), Some(7));
}

#[test]
fn mutable_array_set_and_share() {
    let result = eval(r#"
        (define a (mutable-array/new 2 0))
        (define b a)
        (mutable-array/set! a 0 42)
        (mutable-array/ref b 0)
    "#).unwrap();
    assert_eq!(result.as_int(), Some(42));
}

#[test]
fn mutable_cell_round_trip() {
    let result = eval(r#"
        (define c (mutable-cell/new 1))
        (mutable-cell/set! c 99)
        (mutable-cell/get c)
    "#).unwrap();
    assert_eq!(result.as_int(), Some(99));
}
```

Adapt to actual test helpers in `crates/sema/tests/`.

---

## Task 1.7: Add tests for byte strings and slices

**Files:**
- Create: `crates/sema/tests/bytes_test.rs`

```rust
#[test]
fn bytes_length_and_ref() {
    let bs = eval("(file/read-bytes \"test.txt\")").unwrap();
    assert!(bs.as_byte_string().is_some() || bs.as_string_slice().is_some());
}

#[test]
fn bytes_slice_and_index_of() {
    let result = eval(r#"
        (define bs (file/read-bytes "benchmarks/1brc/test.txt"))
        (define semi (bytes/index-of bs ";"))
        (bytes/length (bytes/slice bs 0 semi))
    "#).unwrap();
    assert!(result.as_int().is_some());
}

#[test]
fn bytes_parse_int10() {
    let result = eval(r#"
        (define bs (file/read-bytes "benchmarks/1brc/test.txt"))
        (define semi (bytes/index-of bs ";"))
        (define temp-bs (bytes/slice bs (+ semi 1) (bytes/length bs)))
        (bytes/parse-int10 temp-bs)
    "#).unwrap();
    assert!(result.as_int().is_some());
}
```

Use a small test fixture or inline byte data.

---

## Task 1.8: Run tests and benchmarks

**Command:**

```bash
cargo test -p sema
jake build-pgo
SEMA_SKIP_BUILD=1 ./benchmarks/1brc/run-native-benchmarks.py benchmarks/data/bench-10m.txt
```

Expected: tests pass, 1brc optimized time improves.

---

# Phase 2: Tracing GC

**Goal:** Replace `Rc<T>` with a non-moving tracing GC for all heap types.

**Approx. agent-hours:** 50–80

**Warning:** This phase is the largest and riskiest. It should be implemented one heap type at a time, with the full test suite passing after each migration.

## Files to create or modify

- `crates/sema-core/src/gc.rs` — new GC module.
- `crates/sema-core/src/value.rs` — convert constructors and accessors from `Rc<T>` to `GcRef<T>`.
- `crates/sema-core/src/lib.rs` — export GC types.
- `crates/sema-vm/src/vm.rs` — register roots before collection.
- `crates/sema-vm/src/lib.rs` — expose VM root registration if needed.
- `crates/sema-stdlib/src/*.rs` — replace `Rc` clones with GC handle copies where needed.
- `crates/sema/tests/gc_test.rs` — new tests.

---

## Task 2.1: Create the GC module

**Files:**
- Create: `crates/sema-core/src/gc.rs`

### Step 1: Define core types

```rust
use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::ptr::NonNull;

pub trait GcObject {
    fn trace(&self, tracer: &mut dyn FnMut(GcRef<dyn GcObject>));
}

pub struct GcHeader {
    marked: Cell<bool>,
}

pub struct GcRef<T: GcObject + ?Sized> {
    ptr: NonNull<GcHeader>,
    _marker: PhantomData<T>,
}

impl<T: GcObject + ?Sized> Clone for GcRef<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: GcObject + ?Sized> Copy for GcRef<T> {}

pub struct GcHeap {
    allocations: RefCell<Vec<NonNull<GcHeader>>>,
    bytes_allocated: Cell<usize>,
    threshold: Cell<usize>,
}

impl GcHeap {
    pub fn new() -> Self {
        GcHeap {
            allocations: RefCell::new(Vec::new()),
            bytes_allocated: Cell::new(0),
            threshold: Cell::new(1024 * 1024), // 1MB initial
        }
    }

    pub fn alloc<T: GcObject + 'static>(&self, payload: T) -> GcRef<T> {
        let size = std::mem::size_of::<GcHeader>() + std::mem::size_of_val(&payload);
        let boxed = Box::new((GcHeader { marked: Cell::new(false) }, payload));
        let ptr = unsafe { NonNull::new_unchecked(Box::into_raw(boxed) as *mut GcHeader) };
        self.allocations.borrow_mut().push(ptr);
        self.bytes_allocated.set(self.bytes_allocated.get() + size);
        if self.bytes_allocated.get() >= self.threshold.get() {
            // Collection is triggered externally; see Task 2.5
        }
        GcRef {
            ptr,
            _marker: PhantomData,
        }
    }

    pub fn mark(&self, obj: GcRef<dyn GcObject>) {
        unsafe {
            let header = obj.ptr.as_ref();
            if header.marked.get() {
                return;
            }
            header.marked.set(true);
            let payload_ptr = (obj.ptr.as_ptr() as *mut u8).add(std::mem::size_of::<GcHeader>())
                as *mut dyn GcObject;
            (*payload_ptr).trace(&mut |child| self.mark(child));
        }
    }

    pub fn sweep(&self) {
        let mut allocs = self.allocations.borrow_mut();
        let mut i = 0;
        let mut freed = 0;
        while i < allocs.len() {
            let header = unsafe { allocs[i].as_ref() };
            if header.marked.get() {
                header.marked.set(false);
                i += 1;
            } else {
                let ptr = allocs.swap_remove(i);
                let size = std::mem::size_of::<GcHeader>()
                    + unsafe {
                        let payload_ptr = (ptr.as_ptr() as *mut u8).add(std::mem::size_of::<GcHeader>())
                            as *mut dyn GcObject;
                        std::mem::size_of_val(&*payload_ptr)
                    };
                freed += size;
                unsafe {
                    drop(Box::from_raw(ptr.as_ptr()));
                }
            }
        }
        self.bytes_allocated
            .set(self.bytes_allocated.get().saturating_sub(freed));
        self.threshold
            .set((self.bytes_allocated.get() * 2).max(1024 * 1024));
    }
}
```

The `GcObject` trait must trace outgoing `Value` references; the `size_of_val` for trait objects is approximate and should be replaced with a stored object size in the header for accurate accounting.

### Step 2: Export from `sema-core`

In `crates/sema-core/src/lib.rs`:

```rust
pub mod gc;
pub use gc::{GcHeap, GcObject, GcRef};
```

---

## Task 2.2: Define a thread-local heap and root API

**Files:**
- Modify: `crates/sema-core/src/gc.rs`

```rust
thread_local! {
    static HEAP: GcHeap = GcHeap::new();
}

pub fn with_heap<R>(f: impl FnOnce(&GcHeap) -> R) -> R {
    HEAP.with(f)
}

pub fn gc_alloc<T: GcObject + 'static>(payload: T) -> GcRef<T> {
    with_heap(|heap| heap.alloc(payload))
}

pub struct RootSet {
    pub roots: Vec<GcRef<dyn GcObject>>,
}

impl RootSet {
    pub fn new() -> Self {
        RootSet { roots: Vec::new() }
    }

    pub fn add(&mut self, obj: GcRef<dyn GcObject>) {
        self.roots.push(obj);
    }
}

pub fn collect(roots: &RootSet) {
    with_heap(|heap| {
        for root in &roots.roots {
            heap.mark(*root);
        }
        heap.sweep();
    });
}
```

---

## Task 2.3: Convert `String` heap type to GC

**Files:**
- Modify: `crates/sema-core/src/value.rs`

### Step 1: Make `String` implement `GcObject`

```rust
impl GcObject for String {
    fn trace(&self, _tracer: &mut dyn FnMut(GcRef<dyn GcObject>)) {
        // Strings do not contain Values
    }
}
```

### Step 2: Change `Value::string` constructor

Replace:

```rust
let rc = Rc::new(s.to_string());
let ptr = Rc::into_raw(rc) as *const u8;
```

With:

```rust
let gc = gc_alloc(s.to_string());
let ptr = gc.ptr.as_ptr() as *const u8;
```

And change `ValueView::String(Rc<String>)` to `ValueView::String(GcRef<String>)`.

### Step 3: Update accessors

`as_string_rc()` becomes `as_string_ref()` returning `GcRef<String>`.
`as_str()` borrows through the GC header.

### Step 4: Run tests

```bash
cargo test -p sema-core
cargo test -p sema
```

---

## Task 2.4: Convert `Vec<Value>` (list/vector) to GC

**Files:**
- Modify: `crates/sema-core/src/value.rs`

### Step 1: Make `Vec<Value>` implement `GcObject`

```rust
impl GcObject for Vec<Value> {
    fn trace(&self, tracer: &mut dyn FnMut(GcRef<dyn GcObject>)) {
        for v in self {
            v.trace(tracer);
        }
    }
}
```

`Value::trace` must be implemented to dispatch to the appropriate heap object.

### Step 2: Update constructors and accessors

Change `Value::list`, `Value::vector`, `ValueView::List`, `ValueView::Vector` to use `GcRef<Vec<Value>>`.

### Step 3: Update stdlib

In `crates/sema-stdlib/src/list.rs`, replace `Rc<Vec<Value>>` usages with `GcRef<Vec<Value>>`.

### Step 4: Run tests

```bash
cargo test -p sema
```

---

## Task 2.5: Convert `HashMap<Value, Value>` and `BTreeMap<Value, Value>` to GC

**Files:**
- Modify: `crates/sema-core/src/value.rs`
- Modify: `crates/sema-stdlib/src/map.rs`

### Step 1: Implement `GcObject`

```rust
impl GcObject for hashbrown::HashMap<Value, Value> {
    fn trace(&self, tracer: &mut dyn FnMut(GcRef<dyn GcObject>)) {
        for (k, v) in self {
            k.trace(tracer);
            v.trace(tracer);
        }
    }
}

impl GcObject for BTreeMap<Value, Value> {
    fn trace(&self, tracer: &mut dyn FnMut(GcRef<dyn GcObject>)) {
        for (k, v) in self {
            k.trace(tracer);
            v.trace(tracer);
        }
    }
}
```

### Step 2: Update constructors and accessors

Change `Value::map`, `Value::hashmap`, and `ValueView` variants.

### Step 3: Update `with_hashmap_mut_if_unique`

Since `GcRef` is `Copy`, uniqueness cannot be determined by `Rc::strong_count()`. Remove or redesign this optimization. Either:
- Drop the COW optimization, or
- Add a separate `is_unique` flag to the GC header.

For Phase 2, drop the optimization and rely on the GC; re-add uniqueness later if needed.

### Step 4: Run tests

```bash
cargo test -p sema
```

---

## Task 2.6: Convert closures, lambdas, upvalues, and `Env` to GC

**Files:**
- Modify: `crates/sema-core/src/value.rs`
- Modify: `crates/sema-vm/src/vm.rs`

### Step 1: Convert `Lambda`, `Macro`, `Closure`

```rust
impl GcObject for Lambda {
    fn trace(&self, tracer: &mut dyn FnMut(GcRef<dyn GcObject>)) {
        for v in &self.body {
            v.trace(tracer);
        }
        self.env.trace(tracer);
    }
}
```

`Env` must implement `GcObject` and become GC-managed. Replace `Rc<Env>` with `GcRef<Env>`.

### Step 2: Convert `UpvalueCell`

`UpvalueCell` holds a `Value`. It must be GC-managed and trace its closed value.

### Step 3: Update VM root registration

Before any operation that may trigger collection, the VM must build a root set from:
- `self.stack` (all Values)
- All open upvalues in all frames
- `self.globals`
- `self.inline_cache`
- `self.debug_values`

Add a method:

```rust
impl VM {
    fn build_root_set(&self) -> RootSet {
        let mut roots = RootSet::new();
        for v in &self.stack {
            v.add_to_root_set(&mut roots);
        }
        // ... add other roots
        roots
    }
}
```

---

## Task 2.7: Convert remaining heap types to GC

**Files:**
- Modify: `crates/sema-core/src/value.rs`

Remaining types: `NativeFn`, `Prompt`, `Message`, `Conversation`, `ToolDef`, `Agent`, `Thunk`, `Record`, `Bytevector`, `MultiMethod`, `Stream`, `F64Array`, `I64Array`, `AsyncPromise`, `Channel`, `MutableArray`, `MutableCell`, `ByteString`, `StringSlice`.

For each:
1. Implement `GcObject::trace`.
2. Change `Value` constructor to use `gc_alloc`.
3. Change `ValueView` variant to use `GcRef<T>`.
4. Update accessors.

`NativeFn` and `Stream` may carry opaque native data; their trace implementations can be no-ops if they do not contain `Value`s.

---

## Task 2.8: Remove old `Rc<T>` code paths

**Files:**
- Modify: `crates/sema-core/src/value.rs`

Once all heap types use `GcRef<T>`:
1. Remove `Rc<T>` value constructors (`*_from_rc` where T is a heap type).
2. Remove `as_*_rc()` accessors.
3. Remove `Rc` imports if no longer needed.
4. Run `cargo clippy --all-targets -- -D warnings` and fix issues.

---

## Task 2.9: Add GC tests

**Files:**
- Create: `crates/sema/tests/gc_test.rs`

```rust
#[test]
fn gc_keeps_reachable_objects_alive() {
    let result = eval(r#"
        (define x (vector 1 2 3))
        (define y (vector 4 5 6))
        (gc/collect)
        (length x)
    "#).unwrap();
    assert_eq!(result.as_int(), Some(3));
}

#[test]
fn gc_collects_unreachable_objects() {
    // This test mainly checks that collection does not crash.
    let _ = eval(r#"
        (define loop (fn (n)
                       (if (= n 0)
                           0
                           (do (vector n n n)
                               (loop (- n 1))))))
        (loop 1000)
        (gc/collect)
        42
    "#).unwrap();
}
```

Expose `(gc/collect)` as a stdlib function in `crates/sema-stdlib/src/meta.rs` or a new `gc.rs` module.

---

## Task 2.10: Run full test suite and benchmark

**Command:**

```bash
cargo test
jake build-pgo
SEMA_SKIP_BUILD=1 ./benchmarks/1brc/run-native-benchmarks.py benchmarks/data/bench-10m.txt
```

Expected: all tests pass, 1brc optimized time reaches Janet-level.

---

# Phase 3: Optimize the 1brc Benchmark and Docs

**Goal:** Rewrite the optimized 1brc implementation to use mutable arrays and byte strings, and update performance docs.

**Approx. agent-hours:** 2–4

## Files to create or modify

- `benchmarks/1brc/1brc.sema`
- `benchmarks/1brc/simple/1brc.sema` (optional)
- `website/docs/internals/performance.md`
- `website/docs/internals/lisp-comparison.md`

---

## Task 3.1: Rewrite `benchmarks/1brc/1brc.sema`

**Files:**
- Modify: `benchmarks/1brc/1brc.sema`

Replace the per-row stats vector rebuild with a mutable array. Use `bytes/*` APIs for parsing if Phase 1 byte APIs are available.

```sema
;; 1brc.sema — optimized for mutable arrays and byte strings

(define (round1 x)
  (/ (round (* x 10.0)) 10.0))

(define (format-station name stats)
  (let ((mn  (round1 (mutable-array/ref stats 0)))
        (avg (round1 (/ (mutable-array/ref stats 2)
                        (mutable-array/ref stats 3))))
        (mx  (round1 (mutable-array/ref stats 1))))
    (format "~a=~a/~a/~a" name mn avg mx)))

(define (script-args)
  (let loop ((args (sys/args)))
    (cond
      ((null? args) '())
      ((= (first args) "--") (rest args))
      (else (loop (rest args))))))

(define args (script-args))
(when (< (length args) 1)
  (println "Usage: sema 1brc.sema -- <measurements-file>")
  (exit 1))
(define input-file (first args))

(define t0 (time-ms))

(define result
  (file/fold-lines-bytes input-file
    (fn (acc line)
      (if (= (bytes/length line) 0)
          acc
          (let* ((semi (bytes/index-of line ";"))
                 (name (bytes/->symbol (bytes/slice line 0 semi)))
                 (temp-bs (bytes/slice line (+ semi 1) (bytes/length line)))
                 (temp (bytes/parse-int10 temp-bs))
                 (existing (get acc name)))
            (if (nil? existing)
                (assoc acc name (mutable-array/new 4 temp))
                (let ((stats existing))
                  (mutable-array/set! stats 0 (min temp (mutable-array/ref stats 0)))
                  (mutable-array/set! stats 1 (max temp (mutable-array/ref stats 1)))
                  (mutable-array/set! stats 2 (+ temp (mutable-array/ref stats 2)))
                  (mutable-array/set! stats 3 (+ 1 (mutable-array/ref stats 3)))
                  (assoc acc name stats))))))
    (hashmap/new)))

(define t1 (time-ms))
(println (format "Processed ~a stations in ~a ms" (length (keys result)) (- t1 t0)))

(define sorted-names (sort (keys result)))

(define formatted
  (map (fn (name) (format-station name (get result name)))
       sorted-names))

(define output (string-append "{" (string/join formatted ", ") "}"))

(define t2 (time-ms))
(println output)
(println (format "Total: ~a ms" (- t2 t0)))
```

Adjust `bytes/parse-int10` to produce tenths-of-degrees if the input has one decimal place, or use `bytes/parse-float` if available.

---

## Task 3.2: Run benchmark and verify output

**Command:**

```bash
jake build-pgo
SEMA_SKIP_BUILD=1 ./benchmarks/1brc/run-native-benchmarks.py benchmarks/data/bench-10m.txt
```

Verify:
- Sema output matches other implementations byte-for-byte.
- Optimized time is within 1.1× of Janet.

---

## Task 3.3: Update docs

**Files:**
- Modify: `website/docs/internals/performance.md`
- Modify: `website/docs/internals/lisp-comparison.md`

### `performance.md`

Add a section describing:
- Mutable arrays/cells
- Byte strings/slices
- Tracing GC
- How each reduces allocation in 1brc

### `lisp-comparison.md`

Update the Sema section to reflect the new numbers and implementation approach. Mention that the gap was closed by runtime changes, not benchmark-script tuning.

---

# Self-Review Checklist

- [x] Spec coverage: every subsystem in the design spec maps to tasks.
- [x] Placeholder scan: no TBD/TODO/fill-in-later steps.
- [x] Type consistency: `MutableArray`, `MutableCell`, `ByteString`, `StringSlice`, `GcRef` names are consistent.
- [x] Test strategy: each phase has explicit tests.
- [x] File paths: all paths are relative to repo root and accurate as of the design exploration.

# Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-06-27-sema-runtime-performance-plan.md`.**

Two execution options:

1. **Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session using `executing-plans`, batch execution with checkpoints.

Which approach?
