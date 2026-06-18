<!-- Keep a Changelog guide -> https://keepachangelog.com -->

# Sema Changelog

## [Unreleased]

### Added

- Code formatting (Reformat Code) for Sema files, with a setting to disable it.
- Structural selection (Extend/Shrink Selection) and code-block navigation by s-expression.
- Call hierarchy, go-to-declaration, and clickable document links for `import`/`load` paths.
- Lazy completion documentation (`completionItem/resolve`) with function signatures for
  user-defined symbols, and context-aware completion sorting.
- Setting to keep the language server running when no Sema files are open.

### Changed

- The configured Sema binary path is now passed to the language server, so code-lens evaluation
  uses the same binary as the rest of the integration (previously it could fall back to `sema` on
  `PATH`).
- Debug launch parameters now include the adapter `type`, matching the run-configuration template.
- Updated the LSP4IJ dependency to 0.20.1.

## [1.0.0]

### Added

- LSP integration via LSP4IJ: code completion, hover documentation, go-to-definition, references,
  rename, diagnostics, folding ranges, inlay hints, document highlight, and semantic token
  colorization.
- Code lenses to evaluate top-level forms inline, with `sema/evalResult` rendering and a
  "Clear Sema Results" action.
- Debug Adapter Protocol (DAP) support: step-through debugging with breakpoints, continue,
  step over/into/out, stack frames, scopes, and variable inspection (launches `sema dap`).
- Sema Notebook editor for `.sema-nb` files: live cell evaluation in a JCEF-backed view, with
  actions to create notebooks, run all cells, open in an external browser, and export to Markdown.
- Custom file-type icons for Sema (`.sema`), compiled Sema (`.semac`), and Sema Notebook
  (`.sema-nb`) files.
- Syntax highlighting, brace matching, auto-pairing, and smart commenting for Sema source.
- Run configuration support for executing Sema files.
- Configurable Sema binary location with a Settings page, binary resolution helpers, and
  missing-binary editor notifications.
