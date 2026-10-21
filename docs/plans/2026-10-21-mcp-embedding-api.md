# MCP access through the public embedding API

**Status:** Revised draft after a five-role adversarial review (embedder, CLI
and MCP-client user, security, runtime safety, performance). Decisions D1-D12
are proposed, not approved. No API in this document is implemented.

## Problem and current behavior

[Kameko's embedding feedback](https://github.com/sema-lisp/sema/pull/172#issuecomment-6303419534)
reports: "my biggest pain point so far is not having MCP functionality by
embedding in the 'sanctioned' way, I have to use internal crates like
`sema-eval` just to accomplish that." Only the server helpers require
`sema-eval`:

- **Client:** `InterpreterBuilder` enables `mcp/connect`, `mcp/tools`,
  `mcp/call`, and `mcp/close` by default; `with_mcp(false)` disables them. An
  embedder can use them from Sema today. The embedding guide does not mention
  them.
- **Server:** `sema_mcp::tools::list_mcp_tools`, `call_mcp_tool`, and the
  server loops take `sema_eval::Interpreter`. The public `sema::Interpreter`
  owns that type in a private field, so this fails to type-check:

```rust,ignore
let interp = sema::Interpreter::new();
sema_mcp::tools::list_mcp_tools(&interp, None, None);
// Expected sema_eval::Interpreter; received sema::Interpreter.
```

### How tool enablement works today

There is one mechanism: a hardcoded default tool list in `list_mcp_tools`
(`crates/sema-mcp/src/tools.rs`) filtered by name through `is_tool_allowed`
(include = exclusive allowlist, exclude = denylist), fed from `sema mcp
--include` / `--exclude`. `eval` is in the default list, so the CLI exposes it
unless `--exclude eval` is passed. The sandbox (`Sandbox { denied: Caps,
allowed_paths }`, parsed from `--sandbox strict | all | no-shell,...`) gates
what scripts do *inside* `eval` and tool handlers; it does not decide which
tools exist. `sema-policy` governs LLM tool calls in agent loops and is not
involved in the MCP server. The design below keeps the name filter as the only
enablement mechanism and adds presets on top of it (D1).

### Corrections to the first draft

- `mcp/connect` from scripts uses the sandbox captured at registration
  (`builtins.rs`, `register_mcp_builtins`). The thread-local `HOST_SANDBOX`
  feeds only `connect_from_config`, `host_capability_allowed`, and
  `browser_open_allowed`, which the workflow `:mcp` resolver and OAuth
  browser-open use. The limitation to document is therefore: two public
  interpreters on one thread share the resolver's sandbox, last built wins;
  `with_mcp(false)` leaves a previous interpreter's value in place.
- `run_file` runs in the session interpreter. The tools that build a fresh
  allow-all interpreter are `compile` and `disasm`, and the notebook engine
  (`sema-notebook/src/engine.rs`, `Engine::new`), which evaluates cells in an
  unsandboxed interpreter. `fmt` (in-place write), `disasm` (read), and
  `notebook/export` (write) also touch the filesystem outside the sandbox.
- `include_tools` as drafted was contradictory: an exclusive allowlist cannot
  also "re-enable" tools on top of a preset. Resolved by D1.

## Decision status

| ID | Decision | Proposed design | Other choices and effect |
|---|---|---|---|
| D1 | Tool presets and `eval` | Two presets over the existing name filter. `cli()` = today's list, `eval` on, same `--include`/`--exclude` behavior. `embedded()` = `deftool` and host-registered tools plus `docs`, `docs_search`, `info`; no `eval`. `enable_eval()` adds `eval`; `only_tools([..])` is the allowlist, `exclude_tools([..])` the denylist, `also_tools([..])` adds named CLI tools. `sema mcp --exclude eval` stays the CLI way to turn it off. | Refusing `enable_eval()` on an unrestricted sandbox unless `allow_unrestricted_eval()` is also called: safer default for hosts, one more call for the common local case. Not proposed; the guide shows a sandbox and `info` reports it. |
| D2 | Sandbox coverage | Tool bodies check `Caps` and `check_path` on the canonical path at every read, write, and spawn (`run_file`, `compile` output, `fmt` in place, `disasm`, `notebook/read`, `/save`, `/export`). `compile` and `disasm` use the session interpreter. The notebook engine takes the session sandbox (`Engine::new_with_sandbox`); until it does, `notebook/*` is excluded whenever the sandbox is restricted. `info` reports the real sandbox. | Leaving tool bodies unsandboxed keeps `sema mcp --sandbox` dishonest; a notebook cell can `(shell ...)` under `strict` today. |
| D3 | Dispatch path | The whole `call_tool` body runs as one runtime root: macro expansion, `eval`, and `deftool` handlers. `deftool` handlers are seeded as a VM frame around the closure (no parse/expand/compile), so `register_async_fn`, `async/*`, `llm/*`, `mcp/*` work inside handlers and nothing reaches `io_block_on` on the caller thread. | Routing only the handler leaves macro expansion of `eval` code and `mcp/close` on the blocking path. Routing through the `eval_str` path imports a per-call span-table clone that grows with session age. |
| D4 | Ambient Tokio runtime | No opt-out. `sema::mcp::serve_on_thread(builder, setup, reader, writer)` spawns the dedicated `std::thread` and owns the interpreter; the message-level API lets an async host drive a session over channels. A dispatch-time runtime check stays as defence in depth and returns `McpError::AmbientRuntime`. | `allow_ambient_runtime(true)` cannot be made safe: `Handle::try_current()` succeeds inside `spawn_blocking` and `block_in_place` where `block_on` is legal, and the thing that aborts (being inside `rt.block_on`) has no public probe. |
| D5 | Error types | `McpError { InvalidOptions, Transport(io::Error), AmbientRuntime, Reentrant }` for the session; `McpCallError { UnknownTool, NotAllowed, InvalidArguments, Timeout, Panicked, Reentrant }` for dispatch. Both `#[non_exhaustive]`. A handler's own error stays inside `CallToolResult { is_error: true }`. | One string-typed `is_error` for everything forces hosts to string-match dispatch failures. |
| D6 | Rust client API | Deferred. Confirm with Kameko whether the Sema builtin path suffices. No `HOST_SANDBOX` rewrite is needed for it; a typed client captures the sandbox like the natives do. | Building it now widens the surface before the need is confirmed. |
| D7 | `build` tool | Removed from `sema-mcp`. `cli()` installs an in-process build hook that the CLI implements with `build::run_build`. No re-exec. | A `current_exe()` name guard breaks the mcpb shims (`sema-linux`, `sema-macos-universal`) and symlinked installs; the re-exec also has argument injection (a source path starting with `-` is parsed as a `sema build` flag). |
| D8 | Session shape | `McpSession` is an owned value with no interpreter lifetime: options plus the notebook cache. Methods take `&Interpreter`. `handle_message(&Interpreter, &str) -> Option<String>` is the primitive; `serve_on` is a loop over it. The async `run_mcp_server_on` is removed. | `McpServer<'interp>` cannot live in a struct beside its interpreter. A loop that owns the reader forecloses pipelining and cancellation. The async loop borrows a `!Send` interpreter inside `block_on`, which is where `io_block_on` aborts. |
| D9 | Timeouts and cancellation | The per-call deadline arms a watchdog on the I/O pool that calls `cancel_root`, so a root parked on an External wait is torn down (its cancel hook drops the future). `block_on_inbox` receives `min(timer, deadline)`. A per-call `timeout_ms` may only shorten the session default. `notifications/cancelled` maps to `cancel_root` for hosts driving `handle_message`. | Today the deadline is a VM back-edge poll: it never wakes a parked wait, never fires inside blocking I/O, and a client disables it with `timeout_ms: 0`. |
| D10 | Output capture | A `HostHookScope` RAII guard saves and restores the thread-local hooks. `eval` and handler roots use root-tagged capture, so output is keyed by root, not thread. `ToolOutput::{Capture, PassThrough, Tee}` and `on_output(stream, &str)` let the host forward output. `max_output_bytes` (default 1 MiB) and `max_frame_bytes` (default 16 MiB) are options. | Thread-keyed capture collects output from other interpreters on the thread and from detached tasks of earlier calls; unbounded buffers are a memory DoS. |
| D11 | Re-entrancy | A session-level `in_call` fence returns `Reentrant` when a host function invoked from a tool calls back into the session. `Interpreter::eval_str` rejects when a runtime quantum is active *before* submitting a root. | Today a nested `eval_str` inserts an orphan root that runs during an unrelated later tool call, into that call's capture buffer. |
| D12 | CLI and embedded parity | `InterpreterBuilder::with_llm_auto_configure(bool)` and `with_workflow_mcp(bool)` (default off, as today). `sema mcp --preset embedded` and `sema mcp --check tools.sema` show tool-file authors what an embedded host sees. One tool registry drives listing, dispatch, and presets. | Without flags, a tool file validated under `sema mcp` behaves differently embedded with no API to fix it; three hand-maintained lists drift. |

## Proposed server API

```rust,ignore
use sema::{mcp::{McpServerOptions, McpSession}, Interpreter, Sandbox, Caps};
use serde_json::json;

let interp = Interpreter::builder()
    .with_sandbox(Sandbox::deny(Caps::SHELL | Caps::PROCESS | Caps::FS_WRITE))
    .build();
interp.eval_str(r#"
  (deftool greet "Greet someone by name."
    {:name {:type :string :description "Who to greet."}}
    (lambda (name) (string-append "hello, " name)))
"#)?;

let session = McpSession::new(
    McpServerOptions::embedded()
        .server_info("my-host", env!("CARGO_PKG_VERSION"))
        .enable_eval(),
)?;
let tools = session.list_tools(&interp);
let result = session.call_tool(&interp, "greet", &json!({"name": "Kameko"}))?;
assert!(!result.is_error);
```

`McpSession` provides:

| Method | Contract |
| --- | --- |
| `list_tools(&self, &Interpreter) -> Vec<Tool>` | Selected builtins and exposed `deftool` definitions. One `is_exposed` predicate is shared with dispatch, so a hidden tool (`_` prefix, `:private`, `:mcp/expose false`) is neither listed nor callable. A `deftool` named like a builtin is rejected at definition time. |
| `call_tool(&self, &Interpreter, name, &serde_json::Value) -> Result<CallToolResult, McpCallError>` | Dispatch with the selection, sandbox, timeout, and output rules above. Map and sequence results also populate `structured_content`. |
| `handle_message(&self, &Interpreter, &str) -> Option<String>` | One JSON-RPC frame in, zero or one out. The host owns transport and framing. |
| `serve_on(&self, &Interpreter, impl BufRead, impl Write) -> Result<(), McpError>` | Blocking loop over `handle_message` until EOF or a transport error. Serial. |
| `serve_stdio(&self, &Interpreter)` | `serve_on` over locked stdio. Documented as "only when your process is the MCP server": host output on stdout corrupts the frame stream. |
| `cancel_handle(&self) -> McpCancelHandle` | `Send + Sync`; cancels the in-flight call's root from another thread. |

`sema::mcp::serve_on_thread(builder, setup, reader, writer)` is the sanctioned
pattern for async hosts: it spawns a plain `std::thread`, builds the interpreter
there, runs `setup(&interp)` (register host functions, load tool files), and
serves. Hosts that need per-request control run the same thread themselves and
exchange owned `String` frames over channels with `handle_message`.

Re-export `Tool`, `CallToolResult`, `ToolContent`, and `McpCancelHandle`
through `sema::mcp`, all `#[non_exhaustive]`, with constructors
(`Tool::new`, `CallToolResult::text`, `::error`, `::structured`) so hosts can
merge their own tools and results. Callers need only `sema-lang` and
`serde_json`. The session emits nothing on stderr; `on_log` receives a typed
`LogEvent` (`Started`, `Stopped`, `WriteFailed`), never request content.
`server_info` overrides the `initialize` response; the default stays.

## Ownership and execution

The session stays on its creating thread with the interpreter (`!Send`, #175).
It does not borrow the interpreter, so a host can hold both in one struct.
Inside a tool call the whole body is one runtime root (D3); `call_tool` and
`handle_message` are not re-entrant (D11). `catch_unwind` around dispatch is a
convenience for unwind hosts only; under `panic = "abort"` (the workspace
release profile) a panic in a tool body aborts the process, and the public docs
say so. Untrusted tool files should run out of process (`sema mcp --sandbox`
as a child) rather than in the host.

### Connections

MCP client connections are a thread-local registry keyed by handle string;
nothing closes them when an interpreter is dropped, and any interpreter on the
thread can use another's handle. The guide documents this and `(mcp/close)`.
Interpreter-owned connections closed from the teardown-hook seam are a
follow-up (medium cost).

### Performance notes

Session creation is free; interpreter construction dominates. The docs index is
process-wide and lazy. Per-call costs to remove in the first implementation:
`params.clone()` on the request, `tools/list` rebuilding every schema, the
`b.clone()` of the capture buffer, `json!` → `to_string` → `format!` on the
response, and the notebook cache allocated for presets that exclude notebook
tools. Fixing the span-table clone in `build_vm_for_exprs` lands before D3's
routing, in its own PR. A reader-thread split that answers interpreter-free
methods (`ping`, `docs`, `docs_search`, static `tools/list`) during a long call
is enabled by D8 and deferred.

## Implementation outline

1. Tool registry: one table of (name, schema, required `Caps`, dispatch fn)
   drives `list_mcp_tools`, dispatch, and the presets; snapshot test pins
   `cli()` to today's list. Shared `is_exposed` predicate (D1, D12).
2. Sandbox in tool bodies, session-interpreter `compile`/`disasm`, notebook
   engine with session sandbox, honest `info`; remove `build` from `sema-mcp`
   and add the CLI hook (D2, D7).
3. Perf prerequisite PR: span-table by reference in `build_vm_for_exprs`;
   `Interpreter::submit_call(handler, args)` seeding a VM frame.
4. Whole-call runtime routing with the `block_on_ops()` CI oracle (D3).
5. Deadline watchdog, `cancel_root` mapping, shortening-only `timeout_ms`
   (D9). Hook scope, root-tagged capture, output policy and caps (D10).
   Re-entrancy fence and the quantum check before `submit_root` (D11).
6. Typed errors, constructors, `structured_content`, `#[non_exhaustive]` (D5).
7. Public `McpSession`, `handle_message`, `serve_on`, `serve_stdio`,
   `serve_on_thread`, `cancel_handle` in `crates/sema/src/`; remove the async
   loop; `Interpreter::inner` stays private (D4, D8).
8. Builder parity flags, `sema mcp --preset` and `--check`, the CLI-vs-embedded
   matrix, client builtins and connection semantics in
   `website/docs/embedding.md` (D12).

## Acceptance checks

- A fixture depending only on `sema-lang` and `serde_json` creates an
  interpreter, registers a `register_fn` and a `register_async_fn` host
  function, defines a `deftool` that calls each, and invokes both through the
  session. The async one succeeds (D3).
- `embedded()` lists exactly `docs`, `docs_search`, `info`, and exposed
  `deftool`s; `enable_eval()` adds `eval`; `cli()` matches a snapshot of what
  `sema mcp` lists today; `--include`/`--exclude` behave as before.
- A hidden `deftool` is neither listed nor callable; a `deftool` named `eval`
  is rejected at definition.
- With `FS_READ` denied, `run_file`, `compile`, `disasm`, `notebook/read`
  return `is_error` without reading; with `FS_WRITE` denied, `compile` output,
  `fmt` in place, `notebook/save` and `/export` return `is_error` without
  writing; a notebook cell `(shell ...)` under `strict` is denied.
- `compile` through the session sees a macro defined in that interpreter and
  does not leak the compiled file's macros into later calls.
- `block_on_ops()` is unchanged across `eval`, `deftool`, macro-expanding
  `eval`, and `mcp/close` tool calls run on a Tokio worker; each returns a
  result or `AmbientRuntime`, never a panic.
- An `eval` parked on a never-resolving host future with `timeout_ms: 100`
  returns `Timeout` within the budget and the host future is dropped (drop
  guard observed). `timeout_ms: 0` and values above the session default are
  clamped.
- A host-installed stdout hook is still installed after `call_tool("eval")`;
  output from a detached task of an earlier call does not appear in a later
  result; a 2 MiB print is truncated at `max_output_bytes` with a marker.
- A host function that calls `call_tool` from inside a tool receives
  `Reentrant`; `eval_str` from inside a tool returns an error and leaves no
  orphan root (`runtime_live_task_count` is 0 afterwards).
- In-memory protocol tests over `handle_message` cover initialize, list, call,
  malformed JSON, oversized frame (`-32600`), notifications, EOF, and transport
  errors, and assert nothing is written to stderr.
- `server_info` changes `initialize`; the default is unchanged.
- An embedded interpreter can `mcp/connect` to an in-process `serve_on`
  fixture, call a tool, and `mcp/close` it (client path).
- The MCP suite runs once under `-C panic=abort` so every "converted to
  `is_error`" claim is proven live or documented as unwind-only.
- Existing CLI protocol tests pass without changing defaults.

## Follow-ups (not in this change)

- `Interpreter::register_tool` / `register_async_tool` from Rust without Sema
  glue (medium).
- Interpreter-owned MCP connections closed on drop (medium).
- Reader-thread split for interpreter-free methods (medium, needs D8).
- Host-supplied Tokio handle for the I/O pool via a sanctioned `sema-io`
  wrapper (medium to large).
- `mcp-server` / `mcp-client` Cargo features on `sema-lang` (medium).
- Docs index memory: borrow the static corpus instead of cloning it three
  times (small).
- Typed Rust MCP client (D6).

## Code references

- Public wrapper and builder: `crates/sema/src/lib.rs`.
- Server loops and dispatch: `crates/sema-mcp/src/server.rs`, `tools.rs`
  (`is_tool_allowed`, `list_mcp_tools`, `call_mcp_tool`, the `compile`,
  `disasm`, `fmt`, `run_file`, `build`, `deftool` arms, `eval_with_capture`).
- Protocol types: `crates/sema-mcp/src/protocol.rs`.
- Client registration and `HOST_SANDBOX`: `crates/sema-mcp/src/builtins.rs`.
- Notebook engine: `crates/sema-notebook/src/engine.rs`; cache:
  `crates/sema-mcp/src/notebook.rs`.
- Output hooks: `crates/sema-core/src/output_hook.rs`; deadline poll:
  `crates/sema-core/src/context.rs`; `cancel_root`:
  `crates/sema-vm/src/runtime/host_api.rs`.
- I/O seam: `crates/sema-core/src/io_backend.rs`, `crates/sema-io/src/lib.rs`.
- CLI entry points: `crates/sema/src/main.rs` (`run_mcp`, `build_interpreter`,
  `resolve_mcp_tool_timeout`), `crates/sema/src/build.rs`.
- Async registration (shipped): `docs/plans/archive/2026-10-21-async-native-registration.md`.
