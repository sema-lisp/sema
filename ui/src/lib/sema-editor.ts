import { html, css, unsafeCSS, nothing } from 'lit';
import { property, state } from 'lit/decorators.js';
import { unsafeHTML } from 'lit/directives/unsafe-html.js';
import { live } from 'lit/directives/live.js';
import { ifDefined } from 'lit/directives/if-defined.js';
import { SemaElement } from '../internal/sema-element.js';
import { highlightToHtmlSync, preloadLanguage } from '../internal/syntax-highlight.js';
import { TextareaUndo } from '../internal/textarea-undo.js';
import syntaxStyles from '../styles/syntax.css?inline';
import scrollbarStyles from '../styles/scrollbar.css?inline';

const SUPPORTS_FIELD_SIZING =
  typeof CSS !== 'undefined' && typeof CSS.supports === 'function' && CSS.supports('field-sizing', 'content');

/**
 * `<sema-editor>` — the single editable, syntax-highlighting code editor for the
 * Sema ecosystem (notebook cells, the playground, anywhere code is edited).
 *
 * A transparent `<textarea>` (real caret / selection / IME / undo) sits over an
 * `aria-hidden` overlay of per-line `<div>`s painted by the **one** shared Shiki
 * highlighter — so it renders identically to `<sema-code>` and works for every
 * language, not just Sema. Highlighting is synchronous after a one-time async
 * grammar warmup; until warm it shows escaped plain text and upgrades in place.
 *
 * ## Gutter (line numbers + breakpoints + debug line)
 * Opt in with `line-numbers`. The gutter is a *mechanism*: it renders the markers it
 * is told about and reports clicks — it owns no policy. Consumers drive it:
 * - `.breakpoints = [3, 7]` — lines that show a breakpoint dot.
 * - `current-line="5"` — the active/debug line (highlighted in gutter + editor).
 * - listen for `gutter-click` (`detail: { line }`) to toggle/snap breakpoints, etc.
 *
 * Events: `input` and `change` as `CustomEvent<{ value }>`; native `keydown` bubbles
 * (composed) so hosts can bind Shift+Enter etc.
 */
export class SemaEditor extends SemaElement {
  static styles = [
    SemaElement.base,
    unsafeCSS(syntaxStyles),
    unsafeCSS(scrollbarStyles),
    css`
      :host {
        display: block;
      }
      .wrap {
        position: relative;
        display: flex;
        height: 100%;
        background: var(--bg-editor, #0a0a0a);
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-sm, 4px);
        overflow: hidden;
      }
      :host([autosize]) .wrap {
        height: auto;
      }
      /* gutter */
      .gutter {
        flex: 0 0 auto;
        overflow: hidden;
        user-select: none;
        background: var(--bg-editor, #0a0a0a);
        color: var(--text-tertiary, #5a5448);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: 0.82rem;
        line-height: 1.7;
        padding: var(--space-sm, 8px) 0;
        text-align: right;
      }
      .gl {
        position: relative;
        padding: 0 0.55em 0 1.4em;
        cursor: pointer;
      }
      .gl:hover {
        color: var(--text-secondary, #a89f8c);
      }
      .gl.cur {
        color: var(--gold, #d4a537);
      }
      .gl.bp::before {
        content: '';
        position: absolute;
        left: 0.45em;
        top: 50%;
        transform: translateY(-50%);
        width: 0.55em;
        height: 0.55em;
        border-radius: 50%;
        background: var(--danger, #e5484d);
      }
      /* editor stack */
      .stack {
        position: relative;
        flex: 1 1 auto;
        overflow: hidden;
        min-height: 1.7em;
      }
      .hl,
      textarea {
        margin: 0;
        padding: var(--space-sm, 8px) var(--space-md, 12px);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: 0.82rem;
        line-height: 1.7;
        tab-size: 2;
        white-space: pre;
        border: 0;
        box-sizing: border-box;
        letter-spacing: normal;
      }
      :host([autosize]) .hl,
      :host([autosize]) textarea {
        white-space: pre-wrap;
        word-break: break-word;
        overflow-wrap: break-word;
      }
      .hl {
        position: absolute;
        inset: 0;
        overflow: hidden;
        pointer-events: none;
        color: var(--text-primary, #d8d0c0);
      }
      .ln {
        display: block;
      }
      .ln.cur {
        background: var(--bg-line-highlight, rgba(212, 165, 55, 0.09));
      }
      textarea {
        position: relative;
        display: block;
        width: 100%;
        height: 100%;
        resize: none;
        background: transparent;
        color: transparent;
        caret-color: var(--text-primary, #d8d0c0);
        outline: none;
        overflow: auto;
      }
      :host([autosize]) textarea {
        /* Grow with content via CSS where supported (no measure-timing race);
           the scrollHeight fallback in _grow() covers browsers without it. */
        field-sizing: content;
        height: auto;
        min-height: 1.7em;
        max-height: none;
        overflow: hidden;
      }
      textarea::selection {
        background: var(--gold-dim, #3a3320);
        color: transparent;
      }
    `,
  ];

  @property() value = '';
  @property({ reflect: true }) lang = 'sema';
  @property() placeholder = '';
  @property({ type: Boolean, reflect: true }) readonly = false;
  @property({ type: Boolean, reflect: true }) autosize = false;
  @property({ type: Number, attribute: 'tab-size' }) tabSize = 2;
  @property() testid = '';
  /** Show the line-number gutter (also enables breakpoint dots + current-line). */
  @property({ type: Boolean, reflect: true, attribute: 'line-numbers' }) lineNumbers = false;
  /** Lines (1-based) that display a breakpoint marker. Consumer-owned. */
  @property({ attribute: false }) breakpoints: number[] = [];
  /** The active/debug line (1-based) to highlight, or 0 for none. */
  @property({ type: Number, attribute: 'current-line' }) currentLine = 0;

  /** Highlighted source split into per-line HTML (aligns with gutter + textarea). */
  @state() private _lines: string[] = [''];
  private _undo?: TextareaUndo;

  private get _ta(): HTMLTextAreaElement | null {
    return this.shadowRoot?.querySelector('textarea') ?? null;
  }

  connectedCallback() {
    super.connectedCallback();
    void this._warm();
  }

  private async _warm() {
    await preloadLanguage(this.lang);
    this._relight(); // upgrade escaped-plain -> highlighted now that the grammar is loaded
  }

  /** Recompute the per-line highlighted overlay. Shiki tokenizes per line and never
   * spans a newline, so splitting the joined output yields valid per-line HTML. */
  private _relight() {
    const painted = highlightToHtmlSync(this.value, this.lang);
    this._lines = painted.length ? painted.split('\n') : [''];
  }

  willUpdate(changed: Map<string, unknown>) {
    if (changed.has('value') || changed.has('lang')) {
      this._relight();
      if (changed.has('lang')) void this._warm();
    }
  }

  firstUpdated() {
    const t = this._ta;
    if (t) this._undo = new TextareaUndo(t, { onChange: () => this._onInput() });
    if (this.autosize && !SUPPORTS_FIELD_SIZING) this._grow();
  }

  updated(changed: Map<string, unknown>) {
    if (changed.has('value') && this.autosize && !SUPPORTS_FIELD_SIZING) this._grow();
  }

  /** Clear the undo/redo history — call after loading unrelated content. */
  resetHistory() {
    this._undo?.reset();
  }

  /** Focus delegates to the inner textarea (the host itself isn't focusable). */
  focus() {
    this._ta?.focus();
  }

  private _grow() {
    const t = this._ta;
    if (!t) return;
    t.style.height = 'auto';
    t.style.height = `${t.scrollHeight}px`;
  }

  private _onInput = (e?: Event) => {
    // Stop the inner textarea's native `input`; we re-emit a typed CustomEvent so
    // consumers never see the value-less native one.
    e?.stopPropagation();
    const t = this._ta;
    if (!t) return;
    this.value = t.value;
    if (this.autosize && !SUPPORTS_FIELD_SIZING) this._grow();
    this.dispatchEvent(
      new CustomEvent('input', { detail: { value: this.value }, bubbles: true, composed: true }),
    );
  };

  private _onChange = (e?: Event) => {
    e?.stopPropagation();
    this.dispatchEvent(
      new CustomEvent('change', { detail: { value: this.value }, bubbles: true, composed: true }),
    );
  };

  private _onScroll = () => {
    const t = this._ta;
    const overlay = this.shadowRoot?.querySelector('.hl') as HTMLElement | null;
    const gut = this.shadowRoot?.querySelector('.gutter') as HTMLElement | null;
    if (t && overlay) {
      overlay.scrollTop = t.scrollTop;
      overlay.scrollLeft = t.scrollLeft;
    }
    if (t && gut) gut.scrollTop = t.scrollTop;
  };

  private _onKeydown = (e: KeyboardEvent) => {
    if (e.key === 'Tab' && !e.metaKey && !e.ctrlKey && !e.altKey) {
      e.preventDefault();
      if (e.shiftKey) this._dedent();
      else this._indent();
    }
  };

  /** Tab: insert spaces at the cursor, or indent every line in a multi-line selection. */
  private _indent() {
    const t = this._ta;
    if (!t) return;
    const { selectionStart: s, selectionEnd: en, value } = t;
    const pad = ' '.repeat(this.tabSize);
    if (s === en || !value.slice(s, en).includes('\n')) {
      t.value = value.slice(0, s) + pad + value.slice(en);
      t.selectionStart = t.selectionEnd = s + pad.length;
    } else {
      const lineStart = value.lastIndexOf('\n', s - 1) + 1;
      const block = value.slice(lineStart, en);
      const indented = block.replace(/^/gm, pad);
      t.value = value.slice(0, lineStart) + indented + value.slice(en);
      t.selectionStart = lineStart;
      t.selectionEnd = en + (indented.length - block.length);
    }
    t.dispatchEvent(new Event('input', { bubbles: true }));
  }

  /** Shift+Tab: remove up to `tab-size` leading spaces from each line in range. */
  private _dedent() {
    const t = this._ta;
    if (!t) return;
    const { selectionStart: s, selectionEnd: en, value } = t;
    const lineStart = value.lastIndexOf('\n', s - 1) + 1;
    const block = value.slice(lineStart, en);
    let firstRemoved = 0;
    let totalRemoved = 0;
    const out = block.split('\n').map((line, i) => {
      const lead = /^ */.exec(line)![0].length;
      const remove = Math.min(lead, this.tabSize);
      if (i === 0) firstRemoved = remove;
      totalRemoved += remove;
      return line.slice(remove);
    });
    if (totalRemoved === 0) return; // nothing to outdent (still swallow the Tab)
    t.value = value.slice(0, lineStart) + out.join('\n') + value.slice(en);
    t.selectionStart = Math.max(lineStart, s - firstRemoved);
    t.selectionEnd = en - totalRemoved;
    t.dispatchEvent(new Event('input', { bubbles: true }));
  }

  private _gutterClick(line: number) {
    this.dispatchEvent(
      new CustomEvent('gutter-click', { detail: { line }, bubbles: true, composed: true }),
    );
  }

  private _onGutterKeydown(e: KeyboardEvent, line: number) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      this._gutterClick(line);
    }
  }

  render() {
    const bp = new Set(this.breakpoints);
    const cur = this.currentLine;
    const overlay = this._lines.map(
      (lineHtml, i) =>
        html`<div class="ln ${i + 1 === cur ? 'cur' : ''}" part="line">${unsafeHTML(lineHtml === '' ? '​' : lineHtml)}</div>`,
    );
    return html`
      <div class="wrap">
        ${this.lineNumbers
          ? html`<div class="gutter" part="gutter">
              ${this._lines.map((_, i) => {
                const n = i + 1;
                return html`<div
                  class="gl ${bp.has(n) ? 'bp' : ''} ${n === cur ? 'cur' : ''}"
                  part="gutter-line"
                  role="button"
                  tabindex="0"
                  aria-label=${`Toggle breakpoint on line ${n}`}
                  @click=${() => this._gutterClick(n)}
                  @keydown=${(e: KeyboardEvent) => this._onGutterKeydown(e, n)}
                >
                  ${n}
                </div>`;
              })}
            </div>`
          : nothing}
        <div class="stack">
          <div class="hl sema-scroll" part="highlight" aria-hidden="true">${overlay}</div>
          <textarea
            class="sema-scroll"
            part="textarea"
            data-testid=${ifDefined(this.testid || undefined)}
            .value=${live(this.value)}
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
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-editor': SemaEditor;
  }
}
customElements.define('sema-editor', SemaEditor);
