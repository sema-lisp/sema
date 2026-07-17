---
outline: [2, 3]
---

# CLI Reference

```
sema [OPTIONS] [FILE] [-- SCRIPT_ARGS...]
```

## Flags & Options

| Flag                 | Description                                  |
| -------------------- | -------------------------------------------- |
| `-e, --eval <EXPR>`  | Evaluate expression, print result if non-nil |
| `-p, --print <EXPR>` | Evaluate expression, always print result     |
| `-l, --load <FILE>`  | Load file(s) before executing (repeatable)   |
| `-q, --quiet`        | Suppress REPL banner                         |
| `-i, --interactive`  | Enter REPL after running file or eval        |
| `--no-llm`           | Disable LLM features (skip provider auto-configuration) |
| `--chat-model <NAME>`       | Set default chat model                |
| `--chat-provider <NAME>`    | Set chat provider                     |
| `--embedding-model <NAME>`  | Set embedding model                   |
| `--embedding-provider <NAME>` | Set embedding provider              |
| `--sandbox <MODE>`   | Restrict dangerous operations (see below)    |
| `-V, --version`      | Print version                                |
| `-h, --help`         | Print help                                   |

## Subcommands

### `sema ast`

Parse source into an AST tree.

```
sema ast [OPTIONS] [FILE]
```

| Flag                | Description                      |
| ------------------- | -------------------------------- |
| `-e, --eval <EXPR>` | Parse expression instead of file |
| `--json`            | Output AST as JSON               |

### `sema eval`

Evaluate Sema code and return results. Designed for machine consumption (editor/LSP integration).

```
sema eval [OPTIONS]
```

| Flag                  | Description                                                      |
| --------------------- | ---------------------------------------------------------------- |
| `--stdin`             | Read program from stdin                                          |
| `--expr <CODE>`       | Evaluate a single expression                                     |
| `--json`              | Emit JSON result envelope                                        |
| `--path <FILE>`       | Set file context for imports and error spans                     |
| `--sandbox <MODE>`    | Sandbox mode (`strict`, `all`, or comma-separated capabilities)  |
| `--no-llm`            | Disable LLM features                                             |
| `--timeout <MS>`      | Kill evaluation after N ms (default: 5000)                       |

**Examples:**

```bash
# Evaluate an expression
sema eval --expr "(+ 1 2)"

# Read from stdin (avoids shell quoting issues)
echo '(* 6 7)' | sema eval --stdin

# JSON output for programmatic use
sema eval --expr "(+ 1 2)" --json
# => {"ok":true,"value":"3","error":null,"elapsedMs":0}

# Multi-form context: defines are available to later expressions
echo '(define pi 3.14) (define (area r) (* pi r r)) (area 10)' | sema eval --stdin --json
# => {"ok":true,"value":"314.0","error":null,"elapsedMs":0}

# Sandboxed evaluation (used by LSP)
sema eval --expr "(+ 1 2)" --json --sandbox strict --no-llm
```

**JSON envelope format:**

```json
{
  "ok": true,
  "value": "42",
  "stdout": "",
  "stderr": "",
  "error": null,
  "elapsedMs": 12
}
```

Output from `print`/`println`/`display` is captured into `stdout`; `print-error`/`println-error` into `stderr`. These fields are always present (empty string when no output).

On error:

```json
{
  "ok": false,
  "value": null,
  "stdout": "",
  "stderr": "",
  "error": {
    "message": "Unbound variable: foo",
    "hint": "Did you mean 'for'?",
    "line": 3,
    "col": 5
  },
  "elapsedMs": 2
}
```

### `sema compile`

Compile a source file to a `.semac` bytecode file. The compiled file can be executed directly with `sema` (auto-detected via magic number). See [Bytecode File Format](./internals/bytecode-format.md) for details on the format.

::: info Imports resolve at runtime
`sema compile` only compiles the specified file — it does not bundle dependencies. When you run the `.semac` file, `(import ...)` and `(load ...)` are resolved from the filesystem at runtime. All imported packages must be installed on the target machine. For a fully self-contained artifact, use [`sema build`](#sema-build) instead.
:::

```
sema compile [OPTIONS] <FILE>
```

| Flag                  | Description                                          |
| --------------------- | ---------------------------------------------------- |
| `-o, --output <FILE>` | Output file path (default: input with `.semac` extension) |
| `--check`             | Validate a `.semac` file without executing            |

```bash
# Compile to bytecode
sema compile script.sema                   # → script.semac
sema compile -o output.semac script.sema   # explicit output path

# Run the compiled bytecode (auto-detected)
sema script.semac

# Validate a bytecode file
sema compile --check script.semac
# ✓ script.semac: valid (format v1, sema 1.6.2, 3 functions, 847 bytes)
```

### `sema build`

Build a standalone executable from a Sema source file. The resulting binary embeds the compiled bytecode, all transitive imports, and any explicitly included assets into a self-contained executable. See [Executable Format](./internals/executable-format.md) for details on the binary format.

```
sema build [OPTIONS] [FILE]
```

| Flag                     | Description                                               |
| ------------------------ | --------------------------------------------------------- |
| `-o, --output <PATH>`   | Output path. A file path is used as-is; a directory (existing, or ending in `/`) means "default filename inside it". Missing parent directories are created; a leading `~` is expanded. |
| `--include <PATH>...`   | Additional files or directories to bundle (repeatable)    |
| `--runtime <PATH>`      | Path to a sema executable to embed the program into, instead of the current executable or the release binary `--target` downloads. The output inherits its platform and version. Conflicts with `--target`. |
| `--target <TARGET>`     | Target platform triple or alias (e.g. `linux`, `macos`, `windows`, `web`, or a full triple like `x86_64-unknown-linux-gnu`). Use `all` to build every supported native target. |
| `--list-targets`        | Show all supported target platforms and aliases            |
| `--no-cache`            | Force re-download of cached runtime binaries              |
| `-v, --verbose`         | Show per-step build detail and runtime cache/checksum info |
| `--json`                | Print a machine-readable build manifest to stdout          |

```bash
# Build a standalone executable
sema build script.sema                        # → ./script
sema build script.sema -o myapp              # explicit output path

# Bundle additional files
sema build script.sema --include data.json   # bundle a file
sema build script.sema --include assets/     # bundle a directory

# Cross-compile for other platforms
sema build script.sema --target linux        # build for Linux (x86_64)
sema build script.sema --target windows      # build for Windows
sema build script.sema --target all          # build for all supported targets
sema build script.sema --target linux --no-cache  # force re-download runtime

# Run the standalone executable
./myapp --arg1 --arg2
```

With `--target all` the program is compiled once and each per-target executable
gets a distinct suffixed filename (`<name>-<triple>`, `.exe` for Windows) —
combined with `-o` a directory means "put them all in here", a file path is used
as the base name. A summary table with sizes and full paths is printed on
stdout when done:

```
Built 5/5 targets in 14.4s:

  macos    arm64    27.9 MB   /home/me/dist/game-aarch64-apple-darwin
  linux    arm64    28.0 MB   /home/me/dist/game-aarch64-unknown-linux-gnu
  macos    x86_64   28.5 MB   /home/me/dist/game-x86_64-apple-darwin
  linux    x86_64   31.0 MB   /home/me/dist/game-x86_64-unknown-linux-gnu
  windows  x86_64   23.9 MB   /home/me/dist/game-x86_64-pc-windows-msvc.exe
```

Progress goes to stderr and the final summary to stdout, so the output is
pipeable. `--json` replaces the summary with a manifest — per target: `path`,
`bytes`, `sha256`, the runtime source (`host` / `cached` / `downloaded` /
`custom`), and `ok`/`error` status — handy for release scripts:

```bash
sema build app.sema --target all --json | jq -r '.targets[].path'
```

Windows executables are built with an embedded Sema icon and a `VERSIONINFO`
resource (name + version in Explorer's Details tab), also when cross-building
from macOS or Linux.

Cross-compilation downloads pre-built runtime binaries from GitHub Releases and caches them at `~/.sema/cache/runtimes/`. Use `--no-cache` to force a fresh download, or `--runtime` to provide your own binary.

#### Using a custom runtime source

If you maintain a fork of Sema or host runtime binaries on your own infrastructure, set `SEMA_RUNTIME_BASE_URL` to point to a directory containing release archives and SHA256 checksums:

```bash
export SEMA_RUNTIME_BASE_URL=https://github.com/yourname/sema/releases/download/v1.11.0
sema build app.sema --target linux
```

The expected file layout at that URL is:

```
sema-lang-<target>.tar.xz          # Linux/macOS archive containing the sema binary
sema-lang-<target>.tar.xz.sha256   # SHA256 checksum (hex hash, optionally followed by filename)
sema-lang-<target>.zip             # Windows archive containing sema.exe
sema-lang-<target>.zip.sha256      # SHA256 checksum
```

Where `<target>` is a full triple like `x86_64-unknown-linux-gnu` or `aarch64-apple-darwin`. This matches the asset naming used by [cargo-dist](https://opensource.axo.dev/cargo-dist/), so forks using cargo-dist will work out of the box.

Alternatively, use `--runtime /path/to/sema` to skip downloading entirely and inject a local binary directly.

### `sema disasm`

Disassemble a compiled `.semac` bytecode file, printing a human-readable listing of the main chunk and all function templates.

```
sema disasm [OPTIONS] <FILE>
```

| Flag     | Description    |
| -------- | -------------- |
| `--json` | Output as JSON |

```bash
sema disasm script.semac          # human-readable text
sema disasm --json script.semac   # structured JSON output
```

### `sema pkg`

Package manager for installing, publishing, and managing Sema packages. Git-based packages work out of the box. Registry commands (`search`, `info`, `publish`, `yank`, `login`) require a running registry instance — see [Self-Hosted Registry](./packages.md#self-hosted-registry). See the full [Packages](./packages.md) documentation for details.

```
sema pkg <COMMAND>
```

| Subcommand                  | Description                                         |
| --------------------------- | --------------------------------------------------- |
| `init`                      | Initialize a new `sema.toml` in the current directory |
| `add <spec> [--registry]`   | Add a package from the registry or git              |
| `install [--locked]`        | Install all deps from `sema.toml` (`--locked` fails if `sema.lock` is missing or out of sync — for CI) |
| `update [name]`             | Update packages (all or specific)                   |
| `remove <name>`             | Remove an installed package                         |
| `list`                      | List installed packages                             |
| `publish [--registry]`      | Publish current package to the registry             |
| `search <query> [--registry]` | Search the registry for packages                 |
| `info <name> [--registry]`  | Show package info from the registry                 |
| `yank <name@version> [--registry]` | Yank a published version                     |
| `login [--token] [--registry]` | Authenticate with a registry                     |
| `logout`                    | Remove stored registry credentials                  |
| `config [key] [value]`      | View or set package manager configuration           |

```bash
# Install a registry package
sema pkg add http-helpers@1.0.0

# Install a git package
sema pkg add github.com/user/repo@v2.0

# Publish to the registry
sema pkg login --token sema_pat_...
sema pkg publish

# Search for packages
sema pkg search json

# Set default registry
sema pkg config registry.url https://my-registry.com
```

### `sema completions`

Generate shell completion scripts. See [Shell Completions](./shell-completions.md) for installation instructions.

```
sema completions [OPTIONS] <SHELL>
```

| Flag        | Description                                                        |
| ----------- | ------------------------------------------------------------------ |
| `--install` | Auto-detect the shell's completion directory and install the script |

Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`.

The `--install` flag is supported for Bash, Zsh, Fish, and Elvish. For PowerShell, use `sema completions powershell` and follow the manual installation steps in [Shell Completions](./shell-completions.md).

```bash
# Print completion script to stdout
sema completions zsh

# Auto-install to the correct directory
sema completions --install zsh
```

### `sema fmt`

Format Sema source files. See [Formatter](./formatter.md) for full documentation.

```
sema fmt [OPTIONS] [FILES...]
```

| Flag | Description |
| --- | --- |
| `--check` | Check formatting without writing (exit 1 if unformatted) |
| `--diff` | Print diff of changes |
| `--width <N>` | Max line width (default: `80`) |
| `--indent <N>` | Indentation width (default: `2`) |
| `--align` | Column-align consecutive similar forms |
| `--max-blank-lines <N>` | Max consecutive blank lines to keep (default: `1`) |
| `--json` | Output result as JSON (useful for editor integrations) |

```bash
# Format all .sema files recursively
sema fmt

# Check in CI
sema fmt --check

# Preview changes
sema fmt --diff
```

### `sema notebook`

Jupyter-inspired cell-based notebook interface with a browser UI. Notebooks are saved as `.sema-nb` JSON files. Cells share a persistent environment — definitions in earlier cells are visible in later ones.

```
sema notebook <COMMAND>
```

| Subcommand                              | Description                                       |
| --------------------------------------- | ------------------------------------------------- |
| `serve [FILE]`                          | Start the notebook server with browser UI          |
| `run <FILE>`                            | Run all cells headlessly (for CI/testing)          |
| `export <FILE>`                         | Export notebook to Markdown                        |
| `new <FILE>`                            | Create a new empty notebook                        |

#### `sema notebook serve`

```
sema notebook serve [OPTIONS] [FILE]
```

| Flag                | Description                                  |
| ------------------- | -------------------------------------------- |
| `--host <HOST>`     | Host address to bind to (default: `127.0.0.1`) |
| `-p, --port <PORT>` | Port to listen on (default: `8888`)          |

Opens a browser-based notebook at `http://localhost:8888`. If `FILE` doesn't exist, a new notebook is created. The UI supports:
- Code and markdown cells with Shift+Enter to evaluate
- Stdout capture — `println` output appears in cell output
- Collapsible output with execution timing
- Single-cell undo with environment rollback
- Between-cell insert buttons
- Keyboard shortcuts (Shift+Enter, Cmd+Enter, Cmd+S, Tab, Escape)

#### `sema notebook run`

```
sema notebook run [OPTIONS] <FILE>
```

| Flag              | Description                                          |
| ----------------- | ---------------------------------------------------- |
| `--cells <CELLS>` | Only run specific cells (1-based, comma-separated)   |

Evaluates all code cells in order without starting a browser. Useful for CI validation and batch execution.

#### `sema notebook export`

```
sema notebook export [OPTIONS] <FILE>
```

| Flag                  | Description                           |
| --------------------- | ------------------------------------- |
| `--format <FMT>`      | Output format (default: `md`)         |
| `-o, --output <FILE>` | Output file (default: stdout)         |

#### `sema notebook new`

```
sema notebook new [OPTIONS] <FILE>
```

| Flag              | Description                                      |
| ----------------- | ------------------------------------------------ |
| `-t, --title <T>` | Notebook title (default: filename stem)          |

```bash
# Create and open a notebook
sema notebook new my-project.sema-nb --title "My Project"
sema notebook serve my-project.sema-nb

# Run cells headlessly (CI / smoke test)
sema notebook run my-project.sema-nb

# Export to Markdown
sema notebook export my-project.sema-nb -o output.md
```

See the full [Notebook documentation](/docs/notebook) for details on the file format, UI features, and keyboard shortcuts.

### `sema lsp`

Start the Language Server Protocol (LSP) server. Communicates over stdio using the standard LSP JSON-RPC protocol.

```
sema lsp
```

Provides diagnostics, completion, hover, go-to-definition, and code lenses. See the [LSP documentation](/docs/lsp) for full feature details and editor setup instructions.

### `sema mcp`

Start the [Model Context Protocol](/docs/mcp) server (exposes Sema's tools to LLM clients), or manage **MCP client** authentication.

```
sema mcp [FILES]...                 # run the MCP server (optionally loading tool files)
sema mcp login  <url> [--device] [--client-id ID]
sema mcp logout <url>
```

- `sema mcp` (no subcommand) starts the stdio MCP **server** — see the [MCP documentation](/docs/mcp).
- `sema mcp login <url>` authenticates to a remote MCP **server** you want to *consume* and caches the OAuth token so later `mcp/connect` calls are silent. `--device` uses the headless device-code flow; `--client-id` supplies a pre-registered OAuth client.
- `sema mcp logout <url>` clears the cached credentials for a server.

```bash
sema mcp login https://mcp.asana.com/mcp
sema mcp logout https://mcp.asana.com/mcp
```

## Examples

```bash
# Parse a file into an AST tree
sema ast script.sema

# Parse an expression into JSON AST
sema ast -e '(+ 1 2)' --json

# Load a prelude before starting the REPL
sema -l prelude.sema

# Load helpers, then run a script
sema -l helpers.sema script.sema

# Run a script and drop into REPL to inspect state
sema -i script.sema

# Quick one-liner for shell pipelines
sema -p '(string/join (map str (range 10)) ",")'

# Run without LLM features (faster startup)
sema --no-llm script.sema

# Compile to bytecode and run
sema compile script.sema
sema script.semac

# Use a specific model
sema --chat-model claude-haiku-4-5-20251001 -e '(llm/complete "Hello!")'

# Run with shell commands disabled
sema --sandbox=no-shell script.sema

# Deny multiple capabilities
sema --sandbox=no-shell,no-network,no-fs-write script.sema

# Strict mode (no shell, fs-write, network, env-write, process, llm, serial)
sema --sandbox=strict script.sema

# Maximum restriction (deny all dangerous operations)
sema --sandbox=all script.sema

# Restrict file operations to specific directories
sema --allowed-paths=./data,./output script.sema

# Combine sandbox and path restrictions
sema --sandbox=strict --allowed-paths=./data script.sema
```

## Shebang Scripts

Sema supports `#!` (shebang) lines, so you can write executable scripts:

```sema
#!/usr/bin/env sema
(println "Hello from a sema script!")
```

Make the file executable and run it directly:

```bash
chmod +x script.sema
./script.sema
```

The shebang line is only allowed on the first line of a file and is treated as a comment. `#!/usr/bin/env sema` uses the standard `env` lookup, so it works regardless of how sema was installed (Homebrew, Cargo, manual, etc.).

## Sandbox

The `--sandbox` flag restricts access to dangerous operations. Functions remain callable but return a `PermissionDenied` error when invoked.

### Modes

| Mode            | Description                                                            |
| --------------- | ---------------------------------------------------------------------- |
| `none`          | Deny no capabilities                                                    |
| `strict`        | Deny shell, fs-write, network, env-write, process, llm, serial (reads allowed) |
| `all`           | Deny all capabilities                                                  |
| Comma-separated | e.g. `no-shell,no-network` — deny specific capabilities                |

### Capabilities

Capability names may be written with or without the `no-` prefix. For example,
`no-network` and `network` both deny the `network` capability. The `no-*`
form is preferred in examples because `--sandbox` is a denial list.

The table below lists every function gated by each capability. It mirrors the `register_fn_gated` / `register_fn_path_gated` call sites in the stdlib — if a function is not listed it is never sandboxed.

| Capability  | Functions affected                                                         |
| ----------- | -------------------------------------------------------------------------- |
| `shell`     | `shell` (also requires `process`)                                          |
| `fs-read`   | `file/read`, `file/read-bytes`, `file/read-lines`, `file/for-each-line`, `file/fold-lines`, `file/exists?`, `file/list`, `file/info`, `file/is-file?`, `file/is-directory?`, `file/is-symlink?`, `file/glob`, `path/absolute`, `load`, `pdf/extract-text`, `pdf/extract-text-pages`, `pdf/page-count`, `pdf/metadata`, `stream/open-input`, `http/file`, `db/query`, `db/query-one`, `db/last-insert-id`, `db/tables`, `db/open-memory` |
| `fs-write`  | `file/write`, `file/write-bytes`, `file/write-lines`, `file/append`, `file/delete`, `file/rename`, `file/mkdir`, `file/copy`, `stream/open-output`, `kv/open`, `kv/set`, `kv/delete`, `db/open`, `db/exec`, `db/exec-batch` |
| `network`   | `http/get`, `http/post`, `http/put`, `http/delete`, `http/request`, `http/serve` |
| `env-read`  | `env`, `sys/env-all`, `sys/cwd`, `sys/home-dir`, `sys/user`, `sys/temp-dir` |
| `env-write` | `sys/set-env`                                                              |
| `process`   | `exit`, `sys/pid`, `sys/args`, `sys/which`, `shell`                        |
| `llm`       | `llm/complete`, `llm/chat`, `llm/send`, `llm/extract-from-image`           |
| `serial`    | `serial/list`, `serial/open`, `serial/close`, `serial/write`, `serial/read-line`, `serial/send` |

`shell` is the only function gated by two capabilities — it requires both `shell` (to launch a system shell) and `process` (because it spawns a child process). Denying either blocks it.

Functions not listed (arithmetic, strings, lists, maps, `println`, `path/join`, `sys/platform`, `sys/arch`, `sys/os`, `sys/hostname`, `sys/sema-home`, `time/now-ms`, etc.) are never restricted.

### Path Restrictions

The `--allowed-paths` flag restricts all file operations to specific directories. Paths are canonicalized, so traversal attacks like `../../etc/passwd` are blocked.

```bash
# Only allow reading/writing within ./project and /tmp
sema --allowed-paths=./project,/tmp script.sema
```

When `--allowed-paths` is set, any file operation (`file/read`, `file/write`, `file/list`, etc.) targeting a path outside the allowed directories returns a `PermissionDenied` error. This works independently of `--sandbox` — you can use both together:

```bash
# Allow filesystem but only within ./data
sema --sandbox=no-shell,no-network --allowed-paths=./data script.sema
```

## Environment Variables

| Variable             | Description                                           |
| -------------------- | ----------------------------------------------------- |
| `ANTHROPIC_API_KEY`  | Anthropic API key (auto-detected)                     |
| `OPENAI_API_KEY`     | OpenAI API key (auto-detected)                        |
| `GROQ_API_KEY`       | Groq API key (auto-detected)                          |
| `XAI_API_KEY`        | xAI/Grok API key (auto-detected)                      |
| `MISTRAL_API_KEY`    | Mistral API key (auto-detected)                       |
| `MOONSHOT_API_KEY`   | Moonshot API key (auto-detected)                      |
| `GOOGLE_API_KEY`     | Google Gemini API key (auto-detected)                 |
| `OLLAMA_HOST`        | Ollama server URL (default: `http://localhost:11434`) |
| `JINA_API_KEY`       | Jina embeddings API key (auto-detected)               |
| `VOYAGE_API_KEY`     | Voyage embeddings API key (auto-detected)             |
| `COHERE_API_KEY`     | Cohere embeddings API key (auto-detected)             |
| `SEMA_HOME`          | Override Sema home directory (default: `~/.sema`)     |
| `SEMA_CHAT_MODEL`    | Default chat model name                               |
| `SEMA_CHAT_PROVIDER` | Preferred chat provider                               |
| `SEMA_EMBEDDING_MODEL` | Default embedding model name                        |
| `SEMA_EMBEDDING_PROVIDER` | Preferred embedding provider                    |
| `SEMA_REGISTRY_URL`  | Override default package registry URL                 |
| `SEMA_RUNTIME_BASE_URL` | Override base URL for cross-compilation runtime downloads |
| `SEMA_MCP_TOKEN_STORE` | MCP client token backend: `file` (0600 file) or `keychain` (OS keychain). Default: keychain when available, else file. |
| `NO_COLOR`           | Disable colored output when set                       |



## REPL Commands

| Command        | Description                          |
| -------------- | ------------------------------------ |
| `,quit` / `,q` | Exit the REPL                        |
| `,help` / `,h` | Show help                            |
| `,env`         | Show user-defined bindings           |
| `,builtins`    | List all built-in functions          |
| `,type EXPR`   | Evaluate expression and show its type |
| `,time EXPR`   | Evaluate expression and show elapsed time |
| `,doc NAME`    | Show info about a binding or special form |

```
sema> ,type 42
:integer

sema> ,type '(1 2 3)
:list

sema> ,doc map
  map : native-fn

sema> ,doc if
  if : special form

sema> ,doc factorial
  factorial : lambda (n)

sema> ,time (foldl + 0 (range 100000))
4999950000
elapsed: 58.424ms
```

## REPL Features

### Tab Completion

The REPL supports tab completion for:

- All built-in function names (e.g., `string/tr` → `string/trim`)
- Special forms (`def` → `define`, `defun`, `defmacro`, ...)
- User-defined bindings
- REPL commands (`,` → `,quit`, `,help`, `,env`, `,builtins`, `,type`, `,time`, `,doc`)

### Multiline Input

The REPL automatically detects incomplete expressions (unbalanced parentheses) and continues on the next line:

```
sema> (define (factorial n)
  ...   (if (= n 0)
  ...     1
  ...     (* n (factorial (- n 1)))))
sema> (factorial 10)
3628800
```

### Shadowing Warnings

The REPL warns when you accidentally redefine a built-in function:

```
sema> (define map 42)
  warning: redefining builtin 'map'
```

This is only a warning — the redefinition still works. It helps catch accidental name collisions.

### History

Command history is saved to `~/.sema/history.txt` and persists across sessions.

## Error Messages

Sema provides detailed, colorized error messages with source context and actionable hints.

### Source Context

Errors show the offending source line with a caret pointing to the problem:

```
Error: Reader error at 1:16: unterminated string
  --> script.sema:1:16
   |
 1 | (define name "hello
   |                ^
  hint: add a closing `"` to end the string
```

### Type Errors

Type errors show the actual value that caused the problem:

```
Error: Type error: expected number, got string ("hello")
  --> <input>:1:1
   |
 1 | (+ "hello" 42)
   | ^
  at + (<input>:1:1)
```

### Arity Errors

When you pass the wrong number of arguments, the error shows what you called:

```
Error: Arity error: f expects 1 args, got 3
  --> <input>:1:18
   |
 1 | (define (f x) x) (f 1 2 3)
   |                  ^
  at f (<input>:1:18)
  note: in: (f 1 2 3)
```

### Mismatched Brackets

Mixed bracket types are caught with specific guidance:

```
Error: Reader error at 1:7: mismatched bracket: expected `]` to close `[`, found `)`
  hint: this vector was opened with `[` — close it with `]`
```

### "Did You Mean?"

Typos in function or variable names trigger fuzzy suggestions:

```
Error: Unbound variable: pritnln
  hint: Did you mean 'println'?
```

### Lisp Dialect Hints

If you use names from other Lisp dialects (Common Lisp, Clojure, Scheme), Sema provides targeted guidance:

```
Error: Unbound variable: setq
  hint: Sema uses 'set!' for variable assignment

Error: Unbound variable: funcall
  hint: In Sema, functions are called directly: (f arg ...)
```

### Stack Overflow

Infinite recursion gets a helpful hint:

```
Error: Eval error: maximum eval depth exceeded (1024)
  hint: this usually means infinite recursion; ensure recursive calls are in
        tail position for TCO, or use 'do' for iteration
```

### NO_COLOR Support

Set `NO_COLOR=1` to disable colored output, or pipe stderr to a file — Sema auto-detects non-TTY output and strips colors.
