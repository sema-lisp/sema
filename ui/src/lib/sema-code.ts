import { html, css, unsafeCSS, nothing } from 'lit';
import { property } from 'lit/decorators.js';
import { unsafeHTML } from 'lit/directives/unsafe-html.js';
import { Task } from '@lit/task';
import { SemaElement } from '../internal/sema-element.js';
import { dedent } from '../internal/dedent.js';
import {
  highlightToHtml,
  escapeHtml,
  canHighlight,
  registerLanguage,
} from '../internal/syntax-highlight.js';
import { CopyController } from '../internal/controllers/copy.js';
import syntaxStyles from '../styles/syntax.css?inline';
import chromeStyles from '../styles/chrome.css?inline';
import scrollbarStyles from '../styles/scrollbar.css?inline';

export type CodeFormatter = (
  code: string,
  opts: { lang: string },
) => string | Promise<string>;

/**
 * `<sema-code>` — drop a code snippet in as slotted text and it is:
 *   1. **dedented** (default) so it survives being authored indented inside markup,
 *   2. optionally **formatted** via a pluggable async hook (`SemaCode.formatter`),
 *   3. optionally **syntax-highlighted** — `sema` is bundled; `json`, `shell`, `rust`,
 *      etc. lazy-load on demand; register more via `SemaCode.registerLanguage(...)`.
 *
 * ```html
 * <sema-code>(define (square x) (* x x))</sema-code>
 * <sema-code lang="json">{ "ok": true }</sema-code>
 * ```
 *
 * Highlighting is async (Shiki, via @lit/task); the dedented plain text renders
 * immediately and is upgraded in place when tokenization resolves (no layout shift).
 */
export class SemaCode extends SemaElement {
  static styles = [
    SemaElement.base,
    unsafeCSS(syntaxStyles),
    unsafeCSS(chromeStyles),
    unsafeCSS(scrollbarStyles),
    css`
      :host {
        display: block;
      }
      .wrap {
        position: relative;
      }
      pre {
        margin: 0;
        padding: var(--space-md, 16px);
        background: var(--bg-editor, #0a0a0a);
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-lg, 6px);
        overflow-x: auto;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-md, 13px);
        line-height: 1.7;
        color: var(--text-primary, #d8d0c0);
        tab-size: 2;
      }
      code {
        font: inherit;
        color: inherit;
      }
      .cl {
        display: block;
        white-space: pre;
      }
      /* line numbers */
      :host([lines]) code {
        counter-reset: ln;
      }
      :host([lines]) .cl {
        padding-left: 3em;
        position: relative;
      }
      :host([lines]) .cl::before {
        counter-increment: ln;
        content: counter(ln);
        position: absolute;
        left: 0;
        width: 2.2em;
        text-align: right;
        color: var(--text-tertiary, #5a5448);
        user-select: none;
      }
      slot {
        display: none;
      }
    `,
  ];

  /** Language id (`sema`, `json`, `shell`, `rust`, …). Unsupported langs render plain. */
  @property({ reflect: true }) lang = 'sema';
  /** Disable indentation normalization of the slotted source. */
  @property({ type: Boolean, reflect: true, attribute: 'no-dedent' }) noDedent = false;
  /** Disable syntax highlighting (render dedented plain text). */
  @property({ type: Boolean, reflect: true, attribute: 'no-highlight' }) noHighlight = false;
  /** Run the registered formatter (`SemaCode.formatter`) before rendering. */
  @property({ type: Boolean, reflect: true }) format = false;
  /** Show a copy-to-clipboard button. */
  @property({ type: Boolean, reflect: true }) copy = false;
  /** Show line numbers. */
  @property({ type: Boolean, reflect: true }) lines = false;

  /** Optional async source formatter, wired by consumers (e.g. to sema-fmt WASM). */
  static formatter?: CodeFormatter;
  /** Register a custom language / grammar (alias of the module `registerLanguage`). */
  static registerLanguage = registerLanguage;
  private static _warned = false;
  private static _warnNoFormatter() {
    if (SemaCode._warned) return;
    SemaCode._warned = true;
    console.warn(
      '[sema-code] `format` set but no formatter registered. Set SemaCode.formatter to enable formatting.',
    );
  }

  /** Raw slotted text, captured on slotchange. */
  private _raw = '';
  /** Processed (dedented/formatted) source — what the copy button writes. */
  private _code = '';
  private _copy = new CopyController(this, () => this._code);

  /** dedent → optional format → optional highlight. @lit/task guards ordering. */
  private _highlightTask = new Task(this, {
    task: async ([raw, lang, noDedent, noHighlight, format]) => {
      const code = noDedent ? raw.replace(/^\n/, '').replace(/\s+$/, '') : dedent(raw);
      let out = code;
      if (format) {
        if (SemaCode.formatter) out = await SemaCode.formatter(code, { lang });
        else SemaCode._warnNoFormatter();
      }
      this._code = out;
      return !noHighlight && canHighlight(lang) ? highlightToHtml(out, lang) : escapeHtml(out);
    },
    args: () => [this._raw, this.lang, this.noDedent, this.noHighlight, this.format] as const,
  });

  private _onSlotChange = (e: Event) => {
    const slot = e.target as HTMLSlotElement;
    this._raw = slot
      .assignedNodes({ flatten: true })
      .map((n) => n.textContent ?? '')
      .join('');
    this.requestUpdate();
  };

  /** Dedented (unformatted) source for the pre-highlight / fallback render. */
  private _plainCode(): string {
    return this.noDedent ? this._raw.replace(/^\n/, '').replace(/\s+$/, '') : dedent(this._raw);
  }

  private _lines(htmlStr: string) {
    return htmlStr
      .split('\n')
      .map((line) => html`<span class="cl">${unsafeHTML(line === '' ? '​' : line)}</span>`);
  }

  render() {
    return html`
      <div class="wrap">
        ${this.copy
          ? html`<button
              class="copy ${this._copy.copied ? 'copied' : ''}"
              part="copy-button"
              type="button"
              aria-label="Copy code"
              @click=${this._copy.copy}
            >
              ${this._copy.copied ? 'Copied' : 'Copy'}
            </button>`
          : nothing}
        <pre class="sema-scroll" part="pre"><code part="code" aria-label=${`${this.lang} code`}>${this._highlightTask.render(
          {
            initial: () => this._lines(escapeHtml(this._plainCode())),
            pending: () => this._lines(this._highlightTask.value ?? escapeHtml(this._plainCode())),
            complete: (htmlStr) => this._lines(htmlStr),
            error: () => this._lines(escapeHtml(this._plainCode())),
          },
        )}</code></pre>
        <slot @slotchange=${this._onSlotChange}></slot>
      </div>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-code': SemaCode;
  }
}
customElements.define('sema-code', SemaCode);
