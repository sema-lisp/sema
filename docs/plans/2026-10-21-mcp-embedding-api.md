# MCP access through the public embedding API

**Status:** Draft design; no API in this document is implemented by this PR.

## Problem and current behavior

[Kameko's embedding feedback](https://github.com/sema-lisp/sema/pull/172#issuecomment-6303419534)
reports: "my biggest pain point so far is not having MCP functionality by
embedding in the 'sanctioned' way, I have to use internal crates like
`sema-eval` just to accomplish that." The comment does not say whether the
integration is a client, a server, or both, but only the server helpers
require `sema-eval`. There are two different cases in the current code:

- **Client:** `InterpreterBuilder` enables `mcp/connect`, `mcp/tools`,
  `mcp/call`, and `mcp/close` by default. An embedder can call them through Sema.
  `with_mcp(false)` disables these builtins. The embedding guide should show
  this existing path and list the builder option. One limitation applies today:
  `register_mcp_builtins` stores the sandbox in a thread-local (`HOST_SANDBOX`,
  `crates/sema-mcp/src/builtins.rs`), so with two interpreters on one thread
  the last one built decides what `mcp/connect` may do for both. The guide must
  document this next to the existing note on shared per-thread LLM state.
- **Server:** `sema_mcp::tools::list_mcp_tools`, `call_mcp_tool`, and the server
  loops accept `sema_eval::Interpreter`. The public `sema::Interpreter` owns
  that type in a private field. An embedder cannot pass its interpreter to
  these helpers, even after adding `sema-mcp` as a dependency.

For example, this fails to type-check today:

```rust,ignore
let interp = sema::Interpreter::new();
sema_mcp::tools::list_mcp_tools(&interp, None, None);
// Expected sema_eval::Interpreter; received sema::Interpreter.
```

The first implementation should document client access and expose a small
server API through `sema`. It should preserve the interpreter's definitions,
registered host functions, sandbox, and telemetry lifetime.

## Proposed server API

Add a `sema::mcp` module with supported options and protocol result types.
An interpreter method constructs a session that borrows the public interpreter
and owns its notebook cache. The following names are proposed:

```rust,ignore
use sema::{mcp::McpServerOptions, Interpreter};
use serde_json::json;

let interp = Interpreter::new();
interp.eval_str(r#"
  (deftool greet "Greet someone by name."
    {:name {:type :string :description "Who to greet."}}
    (lambda (name) (string-append "hello, " name)))
"#)?;

let server = interp.mcp_server(
    McpServerOptions::embedded()
        .server_info("my-host", env!("CARGO_PKG_VERSION"))
        .include_tools(["greet"]),
)?;
let tools = server.list_tools();
let result = server.call_tool("greet", &json!({"name": "Kameko"}));
assert!(!result.is_error);
```

`McpServer<'interp>` should provide:

| Method | Contract |
| --- | --- |
| `list_tools(&self)` | Return the selected builtins and `deftool` definitions using the existing schemas. |
| `call_tool(&self, name, arguments)` | Dispatch with the same selection and timeout rules as the server loop; preserve `CallToolResult` content and `is_error`. Program output written by the tool is captured into the result, not delivered to the host. |
| `serve_stdio(&self)` | Run the blocking stdio protocol loop until EOF or a transport error. |
| `serve_on(&self, reader, writer)` | Use caller-provided blocking `BufRead` and `Write`, including in-memory test streams. |

The methods take `&self`: `call_mcp_tool` already works on a shared
interpreter reference and the notebook cache is interior-mutable. Requests are
still processed serially; the session is not `Send`.

Re-export the existing `Tool`, `CallToolResult`, and `ToolContent` through
`sema::mcp`. Mark all three `#[non_exhaustive]` first: `ToolContent` has only a
`Text` variant today and MCP also defines image, audio, and resource content,
so the enum must be able to grow without a semver break in `sema-lang`. Do not
convert all results to strings. Use a small public error type for invalid
configuration and transport failures; a tool execution error remains a
`CallToolResult`. Callers should need only `sema-lang` and `serde_json` in
their dependencies.

The session must emit nothing on stderr. The current loops print start, exit,
and write-failure messages with `eprintln!`; the library form routes those
through an optional `on_log` callback in the options, defaulting to silence.
The CLI wrappers install a callback that preserves today's messages.

`serverInfo` in the `initialize` response is currently hardcoded to
`sema-mcp` and the crate version. `McpServerOptions::server_info(name, version)`
overrides it; the default stays unchanged so CLI protocol tests pass.

### Tool selection

Options cover include/exclude filters and the default per-call timeout, with
the existing filter precedence. Two constructors differ only in the default
tool set:

| Constructor | Default tools | Intended for |
| --- | --- | --- |
| `McpServerOptions::cli()` | Everything the CLI exposes today. | `sema mcp`, `sema build` binaries, existing tests. |
| `McpServerOptions::embedded()` | `eval`, `docs`, `docs_search`, `info`, and `deftool` definitions. | The public session API. |

`embedded()` is the only constructor the embedding guide shows. Three default
CLI tools are wrong or unsafe inside a host process, which is why the embedded
preset omits them and `include_tools` must name them explicitly to re-enable:

| Tool | Problem when embedded | Source |
| --- | --- | --- |
| `build` | Re-executes `std::env::current_exe()` with `build <src> -o <out>`. In a host that is the host binary, not `sema`. | `crates/sema-mcp/src/tools.rs`, `"build"` arm |
| `compile`, `run_file` (source path) | Construct a fresh `Interpreter::new_with_sandbox(allow_all)`, discarding the embedder's sandbox and macros. | `tools.rs`, `"compile"` and `"run_file"` arms |
| `run_file`, `compile`, `notebook/*` | Read and write paths with `std::fs` directly; the interpreter sandbox gates builtins, not tool bodies. | same arms and `crates/sema-mcp/src/notebook.rs` |

Independently of the preset, the tool bodies themselves should check the
interpreter's sandbox (`Caps::FS_READ`, `FS_WRITE`, `PROCESS`) before touching
the filesystem or spawning. The CLI defaults to an allow-all sandbox, so this
changes nothing there and makes `sema mcp --sandbox` honest. `build` should
additionally refuse to run unless the current executable is the Sema CLI.
`compile` should compile with the session's interpreter (or a child
environment of it) rather than a fresh one, so embedder macros are visible.

The options are not a replacement for the sandbox, and the sandbox does not
cover tool bodies until the checks above exist. A tool timeout is not a
guarantee that arbitrary blocking host code can be interrupted.

## Ownership and execution

The session borrows the **public** interpreter. This keeps its telemetry guard
alive and avoids a second interpreter with different definitions or settings.
The notebook cache lives for the session, not one call. The interpreter and
session stay on their creating thread. The thread-affinity change in #175
should land first so the dedicated-thread example below is checked by the
compiler; the session API itself is single-threaded either way.

### Output capture

`eval` and `deftool` dispatch install thread-local stdout/stderr hooks so
program output lands in the `CallToolResult` instead of the protocol stream.
Today the hooks are reset to `None` afterwards. The session must save the
previous hooks and restore them, so a host that installed its own capture
keeps it across MCP calls. This needs a `take`/`replace` form of the hook
setters in `sema-core`.

### Tokio and blocking I/O

Which tool bodies can run under an ambient Tokio runtime depends on the
dispatch path, not the tool:

- The `eval` tool runs through the interpreter's runtime. LLM and MCP builtins
  inside it suspend on External waits that the I/O pool drives with `spawn`,
  so they do not nest a `block_on`.
- The `deftool` arm calls the handler with `call_value` directly, outside a
  runtime quantum. Any `llm/*` or `mcp/*` builtin inside a tool handler then
  reaches `sema_io::io_block_on`, which calls `Runtime::block_on` on the
  process pool and panics inside a Tokio worker ("Cannot start a runtime from
  within a runtime"). Under `panic = "abort"` that kills the host.

The implementation should route `deftool` handlers through the runtime the
same way `eval` is routed. That removes the asymmetry and gives handlers the
same deadline and cancellation behavior. Until then, the runtime-context check
below must guard `deftool` dispatch, not only session creation.

Detection is best effort. `tokio::runtime::Handle::try_current()` is not
sufficient on its own: it succeeds inside `spawn_blocking` closures, where a
nested `block_on` is allowed, so a naive check would reject the one pattern
async hosts are told to use. The recommended host pattern is therefore a plain
`std::thread::spawn` thread that owns the interpreter and session and exchanges
owned JSON requests and results with the async side over channels; on such a
thread `try_current()` fails and the check is reliable. A host that insists on
`spawn_blocking` must opt out of the check with
`McpServerOptions::allow_ambient_runtime(true)` and accept the deftool
constraint above. Hosts must not construct an interpreter elsewhere and move
it into the thread.

The CLI also evaluates `(llm/auto-configure)` and calls
`workflow_mcp::register_real_resolver()` before serving. The session does
neither; the guide must say so, since a tool file tested under `sema mcp` can
behave differently when embedded.

This proposal does not add a host-async evaluation API. Supporting one requires
a separate execution contract and tests for synchronous LLM and I/O builtins.

## Implementation outline

1. Make the sync loop in `crates/sema-mcp/src/server.rs` generic over
   `BufRead + Write`, taking a borrowed internal interpreter, a persistent
   notebook cache, the options, and the log callback. Reimplement
   `run_mcp_server_sync` and the async `run_mcp_server_on` as wrappers so the
   frame handling exists once.
2. Add sandbox checks and the `current_exe` guard to the `run_file`,
   `compile`, `build`, and notebook tool bodies in `tools.rs`; compile with the
   session interpreter instead of a fresh one.
3. Route `deftool` dispatch through the runtime; add the runtime-context check
   to the remaining host-adapter paths.
4. Add hook save/restore to `eval_with_capture`.
5. Mark the protocol types `#[non_exhaustive]`; add `server_info` and the
   `cli()`/`embedded()` presets to the options.
6. Add the public session in `crates/sema/src/`, delegating tool discovery and
   dispatch to `sema-mcp`. Keep `Interpreter::inner` private; do not add `Deref`
   or a general escape hatch to the internal evaluator.
7. Make runtime-context errors and transport failures explicit in the new API.
   Preserve the existing protocol framing and serial request processing.
8. Add an embedding example and update `website/docs/embedding.md` with client
   builtin usage, the `HOST_SANDBOX` limitation, and the new server API.

## Client follow-up

A typed Rust client wrapper is a possible second milestone, after confirming
the operations embedders need. It should delegate to the existing client
transport, authentication, and connection cleanup. It must check the actual
interpreter's sandbox and tool allowlist, which means replacing the
thread-local `HOST_SANDBOX` with per-interpreter state rather than auditing it.
Define who owns a connection and what closing or dropping its handle does.

Keep workflow MCP resolver registration distinct from ordinary client
builtins: `workflow_mcp::register_real_resolver()` is separate setup today.
The server API must not imply that this workflow setup happens automatically.

## Acceptance checks

- An external fixture depending only on `sema-lang` and `serde_json` creates
  a public interpreter, registers a host function, defines a `deftool` that
  calls it, and invokes the tool through the session.
- In-memory protocol tests over `serve_on` cover initialization, listing,
  calls, malformed input, notifications, EOF, and transport errors without a
  live MCP service, and assert that nothing is written to stderr.
- `embedded()` lists exactly `eval`, `docs`, `docs_search`, `info`, and
  declared tools; `cli()` lists what `sema mcp` lists today.
- With a sandbox that denies `FS_READ`, `run_file`, `compile`, and
  `notebook/read` return `is_error` without reading the file. With `PROCESS`
  denied, `build` returns `is_error` without spawning. `build` returns
  `is_error` when the current executable is not the Sema CLI.
- `compile` through the session sees a macro defined in that interpreter.
- A host-installed stdout hook is still installed after `call_tool("eval")`.
- `server_info` changes the `initialize` response; the default is unchanged.
- Include/exclude filters apply identically to listing and dispatch. Tests
  cover declared tools, tool errors, sandbox restrictions, and timeouts.
- Notebook state persists across calls; dropping the session releases its
  cache. The interpreter and its telemetry guard remain alive for the borrow.
- A `deftool` handler that calls `llm/*` under a Tokio worker returns a
  runtime-context `is_error` result rather than panicking. The same handler
  succeeds on a `std::thread::spawn` thread exchanging owned JSON messages.
- Existing CLI protocol tests continue to pass without changing defaults.

## Code references and open decisions

- Public wrapper and builder: `crates/sema/src/lib.rs`.
- Server loops and dispatch: `crates/sema-mcp/src/server.rs` and `tools.rs`
  (`"build"`, `"compile"`, `"run_file"`, and `deftool` arms; `eval_with_capture`).
- Protocol types: `crates/sema-mcp/src/protocol.rs`.
- Client registration and `HOST_SANDBOX`: `crates/sema-mcp/src/builtins.rs`.
- Output hooks: `crates/sema-core/src/output_hook.rs`.
- CLI entry points that set defaults: `crates/sema/src/main.rs` (`build_interpreter`,
  `DEFAULT_MCP_TOOL_TIMEOUT_MS`) and `crates/sema/src/build.rs`.
- Related embedding work: [earlier embedding API plan](2026-03-11-embedding-api-improvements.md);
  async native registration is proposed separately in #177.

Before implementation, settle the public error type and whether the
`allow_ambient_runtime` opt-out is worth shipping or hosts should be told to
use a plain thread only. Confirm with Kameko whether Rust client methods are
needed in addition to the documented Sema builtin path. These choices do not
require exposing internal interpreter fields.
