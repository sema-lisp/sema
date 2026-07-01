---
outline: [2, 3]
---

# Model Context Protocol (MCP)

Sema includes a built-in [Model Context Protocol](https://modelcontextprotocol.io/) server. This allows LLM clients (such as Claude Desktop, Cursor, or Claude Code) to inspect, compile, format, evaluate, and build Sema code in your host environment, as well as execute user-defined Lisp tools.

The server communicates over standard input/output (stdio) using JSON-RPC 2.0.

```bash
sema mcp
```

Sema is also an MCP **client** — it can *consume* external MCP servers' tools (see [Sema as an MCP client](#sema-as-an-mcp-client) below).

---

## Default MCP Tools

When started, the MCP server exposes a set of core developer tools:

| Tool Name | Description | Parameters |
|---|---|---|
| `run_file` | Run a `.sema` or `.semac` file and get output/return value | `file_path` (string), `arguments` (array of strings, optional) |
| `compile` | Compile a `.sema` file to `.semac` bytecode | `source_path` (string), `output_path` (string, optional) |
| `eval` | Evaluate a single Sema expression string and capture output | `code` (string) |
| `docs` | Retrieve docstring and signature details for any symbol | `symbol` (string) |
| `fmt` | Format a Sema file or code string | `file_path` (string, optional), `code` (string, optional) |
| `disasm` | Disassemble a `.sema` or `.semac` file to VM instructions | `file_path` (string) |
| `build` | Compile a `.sema` file into a standalone executable | `source_path` (string), `output_path` (string) |
| `info` | Get environment and version information about the server | None |

### Path Resolution
All file paths passed to these tools are resolved relative to the current working directory (CWD) of the MCP server process. Both absolute and relative paths are supported.

---

## Filepath Mode & Custom Tools

You can expose custom tools defined in your Sema scripts to the LLM client by passing filepaths when starting the server:

```bash
sema mcp tools/receipts.sema
```

When started in filepath mode, the server instantiates the interpreter context, evaluates the specified files, and automatically exposes any tools defined via the `deftool` special form.

### Defining a Custom Tool: PDF Receipt Extractor

Here is a real-world example of an MCP tool that reads a receipt PDF and extracts structured data from it by combining Sema's [PDF processing](./stdlib/pdf) and [LLM Structured Extraction](./llm/extraction):

```sema
;; tools/receipts.sema

(deftool extract-receipt
  "Extract structured transaction data (merchant, amount, currency, date, line items) from a PDF invoice/receipt."
  {:pdf-path {:type :string :description "Path to the invoice/receipt PDF file (e.g. invoice.pdf)"}}
  (lambda (pdf-path)
    (if (not (file/exists? pdf-path))
        (error (string-append "Receipt file not found: " pdf-path))
        (begin
          (llm/auto-configure)
          ;; Extract text and clean up whitespace for LLM processing
          (define text (text/clean-whitespace (pdf/extract-text pdf-path)))
          ;; Call structured LLM extraction
          (llm/extract
            {:vendor {:type :string :description "Name of the merchant"}
             :amount {:type :number :description "Total bill amount"}
             :currency {:type :string :description "3-letter currency code (e.g. USD, EUR)"}
             :date {:type :string :description "Date of transaction in YYYY-MM-DD format"}
             :line-items {:type :array
                          :description "List of individual items purchased"
                          :items {:type :object
                                  :properties {:description {:type :string}
                                               :price {:type :number}}}}}
            text)))))
```

---

## Standalone Binary Mode

Sema's `build` command compiles scripts into standalone native executables. Every compiled standalone binary has built-in MCP server capabilities out-of-the-box:

```bash
# Compile the custom receipt tool
sema build tools/receipts.sema -o receipt-extractor

# Run as a normal CLI tool (from your shell/scripts)
./receipt-extractor --pdf-path invoice.pdf

# Start the stdio MCP server exposing the embedded tools
./receipt-extractor --mcp
```

When started with `--mcp`, the executable evaluates its embedded bytecode (which registers the `extract-receipt` tool definition in the environment) and then transitions to starting the stdio MCP server loop.

---

## Tool Filtering & Visibility

When loading files in filepath mode, you can control which tools are exposed to the LLM:

### 1. Private Prefix
Any tool whose name begins with an underscore (e.g., `_secret-helper`) is treated as a private helper and excluded from discovery.

### 2. Declarative Metadata
You can declare a tool as private by adding `:mcp/expose #f` or `:private #t` in its parameters metadata map:

```sema
(deftool internal-helper
  "Not visible to MCP clients"
  {:mcp/expose #f}
  (lambda () (println "internal")))
```

### 3. Command Line Filters
You can explicitly include or exclude tools using the `--include` and `--exclude` flags:

```bash
# Only expose the receipt extractor tool
sema mcp tools/receipts.sema --include extract-receipt

# Expose all tools except order-pineapple-pizza (which we probably shouldn't be running automatically)
sema mcp tools/receipts.sema --exclude order-pineapple-pizza
```

---

## Stateful Notebook Tools

The MCP server exposes a set of stateful notebook management and evaluation tools to allow LLMs to directly read, write, and execute cell-based `.sema-nb` files.

| Tool Name | Description | Parameters |
|---|---|---|
| `notebook/new` | Create a new empty `.sema-nb` notebook | `path` (string), `title` (string, optional), `overwrite` (boolean, optional — defaults to `false`; creation fails if a file already exists at `path`) |
| `notebook/read` | Read the structure, source, and outputs of a notebook | `path` (string) |
| `notebook/add_cell` | Append or insert a new cell (code or markdown) | `path` (string), `type` (string: "code"/"markdown"), `source` (string), `after_id` (string, optional) |
| `notebook/update_cell` | Update the source/type of an existing cell | `path` (string), `id` (string), `source` (string, optional), `type` (string, optional) |
| `notebook/delete_cell` | Delete a cell from a notebook | `path` (string), `id` (string) |
| `notebook/eval_cell` | Evaluate a single code cell | `path` (string), `id` (string) |
| `notebook/eval_all` | Evaluate all code cells in order | `path` (string) |
| `notebook/export` | Export a notebook to Markdown or a clean `.sema` script | `path` (string), `format` (string: "markdown"/"source"), `output_path` (string, optional) |

### In-Memory State Caching
To support interactive cell execution (where Cell 2 relies on variables or functions defined in Cell 1), the MCP server maintains an in-memory cache of notebook evaluation engines mapped by their canonical file paths.

When a cell is evaluated, the cached engine runs the code, updates the cell output, saves the updated JSON representation back to disk, and returns the result, ensuring state is preserved across consecutive tool calls.

---

## Sema as an MCP Client

The sections above cover Sema acting as an MCP **server**. Sema can also be an MCP **client** — connecting to an external MCP server and consuming its tools from Sema code, so you can use the wider MCP ecosystem (filesystem, GitHub, Slack, databases, hosted vendors like Asana/Linear, …) without hand-writing a `deftool` for each.

### Client builtins

| Function | Description |
|---|---|
| `mcp/connect` | Connect to a server (stdio or HTTP), run the `initialize` handshake, and return an opaque handle |
| `mcp/tools` | List the server's tools as descriptor maps (`{:name :description :input-schema}`) |
| `mcp/call` | Call a tool by name with an arguments map; returns the result (text collapses to a string) |
| `mcp/tools->sema` | Convert the server's tools into `deftool`-shaped values ready to hand to `defagent` |
| `mcp/close` | Disconnect (terminate the stdio child / end the HTTP session) and drop the handle |

`mcp/connect` chooses its transport from the config map: `:command` for a local **stdio** server, `:url` for a **remote** one.

### Transports

**stdio** — launch the server as a child process and exchange JSON-RPC 2.0 over its stdin/stdout. `:command` is required; `:args`, `:env`, and `:cwd` are optional. A server that needs a credential reads it from the environment Sema hands the child, so pass tokens through `:env`:

```scheme
(define fs (mcp/connect {:command "npx"
                         :args ["-y" "@modelcontextprotocol/server-filesystem" "/tmp"]}))

(mcp/tools fs)
; => ({:name "read_file" :description "…" :input-schema {…}} …)

(mcp/call fs "read_file" {:path "/tmp/notes.txt"})
; => "…file contents…"

(mcp/close fs)

;; A server that reads a token from its environment:
(define gh (mcp/connect {:command "github-mcp-server"
                         :env {"GITHUB_TOKEN" (env "GITHUB_TOKEN")}}))
```

**HTTP** — connect to a remote server by `:url`. Sema speaks the modern **Streamable HTTP** transport (MCP spec `2025-11-25`: a single endpoint, `Mcp-Session-Id` continuity, JSON-or-SSE responses) and **auto-detects and falls back** to the deprecated 2024-11-05 HTTP+SSE two-endpoint transport when a server only speaks that — so you use the same call either way:

```scheme
;; Open server, or a static bearer token you already have:
(define gh (mcp/connect {:url "https://mcp.example.com/mcp"
                         :headers {"Authorization" "Bearer ghp_…"}}))
```

### Authenticated remote servers (OAuth)

For a remote server that requires authorization, `mcp/connect` runs the standards-compliant **OAuth 2.1** flow automatically on the first `401` — no bespoke steps:

1. **Discover** the authorization server from the `WWW-Authenticate` challenge → Protected Resource Metadata (RFC 9728) → Authorization Server Metadata (RFC 8414 / OpenID Connect Discovery).
2. **Obtain a client id** — a pre-registered one you pass as `:auth {:client-id "…"}`, a cached one, or Dynamic Client Registration (RFC 7591).
3. **Authorize** with the Authorization-Code + **PKCE-S256** flow, binding the token to the server with `resource=` (RFC 8707), by **opening your browser** and capturing the redirect on a loopback listener (RFC 8252).
4. **Cache & refresh** — tokens are stored (OS keychain, or a `0600` file) and refreshed automatically, so later connects are silent.

```scheme
;; Browser opens on first use; subsequent runs reuse the cached token.
(define asana (mcp/connect {:url "https://mcp.asana.com/mcp"}))

;; Pin a pre-registered client when the server has no dynamic registration:
(define linear (mcp/connect {:url "https://mcp.linear.app/mcp"
                             :auth {:client-id "your-client-id"}}))
```

Prefer to authenticate ahead of time (recommended, and required on a headless box)? Use the CLI:

```bash
sema mcp login  https://mcp.example.com/mcp             # opens a browser
sema mcp login  https://mcp.example.com/mcp --device    # RFC 8628 device-code flow (headless)
sema mcp login  https://mcp.example.com/mcp --client-id ID   # pre-registered client
sema mcp logout https://mcp.example.com/mcp             # clear cached credentials
```

**Token storage.** Credentials are kept per server URL in the OS keychain by default, falling back to a `0600`-permission `mcp-auth.json` in the platform config directory on headless boxes (Linux: `$XDG_CONFIG_HOME` or `~/.config/sema/`; macOS: `~/Library/Application Support/sema/`; Windows: `%APPDATA%\sema\`). Override the backend with an environment variable:

```bash
export SEMA_MCP_TOKEN_STORE=file      # force the 0600 file store (no keychain prompts)
export SEMA_MCP_TOKEN_STORE=keychain  # force the OS keychain
```

> **Tip (dev):** a locally-built (ad-hoc-signed) `sema` binary gets a new code identity every time you rebuild, so macOS Keychain re-prompts after each `cargo build`. Set `SEMA_MCP_TOKEN_STORE=file` while iterating to avoid the prompts.

### Using MCP tools in an agent

`mcp/tools->sema` produces values structurally identical to what `deftool` yields, so an agent uses them exactly like local tools — no new agent concepts:

```scheme
(define asana (mcp/connect {:url "https://mcp.asana.com/mcp"}))

(defagent assistant
  {:model "claude-sonnet-5"
   :system "You are an Asana assistant. Use the tools to answer."
   :tools (mcp/tools->sema asana)
   :max-turns 8})

(agent/run assistant "List the tasks assigned to me, with due dates.")
```

A tool that reports `isError` surfaces as an error the agent loop feeds back to the model, so the agent can react to failures instead of treating them as success. A full runnable example lives at `examples/mcp/asana-tasks.sema`.

### Security

- **Capabilities.** A stdio connection **spawns a process** (`process` capability); an HTTP connection is **network I/O** (`network` capability). A sandbox that denies the relevant capability cannot open that kind of connection (see the [`--sandbox`](/docs/cli) flag).
- **Server authority.** The tools a server exposes run with the **server's** authority, not Sema's sandbox — connecting to an untrusted MCP server is equivalent to running untrusted code.
- **Untrusted output.** Tool descriptions and results are **data, not instructions**. They come from the server and can contain prompt-injection (e.g. "tell the user to reconnect to server X"). Treat them as untrusted input; never act on instructions embedded in tool output.

### Deterministic testing (cassettes)

MCP `tools/call` results record and replay through the same **cassette** tape as LLM calls, so an agent-over-MCP flow can be captured once and replayed offline (no network, no live server) in CI. Record a session, then replay it:

```scheme
;; Record: real calls run and their results are taped.
(llm/cassette-load "tape.ndjson" {:mode :record})
(define s (mcp/connect {:url "https://mcp.example.com/mcp"}))
(mcp/call s "search" {:q "hello"})
(llm/cassette-save)

;; Replay: the same call is served from the tape without touching the network.
(llm/cassette-load "tape.ndjson" {:mode :replay})
```

A call is keyed by a hash of the server identity + tool name + arguments; a replay with no matching entry is a hard "miss" so drift is caught. (`SEMA_LLM_CASSETTE=tape.ndjson SEMA_LLM_CASSETTE_MODE=replay` installs a tape process-wide for a test suite.)

### Troubleshooting

- **`OAuth login failed` / no browser opens** — on a headless machine, use `sema mcp login <url> --device` (device-code flow) or pass a token directly via `:headers {"Authorization" "Bearer …"}`.
- **Repeated macOS Keychain prompts while developing** — expected for a frequently-rebuilt dev binary; set `SEMA_MCP_TOKEN_STORE=file`.
- **`mcp/connect requires a :command or :url entry`** — the config map needs one of `:command` (stdio) or `:url` (http).
- Errors surface as `SemaError` values, so wrap `mcp/connect`/`mcp/call` in your usual error handling.
