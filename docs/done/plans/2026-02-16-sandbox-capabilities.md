# Sandbox Capabilities Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `--sandbox` CLI flag and `InterpreterBuilder` API that selectively denies groups of native functions (shell, fs, network, env, process) at runtime.

**Architecture:** A `u64` bitflag `Caps` type in `sema-core` represents capability groups. A `Sandbox` struct holds a denied bitmask. At stdlib registration time, dangerous functions are wrapped with a `sandbox.check()` guard that returns a clear `SemaError::PermissionDenied` when the capability is denied. Functions remain registered (discoverable, tab-completable) but error on invocation.

**Tech Stack:** Pure Rust, no new dependencies. Uses existing `SemaError` error infrastructure.

**Status:** Implemented

---

### Task 1: Add `Caps` bitflag and `Sandbox` type to `sema-core`

**Files:**

- Create: `crates/sema-core/src/sandbox.rs`
- Modify: `crates/sema-core/src/lib.rs`
- Modify: `crates/sema-core/src/error.rs`

**Step 1: Create `crates/sema-core/src/sandbox.rs`**

```rust
use crate::SemaError;
use std::fmt;

/// Capability groups for native functions.
/// Each bit represents a group of related operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps(u64);

impl Caps {
    pub const NONE: Self = Self(0);
    pub const FS_READ: Self = Self(1 << 0);
    pub const FS_WRITE: Self = Self(1 << 1);
    pub const SHELL: Self = Self(1 << 2);
    pub const NETWORK: Self = Self(1 << 3);
    pub const ENV_READ: Self = Self(1 << 4);
    pub const ENV_WRITE: Self = Self(1 << 5);
    pub const PROCESS: Self = Self(1 << 6);
    pub const LLM: Self = Self(1 << 7);

    /// Union of all capabilities.
    pub const ALL: Self = Self(
        Self::FS_READ.0
            | Self::FS_WRITE.0
            | Self::SHELL.0
            | Self::NETWORK.0
            | Self::ENV_READ.0
            | Self::ENV_WRITE.0
            | Self::PROCESS.0
            | Self::LLM.0,
    );

    /// Common "strict" preset: no shell, no fs-write, no network, no env-write, no process.
    pub const STRICT: Self = Self(
        Self::SHELL.0
            | Self::FS_WRITE.0
            | Self::NETWORK.0
            | Self::ENV_WRITE.0
            | Self::PROCESS.0
            | Self::LLM.0,
    );

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0 && other.0 != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Human-readable name for error messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::FS_READ => "fs-read",
            Self::FS_WRITE => "fs-write",
            Self::SHELL => "shell",
            Self::NETWORK => "network",
            Self::ENV_READ => "env-read",
            Self::ENV_WRITE => "env-write",
            Self::PROCESS => "process",
            Self::LLM => "llm",
            _ => "unknown",
        }
    }

    /// Parse a capability name from CLI input.
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "fs-read" => Some(Self::FS_READ),
            "fs-write" => Some(Self::FS_WRITE),
            "shell" => Some(Self::SHELL),
            "network" | "net" => Some(Self::NETWORK),
            "env-read" => Some(Self::ENV_READ),
            "env-write" => Some(Self::ENV_WRITE),
            "process" | "proc" => Some(Self::PROCESS),
            "llm" => Some(Self::LLM),
            _ => None,
        }
    }
}

impl fmt::Display for Caps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Sandbox configuration controlling which capabilities are denied.
#[derive(Clone, Debug)]
pub struct Sandbox {
    denied: Caps,
}

impl Default for Sandbox {
    fn default() -> Self {
        Self::allow_all()
    }
}

impl Sandbox {
    /// No restrictions — all capabilities allowed.
    pub fn allow_all() -> Self {
        Self { denied: Caps::NONE }
    }

    /// Deny specific capabilities.
    pub fn deny(denied: Caps) -> Self {
        Self { denied }
    }

    /// Returns true if no capabilities are denied.
    pub fn is_unrestricted(&self) -> bool {
        self.denied == Caps::NONE
    }

    /// Check whether a capability is allowed. Returns `Err` with a
    /// `PermissionDenied` error if the capability is denied.
    pub fn check(&self, required: Caps, fn_name: &str) -> Result<(), SemaError> {
        if self.denied.contains(required) {
            Err(SemaError::PermissionDenied {
                function: fn_name.to_string(),
                capability: required.name().to_string(),
            })
        } else {
            Ok(())
        }
    }

    /// Parse a `--sandbox` CLI value like "no-shell,no-network,no-fs-write" or "strict".
    pub fn parse_cli(value: &str) -> Result<Self, String> {
        if value == "strict" {
            return Ok(Self::deny(Caps::STRICT));
        }
        if value == "all" {
            return Ok(Self::deny(Caps::ALL));
        }

        let mut denied = Caps::NONE;
        for part in value.split(',') {
            let part = part.trim();
            let cap_name = part.strip_prefix("no-").unwrap_or(part);
            match Caps::from_name(cap_name) {
                Some(cap) => denied = denied.union(cap),
                None => return Err(format!("unknown sandbox capability: {cap_name}")),
            }
        }
        Ok(Self::deny(denied))
    }
}
```

**Step 2: Add `PermissionDenied` variant to `SemaError`**

In `crates/sema-core/src/error.rs`, add after the `Io` variant:

```rust
    #[error("Permission denied: {function} requires '{capability}' capability")]
    PermissionDenied {
        function: String,
        capability: String,
    },
```

**Step 3: Re-export from `crates/sema-core/src/lib.rs`**

Add `pub mod sandbox;` and re-export:

```rust
pub use sandbox::{Caps, Sandbox};
```

**Step 4: Run tests to verify no breakage**

Run: `cargo test -p sema-core`
Expected: All existing tests pass, new types compile.

**Step 5: Commit**

```bash
git add crates/sema-core/src/sandbox.rs crates/sema-core/src/error.rs crates/sema-core/src/lib.rs
git commit -m "feat: add Caps/Sandbox types and PermissionDenied error variant"
```

---

### Task 2: Add `register_fn_gated` and thread `Sandbox` through stdlib registration

**Files:**

- Modify: `crates/sema-stdlib/src/lib.rs`

**Step 1: Add `register_fn_gated` helper and update `register_stdlib` signature**

Change `register_stdlib` to accept a `Sandbox` parameter. Add `register_fn_gated` that wraps closures with a sandbox check. Modules that don't need gating still use `register_fn` unchanged.

```rust
use sema_core::{Caps, Env, Sandbox, Value};

pub fn register_stdlib(env: &Env, sandbox: &Sandbox) {
    arithmetic::register(env);
    comparison::register(env);
    list::register(env);
    string::register(env);
    predicates::register(env);
    map::register(env);
    #[cfg(not(target_arch = "wasm32"))]
    io::register(env, sandbox);
    math::register(env);
    #[cfg(not(target_arch = "wasm32"))]
    system::register(env, sandbox);
    json::register(env);
    meta::register(env);
    regex_ops::register(env);
    #[cfg(not(target_arch = "wasm32"))]
    http::register(env, sandbox);
    bitwise::register(env);
    crypto::register(env);
    datetime::register(env);
    csv_ops::register(env);
    bytevector::register(env);
    #[cfg(not(target_arch = "wasm32"))]
    terminal::register(env);
}

fn register_fn_gated(
    env: &Env,
    sandbox: &Sandbox,
    cap: Caps,
    name: &str,
    f: impl Fn(&[Value]) -> Result<Value, sema_core::SemaError> + 'static,
) {
    if sandbox.is_unrestricted() {
        // Fast path: no sandbox, register the function directly (zero overhead).
        register_fn(env, name, f);
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        register_fn(env, name, move |args| {
            sandbox.check(cap, &fn_name)?;
            f(args)
        });
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p sema-stdlib`
Expected: Compilation errors in downstream crates that call `register_stdlib` (expected — fixed in later tasks).

**Step 3: Commit**

```bash
git add crates/sema-stdlib/src/lib.rs
git commit -m "feat: add register_fn_gated and thread Sandbox through register_stdlib"
```

---

### Task 3: Gate dangerous functions in `system.rs`

**Files:**

- Modify: `crates/sema-stdlib/src/system.rs`

**Step 1: Update `register` to accept `&Sandbox` and gate functions**

Change the signature to `pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox)`.

Replace `register_fn` with `register_fn_gated` for dangerous functions:

| Function      | Capability        |
| ------------- | ----------------- |
| `shell`       | `Caps::SHELL`     |
| `exit`        | `Caps::PROCESS`   |
| `env`         | `Caps::ENV_READ`  |
| `sys/set-env` | `Caps::ENV_WRITE` |
| `sys/env-all` | `Caps::ENV_READ`  |
| `sys/args`    | `Caps::PROCESS`   |
| `sys/which`   | `Caps::PROCESS`   |
| `sys/pid`     | `Caps::PROCESS`   |

Leave safe functions ungated: `time-ms`, `sleep`, `sys/cwd`, `sys/platform`, `sys/arch`, `sys/os`, `sys/home-dir`, `sys/temp-dir`, `sys/hostname`, `sys/user`, `sys/interactive?`, `sys/tty`, `sys/elapsed`.

Use `use crate::{register_fn, register_fn_gated};` at the import site. The calls look like:

```rust
register_fn_gated(env, sandbox, Caps::SHELL, "shell", |args| { ... });
register_fn_gated(env, sandbox, Caps::PROCESS, "exit", |args| { ... });
register_fn_gated(env, sandbox, Caps::ENV_READ, "env", |args| { ... });
register_fn_gated(env, sandbox, Caps::ENV_WRITE, "sys/set-env", |args| { ... });
register_fn_gated(env, sandbox, Caps::ENV_READ, "sys/env-all", |args| { ... });
register_fn_gated(env, sandbox, Caps::PROCESS, "sys/args", |args| { ... });
register_fn_gated(env, sandbox, Caps::PROCESS, "sys/which", |args| { ... });
register_fn_gated(env, sandbox, Caps::PROCESS, "sys/pid", |args| { ... });
```

**Step 2: Verify it compiles**

Run: `cargo check -p sema-stdlib`
Expected: Compiles.

**Step 3: Commit**

```bash
git add crates/sema-stdlib/src/system.rs
git commit -m "feat: gate shell/env/process functions with sandbox capabilities"
```

---

### Task 4: Gate dangerous functions in `io.rs`

**Files:**

- Modify: `crates/sema-stdlib/src/io.rs`

**Step 1: Update `register` to accept `&Sandbox` and gate file functions**

Change signature to `pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox)`.

Gate file functions:

| Function             | Capability       |
| -------------------- | ---------------- |
| `file/read`          | `Caps::FS_READ`  |
| `file/exists?`       | `Caps::FS_READ`  |
| `file/is-directory?` | `Caps::FS_READ`  |
| `file/is-file?`      | `Caps::FS_READ`  |
| `file/is-symlink?`   | `Caps::FS_READ`  |
| `file/list`          | `Caps::FS_READ`  |
| `file/read-lines`    | `Caps::FS_READ`  |
| `file/for-each-line` | `Caps::FS_READ`  |
| `file/fold-lines`    | `Caps::FS_READ`  |
| `file/info`          | `Caps::FS_READ`  |
| `path/absolute`      | `Caps::FS_READ`  |
| `load`               | `Caps::FS_READ`  |
| `file/write`         | `Caps::FS_WRITE` |
| `file/append`        | `Caps::FS_WRITE` |
| `file/delete`        | `Caps::FS_WRITE` |
| `file/rename`        | `Caps::FS_WRITE` |
| `file/mkdir`         | `Caps::FS_WRITE` |
| `file/write-lines`   | `Caps::FS_WRITE` |
| `file/copy`          | `Caps::FS_WRITE` |

Leave ungated: `display`, `print`, `println`, `newline`, `print-error`, `println-error`, `read-line`, `read-stdin`, `read`, `read-many`, `error`, `path/join`, `path/dirname`, `path/basename`, `path/extension`.

Note: `file/for-each-line` and `file/fold-lines` use `call_function` / `sema_eval_value` from `list.rs` — the closure bodies don't change, only the wrapping.

**Step 2: Verify it compiles**

Run: `cargo check -p sema-stdlib`
Expected: Compiles.

**Step 3: Commit**

```bash
git add crates/sema-stdlib/src/io.rs
git commit -m "feat: gate file I/O functions with sandbox capabilities"
```

---

### Task 5: Gate all functions in `http.rs`

**Files:**

- Modify: `crates/sema-stdlib/src/http.rs`

**Step 1: Update `register` to accept `&Sandbox` and gate all HTTP functions**

Change signature to `pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox)`.

Gate all 5 functions with `Caps::NETWORK`:

```rust
register_fn_gated(env, sandbox, Caps::NETWORK, "http/get", |args| { ... });
register_fn_gated(env, sandbox, Caps::NETWORK, "http/post", |args| { ... });
register_fn_gated(env, sandbox, Caps::NETWORK, "http/put", |args| { ... });
register_fn_gated(env, sandbox, Caps::NETWORK, "http/delete", |args| { ... });
register_fn_gated(env, sandbox, Caps::NETWORK, "http/request", |args| { ... });
```

**Step 2: Verify it compiles**

Run: `cargo check -p sema-stdlib`
Expected: Compiles.

**Step 3: Commit**

```bash
git add crates/sema-stdlib/src/http.rs
git commit -m "feat: gate HTTP functions with sandbox network capability"
```

---

### Task 6: Update `sema-eval::Interpreter` and `sema::InterpreterBuilder`

**Files:**

- Modify: `crates/sema-eval/src/eval.rs`
- Modify: `crates/sema/src/lib.rs`

**Step 1: Update `sema-eval::Interpreter::new()` to pass default sandbox**

In `crates/sema-eval/src/eval.rs`, change the `register_stdlib` call:

```rust
sema_stdlib::register_stdlib(&env, &sema_core::Sandbox::allow_all());
```

**Step 2: Add `sandbox` field to `InterpreterBuilder`**

In `crates/sema/src/lib.rs`:

```rust
pub use sema_core::{Caps, Sandbox};

pub struct InterpreterBuilder {
    stdlib: bool,
    llm: bool,
    sandbox: Sandbox,
}

impl InterpreterBuilder {
    pub fn new() -> Self {
        Self {
            stdlib: true,
            llm: true,
            sandbox: Sandbox::allow_all(),
        }
    }

    /// Set the sandbox configuration.
    pub fn with_sandbox(mut self, sandbox: Sandbox) -> Self {
        self.sandbox = sandbox;
        self
    }

    // ... existing methods ...
}
```

In `build()`, pass `&self.sandbox` to `register_stdlib`:

```rust
if self.stdlib {
    sema_stdlib::register_stdlib(&env, &self.sandbox);
}
```

**Step 3: Verify it compiles**

Run: `cargo check -p sema`
Expected: Compiles.

**Step 4: Commit**

```bash
git add crates/sema-eval/src/eval.rs crates/sema/src/lib.rs
git commit -m "feat: thread Sandbox through InterpreterBuilder"
```

---

### Task 7: Add `--sandbox` CLI flag

**Files:**

- Modify: `crates/sema/src/main.rs`

**Step 1: Add the CLI argument to the `Cli` struct**

```rust
    /// Sandbox mode: restrict dangerous operations.
    /// Values: "strict", "all", or comma-separated "no-shell,no-network,no-fs-write,..."
    /// Capabilities: shell, fs-read, fs-write, network, env-read, env-write, process, llm
    #[arg(long)]
    sandbox: Option<String>,
```

**Step 2: Parse and apply in `main()`**

After `Cli::parse()`, before creating the interpreter, parse the sandbox value:

```rust
    let sandbox = match &cli.sandbox {
        Some(value) => sema_core::Sandbox::parse_cli(value).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }),
        None => sema_core::Sandbox::allow_all(),
    };
```

Then change `Interpreter::new()` to use the builder:

```rust
    let interpreter = sema::InterpreterBuilder::new()
        .with_sandbox(sandbox)
        .build();
```

Wait — `main.rs` currently uses `sema_eval::Interpreter` directly, not the `sema::InterpreterBuilder`. We need to switch it to use `sema::InterpreterBuilder` to thread the sandbox through, or add sandbox support to the `sema_eval::Interpreter` as well.

Looking at the existing code: `main.rs` uses `sema_eval::Interpreter` and accesses `.global_env` and `.ctx` directly. The simplest approach is to add a `new_with_sandbox` constructor to `sema_eval::Interpreter`:

```rust
    pub fn new_with_sandbox(sandbox: &sema_core::Sandbox) -> Self {
        let env = Env::new();
        let ctx = EvalContext::new();
        sema_stdlib::register_stdlib(&env, sandbox);
        #[cfg(not(target_arch = "wasm32"))]
        {
            sema_llm::builtins::reset_runtime_state();
            sema_llm::builtins::register_llm_builtins(&env);
            sema_llm::builtins::set_eval_callback(eval_value);
        }
        Interpreter {
            global_env: Rc::new(env),
            ctx,
        }
    }
```

Then in `main.rs`:

```rust
    let interpreter = Interpreter::new_with_sandbox(&sandbox);
```

**Step 3: Verify it compiles and runs**

Run: `cargo run -- -e "(+ 1 2)"`
Expected: `3`

Run: `cargo run -- --sandbox=no-shell -e '(shell "echo hi")'`
Expected: Error: Permission denied: shell requires 'shell' capability

Run: `cargo run -- --sandbox=strict -e '(println "hello")'`
Expected: `hello` (println is not gated)

**Step 4: Commit**

```bash
git add crates/sema-eval/src/eval.rs crates/sema/src/main.rs
git commit -m "feat: add --sandbox CLI flag"
```

---

### Task 8: Gate LLM functions

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`

**Step 1: Update `register_llm_builtins` to accept `&Sandbox`**

Change signature: `pub fn register_llm_builtins(env: &Env, sandbox: &sema_core::Sandbox)`.

Gate `llm/complete`, `llm/chat`, and `llm/send` with `Caps::LLM`. These also make network calls, so gate with `Caps::LLM` (the `--sandbox=strict` preset includes `LLM`). Use the same `register_fn_gated` pattern but inline it since `sema-llm` has its own `register_fn`:

```rust
fn register_fn_gated(
    env: &Env,
    sandbox: &sema_core::Sandbox,
    cap: sema_core::Caps,
    name: &str,
    f: impl Fn(&[Value]) -> Result<Value, SemaError> + 'static,
) {
    if sandbox.is_unrestricted() {
        register_fn(env, name, f);
    } else {
        let sandbox = sandbox.clone();
        let fn_name = name.to_string();
        register_fn(env, name, move |args| {
            sandbox.check(cap, &fn_name)?;
            f(args)
        });
    }
}
```

Gate: `llm/complete`, `llm/chat`, `llm/send` with `Caps::LLM`.
Leave ungated: `llm/configure`, `llm/auto-configure`, `llm/define-provider` (configuration, not execution).

Note: `llm/chat` uses `register_fn_ctx` — you'll need a `register_fn_ctx_gated` variant too, following the same pattern.

**Step 2: Update callers**

In `crates/sema-eval/src/eval.rs` and `crates/sema/src/lib.rs`, pass `&sandbox` to `register_llm_builtins`.

**Step 3: Verify**

Run: `cargo run -- --sandbox=no-llm -e '(llm/complete "test")'`
Expected: Error: Permission denied: llm/complete requires 'llm' capability

Run: `cargo run -- --sandbox=no-llm -e '(+ 1 2)'`
Expected: `3`

**Step 4: Commit**

```bash
git add crates/sema-llm/src/builtins.rs crates/sema-eval/src/eval.rs crates/sema/src/lib.rs
git commit -m "feat: gate LLM functions with sandbox llm capability"
```

---

### Task 9: Add integration tests

**Files:**

- Modify: `crates/sema/tests/integration_test.rs`

**Step 1: Write sandbox integration tests**

Add a helper that creates a sandboxed interpreter:

```rust
fn eval_sandboxed(denied: sema_core::Caps, input: &str) -> Result<Value, SemaError> {
    let sandbox = sema_core::Sandbox::deny(denied);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    interp.eval_str(input)
}

#[test]
fn test_sandbox_shell_denied() {
    let result = eval_sandboxed(sema_core::Caps::SHELL, r#"(shell "echo hi")"#);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("Permission denied"),
        "Expected permission denied, got: {err}"
    );
}

#[test]
fn test_sandbox_shell_allowed() {
    let result = eval_sandboxed(sema_core::Caps::NETWORK, r#"(shell "echo hi")"#);
    assert!(result.is_ok(), "shell should be allowed when only network is denied");
}

#[test]
fn test_sandbox_fs_write_denied() {
    let result = eval_sandboxed(sema_core::Caps::FS_WRITE, r#"(file/write "/tmp/test.txt" "hi")"#);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Permission denied"));
}

#[test]
fn test_sandbox_fs_read_denied() {
    let result = eval_sandboxed(sema_core::Caps::FS_READ, r#"(file/exists? "/tmp")"#);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Permission denied"));
}

#[test]
fn test_sandbox_env_denied() {
    let result = eval_sandboxed(sema_core::Caps::ENV_READ, r#"(env "HOME")"#);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Permission denied"));
}

#[test]
fn test_sandbox_safe_functions_always_work() {
    // Pure functions should never be blocked regardless of sandbox
    let result = eval_sandboxed(sema_core::Caps::ALL, "(+ 1 2)");
    assert_eq!(result.unwrap(), Value::Int(3));

    let result = eval_sandboxed(sema_core::Caps::ALL, r#"(string-append "a" "b")"#);
    assert_eq!(result.unwrap(), Value::string("ab"));
}

#[test]
fn test_sandbox_println_always_works() {
    let result = eval_sandboxed(sema_core::Caps::ALL, r#"(println "hello")"#);
    assert!(result.is_ok(), "println should never be sandboxed");
}

#[test]
fn test_sandbox_strict_preset() {
    let sandbox = sema_core::Sandbox::parse_cli("strict").unwrap();
    // strict denies shell, fs-write, network, env-write, process, llm
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(shell "echo hi")"#);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Permission denied"));
}

#[test]
fn test_sandbox_parse_cli() {
    let sandbox = sema_core::Sandbox::parse_cli("no-shell,no-network").unwrap();
    let interp = Interpreter::new_with_sandbox(&sandbox);

    // shell denied
    assert!(interp.eval_str(r#"(shell "echo hi")"#).is_err());

    // fs-read still allowed
    assert!(interp.eval_str(r#"(file/exists? "/tmp")"#).is_ok());
}
```

**Step 2: Run the tests**

Run: `cargo test -p sema --test integration_test -- test_sandbox`
Expected: All sandbox tests pass.

**Step 3: Commit**

```bash
git add crates/sema/tests/integration_test.rs
git commit -m "test: add sandbox integration tests"
```

---

### Task 10: Add `--sandbox` help text to REPL and CLI

**Files:**

- Modify: `crates/sema/src/main.rs`

**Step 1: Print sandbox status in REPL banner**

In the `repl()` function, after the version line, if sandbox is active, print:

```
Sandbox active: shell, network denied
```

This requires passing the `Sandbox` (or a flag) to `repl()`. Add a `sandbox: Option<String>` parameter or just pass the raw CLI value:

```rust
    if !quiet {
        println!(
            "Sema v{} — A Lisp with LLM primitives",
            env!("CARGO_PKG_VERSION")
        );
        if let Some(ref mode) = sandbox_mode {
            println!("Sandbox: {mode}");
        }
        println!("Type ,help for help, ,quit to exit\n");
    }
```

**Step 2: Verify**

Run: `cargo run -- --sandbox=strict`
Expected: REPL shows "Sandbox: strict" in banner.

**Step 3: Commit**

```bash
git add crates/sema/src/main.rs
git commit -m "feat: show sandbox status in REPL banner"
```

---

## Summary of Changes

| Crate         | Files changed                         | Effort              |
| ------------- | ------------------------------------- | ------------------- |
| `sema-core`   | +sandbox.rs, ~error.rs, ~lib.rs       | Small               |
| `sema-stdlib` | ~lib.rs, ~system.rs, ~io.rs, ~http.rs | Medium (mechanical) |
| `sema-llm`    | ~builtins.rs                          | Small               |
| `sema-eval`   | ~eval.rs                              | Tiny                |
| `sema`        | ~lib.rs, ~main.rs, +tests             | Small               |

**Total estimated effort: 2-3 hours**

## Capability Group Summary

| Group       | CLI name    | Functions                                                                                                                                                                                             |
| ----------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `FS_READ`   | `fs-read`   | `file/read`, `file/exists?`, `file/list`, `file/is-file?`, `file/is-directory?`, `file/is-symlink?`, `file/read-lines`, `file/for-each-line`, `file/fold-lines`, `file/info`, `path/absolute`, `load` |
| `FS_WRITE`  | `fs-write`  | `file/write`, `file/append`, `file/delete`, `file/rename`, `file/mkdir`, `file/write-lines`, `file/copy`                                                                                              |
| `SHELL`     | `shell`     | `shell`                                                                                                                                                                                               |
| `NETWORK`   | `network`   | `http/get`, `http/post`, `http/put`, `http/delete`, `http/request`                                                                                                                                    |
| `ENV_READ`  | `env-read`  | `env`, `sys/env-all`                                                                                                                                                                                  |
| `ENV_WRITE` | `env-write` | `sys/set-env`                                                                                                                                                                                         |
| `PROCESS`   | `process`   | `exit`, `sys/args`, `sys/pid`, `sys/which`                                                                                                                                                            |
| `LLM`       | `llm`       | `llm/complete`, `llm/chat`, `llm/send`                                                                                                                                                                |

## CLI Usage Examples

```bash
# No sandbox (default)
sema script.sema

# Deny shell only
sema --sandbox=no-shell script.sema

# Deny multiple capabilities
sema --sandbox=no-shell,no-network,no-fs-write script.sema

# Strict preset (deny shell, fs-write, network, env-write, process, llm)
sema --sandbox=strict script.sema

# Deny everything dangerous
sema --sandbox=all script.sema
```

## Embedder API

```rust
use sema::{InterpreterBuilder, Sandbox, Caps};

// Allow only pure computation + fs-read
let interp = InterpreterBuilder::new()
    .with_sandbox(Sandbox::deny(
        Caps::SHELL.union(Caps::NETWORK).union(Caps::FS_WRITE).union(Caps::ENV_WRITE)
    ))
    .build();
```
