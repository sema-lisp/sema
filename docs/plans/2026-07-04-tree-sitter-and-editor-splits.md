# tree-sitter-sema + Editor Plugin Splits — Research & Plan

**Date:** 2026-07-04
**Status:** **DONE (2026-07-05)** — all editor/grammar repos live under `sema-lisp` with green CI; `editors/` removed from the mono; old `HelgeSverre/tree-sitter-sema` mirror retired. See the "Status update" in `docs/plans/2026-07-04-repo-split-org.md`. This doc is kept as the executed record.
**Related:** `docs/plans/2026-07-04-repo-split-org.md` (org strategy), `docs/plans/2026-02-16-editor-plugin-publishing.md` (per-registry publish targets)

Reference model for the Zed pattern: **`HelgeSverre/zed-applescript`** + **`HelgeSverre/tree-sitter-applescript`** (a working editor-plugin/grammar split by the same author). This plan mirrors that proven structure.

## Goal

Make **`tree-sitter-sema` a standalone repo that is the single source of truth** for the grammar, consumed by the editors that need it via a **pinned commit/tag**; split each editor plugin into its own repo under the `sema-lisp` org with its own publishing workflow; and complete the naming migration `HelgeSverre/*` → `sema-lisp/*` (npm packages under `@sema-lang/`).

## Who actually needs the grammar (consumer map)

Established by inspecting each plugin. **Only three** editors consume tree-sitter; the rest have independent grammars and are decoupled from `tree-sitter-sema` entirely.

| Consumer | Mechanism today | Pin style | Needs `tree-sitter-sema`? |
|---|---|---|---|
| **Zed** (`editors/zed`) | `extension.toml` `[grammars.sema]` + a **duplicate vendored copy** at `grammars/sema/` | `rev = "main"` ⚠️ | **Yes** |
| **Helix** (`editors/helix`) | `languages.toml` `[[grammar]] source = { git, rev }` | `rev = "main"` ⚠️ | **Yes** |
| **Neovim** (`editors/nvim`) | `nvim-treesitter` parser config (`url` + `files=[parser.c,scanner.c]`); ships own `queries/sema/` | git url (unpinned) | **Yes** |
| VS Code (`editors/vscode`) | TextMate `sema.tmLanguage.json` | — | **No** (own grammar; also the tmLanguage source for the website) |
| IntelliJ (`editors/intellij`) | own lexer/parser (JVM) | — | **No** |
| Emacs (`editors/emacs`) | `font-lock` regex keywords | — | **No** |
| Vim (`editors/vim`) | `syntax/sema.vim` regex | — | **No** |
| Website (`website/.vitepress`) | **hand-copy** of vscode's `sema.tmLanguage.json` | — | No (depends on **VS Code**, not tree-sitter) |

⚠️ **Two correctness bugs found, to fix during the split:**
1. **Zed pins `rev = "main"`.** The Zed extension registry requires a **full commit SHA** via the `commit` key (see `zed-applescript`: `commit = "7a5dce5…"`). `rev = "main"` is both the wrong key and a moving target — it must become `commit = "<40-char SHA>"`.
2. **The tmLanguage copies have already drifted.** `editors/vscode/.../sema.tmLanguage.json` (32793 B) and `website/.vitepress/sema.tmLanguage.json` (32818 B) **differ today** — the "keep in sync by hand" rule in AGENTS.md has already failed. The split must replace the hand-copy with a sourced dependency (below).

## tree-sitter-sema: promote mirror → home

**Current state:** the canonical grammar lives in the monorepo at `editors/tree-sitter-sema/`. A workflow (`.github/workflows/subtree-split.yml`) **already mirrors** it to a read-only repo `HelgeSverre/tree-sitter-sema` (exists, pushed 2026-02-27, **0 stars**). Its `package.json`/`tree-sitter.json` already declare that repo as home. The three consumers already point their git URLs there.

**Decision:** turn that repo into the **true home** under the org (`sema-lisp/tree-sitter-sema`) and **retire the subtree mirror**. Rationale: multiple *external* consumers pin git commits of it, so it needs real tags, its own CI, and an independent contribution surface — a read-only mirror can't be a PR target. 0 stars ⇒ nothing to preserve, so a clean history is fine.

**Migration mechanics:**
- Create `sema-lisp/tree-sitter-sema` from the `editors/tree-sitter-sema/` subtree — either `git filter-repo --path editors/tree-sitter-sema/ --path-rename editors/tree-sitter-sema/:` for history, or a clean init (history is subtree-synthetic anyway). Either way, **delete `subtree-split.yml`** and remove `editors/tree-sitter-sema/` from the mono once consumers are repointed.
- **Commit the generated `src/` (parser.c, scanner.c, tree_sitter/).** Zed, Helix, and nvim-treesitter **compile `parser.c` directly — they do not run `tree-sitter generate`** — so the generated output must be committed (it already is). This is non-negotiable for git-consumed grammars.
- **Repo contents:** `grammar.js`, `src/` (committed generated), `queries/*.scm` (canonical), `test/corpus/`, `bindings/` (node/rust/c per `tree-sitter.json`), `tree-sitter.json`, `package.json`, CI + release.
- **CI** (model on `tree-sitter-applescript`/`zed-applescript` ci.yml): pinned `tree-sitter-cli`, `tree-sitter generate` + `tree-sitter test` (corpus), and a query-node-reference `verify` step. Runs on push/PR.
- **Release:** tag `v*` → GitHub release. **npm publish is optional** (see npm decision) — editors consume via git, not npm, so npm is only for `npm install tree-sitter-sema` node-binding users.

## Zed — the correct pattern (from `zed-applescript`)

The reference repo does exactly what we want; replicate it for `sema-lisp/zed-sema`:

- **`.gitmodules`**: submodule `grammars/tree-sitter-sema` → `https://github.com/sema-lisp/tree-sitter-sema.git`. Used for **local dev + CI** (query verify, corpus test) — *not* for the published build.
- **`extension.toml`**:
  ```toml
  [grammars.sema]
  repository = "https://github.com/sema-lisp/tree-sitter-sema"
  commit = "<full 40-char SHA>"   # NOT rev = "main"
  ```
  Zed's registry fetches the grammar at that commit and compiles it.
- **Remove the vendored `grammars/sema/` duplicate** — the submodule (dev) + `commit` pin (registry) fully replace it. Keeping a third copy is the drift hazard.
- **`languages/sema/*.scm`** (highlights, injections, brackets, indents, outline, textobjects, runnables, tasks) stay in the Zed extension repo — these are Zed-flavored queries, distinct from the grammar's canonical `queries/`.
- **CI** (`ci.yml`): `checkout` with `submodules: recursive`, pinned tree-sitter-cli, `just verify` + `just test`.
- **Release** (`release.yml`): on `v*` tag → GitHub release. Publishing to the Zed registry is a **PR to `zed-industries/extensions`** bumping the extension version + tag (the reference repo does the GitHub-release half in CI; the registry PR is the manual half).
- **Bump loop:** grammar changes → new grammar commit/tag → bump `commit` SHA in `extension.toml` → advance submodule → tag Zed extension release → registry PR.

## Helix & Neovim pinning

- **Helix** (`sema-lisp/helix-sema` or contribute upstream): change `source = { git = "…/sema-lisp/tree-sitter-sema", rev = "<tag-or-SHA>" }` — **pin a tag/SHA, not `main`**, for reproducible builds. Long-term upstream path = PR to `helix-editor/helix` (gated on adoption); keep the standalone `languages.toml` doc until then.
- **Neovim** (`sema-lisp/nvim-sema`): parser config `url = "…/sema-lisp/tree-sitter-sema"`, `files = { "src/parser.c", "src/scanner.c" }`; ships its own `queries/sema/`. Upstream path = PR to `nvim-treesitter/nvim-treesitter` `parsers.lua` (which pins a revision in its own lockfile). Keep the standalone plugin for `:TSInstall sema` until upstreamed.

## Editor split matrix

| Plugin | New repo (proposed) | Toolchain | Registry / publish trigger | Grammar dep | Existing workflow to relocate |
|---|---|---|---|---|---|
| Grammar | `sema-lisp/tree-sitter-sema` | tree-sitter-cli | npm (optional), git tags | — (is the source) | replaces `subtree-split.yml` |
| Zed | `sema-lisp/zed-sema` | Rust/TS ext | `zed-industries/extensions` PR on `v*` | submodule + `commit` pin | new (model on zed-applescript) |
| VS Code | `sema-lisp/vscode-sema` | Node/vsce | Marketplace + OpenVSX on `vscode-ext-v*` | none (owns tmLanguage) | `publish-vscode-extension.yml` |
| IntelliJ | `sema-lisp/intellij-sema` | Gradle/Kotlin | JetBrains Marketplace (manual dispatch) | none | `intellij-build.yml` + `intellij-release.yml` |
| Emacs | `sema-lisp/emacs-sema` | elisp | MELPA (recipe `:repo`) | none | `editors/emacs/melpa-recipe` |
| Vim | `sema-lisp/sema.vim` | vimscript | plugin managers (git), optional vim.org | none | — |
| Neovim | `sema-lisp/sema.nvim` | lua | plugin managers; upstream nvim-treesitter | git url + files | — |
| Helix | upstream PR (+doc) | toml/scm | `helix-editor/helix` PR | git + rev pin | — |

**Naming (resolved):** per-ecosystem convention — `<editor>-sema` for Zed/VS Code/IntelliJ/Emacs, and the dot-form `sema.vim` / `sema.nvim` for Vim/Neovim so plugin managers install cleanly.

## tmLanguage sourcing (fix the live drift)

Once VS Code leaves the mono, the website can no longer hand-copy from `editors/vscode/`. Make **`sema-lisp/vscode-sema`'s `sema.tmLanguage.json` the single source** and have the website consume it, not copy it. Options (decision needed):
1. **CI fetch at a pinned tag** — website prebuild pulls the tmLanguage from the vscode repo raw URL at a pinned release tag. Simple; keeps website in the mono; no vendored file in git.
2. **Tiny npm package** (`@sema-lang/tmlanguage` or ship it inside the vscode extension's published assets) that the website depends on. Cleanest dependency graph; one more package to publish.
3. **Vendored copy + CI drift-check** — keep the copy but add a CI job that fails if it differs from upstream at the pinned tag. Lowest effort; still a copy.

Recommendation: **(1)** now (kills the drift immediately, no new package), revisit **(2)** if more surfaces need the grammar. Whichever — **first reconcile the two files that already differ** so the pinned source is correct.

## Naming migration `HelgeSverre/*` → `sema-lisp/*` (npm `@sema-lang/`)

**Surface:** ~185 files reference the owner name (heaviest in `website/`, then `editors/`, `packages/`, `pkg/prototypes`). Categories to update:
- **Grammar git URLs** in Zed `extension.toml`, Helix `languages.toml`, nvim `plugin/sema.lua` + READMEs → `sema-lisp/tree-sitter-sema`.
- **`repository` fields** in every `package.json` / `tree-sitter.json` / `extension.toml`.
- **npm names** already `@sema-lang/*` (`sema`, `sema-wasm`, `sema-web`, `llm-proxy`) — good; only the **Trusted Publisher repo path** changes (`HelgeSverre/sema` → `sema-lisp/sema`), per the npm-transfer analysis (previous session).
- **MELPA recipe** `:repo "HelgeSverre/…"` → `sema-lisp/emacs-sema`.
- **Marketplace publisher identity** — vscode `publisher: "helgesverre"`, JetBrains vendor, Zed author. **Decision:** keep the personal publisher, or create an org publisher? (Org publisher is cleaner long-term but re-publishing under a new publisher ID orphans install stats/reviews — usually keep the existing publisher and just change the repo links.)
- **README / docs links** across `website/` and each plugin.

**Order (resolved):** split the editor/grammar repos out **first**, then transfer `HelgeSverre/sema` → `sema-lisp/sema` **last**, then run the remaining `HelgeSverre → sema-lisp` reference-update pass. GitHub 301-redirects cover old URLs through the transition, so nothing hard-breaks; star-preservation holds for the **transfer** of the main repo (the editor *splits* are new repos with no stars today anyway).

## Sequencing (each phase separately approved)

1. **Promote `tree-sitter-sema` → `sema-lisp/tree-sitter-sema`** (home): CI + tags, commit generated `src/`, cut `v0.1.0`. Retire `subtree-split.yml`.
2. **Repoint the 3 consumers** to the org grammar URL + a **pinned tag/commit** (fixes Zed `rev=main` → `commit=<SHA>`, Helix `rev`, nvim url).
3. **Split Zed → `sema-lisp/zed-sema`** using the zed-applescript template (submodule + commit pin, drop vendored dup, ci/release).
4. **Split VS Code → `sema-lisp/vscode-sema`**; move `publish-vscode-extension.yml`; **fix website tmLanguage sourcing** (reconcile drift + pin source).
5. **Split Emacs / Vim / Neovim / IntelliJ** one at a time; relocate each build/publish workflow; update MELPA recipe.
6. **Transfer main repo → org**; global `HelgeSverre → sema-lisp` reference pass; update npm Trusted Publisher paths; update marketplace repo links.
7. **Helix:** keep the standalone `languages.toml` doc; pursue `helix-editor/helix` upstream PR when adoption supports it.

## Resolved decisions (2026-07-04)

- **Repo naming:** per-ecosystem convention — `tree-sitter-sema`, `zed-sema`, `vscode-sema`, `intellij-sema`, `emacs-sema`, **`sema.vim`**, **`sema.nvim`** (dot-form for vim/nvim so plugin managers install cleanly).
- **`tree-sitter-sema` on npm:** publish **unscoped** (`tree-sitter-sema`) — tree-sitter ecosystem convention (nvim-treesitter/tooling expect the unscoped name). Editors consume via git+commit regardless; npm is only for node-binding users. (The other JS packages stay `@sema-lang/*`.)
- **tmLanguage sourcing:** **CI-fetch at a pinned tag** — the website prebuild pulls `sema.tmLanguage.json` from `sema-lisp/vscode-sema` at a pinned release tag; no vendored file, no new package. **Reconcile the two already-drifted files first**, then pin.
- **Grammar edit home:** the **standalone repo (`sema-lisp/tree-sitter-sema`) is the canonical edit home** post-split; the monorepo copy is removed. Contributors PR `grammar.js`/queries there.
- **Main-repo transfer:** transfer `HelgeSverre/sema` → `sema-lisp/sema` **last**, after the editor/grammar splits (GitHub 301-redirects cover old URLs; stars preserved).
- **Execution start:** **grammar repo only, as the pilot** — stand up `sema-lisp/tree-sitter-sema` (CI, tags, `v0.1.0`) and repoint the 3 consumers; nothing else moves until reviewed.

## Deferred decisions

- **Marketplace publisher identity:** deferred. Nothing is published yet (a personal JetBrains publisher account exists). Leaning toward a **Sema-branded org publisher**, and — importantly — tying any such publisher (VS Code / JetBrains / OpenVSX) to a **`sema-lang.com` role email** (e.g. `publisher@sema-lang.com`), not a personal address, so the listings can be handed off later. Prerequisite: set up that role email before creating org publishers.

## Guardrails

- Planning only — no moves until each phase is individually approved.
- Commit generated `src/` in the grammar repo (git-consumed grammars require it).
- No `git stash`/`checkout --` in the shared mono (AGENTS.md Git Rules); use worktrees/`filter-repo`.
- Reconcile the already-drifted tmLanguage **before** pinning it as a source.
