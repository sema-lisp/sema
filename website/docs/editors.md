---
outline: [2, 2]
---

# Editor Support

Sema has editor plugins for **VS Code, Zed, IntelliJ IDEA, Neovim, Vim, Emacs, Helix, and Sublime Text**. Each plugin lives in its own repository under the [`sema-lisp`](https://github.com/sema-lisp) GitHub org and is published to that editor's registry.

Every plugin provides syntax highlighting for the full standard library, special forms, keyword and character literals, strings, numbers, comments, and LLM primitives. Most also wire up Sema's built-in developer tooling, which ships inside the `sema` binary:

- **[Language Server (LSP)](/docs/lsp)** (`sema lsp`) — diagnostics, completion, hover, go-to-definition, references, rename, and code lenses.
- **[Debugger (DAP)](/docs/dap)** (`sema dap`) — breakpoints, stepping, stack traces, and variable inspection.
- **[MCP server](/docs/mcp)** (`sema mcp`) — exposes Sema's tools (eval, build, notebook, docs) to editor AI agents.

| Editor | Repo | LSP | DAP | MCP | Highlighting |
| --- | --- | :---: | :---: | :---: | --- |
| VS Code | [`vscode-sema`](https://github.com/sema-lisp/vscode-sema) | ✓ | ✓ | ✓ | TextMate |
| Zed | [`zed-sema`](https://github.com/sema-lisp/zed-sema) | ✓ | ✓ | ✓ | tree-sitter |
| IntelliJ | [`intellij-sema`](https://github.com/sema-lisp/intellij-sema) | ✓ | ✓ | — | own lexer |
| Neovim | [`sema.nvim`](https://github.com/sema-lisp/sema.nvim) | ✓ | ✓* | — | tree-sitter |
| Vim | [`sema.vim`](https://github.com/sema-lisp/sema.vim) | — | — | — | Vimscript |
| Emacs | [`emacs-sema`](https://github.com/sema-lisp/emacs-sema) | ✓ | — | — | font-lock |
| Helix | [`helix-sema`](https://github.com/sema-lisp/helix-sema) | ✓ | ✓ | — | tree-sitter |
| Sublime Text | [`sublime-sema`](https://github.com/sema-lisp/sublime-sema) | ✓† | — | — | native syntax |

<small>\* Neovim DAP requires [`nvim-dap`](https://github.com/mfussenegger/nvim-dap). † Sublime LSP requires the [LSP](https://packagecontrol.io/packages/LSP) package.</small>

The features that shell out to `sema` (LSP, DAP, MCP, run/format) need the `sema` binary on your `PATH` — install it from [sema-lang.com](https://sema-lang.com). Syntax highlighting and structural editing work without it.

## VS Code

TextMate-grammar highlighting plus a full LSP client, a debug adapter that runs `sema dap`, an embedded notebook editor, and the Sema MCP server.

### Install

From the [VS Marketplace](https://marketplace.visualstudio.com/items?itemName=sema-lang.sema-lang) (VS Code) or [Open VSX](https://open-vsx.org/extension/sema-lang/sema-lang) (VSCodium, Cursor, Windsurf, Gitpod, …):

```
ext install sema-lang.sema-lang
```

Or open the Extensions view (<kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>X</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>X</kbd>) and search for **Sema Lisp**. Point the extension at a specific binary with the `sema.path` setting if `sema` isn't on your `PATH`.

### Features

- Syntax highlighting (special forms, builtins, LLM primitives, keywords, strings, numbers, regex/f-string literals)
- Bracket matching, auto-closing, and surrounding pairs for `()`, `[]`, `{}`, `""`
- Comment toggling (<kbd>Cmd</kbd>+<kbd>/</kbd> / <kbd>Ctrl</kbd>+<kbd>/</kbd>) and s-expression-aware indentation
- File icons for `.sema` and `.sema-nb`
- **Language server** — completions, hover, go-to-definition, references, rename, signature help, diagnostics, document symbols, and inline eval results
- **Debugging** — line/conditional breakpoints, step in/over/out, stack traces, variable and upvalue inspection, evaluate-on-hover, and an "Uncaught Exceptions" filter
- **Notebooks** — open a `.sema-nb` file to edit it in the embedded notebook UI with live cell execution
- **MCP server** — registers `sema mcp` in the Chat / agent view

## Zed

Extension built on the shared [tree-sitter-sema](https://github.com/sema-lisp/tree-sitter-sema) grammar, with LSP, DAP, runnables, and the Sema MCP context server.

### Install

From inside Zed: <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> → **zed: extensions** → search for **Sema** → Install. The grammar is fetched automatically at the commit pinned in `extension.toml` — no manual grammar setup.

To hack on it locally, clone [`zed-sema`](https://github.com/sema-lisp/zed-sema) and use **zed: install dev extension**.

### Features

- Syntax highlighting, `;` line and `#| … |#` block comments (with TODO/FIXME injection), auto-pairs, and bracket matching
- Code outline for top-level definitions and block forms
- 2-space auto-indent; Vim text objects (`af`/`if` for functions, `ac`/`ic` for agents/tools)
- **Runnables** — a gutter ▶ to run the file or evaluate the selected form (see note below)
- **Language server** and **debugging** via `sema lsp` / `sema dap`
- **MCP server** — registers `sema mcp` with Zed's agent panel
- Secret redaction for `llm/configure`/`llm/define-provider`/`llm/auto-configure` arguments during screen sharing

::: tip Runnables need a one-time task
Zed doesn't let an extension bundle the task its ▶ button runs. Add it once via **zed: open tasks** (see the [repo README](https://github.com/sema-lisp/zed-sema#running-sema-files) for the `tasks.json` snippet).
:::

## IntelliJ IDEA

Full IDE support via [LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij) connecting to the Sema language server, plus a native debugger, notebook editor, and run configurations.

### Requirements

- IntelliJ IDEA 2024.3+ (or any JetBrains IDE on build 243+)
- [LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij) — installed automatically as a plugin dependency
- The `sema` binary on `PATH`, or set it under **Settings → Languages & Frameworks → Sema**

### Install

From the IDE: **Settings → Plugins → Marketplace**, search for **Sema**. To build from source instead:

```bash
git clone https://github.com/sema-lisp/intellij-sema
cd intellij-sema
./gradlew buildPlugin
# Then: Settings → Plugins → ⚙️ → Install Plugin from Disk…
# and pick build/distributions/Sema-<version>.zip
```

### Features

- Syntax highlighting for `.sema`, `.semac`, and `.sema-nb`, with a configurable color settings page
- **LSP** (via LSP4IJ) — completion, hover, go-to-definition, references, rename, diagnostics, folding, inlay hints, document highlight, semantic tokens, call hierarchy, and clickable `import`/`load` links
- **Code lenses** — evaluate top-level forms inline, plus a "Clear Sema Results" action
- **Debugging** — breakpoints, continue, step over/into/out, stack frames, scopes, and variable inspection
- **Notebook editor** for `.sema-nb` — live cell evaluation in a JCEF view, run-all, open-in-browser, export to Markdown
- Reformat Code, brace matching, `()`/`[]`/`{}` auto-pairing, line/block commenting, Extend/Shrink Selection
- Run configurations, custom file icons, and a configurable binary location

## Neovim

Tree-sitter highlighting via [nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter), with a zero-config language server and optional DAP.

### Install

With [lazy.nvim](https://github.com/folke/lazy.nvim):

```lua
{
  "sema-lisp/sema.nvim",
  ft = "sema",
  dependencies = { "nvim-treesitter/nvim-treesitter" },
}
```

With packer.nvim:

```lua
use({ "sema-lisp/sema.nvim", requires = { "nvim-treesitter/nvim-treesitter" } })
```

Then run `:TSInstall sema` once to fetch and compile the pinned [`tree-sitter-sema`](https://github.com/sema-lisp/tree-sitter-sema) grammar.

### Features

- Filetype detection for `.sema`
- Tree-sitter highlighting (registers the parser + ships highlight queries)
- **Language server (automatic)** — on Neovim ≥ 0.11 the plugin registers and enables `sema lsp` with no config (falls back to a `FileType` autocmd on older versions); no `nvim-lspconfig` needed
- **Debugging (optional)** — if [`nvim-dap`](https://github.com/mfussenegger/nvim-dap) is installed, registers the `sema dap` adapter with a "Launch Sema file" configuration

## Vim

Pure Vimscript plugin — syntax highlighting, filetype detection, and Lisp-aware editing with no runtime dependencies. Works in both Vim and Neovim.

### Install

With [vim-plug](https://github.com/junegunn/vim-plug):

```vim
Plug 'sema-lisp/sema.vim'
```

With a native package (Vim 8+ / Neovim):

```bash
git clone https://github.com/sema-lisp/sema.vim.git \
  ~/.vim/pack/plugins/start/sema.vim
```

The repo uses the standard `ftdetect/`, `ftplugin/`, `syntax/` layout, so no runtimepath override is needed.

### Features

- Automatic filetype detection for `.sema`
- Syntax highlighting — special forms, LLM/agent primitives, threading macros (`->`, `->>`, `as->`), keyword/character literals, strings with escapes, and `;` / `#| … |#` comments
- Lisp-aware editing — `lisp` mode with a curated `lispwords` list, 2-space indentation, and `iskeyword` extended for Sema identifiers
- `commentstring`/`comments` configured for `;`

## Emacs

Major mode with Lisp-aware indentation, REPL integration, imenu, and automatic eglot setup.

### Install

From MELPA:

```elisp
;; M-x package-install RET sema-mode
(use-package sema-mode
  :ensure t
  :mode "\\.sema\\'")
```

On Emacs 29+ you can install straight from GitHub:

```elisp
(use-package sema-mode
  :vc (:url "https://github.com/sema-lisp/emacs-sema" :rev :newest)
  :mode "\\.sema\\'")
```

Doom Emacs — in `packages.el`:

```elisp
(package! sema-mode
  :recipe (:host github :repo "sema-lisp/emacs-sema"))
```

### Features

- Syntax highlighting (special forms, `llm/*`/`agent/*`/`conversation/*`/`tool/*` primitives, keyword literals, booleans, `nil`, characters, numbers, strings, comments)
- Lisp-aware indentation layered over `lisp-mode`
- **REPL integration** — send region, last sexp, or whole buffer to an inferior `sema` REPL
- imenu for functions, variables, macros, agents, tools, and record types
- Electric pairs for `()`, `[]`, `{}`, `""`
- **LSP** — registers `sema lsp` with **eglot** automatically (`M-x eglot`)

### Key Bindings

| Key       | Command               | Description                      |
| --------- | --------------------- | -------------------------------- |
| `C-c C-z` | `sema-repl`           | Start or switch to the Sema REPL |
| `C-c C-e` | `sema-send-last-sexp` | Send sexp before point to REPL   |
| `C-c C-r` | `sema-send-region`    | Send selected region to REPL     |
| `C-c C-b` | `sema-send-buffer`    | Send entire buffer to REPL       |
| `C-c C-l` | `sema-run-file`       | Run current file with `sema`     |

## Helix

Tree-sitter highlighting via the dedicated [tree-sitter-sema](https://github.com/sema-lisp/tree-sitter-sema) grammar, with the language server and debug adapter wired through `languages.toml`.

### Install

Helix has no plugin system, so support is installed by placing the grammar queries in the runtime directory and merging the language config into `~/.config/helix/`. Easiest is the install script:

```sh
git clone https://github.com/sema-lisp/helix-sema.git
cd helix-sema
./install.sh
```

The script copies the queries, merges the language config idempotently, and builds the grammar. Verify with `hx --health sema` — it should report the language server, debug adapter, tree-sitter parser, and all queries as ✓. (Manual steps are in the [repo README](https://github.com/sema-lisp/helix-sema).)

### Features

- Tree-sitter highlighting (grammar pinned to a release tag)
- Text objects — `maf`/`mif` for functions, `mac`/`mic` for agent/tool definitions
- Smart auto-pairs, 2-space indentation, and `;` line comments
- **Language server** (`sema lsp`) via the `[language-server.sema-lsp]` block
- **Debugging** (`sema dap`) via the `[language.debugger]` block

## Sublime Text

Native `.sublime-syntax` highlighting with build systems, symbol navigation, and optional LSP.

### Install

Via [Package Control](https://packagecontrol.io/installation): open the command palette → **Package Control: Install Package** → search for **Sema**. Manual install and per-OS `Packages` paths are in the [repo README](https://github.com/sema-lisp/sublime-sema).

### Features

- Syntax highlighting for `.sema` (special forms, builtins, LLM primitives, keywords, strings, numbers, characters, quote operators)
- Comment toggling (<kbd>Cmd</kbd>+<kbd>/</kbd> / <kbd>Ctrl</kbd>+<kbd>/</kbd>) — line `;` and block `#| |#`
- Symbol navigation (<kbd>Cmd</kbd>+<kbd>R</kbd>) for `define`, `defun`, `defmacro`, `defagent`, `deftool`, …
- Build systems for **running** (`sema`), **formatting** (`sema fmt`), and **compiling** (`sema compile`)
- **Language server** (`sema lsp`) via the [LSP](https://packagecontrol.io/packages/LSP) package — completions, hover, go-to-definition, references, rename, signature help, and diagnostics
