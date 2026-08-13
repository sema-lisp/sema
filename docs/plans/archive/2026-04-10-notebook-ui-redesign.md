# Notebook UI Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the notebook's navy/blue/red UI with Sema's black/gold/cream branding and Jupyter-inspired layout patterns.

**Architecture:** All changes are in `crates/sema-notebook/src/ui.rs` — the three inline string functions (`index_html()`, `css()`, `js()`). No backend, API, or data format changes. The HTML structure changes to a gutter+body cell layout. CSS is fully replaced. JS is rewritten to support new interactions (collapsible output, between-cell add, markdown toggle, focus tracking).

**Tech Stack:** Rust (raw string literals embedding HTML/CSS/JS), Axum server (unchanged), Google Fonts (Cormorant, JetBrains Mono)

---

### Task 1: Replace index_html() with new HTML structure

**Files:**
- Modify: `crates/sema-notebook/src/ui.rs:11-46`

- [ ] **Step 1: Replace the `index_html()` function body**

Replace the entire function body with the new HTML. The new structure has:
- Google Fonts link for Cormorant + JetBrains Mono
- Inline SVG favicon
- Toolbar with: logo mark SVG + "Notebook" text, editable title input, icon-only action buttons
- `#cells` scrollable area with `#cells-inner` max-width container
- Status bar footer

```rust
/// Return the main HTML page.
pub fn index_html() -> String {
    r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Sema Notebook</title>
<link rel="icon" type="image/svg+xml" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='6' fill='%231a1a1a'/%3E%3Ctext x='16' y='24.5' text-anchor='middle' font-family='Georgia' font-weight='600' font-size='26' fill='%23c8a855'%3ES%3C/text%3E%3C/svg%3E">
<link href="https://fonts.googleapis.com/css2?family=Cormorant:ital,wght@0,300;0,400;0,500;0,600;1,400&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
<link rel="stylesheet" href="/ui/style.css">
</head>
<body>
<div id="toolbar">
  <div class="toolbar-left">
    <svg class="logo-mark" viewBox="0 0 32 32" xmlns="http://www.w3.org/2000/svg">
      <rect width="32" height="32" rx="6" fill="#1a1a1a"/>
      <text x="16" y="24.5" text-anchor="middle" font-family="Georgia, 'Times New Roman', serif" font-weight="600" font-size="26" fill="#c8a855">S</text>
    </svg>
    <span class="logo-text">Notebook</span>
  </div>
  <div class="toolbar-center">
    <input type="text" id="notebook-title" value="Untitled">
    <svg class="edit-icon" width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <path d="M11.5 1.5l3 3L5 14H2v-3L11.5 1.5z"/>
    </svg>
  </div>
  <div class="toolbar-right">
    <button class="toolbar-btn" title="Add code cell" onclick="Notebook.addCell('code')">
      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <polyline points="4,2 1,8 4,14"/><polyline points="12,2 15,8 12,14"/><line x1="10" y1="2" x2="6" y2="14"/>
      </svg>
    </button>
    <button class="toolbar-btn" title="Add markdown cell" onclick="Notebook.addCell('markdown')">
      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <line x1="2" y1="3" x2="14" y2="3"/><line x1="2" y1="7" x2="10" y2="7"/><line x1="2" y1="11" x2="14" y2="11"/>
      </svg>
    </button>
    <div class="toolbar-sep"></div>
    <button class="toolbar-btn" title="Run all cells" onclick="Notebook.evalAll()">
      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <polygon points="3,1 13,8 3,15" fill="currentColor" stroke="none"/>
      </svg>
    </button>
    <div class="toolbar-sep"></div>
    <button class="toolbar-btn" title="Save notebook" onclick="Notebook.save()">
      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <path d="M13 15H3a1 1 0 01-1-1V2a1 1 0 011-1h8l3 3v10a1 1 0 01-1 1z"/>
        <path d="M10 1v3H6"/><rect x="5" y="9" width="6" height="4"/>
      </svg>
    </button>
    <button class="toolbar-btn" title="Reset environment" onclick="Notebook.reset()">
      <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <path d="M1 4v4h4"/><path d="M2.5 10A6 6 0 1 0 4 4L1 8"/>
      </svg>
    </button>
  </div>
</div>
<div id="cells"><div id="cells-inner"></div></div>
<div id="status-bar">
  <span class="status-text" id="cell-count">0 cells</span>
  <span class="status-text status-ready" id="status-indicator">Ready</span>
</div>
<script src="/ui/notebook.js"></script>
</body>
</html>"##
        .to_string()
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p sema-notebook`
Expected: compiles (CSS/JS still old but that's fine — HTML is self-contained)

- [ ] **Step 3: Commit**

```bash
git add crates/sema-notebook/src/ui.rs
git commit -m "feat(notebook): replace HTML with Sema-branded Jupyter-inspired layout"
```

---

### Task 2: Replace css() with new stylesheet

**Files:**
- Modify: `crates/sema-notebook/src/ui.rs` — the `css()` function

- [ ] **Step 1: Replace the `css()` function body**

Replace the entire CSS string with the Sema-branded stylesheet. Key changes:
- CSS variables: `--bg: #0c0c0c`, `--gold: #c8a855`, etc.
- Fonts: `--serif: 'Cormorant'`, `--mono: 'JetBrains Mono'`
- Toolbar: sticky, icon buttons, editable title
- Cells: gutter (40px) + body layout with gold focus bar
- Output: collapsible, `--bg-output` background, error left border
- Markdown rendered: Cormorant headings, code styling
- Between-cell dividers with add button + dropdown
- Status bar at bottom
- Empty state styling

```rust
fn css() -> &'static str {
    r##"/* Sema Notebook — Stylesheet */
:root {
  --bg: #0c0c0c;
  --bg-editor: #0a0a0a;
  --bg-output: #080808;
  --bg-elevated: #141414;
  --border: #1e1e1e;
  --border-focus: #333;
  --gold: #c8a855;
  --gold-dim: rgba(200, 168, 85, 0.5);
  --gold-glow: rgba(200, 168, 85, 0.08);
  --text: #a09888;
  --text-bright: #d8d0c0;
  --text-dim: #5a5448;
  --success: #6a9955;
  --error: #c85555;
  --error-bg: rgba(200, 85, 85, 0.06);
  --mono: 'JetBrains Mono', monospace;
  --serif: 'Cormorant', Georgia, serif;
}

* { margin: 0; padding: 0; box-sizing: border-box; }

body {
  background: var(--bg);
  color: var(--text);
  font-family: var(--mono);
  font-size: 14px;
  height: 100vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

/* Toolbar */
#toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 1.25rem;
  height: 48px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-elevated);
  flex-shrink: 0;
  z-index: 100;
}

.toolbar-left {
  display: flex;
  align-items: center;
  gap: 0.6rem;
}

.logo-mark { width: 24px; height: 24px; flex-shrink: 0; }

.logo-text {
  font-family: var(--serif);
  font-size: 1.2rem;
  font-weight: 300;
  letter-spacing: 0.06em;
  color: var(--text-bright);
}

.toolbar-center {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  cursor: text;
  padding: 0.2rem 0.5rem;
  border-radius: 3px;
  transition: background 0.15s;
}
.toolbar-center:hover { background: var(--gold-glow); }
.toolbar-center:hover .edit-icon { opacity: 1; }

#notebook-title {
  font-family: var(--serif);
  font-size: 1.1rem;
  font-weight: 400;
  color: var(--text-bright);
  background: none;
  border: none;
  outline: none;
  text-align: center;
  min-width: 120px;
}

.edit-icon {
  color: var(--text-dim);
  opacity: 0;
  transition: opacity 0.15s;
  flex-shrink: 0;
}

.toolbar-right {
  display: flex;
  align-items: center;
  gap: 0.25rem;
}

.toolbar-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--text-dim);
  cursor: pointer;
  transition: color 0.15s, background 0.15s;
  position: relative;
}
.toolbar-btn:hover { color: var(--gold); background: var(--gold-glow); }
.toolbar-btn svg { width: 16px; height: 16px; }

.toolbar-sep {
  width: 1px;
  height: 20px;
  background: var(--border);
  margin: 0 0.25rem;
}

/* Tooltip */
.toolbar-btn[title]::after {
  content: attr(title);
  position: absolute;
  bottom: calc(100% + 6px);
  left: 50%;
  transform: translateX(-50%);
  background: #1a1a1a;
  color: var(--text-bright);
  font-family: var(--mono);
  font-size: 0.6rem;
  padding: 0.3rem 0.5rem;
  border-radius: 3px;
  border: 1px solid var(--border);
  white-space: nowrap;
  pointer-events: none;
  opacity: 0;
  transition: opacity 0.15s;
  z-index: 200;
}
.toolbar-btn:hover[title]::after { opacity: 1; }

/* Cells container */
#cells {
  flex: 1;
  overflow-y: auto;
  padding: 1.5rem 0;
  scrollbar-width: thin;
  scrollbar-color: var(--border) transparent;
}

#cells-inner {
  max-width: 900px;
  margin: 0 auto;
  padding: 0 1.5rem;
}

/* Empty state */
.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 6rem 2rem;
  gap: 1.5rem;
}
.empty-state-text {
  font-family: var(--serif);
  font-size: 1.3rem;
  font-weight: 300;
  color: var(--text-dim);
  letter-spacing: 0.04em;
}
.empty-state-actions { display: flex; gap: 0.75rem; }
.pill-btn {
  font-family: var(--mono);
  font-size: 0.75rem;
  color: var(--gold);
  background: transparent;
  border: 1px solid var(--gold-dim);
  padding: 0.4rem 1rem;
  border-radius: 20px;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s;
  letter-spacing: 0.03em;
}
.pill-btn:hover { background: var(--gold-glow); border-color: var(--gold); }

/* Cell */
.cell {
  display: flex;
  gap: 0;
  margin-bottom: 0;
  position: relative;
}
.cell-gutter {
  width: 40px;
  flex-shrink: 0;
  display: flex;
  align-items: flex-start;
  justify-content: center;
  padding-top: 0.65rem;
  font-family: var(--mono);
  font-size: 0.7rem;
  color: var(--text-dim);
  user-select: none;
}
.cell-gutter .exec-count { color: var(--gold); font-size: 0.7rem; }
.cell-gutter .exec-count.stale { color: var(--gold-dim); }
.cell-gutter .spinner {
  width: 14px;
  height: 14px;
  border: 2px solid var(--border);
  border-top-color: var(--gold);
  border-radius: 50%;
  animation: spin 0.6s linear infinite;
}
@keyframes spin { to { transform: rotate(360deg); } }

.cell-body {
  flex: 1;
  border-left: 3px solid transparent;
  transition: border-color 0.15s;
  min-width: 0;
}
.cell.focused .cell-body { border-left-color: var(--gold); }
.cell.stale .cell-body { border-left: 3px dashed var(--gold-dim); }

/* Cell actions (hover) */
.cell-actions {
  position: absolute;
  top: 0.35rem;
  right: 0;
  display: flex;
  gap: 0.15rem;
  opacity: 0;
  transition: opacity 0.15s;
  z-index: 10;
}
.cell:hover .cell-actions { opacity: 1; }

.cell-action-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border: none;
  border-radius: 3px;
  background: var(--bg-elevated);
  color: var(--text-dim);
  cursor: pointer;
  transition: color 0.15s, background 0.15s;
}
.cell-action-btn:hover { color: var(--gold); background: var(--gold-glow); }
.cell-action-btn.delete:hover { color: var(--error); }
.cell-action-btn svg { width: 13px; height: 13px; }

/* Editor */
.cell-editor { position: relative; }
.cell-editor textarea {
  display: block;
  width: 100%;
  padding: 0.6rem 0.75rem;
  font-family: var(--mono);
  font-size: 13px;
  line-height: 1.65;
  color: var(--text-bright);
  background: var(--bg-editor);
  border: 1px solid var(--border);
  border-radius: 3px;
  outline: none;
  resize: none;
  overflow: hidden;
  tab-size: 2;
  caret-color: var(--gold);
  transition: border-color 0.15s;
}
.cell-editor textarea:focus { border-color: var(--border-focus); }
.cell-editor textarea::selection { background: var(--gold); color: var(--bg); }

.shift-enter-hint {
  position: absolute;
  bottom: 0.4rem;
  right: 0.5rem;
  font-size: 0.6rem;
  color: var(--text-dim);
  letter-spacing: 0.03em;
  opacity: 0;
  transition: opacity 0.15s;
  pointer-events: none;
}
.cell.focused .shift-enter-hint { opacity: 1; }

/* Output */
.cell-output {
  border-top: 1px solid var(--border);
  background: var(--bg-output);
  border-radius: 0 0 3px 3px;
  overflow: hidden;
}
.cell-output-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.3rem 0.75rem;
  cursor: pointer;
  user-select: none;
}
.cell-output-header:hover { background: rgba(255,255,255,0.02); }

.output-chevron {
  font-size: 0.6rem;
  color: var(--text-dim);
  transition: transform 0.15s;
  width: 1rem;
  text-align: center;
}
.output-chevron.collapsed { transform: rotate(-90deg); }

.output-meta {
  font-size: 0.7rem;
  color: var(--text-dim);
}

.cell-output-content {
  padding: 0.5rem 0.75rem 0.6rem;
  font-family: var(--mono);
  font-size: 13px;
  line-height: 1.65;
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--text);
}
.cell-output-content.collapsed { display: none; }

/* Error output */
.cell-output.error { border-left: 3px solid var(--error); }
.cell-output.error .cell-output-content {
  color: var(--error);
  background: var(--error-bg);
}

/* Markdown rendered */
.markdown-rendered {
  padding: 0.6rem 0.75rem;
  cursor: text;
  line-height: 1.7;
  color: var(--text-bright);
}
.markdown-rendered h1,
.markdown-rendered h2,
.markdown-rendered h3,
.markdown-rendered h4 {
  font-family: var(--serif);
  font-weight: 500;
  color: var(--text-bright);
  margin: 0.5em 0 0.3em;
}
.markdown-rendered h1 { font-size: 1.8rem; }
.markdown-rendered h2 { font-size: 1.4rem; }
.markdown-rendered h3 { font-size: 1.15rem; }
.markdown-rendered h4 { font-size: 1rem; }
.markdown-rendered p { margin: 0.4em 0; }
.markdown-rendered code {
  font-family: var(--mono);
  font-size: 0.9em;
  background: var(--bg-editor);
  padding: 0.15em 0.4em;
  border-radius: 3px;
}
.markdown-rendered pre {
  background: var(--bg-editor);
  padding: 0.6rem 0.75rem;
  border-radius: 3px;
  margin: 0.5em 0;
  overflow-x: auto;
}
.markdown-rendered pre code { background: none; padding: 0; }
.markdown-rendered ul, .markdown-rendered ol { padding-left: 1.5em; margin: 0.4em 0; }
.markdown-rendered strong { color: var(--gold); }
.markdown-rendered em { font-style: italic; }
.markdown-rendered a {
  color: var(--gold);
  text-decoration: underline;
  text-underline-offset: 2px;
}

/* Between-cell divider */
.cell-divider {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 24px;
  margin: 0 0 0 40px;
  position: relative;
  opacity: 0;
  transition: opacity 0.2s;
}
.cell-divider:hover { opacity: 1; }
.cell-divider-line {
  position: absolute;
  left: 0; right: 0;
  top: 50%;
  height: 1px;
  background: var(--border);
}
.add-cell-btn {
  position: relative;
  z-index: 2;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: 1px solid var(--border);
  background: var(--bg);
  color: var(--gold);
  font-size: 0.8rem;
  cursor: pointer;
  transition: border-color 0.15s, background 0.15s;
}
.add-cell-btn:hover { border-color: var(--gold); background: var(--gold-glow); }

.add-cell-dropdown {
  position: absolute;
  top: 100%;
  left: 50%;
  transform: translateX(-50%);
  margin-top: 4px;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: 4px;
  overflow: hidden;
  z-index: 50;
  display: none;
}
.add-cell-dropdown.open { display: block; }
.add-cell-dropdown button {
  display: block;
  width: 100%;
  padding: 0.4rem 1rem;
  font-family: var(--mono);
  font-size: 0.7rem;
  color: var(--text);
  background: none;
  border: none;
  cursor: pointer;
  text-align: left;
  white-space: nowrap;
  transition: background 0.1s, color 0.1s;
}
.add-cell-dropdown button:hover { background: var(--gold-glow); color: var(--gold); }
.add-cell-dropdown button + button { border-top: 1px solid var(--border); }

/* Status bar */
#status-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 1rem 0 calc(1.5rem + 40px + 1.5rem);
  height: 24px;
  border-top: 1px solid var(--border);
  background: var(--bg);
  flex-shrink: 0;
}
.status-text { font-size: 0.65rem; color: var(--text-dim); }
.status-ready { color: var(--success); }

/* Scrollbar */
::-webkit-scrollbar { width: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: var(--border); border-radius: 3px; }
::-webkit-scrollbar-thumb:hover { background: var(--border-focus); }
"##
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p sema-notebook`
Expected: compiles cleanly

- [ ] **Step 3: Commit**

```bash
git add crates/sema-notebook/src/ui.rs
git commit -m "feat(notebook): replace CSS with Sema-branded dark gold theme"
```

---

### Task 3: Replace js() with new client-side JavaScript

**Files:**
- Modify: `crates/sema-notebook/src/ui.rs` — the `js()` function

- [ ] **Step 1: Replace the `js()` function body**

The new JS module keeps the same `Notebook` IIFE pattern and same API call signatures, but rewrites the rendering to produce the new cell layout (gutter + body + hover actions + collapsible output + between-cell dividers). Key changes from old JS:

- `renderCell()` produces: `.cell > .cell-gutter + .cell-body` instead of flat `.cell > .cell-header + .cell-editor + .cell-output`
- Adds `focusedCellId` tracking for the gold focus bar
- Adds `renderDivider()` for between-cell `+` buttons with dropdown
- Adds `toggleOutput()` for collapsible output
- Adds markdown edit/render toggle
- Adds `Shift+Enter` hint with localStorage dismissal
- Adds `Cmd/Ctrl+Enter` for run-and-stay
- Adds `Escape` to deselect
- Adds empty state rendering
- Updates status bar cell count

```rust
fn js() -> &'static str {
    r##"/* Sema Notebook — Client-side JavaScript */
const Notebook = (() => {
  let cells = [];
  let focusedCellId = null;
  let shiftEnterUsed = localStorage.getItem('sema-nb-shift-enter-used') === 'true';

  async function api(method, path, body) {
    const opts = { method, headers: { 'Content-Type': 'application/json' } };
    if (body !== undefined) opts.body = JSON.stringify(body);
    const res = await fetch(path, opts);
    if (!res.ok) {
      const text = await res.text();
      throw new Error(text || res.statusText);
    }
    return res.json();
  }

  function escapeHtml(str) {
    return str.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
  }

  function autoResize(textarea) {
    textarea.style.height = 'auto';
    textarea.style.height = textarea.scrollHeight + 'px';
  }

  // SVG icons for cell actions
  const icons = {
    run: '<svg viewBox="0 0 16 16" fill="currentColor"><polygon points="4,2 13,8 4,14"/></svg>',
    delete: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><polyline points="2,4 14,4"/><path d="M5 4V2h6v2"/><path d="M3 4l1 10h8l1-10"/></svg>',
    moveUp: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><line x1="8" y1="13" x2="8" y2="3"/><polyline points="4,7 8,3 12,7"/></svg>',
    moveDown: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><line x1="8" y1="3" x2="8" y2="13"/><polyline points="4,9 8,13 12,9"/></svg>',
  };

  function renderMarkdown(src) {
    let html = escapeHtml(src);
    html = html.replace(/^#### (.+)$/gm, '<h4>$1</h4>');
    html = html.replace(/^### (.+)$/gm, '<h3>$1</h3>');
    html = html.replace(/^## (.+)$/gm, '<h2>$1</h2>');
    html = html.replace(/^# (.+)$/gm, '<h1>$1</h1>');
    html = html.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
    html = html.replace(/\*(.+?)\*/g, '<em>$1</em>');
    html = html.replace(/`([^`]+)`/g, '<code>$1</code>');
    html = html.replace(/```[\w]*\n([\s\S]*?)```/g, '<pre><code>$1</code></pre>');
    html = html.replace(/^- (.+)$/gm, '<li>$1</li>');
    html = html.replace(/(<li>.*<\/li>\n?)+/g, function(m) { return '<ul>' + m + '</ul>'; });
    html = html.replace(/^(?!<[hup]|<li|<ul|<ol|<pre)(.+)$/gm, '<p>$1</p>');
    return html;
  }

  function renderOutputHTML(output) {
    if (!output || !output.content) return '';
    const isError = output.output_type === 'error';
    const cls = isError ? 'cell-output error' : 'cell-output';

    let displayContent = '';
    if (output.content) {
      if (isError) {
        displayContent = escapeHtml(output.content);
      } else {
        displayContent = '<span style="color:var(--gold)">' + escapeHtml(output.content) + '</span>';
      }
    }

    let metaParts = [];
    if (output.meta) {
      if (output.meta.duration_ms != null) metaParts.push(output.meta.duration_ms + 'ms');
      if (output.meta.cost_usd != null) metaParts.push('$' + output.meta.cost_usd.toFixed(4));
    }
    const metaText = metaParts.join(' \u00b7 ');

    return '<div class="' + cls + '">' +
      '<div class="cell-output-header" onclick="Notebook.toggleOutput(this)">' +
        '<span class="output-chevron">\u25bc</span>' +
        '<span class="output-meta">' + metaText + '</span>' +
      '</div>' +
      '<div class="cell-output-content">' + displayContent + '</div>' +
    '</div>';
  }

  function renderCell(cell) {
    const isCode = cell.cell_type === 'code';
    const isFocused = cell.id === focusedCellId;
    const isStale = cell.stale;

    let classes = 'cell';
    if (isFocused) classes += ' focused';
    if (isStale) classes += ' stale';

    // Gutter
    let gutterContent = '';
    if (!isCode) {
      gutterContent = '<span style="color:var(--text-dim)">M</span>';
    } else if (cell._loading) {
      gutterContent = '<div class="spinner"></div>';
    } else if (cell.cell_number != null) {
      const sc = isStale ? ' stale' : '';
      const ss = isStale ? '*' : '';
      gutterContent = '<span class="exec-count' + sc + '">[' + cell.cell_number + ss + ']</span>';
    } else {
      gutterContent = '<span style="color:var(--text-dim)">[ ]</span>';
    }

    // Actions
    let actions = '<div class="cell-actions">';
    if (isCode) {
      actions += '<button class="cell-action-btn" title="Run" onclick="Notebook.evalCell(\'' + cell.id + '\')">' + icons.run + '</button>';
    }
    actions += '<button class="cell-action-btn" title="Move up" onclick="Notebook.moveCell(\'' + cell.id + '\',-1)">' + icons.moveUp + '</button>';
    actions += '<button class="cell-action-btn" title="Move down" onclick="Notebook.moveCell(\'' + cell.id + '\',1)">' + icons.moveDown + '</button>';
    actions += '<button class="cell-action-btn delete" title="Delete" onclick="Notebook.deleteCell(\'' + cell.id + '\')">' + icons.delete + '</button>';
    actions += '</div>';

    // Body content
    let bodyContent = '';
    if (!isCode && cell._rendered) {
      bodyContent = '<div class="markdown-rendered" onclick="Notebook.editMarkdown(\'' + cell.id + '\')">' + renderMarkdown(cell.source) + '</div>';
    } else {
      const rows = Math.max(cell.source.split('\n').length, 1);
      const hint = (!shiftEnterUsed && isFocused) ? '<span class="shift-enter-hint">Shift+Enter</span>' : '';
      bodyContent = '<div class="cell-editor">' +
        '<textarea rows="' + rows + '" spellcheck="false" data-id="' + cell.id + '"' +
        ' onfocus="Notebook.focusCell(\'' + cell.id + '\')"' +
        ' onkeydown="Notebook.onKeyDown(event,\'' + cell.id + '\')"' +
        ' oninput="Notebook.onEdit(this,\'' + cell.id + '\')"' +
        '>' + escapeHtml(cell.source) + '</textarea>' +
        hint + '</div>';
    }

    // Output
    let outputHTML = '';
    if (cell.rendered_outputs && isCode) {
      outputHTML = cell.rendered_outputs.map(renderOutputHTML).join('');
    }

    return '<div class="' + classes + '" id="cell-' + cell.id + '" data-id="' + cell.id + '">' +
      '<div class="cell-gutter">' + gutterContent + '</div>' +
      '<div class="cell-body">' + actions + bodyContent + outputHTML + '</div>' +
    '</div>';
  }

  function renderDivider(afterId) {
    return '<div class="cell-divider" onmouseenter="this.classList.add(\'visible\')" onmouseleave="Notebook.closeDivider(this)">' +
      '<div class="cell-divider-line"></div>' +
      '<div class="add-cell-btn" onclick="Notebook.toggleDropdown(event,\'' + afterId + '\')">+</div>' +
      '<div class="add-cell-dropdown" data-after="' + afterId + '">' +
        '<button onclick="Notebook.insertCell(\'code\',\'' + afterId + '\')">Code</button>' +
        '<button onclick="Notebook.insertCell(\'markdown\',\'' + afterId + '\')">Markdown</button>' +
      '</div>' +
    '</div>';
  }

  function renderAllCells() {
    const container = document.getElementById('cells-inner');

    if (cells.length === 0) {
      container.innerHTML = '<div class="empty-state">' +
        '<span class="empty-state-text">Empty notebook</span>' +
        '<div class="empty-state-actions">' +
          '<button class="pill-btn" onclick="Notebook.addCell(\'code\')">+ Code</button>' +
          '<button class="pill-btn" onclick="Notebook.addCell(\'markdown\')">+ Markdown</button>' +
        '</div></div>';
      updateStatus();
      return;
    }

    // Auto-render markdown cells that haven't been edited
    cells.forEach(function(c) {
      if (c.cell_type === 'markdown' && c._rendered === undefined && c.source) {
        c._rendered = true;
      }
    });

    let html = renderDivider('top');
    cells.forEach(function(cell) {
      html += renderCell(cell);
      html += renderDivider(cell.id);
    });
    container.innerHTML = html;

    // Auto-resize all textareas
    container.querySelectorAll('textarea').forEach(autoResize);
    updateStatus();
  }

  function updateStatus() {
    const el = document.getElementById('cell-count');
    if (el) el.textContent = cells.length + ' cell' + (cells.length !== 1 ? 's' : '');
  }

  function focusCell(id) {
    focusedCellId = id;
    renderAllCells();
    const ta = document.querySelector('#cell-' + id + ' textarea');
    if (ta) ta.focus();
  }

  function editMarkdown(id) {
    const cell = cells.find(function(c) { return c.id === id; });
    if (cell) {
      cell._rendered = false;
      focusedCellId = id;
      renderAllCells();
      const ta = document.querySelector('#cell-' + id + ' textarea');
      if (ta) ta.focus();
    }
  }

  async function load() {
    try {
      const data = await api('GET', '/api/notebook');
      document.getElementById('notebook-title').value = data.title || 'Untitled';
      cells = data.cells || [];
      renderAllCells();
    } catch (e) {
      console.error('Failed to load notebook:', e);
    }
  }

  async function addCell(type, afterId) {
    try {
      const body = { type: type, source: '' };
      if (afterId) body.after = afterId;
      const data = await api('POST', '/api/cells', body);
      await load();
      focusedCellId = data.id;
      renderAllCells();
      const el = document.querySelector('#cell-' + data.id + ' textarea');
      if (el) el.focus();
    } catch (e) {
      console.error('Failed to create cell:', e);
    }
  }

  async function insertCell(type, afterId) {
    closeAllDropdowns();
    const body = { type: type, source: '' };
    if (afterId && afterId !== 'top') body.after = afterId;
    try {
      const data = await api('POST', '/api/cells', body);
      await load();
      focusedCellId = data.id;
      renderAllCells();
      const el = document.querySelector('#cell-' + data.id + ' textarea');
      if (el) el.focus();
    } catch (e) {
      console.error('Failed to insert cell:', e);
    }
  }

  async function evalCell(id) {
    const cell = cells.find(function(c) { return c.id === id; });
    if (!cell) return;

    // Sync source
    const textarea = document.querySelector('#cell-' + id + ' textarea');
    if (textarea) {
      try {
        await api('POST', '/api/cells/' + id, { source: textarea.value });
      } catch (e) { /* ignore */ }
    }

    cell._loading = true;
    renderAllCells();

    try {
      await api('POST', '/api/cells/' + id + '/eval');
      // Advance focus to next cell
      const idx = cells.findIndex(function(c) { return c.id === id; });
      if (idx < cells.length - 1) {
        focusedCellId = cells[idx + 1].id;
      }
      shiftEnterUsed = true;
      localStorage.setItem('sema-nb-shift-enter-used', 'true');
      await load();
    } catch (e) {
      cell._loading = false;
      renderAllCells();
      console.error('Eval failed:', e);
    }
  }

  async function evalCellStay(id) {
    const cell = cells.find(function(c) { return c.id === id; });
    if (!cell) return;

    const textarea = document.querySelector('#cell-' + id + ' textarea');
    if (textarea) {
      try {
        await api('POST', '/api/cells/' + id, { source: textarea.value });
      } catch (e) { /* ignore */ }
    }

    cell._loading = true;
    renderAllCells();

    try {
      await api('POST', '/api/cells/' + id + '/eval');
      focusedCellId = id;
      await load();
    } catch (e) {
      cell._loading = false;
      renderAllCells();
      console.error('Eval failed:', e);
    }
  }

  async function evalAll() {
    const sources = [];
    document.querySelectorAll('.cell-editor textarea').forEach(function(ta) {
      if (ta.dataset.id) sources.push([ta.dataset.id, ta.value]);
    });
    try {
      await api('POST', '/api/eval-all', { sources: sources });
      await load();
    } catch (e) {
      console.error('Eval all failed:', e);
    }
  }

  async function deleteCell(id) {
    try {
      await api('DELETE', '/api/cells/' + id);
      if (focusedCellId === id) focusedCellId = null;
      await load();
    } catch (e) {
      console.error('Delete failed:', e);
    }
  }

  async function moveCell(id, dir) {
    const idx = cells.findIndex(function(c) { return c.id === id; });
    const newIdx = idx + dir;
    if (newIdx < 0 || newIdx >= cells.length) return;

    const ids = cells.map(function(c) { return c.id; });
    const tmp = ids[idx];
    ids[idx] = ids[newIdx];
    ids[newIdx] = tmp;

    try {
      await api('POST', '/api/cells/reorder', { cell_ids: ids });
      await load();
    } catch (e) {
      console.error('Move failed:', e);
    }
  }

  async function save() {
    try {
      await api('POST', '/api/save');
      const btn = document.querySelector('.toolbar-btn[title="Save notebook"]');
      if (btn) {
        btn.style.color = 'var(--success)';
        setTimeout(function() { btn.style.color = ''; }, 600);
      }
    } catch (e) {
      alert('Save failed: ' + e.message);
    }
  }

  async function reset() {
    if (!confirm('Reset the environment? All cell outputs will be cleared.')) return;
    try {
      await api('POST', '/api/reset');
      await load();
    } catch (e) {
      console.error('Reset failed:', e);
    }
  }

  function onEdit(textarea, cellId) {
    autoResize(textarea);
    const cell = cells.find(function(c) { return c.id === cellId; });
    if (cell) cell.source = textarea.value;
  }

  function onKeyDown(event, cellId) {
    if (event.key === 'Enter' && event.shiftKey) {
      event.preventDefault();
      const cell = cells.find(function(c) { return c.id === cellId; });
      if (cell && cell.cell_type === 'markdown') {
        cell._rendered = true;
        renderAllCells();
      } else {
        evalCell(cellId);
      }
      return;
    }
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      evalCellStay(cellId);
      return;
    }
    if (event.key === 'Tab' && !event.shiftKey) {
      event.preventDefault();
      const ta = event.target;
      const start = ta.selectionStart;
      ta.value = ta.value.substring(0, start) + '  ' + ta.value.substring(ta.selectionEnd);
      ta.selectionStart = ta.selectionEnd = start + 2;
      const cell = cells.find(function(c) { return c.id === cellId; });
      if (cell) cell.source = ta.value;
      autoResize(ta);
    }
    if (event.key === 's' && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      save();
    }
    if (event.key === 'Escape') {
      event.target.blur();
      focusedCellId = null;
      renderAllCells();
    }
  }

  function toggleOutput(header) {
    const chevron = header.querySelector('.output-chevron');
    const content = header.nextElementSibling;
    if (chevron) chevron.classList.toggle('collapsed');
    if (content) content.classList.toggle('collapsed');
  }

  function toggleDropdown(event, afterId) {
    event.stopPropagation();
    const divider = event.target.closest('.cell-divider');
    const dropdown = divider ? divider.querySelector('.add-cell-dropdown') : null;
    closeAllDropdowns();
    if (dropdown) dropdown.classList.add('open');
  }

  function closeAllDropdowns() {
    document.querySelectorAll('.add-cell-dropdown.open').forEach(function(d) {
      d.classList.remove('open');
    });
  }

  function closeDivider(el) {
    const dropdown = el.querySelector('.add-cell-dropdown');
    if (!dropdown || !dropdown.classList.contains('open')) {
      el.classList.remove('visible');
    }
  }

  // Close dropdowns on outside click
  document.addEventListener('click', function(e) {
    if (!e.target.closest('.add-cell-btn') && !e.target.closest('.add-cell-dropdown')) {
      closeAllDropdowns();
    }
  });

  document.addEventListener('DOMContentLoaded', load);

  return {
    addCell: addCell,
    insertCell: insertCell,
    evalCell: evalCell,
    evalAll: evalAll,
    deleteCell: deleteCell,
    moveCell: moveCell,
    save: save,
    reset: reset,
    onEdit: onEdit,
    onKeyDown: onKeyDown,
    focusCell: focusCell,
    editMarkdown: editMarkdown,
    toggleOutput: toggleOutput,
    toggleDropdown: toggleDropdown,
    closeDivider: closeDivider,
  };
})();
"##
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p sema-notebook`
Expected: compiles cleanly

- [ ] **Step 3: Run existing tests**

Run: `cargo test -p sema-notebook`
Expected: all tests pass (tests are in render.rs and format.rs — they don't test UI strings, so they should be unaffected)

- [ ] **Step 4: Commit**

```bash
git add crates/sema-notebook/src/ui.rs
git commit -m "feat(notebook): replace JS with Jupyter-inspired cell interactions"
```

---

### Task 4: Manual smoke test

**Files:** None (verification only)

- [ ] **Step 1: Start the notebook server**

Run: `cargo run -- notebook` (or `cargo run -- notebook examples/test.sema-nb` if a test notebook exists)

The server should print: `Sema Notebook server listening on http://127.0.0.1:8080`

- [ ] **Step 2: Open in browser and verify**

Open `http://127.0.0.1:8080` and check:
1. Toolbar: gold "S" mark, "Notebook" in serif, editable title, icon buttons with tooltips
2. Empty state if no cells: "Empty notebook" + pill buttons
3. Add a code cell: gutter shows `[ ]`, gold focus bar, Shift+Enter hint
4. Type `(+ 1 2)` and press Shift+Enter: spinner appears, then output with collapsible chevron and duration
5. Add a markdown cell: type `# Hello` and press Shift+Enter to render
6. Click rendered markdown to re-enter edit mode
7. Hover between cells: `+` button appears with dropdown
8. Hover over cell: action icons appear (run, move, delete)
9. Test Cmd+S: save button flashes green briefly
10. Test Reset: confirm dialog, outputs cleared

- [ ] **Step 3: Commit (final, if any fixups needed)**

```bash
git add crates/sema-notebook/src/ui.rs
git commit -m "fix(notebook): UI polish from smoke test"
```
