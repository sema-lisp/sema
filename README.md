<p align="center">
  <img src="https://raw.githubusercontent.com/helgesverre/sema/main/assets/og-github.jpg" alt="Sema — Stop rewriting the agent loop." width="800">
</p>

<p align="center">
  A Lisp with first-class LLM primitives, implemented in Rust.
</p>

<p align="center">
  <a href="https://sema.run"><img src="https://img.shields.io/badge/try_it-sema.run-c8a855?style=flat" alt="Playground"></a>
  <a href="https://sema-lang.com/docs/"><img src="https://img.shields.io/badge/docs-sema--lang.com-c8a855?style=flat" alt="Docs"></a>
  <a href="https://github.com/HelgeSverre/sema/releases/latest"><img src="https://img.shields.io/github/v/tag/HelgeSverre/sema?label=version&color=c8a855&style=flat" alt="Version"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-c8a855?style=flat" alt="License"></a>
</p>

**Stop rewriting the agent loop.** Every LLM script grows the same scaffolding — retries, caching, cost caps, rate limits, tool dispatch, conversation state. Sema makes that scaffolding the runtime: your script stays the size of its idea, ships as a single binary, and your coding agent already speaks the language.

Sema is a Scheme-like Lisp where **prompts are s-expressions**, **conversations are persistent data structures**, and **LLM calls are just another form of evaluation** — with Clojure-style keywords (`:foo`), map literals (`{:key val}`), and vector literals (`[1 2 3]`).

## What It Looks Like

A coding agent with file tools, safety checks, and budget tracking — in ~40 lines:

```scheme
;; Define tools the LLM can call
(deftool read-file
  "Read a file's contents"
  {:path {:type :string :description "File path"}}
  (lambda (path)
    (if (file/exists? path) (file/read path) "File not found")))

(deftool edit-file
  "Replace text in a file"
  {:path {:type :string} :old {:type :string} :new {:type :string}}
  (lambda (path old new)
    (file/write path (string/replace (file/read path) old new))
    "Done"))

(deftool run-command
  "Run a shell command"
  {:command {:type :string :description "Shell command to run"}}
  (lambda (command) (:stdout (shell "sh" "-c" command))))

;; Create an agent with tools, system prompt, and spending limit
(defagent coder
  {:system (format "You are a coding assistant. Working directory: ~a" (sys/cwd))
   :tools [read-file edit-file run-command]
   :model "claude-sonnet-4-20250514"
   :max-turns 20})

;; Run it — budget is scoped, automatically restored after the block
(llm/with-budget {:max-cost-usd 0.50} (lambda ()
  (define result (agent/run coder "Add error handling to src/main.rs"))
  (println (:response result))
  (println (format "Cost: $~a" (:spent (llm/budget-remaining))))))
```

## Key Features

```scheme
;; Simple completion
(llm/complete "Explain monads in one sentence")

;; Structured data extraction — returns a map, not a string
(llm/extract
  {:vendor {:type :string} :amount {:type :number} :date {:type :string}}
  "Bought coffee for $4.50 at Blue Bottle on Jan 15")
;; => {:amount 4.5 :date "2025-01-15" :vendor "Blue Bottle"}

;; Classification
(llm/classify (list :positive :negative :neutral) "This product is amazing!")
;; => :positive

;; Multi-turn conversations as immutable data
(define conv (conversation/new {:model "claude-haiku-4-5-20251001"}))
(define conv (conversation/say conv "The secret number is 7"))
(define conv (conversation/say conv "What's the secret number?"))
(conversation/last-reply conv) ;; => "The secret number is 7."

;; Streaming
(llm/stream "Tell me a story" {:max-tokens 500})

;; Batch — all prompts sent concurrently
(llm/batch (list "Translate 'hello' to French"
                 "Translate 'hello' to Spanish"
                 "Translate 'hello' to German"))

;; Vision — extract structured data from images
(llm/extract-from-image
  {:text :string :background_color :string}
  "assets/logo.png")
;; => {:background_color "white" :text "Sema"}

;; Multi-modal chat — send images in messages
(define img (file/read-bytes "photo.jpg"))
(llm/chat (list (message/with-image :user "Describe this image." img)))

;; Cost tracking
(llm/set-budget 1.00)
(llm/budget-remaining) ;; => {:limit 1.0 :spent 0.05 :remaining 0.95}

;; Response caching — avoid duplicate API calls during development
(llm/with-cache (lambda ()
  (llm/complete "Explain monads")))

;; Fallback chains — automatic provider failover
(llm/with-fallback [:anthropic :openai :groq]
  (lambda () (llm/complete "Hello")))

;; In-memory vector store for semantic search (RAG)
(vector-store/create "docs")
(vector-store/add "docs" "id" (llm/embed "text") {:source "file.txt"})
(vector-store/search "docs" (llm/embed "query") 5)

;; Text chunking for LLM pipelines
(text/chunk long-document {:size 500 :overlap 100})

;; Prompt templates
(prompt/render "Hello {{name}}" {:name "Alice"})
; => "Hello Alice"

;; Persistent key-value store
(kv/open "cache" "cache.json")
(kv/set "cache" "key" {:data "value"})
(kv/get "cache" "key")
```

## Supported Providers

All providers are auto-configured from environment variables — just set the API key and go.

| Provider              | Chat | Stream | Tools | Embeddings | Vision |
| --------------------- | ---- | ------ | ----- | ---------- | ------ |
| **Anthropic**         | ✅   | ✅     | ✅    | —          | ✅     |
| **OpenAI**            | ✅   | ✅     | ✅    | ✅         | ✅     |
| **Google Gemini**     | ✅   | ✅     | ✅    | —          | ✅     |
| **Ollama**            | ✅   | ✅     | ✅    | —          | ✅     |
| **Groq**              | ✅   | ✅     | ✅    | —          | —      |
| **xAI**               | ✅   | ✅     | ✅    | —          | —      |
| **Mistral**           | ✅   | ✅     | ✅    | —          | —      |
| **Moonshot**          | ✅   | ✅     | ✅    | —          | —      |
| **Jina**              | —    | —      | —     | ✅         | —      |
| **Voyage**            | —    | —      | —     | ✅         | —      |
| **Cohere**            | —    | —      | —     | ✅         | —      |
| **Any OpenAI-compat** | ✅   | ✅     | ✅    | —          | ✅     |
| **Custom (Lisp)**     | ✅   | —      | ✅    | —          | —      |

## It's Also a Real Lisp

Hundreds of built-in functions, tail-call optimization, macros, modules, error handling — not a toy.

```scheme
;; Closures, higher-order functions, TCO
(define (fibonacci n)
  (let loop ((i 0) (a 0) (b 1))
    (if (= i n) a (loop (+ i 1) b (+ a b)))))
(fibonacci 50) ;; => 12586269025

;; Maps, keywords-as-functions, f-strings
(define person {:name "Ada" :age 36 :langs ["Lisp" "Rust"]})
(:name person) ;; => "Ada"
(println f"${(:name person)} knows ${(length (:langs person))} languages")

;; Destructuring
(let (({:keys [name age]} person))
  (println f"${name} is ${age}"))

;; Pattern matching with guards
(define (classify n)
  (match n
    (x when (> x 100) "big")
    (x when (> x 0)   "small")
    (_                 "non-positive")))

;; Functional pipelines
(->> (range 1 100)
     (filter even?)
     (map #(* % %))
     (take 5))
;; => (4 16 36 64 100)

;; Nested data access
(define config {:db {:host "localhost" :port 5432}})
(get-in config [:db :host])  ;; => "localhost"

;; Macros
(defmacro unless (test . body)
  `(if ,test nil (begin ,@body)))

;; Modules
(module utils (export square)
  (define (square x) (* x x)))

;; HTTP, JSON, regex, file I/O, crypto, CSV, datetime...
(define data (json/decode (http/get "https://api.example.com/data")))
```

> 📖 Full language reference, stdlib docs, and more examples at **[sema-lang.com/docs](https://sema-lang.com/docs/)**

## Try It Now

> **[sema.run](https://sema.run)** — Browser-based playground with 20+ example programs.
> No install required. Runs entirely in WebAssembly.

## Installation

Install pre-built binaries (no Rust required):

```bash
# macOS / Linux
curl -fsSL https://sema-lang.com/install.sh | sh

# Windows (PowerShell)
powershell -ExecutionPolicy ByPass -c "irm https://github.com/HelgeSverre/sema/releases/latest/download/sema-lang-installer.ps1 | iex"

# Homebrew (macOS / Linux)
brew install helgesverre/tap/sema-lang
```

Or install from [crates.io](https://crates.io/crates/sema-lang):

```bash
cargo install sema-lang
```

Or build from source:

```bash
git clone https://github.com/HelgeSverre/sema
cd sema && cargo build --release
# Binary at target/release/sema
```

```bash
sema                          # REPL (with tab completion)
sema script.sema              # Run a file
sema -e '(+ 1 2)'             # Evaluate expression
sema --no-llm script.sema     # Run without LLM (faster startup)
sema build app.sema -o myapp  # Build standalone executable
./myapp                       # Run without sema installed
```

### Shell Completions

Generate tab-completion scripts for your shell:

```bash
# Zsh (macOS / Linux)
mkdir -p ~/.zsh/completions
sema completions zsh > ~/.zsh/completions/_sema

# Bash
mkdir -p ~/.local/share/bash-completion/completions
sema completions bash > ~/.local/share/bash-completion/completions/sema

# Fish
sema completions fish > ~/.config/fish/completions/sema.fish
```

> 📖 Full setup instructions for all shells: **[sema-lang.com/docs/shell-completions](https://sema-lang.com/docs/shell-completions.html)**

> 📖 Full CLI reference, flags, and REPL commands: **[sema-lang.com/docs/cli](https://sema-lang.com/docs/cli.html)**

### Editor Support

| Editor           | Install                                                                                   |
| ---------------- | ----------------------------------------------------------------------------------------- |
| **VS Code**      | See [install instructions](https://sema-lang.com/docs/editors.html)                       |
| **Zed**          | Install Dev Extension → select `editors/zed`                                              |
| **Vim / Neovim** | `Plug 'helgesverre/sema', { 'rtp': 'editors/vim' }`                                       |
| **Emacs**        | `(require 'sema-mode)` — see [docs](https://sema-lang.com/docs/editors.html)              |
| **Helix**        | Copy `languages.toml` + query files — see [docs](https://sema-lang.com/docs/editors.html) |

All editors provide syntax highlighting for builtins, special forms, keyword literals, character literals, LLM primitives, and more.

> 📖 Full installation instructions: **[sema-lang.com/docs/editors](https://sema-lang.com/docs/editors.html)**

### Notebook

Sema includes a Jupyter-inspired notebook interface with a browser UI:

```bash
sema notebook new my-notebook.sema-nb        # Create a notebook
sema notebook serve my-notebook.sema-nb      # Open in browser (localhost:8888)
sema notebook run my-notebook.sema-nb        # Run all cells headlessly
sema notebook export my-notebook.sema-nb     # Export to Markdown
```

Cells share a persistent environment — definitions in earlier cells are visible in later ones. Notebooks are saved as `.sema-nb` JSON files.

> 📖 Full notebook documentation: **[sema-lang.com/docs/notebook](https://sema-lang.com/docs/notebook.html)**

### Language Tooling

A full toolchain ships in the box — no plugins to assemble:

```bash
sema fmt script.sema     # Canonical code formatter
sema lsp                 # Language Server (completions, hover, go-to-def, rename)
sema dap                 # Debug Adapter (breakpoints, stepping, variable inspection)
sema mcp                 # Model Context Protocol server for LLM clients
```

The **MCP server** lets LLM clients (Claude Desktop, Cursor, Claude Code) compile, format, evaluate, and build Sema code — and call your own `deftool` Lisp tools — directly in your environment.

> 📖 [Formatter](https://sema-lang.com/docs/formatter.html) · [LSP](https://sema-lang.com/docs/lsp.html) · [Debugger](https://sema-lang.com/docs/dap.html) · [MCP](https://sema-lang.com/docs/mcp.html)

## Example Programs

The [`examples/`](https://github.com/helgesverre/sema/tree/main/examples) directory has 50+ programs:

| Example                                                                                                       | What it does                                                 |
| ------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| [`coding-agent.sema`](https://github.com/helgesverre/sema/blob/main/examples/ai-tools/coding-agent.sema)      | Full coding agent with file editing, search, and shell tools |
| [`review.sema`](https://github.com/helgesverre/sema/blob/main/examples/ai-tools/review.sema)                  | AI code reviewer for git diffs                               |
| [`commit-msg.sema`](https://github.com/helgesverre/sema/blob/main/examples/ai-tools/commit-msg.sema)          | Generate conventional commit messages from staged changes    |
| [`summarize.sema`](https://github.com/helgesverre/sema/blob/main/examples/ai-tools/summarize.sema)            | Summarize files or piped input                               |
| [`game-of-life.sema`](https://github.com/helgesverre/sema/blob/main/examples/game-of-life.sema)               | Conway's Game of Life                                        |
| [`brainfuck.sema`](https://github.com/helgesverre/sema/blob/main/examples/brainfuck.sema)                     | Brainfuck interpreter                                        |
| [`mandelbrot.sema`](https://github.com/helgesverre/sema/blob/main/examples/mandelbrot.sema)                   | ASCII Mandelbrot set                                         |
| [`json-api.sema`](https://github.com/helgesverre/sema/blob/main/examples/json-api.sema)                       | Fetch and process JSON APIs                                  |
| [`test-vision.sema`](https://github.com/helgesverre/sema/blob/main/examples/llm/test-vision.sema)             | Vision extraction and multi-modal chat tests                 |
| [`test-extract.sema`](https://github.com/helgesverre/sema/blob/main/examples/llm/test-extract.sema)           | Structured extraction and classification                     |
| [`test-batch.sema`](https://github.com/helgesverre/sema/blob/main/examples/llm/test-batch.sema)               | Batch/parallel LLM completions                               |
| [`test-pipeline.sema`](https://github.com/helgesverre/sema/blob/main/examples/llm/test-pipeline.sema)         | Caching, budgets, rate limiting, retry, fallback chains      |
| [`test-text-tools.sema`](https://github.com/helgesverre/sema/blob/main/examples/llm/test-text-tools.sema)     | Text chunking, prompt templates, document abstraction        |
| [`test-vector-store.sema`](https://github.com/helgesverre/sema/blob/main/examples/llm/test-vector-store.sema) | In-memory vector store with similarity search                |
| [`test-kv-store.sema`](https://github.com/helgesverre/sema/blob/main/examples/llm/test-kv-store.sema)         | Persistent JSON-backed key-value store                       |
| [`expr-evaluator.sema`](https://github.com/helgesverre/sema/blob/main/examples/expr-evaluator.sema)           | Mini calculator using `match` on tagged vectors              |
| [`shape-geometry.sema`](https://github.com/helgesverre/sema/blob/main/examples/shape-geometry.sema)           | Shape areas/perimeters with map pattern matching             |
| [`http-router.sema`](https://github.com/helgesverre/sema/blob/main/examples/http-router.sema)                 | HTTP router with `match` on nested maps and guards           |
| [`destructuring.sema`](https://github.com/helgesverre/sema/blob/main/examples/destructuring.sema)             | Comprehensive destructuring showcase (vector, map, lambda)   |
| [`demo.sema-nb`](https://github.com/helgesverre/sema/blob/main/examples/notebook/demo.sema-nb)               | Interactive notebook demo (run with `sema notebook serve`)   |

## Why Sema?

- **LLMs as language primitives** — prompts, messages, conversations, tools, and agents are first-class data types, not string templates bolted on
- **Multi-provider** — swap between Anthropic, OpenAI, Gemini, Ollama, any OpenAI-compatible endpoint, or define your own provider in Sema
- **Pipeline-ready** — response caching, fallback chains, rate limiting, retry with backoff, text chunking, prompt templates, vector store, and a persistent KV store
- **Cost-aware** — built-in budget tracking with a bundled pricing snapshot ([models.dev](https://models.dev)), updated per release
- **Observable** — every LLM/agent run is auto-traced with OpenTelemetry (GenAI semantic conventions): tokens, cost, latency, and the full `invoke_agent → chat → execute_tool` tree, exportable to Jaeger, Grafana, Datadog, Langfuse, Arize Phoenix, and more — zero manual instrumentation, off by default
- **Practical Lisp** — closures, TCO, macros, modules, error handling, HTTP, file I/O, regex, JSON, and a comprehensive stdlib
- **Standalone executables** — `sema build` compiles programs into self-contained binaries with auto-traced imports and bundled assets
- **Embeddable** — [available on crates.io](https://crates.io/crates/sema-lang), clean Rust crate structure with a builder API
- **Full toolchain** — formatter, language server (LSP), debugger (DAP), and an MCP server for LLM clients, all built in
- **Developer-friendly** — REPL with tab completion, structured error messages with hints, and 50+ example programs

### Why Not Sema?

- No full numeric tower (rationals, bignums, complex numbers)
- No continuations (`call/cc`) or fully hygienic macros (`syntax-rules`) — has auto-gensym (`foo#`) for preventing variable capture
- Single-threaded — `Rc`-based, no cross-thread sharing of values
- No JIT — bytecode compiler + stack-based VM, no native code generation
- Package manager is git-based — central registry not yet live
- Young language — solid but not battle-tested at scale

## Architecture

```
crates/
  sema-core/     NaN-boxed Value type, errors, environment
  sema-reader/   Lexer and s-expression parser
  sema-vm/       Bytecode compiler and virtual machine
  sema-eval/     Trampoline-based evaluator, special forms, modules
  sema-stdlib/   Built-in functions across many modules
  sema-llm/      LLM provider trait + multi-provider clients
  sema-docs/     Canonical builtin docs (powers LSP hover + REPL apropos)
  sema-lsp/      Language Server Protocol implementation
  sema-dap/      Debug Adapter Protocol server
  sema-fmt/      Source code formatter
  sema-mcp/      Model Context Protocol server
  sema-notebook/ Jupyter-inspired notebook interface with browser UI
  sema-wasm/     WebAssembly build for sema.run playground
  sema/          CLI binary: REPL + file runner + standalone builder
```

> 🔬 Deep-dive into the internals: [Architecture](https://sema-lang.com/docs/internals/architecture.html) · [Evaluator](https://sema-lang.com/docs/internals/evaluator.html) · [Lisp Comparison](https://sema-lang.com/docs/internals/lisp-comparison.html)

## License

MIT — see [LICENSE](https://github.com/helgesverre/sema/blob/main/LICENSE).
