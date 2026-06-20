# Sema Lisp — Style, Security, and Architectural Review

> ✅ **ARCHIVED (2026-06-20) — one-time audit, findings actioned or tracked.** The
> deep findings shipped (§2.1/§2.2 stack-balance verifier — ADR #56; the §8
> stabilization items — see the RESOLVED banners with commit refs). The §3 SSRF
> notes are explicitly out of scope. The §4 "tree-walker vs VM" divergences are
> moot since the tree-walker was retired in 1.18.0, except the two real residuals
> tracked in `docs/deferred.md`: VM stack traces (**VM-1**) and the `(type (fn))`
> → `:native-fn` reflection nit (**C1 follow-up**). Kept for historical context;
> not a live checklist.

This document presents a deep-dive technical assessment of the **Sema Lisp** monorepo codebase. It evaluates the project's architecture, memory model, compiler/VM safety, language server protocol (LSP) implementation, and sandbox security constraints against the state-of-the-art in Rust system engineering.

Recommendations and findings are backed by citations of standard Rust guidelines (e.g., [mre/idiomatic-rust](https://github.com/mre/idiomatic-rust), the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)) and security auditing practices.

---

## 1. Monorepo Architecture & Dependency Hygiene

Sema uses a multi-crate Cargo workspace divided into 13 distinct crates, implementing a clear separation of concerns (dependency flow: `sema-core ← sema-reader ← sema-vm ← sema-eval ← sema-stdlib/sema-llm ← sema`).

### 1.1 Dependency Duplication and Version Skew
Sema utilizes `[workspace.dependencies]` in the root [Cargo.toml](file:///Users/helge/code/sema-lisp/Cargo.toml) to enforce version lock-step. However, two frontend/target crates bypass this centralization:
1. **[crates/sema-wasm/Cargo.toml](file:///Users/helge/code/sema-lisp/crates/sema-wasm/Cargo.toml)**:
   * Explicitly redeclares path dependencies (e.g., `sema-core = { path = "../sema-core" }`) rather than inheriting from the workspace: `sema-core.workspace = true`.
   * Directly declares third-party libraries (e.g., `wasm-bindgen`, `js-sys`, `web-sys`, `wasm-bindgen-futures`, `getrandom`) with hardcoded versions instead of lifting them to the root.
2. **[crates/sema-notebook/Cargo.toml](file:///Users/helge/code/sema-lisp/crates/sema-notebook/Cargo.toml)**:
   * Declares `gag = "1.0.0"` locally, bypassing the workspace dependencies block.

### 1.2 Clippy Suppression Antipattern
Almost all crate entry points (e.g. `lib.rs` in `sema-core`, `sema-eval`, `sema-stdlib`, `sema-vm`) declare `#![allow(clippy::mutable_key_type)]` and `#![allow(clippy::cloned_ref_to_slice_refs)]`.
* `clippy::mutable_key_type` triggers because `Value` uses interior mutability (due to Nan-boxing wrapping pointer allocations like `Rc<RefCell<...>>`) and is used as keys in `HashMap` or `HashSet` (e.g., inside environment frames).
* *Architectural Risk*: Placing this allowance at the crate root suppresses legitimate bugs where key objects mutate *after* insertion, leading to orphan hash values and memory leaks or logically unreachable map nodes.

---

## 2. Virtual Machine Deep-Dive: Safety & Robustness

The virtual machine (`sema-vm`) is a stack-based bytecode execution engine. It uses NaN-boxing for the representation of Lisp values (wrapping pointers and immediates into a single `u64`).

> **Status (2026-06-16, RESOLVED):** §2.1 and §2.2 are now **FIXED**. A sound abstract stack-depth verifier (`Op::stack_effect` + worklist abstract interpretation in `validate_bytecode`, including validated exception-handler depths) rejects any unbalanced/underflowing `.semac` before execution, so `pop_unchecked` is sound for untrusted bytecode. The earlier `DUP`-on-empty and `CallNative` bounds-check point-fixes remain. Regression tests: `crates/sema-vm/tests/bytecode_validator_regression.rs`. See §8 (second follow-up) for details. The historical description below is retained for context.

### 2.1 Stack Underflow Vulnerability (`pop_unchecked`)
In the core dispatch loop of [crates/sema-vm/src/vm.rs:555](file:///Users/helge/code/sema-lisp/crates/sema-vm/src/vm.rs#L555), the helper function `pop_unchecked` is implemented as:
```rust
unsafe fn pop_unchecked(stack: &mut Vec<Value>) -> Value {
    let len = stack.len();
    debug_assert!(len > 0, "pop_unchecked on empty stack");
    let v = std::ptr::read(stack.as_ptr().add(len - 1));
    stack.set_len(len - 1);
    v
}
```
* *The Bug*: This function assumes that the compiler has guaranteed stack balance. While this is true for bytecode produced in-memory by the compiler, **it is not validated for deserialized `.semac` files** loaded from disk or network.
* *The Risk*: A handcrafted `.semac` file with an unbalanced instruction sequence (e.g. executing `Pop` or `Dup` on an empty stack) causes `len - 1` to wrap to `usize::MAX`. In release builds (where `debug_assert!` is compiled out), this causes `std::ptr::read` to perform out-of-bounds reads and writes `stack.set_len(usize::MAX)`, corrupting arbitrary memory.

### 2.2 Bytecode Verification Deficit
The bytecode deserializer in [serialize.rs](file:///Users/helge/code/sema-lisp/crates/sema-vm/src/serialize.rs) implements `validate_bytecode`, which checks:
1. Magic header and version numbers.
2. Jump targets (verifying that they land on valid instruction boundaries and not in the middle of operands).
3. Constant pool and upvalue descriptor bounds.

However, it **lacks a stack depth/balance validator** (abstract interpreter). A complete bytecode validator must simulate execution paths to compute the exact stack depth at each program point, rejecting chunks that underflow the stack or exceed the stack frame limits.

### 2.3 Closure Upvalue Leak in HOF Callbacks (Upvalue Closing)
SemaVM implements Lua-style open upvalues ([vm.rs:25](file:///Users/helge/code/sema-lisp/crates/sema-vm/src/vm.rs#L25)). When a closure captures local variables, they refer to stack offsets. When the declaring stack frame exits, they are migrated to the heap ("closed").
* *The Bug*: To support higher-order functions (HOFs) written in Rust (like `map`, `filter`, `retry`), the VM converts Lisp closures into `NativeFn` wrappers. When these HOFs invoke the closure, the VM executes them on a *fresh* VM context (`NativeFn::func`).
* *Divergence*: To make this safe, the VM calls `close_open_upvalues` *before* entering the native HOF bridge. This acts as a snapshot. If the closure mutates its captured variable via `set!`, the mutation lands in the closed snapshot on the heap, and **the local variable on the parent stack remains unchanged**.
* *Reproduction*:
  ```scheme
  (let ((c 0)) 
    (map (fn (x) (set! c (+ c x))) (list 1 2 3)) 
    c)
  ```
  Returns `6` on the Tree-walker, but `0` on the Bytecode VM.

---

## 3. Sandboxing & Security Gaps (SSRF Vulnerability) -- EXPLICITLY IGNORE THIS, ITS NOT IMPORTANT.

Sema provides sandboxed execution limits via `Caps::FS_WRITE`, `Caps::NET`, etc. These restrictions are enforced when registering native builtins.

### 3.1 DNS-Rebinding / SSRF Bypass
In [crates/sema-llm/src/builtins.rs:711](file:///Users/helge/code/sema-lisp/crates/sema-llm/src/builtins.rs#L711), the function `guard_provider_url` is used to prevent sandboxed code from reaching local or loopback targets (Server-Side Request Forgery):
```rust
fn guard_provider_url(unrestricted: bool, opts: &BTreeMap<Value, Value>) -> Result<(), SemaError> {
    if unrestricted { return Ok(()); }
    let url = get_opt_string(opts, "base-url").or_else(|| get_opt_string(opts, "host"));
    if let Some(url) = url {
        if url_host(&url).is_some_and(|h| is_internal_host(&h)) {
            return Err(SemaError::eval(...));
        }
    }
    Ok(())
}
```
* *The Bug*: The validation of `is_internal_host` only parses the host string *prior* to connection. If a hostname is supplied (e.g. `evil-endpoint.local-check.com`), the host parsing checks fail to match loopback or private subnets. The URL is approved, and `reqwest` later resolves the domain to `127.0.0.1` or `10.0.0.1` at connection time.
* *The Risk*: Sandboxed users can pivot to the host's loopback interface, accessing metadata endpoints, databases, or local LLM instances (like Ollama).

### 3.2 Custom URL Parsing Differential
The helper function `url_host` in `builtins.rs:615` parses URLs manually to extract the hostname:
```rust
fn url_host(url: &str) -> Option<String> {
    let after = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = after.split(['/', '?', '#']).next().unwrap_or("");
    ...
}
```
* *The Bug*: Writing a custom URL parser is a well-known security antipattern. Discrepancies between custom string-slicing logic and the actual parser in `reqwest`/`hyper` (e.g., handling backslashes, userinfo separators `@`, IPv6 square brackets, or encoded characters) can be exploited to bypass checks (a Parser Differential attack).

---

## 4. Tree-Walker vs. VM Divergences

Sema maintains two execution engines. While dual-eval tests exist, there are structural divergences:

### 4.1 Missing VM Stack Traces
The tree-walker backend captures detailed call frames and spans, injecting a `:stack-trace` element into error maps caught by `try-catch`.
* On the Bytecode VM, stack traces are **omitted from caught error maps**, and runtime crashes print limited location tags. This forces developers to use `--tw` (tree-walker) to debug issues, defeating the purpose of the VM as the primary execution engine.

### 4.2 Eval Environment Scoping
In the tree-walker, `(eval expr)` evaluates code within the caller's lexical scope.
* In the VM, `eval` is delegated to `__vm-eval`, which accesses the **global environment only**. Lexical locals are invisible.
  ```scheme
  (define (run x) (eval 'x)) 
  (run 42) ; => Unbound variable: x (on VM)
  ```

### 4.3 Type Reflection Discrepancy
Reflecting on closure types diverges between backends:
```scheme
(type (fn (x) x))
```
* Tree-Walker returns `:lambda`.
* VM returns `:native-fn` (due to the `NativeFn` wrapper applied for stdlib HOF interop).

---

## 5. LSP Architecture & Monolithic Implementations

The Language Server Protocol (`sema-lsp`) is a single-threaded server. It employs an actor pattern to run queries against a shared state.

### 5.1 The 4,800+ Line Monolith
The file [crates/sema-lsp/src/lib.rs](file:///Users/helge/code/sema-lisp/crates/sema-lsp/src/lib.rs) is **~180 KB and contains 4,826 lines of code**. It handles AST re-parsing, diagnostics compilation, command execution, and LSP endpoints (completion, formatting, semantic tokens, hover, definition, and signature help).
* *Architectural Smell*: This violates the Single Responsibility Principle. Legitimate modifications to hover logic require navigating the same file that sets up TCP sockets and parses input frames.

### 5.2 Single-Threaded Actor Design (Decision Justification)
The LSP uses a single-threaded execution thread:
```rust
let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<LspRequest>();
let backend_handle = std::thread::spawn(move || {
    let mut state = BackendState::new();
    while let Some(req) = rx.blocking_recv() { ... }
});
```
* *Why it is idiomatic*: The core `Value` type and `Env` environment of Sema use `Rc` and `RefCell` (non-Send, single-threaded pointers). If the LSP ran requests concurrently across threads, it would require wrapping the entire interpreter state in `Arc<Mutex<Env>>` or `RwLock`, significantly slowing down evaluation and introducing deadlocks. The single-threaded loop treats `BackendState` as an actor, keeping the non-Send state safely pinned to a single thread.

### 5.3 Cooperative Workspace Scanning
To prevent workspace-wide scanning from blocking interactive requests, the scanner is implemented cooperatively using deferred queues:
```rust
LspRequest::ScanWorkspace { root } => {
    let scanner = WorkspaceScanner::new(&root);
    deferred.push_back(LspRequest::ScanWorkspaceContinue { scanner });
}
```
* *How it works*: `ScanWorkspaceContinue` processes a batch of 10 files and then re-enqueues itself at the back of the queue. This ensures that any typing/completion requests sent by the client are processed immediately, preventing LSP freezes during initial imports.

---

## 6. Actionable Implementation Plan

Below is the prioritized roadmap to resolve the identified architectural and security issues.

### Phase 1: Security & Sandboxing (Urgent)
* [ ] **SSRF DNS Hook**: Replace pre-request hostname string validation with connection-level IP verification. Configure the `reqwest` client builder with a custom socket connector or resolve the hostname and validate all resolved IPs against the blacklist.
* [ ] **Integrate `url` Crate**: Delete the custom `url_host` parser in `builtins.rs` and utilize the `url` crate.

### Phase 2: Virtual Machine Security (High)
* [ ] **Bytecode Stack Verifier**: Implement a stack depth verifier in `serialize.rs`. This should run before executing or saving any bytecode, validating that the stack does not underflow or overflow for all code paths.
* [ ] **Safe `pop_unchecked`**: Temporarily add bounds checks in release mode to `pop_unchecked` (or replace with standard `.pop()`) until the verifier guarantees safety.

### Phase 3: Workspace Hygiene & Modularity (Medium)
* [ ] **Unify Subcrate Dependency Definitions**: Refactor `sema-wasm/Cargo.toml` and `sema-notebook/Cargo.toml` to inherit versions and path dependencies from `[workspace.dependencies]`.
* [ ] **Split Monolithic Files**:
  * Deconstruct `sema-llm/src/builtins.rs` into `providers/`, `budget.rs`, and `security.rs`.
  * Divide `sema-lsp/src/lib.rs` into `handlers/` (completion, formatting, etc.), `state.rs`, and `server.rs`.
* [ ] **Scoping allowances**: Restrict `#![allow(clippy::mutable_key_type)]` to specific map declarations where safety is commented.

### Phase 4: Evaluator Convergence (Medium)
* [ ] **Closure HOF Upvalue Fix**: Modify HOF closure execution to pass pointers/references to the mutable stack slots (e.g. using cell indicators) rather than cloning snapshot upvalues.
* [ ] **VM Stack Traces**: Implement trace mapping for caught errors inside the VM interpreter.

---

## 7. DAP (Debug Adapter Protocol) Architecture & Deadlock Resolution

The Debug Adapter Protocol (DAP) implementation (`sema-dap`) enables interactive debugging of Sema programs under the bytecode VM using DAP clients (e.g., VS Code or editor plugins).

### 7.1 The Pre-Configuration Deadlock Condition
During initial integration testing, a race condition and deadlock were identified in the message loop between the async DAP server frontend and the execution thread.
* **The Symptom**: When a DAP client configured breakpoints *after* sending `launch` but *before* sending `configurationDone` (which is standard behavior for many editors), the entire DAP session hung indefinitely.
* **The Root Cause**: 
  1. Once `launch` is processed, a command sender channel `dbg_cmd_tx` is initialized and stored in the server state.
  2. The server previously assumed that `dbg_cmd_tx.is_some()` meant the VM debugger loop was active and responsive.
  3. Consequently, requests like `setBreakpoints`, `stackTrace`, `scopes`, or `variables` were forwarded to the VM via `DebugCommand` sync/oneshot channels.
  4. However, the VM does not actually spawn the execution loop or poll its command channel until `configurationDone` is received.
  5. The server blocked on `reply_rx.recv()`, waiting for a response that would never arrive because the VM was not running yet.

### 7.2 The Resolution (`vm_active` Tracking)
To resolve the deadlock, we decoupled VM command routing from channel presence by tracking the VM's lifecycle state:
1. **State Tracking**: Added a `vm_active: bool` flag to the DAP event loop.
2. **Activation**: The flag is set to `true` when the client sends the `configurationDone` request (which executes the VM via `execute_debug`).
3. **Deactivation**: The flag is reset to `false` when the VM finishes execution and sends a `DebugEvent::Terminated` event.
4. **Conditional Routing**:
   * **`setBreakpoints`**: If `vm_active` is `false`, breakpoints are routed to the backend thread via `BackendRequest::SetBreakpoints` to be stored as pending breakpoints. If `vm_active` is `true`, they are sent to the running VM via `DebugCommand::SetBreakpoints`.
   * **`stackTrace` / `scopes` / `variables`**: If `vm_active` is `false` (e.g. before execution starts or after it finishes), the server responds immediately with empty/default arrays instead of waiting on the unresponsive VM channel.

> **Status (2026-06-16):** this `vm_active`-based routing is now known to be **fragile**. `vm_active` is a frontend-side guess about backend state across an async boundary; it can desync from the actual VM (set `true` at `configurationDone` before the VM loop is polling; left `Some`/stale on launch errors; not reset on early termination). A post-merge fix replaced the unbounded `reply_rx.recv()` with a 2-second `DEBUG_REPLY_TIMEOUT`, which prevents the hang but **silently returns empty data when the VM is alive but busy in a long blocking native call**. See DAP-1/DAP-2/DAP-3 in §8 for the deeper fix (gate inspection on `vm_suspended`; have the backend drain `command_rx` on exit).

### 7.3 Verification
A new integration test `test_dap_breakpoint_after_launch` was added to [crates/sema-dap/tests/integration_test.rs](file:///Users/helge/code/sema-lisp/crates/sema-dap/tests/integration_test.rs#L317-L400). It explicitly sets a breakpoint between the `launch` and `configurationDone` commands using a strict 2-second timeout helper `read_dap_timeout` to guard against regressions.

### 7.4 Feature Status (updated 2026-06-16 after PR #44)
PR #44 closed most of the gaps originally listed here. Current status:

**FIXED (verified in code):**
1. **IntelliJ `stopOnEntry`**: now `false` by default (`SemaDebugAdapterDescriptorFactory.kt` sends `"stopOnEntry": false`; server default is `false`; asserted by `SemaDebugAdapterDescriptorFactoryTest.kt`).
2. **Path/URI Format Mismatch**: resolved via `clean_path`/`decode_percent` in `server.rs` (strips `file://`, percent-decodes). *Residual latent gap: `decode_percent` pushes raw bytes as `char`, so multi-byte UTF-8 percent-encoding in non-ASCII paths decodes incorrectly — low severity, see §8.3.*
3. **Breakpoint Verification & Sliding**: implemented — `set_breakpoints` snaps to the nearest executable line and returns `verified: false` with a message when no executable line exists (`debug.rs`); covered by `test_dap_breakpoint_on_blank_line_slides_to_executable_line`.
6. **`evaluate` / `setVariable`**: both implemented (`server.rs`, `vm.rs`, `debug_evaluate_mut`/`debug_set_variable`), capabilities advertised, integration-tested. *But see DAP-4 in §8.2 for a shadowed-local correctness bug in the write path.*

**STILL OPEN:**
4. **Tree-Walker Code Bypasses Debugger**: code loaded/imported via `(load …)` / `(import …)` runs on the tree-walker, bypassing the VM debug loop — breakpoints in dynamically loaded files are never hit. No mitigation.
5. **Mocked Threads**: `threads` still returns a single hardcoded `{ "id": 1, "name": "main" }`. Acceptable for synchronous code, but async tasks/channels are invisible to the debugger.

---

## 8. Stabilization Review — Post MCP/DAP Merge (2026-06-16)

After merging **PR #43** (built-in MCP server) and **PR #44** (DAP ergonomics) into `main`, an adversarial post-merge review fixed 10 issues (commit `d2a5e6e`). A follow-up stabilization pass (this section) examined the two newly-shipped subsystems plus the VM safety items in §2 to identify what is still **fragile** before cutting the next release. Findings are grouped by priority. Each lists file pointers, the concrete trigger, and a hardening direction.

> **Status (2026-06-16, follow-up):** the release-blocker and shadowed-local items are now **FIXED** (no band-aids — the DAP timeout was removed in favour of a guaranteed-reply architecture):
> - **MCP-1 FIXED** — tool dispatch is wrapped in `catch_unwind`; a panic becomes an `isError` result. `sys/args` override now restored via a `Drop` guard (also covers MCP-6).
> - **MCP-2 FIXED** — `gag` fd-redirection removed entirely (dependency dropped); `eval_with_capture` and the notebook engine now capture via sema-core's thread-local output hook, so program output can never reach the protocol fd, and concurrent engine threads no longer contend on a global redirect.
> - **MCP-3 RESOLVED (by decision)** — sandboxing stays **opt-in / allow-all by default**, consistent with the CLI/REPL/notebook (deliberate, not a default change). Only the misleading `info` string was corrected to state the unrestricted posture honestly.
> - **DAP-1/2/3 FIXED** — `DEBUG_REPLY_TIMEOUT` removed; the backend now drains `command_rx` after `execute_debug` (replying to late commands until the frontend drops its sender), inspection handlers are gated on `vm_suspended`, and `dbg_cmd_tx` is cleared on `Terminated`. Replies are now guaranteed, so the blocking reply wait can't hang and never returns fabricated empty data for a live VM. Regression test: `test_dap_inspection_after_termination_does_not_hang`.
> - **DAP-4 FIXED** — locals display, `setVariable`, `set!` write-back, and `evaluate` reads all resolve a name through one `in_scope_locals` helper (pc-scoped, innermost-wins), so shadowed bindings are consistent. MCP-2 regression: `test_mcp_print_output_does_not_corrupt_protocol`.
>
> **Still deferred/open (at the time of the first pass):** VM-1, MCP-4, MCP-5, DAP-5..9, §7.4 #4/#5.
>
> **Status (2026-06-16, second follow-up — all now FIXED):**
> - **VM-1 FIXED** — §2.1/§2.2 resolved properly: a sound abstract stack-depth verifier (per-opcode `Op::stack_effect`, worklist abstract interpretation with strict-equality joins) now runs in `validate_bytecode` and rejects any unbalanced `.semac` before execution, making `pop_unchecked` sound for untrusted bytecode. The adversarial review caught an exception-handler soundness gap (handler `stack_depth` seeded from the file without validation); closed by requiring every reachable pc in a protected range to hold ≥ `stack_depth − n_locals` operands. Regression tests in `crates/sema-vm/tests/bytecode_validator_regression.rs`. **§2.1/§2.2 are no longer open** — update those sections' status accordingly.
> - **DAP-6 FIXED** — `local_scopes` now serialized (bytecode format v3→**v4**, spec + serializer in lock-step), so `.semac`-loaded functions get correct pc-scoped locals.
> - **DAP-7 FIXED** — `as_local_set` only short-circuits to write-back for the genuine builtin `set!` on an in-scope target; falls through to normal eval otherwise.
> - **MCP-5 FIXED** — deftool arg mapping fully validates against the declared schema: missing-required → error (names field), strict type coercion (incl. `:number` preserving int/float kind, `:int` rejecting non-integral), rest/variadic collection, absent-vs-null distinction. Tests in `crates/sema-mcp/tests/`.
> - **MCP-4 FIXED** — bounded LRU notebook cache (cap 16) + out-of-band mtime reload + full-path canonicalization (symlinked leaf collapses to one key).
> - **MCP-6 FIXED** — server-loop response serialization no longer `unwrap()`s; falls back to a -32603 frame.
> - **DAP-5 FIXED** — resume commands only sent while `vm_suspended`.
> - **DAP-9 FIXED** — `decode_percent` decodes into bytes then UTF-8 (multi-byte `file://` paths correct).
> - **§7.4 #4 FIXED (warning)** — load/import under a debug session emits a one-time warning that breakpoints in dynamically loaded files aren't hit (documented limitation; no risky module-loader rearchitecture).
> - **§7.4 #5** — left single-thread by design (synchronous VM); documented.
>
> No open robustness/correctness items from this review remain. Low nits noted by verification (i64-boundary rounding in int coercion, rest-element type not enforced, mtime granularity) are acceptable and documented inline.

The original triage follows.

### 8.1 Release blockers / High

- **MCP-1 — No `catch_unwind` around tool dispatch.** `handle_request` → `call_mcp_tool` runs synchronously with no unwind guard (`crates/sema-mcp/src/server.rs`, `crates/sema-mcp/src/tools.rs`). A panic anywhere in eval / VM execution / `run_bytecode_bytes` (crafted `.semac`) / fmt unwinds out and **terminates the whole MCP session**. Logical errors are correctly returned as `isError` results — only panics are fatal. *Fix:* wrap each dispatch in `std::panic::catch_unwind` and convert a caught panic into an `isError` result. Highest-leverage single fix.
- **MCP-2 — `gag` fd-redirect can corrupt the JSON-RPC stream.** `eval_with_capture` (`tools.rs`) redirects process-global fd 1 via `gag`, but the JSON-RPC responses are also written to fd 1. (a) If `BufferRedirect::stdout()` fails (second redirect already active, temp-file/fd exhaustion) the error is swallowed and user `(print …)` output goes straight into the protocol stream, breaking line-delimited JSON on the client. (b) Nested capture (a `deftool` that triggers a notebook eval) hits gag's single-redirect guard → inner output escapes. *Fix:* treat a failed redirect as fatal-to-the-call (don't run user code against the live protocol fd); serialize captures with a mutex; long-term route Sema `print` through a thread-local sink instead of OS stdout so fd 1 is never shared.
- **MCP-3 — `Sandbox::allow_all()` is the MCP default.** The interpreter is built once with `allow_all()` (`crates/sema/src/main.rs` ~L631) and shared across all calls; with the no-op `sandbox` param removed, MCP `eval`/`run_file`/`build` execute arbitrary host code (FS/shell/network/subprocess) driven by an LLM client — a prompt-injection-to-RCE primitive. *Fix:* default-deny + explicit opt-in flag (`sema mcp --allow fs,net,shell` or `--allow-all`); the plumbing already exists (`Interpreter::new_with_sandbox`). Correct the `info` tool's misleading `"Environment Context: standard"` string, and consider shipping only read-only tools by default.
- **DAP-1 — `DEBUG_REPLY_TIMEOUT` returns silently-wrong data for a live-but-busy VM.** The 2s timeout (`crates/sema-dap/src/server.rs`) collapses three states into one empty result: terminated (intended), alive-but-mid-blocking-native-call, and alive-but->2s-of-bytecode. The running-mode command poll only drains `command_rx` every 128 VM instructions (`vm.rs`), and a blocking native (`http/get`, LLM, sleep) is a single instruction — so a `stackTrace`/`scopes`/`variables` during it times out and the IDE shows an **empty stack/variables for a running program**. *Fix:* have the backend drain `command_rx` and reply with a definitive "session ended" error after `execute_debug` returns (removes the need for a guessed timeout), and gate the query handlers on `vm_suspended` (proven by a received `Stopped` event) rather than `vm_active`. Never fabricate empty data for a live VM.
- **VM-1 — §2.1/§2.2 still open (`pop_unchecked` + missing stack verifier).** A crafted `.semac` with an unbalanced operand stack passes all current `validate_bytecode` checks and reaches `pop_unchecked` / the fused-comparison stack peeks (`vm.rs`), producing `set_len(usize::MAX)` + OOB `ptr::read` — arbitrary memory corruption in release. Currently mitigated only by the *trust-model* (`.semac` is trusted-source-only; ADR #56 verifier proposed). *Decision needed:* if the MCP `build`/`run_file` flow or any "run downloaded program" story is in scope for this release, land either the abstract-interpretation stack-depth verifier (ADR #56) or the cheap interim release-mode bounds check in `pop_unchecked`. If `.semac` stays strictly trusted-source-only, this can remain documented-and-deferred — but MCP now makes "run a file an agent produced" much more reachable, so revisit the trust assumption.

### 8.2 Medium

- **DAP-2 — `vm_active` activation race.** Set `true` at `configurationDone` before the backend has entered `execute_debug` (`server.rs`); a fast inspection request can route to a VM not yet polling → timeout/empty. Subsumed by the DAP-1 "gate on `vm_suspended`" fix.
- **DAP-3 — `dbg_cmd_tx` desync on terminate/launch-error.** On `DebugEvent::Terminated` the frontend resets `vm_active`/`vm_suspended` but leaves `dbg_cmd_tx = Some(...)`; on a launch/compile/`VM::new` failure the backend emits `Terminated` without ever building a `DebugState`, so a later `configurationDone` re-flips `vm_active=true` and requests route to a dead receiver. *Fix:* reset `dbg_cmd_tx = None` on `Terminated`; have the backend reply to `ConfigurationDone` with a launched/failed status the frontend keys off.
- **DAP-4 — `set!`/`setVariable` resolve a shadowed local to the *first* name-matching slot, not the pc-active one.** `debug_locals` correctly pc-filters which binding is *displayed*, but `frame_has_binding`/`debug_set_local`/`debug_env_for_frame` (`vm.rs`) use `.find()`/`.any()`/insert-all on the name, so for shadowing `let`s the displayed value and the written value can refer to different slots. This is a correctness bug in the just-shipped evaluate/setVariable path. *Fix:* thread the same `local_scopes` pc predicate through the write/eval paths so all three resolve to the innermost in-scope slot.
- **DAP-5 — `Continue`/step dropped while running.** The frontend sends resume commands whenever `vm_active` is true regardless of stop state; the running-mode poll's catch-all silently drops them (`vm.rs`), so the frontend can believe it resumed when the VM never saw a state change. *Fix:* only send resume commands when `vm_suspended`; treat the flag as advisory.
- **MCP-4 — `NotebookCache` is unbounded and never evicts.** Process-lifetime `BTreeMap` of `Rc<RefCell<Engine>>` (`notebook.rs`); pointing an agent at many notebooks grows memory monotonically, and a cached engine serves a stale in-memory copy if the `.sema-nb` is edited out-of-band (then clobbers it on save). The d2a5e6e canonicalization fix closed the symlinked-*parent* divergence but a **symlinked leaf filename** still produces two keys for one file. *Fix:* LRU/TTL cap; re-stat on access; resolve the leaf symlink for the key when the target exists.
- **MCP-5 — `deftool` arg mapping is silent and lossy.** `json_args_to_sema` (`tools.rs`) passes missing args as `nil` (no arity error), never coerces/validates declared schema types, **ignores `has_rest`** (variadic handlers get one positional nil instead of a collected list), and silently drops extra args. Doesn't crash, but fails confusingly deep in Sema. *Fix:* validate against the schema before dispatch (reject missing `required`, coerce types, collect rest args, distinguish absent-vs-nil).

### 8.3 Low / latent

- **DAP-6 — `.semac`-loaded functions show all locals.** `local_scopes` is not serialized (hardcoded empty on deserialize), so the pc-filter falls back to "show everything" for precompiled programs, including not-yet-initialized/exited block locals. *Fix:* serialize `local_scopes` (cheap `Vec<(u16,u32,u32)>`; requires the usual format-spec + serializer update), or mark such locals "may be out of scope".
- **DAP-7 — `as_local_set` matches `set!` by head symbol only**; if `set!` is rebound or the target is meant as a global, the write-back redirects to the frame local, diverging from a plain `eval`. Acceptable for the normal flow; document the precedence.
- **DAP-8 — running-mode inspection is a best-effort snapshot** off a live, mutating stack with no consistent source location. Inherent to inspecting a non-stopped VM; most clients only inspect while stopped.
- **DAP-9 — `decode_percent` decodes multi-byte UTF-8 percent-encoding incorrectly** (pushes raw bytes as `char`), affecting non-ASCII `file://` paths. ASCII paths are fine.
- **MCP-6 — latent panics on the hot path:** `serde_json::to_string(&resp).unwrap()` in the loop, and the `sys/args` save/restore in `run_file` is not panic-safe (a panic mid-eval leaks the previous call's args into the shared global). Both are subsumed once MCP-1's `catch_unwind` lands, but the `sys/args` restore should use a `Drop` guard regardless.

### 8.4 Confirmed safe (no action)

- v3 `upvalue_names` / `local_names` deserialization: counts are `u16` (bounded ~64K, no OOM via `with_capacity`), every spur remap index is bounds-checked, and an `upvalue_names.len() == upvalue_descs.len()` consistency check exists. No new panic/OOM/UB surface.
- `Function.local_scopes`: never serialized → not attacker-influenceable.
- 1.16.0 point-fixes (`DUP`-on-empty clean error; `CallNative` native-id runtime bounds check) verified present and correct. `CallNative` `argc` underflow degrades to a clean slice-bounds **panic** (not UB) — subsumed by the VM-1 verifier.

### 8.5 Recommended stabilization order

1. **MCP-1** (`catch_unwind`) — small, removes the most likely crash; also de-risks MCP-6.
2. **MCP-2** (gag protocol-corruption) — correctness of the stdio transport itself.
3. **DAP-1 + DAP-2 + DAP-3** together — replace the timeout band-aid with backend-drains-on-exit + gate-on-`vm_suspended`; fixes the silently-wrong debugger output and the activation/desync races in one change.
4. **MCP-3** (sandbox default) — security posture decision; needs a product call on the default.
5. **DAP-4** (shadowed-local write path) — correctness bug in shipped evaluate/setVariable.
6. **VM-1** decision — confirm whether `.semac` stays trusted-source-only given MCP, or land the verifier / interim bounds check.
7. Remaining medium/low items (MCP-4, MCP-5, DAP-5..9) as capacity allows.
