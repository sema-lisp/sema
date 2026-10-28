---
title: Migrating to Sema 2.0
description: Update Rust embedding code, retrieval configuration, recorded cassettes, and MCP hosts from Sema 1.37.
---

# Migrating to Sema 2.0

Sema 2.0 includes changes that require updates to Rust applications that embed
Sema. Retrieval defaults and validation also change. Read the sections that
apply to your application before updating its dependencies or stored vectors.

## Rust values stay on their interpreter thread

`Value` and `SemaError` no longer implement `Send` or `Sync`. Values use
non-atomic reference counts, and keyword and symbol names belong to a
thread-local string interner. Moving them between threads was unsafe even
when the Rust compiler accepted it in 1.x.

Send owned Rust strings, structs, or `serde_json::Value` between threads. Create
Sema values on the thread that owns the interpreter:

```rust
use sema::{Interpreter, Value};

let name = std::thread::spawn(|| String::from("ready")).join().unwrap();
let interpreter = Interpreter::new();
interpreter.env().set_str("status", Value::keyword(&name));
assert_eq!(interpreter.eval_str("status").unwrap(), Value::keyword("ready"));
```

Convert errors to owned text on their original thread before sending them to a
worker or wrapping them in an error type that requires `Send + Sync`, such as
`anyhow::Error`. Raw `Spur` interner keys remain thread-local identifiers; their
Rust type does not enforce that restriction.

Use [`register_async_fn` or `register_async_fn_typed`](/docs/embedding) to await
host work. The async future receives owned `Send` input and produces owned
`Send` output; conversion to and from `Value` runs on the interpreter thread.
A Tokio task that may move between worker threads must not hold a Sema value
across an `.await`.

## Add fields to Rust retrieval requests

`sema_llm::types::EmbedRequest` adds `options: EmbedOptions`. Add the field to
existing struct literals, or use the request's `Default` implementation:

```rust
use sema_llm::types::EmbedRequest;

let request = EmbedRequest {
    texts: vec!["example".to_owned()],
    model: None,
    options: Default::default(),
};
```

`RerankRequest` adds `timeout_ms: Option<u64>`:

```rust
use sema_llm::types::RerankRequest;

let request = RerankRequest {
    query: "example".to_owned(),
    documents: vec!["an example document".to_owned()],
    top_k: Some(1),
    model: None,
    timeout_ms: None,
};
```

Custom `LlmProvider` implementations can keep using the new trait methods'
defaults. Override the embedding and reranking model/option methods when those
defaults differ from the chat model. Retrieval configuration now has its own
registry, so configuring embeddings does not replace a chat provider with the
same name.

## Pin the model used by an existing vector index

The implicit Voyage embedding model changes from `voyage-3-lite` to
`voyage-3`. Set the model explicitly when continuing to query an existing
index:

```sema
(llm/configure-embeddings :voyage {:model "voyage-3-lite"})
```

Equal vector dimensions do not make models interchangeable. To adopt a new
model, rebuild the stored vectors with that model and update queries to use it.

The default Together reranker changes from `BAAI/bge-reranker-v2-m3` to
`mixedbread-ai/mxbai-rerank-large-v2`. Pin a reranker model explicitly if result
ordering must remain comparable to earlier runs.

Ollama uses its native `/api/embed` endpoint. Sema's default embedding model is
`embeddinggemma`; select the locally installed model if it differs. Nomic's
hosted preset uses its native text embedding endpoint and supports embeddings.
The public `RerankDialect::Nomic` variant remains for custom hosts; it does not
enable a hosted Nomic reranker.

See [embedding configuration](/docs/llm/embeddings) for provider-specific options.

## Re-record embedding cassettes

Embedding cassette identities use `embed-v2` and include the provider, endpoint,
resolved model, and semantic options. Existing embedding tapes must be
re-recorded. Keep the model and options explicit when recordings are used with
an existing vector index.

## Handle stricter retrieval errors

Embedding inputs must be strings. Convert other values deliberately before
calling `llm/embed`; 1.x implicitly converted some non-string inputs.

Malformed options and options unsupported by the selected protocol return an
error before dispatch. Responses must contain one nonempty, finite vector per
input, with consistent dimensions. Indexed responses are returned in input
order. Reranking rejects duplicate or out-of-range indices and non-finite
scores, sorts by descending score, and applies `top-k`. Cassette replay follows
the same checks as live responses.

If a custom provider starts returning an error, fix its request or response
contract rather than relying on malformed rows being silently discarded.

## Use the public MCP embedding API

Rust hosts can use `sema::mcp::McpSession` with the public `sema::Interpreter`.
They no longer need access to the wrapper's private internal interpreter.
The default embedded preset exposes visible `deftool` definitions, `docs`,
`docs_search`, and `info`. Enable `eval` explicitly only when clients should
have the interpreter's full host access.

MCP sessions remain on the interpreter thread. Pass owned JSON between an
async host and the interpreter. Host Rust `println!` writes directly to stdout
and can corrupt a stdio MCP connection; use Sema's captured output or a host
logging channel.

The public MCP protocol result types are non-exhaustive. Use their constructors
and add a wildcard arm when matching `ToolContent`, so future content variants
can be handled safely.

Per-call MCP `timeout_ms` can shorten the session limit. Zero retains that
limit, and larger values are clamped; use the server's `--timeout-ms 0` to
disable its default. Hidden deftools cannot be called or described through MCP,
and builtin tool names are reserved for MCP dispatch.

The CLI installs an in-process `build` hook that requires an unrestricted
session. Library adapters and built `--mcp` binaries omit `build` unless the
host supplies a hook. File tools enforce the interpreter's capabilities and
allowed paths. Notebook evaluation tools are unavailable in restricted
sessions. Existing `--include` configurations that name unavailable tools now
return an option error. Standalone MCP binaries use the same argument parser
as the CLI and reject malformed option values.

Sessions default to 16 MiB for each input frame or source file and 1 MiB for
captured output and result text. Oversized frames return an error; oversized
output ends with `[output truncated]`. Collection results keep their JSON text
format, with `[maximum depth]` at 128 nested levels. Rust hosts can configure
the byte limits through `McpServerOptions`.

See [Rust embedding](/docs/embedding) and [MCP serving](/docs/mcp) for examples.
