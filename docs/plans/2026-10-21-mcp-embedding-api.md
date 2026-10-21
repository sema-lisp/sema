# MCP access through the public embedding API

**Status:** Draft design; no API in this document is implemented by this PR.

## Problem and current behavior

[Kameko's embedding feedback](https://github.com/sema-lisp/sema/pull/172#issuecomment-6303419534)
reports difficulty accessing MCP from an embedded interpreter. The comment does
not establish whether the intended integration is a client, a server, or both.
There are two different cases in the current code:

- **Client:** `InterpreterBuilder` enables `mcp/connect`, `mcp/tools`,
  `mcp/call`, and `mcp/close` by default. An embedder can call them through Sema.
  `with_mcp(false)` disables these builtins. The embedding guide should show
  this existing path and list the builder option.
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
interp.eval_str("(define answer 42)")?;

let mut server = interp.mcp_server(
    McpServerOptions::new().include_tools(["eval"]),
)?;
let tools = server.list_tools();
let result = server.call_tool("eval", &json!({"code": "answer"}));
assert!(!result.is_error);
```

`McpServer<'interp>` should provide:

| Method | Contract |
| --- | --- |
| `list_tools(&self)` | Return the selected builtins and `deftool` definitions using the existing schemas. |
| `call_tool(&mut self, name, arguments)` | Dispatch with the same selection and timeout rules as the server loop; preserve `CallToolResult` content and `is_error`. |
| `serve_stdio(&mut self)` | Run the blocking stdio protocol loop until EOF or a transport error. |
| `serve_on(&mut self, reader, writer)` | Use caller-provided blocking buffered input and output, including in-memory test streams. |

Re-export the existing `Tool`, `CallToolResult`, and `ToolContent` through
`sema::mcp`, after reviewing their compatibility contract. Do not convert all
results to strings. Use a small public error type for invalid configuration
and transport failures; a tool execution error remains a `CallToolResult`.
Callers should need only `sema-lang` and `serde_json` in their dependencies.

Options cover include/exclude filters and the default per-call timeout.
Preserve the existing CLI defaults and filter precedence. Document that omitting
filters exposes the existing default tool set, including `eval`, `run_file`,
`compile`, and notebook operations; examples should select tools explicitly.
Preserve the current per-request timeout override behavior and outer deadline
handling. A tool timeout is not a guarantee that arbitrary blocking host code
can be interrupted. The options are not a replacement for the sandbox.

## Ownership and execution

The session borrows the **public** interpreter. This keeps its telemetry guard
alive and avoids a second interpreter with different definitions or settings.
The notebook cache lives for the session, not one call. The interpreter and
session stay on their creating thread. The thread-affinity fix for `Value`
must land before this is implemented.

The existing synchronous server loop explicitly requires execution outside a
Tokio runtime: some tool bodies call `sema_io::io_block_on`, which cannot start
a nested runtime. Merely exposing the existing async server loop would not
solve that problem. For the first API, reject an unsupported ambient runtime
with an actionable error before evaluating a tool, rather than allowing a
panic in a tool body. Async hosts can create the interpreter inside a dedicated
blocking thread and exchange owned JSON requests and results with that thread.
They must not construct an interpreter elsewhere and move it into the thread.

This proposal does not add a host-async evaluation API. Supporting one requires
a separate execution contract and tests for synchronous LLM and I/O builtins.

## Implementation outline

1. Extract the blocking input/output loop so it accepts a borrowed internal
   interpreter, a persistent notebook cache, and the existing options. Keep
   the current owning entry points as wrappers for compatibility.
2. Add the public session in `crates/sema/src/`, delegating tool discovery and
   dispatch to `sema-mcp`. Keep `Interpreter::inner` private; do not add `Deref`
   or a general escape hatch to the internal evaluator.
3. Make runtime-context errors and transport failures explicit in the new API.
   Preserve the existing protocol framing and serial request processing.
4. Add an embedding example and update `website/docs/embedding.md` with both
   client builtin usage and the new server API.

## Client follow-up

A typed Rust client wrapper is a possible second milestone, after confirming
the operations embedders need. It should delegate to the existing client
transport, authentication, and connection cleanup. It must check the actual
interpreter's sandbox and tool allowlist. Audit `HOST_SANDBOX` and other
thread-local client state with two interpreters on one thread before promising
per-interpreter isolation. Define who owns a connection and what closing or
dropping its handle does.

Keep workflow MCP resolver registration distinct from ordinary client
builtins: `workflow_mcp::register_real_resolver()` is separate setup today.
The server API must not imply that this workflow setup happens automatically.

## Acceptance checks

- An external fixture depending only on `sema-lang` and `serde_json` creates
  a public interpreter, registers a host function, and calls it through MCP
  using definitions already loaded in that same interpreter.
- In-memory protocol tests cover initialization, listing, calls, malformed
  input, notifications, EOF, and transport errors without a live MCP service.
- Include/exclude filters apply identically to listing and dispatch. Tests
  cover declared tools, tool errors, sandbox restrictions, and timeouts.
- Notebook state persists across calls; dropping the session releases its
  cache. The interpreter and its telemetry guard remain alive for the borrow.
- A Tokio host receives a clear runtime-context error rather than a nested
  runtime panic. A dedicated-thread example works using owned JSON messages.
- Existing CLI protocol tests continue to pass without changing defaults.

## Code references and open decisions

- Public wrapper and builder: `crates/sema/src/lib.rs`.
- Server loops and dispatch: `crates/sema-mcp/src/server.rs` and `tools.rs`.
- Protocol types: `crates/sema-mcp/src/protocol.rs`.
- Client registration and configuration: `crates/sema-mcp/src/builtins.rs`.
- Related embedding work: [earlier embedding API plan](2026-03-11-embedding-api-improvements.md).

Before implementation, settle the public error type and whether runtime-context
validation belongs at session creation or at each operation that evaluates
code. Confirm whether Kameko needs Rust client methods in addition to the
documented Sema builtin path. These choices do not require exposing internal
interpreter fields.
