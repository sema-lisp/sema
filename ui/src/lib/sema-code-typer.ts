import { html, css, unsafeCSS, nothing } from 'lit';
import { property, state } from 'lit/decorators.js';
import { Task } from '@lit/task';
import { SemaElement } from '../internal/sema-element.js';
import { dedent } from '../internal/dedent.js';
import { tokenize, canHighlight, type CodeToken } from '../internal/syntax-highlight.js';
import syntaxStyles from '../styles/syntax.css?inline';

/** The Sema wordmark logo (gold brackets `#c8a855`, white `sema`). Rendered when `logo` is set. */
const SEMA_LOGO = html`<svg
  class="logo-svg"
  viewBox="0 0 366 132"
  role="img"
  aria-label="Sema"
  xmlns="http://www.w3.org/2000/svg"
>
  <path
    d="M48.5 104.3L48.5 114Q34 110.7 26.05 100.5Q18.1 90.3 18.1 75L18.1 57Q18.1 41.7 26.05 31.5Q34 21.3 48.5 18L48.5 27.6Q42.2 29.1 37.6 33.15Q33 37.2 30.5 43.3Q28 49.4 28 57L28 75Q28 82.6 30.5 88.65Q33 94.7 37.6 98.75Q42.2 102.8 48.5 104.3"
    fill="#c8a855"
  />
  <path
    d="M93.2 102.8L88.8 102.8Q79.4 102.8 74.2 98.6Q69 94.4 69 86.8L78.8 86.8Q78.8 90.4 81.45 92.45Q84.1 94.5 88.8 94.5L93.2 94.5Q98.1 94.5 100.75 92.4Q103.4 90.3 103.4 86.5Q103.4 79.8 96.8 79L82 76.9Q76.1 76 72.9 72.05Q69.7 68.1 69.7 61.8Q69.7 54.4 74.7 50.3Q79.7 46.2 88.7 46.2L93.1 46.2Q101.5 46.2 106.7 50.2Q111.9 54.2 112.2 60.8L102.2 60.8Q102 58 99.6 56.15Q97.2 54.3 93.1 54.3L88.7 54.3Q84.2 54.3 81.75 56.3Q79.3 58.3 79.3 61.7Q79.3 67.2 84.8 67.9L98.7 69.9Q113 71.8 113 86.5Q113 94.3 107.85 98.55Q102.7 102.8 93.2 102.8 M152 103Q142.1 103 136.05 97.1Q130 91.2 130 81L130 68Q130 57.8 136.05 51.9Q142.1 46 152 46Q158.6 46 163.55 48.65Q168.5 51.3 171.25 56.05Q174 60.8 174 67.1L174 77L139.7 77L139.7 81.8Q139.7 87.8 143 91.2Q146.3 94.6 152 94.6Q156.8 94.6 159.9 92.8Q163 91 163.6 87.8L173.5 87.8Q172.5 94.8 166.6 98.9Q160.7 103 152 103M139.7 67.1L139.7 69.7L164.3 69.7L164.3 67.1Q164.3 60.8 161.1 57.4Q157.9 54 152 54Q146.1 54 142.9 57.4Q139.7 60.8 139.7 67.1 M197.7 102L188.7 102L188.7 47L197.1 47L197.1 54.5L197.4 54.5Q197.8 50.7 200.25 48.35Q202.7 46 206.5 46Q210.2 46 212.7 48.2Q215.2 50.4 216.3 54.1Q216.9 50.3 219.4 48.15Q221.9 46 225.7 46Q230.9 46 234.1 49.95Q237.3 53.9 237.3 60.2L237.3 102L228.3 102L228.3 60.3Q228.3 57.2 226.75 55.35Q225.2 53.5 222.6 53.5Q220 53.5 218.5 55.3Q217 57.1 217 60.3L217 102L209 102L209 60.3Q209 57.2 207.5 55.35Q206 53.5 203.4 53.5Q200.8 53.5 199.25 55.3Q197.7 57.1 197.7 60.3 M268.9 103Q260.4 103 255.45 98.25Q250.5 93.5 250.5 85.8Q250.5 78.1 255.65 73.4Q260.8 68.7 269.2 68.7L285.3 68.7L285.3 64.5Q285.3 54.4 274.1 54.4Q269.1 54.4 266.05 56.25Q263 58.1 262.8 61.4L253 61.4Q253.5 54.7 259.1 50.35Q264.7 46 274.1 46Q284.2 46 289.7 50.8Q295.2 55.6 295.2 64.3L295.2 102L285.5 102L285.5 91.9L285.3 91.9Q284.6 97 280.25 100Q275.9 103 268.9 103M271.5 94.7Q277.8 94.7 281.55 91.65Q285.3 88.6 285.3 83.3L285.3 76L270.1 76Q265.8 76 263.15 78.55Q260.5 81.1 260.5 85.3Q260.5 89.6 263.4 92.15Q266.3 94.7 271.5 94.7"
    fill="#ffffff"
  />
  <path
    d="M316.5 114L316.5 104.3Q322.8 102.8 327.4 98.75Q332 94.7 334.55 88.65Q337.1 82.6 337.1 75L337.1 57Q337.1 49.4 334.55 43.3Q332 37.2 327.4 33.15Q322.8 29.1 316.5 27.6L316.5 18Q331 21.3 339 31.5Q347 41.7 347 57L347 75Q347 90.3 339 100.5Q331 110.7 316.5 114"
    fill="#c8a855"
  />
</svg>`;

/**
 * Split the first `revealed` characters of a token list into lines (each a list of
 * `{ text, cls }` segments). Newline tokens start a new line. Pure + exported for tests.
 */
export function revealLines(tokens: CodeToken[], revealed: number): CodeToken[][] {
  const lines: CodeToken[][] = [[]];
  let count = 0;
  for (const tok of tokens) {
    if (count >= revealed) break;
    const text = tok.text.length > revealed - count ? tok.text.slice(0, revealed - count) : tok.text;
    count += text.length;
    const segs = text.split('\n');
    segs.forEach((seg, i) => {
      if (i > 0) lines.push([]);
      if (seg) lines[lines.length - 1].push({ text: seg, cls: tok.cls });
    });
  }
  return lines;
}

/**
 * `<sema-code-typer>` — types provided code out character-by-character with live Sema
 * syntax highlighting, a moving caret, and optional editor chrome (frame, gutter, status).
 *
 * Highlights once (Shiki tokens) then reveals by character — cheap and accurate. Colors
 * come from the shared `--syntax-*` / design tokens (no hardcoded hex). Honors
 * `prefers-reduced-motion` (renders the full code, no animation).
 *
 * ```html
 * <sema-code-typer frame status filename="maze.sema" loop>
 *   (define (square x) (* x x))
 * </sema-code-typer>
 * ```
 *
 * Imperative: `play()`, `pause()`, `restart()`, `seek(n)`. Fires `sema-typer-done`.
 */
export class SemaCodeTyper extends SemaElement {
  static styles = [
    SemaElement.base,
    unsafeCSS(syntaxStyles),
    css`
      :host {
        display: block;
      }
      /* The legend straddles the top border (negative top); reserve space above the
         frame so it's never clipped — including in element screenshots / GIF export. */
      :host([frame]) {
        padding-top: 0.9em;
      }
      :host([frame][logo]) {
        padding-top: 1.35em;
      }
      .frame {
        position: relative;
        border: 1px solid var(--syntax-punctuation, #6a6258);
        border-radius: var(--radius-lg, 6px);
        background: var(--bg, #131110);
        padding: 16px 14px 9px;
      }
      .legend {
        position: absolute;
        top: -0.72em;
        left: 16px;
        padding: 0 9px;
        background: var(--bg, #131110);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-md, 13px);
        font-weight: 600;
        line-height: 1;
      }
      .lpar {
        color: var(--gold, #c8a855);
      }
      .lname {
        color: var(--text-primary, #e9e3d6);
      }
      .legend .logo-svg {
        display: block;
        height: 1.25em;
        width: auto;
      }
      .frame.has-logo .legend {
        top: -0.95em;
        padding: 0 10px;
      }
      /* let a custom status slot's children participate in the status flex row */
      .status slot {
        display: contents;
      }
      .viewport {
        overflow: hidden;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-md, 13px);
        line-height: 1.5;
        color: var(--text-primary, #e9e3d6);
        tab-size: 2;
      }
      .code {
        display: block;
      }
      .cl {
        display: block;
        white-space: pre;
        min-height: 1.5em;
      }
      :host([line-numbers]) .code {
        counter-reset: ln;
      }
      :host([line-numbers]) .cl {
        padding-left: 3em;
        position: relative;
      }
      :host([line-numbers]) .cl::before {
        counter-increment: ln;
        content: counter(ln);
        position: absolute;
        left: 0;
        width: 2.2em;
        text-align: right;
        color: var(--text-tertiary, #5a5448);
        user-select: none;
      }
      .cursor {
        display: inline-block;
        width: 0.6ch;
        height: 1.05em;
        vertical-align: text-bottom;
        margin-left: 1px;
        background: var(--gold, #c8a855);
        animation: blink 1s steps(1) infinite;
      }
      @keyframes blink {
        50% {
          opacity: 0;
        }
      }
      .status {
        display: flex;
        align-items: center;
        margin-top: 8px;
        padding-top: 6px;
        border-top: 1px solid var(--border, #2b2620);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xs, 11px);
        color: var(--text-secondary, #968c79);
      }
      .status .mode {
        color: var(--gold, #c8a855);
        font-weight: 600;
        margin-right: 10px;
      }
      .status .spacer {
        flex: 1;
      }
      .status .seg,
      .status .pos {
        margin-left: 12px;
      }
      slot:not([name]) {
        display: none;
      }
    `,
  ];

  /** Language id for highlighting (`sema`, `json`, `rust`, …). */
  @property({ reflect: true }) lang = 'sema';
  /** Typing speed in characters per second. */
  @property({ type: Number }) cps = 45;
  /** Delay (ms) before typing begins. */
  @property({ type: Number, attribute: 'start-delay' }) startDelay = 400;
  /** Restart after finishing (single snippet) / loop the snippet list. */
  @property({ type: Boolean, reflect: true }) loop = false;
  /** Delay (ms) between completion and restart/next snippet. */
  @property({ type: Number, attribute: 'loop-delay' }) loopDelay = 1500;
  /** Start typing automatically once the source is available. */
  @property({ type: Boolean, reflect: true }) autoplay = true;
  /** Render the titled editor frame/box. */
  @property({ type: Boolean, reflect: true }) frame = false;
  /** Use the Sema wordmark logo (SVG) as the frame legend instead of the `( sema )` text. */
  @property({ type: Boolean, reflect: true }) logo = false;
  /** Show the line-number gutter. */
  @property({ type: Boolean, reflect: true, attribute: 'line-numbers' }) lineNumbers = false;
  /** Show the status line (mode · filename · Ln:Col). Requires `frame`. */
  @property({ type: Boolean, reflect: true }) status = false;
  /** Filename shown in the status line. */
  @property({ reflect: true }) filename = '';
  /** Visible height in lines (0 = grow to content, no scroll). */
  @property({ type: Number }) rows = 0;
  /** Disable indentation normalization of the slotted source. */
  @property({ type: Boolean, reflect: true, attribute: 'no-dedent' }) noDedent = false;
  /** Optional list of snippets to cycle through (JS property). Overrides slotted source. */
  @property({ attribute: false }) snippets?: string[];

  @state() private _revealed = 0;
  @state() private _index = 0;

  private _raw = '';
  private _code = '';
  private _total = 0;
  private _raf = 0;
  private _timer = 0;
  private _startAt = 0;
  private _playing = false;
  private _reduce = false;

  connectedCallback(): void {
    super.connectedCallback();
    this._reduce =
      typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches;
  }

  disconnectedCallback(): void {
    super.disconnectedCallback();
    this._stop();
    clearTimeout(this._timer);
  }

  private _sources(): string[] {
    return this.snippets?.length ? this.snippets : [this._raw];
  }

  /** dedent → tokenize the active snippet, then (re)start typing it. */
  private _tokensTask = new Task(this, {
    task: async ([raw, lang, noDedent]) => {
      const code = noDedent ? raw.replace(/^\n/, '').replace(/\s+$/, '') : dedent(raw);
      this._code = code;
      this._total = code.length;
      const toks: CodeToken[] = canHighlight(lang)
        ? await tokenize(code, lang)
        : code
          ? [{ text: code, cls: '' }]
          : [];
      this._begin();
      return toks;
    },
    args: () => [this._sources()[this._index % this._sources().length] ?? '', this.lang, this.noDedent] as const,
  });

  private _onSlotChange = (e: Event) => {
    const slot = e.target as HTMLSlotElement;
    this._raw = slot
      .assignedNodes({ flatten: true })
      .map((n) => n.textContent ?? '')
      .join('');
    this._index = 0;
    this.requestUpdate();
  };

  // --- typing engine ---

  private _begin() {
    this._stop();
    if (this._total === 0) {
      this._revealed = 0;
      return;
    }
    if (this._reduce) {
      this._revealed = this._total;
      return;
    }
    this._revealed = 0;
    if (this.autoplay) {
      this._startAt = performance.now() + this.startDelay;
      this._playing = true;
      this._raf = requestAnimationFrame(this._tick);
    }
  }

  private _tick = (now: number) => {
    if (!this._playing) return;
    const elapsed = Math.max(0, now - this._startAt) / 1000;
    this._revealed = Math.min(this._total, Math.floor(elapsed * this.cps));
    if (this._revealed >= this._total) {
      this._playing = false;
      this.dispatchEvent(new CustomEvent('sema-typer-done', { bubbles: true, composed: true }));
      this._afterComplete();
      return;
    }
    this._raf = requestAnimationFrame(this._tick);
  };

  private _afterComplete() {
    const sources = this._sources();
    if (sources.length > 1) {
      const next = this._index + 1;
      if (next < sources.length || this.loop) {
        this._timer = window.setTimeout(() => {
          this._index = next % sources.length; // arg change → task re-runs → _begin()
        }, this.loopDelay);
      }
    } else if (this.loop) {
      this._timer = window.setTimeout(() => this._begin(), this.loopDelay);
    }
  }

  private _stop() {
    cancelAnimationFrame(this._raf);
    this._raf = 0;
    this._playing = false;
  }

  // --- imperative API ---

  /** Total character count of the active (dedented) snippet — available once tokenized. */
  get total(): number {
    return this._total;
  }

  /** Resume typing from the current caret. */
  play() {
    if (this._playing || this._revealed >= this._total) return;
    this._startAt = performance.now() - (this._revealed / this.cps) * 1000;
    this._playing = true;
    this._raf = requestAnimationFrame(this._tick);
  }

  /** Pause typing. */
  pause() {
    this._stop();
  }

  /** Reset to the start of the current snippet and play. */
  restart() {
    this._stop();
    this._revealed = 0;
    this._startAt = performance.now() + this.startDelay;
    this._playing = true;
    this._raf = requestAnimationFrame(this._tick);
  }

  /** Jump to character `n` (clamped) and pause there. */
  seek(n: number) {
    this._stop();
    this._revealed = Math.max(0, Math.min(this._total, Math.floor(n)));
  }

  private _pos(): { ln: number; col: number } {
    const vis = this._code.slice(0, this._revealed);
    const nl = vis.lastIndexOf('\n');
    const ln = (vis.match(/\n/g)?.length ?? 0) + 1;
    return { ln, col: this._revealed - (nl + 1) + 1 };
  }

  updated() {
    if (this.rows > 0) {
      const vp = this.renderRoot.querySelector<HTMLElement>('.viewport');
      if (vp) vp.scrollTop = vp.scrollHeight;
    }
  }

  private _renderEditor() {
    const tokens = this._tokensTask.value ?? [];
    const lines = revealLines(tokens, this._reduce ? this._total : this._revealed);
    const style = this.rows > 0 ? `height:${this.rows * 1.5}em` : '';
    return html`<div class="viewport" style=${style}><code class="code" part="code" aria-label=${`${this.lang} code`}
      >${lines.map(
        (segs, i) =>
          html`<span class="cl"
            >${segs.map((s) => (s.cls ? html`<span class="${s.cls}">${s.text}</span>` : s.text))}${i ===
            lines.length - 1
              ? html`<span class="cursor" aria-hidden="true"></span>`
              : nothing}</span
          >`,
      )}</code></div>`;
  }

  render() {
    const slot = html`<slot @slotchange=${this._onSlotChange}></slot>`;
    if (!this.frame) return html`${this._renderEditor()}${slot}`;
    const { ln, col } = this._pos();
    return html`
      <div class="frame ${this.logo ? 'has-logo' : ''}">
        <span class="legend" part="legend"
          ><slot name="legend"
            >${this.logo
              ? SEMA_LOGO
              : html`<span class="lpar">(</span> <span class="lname">sema</span> <span class="lpar">)</span>`}</slot
          ></span>
        ${this._renderEditor()}
        ${this.status
          ? html`<div class="status" part="status">
              <slot name="status"
                ><span class="mode">EDIT</span><span class="fname">${this.filename}</span
                ><span class="spacer"></span><span class="pos">${ln}:${col}</span><span class="seg">LF</span></slot
              >
            </div>`
          : nothing}
        ${slot}
      </div>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-code-typer': SemaCodeTyper;
  }
}
customElements.define('sema-code-typer', SemaCodeTyper);
