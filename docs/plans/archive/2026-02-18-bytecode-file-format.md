# Bytecode File Format (.semac) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `.semac` bytecode file serialization/deserialization to Sema, plus `compile`, `run`, and `disasm` CLI subcommands.

**Architecture:** New `serialize.rs` module in `sema-vm` handles serialization/deserialization of `CompileResult` (Chunk + Vec\<Function\>) to/from the `.semac` binary format. The format uses a 24-byte header with magic number, followed by sections (string table, function table, main chunk, optional debug sections). Spur values are remapped via a string table. The CLI gets three new subcommands.

**Tech Stack:** Rust, sema-vm crate, sema (CLI binary), clap subcommands

**Spec:** `website/docs/internals/bytecode-format.md` is the single source of truth for the binary format. Any deviation from the spec is a bug.

---

## Task 1: Serialization — String Table Builder

**Files:**
- Create: `crates/sema-vm/src/serialize.rs`
- Modify: `crates/sema-vm/src/lib.rs` (add `pub mod serialize;`)

**Step 1: Write the failing test**

```rust
// In crates/sema-vm/src/serialize.rs
#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::intern;

    #[test]
    fn test_string_table_builder() {
        let mut builder = StringTableBuilder::new();
        // Index 0 is always ""
        assert_eq!(builder.intern_str(""), 0);
        let idx_hello = builder.intern_str("hello");
        let idx_world = builder.intern_str("world");
        let idx_hello2 = builder.intern_str("hello");
        assert_eq!(idx_hello, idx_hello2); // deduplication
        assert_ne!(idx_hello, idx_world);

        let table = builder.finish();
        assert_eq!(table.len(), 3); // "", "hello", "world"
        assert_eq!(table[0], "");
        assert_eq!(table[idx_hello as usize], "hello");
        assert_eq!(table[idx_world as usize], "world");
    }

    #[test]
    fn test_string_table_spur_interning() {
        let mut builder = StringTableBuilder::new();
        let spur = intern("my-var");
        let idx = builder.intern_spur(spur);
        assert!(idx > 0);
        let idx2 = builder.intern_spur(spur);
        assert_eq!(idx, idx2);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p sema-vm -- test_string_table`
Expected: FAIL — module/structs don't exist yet

**Step 3: Write minimal implementation**

```rust
// crates/sema-vm/src/serialize.rs
use hashbrown::HashMap;
use sema_core::{resolve, Spur};

/// Builds a deduplicated string table for serialization.
pub struct StringTableBuilder {
    strings: Vec<String>,
    index: HashMap<String, u32>,
}

impl StringTableBuilder {
    pub fn new() -> Self {
        let mut b = StringTableBuilder {
            strings: Vec::new(),
            index: HashMap::new(),
        };
        b.intern_str(""); // index 0 = empty string
        b
    }

    pub fn intern_str(&mut self, s: &str) -> u32 {
        if let Some(&idx) = self.index.get(s) {
            return idx;
        }
        let idx = self.strings.len() as u32;
        self.strings.push(s.to_string());
        self.index.insert(s.to_string(), idx);
        idx
    }

    pub fn intern_spur(&mut self, spur: Spur) -> u32 {
        let s = resolve(spur);
        self.intern_str(&s)
    }

    pub fn finish(self) -> Vec<String> {
        self.strings
    }
}
```

Also add `pub mod serialize;` to `crates/sema-vm/src/lib.rs`.

**Step 4: Run test to verify it passes**

Run: `cargo test -p sema-vm -- test_string_table`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/sema-vm/src/serialize.rs crates/sema-vm/src/lib.rs
git commit -m "feat(vm): add StringTableBuilder for bytecode serialization"
```

---

## Task 2: Value Serialization

**Files:**
- Modify: `crates/sema-vm/src/serialize.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_serialize_value_roundtrip_primitives() {
    let mut buf = Vec::new();
    let mut stb = StringTableBuilder::new();

    // Nil
    serialize_value(&Value::nil(), &mut buf, &mut stb).unwrap();
    // Bool
    serialize_value(&Value::bool(true), &mut buf, &mut stb).unwrap();
    serialize_value(&Value::bool(false), &mut buf, &mut stb).unwrap();
    // Int
    serialize_value(&Value::int(42), &mut buf, &mut stb).unwrap();
    // Float
    serialize_value(&Value::float(3.14), &mut buf, &mut stb).unwrap();
    // String
    serialize_value(&Value::string("hello"), &mut buf, &mut stb).unwrap();
    // Symbol
    serialize_value(&Value::symbol("foo"), &mut buf, &mut stb).unwrap();
    // Keyword
    serialize_value(&Value::keyword("bar"), &mut buf, &mut stb).unwrap();

    // Deserialize
    let table = stb.finish();
    let remap = build_remap_table(&table);
    let mut cursor = 0;
    assert_eq!(deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(), Value::nil());
    assert_eq!(deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(), Value::bool(true));
    assert_eq!(deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(), Value::bool(false));
    assert_eq!(deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(), Value::int(42));
    // Float comparison
    let f = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
    assert_eq!(f.as_float(), Some(3.14));
    let s = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
    assert_eq!(s.as_str().unwrap(), "hello");
    // Symbol and Keyword
    let sym = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
    assert!(sym.as_symbol().is_some());
    let kw = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
    assert!(kw.as_keyword().is_some());
}

#[test]
fn test_serialize_value_roundtrip_collections() {
    let mut buf = Vec::new();
    let mut stb = StringTableBuilder::new();

    let list = Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]);
    serialize_value(&list, &mut buf, &mut stb).unwrap();

    let vec = Value::vector(vec![Value::string("a"), Value::string("b")]);
    serialize_value(&vec, &mut buf, &mut stb).unwrap();

    let table = stb.finish();
    let remap = build_remap_table(&table);
    let mut cursor = 0;

    let list2 = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
    assert_eq!(list2, list);

    let vec2 = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
    assert_eq!(vec2, vec);
}
```

**Step 2: Run to verify fail**

Run: `cargo test -p sema-vm -- test_serialize_value`
Expected: FAIL — functions don't exist

**Step 3: Implement serialize_value / deserialize_value / build_remap_table**

Implement `serialize_value(val: &Value, buf: &mut Vec<u8>, stb: &mut StringTableBuilder) -> Result<(), SemaError>` and `deserialize_value(buf: &[u8], cursor: &mut usize, table: &[String], remap: &[Spur]) -> Result<Value, SemaError>` per the spec's tag table in `website/docs/internals/bytecode-format.md`.

Also implement `build_remap_table(table: &[String]) -> Vec<Spur>` which interns each string to get a process-local Spur.

**Step 4: Run test to verify pass**

Run: `cargo test -p sema-vm -- test_serialize_value`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/sema-vm/src/serialize.rs
git commit -m "feat(vm): add Value serialization/deserialization for bytecode format"
```

---

## Task 3: Chunk Serialization

**Files:**
- Modify: `crates/sema-vm/src/serialize.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_chunk_roundtrip() {
    use crate::emit::Emitter;
    use crate::opcodes::Op;

    let mut e = Emitter::new();
    e.emit_const(Value::int(42));
    e.emit_const(Value::string("hello"));
    e.emit_op(Op::Add);
    e.emit_op(Op::Return);
    let mut chunk = e.into_chunk();
    chunk.n_locals = 2;
    chunk.max_stack = 4;

    let mut buf = Vec::new();
    let mut stb = StringTableBuilder::new();
    serialize_chunk(&chunk, &mut buf, &mut stb).unwrap();

    let table = stb.finish();
    let remap = build_remap_table(&table);
    let mut cursor = 0;
    let chunk2 = deserialize_chunk(&buf, &mut cursor, &table, &remap).unwrap();

    assert_eq!(chunk2.code, chunk.code);
    assert_eq!(chunk2.consts.len(), chunk.consts.len());
    assert_eq!(chunk2.n_locals, 2);
    assert_eq!(chunk2.max_stack, 4);
}
```

**Step 2–5:** Implement `serialize_chunk` / `deserialize_chunk` per spec, test, commit.

```bash
git commit -m "feat(vm): add Chunk serialization/deserialization"
```

---

## Task 4: Function Table Serialization

**Files:**
- Modify: `crates/sema-vm/src/serialize.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_function_roundtrip() {
    use crate::chunk::{Function, UpvalueDesc};
    use crate::emit::Emitter;
    use crate::opcodes::Op;
    use sema_core::intern;

    let mut e = Emitter::new();
    e.emit_op(Op::LoadLocal0);
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();

    let func = Function {
        name: Some(intern("my-func")),
        chunk,
        upvalue_descs: vec![UpvalueDesc::ParentLocal(0), UpvalueDesc::ParentUpvalue(1)],
        arity: 2,
        has_rest: true,
        local_names: vec![(0, intern("x")), (1, intern("y"))],
    };

    let mut buf = Vec::new();
    let mut stb = StringTableBuilder::new();
    serialize_function(&func, &mut buf, &mut stb).unwrap();

    let table = stb.finish();
    let remap = build_remap_table(&table);
    let mut cursor = 0;
    let func2 = deserialize_function(&buf, &mut cursor, &table, &remap).unwrap();

    assert_eq!(func2.arity, 2);
    assert!(func2.has_rest);
    assert_eq!(func2.upvalue_descs.len(), 2);
    assert_eq!(func2.local_names.len(), 2);
}
```

**Step 2–5:** Implement, test, commit.

```bash
git commit -m "feat(vm): add Function serialization/deserialization"
```

---

## Task 5: Spur Remapping in Bytecode

**Files:**
- Modify: `crates/sema-vm/src/serialize.rs`

This is the critical piece: walking bytecode to rewrite `LoadGlobal`/`StoreGlobal`/`DefineGlobal` operands.

**Step 1: Write the failing test**

```rust
#[test]
fn test_spur_remapping_in_bytecode() {
    use crate::emit::Emitter;
    use crate::opcodes::Op;
    use sema_core::intern;

    let spur = intern("my-global");
    let mut e = Emitter::new();
    e.emit_op(Op::LoadGlobal);
    e.emit_u32(spur_to_u32(spur));
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();

    let mut buf = Vec::new();
    let mut stb = StringTableBuilder::new();
    serialize_chunk(&chunk, &mut buf, &mut stb).unwrap();

    // Deserialize in a "fresh process" (remap will produce different Spur values)
    let table = stb.finish();
    let remap = build_remap_table(&table);
    let mut cursor = 0;
    let chunk2 = deserialize_chunk(&buf, &mut cursor, &table, &remap).unwrap();

    // The spur in the deserialized bytecode should resolve to "my-global"
    let spur2_bits = u32::from_le_bytes([chunk2.code[1], chunk2.code[2], chunk2.code[3], chunk2.code[4]]);
    let spur2: Spur = unsafe { std::mem::transmute(spur2_bits) };
    assert_eq!(sema_core::resolve(spur2), "my-global");
}
```

**Step 2–5:** The serializer must:
1. Walk bytecode to find global opcodes
2. Extract Spur u32, resolve to string, intern into string table
3. Replace the u32 with the string table index
4. On deserialization, reverse: replace string table index with the new process-local Spur u32

Commit:
```bash
git commit -m "feat(vm): add Spur remapping for bytecode global opcodes"
```

---

## Task 6: Full File Serialization (Header + Sections)

**Files:**
- Modify: `crates/sema-vm/src/serialize.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_full_file_roundtrip() {
    use crate::compiler::{compile, CompileResult};
    use crate::emit::Emitter;
    use crate::opcodes::Op;

    // Create a simple CompileResult
    let mut e = Emitter::new();
    e.emit_const(Value::int(42));
    e.emit_op(Op::Return);
    let chunk = e.into_chunk();
    let result = CompileResult {
        chunk,
        functions: vec![],
    };

    let bytes = serialize_to_bytes(&result, 0).unwrap();
    assert_eq!(&bytes[0..4], b"\x00SEM");

    let result2 = deserialize_from_bytes(&bytes).unwrap();
    assert_eq!(result2.chunk.consts.len(), 1);
    assert_eq!(result2.functions.len(), 0);
}

#[test]
fn test_magic_detection() {
    assert!(is_bytecode_file(b"\x00SEM\x01\x00"));
    assert!(!is_bytecode_file(b"(define x 1)"));
    assert!(!is_bytecode_file(b""));
    assert!(!is_bytecode_file(b"\x00SE")); // too short
}
```

**Step 2–5:** Implement public API:
- `serialize_to_bytes(result: &CompileResult, source_hash: u32) -> Result<Vec<u8>, SemaError>`
- `deserialize_from_bytes(bytes: &[u8]) -> Result<CompileResult, SemaError>`
- `is_bytecode_file(bytes: &[u8]) -> bool`

Commit:
```bash
git commit -m "feat(vm): add full .semac file serialization/deserialization"
```

---

## Task 7: CLI — `sema compile` Subcommand

**Files:**
- Modify: `crates/sema/src/main.rs`

**Step 1: Write integration test**

```rust
// crates/sema/tests/integration_test.rs
#[test]
fn test_compile_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("test.sema");
    std::fs::write(&src, "(define x 42)").unwrap();

    let output = Command::new(cargo_bin())
        .args(["compile", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let semac = dir.path().join("test.semac");
    assert!(semac.exists());

    // Verify magic number
    let bytes = std::fs::read(&semac).unwrap();
    assert_eq!(&bytes[0..4], b"\x00SEM");
}
```

**Step 2–5:** Add `Commands::Compile { file, output, strip }` variant to the CLI enum, implement the handler that:
1. Reads the source file
2. Parses with `sema_reader::parse`
3. Compiles with the VM pipeline (lower → resolve → compile)
4. Serializes to bytes
5. Writes to output file (default: replace `.sema` extension with `.semac`)

Commit:
```bash
git commit -m "feat(cli): add 'sema compile' subcommand for bytecode serialization"
```

---

## Task 8: CLI — Auto-detect `.semac` and `sema run`

**Files:**
- Modify: `crates/sema/src/main.rs`

**Step 1: Write integration test**

```rust
#[test]
fn test_run_semac_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("hello.sema");
    std::fs::write(&src, "(println \"hello from bytecode\")").unwrap();

    // Compile
    let output = Command::new(cargo_bin())
        .args(["compile", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());

    // Run the .semac file
    let semac = dir.path().join("hello.semac");
    let output = Command::new(cargo_bin())
        .args(["--vm", semac.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello from bytecode"));
}

#[test]
fn test_auto_detect_bytecode_vs_source() {
    // Running a .semac file without --vm should auto-detect and use the VM
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("auto.sema");
    std::fs::write(&src, "(println \"auto-detect\")").unwrap();

    let _ = Command::new(cargo_bin())
        .args(["compile", src.to_str().unwrap()])
        .output()
        .unwrap();

    let semac = dir.path().join("auto.semac");
    let output = Command::new(cargo_bin())
        .arg(semac.to_str().unwrap())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("auto-detect"));
}
```

**Step 2–5:** Modify the file execution path in `main()`:
1. Read the first 4 bytes of the input file
2. If magic matches `\x00SEM`, deserialize and run via VM
3. Otherwise, proceed with source parsing as before

Commit:
```bash
git commit -m "feat(cli): auto-detect .semac bytecode files and run via VM"
```

---

## Task 9: CLI — `sema disasm` for `.semac` Files

**Files:**
- Modify: `crates/sema/src/main.rs`

**Step 1: Write integration test**

```rust
#[test]
fn test_disasm_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("dis.sema");
    std::fs::write(&src, "(+ 1 2)").unwrap();

    let _ = Command::new(cargo_bin())
        .args(["compile", src.to_str().unwrap()])
        .output()
        .unwrap();

    let semac = dir.path().join("dis.semac");
    let output = Command::new(cargo_bin())
        .args(["disasm", semac.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("CONST") || stdout.contains("RETURN"));
}
```

**Step 2–5:** Add `Commands::Disasm { file, json }` variant. Load the `.semac`, deserialize, run through existing `disasm::disassemble()`.

Commit:
```bash
git commit -m "feat(cli): add 'sema disasm' subcommand for bytecode disassembly"
```

---

## Task 10: Update CLI Docs

**Files:**
- Modify: `website/docs/cli.md`
- Modify: `website/docs/internals/bytecode-vm.md` (add link to format spec)
- Modify: `website/docs/internals/bytecode-format.md` (remove "design phase" banner if fully implemented)

**Step 1:** Add the new subcommands to the CLI Reference page:

```markdown
### `sema compile`

Compile source to bytecode.

\`\`\`
sema compile [OPTIONS] <FILE>
\`\`\`

| Flag | Description |
|------|-------------|
| `-o, --output <FILE>` | Output file path (default: input with `.semac` extension) |
| `--strip` | Strip debug information |
| `--check` | Validate a `.semac` file without executing |

### `sema disasm`

Disassemble a compiled bytecode file.

\`\`\`
sema disasm [OPTIONS] <FILE>
\`\`\`

| Flag | Description |
|------|-------------|
| `--json` | Output as JSON |
```

**Step 2:** Add cross-reference from `bytecode-vm.md` to `bytecode-format.md`.

**Step 3: Commit**

```bash
git add website/
git commit -m "docs: add compile/disasm/run commands to CLI reference"
```

---

## Task 11: End-to-End Integration Test

**Files:**
- Modify: `crates/sema/tests/integration_test.rs`

**Step 1: Write the integration test**

```rust
#[test]
fn test_bytecode_file_end_to_end() {
    // Compile a non-trivial program with closures, globals, strings
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("e2e.sema");
    std::fs::write(&src, r#"
        (define (make-adder n)
          (lambda (x) (+ n x)))
        (define add5 (make-adder 5))
        (println (add5 10))
    "#).unwrap();

    // Compile
    let output = Command::new(cargo_bin())
        .args(["compile", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success(), "compile failed: {}", String::from_utf8_lossy(&output.stderr));

    // Run from bytecode
    let semac = dir.path().join("e2e.semac");
    let output = Command::new(cargo_bin())
        .arg(semac.to_str().unwrap())
        .output()
        .unwrap();
    assert!(output.status.success(), "run failed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("15"));
}
```

**Step 2: Run and verify pass**

Run: `cargo test -p sema --test integration_test -- test_bytecode_file_end_to_end`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/sema/tests/integration_test.rs
git commit -m "test: add end-to-end bytecode file integration test"
```

---

## Task Summary

| # | Task | Files | Status |
|---|------|-------|--------|
| 1 | String Table Builder | `serialize.rs`, `lib.rs` | ✅ Done |
| 2 | Value Serialization | `serialize.rs` | ✅ Done |
| 3 | Chunk Serialization | `serialize.rs` | ✅ Done |
| 4 | Function Table Serialization | `serialize.rs` | ✅ Done |
| 5 | Spur Remapping in Bytecode | `serialize.rs` | ✅ Done |
| 6 | Full File Serialization | `serialize.rs` | ✅ Done |
| 7 | CLI `sema compile` | `main.rs` | ✅ Done (incl. `--check`) |
| 8 | CLI auto-detect + `sema run` | `main.rs` | ✅ Done |
| 9 | CLI `sema disasm` | `main.rs` | ✅ Done (incl. `--json`) |
| 10 | Docs update | `cli.md`, `bytecode-vm.md`, `bytecode-format.md` | ✅ Done |
| 11 | E2E integration test | `integration_test.rs` | ✅ Done |

### Post-implementation hardening (code review)
- ✅ Replaced unsafe Spur transmute with safe NonZeroU32 APIs
- ✅ Section boundary enforcement during deserialization
- ✅ Recursion depth limit for nested value deserialization
- ✅ DoS allocation limits on attacker-controlled sizes
- ✅ MakeClosure spec updated to match 4-byte encoding
- ✅ Reserved header field validation
- ✅ String table index 0 validation
- ✅ Section payload consumption checks
- ✅ Post-deserialization operand bounds validation
- ✅ Macro expansion in compile subcommand
- ✅ Capacity check fixes (off-by-one, stale remaining)
- ✅ Local/upvalue slot bounds validation
- ✅ Smoke test for all 66 examples (65/66 pass, 1 timeout skip)

**Total: ~2.5 hours**

## Dependencies Between Tasks

```
Task 1 (String Table)
  └→ Task 2 (Value Serialization)
       └→ Task 3 (Chunk Serialization)
            ├→ Task 4 (Function Serialization)
            └→ Task 5 (Spur Remapping)
                 └→ Task 6 (Full File Serialization)
                      ├→ Task 7 (CLI compile)
                      ├→ Task 8 (CLI auto-detect/run)
                      └→ Task 9 (CLI disasm)
                           └→ Task 10 (Docs)
                                └→ Task 11 (E2E test)
```

Tasks 7, 8, 9 can be parallelized after Task 6 completes.
