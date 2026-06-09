# Notebook UI Redesign — Sema Branding Alignment

**Date:** 2026-04-10
**Status:** Approved
**Approach:** Streamlined Jupyter (Approach B) — cherry-pick proven Jupyter UX patterns, apply Sema's black/gold/cream brand identity

## Problem

The notebook UI uses a navy/blue/red palette (`--bg: #1a1a2e`, `--accent: #e94560`) that is inconsistent with the rest of Sema's branding (playground at sema.run, website at sema-lang.com), which uses a black/gold/cream palette (`--bg: #0c0c0c`, `--gold: #c8a855`). The layout is also a basic toolbar + cells list that doesn't leverage proven notebook UX patterns.

## Deliverables

1. **Self-contained HTML prototype** — single file, no dependencies, previews the full redesigned notebook UI in a browser with mock data
2. **Production code changes** — update `crates/sema-notebook/src/ui.rs` (HTML, CSS, JS) to match the prototype

This spec covers both the prototype and the production design. The prototype is built first to validate the design visually before touching Rust code.

---

## Design

### Color Palette

All CSS custom properties on `:root`:

| Variable | Value | Usage |
|----------|-------|-------|
| `--bg` | `#0c0c0c` | Page background |
| `--bg-editor` | `#0a0a0a` | Cell editor background |
| `--bg-output` | `#080808` | Output area background |
| `--bg-elevated` | `#141414` | Toolbar, raised surfaces |
| `--border` | `#1e1e1e` | Default borders |
| `--border-focus` | `#333` | Focused/hover borders |
| `--gold` | `#c8a855` | Primary accent (buttons, active cell, execution counts) |
| `--gold-dim` | `rgba(200, 168, 85, 0.5)` | Semi-transparent gold |
| `--gold-glow` | `rgba(200, 168, 85, 0.08)` | Subtle gold highlight background |
| `--text` | `#a09888` | Body text |
| `--text-bright` | `#d8d0c0` | Headings, emphasized text |
| `--text-dim` | `#5a5448` | Secondary/muted text |
| `--success` | `#6a9955` | Success state |
| `--error` | `#c85555` | Error state |
| `--error-bg` | `rgba(200, 85, 85, 0.06)` | Error output background |
| `--stale` | `#c8a855` | Stale cell indicator (gold, on-brand) |

### Typography

| Variable | Value | Usage |
|----------|-------|-------|
| `--serif` | `'Cormorant', Georgia, serif` | Logo, notebook title, markdown headings |
| `--mono` | `'JetBrains Mono', monospace` | Code, buttons, labels, cell counts, all UI text |

Font imports: Google Fonts — Cormorant (300, 400, 500, 600, italic 400) and JetBrains Mono (400, 500).

### Header/Toolbar

- **Sticky** at top, background `--bg-elevated`, bottom border `1px --border`
- **Left:** Inline SVG "S" mark (gold letter on dark rounded square, matching favicon) + "Notebook" in Cormorant serif
- **Center:** Editable notebook title in Cormorant. Pencil icon appears on hover to signal editability
- **Right:** Icon-only buttons with tooltips. SVG line icons in `--text-dim`, brighten to `--gold` on hover:
  - Add Code Cell (brackets icon)
  - Add Markdown Cell (text/paragraph icon)
  - Run All (double-play icon)
  - Save (disk icon)
  - Reset (refresh icon)

### Cell Layout

#### Structure
Each cell is a horizontal flex row: **gutter** (40px) + **body** (flex: 1).

#### Left Gutter
- Execution count badge: `[1]` in gold monospace for executed code cells
- `[ ]` for unexecuted code cells
- `M` in dim text for markdown cells
- Vertically centered in the gutter

#### Active Cell Indicator
- Focused cell: `3px solid --gold` left border on the cell body
- Unfocused cell: `3px solid transparent` left border (no layout shift)

#### Cell Editor
- Background `--bg-editor`, border `1px --border`, border-radius 3px
- On focus: border transitions to `--border-focus`
- Auto-resizing textarea

#### Cell Actions (Hover-Revealed)
Hidden by default. On cell hover, a row of small icon buttons appears at the top-right corner of the cell body:
- Run (play triangle)
- Delete (trash)
- Move Up (arrow up)
- Move Down (arrow down)
- Toggle Type (code/markdown swap)

All icons in `--text-dim`, transition to `--gold` on hover.

#### Shift+Enter Hint
Tiny `Shift+Enter` text in `--text-dim` at the bottom-right of the focused cell's editor. Dismissed permanently after first cell execution (stored in localStorage).

### Output Rendering

#### Code Cell Output
- Directly below editor, separated by `1px --border` top border (no gap)
- Background `--bg-output`
- Text in `--text`, errors in `--error` on `--error-bg` background
- Error outputs get a `3px --error` left border (replacing the gold active bar)
- Metadata line below output: duration, cost — in `--text-dim` at 0.75rem
- **Collapsible:** Small chevron in the output area toggles visibility. Collapsed shows: "Output hidden (N lines)"
- **Loading state:** Gold border spinner replaces execution count badge while cell is running

#### Markdown Cells
- **Dual mode:** Edit mode (textarea) and rendered mode
- Click rendered markdown to enter edit mode; Shift+Enter or click away to render
- Rendered headings (h1-h4) in Cormorant serif
- Inline code and code blocks in JetBrains Mono on `--bg-editor` background
- Rendered state has no visible border — flows naturally as document content

### Between-Cell Add Affordance

On hover between any two cells (or below the last cell), a thin horizontal line appears with a centered `+` button (gold, small circle). Clicking shows a minimal dropdown: "Code" / "Markdown". This supplements the toolbar buttons for inline cell creation.

### Empty State

When notebook has no cells:
- Centered message: "Empty notebook" in `--text-dim`
- Two gold-outlined pill buttons below: "+ Code" and "+ Markdown"

### Stale Cells

- Gold **dashed** left border instead of solid gold
- Execution count shows `*` suffix: `[3*]`

### Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Shift+Enter` | Run cell, advance to next |
| `Cmd/Ctrl+Enter` | Run cell, stay focused |
| `Cmd/Ctrl+S` | Save notebook |
| `Escape` | Deselect cell |
| `Tab` | Insert 2 spaces |

### Scrolling

- Toolbar sticky at top (z-index 100)
- Cells area scrolls independently

---

## Scope Boundaries

**In scope:**
- Self-contained HTML prototype with mock data
- Visual redesign: palette, typography, layout, icons
- Jupyter-inspired cell interactions (gutter, focus bar, hover actions, collapsible output, inline add)
- Markdown edit/render toggle

**Out of scope:**
- Backend/API changes (the Axum server routes stay the same)
- Code editor enhancements (syntax highlighting, autocomplete) — plain textarea for now
- Jupyter command mode / cell selection with arrow keys
- Multi-select or drag-and-drop cell reordering
- File browser or sidebar panels
