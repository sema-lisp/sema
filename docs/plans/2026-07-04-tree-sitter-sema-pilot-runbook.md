# Pilot Runbook — `tree-sitter-sema` → `sema-lisp/tree-sitter-sema`

**Date:** 2026-07-04
**Status:** Runbook — **reviewable, not yet executed**. Each numbered step is a discrete, approvable action.
**Parent plan:** `docs/plans/2026-07-04-tree-sitter-and-editor-splits.md` (decisions), `docs/plans/2026-07-04-repo-split-org.md` (org strategy)

Goal: make `sema-lisp/tree-sitter-sema` the standalone, history-preserving home for the grammar; repoint the three git-consumers (Zed, Helix, Neovim) to a pinned commit/tag; retire the subtree mirror and the monorepo copy. **Nothing else splits in this pilot.**

## Ground truth (verified 2026-07-04)

- Grammar lives at `editors/tree-sitter-sema/`. Generated `src/` (parser.c, scanner.c, grammar.json, node-types.json, tree_sitter headers) **is tracked** ✓ — required, since Zed/Helix/nvim compile `parser.c` directly (they never run `tree-sitter generate`).
- `tree-sitter-sema.wasm` is **gitignored** (build artifact) — not needed by the three git-consumers; only web/playground would want it. Build in CI if/when needed.
- `package.json` `main: "bindings/node"` but **no `bindings/` dir exists** → npm node-binding is broken today. **npm publish is deferred** out of this pilot (editors consume via git). Fix later by scaffolding bindings or dropping `main`.
- Queries in the grammar repo = `queries/highlights.scm` only; each editor ships its own richer query set (Zed `languages/sema/*.scm`, nvim `queries/sema/`). Those stay with their editors.
- Consumers today (all point at `HelgeSverre/tree-sitter-sema`):
  - **Zed** `editors/zed/extension.toml`: `repository=…/HelgeSverre/tree-sitter-sema`, `rev = "main"` ⚠️ (must become `commit = "<SHA>"`).
  - **Helix** `editors/helix/languages.toml`: `source = { git = …, rev = "main" }` ⚠️ (pin a tag/SHA).
  - **Neovim** `editors/nvim/plugin/sema.lua` + README: `url = …`.
- Mono couplings to retire: `Makefile` targets `ts-generate`/`ts-test`/`ts` (lines 399–414, `TS_DIR := editors/tree-sitter-sema`); `.github/workflows/subtree-split.yml` (the mirror); the **vendored duplicate** `editors/zed/grammars/sema/`.
- `git-filter-repo` is installed (`/opt/homebrew/bin/git-filter-repo`).
- Old mirror `HelgeSverre/tree-sitter-sema` exists, not archived, 0 stars.

---

## Step 1 — Extract with history via `git filter-repo`

Run on a **fresh throwaway clone** — `filter-repo` rewrites history destructively and refuses to run on a repo with a configured remote/uncommitted state. Never run it in your working checkout.

```bash
# fresh clone of the current main (has the grammar history under editors/tree-sitter-sema/)
git clone https://github.com/HelgeSverre/sema.git /tmp/tss-extract
cd /tmp/tss-extract

# rewrite history to ONLY the grammar subtree, re-rooted at repo root
git filter-repo --path editors/tree-sitter-sema/ --path-rename editors/tree-sitter-sema/:

# sanity checks
ls                       # → grammar.js src/ queries/ test/ tree-sitter.json package.json README.md .gitignore
git log --oneline | wc -l   # nonzero: the subtree's own commit history, preserved
git log --oneline -- src/parser.c | head   # history follows the generated parser too
```

`--path-rename editors/tree-sitter-sema/:` strips the prefix so files land at the repo root. `filter-repo` also drops the `origin` remote (expected — re-added in Step 3).

## Step 2 — Repo hygiene (before first push)

In `/tmp/tss-extract`, make it a clean standalone repo:

1. **`package.json`** — repoint and remove the broken node-binding `main` (npm deferred):
   ```jsonc
   "repository": { "type": "git", "url": "https://github.com/sema-lisp/tree-sitter-sema.git" },
   // remove the "main": "bindings/node" line and the "nan" dependency until node bindings exist
   ```
2. **`tree-sitter.json`** — `metadata.links.repository` → `https://github.com/sema-lisp/tree-sitter-sema`.
3. **`README.md`** — update any `HelgeSverre/tree-sitter-sema` URLs → `sema-lisp/tree-sitter-sema`.
4. **Add `LICENSE`** (MIT) — the extracted subtree has none; copy the repo-root MIT license text, author Helge Sverre.
5. **Add the two workflows** from Step 5 (`.github/workflows/ci.yml`, `release.yml`).
6. Leave `.gitignore` as-is (`*.wasm` stays ignored — build artifact).

Commit the hygiene changes:
```bash
git add -A
git commit -m "chore: standalone repo hygiene (repository url, license, CI)"
```

## Step 3 — Create the org repo and push

```bash
gh repo create sema-lisp/tree-sitter-sema --public \
  --description "Tree-sitter grammar for the Sema programming language"

git remote add origin git@github.com:sema-lisp/tree-sitter-sema.git
git push -u origin main

# tag the first release
git tag v0.1.0
git push origin v0.1.0

# turn on private vulnerability reporting (matches org policy)
gh api --method PUT repos/sema-lisp/tree-sitter-sema/private-vulnerability-reporting --silent
```

**Capture the pin** (Zed needs a full commit SHA; Helix/nvim can use the tag):
```bash
SHA=$(git rev-parse v0.1.0^{commit}); echo "$SHA"
```

## Step 4 — Verify the new repo builds

Confirm CI is green on the pushed repo (Actions tab), and locally:
```bash
cd /tmp/tss-extract
npm install -g tree-sitter-cli@0.24
tree-sitter generate && git diff --exit-code src/   # committed parser matches grammar.js
tree-sitter test                                     # corpus passes
```

## Step 5 — Workflows for the new repo

**`.github/workflows/ci.yml`** — runs on push/PR; guarantees committed `src/` matches `grammar.js` and the corpus passes.
```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:
jobs:
  grammar:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
      - name: Install tree-sitter CLI
        run: npm install -g tree-sitter-cli@0.24   # matches package.json devDependency
      - name: Regenerate and verify committed src/ is in sync
        run: |
          tree-sitter generate
          git diff --exit-code src/ \
            || { echo "::error::src/ is stale — run 'tree-sitter generate' and commit"; exit 1; }
      - name: Corpus tests
        run: tree-sitter test
```

**`.github/workflows/release.yml`** — on `v*` tag, re-verify then cut a GitHub release. The wasm build is optional (only web consumers need it) and is gated behind emscripten setup, so a wasm hiccup never blocks the release.
```yaml
name: Release
on:
  push:
    tags: ['v*']
permissions:
  contents: write
jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
      - name: Install tree-sitter CLI
        run: npm install -g tree-sitter-cli@0.24
      - name: Verify + test
        run: |
          tree-sitter generate
          git diff --exit-code src/
          tree-sitter test
      # --- optional wasm artifact (web/playground); safe to remove ---
      - uses: mymindstorm/setup-emsdk@v14
      - name: Build wasm
        run: tree-sitter build --wasm --output tree-sitter-sema.wasm
      # ---------------------------------------------------------------
      - name: GitHub release
        uses: softprops/action-gh-release@v2
        with:
          generate_release_notes: true
          files: tree-sitter-sema.wasm
```

## Step 6 — Repoint the three consumers (monorepo branch)

Do this on a branch in the mono (e.g. `feature/tree-sitter-split`); **do not commit to main directly**. Uses `$SHA` / `v0.1.0` from Step 3.

**Zed** — `editors/zed/extension.toml` (owner + `rev`→`commit`):
```toml
[grammars.sema]
repository = "https://github.com/sema-lisp/tree-sitter-sema"
commit = "<SHA>"          # full 40-char SHA — NOT rev = "main"
```

**Helix** — `editors/helix/languages.toml`:
```toml
[[grammar]]
name = "sema"
source = { git = "https://github.com/sema-lisp/tree-sitter-sema", rev = "v0.1.0" }
```

**Neovim** — `editors/nvim/plugin/sema.lua` and `editors/nvim/README.md`:
```lua
url = "https://github.com/sema-lisp/tree-sitter-sema",
```

Also update doc links: `editors/zed/README.md`, `editors/nvim/README.md` (`HelgeSverre` → `sema-lisp`).

**Verify each consumer resolves the new source before merging:**
- **Zed:** `zed: install dev extension` → point at `editors/zed`; confirm the grammar downloads at `$SHA` and `.sema` highlights.
- **Helix:** `hx --grammar fetch && hx --grammar build`; open a `.sema` file, confirm highlighting.
- **Neovim:** `:TSInstall sema` (from the new URL), open a `.sema` file, confirm highlighting.

## Step 7 — Retire the mono copy & mirror (follow-up, after Step 6 verified)

Separate commit, after the consumers are confirmed working:

1. **Remove the grammar from the mono** (standalone repo is now the edit home):
   ```bash
   git rm -r editors/tree-sitter-sema
   ```
2. **Remove the vendored Zed dup** (redundant with the pin; the Zed split later adds a submodule for dev):
   ```bash
   git rm -r editors/zed/grammars/sema editors/zed/grammars/sema.wasm
   ```
3. **Delete the mirror workflow:** `git rm .github/workflows/subtree-split.yml`.
4. **Remove the Makefile grammar targets** (`Makefile` lines ~399–414: `TS_DIR`, `ts-generate`, `ts-test`, `ts`, plus the `$(TS_DIR)/node_modules` rule). Grammar building now lives in the standalone repo.
5. **Update `AGENTS.md`** — the architecture/adding-functionality notes reference `editors/tree-sitter-sema`; change to point at the external `sema-lisp/tree-sitter-sema`.
6. **Archive the old mirror** so it stops looking canonical:
   ```bash
   # add a README pointer first (manually), then:
   gh repo edit HelgeSverre/tree-sitter-sema --archived
   ```
   (Its `editors/tree-sitter-sema/**` push trigger is gone once subtree-split.yml is deleted; archiving makes the redirect intent explicit. GitHub can't 301 a non-transferred repo, so a README pointer to `sema-lisp/tree-sitter-sema` is the redirect.)

## Deferred out of this pilot

- **npm publish** of `tree-sitter-sema` (unscoped) — blocked on the missing `bindings/` (`main` gap). Follow-up: scaffold node bindings via `tree-sitter generate` on a modern CLI (or drop `main`), then add an npm-publish job. Editors don't need it.
- **wasm distribution** — the release workflow builds it optionally; wiring web/playground to consume the released wasm is separate.
- **Zed/Helix/nvim standalone repos** — this pilot only repoints them *in the mono*. The `sema-lisp/zed-sema` (submodule + `commit`) split is the next phase per the parent plan.
- **Upstreaming** to `nvim-treesitter` / `helix-editor/helix` — later, adoption-gated.

## Guardrails

- `filter-repo` only on the throwaway clone; never the working checkout.
- No commits to mono `main` for the consumer repoints — use a branch/PR.
- Keep generated `src/` committed in the grammar repo (git-consumers require it).
- Do Step 7 (removals) only after Step 6 verification passes on all three editors.
