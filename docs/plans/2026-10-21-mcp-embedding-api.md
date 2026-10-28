# MCP access through the public embedding API

**Status:** Fourth draft after two five-role adversarial review rounds
(embedder, CLI and MCP-client user, security, runtime safety, performance) and
a reuse audit against the runtime host API and the notebook engine. Decisions
D1-D13 are proposed, not approved. No API in this document is implemented.

## Problem and current behavior

[Kameko's embedding feedback](https://github.com/sema-lisp/sema/pull/172#issuecomment-6303419534)
reports: "my biggest pain point so far is not having MCP functionality by
embedding in the 'sanctioned' way, I have to use internal crates like
`sema-eval` just to accomplish that." Only the server helpers require
`sema-eval`:

- **Client:** `InterpreterBuilder` enables `mcp/connect`, `mcp/tools`,
  `mcp/tools->sema`, `mcp/call`, and `mcp/close` by default; `with_mcp(false)`
  disables them. An embedder can use them from Sema today. The embedding guide
  does not mention them.
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
(include = exclusive allowlist that also applies to `deftool`s, exclude =
denylist), fed from `sema mcp --include` / `--exclude`. `eval` is in the
default list, so the CLI exposes it unless `--exclude eval` is passed. The
sandbox (`Sandbox { denied: Caps, allowed_paths }`, parsed from `--sandbox
strict | all | no-shell,...`) gates what scripts do *inside* `eval` and tool
handlers; it does not decide which tools exist. `sema-policy` governs LLM tool
calls in agent loops and is not involved in the MCP server. The design keeps
the name filter as the only enablement mechanism and adds presets on top (D1).

### Corrections to the first draft

- `mcp/connect` from scripts uses the sandbox captured at registration. The
  thread-local `HOST_SANDBOX` feeds only `connect_from_config`,
  `host_capability_allowed`, and `browser_open_allowed`, used by the workflow
  `:mcp` resolver and OAuth browser-open. The limitation to document: two
  public interpreters on one thread share the resolver's sandbox, last built
  wins; `with_mcp(false)` leaves a previous interpreter's value in place.
- `run_file` runs in the session interpreter. `compile` and `disasm` build a
  fresh allow-all interpreter. The notebook engine (`Engine::new`) builds an
  unsandboxed `Interpreter::new()` for cells and calls `reset_runtime_state`,
  which wipes the session's thread-local LLM providers, usage and budget.
  `fmt` writes in place, `disasm` reads, `notebook/export` writes, all
  outside the sandbox.
- `include_tools` as drafted was contradictory. Resolved by D1.
- `build` inside a `sema build --mcp` binary is broken today: the re-exec
  runs the embedded script, not the compiler. D7 covers it.

## Reuse of existing runtime and host primitives

The notebook engine (`crates/sema-notebook/src/engine.rs`, `bridge.rs`) is
already the reference host consumer of the unified runtime. The MCP session is
built from the same primitives; it introduces no new scheduler concept.

| Need | Existing primitive | Action |
|---|---|---|
| Run a tool call as a root | `Interpreter::submit_value` / `submit_str` + `drive_until_settled` | Reuse. |
| Capture a call's output, including output from detached tasks it started | `RootOptions { capture_output: true }` + `take_output`; the engine's `output_roots` attribution map | Reuse; add a per-root byte budget inside the existing capture sink. |
| Cancel the in-flight call from another thread | notebook `CancelToken` (`Interpreter::command_handle()` + a shared `running_root`) | Lift `CancelToken` from `sema-notebook` into `sema-eval` as the shared host type; `CallId` is the root's `RootId`. |
| Per-call deadline | `ctx.set_eval_deadline`, polled at VM back-edges | Reuse; fix the shared gap below once in `drive_until_settled`. |
| Async host bridge | `sema_notebook::bridge::EngineHandle::spawn` (dedicated 16 MiB thread, `tokio::mpsc` requests, `oneshot` replies, cancel token returned); `sema-dap` uses the same shape with `sync_channel` + `spawn_blocking` | Generalize the notebook bridge into a shared worker in `sema-eval` or `sema`; `McpWorker` is a request enum over it, and the notebook bridge becomes a second user. |
| Keep one call from running earlier calls' detached work | `Interpreter::drive_roots(&[root])` | Reuse. |
| Reject nested evaluation inside a quantum | the predicate in `ensure_legacy_vm_entry_allowed` | Reuse the predicate before `submit_root`. |
| Macro expansion inside the root | the `__vm-eval` runtime path | Reuse. |
| Frame reading | the existing `read_until` loop in `server.rs` | Extend with `Read::take` and drain-to-newline. |

**Shared gap found by the audit.** The native drive loop parks on
`block_on_inbox` bounded only by the runtime's timer deadline, never by
`eval_deadline` (`crates/sema-eval/src/eval.rs`, the `DriveState::Idle` arms).
A notebook cell or MCP tool parked on an `llm/*` External wait therefore
ignores its timeout until the request completes. The fix lives once in
`drive_until_settled`: bound the park by the earlier of the timer and the eval
deadline, and cancel the root when the eval deadline passes, as the wasm arm
already does. Both the notebook and the MCP server get it.

Genuinely new and justified: the tool registry (it replaces three
hand-maintained lists), presets over the existing name filter, typed errors,
the frame cap, and the byte budget.

## Decision status

| ID | Decision | Proposed design | Other choices and effect |
|---|---|---|---|
| D1 | Tool presets and `eval` | Two presets over the existing name filter. `cli()` = today's list with `eval` on and today's `--include`/`--exclude` behavior. `embedded()` = exposed `deftool`s plus `docs`, `docs_search`, `info`; no `eval`. `enable_eval()` is the only way to add `eval`: `also_tools(["eval"])` or `only_tools(["eval"])` without it is `InvalidOptions`. Precedence: `exclude_tools` beats everything; `only_tools` intersects the enabled set and applies to `deftool`s as today; hidden `deftool`s beat any include; unknown names are `InvalidOptions`. With `eval` on, the filter is not a security boundary: a client can redefine tools and call any host-registered function; the guide says so. | A marker argument (`enable_eval(EvalAccess::FullHostAccess)`) or refusing `eval` on an unrestricted sandbox: more friction for the local case; the guide's example sandbox and an honest `info` are the chosen mitigation. |
| D2 | Sandbox coverage | Tool bodies check `Caps` and `check_path` on the canonical path at every read, write and spawn, with `is_file()` and a `max_source_bytes` cap before reading (`run_file`, `compile` output, `fmt` in place, `disasm`, `notebook/read`, `/save`, `/export`). `compile` and `disasm` use the session interpreter. The notebook engine is built from the session's builder config (`Engine::from_session`: sandbox, stdlib/LLM/MCP flags, session deadline) and does not reset LLM state. Until that lands, `notebook/eval_cell` and `/eval_all` are absent from `embedded()`, and requesting them while the sandbox is restricted is `InvalidOptions`; the other notebook tools stay. `docs` uses the shared `is_exposed` predicate. `info` reports the real sandbox. | Excluding all `notebook/*` whenever the sandbox is restricted removes read-only tools under the documented `--sandbox no-shell,no-fs-write`. |
| D3 | Dispatch path | `eval`, `run_file`, `deftool` handlers and `mcp/*` calls run as one runtime root each, submitted with `submit_value` on the form `((quote handler) (quote arg) ...)`, so closures and natives dispatch uniformly and `register_async_fn`, `async/*`, `llm/*` work inside handlers. Macro expansion of `eval` code runs inside the root through the existing `__vm-eval` runtime path. `notebook/*`, `compile`, `disasm`, `fmt`, `docs*` and `info` stay on the host path: constructing an interpreter inside a quantum panics in prelude loading. A conformance test requires every `io_block_on` call site in a native to be dominated by an `in_runtime_quantum()` branch, since moving handlers into a quantum turns an unguarded site from "works" into an abort. | Seeding a VM frame needs new `sema-vm` API and does not cover native handlers. Routing everything through roots aborts on notebook and compile. |
| D4 | Async hosts | `McpWorker::spawn(builder, options, setup) -> McpWorkerHandle` is the notebook bridge generalized: the interpreter lives on a dedicated `std::thread` (same stack size), requests are a `tokio::mpsc` enum with `oneshot` replies, and the cancel token is the lifted notebook `CancelToken`. The handle is `Send + Sync + Clone` with `call_tool`, `list_tools`, `handle_message`, `cancel(CallId)`, `shutdown`. `serve_stdio` and `serve_on` are adapters over it. No ambient-runtime opt-out and no dispatch-time `Handle::try_current()` check: it is a false positive inside `spawn_blocking` and `block_in_place`, and with D3 nothing reaches `io_block_on` on the interpreter thread. The `block_on_ops()` oracle and the D3 conformance test are the guarantee. | `serve_on_thread(builder, setup, reader, writer)` forces an rmcp or axum host to pipe JSON-RPC to itself. `allow_ambient_runtime` cannot be made safe. |
| D5 | Error types | `McpError { InvalidOptions, Transport(io::Error), Reentrant }` for sessions; `McpCallError { UnknownTool, NotAllowed, InvalidArguments, Timeout, Panicked, Reentrant }` for `call_tool`. Both `#[non_exhaustive]`. On the wire every `McpCallError` stays an `is_error` result (today's behavior, pinned by CLI tests); `From<McpCallError> for CallToolResult` does the mapping. | Mapping dispatch failures to JSON-RPC error codes breaks existing CLI protocol tests and clients. |
| D6 | Rust client API | Deferred. Confirm with Kameko whether the Sema builtin path suffices. No `HOST_SANDBOX` rewrite is needed. | Widening the surface before the need is confirmed. |
| D7 | `build` tool | Removed from `sema-mcp`. `cli().with_build_hook(f)` injects an in-process implementation from the CLI binary; `build` is listed only when a hook is installed. The hook compiles through the session interpreter, gates `FS_READ`/`FS_WRITE`/`NETWORK` with `check_path` on source, includes and output, returns text and never prints. Built `--mcp` binaries install no hook. | A `current_exe()` guard breaks the mcpb shims and symlinked installs; the re-exec has argument injection; a naive in-process hook reintroduces unsandboxed compile, a runtime download and stdout writes. |
| D8 | Session shape | `McpSession::new(&Interpreter, options)` returns an owned value: options, a runtime command handle bound at creation, and a lazily created notebook cache. Methods take `&Interpreter`. `handle_message(&self, &Interpreter, &[u8], &mut dyn Write) -> Result<(), McpError>` is the primitive; `serve_on` wraps a `Take`-capped `read_until` loop over it. The async `run_mcp_server_on` is removed and its tests move to `handle_message`. | A borrowed session cannot live beside its interpreter; a `&str -> Option<String>` primitive forces two full-frame copies and cannot stream. |
| D9 | Timeouts and cancellation | No per-call pool watchdog. The per-call budget is `ctx.set_eval_deadline`, as the notebook does; `drive_until_settled` is fixed once to bound its park by the earlier of the timer and the eval deadline and to cancel the root when the eval deadline passes (the wasm arm already does). `cancel(CallId)` and `notifications/cancelled` go through the lifted `CancelToken`, which targets the running root; a late cancel of a settled root is inert. Per-call `timeout_ms` may only shorten the session default; `0` no longer disables per call (a documented CLI change; `--timeout-ms 0` stays the global escape). Cancellation is cooperative: it reaches External waits and VM back-edges, never a blocking-tier job or a CPU-bound native, and a timed-out blocking job keeps its pool permit until it finishes. | A pool watchdog per call is a task and timer per call that must be aborted on every normal completion, and it cannot reach anything the self-cancel cannot. |
| D10 | Output capture | Roots are submitted with `RootOptions { capture_output: true }` and drained with `take_output`, attributing a detached task's output to the call that started it through an `output_roots` map exactly as the notebook engine does. A per-root byte budget is enforced in the sema-core sink (`max_output_bytes`, default 1 MiB, marker on overflow). A `HostHookScope` RAII guard saves and restores the thread-local hooks for the host-path tools, and a session-lifetime host hook routes anything that falls through to `on_output` or discards it, never to stdout. `ToolOutput::{Capture, PassThrough, Tee}`; `serve_stdio` rejects `PassThrough` and stdout `Tee` with `InvalidOptions`. `max_frame_bytes` (default 16 MiB) is enforced by `Read::take` and, on overflow, the reader drains to the next newline before replying `-32600`. `structured_content` is covered by the same byte budget and by a depth cap in the JSON converter. Rust `println!` in a host function bypasses all of this and corrupts stdio serving; the guide says so. | A cap applied after the call does not bound memory; thread-keyed capture collects other interpreters' output; an overflowed line without a drain is frame injection. |
| D11 | Re-entrancy | An RAII `in_call` guard on the session returns `Reentrant` when a host function invoked from a tool calls back into the session. `Interpreter::eval_str` rejects before `submit_root` when `ctx.runtime_quantum_active() || in_runtime_quantum()`, the same predicate as the legacy VM entry guard, so a second interpreter on the same thread fails closed instead of reaching the prelude panic. | A per-interpreter-only predicate lets a nested interpreter abort; a non-RAII flag leaves the session wedged after `Timeout` or `Panicked`. |
| D12 | CLI and embedded parity | `InterpreterBuilder::with_llm_auto_configure(bool)` and `with_workflow_mcp(bool)` (default off, as today). `sema mcp --preset embedded` and `sema mcp --check tools.sema` (which evaluates the file) show tool-file authors what an embedded host sees; built binaries share the CLI's argument parser. One tool registry with a per-tool kind (`Host` or `RuntimeRoot`) and required `Caps` drives listing, dispatch, presets and the D3 oracle. A `deftool` whose name collides with a reserved builtin name (union of all registry names plus the `_` prefix) is dropped at listing and dispatch with `LogEvent::ToolShadowed`, not rejected at definition. | Definition-time rejection reserves MCP names language-wide, including for `defagent` tool sets and embedded hosts where the builtin does not exist. |
| D13 | Drive isolation | MCP calls drive only their own root (`drive_roots(&[root])`), with an idle drive turn between calls for detached work, so a trivial tool does not spend its deadline on an earlier call's `async/spawn`. | Today's `drive(budget)` runs any ready detached task inside the current call's deadline and capture window. |

## Proposed server API

```rust,ignore
use sema::{mcp::{McpServerOptions, McpSession}, Interpreter, Sandbox, Caps};
use serde_json::json;

let interp = Interpreter::builder()
    .with_sandbox(Sandbox::deny(Caps::SHELL | Caps::PROCESS | Caps::FS_WRITE | Caps::NETWORK | Caps::LLM))
    .build();
interp.eval_str(r#"
  (deftool greet "Greet someone by name."
    {:name {:type :string :description "Who to greet."}}
    (lambda (name) (string-append "hello, " name)))
"#)?;

let session = McpSession::new(
    &interp,
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
| `list_tools(&self, &Interpreter) -> Vec<Tool>` | Selected builtins and exposed `deftool`s. One `is_exposed` predicate is shared with dispatch and `docs`. |
| `call_tool(&self, &Interpreter, name, &serde_json::Value) -> Result<CallToolResult, McpCallError>` | Dispatch with the selection, sandbox, timeout and output rules above. Map and sequence results also populate `structured_content`. |
| `call_tool_with_id(&self, &Interpreter, CallId, name, args)` | Same, with a caller-chosen id that `cancel` can target. |
| `handle_message(&self, &Interpreter, &[u8], &mut dyn Write) -> Result<(), McpError>` | One JSON-RPC frame in, zero or one frame written out. The host owns transport and framing. |
| `serve_on(&self, &Interpreter, impl BufRead, impl Write) -> Result<(), McpError>` | Blocking, serial loop over `handle_message` with the frame cap, until EOF or a transport error. Stops when the reader closes. |
| `serve_stdio(&self, &Interpreter)` | `serve_on` over locked stdio. Only when the process is the MCP server: host output on stdout corrupts the frame stream. |
| `cancel_handle(&self) -> McpCancelHandle` | `Send + Sync`; `cancel(CallId)` cancels that call's root from another thread. Never exposes `cancel_all`. |

For async hosts:

```rust,ignore
let worker = sema::mcp::McpWorker::spawn(
    Interpreter::builder().with_sandbox(sandbox),
    McpServerOptions::embedded(),
    |interp| { interp.register_fn("host/now", ..); interp.load_file("tools.sema") },
)?;
let handle = worker.handle(); // Send + Sync + Clone
let result = handle.call_tool("greet", json!({"name": "Kameko"})).await?;
handle.cancel(call_id);
worker.shutdown().await;
```

The worker thread owns the interpreter and session; the handle exchanges owned
JSON over channels. `on_output` and `on_log` closures must be `Send + 'static`
and must not block (they run inside a quantum); a `try_send` into a channel is
the pattern.

Re-export `Tool`, `CallToolResult`, `ToolContent`, `CallId`, and
`McpCancelHandle` through `sema::mcp`, all `#[non_exhaustive]`, with
constructors (`Tool::new`, `CallToolResult::text`, `::error`,
`::structured`) and JSON shape tests so hosts can merge them with other MCP
libraries. Callers need only `sema-lang` and `serde_json`. `on_log` receives a
typed `LogEvent` (`Started`, `Stopped`, `WriteFailed`, `ToolShadowed`,
`ToolExcluded`), never request content; the CLI prints today's messages from
it. `server_info` overrides the `initialize` response; the default stays.
`protocolVersion` stays `2024-11-05` until `structured_content` and
`outputSchema` are adopted together.

## Ownership and execution

The session stays on its creating thread with the interpreter (`!Send`, #175).
It does not borrow the interpreter. Dropping the interpreter after the session
is safe: the command handle's sends return `false`. `catch_unwind` around
dispatch is a convenience for unwind hosts only; under `panic = "abort"` (the
workspace release profile) a panic in a tool body aborts the process, and the
public docs say so. Untrusted tool files run out of process (`sema mcp
--sandbox` as a child). The guide's example sandbox denies `NETWORK` and `LLM`
because `llm/configure` with a caller-supplied base URL is an egress channel
that `NETWORK` does not gate today.

### Connections

MCP client connections live in a thread-local registry keyed by handle string;
any interpreter on the thread can use another's handle, and nothing closes them
when an interpreter is dropped. They are torn down when the thread exits
(stdio children are killed; HTTP sessions get no DELETE). `sema_mcp::close_handle`
closes one from Rust. The guide documents this, `(mcp/close)`, and the
`:command` → `PROCESS`, `:url` → `NETWORK` capability mapping. Interpreter-owned
connections are a follow-up.

### Performance notes

Session creation is free. The `eval` tool already pays `build_vm_for_exprs`
per call, whose cost grows with session age: a whole span-table clone there and
again inside `lower` per top-level form, plus `collect_native_names` over every
global. The prerequisite PR passes the span table by reference in both places
and caches native names on `Env.version`. Whole-call root routing adds roughly
10 to 20 µs per trivial tool call, below the frame cost. Per-call copies to
remove: `params.clone()`, rebuilding every schema in `tools/list`, the capture
buffer clone, and the response `to_string` then `format!`. `structured_content`
as `RawValue` and a reader-thread split for interpreter-free methods are
follow-ups enabled by D8.

## Implementation outline

1. Prerequisite PR: span table by reference in `build_vm_for_exprs` and
   `lower`; `collect_native_names` cached on `Env.version`; eval deadline
   honored by `drive_until_settled` while parked; `CancelToken` and the
   worker bridge lifted out of `sema-notebook` with the notebook as their
   first user (no behavior change there).
2. Tool registry with kind and required `Caps`; shared `is_exposed`;
   reserved-name shadowing with `LogEvent`; snapshot test pinning `cli()` with
   and without a build hook (D1, D12).
3. Sandbox in tool bodies with size caps; session-interpreter `compile` and
   `disasm`; `Engine::from_session`; honest `info`; `build` removed with the
   CLI hook (D2, D7).
4. Conformance test for `io_block_on` sites; runtime-root dispatch via
   `submit_value` for `eval`, `run_file`, `deftool`, `mcp/*`; `drive_roots`
   per call (D3, D13).
5. Self-cancel at deadline, `CallId` and `cancel_root` mapping, shortening-only
   `timeout_ms` with doc and schema updates (D9). Hook scope, per-root byte
   budget in sema-core, orphan route, output policy, bounded frame reader (D10).
   RAII fence and the pre-submit quantum check (D11).
6. Typed errors, constructors, `structured_content` with depth cap, JSON
   shape tests (D5).
7. Public `McpSession`, `handle_message`, `serve_on`, `serve_stdio`,
   `cancel_handle`; `McpWorker` and handle; async loop removed and its tests
   migrated (D4, D8).
8. Builder parity flags, `sema mcp --preset` and `--check`, shared argument
   parser for built binaries, the CLI-vs-embedded matrix, client builtins and
   connection semantics in `website/docs/embedding.md` (D12).

## Acceptance checks

- A fixture depending only on `sema-lang` and `serde_json` registers a
  `register_fn` and a `register_async_fn` host function, defines a `deftool`
  calling each, and invokes both through the session; the async one succeeds.
- `embedded()` lists exactly `docs`, `docs_search`, `info`, and exposed
  `deftool`s; `enable_eval()` adds `eval`; `also_tools(["eval"])` without it is
  `InvalidOptions`; `exclude_tools(["eval"])` beats `enable_eval()`;
  `only_tools` applies to `deftool`s; an unknown name is `InvalidOptions`.
  `cli()` matches a snapshot of today's list with and without a build hook.
- A hidden `deftool` is neither listed, callable, nor described by `docs`,
  even when named in `only_tools`; a `deftool` named `eval` or `run_file` is
  dropped with `ToolShadowed` under both presets.
- With `FS_READ` denied, `run_file`, `compile`, `disasm`, `notebook/read`
  return `is_error` without reading; with `FS_WRITE` denied, `compile` output,
  `fmt` in place, `notebook/save` and `/export` return `is_error` without
  writing; `fmt` outside `--allowed-paths` is denied; `run_file` on a 1 GiB
  file or a FIFO returns `is_error` without reading it.
- A notebook cell `(shell ...)` under `strict` is denied; opening a notebook
  leaves the session's LLM providers configured; requesting
  `notebook/eval_cell` under a restricted sandbox is `InvalidOptions` until
  `Engine::from_session` lands.
- `compile` through the session sees a macro defined in that interpreter and
  does not leak the compiled file's macros into later calls.
- The build hook under `cli()` with `--sandbox strict` returns `is_error`
  without reading, writing or downloading, and writes nothing to stdout.
- `block_on_ops()` is unchanged and no panic occurs across `eval`, `deftool`,
  macro-expanding `eval`, `run_file`, and `mcp/close` calls run on a Tokio
  worker thread; the conformance test lists no unguarded `io_block_on` site.
- An `eval` parked on a never-resolving host future with `timeout_ms: 100`
  returns `Timeout` within the budget and the host future is dropped (drop
  guard observed). `timeout_ms: 0` and values above the session default are
  clamped. `cancel(CallId)` from another thread cancels that call and the
  session is usable afterwards; a cancel for a settled id is a no-op.
- A host-installed stdout hook is still installed after `call_tool("eval")`;
  output from a detached task of an earlier call reaches `on_output`, not
  stdout; `(loop (print "x"))` under a 2 s timeout stays under
  `max_output_bytes` plus a constant and ends with the marker; an `eval` that
  prints a crafted JSON-RPC line through `serve_on` yields exactly one response
  frame; `PassThrough` with `serve_stdio` is `InvalidOptions`.
- An oversized frame yields `-32600` and the next frame is parsed correctly.
- A host function that calls `call_tool` from inside a tool receives
  `Reentrant`; `eval_str` on the same or a second interpreter from inside a
  tool returns an error, leaves no orphan root (`runtime_live_task_count` is
  0 afterwards), and the fence is released after `Timeout` and `Panicked`.
- A trivial `deftool` call completes within its budget while an earlier
  call's detached `(async/spawn (loop ...))` is still queued (D13).
- In-memory protocol tests over `handle_message` cover initialize, list, call,
  malformed JSON, oversized frame, notifications, EOF and transport errors,
  and assert nothing is written to stderr. The former async-loop tests pass
  through `handle_message`.
- `McpWorker` from inside a `#[tokio::test]`: `call_tool` resolves, `cancel`
  works, `shutdown` returns within the interpreter's 2 s drop budget.
- A test binary in `crates/sema` serves `McpSession::serve_stdio`; an embedded
  interpreter does `mcp/connect {:command ...}`, `mcp/call`, `mcp/close`
  against it (client path over a real pipe).
- `server_info` changes `initialize`; the default is unchanged.
- The MCP suite runs once under `-C panic=abort` so every "converted to
  `is_error`" claim is proven live or documented as unwind-only.
- Existing CLI protocol tests pass, except the documented `timeout_ms: 0`
  change, whose test is updated with the doc and schema text.

## Follow-ups (not in this change)

- `Interpreter::register_tool` / `register_async_tool` from Rust (medium).
- Interpreter-owned MCP connections closed on drop (medium).
- Reader-thread split for interpreter-free methods and server-to-client
  notifications during a call (medium, needs D8).
- `Engine::from_session` if it does not fit the first change (medium).
- Host-supplied Tokio handle for the I/O pool via a sanctioned `sema-io`
  wrapper (medium to large).
- `mcp-server` / `mcp-client` Cargo features on `sema-lang` (medium).
- `structured_content` as `RawValue`; docs index borrowing the static corpus
  (small each).
- Gate non-default `llm/configure` base URLs on `NETWORK` when sandboxed
  (small, separate security change).
- Typed Rust MCP client (D6).

## Code references

- Public wrapper and builder: `crates/sema/src/lib.rs`.
- Server loops and dispatch: `crates/sema-mcp/src/server.rs`, `tools.rs`
  (`is_tool_allowed`, `list_mcp_tools`, `call_mcp_tool`, the `compile`,
  `disasm`, `fmt`, `run_file`, `build`, `deftool` arms, `eval_with_capture`).
- Protocol types: `crates/sema-mcp/src/protocol.rs`.
- Client registration, connections and `HOST_SANDBOX`:
  `crates/sema-mcp/src/builtins.rs`; `close_handle` there and in
  `crates/sema/src/workflow_mcp.rs`.
- Notebook engine: `crates/sema-notebook/src/engine.rs`; cache:
  `crates/sema-mcp/src/notebook.rs`.
- Notebook host pattern to generalize: `crates/sema-notebook/src/engine.rs`
  (`CancelToken`, `running_root`, `output_roots`, `eval_source_named`) and
  `bridge.rs` (`EngineHandle::spawn`).
- Runtime entry and expansion: `crates/sema-eval/src/eval.rs`
  (`submit_value`, `submit_str`, `drive_until_settled`, `drive_roots`,
  `take_output`, `command_handle`, `build_vm_for_exprs`, the `__vm-eval`
  runtime path, `Drop`); `crates/sema-vm/src/lower.rs` (span
  table clone); `crates/sema-vm/src/runtime/host_api.rs` (`cancel_root`,
  `command_handle`, `drive_roots`); `wait.rs` (`block_on_inbox`).
- Output hooks and capture sink: `crates/sema-core/src/output_hook.rs`;
  deadline poll: `crates/sema-core/src/context.rs`.
- I/O seam: `crates/sema-core/src/io_backend.rs`, `crates/sema-io/src/lib.rs`.
- CLI entry points: `crates/sema/src/main.rs` (`run_mcp`, `build_interpreter`,
  `resolve_mcp_tool_timeout`), `crates/sema/src/build.rs` (`run_build`,
  `try_run_embedded`), `mcpb/`.
- Async registration (shipped): `docs/plans/archive/2026-10-21-async-native-registration.md`.
