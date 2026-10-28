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
sema-lang = "2.0"
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

### Typed Functions

`register_fn_typed` converts arguments and the result for you. Arguments implement
`FromValue` (`i64`, `f64`, `bool`, `char`, `String`, range-checked integer types,
`Keyword`, `Symbol`, `Value`, `Option<T>` for nil-able, `Vec<T>` for lists and
vectors); results implement `IntoValue`. Wrong argument counts raise `:arity`
errors and wrong types raise `:type-error` errors noting the argument position,
the same classification as a hand-written `register_fn`.

```rust
use sema::{Keyword, SemaError};

interp.register_fn_typed("host/repeat", |text: String, times: usize| {
    Ok::<_, SemaError>(text.repeat(times))
});
interp.register_fn_typed("host/status", |code: i64| {
    Ok::<_, SemaError>(Keyword(if code < 400 { "ok" } else { "error" }.into()))
});
```

Up to six arguments are supported.

### Matching Arguments

For variadic or overloaded functions, `ValueRef::slice` turns the argument list
into a matchable enum. Keyword and symbol names compare with `&str`; maps come
back as `MapRef` (sorted or hash) with `get`, `get_keyword` and `iter`; anything
without its own variant (procedures, promises, records, big numbers) is
`ValueRef::Other`.

```rust
use sema::{SemaError, Value, ValueRef};

interp.register_fn("host/config", |args| {
    match ValueRef::slice(args).as_slice() {
        [] => Ok(Value::nil()),
        [ValueRef::Keyword(k), ValueRef::Int(n)] if *k == "retries" => Ok(Value::int(*n)),
        [ValueRef::Map(m)] => Ok(m.get_keyword("retries").cloned().unwrap_or(Value::nil())),
        other => Err(SemaError::eval(format!("host/config: unexpected {} arguments", other.len()))),
    }
});
```

### Async Functions

Use `register_async_fn` for host work that awaits a future (HTTP, database,
timers). The call splits into three steps so no `Value` crosses a thread:

| Step | Runs on | Role |
| --- | --- | --- |
| `prepare` | interpreter thread | validate `&[Value]`, return an owned `Send` input |
| `run` | Sema's process-wide Tokio pool | await a future over the input, return an owned `Send` output or a host error |
| `finish` | interpreter thread | turn the output into a `Value` |

```rust
use sema::{Interpreter, SemaError, Value};
use std::{convert::Infallible, time::Duration};

let interp = Interpreter::new();
interp.register_async_fn(
    "host/delayed-keyword",
    |args| {
        if args.len() != 1 {
            return Err(SemaError::eval("expected one string argument"));
        }
        args[0]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))
    },
    |name: String| async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok::<String, Infallible>(name)
    },
    |name| Ok(Value::keyword(&name)),
);
assert_eq!(
    interp.eval_str("(host/delayed-keyword \"ready\")")?,
    Value::keyword("ready")
);
```

Calling the function suspends the calling Sema task while the future runs, so
other tasks (`async/spawn`, `async/all`) keep making progress. `eval_str` still
blocks its Rust caller until the whole evaluation finishes.

Two shorter forms build on the same mechanism:

- **`register_async_fn_typed`** takes typed, owned arguments and returns a future:

  ```rust
  interp.register_async_fn_typed("host/delayed-upper", |text: String, ms: u64| async move {
      tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
      Ok::<_, std::convert::Infallible>(text.to_uppercase())
  });
  ```

- **`register_async_fn_cx`** takes one closure with a context. It returns a value
  now (`Ok(value.into())`) or `cx.await_future(make, finish)`, which suspends the
  calling Sema task. `cx.spawn(make)` starts fire-and-forget tasks on Sema's pool
  without you holding a runtime handle, and `cx.cancellation_token()` fires when
  the awaited call is cancelled:

  ```rust
  interp.register_async_fn_cx("host/fetch", |cx, args| {
      let url = args[0].as_str().ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?.to_owned();
      let token = cx.cancellation_token();
      cx.spawn(move || async move { token.cancelled().await; /* clean up */ });
      Ok(cx.await_future(move || fetch(url), |body: String| Ok(Value::string(&body))))
  });
  ```

  `make` closures run on Sema's pool, so Tokio timers and sockets they create are
  in runtime context. There is no way to block on a future inside the closure:
  it runs inside the cooperative runtime, where blocking would freeze every Sema
  task. A closure that returns a ready value also works where callers cannot
  suspend (macro expansion); one that awaits fails there.

What to know before relying on it:

- **Runtime.** `run` executes on Sema's own Tokio pool, not your runtime (the
  example needs `tokio` with the `time` feature in your crate). Do not call
  `block_on` inside it. A `tokio::spawn` inside `run` lands on Sema's pool, and
  resources created on your own runtime follow Tokio's cross-runtime rules.
  Calling `eval_str` from inside an ambient Tokio runtime is not a supported
  context.
- **Errors.** A host error is rendered to text (its `Display` chain) and raised as
  `SemaError::Io("<name>: <text>")`, catchable from Sema as `:io`. Errors from
  `prepare` and `finish` are raised unchanged. Runtime-side failures are `:eval`
  errors whose message starts with `<name>: ` and one of `executor rejected the
  operation`, `operation cancelled`, `deadline exceeded`, `bound exceeded`,
  `worker panicked`, or `payload decode failed`. A panic inside `run` is
  reported as `worker panicked` only under the `unwind` panic profile; with
  `panic = "abort"` the process aborts.
- **Cancellation.** `async/cancel`, `async/with-timeout` and interpreter drop
  drop the host future at its next poll. Write drop-cancel-safe futures; remote
  side effects are not undone, and a future stuck in a blocking call is not
  stopped. Interpreter drop waits at most 2 s for in-flight host futures.
- **Where it cannot be called.** A macro transformer, a synchronous
  `apply`/`call-with-values` path and debug evaluation cannot suspend; the call
  fails there with `<name>: async host function cannot run here`. `prepare`
  and `finish` run inside the runtime and must not call `eval_str` or any
  `block_on`.
- **Captured state.** As with `register_fn`, none of the closures may strongly
  capture a `Value` or `Env`. `run`'s `Send` bound enforces this for the
  worker; `prepare` and `finish` rely on you.

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
- `Value` and `SemaError` are neither `Send` nor `Sync`, so Rust rejects moving or sharing them across threads. This includes values that hold only a number, boolean, symbol, or keyword: a keyword is a key into its thread's string interner, and the same key can mean `"status"` on one thread and `"name"` on another.
- Raw `Spur` keys returned by `intern` have the same restriction, even though their Rust type does not enforce it.
- LLM state (provider registry, usage tracking, budgets) is per-thread and shared across all interpreters on the same thread.

Send owned Rust data across a channel or task boundary, then create Sema values
on the interpreter thread:

```rust
use sema::{Interpreter, Value};

// The worker returns a Rust String, not a Sema Value or an interned key.
let status_name = std::thread::spawn(|| String::from("ready"))
    .join()
    .unwrap();

let interp = Interpreter::new();
interp.env().set_str("status", Value::keyword(&status_name));
assert_eq!(interp.eval_str("(= status :ready)").unwrap(), Value::bool(true));
```

For structured data, transfer a Rust struct or `serde_json::Value` and convert it
on the receiving thread. Resolve existing keyword or symbol names to owned Rust
strings on their original thread before sending them, and convert a `SemaError`
to owned text the same way. Create a separate interpreter inside each worker if
the workers need to evaluate Sema themselves. The same rules apply to async
executors: a task that can move between worker threads must not hold a Sema
value across an `.await`.

## Serving MCP from a Rust host

Use the public `sema::mcp` module to expose an existing interpreter. The
interpreter keeps its definitions, registered host functions, sandbox, and
telemetry configuration. A session is bound to that interpreter and stays on
its thread; passing a different interpreter returns an error.

```toml
[dependencies]
sema-lang = "2.0"
serde_json = "1"
```

```rust
use sema::{Caps, Interpreter, Sandbox};
use sema::mcp::{McpServerOptions, McpSession};
use serde_json::json;

let interpreter = Interpreter::builder()
    .with_sandbox(Sandbox::deny(Caps::STRICT))
    .with_llm(false)
    .with_mcp(false)
    .build();
interpreter.eval_str(r#"
    (deftool greet "Greet someone."
      {:name {:type :string}}
      (lambda (name) (string-append "hello, " name)))
"#)?;

let session = McpSession::new(
    &interpreter,
    McpServerOptions::embedded().server_info("my-host", "2.0.0"),
)?;
let result = session.call_tool(&interpreter, "greet", &json!({"name": "Kameko"}))?;
assert!(!result.is_error);
```

The embedded preset exposes visible `deftool` definitions, `docs`,
`docs_search`, and `info`. `enable_eval()` adds arbitrary evaluation;
`also_tools(["eval"])` without it is an error. `also_tools` adds other builtin
tools, `only_tools` narrows the enabled set, and `exclude_tools` takes
precedence. Hidden tools remain hidden. Builtin tool names and names beginning
with `_` cannot be exposed as user tools.

Enabling `eval` gives the client access to every function registered in the
interpreter, within its sandbox. A tool-name filter does not limit that access.
Rust host functions must enforce their own permissions; the sandbox cannot
restrict arbitrary Rust code inside a callback.

The session can process one JSON-RPC frame with `handle_message`, serve a
blocking `BufRead`/`Write` pair with `serve_on`, or own process stdio with
`serve_stdio`. Its default limits are 16 MiB per input frame, 16 MiB per source
file, 1 MiB of captured output per root, and five minutes per call. Oversized
input lines are drained before processing the next frame. `ToolOutput::Capture`
is required for stdio serving. Output hooks installed by the host are restored
after a call.

Host Rust `println!` bypasses Sema's output capture and can corrupt a stdio
connection. Send host logs through a separate channel. `on_log` receives typed
session events; `on_output` receives detached output and output routed by
`PassThrough` or `Tee`. These callbacks run on the interpreter thread and
must not block or evaluate Sema.

### Async Rust hosts

Use `McpWorker` from Tokio or another async host. It creates the interpreter
on a dedicated thread with a 16 MiB stack. Its cloneable handle exchanges owned
JSON; Sema values never cross the thread boundary. The setup closure runs on
the interpreter thread and can register synchronous or async host functions.

```rust
use sema::Interpreter;
use sema::mcp::{McpServerOptions, McpWorker};
use serde_json::json;

let worker = McpWorker::spawn(
    Interpreter::builder(),
    McpServerOptions::embedded(),
    |interpreter| {
        interpreter.register_async_fn_typed("host/double", |n: i64| async move {
            Ok::<_, std::convert::Infallible>(n * 2)
        });
        interpreter.eval_str(r#"
            (deftool double "Double a number." {:n {:type :integer}}
              (lambda (n) (host/double n)))
        "#)?;
        Ok(())
    },
)?;
let result = worker.handle().call_tool("double", json!({"n": 21})).await?;
assert!(!result.is_error);
worker.shutdown().await?;
```

For explicit cancellation, allocate an id with `handle.next_call_id()`, pass
it to `call_tool_with_id`, and call `handle.cancel(id)` from another task.
A session's `cancel_handle()` provides the same control across native threads.
Cancellation targets a running call; an id that has already settled is inert.
Worker `handle_message` accepts numeric and string JSON-RPC request ids and
processes `notifications/cancelled` without waiting behind the active call.

Cancellation and deadlines reach VM back-edges and external async waits. They
cannot interrupt a CPU-bound native callback or a future blocked in a system
call. Those operations must enforce their own time limits. Panic conversion
requires an unwinding host; the workspace release profile uses `panic=abort`,
where a panic in a host callback terminates the process. Run untrusted tool
files in a separate process.

Do not call synchronous interpreter evaluation from a running host callback.
It returns an error before root submission, including when the target is a
different interpreter on the same thread. Return the callback's result first,
or use the async host-function APIs and process the next evaluation after the
current call settles.

`with_llm_auto_configure(true)` attempts the CLI's environment-based LLM setup;
call `(llm/auto-configure)` explicitly to inspect configuration errors.
`with_workflow_mcp(true)` installs the workflow MCP resolver. Both flags default
to off. Notebook evaluation tools remain unavailable in restricted MCP
sessions. The CLI build hook requires an unrestricted session, and a built
`--mcp` binary has no build hook.

### MCP clients inside an embedded interpreter

The builder enables `mcp/connect`, `mcp/tools`, `mcp/tools->sema`, `mcp/call`,
and `mcp/close` by default. `with_mcp(false)` disables them. A stdio connection
requires `PROCESS`; an HTTP connection requires `NETWORK`.

Connections are stored per thread and are shared by interpreters on that
thread. Close them explicitly with `mcp/close`. Dropping an interpreter does
not close its connections; thread exit tears down stdio children. The workflow
resolver and OAuth browser-open checks also share the last registered MCP
sandbox on the thread. Give independent hosts their own worker threads when
they need independent connection lifetimes and resolver configuration.

A complete stdio server example is in
[`crates/sema/examples/embedded_mcp.rs`](https://github.com/sema-lisp/sema/blob/main/crates/sema/examples/embedded_mcp.rs).

## Security Considerations

See [Migrating to 2.0](/docs/migrating-to-2) when updating a 1.x Rust host.

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
| `interp.register_async_fn(name, prepare, run, finish)` | Register an async native function; `run` awaits on Sema's Tokio pool |
| `interp.register_fn_typed(name, closure)` | Register a native function with typed arguments and result |
| `interp.register_async_fn_typed(name, closure)` | Register an async native function with typed arguments |
| `interp.register_async_fn_cx(name, closure)` | Register an async native function with one closure and an `AsyncCx` |
| `ValueRef::of(&value)` / `ValueRef::slice(args)` | Matchable view of values for pattern matching |
