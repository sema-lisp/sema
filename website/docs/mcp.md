---
outline: [2, 3]
---

# Model Context Protocol (MCP)

Sema includes a built-in [Model Context Protocol](https://modelcontextprotocol.io/) server. This allows LLM clients (such as Claude Desktop, Cursor, or Claude Code) to inspect, compile, format, evaluate, and build Sema code in your host environment, as well as execute user-defined Lisp tools.

The server communicates over standard input/output (stdio) using JSON-RPC 2.0.

```bash
sema mcp
```

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
| `notebook/new` | Create a new empty `.sema-nb` notebook | `path` (string), `title` (string, optional) |
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
