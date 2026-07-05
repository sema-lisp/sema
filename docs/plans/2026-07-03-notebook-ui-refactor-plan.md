# Notebook UI Refactor — `sema-code-editor` + markdown components — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the Sema notebook a real syntax-highlighting code editor and a proper markdown renderer by extracting the playground editor and adding markdown components to `@sema/ui`, then rewiring the notebook cells onto them (first slice of issue #69).

**Architecture:** Add three Lit web components to `@sema/ui`: `sema-code-editor` (transparent-textarea + highlight-overlay + gutter + undo, extracted from `playground/src/{app,highlight,undo}.js`), `sema-markdown` (marked → token-styled HTML, reusing Shiki for fenced code), and `sema-editable-markdown` (compound: click-to-edit ↔ render). The notebook keeps Alpine.js for state and swaps its raw `<textarea>` + regex renderer for these components; the built `@sema/ui` bundle is vendored into the notebook crate and embedded like the offline fonts.

**Tech Stack:** Lit 3, TypeScript, Vite (lib build), Vitest browser tests (Playwright/Chromium), `marked` (new dep), Shiki (existing), Alpine.js (notebook), Rust (`include_str!` embedding), Playwright e2e.

**Companion design spec:** `docs/plans/2026-07-03-notebook-ui-refactor.md` (read it first — context, prerequisites, scope decision).

## Global Constraints

- **No provider/behavior branching in components** — components are framework-agnostic; the notebook adapts to them, not vice-versa.
- **Single-binary + offline:** the notebook must run with no network. Anything the notebook loads at runtime is embedded via `include_str!`/`include_bytes!` from *inside* `crates/sema-notebook/`. No dynamic `import()` of un-vendored chunks.
- **Preserve e2e testids:** `cell-textarea`, `markdown-rendered`, `cell-editor`, `shift-enter-hint` must keep resolving in `crates/sema-notebook/tests/e2e/notebook.spec.ts`. The editable control's testid must land on the actual `<textarea>` so Playwright `.fill()`/`.type()` work through the open shadow root.
- **New dependencies:** only `marked` (^12) added to `ui/package.json`. No DOMPurify, no CodeMirror/Monaco.
- **Highlighting in the editor is synchronous** (`ui/src/internal/sema-tokenize.ts`); Shiki (`highlightToHtml`, async) stays for `sema-code` and markdown fences only.
- **Design tokens:** components style via CSS custom properties (`--mono`, `--bg-editor`, `--border`, `--text-primary`, `--syntax-*`, `tok-*` classes from `src/styles/syntax.css`). No hard-coded palette.
- **Lit conventions:** extend `SemaElement`; reflect boolean attrs; `part`s for theming; `customElements.define` at module end; `HTMLElementTagNameMap` augmentation.
- **Gate before PR:** `cd ui && npm run lint && npm run test && npm run build`; `cargo test -p sema-notebook`; `make test-notebook-e2e`; `make example-notebook`.

---

## File structure

**New (`@sema/ui`):**
- `ui/src/internal/sema-tokenize.ts` — synchronous Sema tokenizer/highlighter (port).
- `ui/src/internal/textarea-undo.ts` — undo/redo stack for a textarea (port).
- `ui/src/lib/sema-code-editor.ts` — editable code editor component.
- `ui/src/lib/sema-markdown.ts` — markdown renderer component.
- `ui/src/lib/sema-editable-markdown.ts` — compound edit-in-place component.
- `ui/tests/{sema-tokenize,textarea-undo,sema-code-editor,sema-markdown,sema-editable-markdown}.test.ts` — tests.

**Modified (`@sema/ui`):**
- `ui/src/lib/index.ts`, `ui/src/index.ts` — export the new components.
- `ui/package.json` — add `marked`.

**Modified (notebook):**
- `Makefile` — add `notebook-ui-vendor` target.
- `crates/sema-notebook/src/ui.rs` — serve `vendor/sema-ui.js`.
- `crates/sema-notebook/src/ui/index.html` — use the new elements; load the bundle.
- `crates/sema-notebook/src/ui/notebook.js` — adapt handlers; delete the regex renderer.
- `crates/sema-notebook/src/ui/vendor/sema-ui.js` — vendored build output (generated).
- `crates/sema-notebook/tests/e2e/notebook.spec.ts` — adjust selectors if needed.

**Note on tokenize test placement:** `sema-tokenize` is a pure function → node project. `vite.config.ts`'s node project only includes `tests/tokens.test.ts`. Add `tests/sema-tokenize.test.ts` to that project's `include` (Step in Task 1). All other new tests are browser tests (default `tests/**/*.test.ts`).

---

### Task 1: Synchronous Sema tokenizer (`sema-tokenize.ts`)

Port `playground/src/highlight.js` into `@sema/ui`, emitting `tok-*` classes (from `syntax.css`) instead of the playground's `hl-*` classes, so the editor overlay themes identically to `sema-code`.

**Files:**
- Create: `ui/src/internal/sema-tokenize.ts`
- Test: `ui/tests/sema-tokenize.test.ts`
- Modify: `ui/vite.config.ts` (add the test to the node project's `include`)

**Interfaces:**
- Produces:
  - `tokenizeSema(code: string): Array<{ type: string; text: string }>`
  - `highlightSemaSync(code: string, lang?: string): string` — returns `<pre>`-ready inner HTML; `lang !== 'sema'` ⇒ escaped plain text.
  - `escapeHtml(s: string): string`

- [ ] **Step 1: Write the failing test**

```ts
// ui/tests/sema-tokenize.test.ts
import { describe, expect, it } from 'vitest'
import { tokenizeSema, highlightSemaSync, escapeHtml } from '../src/internal/sema-tokenize.js'

describe('tokenizeSema', () => {
  it('classifies comments, strings, numbers, booleans, keyword-literals, keywords', () => {
    const types = (s: string) => tokenizeSema(s).map((t) => `${t.type}:${t.text}`)
    expect(types('; hi')).toEqual(['comment:; hi'])
    expect(types('"a\\"b"')).toEqual(['string:"a\\"b"'])
    expect(types('42')).toEqual(['number:42'])
    expect(types('#t')).toEqual(['boolean:#t'])
    expect(types(':key')).toEqual(['keyword-lit::key'])
    expect(types('define')).toEqual(['keyword:define'])
    expect(types('foo')).toEqual(['plain:foo'])
  })

  it('concatenated token text reconstructs the input exactly', () => {
    const src = '(define (sq x) ; c\n  (* x x))'
    expect(tokenizeSema(src).map((t) => t.text).join('')).toBe(src)
  })
})

describe('highlightSemaSync', () => {
  it('wraps classified tokens in tok-* spans and escapes html', () => {
    const html = highlightSemaSync('(define x "a<b")')
    expect(html).toContain('<span class="tok-keyword">define</span>')
    expect(html).toContain('<span class="tok-string">"a&lt;b"</span>')
    expect(html).toContain('<span class="tok-punctuation">(</span>')
  })

  it('returns escaped plain text for non-sema langs', () => {
    expect(highlightSemaSync('# h <x>', 'markdown')).toBe('# h &lt;x&gt;')
  })

  it('appends a space when the source ends in a newline (pre renders the final line)', () => {
    expect(highlightSemaSync('a\n').endsWith(' ')).toBe(true)
  })

  it('escapeHtml escapes &, <, >', () => {
    expect(escapeHtml('a & <b>')).toBe('a &amp; &lt;b&gt;')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ui && npx vitest run tests/sema-tokenize.test.ts`
Expected: FAIL — cannot resolve `../src/internal/sema-tokenize.js`.

- [ ] **Step 3: Write the implementation**

```ts
// ui/src/internal/sema-tokenize.ts
/**
 * Synchronous Sema tokenizer/highlighter for the live editor overlay.
 *
 * The editor repaints on every keystroke, so it needs a synchronous highlighter;
 * @sema/ui's Shiki path (`highlightToHtml`) is async and reserved for static
 * `sema-code` and markdown fences. Output uses the shared `tok-*` classes from
 * `styles/syntax.css`, so the overlay themes via the same `--syntax-*` variables.
 * Ported from `playground/src/highlight.js`; keep aligned with the canonical
 * TextMate grammar (`grammars/sema.tmLanguage.json`).
 */
export const SEMA_KEYWORDS = new Set<string>([
  'define', 'defun', 'lambda', 'fn', 'if', 'cond', 'case', 'when', 'unless',
  'let', 'let*', 'letrec', 'begin', 'do', 'and', 'or', 'not',
  'set!', 'quote', 'quasiquote', 'unquote', 'unquote-splicing',
  'define-record-type', 'defmacro', 'defagent', 'deftool',
  'try', 'catch', 'throw', 'error',
  'import', 'module', 'export', 'load', 'require',
  'delay', 'force', 'eval', 'macroexpand', 'with-budget', 'else',
  '->', '->>', 'as->', 'some->',
  'map', 'filter', 'foldl', 'foldr', 'reduce', 'for-each', 'apply',
])

export interface SemaToken { type: string; text: string }

export function tokenizeSema(code: string): SemaToken[] {
  const tokens: SemaToken[] = []
  let i = 0
  while (i < code.length) {
    if (code[i] === ';') {
      const start = i
      while (i < code.length && code[i] !== '\n') i++
      tokens.push({ type: 'comment', text: code.slice(start, i) })
    } else if (code[i] === '"') {
      const start = i
      i++
      while (i < code.length && code[i] !== '"') {
        if (code[i] === '\\' && i + 1 < code.length) i++
        i++
      }
      if (i < code.length) i++
      tokens.push({ type: 'string', text: code.slice(start, i) })
    } else if ('()[]{}\'`,'.includes(code[i])) {
      tokens.push({ type: 'paren', text: code[i] })
      i++
    } else if (/\s/.test(code[i])) {
      const start = i
      while (i < code.length && /\s/.test(code[i])) i++
      tokens.push({ type: 'ws', text: code.slice(start, i) })
    } else {
      const start = i
      while (i < code.length && !/[\s()[\]{}"`;,]/.test(code[i])) i++
      const word = code.slice(start, i)
      if (word === '#t' || word === '#f' || word === 'true' || word === 'false' || word === 'nil') {
        tokens.push({ type: 'boolean', text: word })
      } else if (/^-?\d+(\.\d+)?$/.test(word)) {
        tokens.push({ type: 'number', text: word })
      } else if (word.startsWith(':') && word.length > 1) {
        tokens.push({ type: 'keyword-lit', text: word })
      } else if (SEMA_KEYWORDS.has(word)) {
        tokens.push({ type: 'keyword', text: word })
      } else {
        tokens.push({ type: 'plain', text: word })
      }
    }
  }
  return tokens
}

export function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

const TYPE_TO_CLASS: Record<string, string> = {
  comment: 'tok-comment',
  string: 'tok-string',
  number: 'tok-number',
  boolean: 'tok-boolean',
  'keyword-lit': 'tok-keyword-lit',
  keyword: 'tok-keyword',
  paren: 'tok-punctuation',
}

/** Inner HTML for a `<pre>` overlay. Non-sema langs render as escaped plain text. */
export function highlightSemaSync(code: string, lang = 'sema'): string {
  if (lang !== 'sema') return escapeHtml(code)
  if (!code) return '\n'
  let html = ''
  for (const t of tokenizeSema(code)) {
    const escaped = escapeHtml(t.text)
    const cls = TYPE_TO_CLASS[t.type]
    html += cls ? `<span class="${cls}">${escaped}</span>` : escaped
  }
  if (code.endsWith('\n')) html += ' '
  return html
}
```

- [ ] **Step 4: Register the test in the node project**

In `ui/vite.config.ts`, change the node project's `include`:

```ts
// was: include: ['tests/tokens.test.ts'],
include: ['tests/tokens.test.ts', 'tests/sema-tokenize.test.ts'],
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd ui && npx vitest run tests/sema-tokenize.test.ts`
Expected: PASS (all cases).

- [ ] **Step 6: Commit**

```bash
git add ui/src/internal/sema-tokenize.ts ui/tests/sema-tokenize.test.ts ui/vite.config.ts
git commit -m "feat(ui): synchronous Sema tokenizer for the editor overlay"
```

---

### Task 2: Textarea undo/redo stack (`textarea-undo.ts`)

Port `playground/src/undo.js` to TypeScript. Overlay editors lose native undo once `value` is set programmatically; this restores it.

**Files:**
- Create: `ui/src/internal/textarea-undo.ts`
- Test: `ui/tests/textarea-undo.test.ts` (browser — needs a real `<textarea>`)

**Interfaces:**
- Produces: `class TextareaUndo { constructor(ta: HTMLTextAreaElement, opts?: { max?: number; mergeDelay?: number; onChange?: (() => void) | null }); undo(): void; redo(): void; transact(fn: () => void): void; reset(): void }`

- [ ] **Step 1: Write the failing test**

```ts
// ui/tests/textarea-undo.test.ts
import { beforeEach, describe, expect, it } from 'vitest'
import { TextareaUndo } from '../src/internal/textarea-undo.js'

function type(ta: HTMLTextAreaElement, value: string, inputType = 'insertText') {
  ta.value = value
  ta.selectionStart = ta.selectionEnd = value.length
  ta.dispatchEvent(new InputEvent('beforeinput', { inputType, bubbles: true }))
  ta.dispatchEvent(new InputEvent('input', { inputType, bubbles: true }))
}

describe('TextareaUndo', () => {
  let ta: HTMLTextAreaElement
  beforeEach(() => {
    document.body.innerHTML = '<textarea></textarea>'
    ta = document.body.querySelector('textarea')!
  })

  it('undo restores the previous committed value; redo re-applies it', () => {
    const undo = new TextareaUndo(ta, { mergeDelay: 0 })
    type(ta, 'a')
    type(ta, 'ab', 'deleteContentBackward') // force a distinct kind → new entry
    undo.undo()
    expect(ta.value).toBe('a')
    undo.redo()
    expect(ta.value).toBe('ab')
  })

  it('reset drops history to the current value', () => {
    const undo = new TextareaUndo(ta, { mergeDelay: 0 })
    type(ta, 'x')
    undo.reset()
    undo.undo()
    expect(ta.value).toBe('x')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ui && npx vitest run tests/textarea-undo.test.ts`
Expected: FAIL — cannot resolve module.

- [ ] **Step 3: Write the implementation**

Port `playground/src/undo.js` verbatim as a TS class (types added; behavior unchanged). Full source:

```ts
// ui/src/internal/textarea-undo.ts
interface UndoState { value: string; start: number; end: number }
interface UndoOpts { max?: number; mergeDelay?: number; onChange?: (() => void) | null }

/**
 * Undo/redo history for a textarea. Overlay editors set `.value` programmatically
 * (highlight repaint, undo apply), which erases the browser's native undo stack —
 * this restores Cmd/Ctrl+Z. Ported from `playground/src/undo.js`.
 */
export class TextareaUndo {
  private ta: HTMLTextAreaElement
  private max: number
  private mergeDelay: number
  private onChange: (() => void) | null
  private stack: UndoState[]
  private index: number
  private _applying = false
  private _inTransaction = 0
  private _suppress = false
  private _lastInputType: string | null = null
  private _lastPushAt = 0
  private _lastKind: string | null = null
  private _composing = false
  private _forceNew = false

  constructor(ta: HTMLTextAreaElement, { max = 200, mergeDelay = 600, onChange = null }: UndoOpts = {}) {
    this.ta = ta
    this.max = max
    this.mergeDelay = mergeDelay
    this.onChange = onChange
    this.stack = [this._read()]
    this.index = 0

    ta.addEventListener('beforeinput', (e) => { this._lastInputType = (e as InputEvent).inputType || null })
    ta.addEventListener('compositionstart', () => { this._composing = true })
    ta.addEventListener('compositionend', () => { this._composing = false; this._forceNew = true })
    ta.addEventListener('input', () => {
      if (this._applying || this._suppress || this._inTransaction || this._composing) return
      this._record()
    })
    ta.addEventListener('keydown', (e) => {
      const mod = e.metaKey || e.ctrlKey
      if (mod && !e.altKey && e.key.toLowerCase() === 'z') {
        e.preventDefault()
        e.shiftKey ? this.redo() : this.undo()
      } else if (mod && !e.altKey && e.key.toLowerCase() === 'y') {
        e.preventDefault()
        this.redo()
      }
    })
  }

  private _read(): UndoState {
    return { value: this.ta.value, start: this.ta.selectionStart ?? 0, end: this.ta.selectionEnd ?? 0 }
  }

  undo() { if (this.index > 0) { this.index--; this._apply(this.stack[this.index]) } }
  redo() { if (this.index < this.stack.length - 1) { this.index++; this._apply(this.stack[this.index]) } }

  transact(fn: () => void) {
    this._inTransaction++
    try { fn() } finally {
      this._inTransaction--
      if (this._inTransaction === 0) this._record(true)
    }
  }

  reset() { this.stack = [this._read()]; this.index = 0; this._lastPushAt = 0; this._lastKind = null }

  private _record(forceNew = false) {
    const next = this._read()
    const cur = this.stack[this.index]
    if (cur.value === next.value && cur.start === next.start && cur.end === next.end) return

    const now = performance.now()
    const it = this._lastInputType
    const kind = it?.startsWith('insert') ? 'insert' : it?.startsWith('delete') ? 'delete' : 'other'
    const forcedByType = it === 'insertFromPaste' || it === 'insertFromDrop' || it === 'deleteByCut'

    let merge = false
    if (!forceNew && !this._forceNew && !forcedByType) {
      merge = (now - this._lastPushAt) <= this.mergeDelay
        && kind === this._lastKind
        && cur.start === cur.end && next.start === next.end
        && (kind === 'insert' || kind === 'delete')
    }
    this._forceNew = false

    if (merge) {
      this.stack[this.index] = next
    } else {
      this.stack.splice(this.index + 1)
      this.stack.push(next)
      this.index++
      if (this.stack.length > this.max) {
        const overflow = this.stack.length - this.max
        this.stack.splice(0, overflow)
        this.index = Math.max(0, this.index - overflow)
      }
    }
    this._lastPushAt = now
    this._lastKind = kind
  }

  private _apply(state: UndoState) {
    this._applying = true
    this.ta.value = state.value
    this.ta.setSelectionRange(state.start, state.end)
    if (this.onChange) this.onChange()
    else { this._suppress = true; this.ta.dispatchEvent(new Event('input', { bubbles: true })); this._suppress = false }
    this._applying = false
  }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ui && npx vitest run tests/textarea-undo.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ui/src/internal/textarea-undo.ts ui/tests/textarea-undo.test.ts
git commit -m "feat(ui): port textarea undo/redo stack for the editor"
```

---

### Task 3: `sema-code-editor` component

Editable code editor: transparent `<textarea>` over a synchronously-highlighted overlay, optional gutter, autosize, tab-to-spaces, undo, testid forwarding, and events for the host.

**Files:**
- Create: `ui/src/lib/sema-code-editor.ts`
- Test: `ui/tests/sema-code-editor.test.ts`

**Interfaces:**
- Consumes: `highlightSemaSync` (Task 1), `TextareaUndo` (Task 2), `SemaElement` (`../internal/sema-element.js`).
- Produces: `class SemaCodeEditor` (tag `sema-code-editor`) with properties `value: string`, `lang: string` (default `sema`), `placeholder: string`, `readonly: boolean`, `gutter: boolean`, `autosize: boolean`, `tabSize: number` (attr `tab-size`, default 2), `testid: string`. Events: `input` (`CustomEvent<{ value: string }>`), `change` (`CustomEvent<{ value: string }>`); native `keydown` bubbles composed. Static hook: `SemaCodeEditor.highlighter: (code: string, lang: string) => string` (default `highlightSemaSync`).

- [ ] **Step 1: Write the failing test**

```ts
// ui/tests/sema-code-editor.test.ts
import { beforeEach, describe, expect, it, vi } from 'vitest'
import '../src/lib/sema-code-editor.js'
import type { SemaCodeEditor } from '../src/lib/sema-code-editor.js'

async function mount(attrs = ''): Promise<SemaCodeEditor> {
  document.body.innerHTML = `<sema-code-editor ${attrs}></sema-code-editor>`
  const el = document.body.querySelector('sema-code-editor') as SemaCodeEditor
  await el.updateComplete
  return el
}
const ta = (el: SemaCodeEditor) => el.shadowRoot!.querySelector('textarea') as HTMLTextAreaElement
const hl = (el: SemaCodeEditor) => el.shadowRoot!.querySelector('.hl') as HTMLElement

describe('sema-code-editor', () => {
  beforeEach(() => { document.body.innerHTML = '' })

  it('renders the value into the textarea and highlights it in the overlay', async () => {
    const el = await mount()
    el.value = '(define x 1)'
    await el.updateComplete
    expect(ta(el).value).toBe('(define x 1)')
    expect(hl(el).innerHTML).toContain('tok-keyword')
  })

  it('emits input with the new value on typing', async () => {
    const el = await mount()
    const spy = vi.fn()
    el.addEventListener('input', (e) => spy((e as CustomEvent).detail.value))
    ta(el).value = '42'
    ta(el).dispatchEvent(new InputEvent('input', { bubbles: true }))
    expect(spy).toHaveBeenCalledWith('42')
    expect(el.value).toBe('42')
  })

  it('Tab inserts tab-size spaces instead of moving focus', async () => {
    const el = await mount('tab-size="2"')
    const t = ta(el)
    t.focus()
    t.selectionStart = t.selectionEnd = 0
    t.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true, cancelable: true }))
    expect(t.value.startsWith('  ')).toBe(true)
  })

  it('forwards the testid onto the inner textarea (for e2e .fill through shadow DOM)', async () => {
    const el = await mount('testid="cell-textarea"')
    expect(ta(el).getAttribute('data-testid')).toBe('cell-textarea')
  })

  it('lets native keydown reach the host (composed) so hosts can bind Shift+Enter', async () => {
    const el = await mount()
    const spy = vi.fn()
    el.addEventListener('keydown', (e) => { if ((e as KeyboardEvent).shiftKey) spy() })
    ta(el).dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', shiftKey: true, bubbles: true, composed: true }))
    expect(spy).toHaveBeenCalled()
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ui && npx vitest run tests/sema-code-editor.test.ts`
Expected: FAIL — cannot resolve `../src/lib/sema-code-editor.js`.

- [ ] **Step 3: Write the implementation**

```ts
// ui/src/lib/sema-code-editor.ts
import { html, css, unsafeCSS } from 'lit'
import { property, state } from 'lit/decorators.js'
import { unsafeHTML } from 'lit/directives/unsafe-html.js'
import { SemaElement } from '../internal/sema-element.js'
import { highlightSemaSync } from '../internal/sema-tokenize.js'
import { TextareaUndo } from '../internal/textarea-undo.js'
import syntaxStyles from '../styles/syntax.css?inline'
import scrollbarStyles from '../styles/scrollbar.css?inline'

/**
 * `<sema-code-editor>` — an editable, syntax-highlighting code editor.
 *
 * A transparent `<textarea>` (real caret/selection/IME) sits over an `aria-hidden`
 * overlay painted by a synchronous highlighter (default: Sema). Repaint is
 * rAF-debounced; the overlay and optional gutter scroll-sync to the textarea.
 * Extracted from the sema.run playground editor.
 */
export class SemaCodeEditor extends SemaElement {
  static styles = [
    SemaElement.base,
    unsafeCSS(syntaxStyles),
    unsafeCSS(scrollbarStyles),
    css`
      :host { display: block; }
      .wrap { position: relative; display: flex; background: var(--bg-editor, #0a0a0a);
        border: 1px solid var(--border, #1e1e1e); border-radius: var(--radius-sm, 4px); }
      .gutter { flex: 0 0 auto; padding: var(--space-sm, 8px) 0; text-align: right;
        color: var(--text-tertiary, #5a5448); user-select: none; overflow: hidden;
        font-family: var(--mono, monospace); font-size: 0.82rem; line-height: 1.7; }
      .gutter div { padding: 0 0.6em 0 0.9em; }
      .stack { position: relative; flex: 1 1 auto; overflow: hidden; }
      .hl, textarea {
        margin: 0; padding: var(--space-sm, 8px) var(--space-md, 12px);
        font-family: var(--mono, 'JetBrains Mono', monospace); font-size: 0.82rem;
        line-height: 1.7; tab-size: 2; white-space: pre; overflow-wrap: normal;
        border: 0; box-sizing: border-box; letter-spacing: normal;
      }
      .hl { position: absolute; inset: 0; pointer-events: none; overflow: auto;
        color: var(--text-primary, #d8d0c0); }
      textarea {
        position: relative; width: 100%; height: 100%; resize: none; background: transparent;
        color: transparent; caret-color: var(--text-primary, #d8d0c0); outline: none;
        overflow: auto;
      }
      :host([autosize]) .stack { min-height: 1.7em; }
      textarea::selection { background: var(--gold-dim, #3a3320); }
    `,
  ]

  @property() value = ''
  @property({ reflect: true }) lang = 'sema'
  @property() placeholder = ''
  @property({ type: Boolean, reflect: true }) readonly = false
  @property({ type: Boolean, reflect: true }) gutter = false
  @property({ type: Boolean, reflect: true }) autosize = false
  @property({ type: Number, attribute: 'tab-size' }) tabSize = 2
  @property() testid = ''

  /** Swappable synchronous highlighter (code, lang) → overlay inner HTML. */
  static highlighter: (code: string, lang: string) => string = highlightSemaSync

  @state() private _html = ''
  private _undo?: TextareaUndo
  private _raf = 0

  private get _ta(): HTMLTextAreaElement | null {
    return this.shadowRoot?.querySelector('textarea') ?? null
  }

  firstUpdated() {
    const t = this._ta
    if (t) this._undo = new TextareaUndo(t, { onChange: () => this._onInput() })
    this._repaint()
  }

  updated(changed: Map<string, unknown>) {
    if (changed.has('value')) {
      this._repaint()
      if (this.autosize) this._grow()
    }
  }

  private _repaint() {
    this._html = SemaCodeEditor.highlighter(this.value, this.lang)
  }

  private _scheduleRepaint() {
    cancelAnimationFrame(this._raf)
    this._raf = requestAnimationFrame(() => this._repaint())
  }

  private _grow() {
    const t = this._ta
    if (!t) return
    t.style.height = 'auto'
    t.style.height = `${t.scrollHeight}px`
  }

  private _onInput = () => {
    const t = this._ta
    if (!t) return
    this.value = t.value
    this._scheduleRepaint()
    if (this.autosize) this._grow()
    this.dispatchEvent(new CustomEvent('input', { detail: { value: this.value }, bubbles: true, composed: true }))
  }

  private _onChange = () => {
    this.dispatchEvent(new CustomEvent('change', { detail: { value: this.value }, bubbles: true, composed: true }))
  }

  private _onScroll = () => {
    const t = this._ta
    const overlay = this.shadowRoot?.querySelector('.hl') as HTMLElement | null
    const gut = this.shadowRoot?.querySelector('.gutter') as HTMLElement | null
    if (t && overlay) { overlay.scrollTop = t.scrollTop; overlay.scrollLeft = t.scrollLeft }
    if (t && gut) gut.scrollTop = t.scrollTop
  }

  private _onKeydown = (e: KeyboardEvent) => {
    if (e.key === 'Tab' && !e.metaKey && !e.ctrlKey) {
      e.preventDefault()
      const t = this._ta!
      const s = t.selectionStart, en = t.selectionEnd
      const pad = ' '.repeat(this.tabSize)
      t.value = t.value.slice(0, s) + pad + t.value.slice(en)
      t.selectionStart = t.selectionEnd = s + pad.length
      this._onInput()
    }
    // All other keydowns bubble (composed) so the host can bind Shift+Enter etc.
  }

  private _lineNumbers() {
    const n = (this.value.match(/\n/g)?.length ?? 0) + 1
    return Array.from({ length: n }, (_, i) => html`<div>${i + 1}</div>`)
  }

  render() {
    return html`
      <div class="wrap">
        ${this.gutter ? html`<div class="gutter" part="gutter">${this._lineNumbers()}</div>` : ''}
        <div class="stack">
          <div class="hl sema-scroll" part="highlight" aria-hidden="true">${unsafeHTML(this._html || '\n')}</div>
          <textarea
            class="sema-scroll"
            part="textarea"
            part-testid=${this.testid}
            data-testid=${this.testid || undefined}
            .value=${this.value}
            ?readonly=${this.readonly}
            placeholder=${this.placeholder}
            spellcheck="false"
            autocapitalize="off"
            autocomplete="off"
            @input=${this._onInput}
            @change=${this._onChange}
            @scroll=${this._onScroll}
            @keydown=${this._onKeydown}
          ></textarea>
        </div>
      </div>
    `
  }
}

declare global {
  interface HTMLElementTagNameMap { 'sema-code-editor': SemaCodeEditor }
}
customElements.define('sema-code-editor', SemaCodeEditor)
```

> Note (execution): `data-testid=${this.testid || undefined}` uses Lit's attribute removal on `undefined`. Remove the stray `part-testid` line if the linter flags it — it is not required. Visual caret/overlay alignment (font metrics, padding parity between `.hl` and `textarea`) may need a pixel tweak during `npm run dev`; the tests assert value/events/highlight, not alignment.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ui && npx vitest run tests/sema-code-editor.test.ts`
Expected: PASS (5 cases).

- [ ] **Step 5: Manual alignment check (no test)**

Run: `cd ui && npm run dev`, open the component in `index.html` (add a demo instance), type multi-line Sema, confirm caret sits on the highlighted glyphs and scrolling keeps overlay+gutter aligned. Adjust padding/line-height parity if needed.

- [ ] **Step 6: Commit**

```bash
git add ui/src/lib/sema-code-editor.ts ui/tests/sema-code-editor.test.ts
git commit -m "feat(ui): sema-code-editor (highlighting editable code editor)"
```

---

### Task 4: `sema-markdown` renderer + `marked` dependency

Render a markdown string as token-styled HTML; fenced code uses the existing async Shiki highlighter.

**Files:**
- Modify: `ui/package.json` (add `marked`)
- Create: `ui/src/lib/sema-markdown.ts`
- Test: `ui/tests/sema-markdown.test.ts`

**Interfaces:**
- Consumes: `marked`, `highlightToHtml`/`canHighlight` (`../internal/syntax-highlight.js`), `SemaElement`.
- Produces: `class SemaMarkdown` (tag `sema-markdown`) with `value: string` (or slotted text), `testid: string`. Renders sanitized HTML into `part="content"`.

- [ ] **Step 1: Add the dependency**

Run: `cd ui && npm install marked@^12`
Expected: `marked` appears under `dependencies` in `ui/package.json`.

- [ ] **Step 2: Write the failing test**

```ts
// ui/tests/sema-markdown.test.ts
import { beforeEach, describe, expect, it } from 'vitest'
import '../src/lib/sema-markdown.js'
import type { SemaMarkdown } from '../src/lib/sema-markdown.js'

async function mount(value: string): Promise<SemaMarkdown> {
  document.body.innerHTML = '<sema-markdown></sema-markdown>'
  const el = document.body.querySelector('sema-markdown') as SemaMarkdown
  el.value = value
  await el.updateComplete
  return el
}
const content = (el: SemaMarkdown) => el.shadowRoot!.querySelector('[part="content"]') as HTMLElement

describe('sema-markdown', () => {
  beforeEach(() => { document.body.innerHTML = '' })

  it('renders headings, emphasis, lists, and links', async () => {
    const el = await mount('# Title\n\n- **bold** item\n\n[x](https://a.b)')
    const h = content(el)
    expect(h.querySelector('h1')?.textContent).toBe('Title')
    expect(h.querySelector('li strong')?.textContent).toBe('bold')
    const a = h.querySelector('a') as HTMLAnchorElement
    expect(a.getAttribute('href')).toBe('https://a.b')
    expect(a.getAttribute('rel')).toContain('noopener')
  })

  it('strips <script> and inline event handlers (sanitization)', async () => {
    const el = await mount('<script>window.x=1</script>\n\n<img src=x onerror="alert(1)">')
    const h = content(el).innerHTML
    expect(h).not.toContain('<script')
    expect(h).not.toContain('onerror')
  })

  it('renders fenced code as a highlighted block', async () => {
    const el = await mount('```sema\n(define x 1)\n```')
    expect(content(el).querySelector('pre code')).toBeTruthy()
  })
})
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cd ui && npx vitest run tests/sema-markdown.test.ts`
Expected: FAIL — cannot resolve `../src/lib/sema-markdown.js`.

- [ ] **Step 4: Write the implementation**

```ts
// ui/src/lib/sema-markdown.ts
import { html, css, unsafeCSS } from 'lit'
import { property, state } from 'lit/decorators.js'
import { unsafeHTML } from 'lit/directives/unsafe-html.js'
import { marked } from 'marked'
import { SemaElement } from '../internal/sema-element.js'
import { highlightToHtml, canHighlight, escapeHtml } from '../internal/syntax-highlight.js'
import syntaxStyles from '../styles/syntax.css?inline'

/**
 * `<sema-markdown>` — renders a markdown string (`value` or slotted text) as
 * token-styled HTML. Fenced code is highlighted with the shared Shiki pipeline so
 * it matches `sema-code`. Output is sanitized (tag/attr allowlist) before injection.
 */
export class SemaMarkdown extends SemaElement {
  static styles = [
    SemaElement.base,
    unsafeCSS(syntaxStyles),
    css`
      :host { display: block; color: var(--text-primary, #d8d0c0); }
      [part='content'] { font-family: var(--sans, system-ui); line-height: 1.6; }
      h1, h2, h3, h4 { font-family: var(--serif, Cormorant, serif); line-height: 1.2; }
      code { font-family: var(--mono, monospace); background: var(--bg-editor, #0a0a0a);
        padding: 0.1em 0.3em; border-radius: 3px; }
      pre { background: var(--bg-editor, #0a0a0a); border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-sm, 4px); padding: var(--space-md, 12px); overflow-x: auto; }
      pre code { background: none; padding: 0; }
      a { color: var(--gold, #d4a537); }
      table { border-collapse: collapse; }
      th, td { border: 1px solid var(--border, #1e1e1e); padding: 0.3em 0.6em; }
    `,
  ]

  @property() value = ''
  @property() testid = ''
  @state() private _html = ''
  private _slotText = ''

  updated(changed: Map<string, unknown>) {
    if (changed.has('value')) void this._render()
  }

  private async _render() {
    const src = this.value || this._slotText
    let out: string
    try {
      out = await marked.parse(src, { async: true, gfm: true }) as string
      out = await this._highlightFences(out)
    } catch {
      out = `<pre>${escapeHtml(src)}</pre>`
    }
    this._html = sanitize(out)
  }

  /** Replace ```lang fenced blocks with Shiki-highlighted markup (best-effort). */
  private async _highlightFences(rendered: string): Promise<string> {
    // marked emits <pre><code class="language-xxx">…</code></pre>; upgrade known langs.
    const re = /<pre><code class="language-([\w-]+)">([\s\S]*?)<\/code><\/pre>/g
    const jobs: Promise<void>[] = []
    const parts: string[] = []
    let last = 0, m: RegExpExecArray | null
    while ((m = re.exec(rendered))) {
      const [full, lang, body] = m
      parts.push(rendered.slice(last, m.index))
      const idx = parts.push('') - 1
      last = m.index + full.length
      if (canHighlight(lang)) {
        const decoded = decodeEntities(body)
        jobs.push(highlightToHtml(decoded, lang).then((hl) => { parts[idx] = `<pre><code>${hl}</code></pre>` }))
      } else {
        parts[idx] = m[0]
      }
    }
    parts.push(rendered.slice(last))
    await Promise.all(jobs)
    return parts.join('')
  }

  private _onSlot = (e: Event) => {
    this._slotText = (e.target as HTMLSlotElement).assignedNodes({ flatten: true })
      .map((n) => n.textContent ?? '').join('')
    if (!this.value) void this._render()
  }

  render() {
    return html`
      <div part="content" data-testid=${this.testid || undefined}>${unsafeHTML(this._html)}</div>
      <slot @slotchange=${this._onSlot} hidden></slot>
    `
  }
}

function decodeEntities(s: string): string {
  return s.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&amp;/g, '&')
}

/** Small allowlist sanitizer: strips <script>/<style>, event-handler attrs, javascript: URLs. */
function sanitize(dirty: string): string {
  const tpl = document.createElement('template')
  tpl.innerHTML = dirty
  tpl.content.querySelectorAll('script, style, iframe, object, embed').forEach((n) => n.remove())
  tpl.content.querySelectorAll('*').forEach((el) => {
    for (const attr of Array.from(el.attributes)) {
      const name = attr.name.toLowerCase()
      const val = attr.value.trim().toLowerCase()
      if (name.startsWith('on')) el.removeAttribute(attr.name)
      else if ((name === 'href' || name === 'src') && val.startsWith('javascript:')) el.removeAttribute(attr.name)
    }
    if (el.tagName === 'A') { el.setAttribute('rel', 'noopener noreferrer'); el.setAttribute('target', '_blank') }
  })
  return tpl.innerHTML
}

declare global {
  interface HTMLElementTagNameMap { 'sema-markdown': SemaMarkdown }
}
customElements.define('sema-markdown', SemaMarkdown)
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd ui && npx vitest run tests/sema-markdown.test.ts`
Expected: PASS (3 cases).

- [ ] **Step 6: Commit**

```bash
git add ui/package.json ui/package-lock.json ui/src/lib/sema-markdown.ts ui/tests/sema-markdown.test.ts
git commit -m "feat(ui): sema-markdown renderer (marked + Shiki fences + sanitize)"
```

---

### Task 5: `sema-editable-markdown` compound

Compose the editor + renderer with the click-to-edit / blur-to-render / Shift+Enter toggle.

**Files:**
- Create: `ui/src/lib/sema-editable-markdown.ts`
- Test: `ui/tests/sema-editable-markdown.test.ts`

**Interfaces:**
- Consumes: `sema-code-editor` (Task 3), `sema-markdown` (Task 4), `SemaElement`.
- Produces: `class SemaEditableMarkdown` (tag `sema-editable-markdown`) with `value: string`, `placeholder: string`, `readonly: boolean`. Events: `input` (`{ value }`) per keystroke, `change` (`{ value }`) on commit (blur/Shift+Enter). Forwards testids `markdown-rendered` (view) and `cell-textarea` (edit).

- [ ] **Step 1: Write the failing test**

```ts
// ui/tests/sema-editable-markdown.test.ts
import { beforeEach, describe, expect, it, vi } from 'vitest'
import '../src/lib/sema-editable-markdown.js'
import type { SemaEditableMarkdown } from '../src/lib/sema-editable-markdown.js'

async function mount(value = ''): Promise<SemaEditableMarkdown> {
  document.body.innerHTML = '<sema-editable-markdown></sema-editable-markdown>'
  const el = document.body.querySelector('sema-editable-markdown') as SemaEditableMarkdown
  el.value = value
  await el.updateComplete
  return el
}
const view = (el: SemaEditableMarkdown) => el.shadowRoot!.querySelector('sema-markdown')
const editor = (el: SemaEditableMarkdown) => el.shadowRoot!.querySelector('sema-code-editor')

describe('sema-editable-markdown', () => {
  beforeEach(() => { document.body.innerHTML = '' })

  it('starts in rendered view when it has content', async () => {
    const el = await mount('# Hi')
    expect(view(el)).toBeTruthy()
    expect(editor(el)).toBeFalsy()
  })

  it('click on the rendered view enters edit mode', async () => {
    const el = await mount('# Hi')
    ;(view(el) as HTMLElement).click()
    await el.updateComplete
    expect(editor(el)).toBeTruthy()
  })

  it('emits change with the source when committed (Shift+Enter)', async () => {
    const el = await mount('')
    const spy = vi.fn()
    el.addEventListener('change', (e) => spy((e as CustomEvent).detail.value))
    // enter edit mode (empty starts in edit)
    const ed = editor(el) as HTMLElement & { value: string }
    ed.value = '# New'
    ed.dispatchEvent(new CustomEvent('input', { detail: { value: '# New' }, bubbles: true, composed: true }))
    ed.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', shiftKey: true, bubbles: true, composed: true }))
    await el.updateComplete
    expect(spy).toHaveBeenCalledWith('# New')
    expect(view(el)).toBeTruthy()
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd ui && npx vitest run tests/sema-editable-markdown.test.ts`
Expected: FAIL — cannot resolve module.

- [ ] **Step 3: Write the implementation**

```ts
// ui/src/lib/sema-editable-markdown.ts
import { html, css } from 'lit'
import { property, state } from 'lit/decorators.js'
import { SemaElement } from '../internal/sema-element.js'
import './sema-code-editor.js'
import './sema-markdown.js'

/**
 * `<sema-editable-markdown>` — edit-in-place markdown. Shows rendered markdown;
 * click to edit (highlighted markdown source), blur or Shift+Enter to render.
 * Owns the view↔edit toggle so hosts only bind `value` + `change`.
 */
export class SemaEditableMarkdown extends SemaElement {
  static styles = [SemaElement.base, css`
    :host { display: block; }
    .empty { color: var(--text-tertiary, #5a5448); font-style: italic; cursor: text; padding: 0.4em 0; }
  `]

  @property() value = ''
  @property() placeholder = 'Empty markdown — click to edit'
  @property({ type: Boolean, reflect: true }) readonly = false
  @state() private _editing = false

  connectedCallback() {
    super.connectedCallback()
    // Empty cells open in edit mode; non-empty cells start rendered.
    this._editing = this.value.trim() === '' && !this.readonly
  }

  private _edit = () => {
    if (this.readonly) return
    this._editing = true
    this.updateComplete.then(() => {
      const ed = this.shadowRoot?.querySelector('sema-code-editor') as HTMLElement | null
      ed?.querySelector('textarea')?.focus?.()
      ;(ed as unknown as { focus?: () => void })?.focus?.()
    })
  }

  private _commit() {
    this._editing = false
    this.dispatchEvent(new CustomEvent('change', { detail: { value: this.value }, bubbles: true, composed: true }))
  }

  private _onInput = (e: Event) => {
    this.value = (e as CustomEvent).detail.value
    this.dispatchEvent(new CustomEvent('input', { detail: { value: this.value }, bubbles: true, composed: true }))
  }
  private _onBlur = () => { if (this.value.trim()) this._commit() }
  private _onKeydown = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && e.shiftKey) { e.preventDefault(); this._commit() }
  }

  render() {
    if (this._editing) {
      return html`<sema-code-editor
        lang="markdown" autosize testid="cell-textarea"
        .value=${this.value} .placeholder=${this.placeholder}
        @input=${this._onInput} @blur=${this._onBlur} @keydown=${this._onKeydown}
      ></sema-code-editor>`
    }
    if (!this.value.trim()) {
      return html`<div class="empty" data-testid="markdown-rendered" @click=${this._edit}>${this.placeholder}</div>`
    }
    return html`<sema-markdown
      testid="markdown-rendered" .value=${this.value} @click=${this._edit}
    ></sema-markdown>`
  }
}

declare global {
  interface HTMLElementTagNameMap { 'sema-editable-markdown': SemaEditableMarkdown }
}
customElements.define('sema-editable-markdown', SemaEditableMarkdown)
```

> Note (execution): `sema-code-editor` doesn't emit its own `blur`; the native `blur` from the inner textarea bubbles composed and is caught by `@blur` on the host element. Verify in the manual step; if it doesn't propagate, add a `blur` re-dispatch to `sema-code-editor`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd ui && npx vitest run tests/sema-editable-markdown.test.ts`
Expected: PASS (3 cases).

- [ ] **Step 5: Commit**

```bash
git add ui/src/lib/sema-editable-markdown.ts ui/tests/sema-editable-markdown.test.ts
git commit -m "feat(ui): sema-editable-markdown compound (edit-in-place)"
```

---

### Task 6: Export the new components + build

**Files:**
- Modify: `ui/src/lib/index.ts`, `ui/src/index.ts`

- [ ] **Step 1: Add exports to `ui/src/lib/index.ts`**

Append:

```ts
export { SemaCodeEditor } from './sema-code-editor.js';
export { SemaMarkdown } from './sema-markdown.js';
export { SemaEditableMarkdown } from './sema-editable-markdown.js';
```

- [ ] **Step 2: Add to the aggregate export in `ui/src/index.ts`**

Add `SemaCodeEditor, SemaMarkdown, SemaEditableMarkdown,` to the `export { … } from './lib/index.js'` list.

- [ ] **Step 3: Typecheck, lint, and build**

Run: `cd ui && npm run typecheck && npm run lint && npm run test && npm run build`
Expected: typecheck clean; lint clean; all tests pass; `dist/sema-ui.js` written.

- [ ] **Step 4: Commit**

```bash
git add ui/src/lib/index.ts ui/src/index.ts ui/dist
git commit -m "feat(ui): export sema-code-editor + markdown components; rebuild bundle"
```

---

### Task 7: Vendor the bundle into the notebook + serve it

**Files:**
- Modify: `Makefile`
- Create (generated): `crates/sema-notebook/src/ui/vendor/sema-ui.js`
- Modify: `crates/sema-notebook/src/ui.rs`
- Test: `crates/sema-notebook/tests/` (asset route)

- [ ] **Step 1: Add the vendor target to `Makefile`**

```make
# Build @sema/ui and vendor its bundle into the notebook crate (embedded via
# include_str! like the offline fonts). Re-run after changing anything in ui/.
notebook-ui-vendor:
	cd ui && npm run build
	mkdir -p crates/sema-notebook/src/ui/vendor
	cp ui/dist/sema-ui.js crates/sema-notebook/src/ui/vendor/sema-ui.js
```

- [ ] **Step 2: Run it**

Run: `make notebook-ui-vendor`
Expected: `crates/sema-notebook/src/ui/vendor/sema-ui.js` exists (~non-empty).

- [ ] **Step 3: Write the failing Rust test**

Add to `crates/sema-notebook/tests/` (new file `assets_test.rs`) or an existing integration test:

```rust
#[test]
fn serves_vendored_sema_ui_bundle() {
    let asset = sema_notebook::ui::asset("vendor/sema-ui.js");
    assert!(asset.is_some(), "vendored @sema/ui bundle must be served");
    let (body, ct) = asset.unwrap();
    assert!(body.contains("sema-code-editor"), "bundle should define the editor element");
    assert_eq!(ct, "application/javascript");
}
```

Confirm `ui` module + `asset` are `pub` (they are: `pub fn asset`). If `ui` isn't re-exported from the crate root, add `pub mod ui;` visibility or a `pub use`.

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test -p sema-notebook serves_vendored`
Expected: FAIL — `asset("vendor/sema-ui.js")` returns `None`.

- [ ] **Step 5: Serve the asset in `ui.rs`**

In `crates/sema-notebook/src/ui.rs`, add a match arm in `asset()` and a helper:

```rust
        "notebook.js" => Some((js().to_string(), "application/javascript".to_string())),
        "vendor/sema-ui.js" => Some((
            sema_ui_js().to_string(),
            "application/javascript".to_string(),
        )),
        _ => None,
```

```rust
fn sema_ui_js() -> &'static str {
    include_str!("ui/vendor/sema-ui.js")
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p sema-notebook serves_vendored`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add Makefile crates/sema-notebook/src/ui.rs crates/sema-notebook/src/ui/vendor/sema-ui.js crates/sema-notebook/tests/assets_test.rs
git commit -m "build(notebook): vendor + serve the @sema/ui bundle (single-binary)"
```

---

### Task 8: Rewire notebook code cells to `sema-code-editor`

**Files:**
- Modify: `crates/sema-notebook/src/ui/index.html`, `crates/sema-notebook/src/ui/notebook.js`

- [ ] **Step 1: Load the bundle in `index.html`**

In `<head>` (after the existing scripts), add:

```html
<script type="module" src="vendor/sema-ui.js"></script>
```

- [ ] **Step 2: Replace the code-cell editor markup**

In `crates/sema-notebook/src/ui/index.html`, replace the `<textarea …>` inside the
`data-testid="cell-editor"` block (lines ~150-164) with:

```html
                    <sema-code-editor
                              lang="sema"
                              autosize
                              testid="cell-textarea"
                              :data-id="cell.id"
                              :value="cell.source"
                              @focus="focusedCellId = cell.id"
                              @blur="onBlur(cell)"
                              @input="cell.source = $event.detail.value"
                              @keydown.shift.enter.prevent="handleShiftEnter(cell)"
                              @keydown.meta.enter.prevent="evalCellStay(cell.id)"
                              @keydown.ctrl.enter.prevent="evalCellStay(cell.id)"
                              @keydown.escape="focusedCellId = null"
                    ></sema-code-editor>
```

(The editor handles Tab and undo internally, so `@keydown.tab` and `autoResize`/`x-init` are gone. Alpine's `@keydown.shift.enter` etc. still fire because native keydown bubbles composed from the shadow textarea.)

- [ ] **Step 3: Simplify `notebook.js` handlers no longer needed for code cells**

In `crates/sema-notebook/src/ui/notebook.js`, delete `insertTab` and `autoResize`
(the editor owns indentation + sizing). Keep `onBlur`, `handleShiftEnter`, `evalCellStay`,
`focusedCellId`. `onBlur(cell)` still persists source (unchanged).

- [ ] **Step 4: Manual verification**

Run: `make example-notebook-serve` (or `cargo run -- notebook serve <file>`), open the
browser, confirm: code cells show highlighted Sema, typing works, Tab indents, Cmd+Z undoes,
Shift+Enter evaluates and advances, Cmd/Ctrl+Enter evaluates in place, Escape blurs.

- [ ] **Step 5: Commit**

```bash
git add crates/sema-notebook/src/ui/index.html crates/sema-notebook/src/ui/notebook.js
git commit -m "feat(notebook): code cells use sema-code-editor"
```

---

### Task 9: Rewire notebook markdown cells to `sema-editable-markdown`

**Files:**
- Modify: `crates/sema-notebook/src/ui/index.html`, `crates/sema-notebook/src/ui/notebook.js`

- [ ] **Step 1: Replace the markdown-cell templates**

In `index.html`, replace BOTH the "Markdown rendered view" template (lines ~142-145) and
remove the markdown branch from the editor template condition. The markdown case becomes a
single element rendered when `cell.cell_type === 'markdown'`:

```html
                <!-- Markdown cell: edit-in-place -->
                <template x-if="cell.cell_type === 'markdown'">
                  <sema-editable-markdown
                    :value="cell.source"
                    @input="cell.source = $event.detail.value"
                    @change="onMarkdownChange(cell, $event)"
                  ></sema-editable-markdown>
                </template>
```

Change the editor template condition from `cell.cell_type === 'code' || !cell._rendered`
to just `cell.cell_type === 'code'` (markdown no longer uses the shared editor template).

- [ ] **Step 2: Update `notebook.js`**

- Delete `renderMarkdown`, `editMarkdown`, and `escapeHtml` (only used by `renderMarkdown`).
- In `handleShiftEnter`, drop the markdown branch (the component owns it); keep the code branch:

```js
    handleShiftEnter(cell) {
      this.evalCell(cell.id);
    },
```

- In `onBlur(cell)`, drop the markdown `_rendered` branch; keep `persistSource(cell)`.
- Remove `_rendered` bookkeeping in `load()` (the markdown auto-render loop) — no longer used.
- Add:

```js
    onMarkdownChange(cell, e) {
      cell.source = e.detail.value;
      this.persistSource(cell);
    },
```

- [ ] **Step 3: Manual verification**

Add a markdown cell, type `# Hello **world**` + a `- list`, blur → renders; click → edits;
Shift+Enter → renders; empty cell shows the "click to edit" affordance; Save → reload keeps
it rendered with the saved source.

- [ ] **Step 4: Commit**

```bash
git add crates/sema-notebook/src/ui/index.html crates/sema-notebook/src/ui/notebook.js
git commit -m "feat(notebook): markdown cells use sema-editable-markdown; drop regex renderer"
```

---

### Task 10: e2e adaptation + full gate

**Files:**
- Modify (if needed): `crates/sema-notebook/tests/e2e/notebook.spec.ts`

- [ ] **Step 1: Run the e2e suite as-is**

Run: `make test-notebook-e2e`
Expected: most pass. Watch for failures where `getByTestId('cell-textarea')` no longer
resolves to an editable node, or `markdown-rendered` structure changed.

- [ ] **Step 2: Fix selectors that broke**

For the code editor, the testid is on the inner `<textarea>` (Task 3 forwards it) and
Playwright pierces open shadow roots, so `getByTestId('cell-textarea').fill(...)` should work.
If a test used `.locator('textarea')` directly under a cell, change it to
`getByTestId('cell-textarea')`. For markdown, `markdown-rendered` now sits on the
`sema-editable-markdown` view; `getByTestId('markdown-rendered')` still resolves. Update any
assertion that reached into the old `.markdown-rendered` inner HTML to target the component.

Example fix pattern (only apply where a test fails):

```ts
// before: const ta = cell.locator('textarea')
const ta = cell.getByTestId('cell-textarea')
```

- [ ] **Step 3: Re-run e2e until green**

Run: `make test-notebook-e2e`
Expected: PASS.

- [ ] **Step 4: Full gate**

Run:
```bash
cd ui && npm run lint && npm run test && npm run build && cd ..
make notebook-ui-vendor
cargo test -p sema-notebook
make test-notebook-e2e
make example-notebook
```
Expected: all green; `make example-notebook` renders without errors.

- [ ] **Step 5: Commit any e2e fixes**

```bash
git add crates/sema-notebook/tests/e2e/notebook.spec.ts
git commit -m "test(notebook): adapt e2e selectors to shadow-DOM editor"
```

---

## Self-Review

**Spec coverage:**
- §4.1 `sema-code-editor` → Tasks 1–3. §4.2 `sema-markdown` → Task 4. §4.3
  `sema-editable-markdown` → Task 5. §4.4 bundling → Tasks 6–7. §4.5 notebook wiring →
  Tasks 8–9. §2.2 e2e shadow-DOM adaptation → Tasks 3 (testid forwarding) + 10. §2.1
  single-binary → Task 7 (`include_str!`). §7 testing → per-task tests + Task 10 gate. All
  spec sections map to a task.

**Placeholder scan:** No TBD/TODO/"add error handling"; every code step shows complete code.
Two explicit "Note (execution)" callouts flag visual-alignment and blur-propagation checks
that a unit test can't assert — these are verification steps, not missing code.

**Type consistency:** `highlightSemaSync(code, lang)` defined in Task 1, consumed as the
default `SemaCodeEditor.highlighter` in Task 3. `TextareaUndo` ctor signature matches between
Task 2 and Task 3. `sema-code-editor` `input`/`change` `CustomEvent<{ value }>` produced in
Task 3, consumed by `sema-editable-markdown` (Task 5) and the notebook (Tasks 8–9). `asset()`
return `(String, String)` matches the existing `ui.rs` signature (Task 7).

## Execution Handoff

Two execution options:
1. **Subagent-Driven (recommended)** — a fresh subagent per task with review between tasks.
2. **Inline Execution** — execute tasks in this session with checkpoints.
