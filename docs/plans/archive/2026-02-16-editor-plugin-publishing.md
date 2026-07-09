# Editor Plugin Publishing Plans

**Date:** 2026-02-16
**Status:** Draft
**Implementation:** Not started

## Overview

The editor install docs (`website/docs/editors.md`) currently use `curl` to download plugin files from raw GitHub URLs. This works but is a stopgap. This document outlines the ideal publishing path for each editor plugin so users can install through their editor's native package system.

## VS Code — Publish to Marketplace

The extension at `editors/vscode/sema/` is already marketplace-ready. The `package.json` has all required fields:

- `publisher`: `"helgesverre"`
- `icon`: `"icon.png"`
- `categories`: `["Programming Languages"]`
- `engines.vscode`: `"^1.75.0"`
- `repository`, `homepage`, `bugs` URLs

### Steps

1. Create an Azure DevOps Personal Access Token (PAT) with **Marketplace > Manage** scope at https://dev.azure.com
2. Log in: `npx @vscode/vsce login helgesverre`
3. Publish from the extension directory: `cd editors/vscode/sema && npx @vscode/vsce publish`
4. Verify the extension appears at https://marketplace.visualstudio.com

### After publishing

Update `website/docs/editors.md` to make the primary install method:

> Search for **Sema** in the Extensions sidebar (`Ctrl+Shift+X` / `Cmd+Shift+X`), or run `ext install sema-lang.sema-lang` from the command palette.

Keep the `curl`-based manual install as a fallback.

### CI automation (optional)

Add a GitHub Action that auto-publishes on version tags using [`HaaLeo/publish-vscode-extension`](https://github.com/HaaLeo/publish-vscode-extension). Store the Azure PAT as a repository secret (`VSCE_PAT`).

---

## Emacs — Submit to MELPA

`editors/emacs/sema-mode.el` already has all required MELPA package headers:

- `Author`, `URL`, `Version`, `Package-Requires`, `Keywords`
- `;;;###autoload` cookie on the mode definition
- `(provide 'sema-mode)` and proper file footer comment

### Steps

1. Fork [`melpa/melpa`](https://github.com/melpa/melpa) on GitHub
2. Add a recipe file at `recipes/sema-mode`:
   ```elisp
   (sema-mode :fetcher github :repo "HelgeSverre/sema" :files ("editors/emacs/*.el"))
   ```
3. Test locally with `make recipes/sema-mode` in the MELPA checkout
4. Submit a PR to `melpa/melpa` — MELPA maintainers review packaging, not content

### After accepted

Update `website/docs/editors.md` to make the primary install method:

> ```
> M-x package-install RET sema-mode RET
> ```
>
> Or add to your config: `(use-package sema-mode :ensure t)`

Keep the `curl`-based manual install as a fallback for users not using MELPA.

---

## Vim / Neovim — Already Good (Optional Standalone Repo)

The current install via plugin managers already works well. Both vim-plug and lazy.nvim support pointing at the monorepo with an `rtp` override to `editors/vim`:

```vim
" vim-plug
Plug 'helgesverre/sema', { 'rtp': 'editors/vim' }
```

```lua
-- lazy.nvim
{ "helgesverre/sema", config = function()
  vim.opt.rtp:prepend(vim.fn.stdpath("data") .. "/lazy/sema/editors/vim")
end }
```

### Optional improvement: standalone repo

Create a `helgesverre/sema.vim` repository (or a GitHub mirror/subtree of `editors/vim/`) so plugin managers work without the `rtp` override:

```vim
Plug 'helgesverre/sema.vim'
```

This could be maintained as a git subtree push or a GitHub Action that syncs `editors/vim/` to the standalone repo on changes.

### vim.org

Submitting to vim.org/scripts is possible but increasingly legacy — most users use plugin managers.

---

## Helix — Upstream to helix-editor Core

Helix has no plugin/package system. The ideal path is contributing language support directly to the editor.

### Steps

1. Submit a PR to [`helix-editor/helix`](https://github.com/helix-editor/helix) adding:
   - A `[[language]]` entry in `languages.toml` with file types, comment tokens, indent settings
   - Tree-sitter query files in `runtime/queries/sema/` (highlights, injections, textobjects)
2. This requires either a published `tree-sitter-sema` grammar or inclusion of one in the PR

### Prerequisites

- Helix typically requires languages to have some level of adoption/community before accepting them
- A `tree-sitter-sema` grammar would independently benefit other editors (Neovim, Zed, etc.)
- Consider publishing `tree-sitter-sema` to npm/crates.io first, then referencing it in the Helix PR

### Until then

The `curl`-based manual install (copying query files to `~/.config/helix/runtime/queries/sema/` and adding a `languages.toml` entry) is the best available option and is already documented in `editors.md`.
