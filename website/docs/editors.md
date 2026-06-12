---
outline: [2, 2]
---

# Editor Support

Sema has editor plugins for VS Code, IntelliJ IDEA, Vim/Neovim, Emacs, Helix, and Zed. All plugins provide syntax highlighting for the full standard library, special forms, keyword literals, character literals, strings, numbers, comments, and LLM primitives.

Sema also includes a built-in [Language Server (LSP)](/docs/lsp.html) that provides diagnostics, completion, hover, go-to-definition, and code lenses. See the [LSP documentation](/docs/lsp.html) for setup instructions and feature details.

Source code for all editor plugins is in the [`editors/`](https://github.com/HelgeSverre/sema/tree/main/editors) directory.

## VS Code

TextMate grammar-based extension with full syntax highlighting, bracket matching, auto-closing pairs, comment toggling, and indentation support.

### Install

```bash
EXT_DIR=~/.vscode/extensions/helgesverre.sema-0.1.0
mkdir -p "$EXT_DIR/syntaxes"
BASE=https://raw.githubusercontent.com/HelgeSverre/sema/main/editors/vscode/sema
curl -fsSL "$BASE/package.json" -o "$EXT_DIR/package.json"
curl -fsSL "$BASE/language-configuration.json" -o "$EXT_DIR/language-configuration.json"
curl -fsSL "$BASE/syntaxes/sema.tmLanguage.json" -o "$EXT_DIR/syntaxes/sema.tmLanguage.json"
curl -fsSL "$BASE/icon.png" -o "$EXT_DIR/icon.png"
```

Restart VS Code after installing.

### Features

- Syntax highlighting (special forms, builtins, LLM primitives, keywords, strings, numbers, booleans, character literals, comments)
- Bracket matching and auto-closing for `()`, `[]`, `{}`, `""`
- Comment toggling (<kbd>Ctrl</kbd>+<kbd>/</kbd> / <kbd>Cmd</kbd>+<kbd>/</kbd>)
- Indentation rules for all bracket types
- Arithmetic/comparison operator highlighting

## IntelliJ IDEA

Full IDE support via the [LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij) plugin, connecting to the built-in Sema [Language Server](/docs/lsp.html) for completions, diagnostics, hover docs, go-to-definition, code lenses, and more.

### Requirements

- IntelliJ IDEA 2024.1+ (Community or Ultimate)
- [LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij) plugin (installed automatically as a dependency)
- `sema` binary on PATH (or set the `SEMA_PATH` environment variable)

### Install

Build and install from source:

```bash
cd editors/intellij
./gradlew buildPlugin
```

Then install the generated ZIP:

1. Open **Settings → Plugins → ⚙️ → Install Plugin from Disk…**
2. Select `editors/intellij/build/distributions/sema-intellij-*.zip`
3. Restart the IDE

### Features

- Syntax highlighting (special forms, builtins, keywords, strings, numbers, booleans, character literals, comments, regex literals)
- Code completion — builtins, special forms, user-defined symbols, scope-aware local bindings
- Hover documentation — builtin docs, function signatures, import info
- Go to definition — user definitions, cross-module navigation, import path resolution
- Find references — scope-aware, local and cross-file
- Rename — scope-aware, blocks renaming builtins and special forms
- Diagnostics — real-time parse errors and compile-time warnings
- Code lenses — ▶ Run top-level forms with inline result display
- Brace matching — auto-pair `()`, `[]`, `{}`
- Commenting — line (`;`) and block (`#| |#`) comments
- Run configurations — right-click `.sema` files to run, or create from the Run menu
- File icons — `.sema` source and `.semac` bytecode
- Color settings page — customizable syntax colors under **Settings → Editor → Color Scheme → Sema**

### Configuration

Set the `SEMA_PATH` environment variable to the path of your `sema` binary if it's not on PATH:

```bash
export SEMA_PATH=/path/to/sema
```

## Vim / Neovim

Pure Vimscript plugin with syntax highlighting, filetype detection, and Lisp-aware indentation.

### vim-plug

```vim
Plug 'helgesverre/sema', { 'rtp': 'editors/vim' }
```

### lazy.nvim

```lua
{
  "helgesverre/sema",
  config = function(plugin)
    vim.opt.rtp:append(plugin.dir .. "/editors/vim")
  end,
}
```

### Manual (Vim)

```bash
mkdir -p ~/.vim/syntax ~/.vim/ftdetect ~/.vim/ftplugin
BASE=https://raw.githubusercontent.com/HelgeSverre/sema/main/editors/vim
curl -fsSL "$BASE/syntax/sema.vim" -o ~/.vim/syntax/sema.vim
curl -fsSL "$BASE/ftdetect/sema.vim" -o ~/.vim/ftdetect/sema.vim
curl -fsSL "$BASE/ftplugin/sema.vim" -o ~/.vim/ftplugin/sema.vim
```

### Manual (Neovim)

```bash
mkdir -p ~/.config/nvim/syntax ~/.config/nvim/ftdetect ~/.config/nvim/ftplugin
BASE=https://raw.githubusercontent.com/HelgeSverre/sema/main/editors/vim
curl -fsSL "$BASE/syntax/sema.vim" -o ~/.config/nvim/syntax/sema.vim
curl -fsSL "$BASE/ftdetect/sema.vim" -o ~/.config/nvim/ftdetect/sema.vim
curl -fsSL "$BASE/ftplugin/sema.vim" -o ~/.config/nvim/ftplugin/sema.vim
```

### Features

- Full syntax highlighting (special forms, builtins, LLM primitives, keywords, character literals, comments)
- Automatic filetype detection for `.sema` files
- Lisp-aware indentation with correct `lispwords` for all Sema special forms
- Comment string configured for `;`

## Emacs

Major mode derived from `prog-mode` with Lisp-aware indentation, REPL integration, and imenu support.

### Manual

```bash
mkdir -p ~/.emacs.d/site-lisp
curl -fsSL https://raw.githubusercontent.com/HelgeSverre/sema/main/editors/emacs/sema-mode.el \
  -o ~/.emacs.d/site-lisp/sema-mode.el
```

```elisp
(add-to-list 'load-path "~/.emacs.d/site-lisp")
(require 'sema-mode)
```

### use-package

```elisp
(use-package sema-mode
  :load-path "~/.emacs.d/site-lisp"
  :mode "\\.sema\\'")
```

### Doom Emacs

In `packages.el`:

```elisp
(package! sema-mode :recipe (:local-repo "~/.emacs.d/site-lisp"))
```

In `config.el`:

```elisp
(use-package! sema-mode :mode "\\.sema\\'")
```

### Features

- Syntax highlighting (special forms, builtins, keyword literals, booleans, character literals, numbers, strings, comments)
- Buffer-local Lisp indentation with Sema-specific form rules
- REPL integration — start a Sema REPL and send code interactively
- imenu support for navigating `defun`, `define`, `defmacro`, `defagent`, `deftool`, and `define-record-type` definitions
- Electric pairs for `()`, `[]`, `{}`, `""`
- Proper sexp navigation with quote (`'`), quasiquote (`` ` ``), and unquote (`,`) prefix handling

### Key Bindings

| Key       | Command               | Description                      |
| --------- | --------------------- | -------------------------------- |
| `C-c C-z` | `sema-repl`           | Start or switch to the Sema REPL |
| `C-c C-e` | `sema-send-last-sexp` | Send sexp before point to REPL   |
| `C-c C-r` | `sema-send-region`    | Send selected region to REPL     |
| `C-c C-b` | `sema-send-buffer`    | Send entire buffer to REPL       |
| `C-c C-l` | `sema-run-file`       | Run current file with `sema`     |

### Configuration

```elisp
;; Path to the sema binary (default: "sema")
(setq sema-program "/path/to/sema")
```

## Helix

Syntax highlighting using the dedicated [tree-sitter-sema](https://github.com/helgesverre/tree-sitter-sema) grammar, with Sema-specific highlight queries, text objects, and indentation.

### Install

1. Download and append the language config to your Helix configuration:

   ```bash
   BASE=https://raw.githubusercontent.com/HelgeSverre/sema/main/editors/helix
   curl -fsSL "$BASE/languages.toml" >> ~/.config/helix/languages.toml
   ```

   > If you already have a `languages.toml`, manually merge the `[[language]]` and `[[grammar]]` sections.

2. Download the query files:

   ```bash
   mkdir -p ~/.config/helix/runtime/queries/sema
   for f in highlights indents textobjects injections; do
     curl -fsSL "$BASE/queries/sema/$f.scm" \
       -o ~/.config/helix/runtime/queries/sema/$f.scm
   done
   ```

3. Fetch and build the Sema grammar:

   ```bash
   hx --grammar fetch
   hx --grammar build
   ```

4. Verify:

   ```bash
   hx --health sema
   ```

### Features

- Syntax highlighting via tree-sitter queries (special forms, builtins, LLM primitives, keywords, booleans, character literals, strings, comments)
- Text objects — `maf`/`mif` for function definitions, `mac`/`mic` for agent/tool definitions
- Smart auto-pairs for `()`, `[]`, `{}`, `""`
- Indentation support
- `;` line comments

### How It Works

The `grammar = "sema"` setting tells Helix to parse `.sema` files using the [tree-sitter-sema](https://github.com/helgesverre/tree-sitter-sema) grammar, which provides native support for Sema-specific syntax like keyword literals (`:name`), hash maps, and vectors. Custom query files in `queries/sema/` provide Sema-specific captures for LLM primitives, slash-namespaced builtins (`string/trim`, `llm/chat`), and special forms like `defagent` and `deftool`.

## Zed

Extension using the dedicated [tree-sitter-sema](https://github.com/helgesverre/tree-sitter-sema) grammar with full syntax highlighting, bracket matching, code outline, and auto-pairs.

### Install

1. Open Zed
2. Go to **Zed → Extensions** (or <kbd>Cmd</kbd>+<kbd>Shift</kbd>+<kbd>X</kbd>)
3. Click **Install Dev Extension**
4. Select the `editors/zed` directory from the Sema repository

### Features

- Syntax highlighting (special forms, builtins, LLM primitives, keyword literals, booleans, `nil`, strings, comments)
- Smart auto-pairs for `()`, `[]`, `{}`, `""`
- Code outline for `define`, `defun`, `defmacro`, `defagent`, `deftool`
- Bracket matching
- `;` line comments
- 2-space indentation
