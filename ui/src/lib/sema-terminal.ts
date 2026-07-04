import { html, css, unsafeCSS, nothing } from 'lit';
import { property } from 'lit/decorators.js';
import { unsafeHTML } from 'lit/directives/unsafe-html.js';
import { SemaElement } from '../internal/sema-element.js';
import { dedent } from '../internal/dedent.js';
import { escapeHtml } from '../internal/syntax-highlight.js';
import { CopyController } from '../internal/controllers/copy.js';
import syntaxStyles from '../styles/syntax.css?inline';
import chromeStyles from '../styles/chrome.css?inline';
import scrollbarStyles from '../styles/scrollbar.css?inline';

type TermLine =
  | { kind: 'command'; prompt: string; html: string; copyText: string }
  | { kind: 'comment'; html: string }
  | { kind: 'output'; text: string }
  | { kind: 'blank' };

/**
 * `<sema-terminal>` — render a shell-command transcript from slotted plain text.
 *
 * The slotted text is dedented, then parsed line-by-line:
 *  - a line beginning with the prompt marker + space (default `$ `) is a **command**
 *    — the prompt is rendered as a styled glyph, quoted args are colored as strings,
 *    and a trailing ` # …` becomes a dim comment;
 *  - a line whose first non-space char is `#` is a **comment** line;
 *  - any other non-blank line is **output** (dim, verbatim).
 *
 * In `prefix` mode every non-blank line is treated as a command and the prompt is
 * prepended for display — handy for pasting raw command lists (e.g. docs that show
 * `sema compile app.sema  # → app.semac` without a `$`).
 *
 * ```html
 * <sema-terminal>
 *   $ brew install helgesverre/tap/sema-lang
 *   $ sema -e '(+ 1 2)'   # eval an expression
 * </sema-terminal>
 * ```
 */
export class SemaTerminal extends SemaElement {
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
      }
      code {
        font: inherit;
        color: inherit;
      }
      .term-line {
        display: block;
        white-space: pre;
      }
      .term-prompt {
        color: var(--gold, #c8a855);
        user-select: none;
      }
      .term-output {
        color: var(--text-tertiary, #5a5448);
      }
      slot {
        display: none;
      }
    `,
  ];

  /** Prompt glyph rendered before commands, and the marker used to detect them. */
  @property({ reflect: true }) prompt = '$';
  /**
   * Treat every non-blank line as a command and prepend the prompt (raw command lists).
   * Exposed as the `prefix` attribute; the property is named `autoPrompt` because
   * `prefix` is a reserved DOM property (`Element.prefix`).
   */
  @property({ type: Boolean, reflect: true, attribute: 'prefix' }) autoPrompt = false;
  /** Show a copy button that writes the commands (prompt + comments stripped). */
  @property({ type: Boolean, reflect: true }) copy = false;

  /** Raw slotted text, captured on slotchange. */
  private _raw = '';
  private _copy = new CopyController(this, () => this._commands());

  private _onSlotChange = (e: Event) => {
    const slot = e.target as HTMLSlotElement;
    this._raw = slot
      .assignedNodes({ flatten: true })
      .map((n) => n.textContent ?? '')
      .join('');
    this.requestUpdate();
  };

  private _renderCommand(cmd: string): { html: string; copyText: string } {
    // Split a trailing comment: whitespace, then `#` followed by a space. The
    // hash-then-space rule avoids matching Sema booleans like #t/#f in args.
    const m = cmd.match(/^(.*?)(\s+)(#\s.*)$/);
    const cmdPart = m ? m[1] : cmd;
    const gap = m ? m[2] : '';
    const comment = m ? m[3] : '';
    // Escape, then color quoted args as strings (mirrors the website's `.s` spans).
    const cmdHtml = escapeHtml(cmdPart).replace(
      /("[^"]*"|'[^']*')/g,
      '<span class="tok-string">$1</span>',
    );
    const htmlOut = comment
      ? `${cmdHtml}${escapeHtml(gap)}<span class="tok-comment">${escapeHtml(comment)}</span>`
      : cmdHtml;
    return { html: htmlOut, copyText: cmdPart };
  }

  private _parse(): TermLine[] {
    const src = dedent(this._raw);
    if (!src) return [];
    const marker = this.prompt || '$';
    return src.split('\n').map((line): TermLine => {
      if (line.trim() === '') return { kind: 'blank' };
      if (/^\s*#/.test(line)) return { kind: 'comment', html: `<span class="tok-comment">${escapeHtml(line)}</span>` };

      let cmd: string | null = null;
      if (this.autoPrompt) cmd = line;
      else if (line.startsWith(marker + ' ')) cmd = line.slice(marker.length + 1);
      else if (line === marker) cmd = '';

      if (cmd === null) return { kind: 'output', text: line };
      return { kind: 'command', prompt: marker, ...this._renderCommand(cmd) };
    });
  }

  /** The command lines (prompt + trailing comments stripped) — what copy writes. */
  private _commands(): string {
    return this._parse()
      .filter((l): l is Extract<TermLine, { kind: 'command' }> => l.kind === 'command')
      .map((l) => l.copyText)
      .join('\n');
  }

  render() {
    const lines = this._parse();
    return html`
      <div class="wrap">
        ${this.copy
          ? html`<button
              class="copy ${this._copy.copied ? 'copied' : ''}"
              part="copy-button"
              type="button"
              aria-label="Copy commands"
              @click=${this._copy.copy}
            >
              ${this._copy.copied ? 'Copied' : 'Copy'}
            </button>`
          : nothing}
        <pre class="sema-scroll" part="pre"><code part="code">${lines.map((l) => {
          switch (l.kind) {
            case 'command':
              return html`<span class="term-line"><span class="term-prompt" part="prompt"
                  >${l.prompt}</span
                > ${unsafeHTML(l.html)}</span>`;
            case 'comment':
              return html`<span class="term-line">${unsafeHTML(l.html)}</span>`;
            case 'output':
              return html`<span class="term-line term-output" part="output">${l.text}</span>`;
            case 'blank':
              return html`<span class="term-line"> </span>`;
          }
        })}</code></pre>
        <slot @slotchange=${this._onSlotChange}></slot>
      </div>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-terminal': SemaTerminal;
  }
}
customElements.define('sema-terminal', SemaTerminal);
