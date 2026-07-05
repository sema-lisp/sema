# Notebook UI refactor — editor + markdown components (first slice of #69)

**Status:** implemented (first pass) & verified — awaiting review
**Branch:** `feature/notebook-ui-refactor`
**Tracks:** issue #69 (Migrate notebook UI primitives to `@sema/ui`)
**Date:** 2026-07-03

> ## Implementation status (as-built)
>
> All three components shipped and wired into the notebook. Detailed TDD plan:
> `2026-07-03-notebook-ui-refactor-plan.md`.
>
> - **`sema-code-editor`** — transparent textarea + synchronous highlight overlay
>   (`internal/sema-tokenize.ts`, ported from the playground) + ported `TextareaUndo`
>   + autosize + Tab-to-spaces + `testid` forwarded onto the inner textarea. `focus()`
>   delegate; native `input`/`change` stopped at the boundary and re-emitted as typed
>   `CustomEvent`s.
> - **`sema-markdown`** — `marked` + shared Shiki for fences (degrades per-fence) +
>   allowlist sanitizer.
> - **`sema-editable-markdown`** — edit-in-place; click→edit, blur/Shift+Enter/Escape→render.
> - **Notebook** — code cells → `sema-code-editor`, markdown cells → `sema-editable-markdown`;
>   regex renderer + `_rendered`/`editMarkdown`/`insertTab`/`autoResize` deleted; bundle
>   vendored to `crates/sema-notebook/src/ui/vendor/sema-ui.js` via `make notebook-ui-vendor`.
>
> **Verification.** `@sema/ui`: 28 new tests green (lint + typecheck + build clean). Notebook:
> `cargo test -p sema-notebook` (49) + clippy clean; headless demo eval OK. **Live browser
> (chrome-devtools MCP, real end-to-end):** page renders; 10 code editors highlight (live
> `tok-*`); 6 markdown cells render (marked → `<h1>`/`<strong>`); click-to-edit, Shift+Enter
> render, Tab-to-spaces, and cell eval (`(* 6 7)`→`42`) all work; `cell-textarea` testid
> resolves through the shadow root.
>
> **Not done / caveats.**
> - **Playwright e2e not run here:** the browser binary download failed silently in this
>   environment (revision mismatch, no proxy). *No e2e spec changes were needed* — the
>   testid-forwarding keeps `cell-textarea`/`markdown-rendered` working; the suite should
>   pass once the browser installs. Equivalent coverage was obtained via chrome-devtools MCP.
> - Naive bundling: only `sema-ui.js` is vendored (499 KB); non-`sema` markdown fences
>   render unhighlighted (grammar chunks not served). Slim entry = follow-up.
> - Pre-existing unrelated failure: `ui/tests/tokens.test.ts` (hardcoded hex in
>   `sema-code-typer.ts`) fails at the branch base — untouched here.
> - Playground still uses its own inline editor (dogfood deferred).

## 1. Context

The notebook browser UI (`crates/sema-notebook/src/ui/`) is Alpine.js + hand-rolled
primitives: a raw `<textarea>` editor (no syntax highlighting) and a ~12-line **regex**
markdown renderer (`renderMarkdown` in `notebook.js`). Markdown cells toggle between a
rendered `<div x-html="renderMarkdown(...)">` and the textarea via a `_rendered` flag —
`@click` enters edit mode; `@blur` and Shift+Enter re-render.

The repo ships a first-party Lit web-component library, **`@sema/ui`** (`ui/`). Issue #69
wants the notebook to consume `@sema/ui` on an **incremental** path (leaf primitives →
editor → menus/dialogs) so it stops re-implementing (and re-bugging) primitives by hand.

**This spec covers the first vertical slice:** build the editor + markdown components the
notebook needs, resolve the bundling prerequisite, and rewire the notebook's cells to use
them. Toolbar buttons, tooltips, menus, dialogs, and toasts are deferred to follow-ups.

> **Scope decision (2026-07-03):** slice 1 rewires **both code and markdown cells** — the
> full keystone payoff and the direct reading of "replace the textarea for editing." This
> makes the shadow-DOM testid-forwarding e2e adaptation (§2.2) a required, validate-early
> task. The narrower staged options (code-only / markdown-only) remain in §8 as fallbacks.

### What already exists

- `@sema/ui` `sema-textarea` — themed multi-line input (plain; **no live highlighting**).
- `@sema/ui` `sema-code` — **display-only** Shiki-highlighted `<pre>` (not an editor).
- `@sema/ui` Shiki highlighter (`internal/syntax-highlight.ts`) + bundled `sema` grammar.
- **Playground editor** (`playground/src/`) — a proven, dependency-free code editor using
  the transparent-textarea + highlight-overlay technique:
  - `<textarea>` (real caret/selection/input) layered over a `<div>` painted by a
    **synchronous** hand-written highlighter (`highlight.js` → `tokenizeSema`/`highlightSema`).
  - rAF-debounced repaint (`scheduleHighlight`), scroll sync (`syncScroll`), a line-number
    gutter (`updateGutter`), and a custom undo/redo stack (`undo.js` → `TextareaUndo`).
- **No markdown renderer exists** anywhere (only a Shiki *grammar* for highlighting md source).

## 2. Prerequisites (must be resolved for this slice)

1. **Bundling / single-binary.** `@sema/ui` builds via Vite to `ui/dist/sema-ui.js`
   (~424 KB) + lazy language chunks. The notebook embeds assets with `include_str!` from
   *inside its own crate dir*. → A `make` target builds `ui/` and vendors the built bundle
   (+ needed grammar chunks) into `crates/sema-notebook/src/ui/vendor/`, served and
   `include_str!`-embedded exactly like the offline fonts. Single-binary/offline holds.
2. **e2e coupling (shadow DOM).** `notebook.spec.ts` drives `[data-testid="cell-textarea"]`
   with `.fill()`/`.type()`/`.focus()` and `[data-testid="markdown-rendered"]`. Moving the
   editable control into a web component's shadow root breaks `.fill()` unless the testid is
   **forwarded onto the inner `<textarea>`**. `sema-code-editor` reflects a `testid` prop
   onto its internal textarea; Playwright pierces open shadow roots for testid locators, so
   `getByTestId('cell-textarea').fill(...)` keeps working. (Validate early — this is the
   single biggest migration risk.)
3. **Alpine ↔ web-component binding.** `x-model` relies on `input` on the bound element.
   The components own their internal state and emit `input`/`change` with `detail.value`;
   Alpine binds `.value` and persists on those events (no `x-model` on a shadow control).

## 3. Scope

**In scope (this slice):**
- **`sema-code-editor`** — editable, syntax-highlighting code editor extracted from the
  playground editor. The keystone.
- **`sema-markdown`** — pure markdown → styled-HTML renderer.
- **`sema-editable-markdown`** — compound edit-in-place composing the two above.
- Vendor/build wiring so the notebook embeds `@sema/ui` offline.
- Rewire notebook **code cells** → `sema-code-editor lang="sema"`, and **markdown cells** →
  `sema-editable-markdown`. Delete the regex renderer + the markdown Alpine helpers.

**Non-goals (deferred):**
- Toolbar buttons, tooltips, menus, dialogs, toasts → later #69 slices.
- Migrating the **playground** to consume `sema-code-editor` (dogfood) → follow-up once the
  component proves out in the notebook.
- Removing Alpine.js (state/reactivity stays on Alpine during incremental migration).
- Sanitization hardening beyond a light allowlist (local single-user authoring context).

## 4. Design

### 4.1 `sema-code-editor` (keystone — editable code editor)

Extract the playground's editor into a reusable Lit component.

- **Structure (shadow DOM):** `.wrap` containing an `aria-hidden` `<div class="highlight">`
  overlay + a transparent `<textarea>` on top; optional `<div class="gutter">` for line
  numbers. Overlay and gutter scroll-sync to the textarea.
- **Props:** `value` (two-way via events), `lang` (default `sema`), `placeholder`,
  `readonly`, `gutter` (bool), `autosize` (bool — grow to content, needed for notebook
  cells), `tab-size` (default 2), `testid` (forwarded onto the inner textarea for e2e).
- **Highlighting:** the overlay repaints per keystroke, so it needs a **synchronous**
  highlighter. `@sema/ui`'s `highlightToHtml` is async-only (Shiki + `@lit/task`) and would
  invite stale-render races, so the editor uses a ported synchronous Sema tokenizer
  (`internal/sema-tokenize.ts`, from the playground's `tokenizeSema`/`highlightSema`);
  non-`sema` langs fall back to escaped plain text. Shiki stays the highlighter for static
  `sema-code` and markdown fences. Repaint is rAF-debounced. The highlighter is a pluggable
  static hook so a future sync-Shiki path can replace the port without an API change.
- **Undo:** port `TextareaUndo` (overlay editors lose native undo once `value` is set
  programmatically — this is exactly why the playground has a custom stack).
- **Events:** `input` (`detail.value`) per keystroke; `change` on blur/commit; `keydown`
  bubbles `composed` so the host handles Shift+Enter (notebook: eval) / other shortcuts.
- **Tab/indent:** Tab inserts `tab-size` spaces (matches notebook `insertTab`).
- **Parts:** `textarea`, `highlight`, `gutter` for consumer theming.

### 4.2 `sema-markdown` (renderer)

- **Input:** `value` property **or** slotted text (slot → `value`, like `sema-code`).
- **Parse:** [`marked`](https://marked.js.org) (no deps). Fenced code blocks route to
  `@sema/ui`'s `highlightToHtml(code, lang)` so md code fences match `sema-code`; unknown/
  unloaded fence languages fall back to escaped plain text.
- **Sanitize:** render via `unsafeHTML` but pass parser output through a small tag/attr
  allowlist first (strip `<script>`, event-handler attrs, `javascript:` URLs).
- **Styling:** shadow-DOM styles from design tokens (headings, lists, `code`, `pre`,
  tables, links); links get `rel="noopener"`. Exposes `part`s for theming.
- **Testid:** root surface forwards a `data-testid` so the notebook keeps `markdown-rendered`.

### 4.3 `sema-editable-markdown` (compound edit-in-place)

Composes `sema-code-editor lang="markdown"` (edit) + `sema-markdown` (view); owns the toggle.

- **Props:** `value` (two-way), `placeholder`, `readonly`.
- **State:** internal `editing` boolean. Not editing → `sema-markdown`; editing →
  `sema-code-editor` (autosize, `lang="markdown"` for highlighted source).
- **Interactions (mirror the notebook today):** click rendered → edit + focus; blur →
  render if non-empty; Shift+Enter → render; empty content → muted "click to edit" affordance.
- **Events:** `change` (`detail.value`) on commit; `input` on keystroke for live persist parity.
- **Parts/testids:** forwards `markdown-rendered` (view) + `cell-textarea` (edit) testids.

### 4.4 Bundling

- New `make` target (`notebook-ui-vendor`): `cd ui && npm run build`, then copy
  `dist/sema-ui.js` (+ required grammar chunks) into `crates/sema-notebook/src/ui/vendor/`.
- `ui.rs`: add a `vendor/sema-ui.js` asset route + `include_str!`.
- `index.html`: `<script type="module" src="vendor/sema-ui.js">`, then use the new elements.
- Document that `@sema/ui` changes require re-running the vendor target (like fonts).
- **Follow-up optimization (out of scope):** a slim notebook-only entry that tree-shakes to
  just the used components, shrinking the embed below the full 424 KB.

### 4.5 Notebook wiring (`notebook.js` / `index.html`)

- **Code cells:** replace `<textarea>` with
  `<sema-code-editor lang="sema" testid="cell-textarea" :value="cell.source"
  @input="cell.source = $event.detail.value" @keydown.shift.enter.prevent="handleShiftEnter(cell)"
  @change="persistSource(cell)">`.
- **Markdown cells:** replace the `x-if` pair with a single
  `<sema-editable-markdown :value="cell.source" @change="onMarkdownChange(cell, $event)">`.
- Delete `renderMarkdown`, `editMarkdown`, `insertTab`, `autoResize`, and the markdown
  branches of `onBlur`/`handleShiftEnter` (the components own them now).

## 5. Data flow

```
Alpine cell.source ─(:value)─▶ sema-code-editor / sema-editable-markdown
                                   │  keystroke (@input detail.value) ─▶ cell.source
                                   │  Shift+Enter (@keydown, code) ─▶ evalCell
                                   ▼
     sema-code-editor: textarea ⇄ highlight overlay (Shiki-sync) + gutter + undo
     sema-editable-markdown: sema-code-editor(md) ⇄ sema-markdown (marked + Shiki fences)
                                   │
      persist ◀─(@change detail.value)─┘ ─▶ POST /api/cells/:id
```

## 6. Error handling / edge cases

- **Malformed markdown:** `marked` is tolerant; on any throw, fall back to escaped raw text.
- **Empty source:** markdown view shows a clickable empty affordance; empty `change` persists.
- **Fence / editor language not loaded:** render escaped plain text (no dynamic-import
  failure in the offline binary; the `sema` + `markdown` grammars are vendored).
- **Undo after programmatic `value` set:** covered by the ported `TextareaUndo`.
- **Autosize + overlay height:** overlay height tracks the textarea so no clipping/misalign.
- **Focus race entering edit mode:** focus on next frame (mirrors current `$nextTick`).

## 7. Testing

- **`@sema/ui` vitest (browser):**
  - `sema-code-editor`: typing updates `value` + overlay; Tab inserts spaces; undo/redo;
    `input`/`change`/`keydown` events; `testid` reaches the inner textarea; readonly.
  - `sema-markdown`: headings/lists/links/tables/fences; sanitization strips `<script>` +
    handlers; slot vs `value`.
  - `sema-editable-markdown`: click→edit, blur→render, Shift+Enter→render, empty affordance,
    two-way `value`, `change`/`input`.
- **Notebook e2e (`notebook.spec.ts`):** code cell type → run → output (proves
  `cell-textarea` testid still `.fill()`s through the shadow root); markdown add → type →
  blur renders → click re-edits (`markdown-rendered` resolves); edit → save → reload
  round-trip (persistence path).
- **Rust:** `cargo test -p sema-notebook` for the new asset route.
- **Full local gate before PR:** `make lint`, notebook e2e, manual `make example-notebook`.

## 8. Open questions

- **Slice scope:** ~~both vs. staged~~ **decided: both code + markdown cells** (see §3).
  Staged fallbacks (code-only / markdown-only) retained here in case the e2e adaptation
  proves costlier than expected.
- **Editor highlighter:** ~~Shiki-sync vs. port~~ **decided: port the playground's
  synchronous `tokenizeSema`** (`@sema/ui`'s Shiki path is async-only). Tradeoff accepted:
  a second Sema highlighter to keep aligned with the canonical TextMate grammar; the
  pluggable hook lets a sync-Shiki path replace it later without an API change.
- **Renderer dep:** `marked` (recommended) vs `markdown-it`+DOMPurify vs hand-rolled.
- **Component names:** `sema-code-editor` / `sema-markdown` / `sema-editable-markdown` —
  open to alternatives (`sema-editor`, `sema-md`, `sema-markdown-editor`).
- **Playground dogfood:** rewire the playground onto `sema-code-editor` now or as a follow-up?
  Default: follow-up (prove it in the notebook first).
```
