# Repo Split & GitHub Org Migration Plan

**Date:** 2026-07-04
**Status:** Draft — planning/research only, **no repo moves executed yet**
**Implementation:** Not started
**Related:** `docs/plans/2026-02-16-editor-plugin-publishing.md` (per-editor publishing targets — this plan is the *repo-structure* prerequisite for those workflows)

## Goal

Move Sema from a single monorepo (`HelgeSverre/sema-lisp`) toward a GitHub **org** that hosts the loosely-coupled components — editor plugins, the tree-sitter grammar, and the UI component library — as their own repos with their own release/publishing workflows, while keeping the tightly-coupled Rust workspace as a monorepo.

Explicitly **out of scope / do-not-do-yet**: any actual `git filter-repo`, subtree split, file move, or deletion. This document decides *what* splits, *how*, and *in what order*. Execution is a later, separately-approved step.

## Guiding principle — split by coupling, not by folder

The repo is three populations with very different coupling. Split decisions follow the coupling, not a uniform "one repo per top-level dir" rule.

| Tier | Members | Coupling | Decision |
|---|---|---|---|
| **A. Rust workspace** | `crates/*` | Very high — single `Cargo.lock`, inter-crate `=X.Y.Z` pins bumped in lockstep, one CI/release gate, `sema-core ← … ← sema` dependency chain | **Keep as monorepo. Do not split.** |
| **B. Editor plugins + grammar** | `editors/*` | Low — independent toolchains (TS, Kotlin/Gradle, elisp, vimscript/lua), independent registries, independent release cadence | **Split into separate repos under the org.** |
| **C. Web + UI** | `website`, `playground`, `ui`, `packages`, `pkg` | Medium — `website`+`playground` are Vercel-coupled; `ui` is vendored by copy today | **Extract `ui` to its own npm package/repo; keep website+playground coupled (together or in the mono).** |

### Why crates stay put
Splitting `crates/*` would replace in-workspace path deps with cross-repo version juggling on *every* core change — the exact pain Cargo workspaces exist to remove. The release procedure (workspace version + every `=X.Y.Z` pin bumped together, single `verify` gate) assumes one repo. Nothing about the org move requires touching this. **This is the wrong thing to split.**

## GitHub org name — availability (checked 2026-07-04 via `gh api`)

**Recommended: `sema-lisp`** — matches the current repo name, distinctive, unambiguous, available.

Available fallbacks: `sema-org`, `sema-run` (matches the sema.run playground domain), `semalisp`, `sema-io`, `sema-project`, `semaproject`, `sema-hq`, `sema-team`, `usesema`.

Taken — nature noted:
- `sema-lang` — **dormant squat** (User, created 2022-10-29, 0 repos, empty profile). Trademark-complaint target; see trademark note below.
- `semalang` — **second dormant squat** (Organization, created 2025-10-29, 0 repos, empty) — created 3 years to the day after `sema-lang`; possible systematic parking of "sema" handles. Also reclaimable with a registered mark.
- `semaio` — **legit active company** (Org since 2015, 20 repos). Avoid collisions with it.
- `getsema`, `semacode`, `sema-dev`, `thesema` — dormant empty parks.

**Trademark tie-in:** reclaiming `sema-lang` / `semalang` from the squatters is far stronger with a registered "Sema" mark (Class 9 downloadable software + Class 42 SaaS/dev). Pursue the org under an *available* name now (`sema-lisp`); pursue the squatted handles via GitHub's trademark-complaint path once/if a mark is filed. See separate trademark research (not yet a doc).

## Tier B — editor plugins & grammar

### The grammar is the shared dependency — split it first
`editors/tree-sitter-sema` is the canonical grammar and is (or should be) the single source of truth every other plugin references. Today there is a **sync hazard**: `website/.vitepress/sema.tmLanguage.json` is a hand-copy of `editors/vscode/sema/syntaxes/sema.tmLanguage.json` (per AGENTS.md "keep in sync"). Once these live in different repos, "keep in sync" becomes cross-repo and will drift.

**Decision:** `tree-sitter-sema` becomes its own repo (`sema-lisp/tree-sitter-sema`), published to **npm** (and optionally crates.io) as the versioned artifact. Other plugins consume a **pinned version**, not a floating copy.

**Consumption mechanism — pinned, not submodule (default):**
- Editors that build against the grammar (Zed, Neovim via nvim-treesitter, Helix PR) reference a **git tag / npm version pin**. A pinned version is explicit, reviewable, and updated deliberately — matching how the Rust crates already pin `=X.Y.Z`.
- **Submodule** is the fallback only where a specific plugin's build genuinely needs the grammar *source tree* in-place at build time and can't consume a published package. Submodules add clone/CI friction (`--recurse-submodules`, detached-HEAD updates) — avoid unless a toolchain forces it. A `git pull` does *not* auto-advance a submodule; it still needs an explicit `submodule update`, so "auto-synced on pull" is not a real advantage over a version pin.
- **Rule of thumb:** publish the grammar → pin the version everywhere possible → submodule only where a build demands the raw tree. Never re-vendor a hand-copied `tmLanguage.json`; generate/pull it from the grammar repo in CI instead.

### Split mechanism: `git filter-repo`, not subtree
For a **one-time move** of each `editors/<plugin>` into its own repo with history preserved, use `git filter-repo --path editors/<plugin>/ --path-rename editors/<plugin>/:`. This gives each plugin an independent home (own issues, PRs, releases, contributors who don't clone the Rust tree).

**Subtree split is *not* the default here.** Subtree makes sense only if you want the monorepo to remain the source of truth and merely *export* a read-only mirror for a registry that demands a repo-rooted checkout. For editor plugins we want true independent homes, so filter-repo is correct. Revisit subtree only if a specific registry forces it.

### Per-plugin split order (pilot first)
1. **`tree-sitter-sema`** — split first; it's the shared dep and npm-friendly. Prove the filter-repo + npm-publish + tag flow end-to-end here.
2. **`zed`** — self-contained TS/extension, consumes the grammar; good second pilot for the "consume pinned grammar" pattern.
3. Then, one at a time: `vscode`, `intellij`, `emacs`, `nvim`, `vim`, `helix`.

Each plugin's publishing target (Marketplace/OpenVSX, JetBrains, MELPA, etc.) is already specced in `docs/plans/2026-02-16-editor-plugin-publishing.md`; those CI workflows move *into* each new repo as part of its split. The in-flight `.github/workflows/publish-vscode-extension.yml` and `editors/emacs/melpa-recipe` are the first artifacts to relocate.

### Pre-split checklist per plugin (research, do before executing any move)
- [ ] Inventory cross-references *out* of `editors/<plugin>/` into the rest of the mono (build scripts, shared assets, `examples/`, icons). Each becomes either a vendored asset or a published-package dep.
- [ ] Confirm the plugin's `repository`/`homepage`/`bugs` URLs and MELPA/marketplace `:repo` recipe get repointed to the new org repo (e.g. MELPA recipe `:repo "sema-lisp/emacs"`).
- [ ] Confirm the registry publish workflow can run from a repo rooted at the plugin (this is exactly what the split provides).
- [ ] Decide grammar consumption: pinned npm version (default) vs submodule (only if forced).

## Tier C — web & UI

- **`ui` → own repo + npm package** (`@sema/ui` or org-scoped). This is the real fix for the current *temporary* workaround where `ui/dist` is copied/vendored into `website/`, `playground/`, etc. (AGENTS.md documents that the brand-page `<sema-code-typer>` showcase is currently commented out precisely because it reaches outside `website/` to the repo-root `@sema/ui` bundle, which breaks Vercel's `website/`-only upload). Publishing `ui` to npm lets `website`/`playground` add it as a normal dependency — no out-of-folder imports, re-enabling the commented-out showcase.
- **`website` + `playground`** stay **coupled** — either kept together in the mono, or moved together to one repo later. Low priority; do only after Tier B proves the flow. Preserve the Vercel `website/`-only-upload constraint: once `ui` is an npm dep, the monorepo-import gotcha for the UI bundle disappears.
- **`pkg`, `packages`** — inventory before deciding; likely stay in the mono unless a clear consumer benefits from separation.

## Phasing (execution order — each step separately approved)

0. **(done)** Research org-name availability + this plan doc.
1. **Create the org** under an available name (recommended `sema-lisp`). Non-destructive; also the foundation for the trademark/handle-reclaim track.
2. **Pilot split: `tree-sitter-sema`** → org repo + npm publish + version tags. Prove filter-repo + publish end-to-end.
3. **Fix the tmLanguage sync hazard**: make the grammar repo the source; `website` + `vscode` pull/generate from a pinned grammar version in CI instead of hand-copying.
4. **Second pilot: `zed`** consuming the pinned grammar.
5. **Roll out remaining `editors/*`** one at a time, moving each publish workflow into its repo.
6. **Extract `ui` → npm package**; switch `website`/`playground` to consume it; re-enable the commented-out brand showcase.
7. **(optional, later)** Move `website`+`playground` together to their own repo.
8. **Never:** split `crates/*`. Keep the Rust workspace as the mono.

## Build automation across the split (Jakefile)

The Makefile→Jake migration (PR #71, branch `feature/jakefile-migration`) is a modular `Jakefile` + `jake/*.jake`. Structuring it to mirror the split boundaries turns each extraction into a mechanical lift instead of a rewrite. Do (1)–(3) up front — they're cheap in the mono and pay off at every split.

**Principle: one `jake/*.jake` module per split destination.** The root `Jakefile`'s `@import` list should map 1:1 to future repos, so splitting a plugin = delete one `@import` line + `git mv` the module into the new repo, where it becomes that repo's standalone Jakefile.

1. **De-mix `jake/editors.jake`.** It currently bundles three different fates: the tree-sitter grammar (`ts-*`, splits *first* → `sema-lisp/tree-sitter-sema`), the VS Code/IntelliJ packaging (`ext` group → their own repos), and the browser E2E (`test-notebook-e2e`/`test-web-e2e`, *stays* in the mono). Split into `jake/grammar.jake`, `jake/editors.jake` (packaging only), and fold the E2E in with the crate/notebook it exercises. Then every module has exactly one destiny.

2. **Root each split-bound module through one path variable** so it works in-mono and standalone unchanged. `jake/grammar.jake` already funnels through `ts_dir = "editors/tree-sitter-sema"`; after `git filter-repo` re-roots the grammar at repo root, the standalone copy just sets `ts_dir = "."` — no recipe edits. Same for editor packaging (`vscode_dir`, `intellij_dir`). The recipes already written (`ed.vscode-package`/`-publish`, `ed.intellij-build`/`-test`/`-verify`/`-publish`) are the right *content* for those repos' Jakefiles — they lift out with a namespace drop and a path re-root.

3. **Put the `@sema/ui` bundle behind a single indirection — the big one.** Today `jake/ui.jake` builds the bundle from local source (`file ui/dist/sema-ui.js: ui/src/**`) and copies it into notebook/playground. After the npm extraction the bundle comes from `node_modules/@sema-lang/ui/dist`, not a local build. Localize that swap with two variables:
   - `ui_bundle` — a **path** the vendor recipes depend on (today `ui/dist/sema-ui.js`; post-split `node_modules/@sema-lang/ui/dist/sema-ui.js`), and
   - a pinned `ui_version`.

   Then the *only* thing that changes on extraction is the "produce the bundle" step: the local `file … : ui/src/**` build recipe is swapped for `npm install @sema-lang/ui@{{ui_version}}`. The vendor-into-notebook/playground copies **don't change** — they already just copy `{{ui_bundle}}`. Better still, once `ui` is a real npm dep the consumers pull it through their own `package.json` + bundler and the hand-vendoring disappears entirely — so keep the vendoring a thin, clearly-labelled *temporary* layer isolated in `jake/ui.jake`, removable by deleting one module + `@import`. (This is exactly the vendored-copy workaround Tier C exists to kill.)

4. **Codify the cross-repo pins + tmLanguage fetch as tasks.** The split trades vendored copies for *version pins* (grammar tag for Zed/Helix/nvim, `@sema-lang/ui` npm version, the tmLanguage tag the website CI-fetches). Pins rot without tooling. Add a pins variable block + a `jake pins` task that prints them, and a `jake site.fetch-grammar-assets` task that curls `sema.tmLanguage.json` from `sema-lisp/vscode-sema` at `{{tmlang_version}}` into `website/` — turning the "CI-fetch at a pinned tag" decision (see the splits doc) into one reviewable command instead of a manual step.

5. **Retire, don't rewrite, on each split.** Because modules are per-destination, the Makefile-target retirement the pilot runbook lists (`ts-generate`/`ts-test`/`ts`) becomes: drop `@import "jake/grammar.jake"` and `git mv` it out. No recipe surgery in the mono.

Net: with (1)–(3) in place, the tree-sitter and `ui` extractions each reduce to *move one module, swap one variable*. Jake itself gained the enablers during the spike — incremental `file` recipes, recipe-scoped `@require`, `@dotenv` (jake v0.9.1) — so each split-out repo can carry a tiny standalone Jakefile with the same ergonomics.

## Open questions
- Org name final pick: `sema-lisp` (recommended) vs `sema-run` (domain match) vs `sema-org`.
- Does the current `HelgeSverre/sema-lisp` repo get *renamed/transferred into* the org as the crates monorepo, or stay under the personal account with only the split-out repos in the org? (Recommendation: transfer the mono into the org too, so everything lives under one org; GitHub sets up redirects.)
- Grammar publishing target: npm only, or npm + crates.io (for Rust-side consumers)?
- `ui` package scope/name: `@sema/ui`, `@sema-lisp/ui`, or unscoped.

## Non-goals / guardrails
- No history-destructive operations. Use `git filter-repo` for clean history-preserving extraction; never `git stash`/`checkout --` on the shared mono (see AGENTS.md Git Rules).
- No crate splitting.
- No moves until the org exists and each split is individually approved.
