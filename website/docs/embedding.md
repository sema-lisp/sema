---
outline: [2, 3]
---

# Embedding Sema

## Overview

Sema can be embedded as a Rust library, letting you use it as a scripting or configuration language inside your own applications. The crate exposes a builder API for creating interpreters, registering native functions, and evaluating Sema code from Rust.

## Quick Start

Add Sema to your project:

```toml
[dependencies]
sema-lang = "1.11"
```

Or use the latest unreleased version from git:

```toml
[dependencies]
sema-lang = { git = "https://github.com/sema-lisp/sema" }
```

Evaluate an expression in three lines:

```rust
use sema::{Interpreter, Value};

fn main() -> sema::Result<()> {
    let interp = Interpreter::new();

    let result = interp.eval_str("(+ 1 2 3)")?;
    println!("{result}"); // 6
    Ok(())
}
```

## The Builder

`Interpreter::builder()` returns an `InterpreterBuilder` with these options:

| Method              | Default       | Description                          |
| ------------------- | ------------- | ------------------------------------ |
| `.with_stdlib(b)`   | `true`        | Register the full standard library   |
| `.with_llm(b)`      | `true`        | Enable LLM functions and auto-config |
| `.without_stdlib()` | —             | Shorthand for `.with_stdlib(false)`  |
| `.without_llm()`    | —             | Shorthand for `.with_llm(false)`     |
| `.with_sandbox(sb)` | `allow_all()` | Set sandbox to restrict capabilities |
| `.with_allowed_paths(p)` | unrestricted | Restrict file ops to specific directories |

### Default Interpreter

`Interpreter::new()` gives you everything — stdlib and LLM builtins enabled:

```rust
let interp = Interpreter::new();
interp.eval_str("(+ 1 2)")?; // => 3
```

### Minimal Interpreter

No stdlib, no LLM — only special forms and core evaluation:

```rust
let interp = Interpreter::builder()
    .without_stdlib()
    .without_llm()
    .build();
```

### Stdlib Only (No LLM)

Disable LLM builtins for faster startup when you don't need them:

```rust
let interp = Interpreter::builder()
    .without_llm()
    .build();
```

### Sandboxed Interpreter

Restrict specific capabilities while keeping the full stdlib available:

```rust
use sema::{Interpreter, Sandbox, Caps};

// Allow computation but deny shell and network access
let interp = Interpreter::builder()
    .with_sandbox(Sandbox::deny(
        Caps::SHELL.union(Caps::NETWORK)
    ))
    .build();

interp.eval_str("(+ 1 2)")?;             // => 3 (always works)
interp.eval_str(r#"(shell "ls")"#)?;      // => PermissionDenied error
interp.eval_str(r#"(http/get "...")"#)?;  // => PermissionDenied error
```

### Path-Restricted Interpreter

Confine file operations to specific directories (e.g., for LLM agents):

```rust
use std::path::PathBuf;
use sema::Interpreter;

let interp = Interpreter::builder()
    .with_allowed_paths(vec![
        PathBuf::from("./workspace"),
        PathBuf::from("/tmp"),
    ])
    .build();

interp.eval_str(r#"(file/write "./workspace/out.txt" "ok")"#)?;  // works
interp.eval_str(r#"(file/read "/etc/passwd")"#)?;                 // => PermissionDenied
```

### Multiple Interpreters

Each `Interpreter` has its own `EvalContext` with fully isolated state — module cache, call stack, span table, and depth counters are not shared:

```rust
let interp_a = Interpreter::new();
let interp_b = Interpreter::new();

interp_a.eval_str("(define x 1)")?;
interp_b.eval_str("(define x 2)")?;

// Each interpreter has its own bindings
assert_eq!(interp_a.eval_str("x")?, Value::Int(1));
assert_eq!(interp_b.eval_str("x")?, Value::Int(2));
```

## Registering Native Functions

Use `register_fn` to expose Rust functions to Sema scripts. The closure receives `&[Value]` and returns `Result<Value, SemaError>`.

### Basic Example

```rust
interp.register_fn("add1", |args| {
    let n = args[0]
        .as_int()
        .ok_or_else(|| sema::SemaError::type_error("int", args[0].type_name()))?;
    Ok(Value::Int(n + 1))
});
```

```sema
(add1 41) ; => 42
```

### Capturing State

Use `Rc<RefCell<T>>` to share mutable state between Rust and Sema:

```rust
use std::rc::Rc;
use std::cell::RefCell;

let counter = Rc::new(RefCell::new(0_i64));
let c = counter.clone();
interp.register_fn("inc!", move |_| {
    *c.borrow_mut() += 1;
    Ok(Value::Int(*c.borrow()))
});
```

```sema
(inc!) ; => 1
(inc!) ; => 2
(inc!) ; => 3
```

## Real-World Example: Data Pipeline

A Rust CLI tool that uses Sema as a scripting language for user-defined data transformations. The host app provides utility functions and loads a user-written `.sema` script that defines the transform logic.

### Rust Host

```rust
use sema::{Interpreter, Value, SemaError};
use std::rc::Rc;
use std::collections::BTreeMap;

fn main() -> sema::Result<()> {
    let interp = Interpreter::builder()
        .without_llm()
        .build();

    // Provide a logging function
    interp.register_fn("log", |args| {
        for a in args {
            eprintln!("[script] {a}");
        }
        Ok(Value::Nil)
    });

    // Load user transform script
    let script = std::fs::read_to_string("transform.sema")
        .map_err(|e| SemaError::eval(format!("failed to read script: {e}")))?;
    interp.eval_str(&script)?;

    // Process records through the user's transform function
    let records = vec![
        make_record("Alice", 34, "engineering"),
        make_record("Bob", 28, "marketing"),
        make_record("Carol", 45, "engineering"),
    ];

    for record in records {
        interp.env().set_str("__record", record);
        let result = interp.eval_str("(transform __record)")?;
        println!("{result}");
    }

    Ok(())
}

fn make_record(name: &str, age: i64, dept: &str) -> Value {
    let mut map = BTreeMap::new();
    map.insert(
        Value::Keyword(sema::intern("name")),
        Value::String(Rc::new(name.to_string())),
    );
    map.insert(
        Value::Keyword(sema::intern("age")),
        Value::Int(age),
    );
    map.insert(
        Value::Keyword(sema::intern("dept")),
        Value::String(Rc::new(dept.to_string())),
    );
    Value::Map(Rc::new(map))
}
```

### User Script (`transform.sema`)

```sema
(define (transform record)
  (log (format "Processing: ~a" (:name record)))
  (if (> (:age record) 30)
      (assoc record :senior #t)
      record))
```

### Output

```
[script] Processing: Alice
{:age 34 :dept "engineering" :name "Alice" :senior #t}
[script] Processing: Bob
{:age 28 :dept "marketing" :name "Bob"}
[script] Processing: Carol
{:age 45 :dept "engineering" :name "Carol" :senior #t}
```

## Threading Model

Sema is **single-threaded by design**. It uses `Rc` (not `Arc`) for reference counting and a thread-local string interner for keywords and symbols.

- Multiple `Interpreter` instances can coexist on the same thread with **fully isolated evaluator state** — each has its own module cache, call stack, span table, and depth counters.
- Do **not** send `Value` instances across thread boundaries — they are not `Send` or `Sync`.
- The string interner is per-thread, so interned keys from one thread are not valid in another.
- LLM state (provider registry, usage tracking, budgets) is per-thread and shared across all interpreters on the same thread.

## Security Considerations

By default, Sema scripts have full access to the filesystem, shell, network, and environment. For untrusted code, you have two options:

**Option 1: Sandbox (recommended)** — Keep the full stdlib but deny dangerous capabilities:

```rust
use sema::{Interpreter, Sandbox, Caps};

let interp = Interpreter::builder()
    .with_sandbox(Sandbox::deny(Caps::STRICT))  // deny shell, fs-write, network, env-write, process, llm
    .build();
```

Sandboxed functions remain callable (tab-completable, discoverable) but return a `PermissionDenied` error when invoked.

**Option 2: Minimal** — No stdlib at all, register only what you need:

```rust
let interp = Interpreter::builder()
    .without_stdlib()
    .without_llm()
    .build();
// Register only safe functions manually
```

See [CLI Sandbox docs](./cli.md#sandbox) for the full list of capabilities and affected functions.

## Loading Files and Preloading Modules

### Load a File

`load_file` reads and evaluates a `.sema` file. Definitions persist in the global environment:

```rust
let interp = Interpreter::new();
interp.load_file("prelude.sema")?;
interp.eval_str("(my-prelude-fn 42)")?;
```

You can also embed files at compile time:

```rust
interp.eval_str(include_str!("../scripts/prelude.sema"))?;
```

### Preload Virtual Modules

`preload_module` injects a module into the module cache so that `(import "name")` resolves without a file on disk. This is useful for bundling standard libraries, providing host APIs as importable modules, or testing:

```rust
let interp = Interpreter::new();

// All top-level definitions are exported by default
interp.preload_module("utils", r#"
    (define (double x) (* x 2))
    (define pi 3.14159)
"#)?;

// Use `(module ...)` with `(export ...)` for selective exports
interp.preload_module("math", r#"
    (module math (export square cube)
      (define (square x) (* x x))
      (define (cube x) (* x x x))
      (define internal-helper 42))
"#)?;
```

Scripts can then import these modules as if they were files:

```sema
(import "utils")
(double pi)  ; => 6.28318

(import "math" square)
(square 5)   ; => 25
```

## API Reference

| Type / Method                        | Description                                                      |
| ------------------------------------ | ---------------------------------------------------------------- |
| `Interpreter`                        | Holds the global environment; evaluates code                     |
| `InterpreterBuilder`                 | Configures and builds an `Interpreter`                           |
| `Value`                              | Core value enum — Int, Float, String, List, Map, etc.            |
| `SemaError`                          | Error type with `eval()`, `type_error()`, `arity()` constructors |
| `Sandbox`                            | Configures which capabilities are denied                         |
| `Caps`                               | Capability bitflags (FS_READ, SHELL, NETWORK, etc.)              |
| `Env`                                | Environment (scope chain backed by `Rc<RefCell<BTreeMap>>`)      |
| `intern(s)`                          | Intern a string, returning a `Spur` handle                       |
| `resolve(spur)`                      | Resolve a `Spur` back to a `&str`                                |
| `interp.eval_str(code)`              | Parse and evaluate a string of Sema code                         |
| `interp.load_file(path)`             | Read and evaluate a `.sema` file                                 |
| `interp.preload_module(name, source)`| Inject a virtual module into the import cache                    |
| `interp.register_fn(name, closure)`  | Register a native Rust function callable from Sema               |
