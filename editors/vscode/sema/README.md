# Sema for Visual Studio Code

Language support for [Sema](https://sema-lang.com), a Lisp dialect with first-class LLM primitives.

- **Homepage**: [sema-lang.com](https://sema-lang.com)
- **Source**: [github.com/HelgeSverre/sema](https://github.com/HelgeSverre/sema)
- **Playground**: [sema.run](https://sema.run)

## Features

- Syntax highlighting for `.sema` files
- Bracket matching and auto-closing
- Comment toggling (`Ctrl+/` / `Cmd+/`)
- Highlighting for special forms, builtins, LLM primitives, keywords, strings, numbers, and more
- **Language server** (`sema lsp`): completions, hover docs, go-to-definition, references, rename, signature help, diagnostics, document symbols, and inline eval results
- **Debugging** (`sema dap`): step-through debugging with breakpoints
- **Notebooks**: open a `.sema-nb` file to edit it in the Sema notebook UI

All three shell out to the `sema` binary — set its location with the `sema.path` setting if it isn't on your `PATH`.

## Debugging

The extension bundles a debug adapter backed by the `sema dap` server. Add a launch configuration to `.vscode/launch.json`:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "sema",
      "request": "launch",
      "name": "Debug Sema Program",
      "program": "${file}",
      "stopOnEntry": false
    }
  ]
}
```

Then press `F5` in a `.sema` file. Supported: line breakpoints (incl. conditional), step in/over/out, stack traces, variable and upvalue inspection, evaluate-on-hover, pause/continue, and an "Uncaught Exceptions" filter.

## Notebooks

Opening a `.sema-nb` file launches `sema notebook serve` for that file and embeds its web UI directly in the editor tab (live cell execution, one server per open notebook, stopped when you close the tab). No extra setup — it uses the same `sema.path` binary.

## Installation

### From VSIX

1. Build the VSIX package:
   ```bash
   cd editors/vscode/sema
   npx @vscode/vsce package
   ```
2. Install in VS Code:
   ```
   code --install-extension sema-0.1.0.vsix
   ```

### Manual Install

Copy the extension folder to your VS Code extensions directory:

```bash
# macOS / Linux
cp -r editors/vscode/sema ~/.vscode/extensions/helgesverre.sema-0.1.0

# Windows
xcopy editors\vscode\sema %USERPROFILE%\.vscode\extensions\helgesverre.sema-0.1.0\ /E /I
```

Restart VS Code after copying.

### Development

1. Open the `editors/vscode/sema` folder in VS Code
2. Press `F5` to launch an Extension Development Host
3. Open any `.sema` file to see syntax highlighting
