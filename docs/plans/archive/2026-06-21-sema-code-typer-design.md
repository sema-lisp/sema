# `<sema-code-typer>` — animated code typer component

**Date:** 2026-06-21
**Status:** Design (awaiting review)
**Repo:** sema-lisp (`ui/` component library + `website/` brand page)

## Context

A "hacker-typer" animation — code typed out character-by-character with live syntax
highlighting, a moving caret, and an editor frame — was built as a one-off Playwright
GIF for a GitHub profile README (typing `examples/maze.sema` in a fedit-style box). It
reads really well as a brand visual. We want it as a **reusable, live web component** in
the `@sema/ui` toolkit, and showcased on the Sema brand page as an illustrative asset for
"animated code-stuff."

This is the live, in-browser version of that idea — not a GIF. It reuses the existing
Shiki highlighter and design tokens so it stays on-brand and DRY.

## Decisions (locked with user)

- **Content:** types *provided* code (deterministic), with optional cycling through
  multiple snippets. No procedural/random generator.
- **Chrome:** built-in but **optional** — titled frame/box, line-number gutter, status
  line. Off by default-able for a bare typer.
- **Highlighting:** tokenize once, reveal by character (not re-highlight per tick).
- **Home:** new Lit component in `ui/src/lib/`, demoed in `website/.vitepress/theme/BrandGuide.vue`.

## Goal

Ship `<sema-code-typer>` in `@sema/ui` and a brand-page demo, with a clean imperative
API (`play/pause/restart/seek`) that also makes it deterministic for tests and reusable
for future GIF export.

## Architecture

New component `ui/src/lib/sema-code-typer.ts` (Lit, extends `SemaElement`, Shadow DOM),
plus a small extension to the existing highlighter.

### Highlight reuse (no duplication)
- Extend `ui/src/internal/syntax-highlight.ts` with a `tokenize(code, lang): Token[]`
  that returns `{ text: string, cls: string }[]` (where `cls` is the existing `.tok-*`
  class), built on the same Shiki path the HTML highlighter already uses. The current
  HTML-returning function stays; the typer consumes tokens.
- Colors come entirely from `styles/syntax.css` (`.tok-*`) + `styles/tokens.css` CSS
  variables — the component hardcodes no hex.

### Reveal mechanism
- On code/lang change: tokenize once → flat token list with cumulative character offsets.
- A time-accumulated rAF loop advances a `revealed` char count by `cps` (chars/sec).
- Render = all whole tokens up to `revealed`, plus one partial token sliced to the caret,
  then a cursor element. Bottom-anchored scroll viewport keeps the caret in view.
- `prefers-reduced-motion: reduce` → skip animation, render the full highlighted code.

### Component API
Attributes / properties:
- content: slotted text (single snippet, dedented like `<sema-code>`); `.snippets`
  (string[]) to cycle; `lang` (default `"sema"`).
- timing: `cps` (default ~45), `start-delay` (ms), `loop` (bool), `loop-delay` (ms),
  `autoplay` (default true), `cycle-delay` (ms, between snippets).
- chrome (all optional): `frame` (bool), `line-numbers` (bool), `status` (bool),
  `filename` (string); `logo` (bool) renders the Sema wordmark SVG as the legend; a
  `legend` slot overrides the legend and a `status` slot overrides the status content.
- imperative methods: `play()`, `pause()`, `restart()`, `seek(charIndex)`; `total` getter.
- events: `sema-typer-done` (fires when a snippet finishes; on loop, each cycle).
- When framed, the host reserves top padding so the border-straddling legend is never
  clipped (incl. in element screenshots / GIF export).

Internals are split for clarity:
- a typing controller (Lit `ReactiveController`) owns the rAF loop + reveal state;
- the element owns rendering (tokens → DOM), chrome, and the scroll viewport.

### Chrome
`frame` renders a titled box (CSS-variable bordered, `legend` slot for `( sema )`), with
an optional `status` line (`EDIT`-style mode + `filename` + `Ln:Col` derived from the
caret) and an optional `line-numbers` gutter. With all chrome off, it's a bare inline
typer. The fedit "look" is achieved purely via tokens/CSS variables; the component itself
is generic.

### Export tool (GIF / WebP)
`ui/scripts/export-typer.mjs` (npm `export:typer`) drives the *real* component headlessly
via Playwright + its `seek()`/`total` API — pixel-identical to the browser. It steps a
fixed number of frames (`--frames`, decoupled from playback `--fps` so output size stays
bounded regardless of file length), screenshots the element each step, and encodes a GIF
(`gifenc`) or animated WebP (GIF buffer → `sharp`). Flags mirror the component
(`--frame --status --line-numbers --logo --rows --filename --width`). Same component =
one look across live UI, brand page, and exported marketing assets.

### Brand page integration
Add a "Code Typer" entry to `BrandGuide.vue` as a toolkit asset:
- the framed editor variant typing `examples/maze.sema` with the `( sema )` legend,
- a bare inline variant,
- a one-line description + a copy-paste usage snippet.
Register via the site's existing `@sema/ui` import path.

## Reuse summary
- `internal/syntax-highlight.ts` (+ new `tokenize`), `styles/syntax.css`,
  `styles/tokens.css`, `SemaElement`, and the `ui/` build/exports (`.` and
  `./standalone`). Example code pulled from `examples/*.sema`.

## Testing
Vitest + Playwright browser mode (existing harness):
- `seek(n)` reveals exactly the first `n` characters (token slicing correct, incl. inside
  strings/comments).
- `prefers-reduced-motion` renders the full code with no animation.
- chrome attributes (`frame`/`line-numbers`/`status`) toggle the right DOM.
- `sema-typer-done` fires on completion; `loop`/`.snippets` cycle.
- highlighting parity: typed-out result matches `<sema-code>` output for the same source.

## Verification
1. `cd ui && npm run build` succeeds; `npm test` passes.
2. Local demo page mounts `<sema-code-typer>` from `./standalone` and types `maze.sema`
   with the framed look; caret moves, scrolls, loops; reduced-motion shows full code.
3. `cd website && vitepress dev` → brand page shows the Code Typer section rendering live.

## Out of scope
- Procedural/endless code generator.
- Rewiring the GitHub-profile README GIF onto this component (the export tool + `seek()`
  make it straightforward later).
- DRY-ing the palette duplicated between `tokens.css` and `BrandGuide.vue`.
