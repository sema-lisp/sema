import { html, css, unsafeCSS } from 'lit';
import { property, state } from 'lit/decorators.js';
import { unsafeHTML } from 'lit/directives/unsafe-html.js';
import { ifDefined } from 'lit/directives/if-defined.js';
import { marked } from 'marked';
import { SemaElement } from '../internal/sema-element.js';
import { highlightToHtml, canHighlight, escapeHtml } from '../internal/syntax-highlight.js';
import syntaxStyles from '../styles/syntax.css?inline';

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
      :host {
        display: block;
        color: var(--text-primary, #d8d0c0);
      }
      [part='content'] {
        font-family: var(--sans, system-ui, sans-serif);
        line-height: 1.6;
      }
      [part='content'] > :first-child {
        margin-top: 0;
      }
      [part='content'] > :last-child {
        margin-bottom: 0;
      }
      h1,
      h2,
      h3,
      h4 {
        font-family: var(--serif, 'Cormorant', Georgia, serif);
        line-height: 1.2;
        font-weight: 600;
      }
      code {
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: 0.9em;
        background: var(--bg-editor, #0a0a0a);
        padding: 0.1em 0.3em;
        border-radius: 3px;
      }
      pre {
        background: var(--bg-editor, #0a0a0a);
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-sm, 4px);
        padding: var(--space-md, 12px);
        overflow-x: auto;
      }
      pre code {
        background: none;
        padding: 0;
        font-size: var(--text-md, 13px);
      }
      a {
        color: var(--gold, #d4a537);
      }
      table {
        border-collapse: collapse;
      }
      th,
      td {
        border: 1px solid var(--border, #1e1e1e);
        padding: 0.3em 0.6em;
      }
      blockquote {
        margin: 0.6em 0;
        padding-left: 0.9em;
        border-left: 2px solid var(--border, #1e1e1e);
        color: var(--text-secondary, #a89f8c);
      }
    `,
  ];

  @property() value = '';
  @property() testid = '';
  @state() private _html = '';
  private _slotText = '';

  updated(changed: Map<string, unknown>) {
    if (changed.has('value')) void this._render();
  }

  private async _render() {
    const src = this.value || this._slotText;
    let out: string;
    try {
      out = await marked.parse(src, { gfm: true });
      out = await this._highlightFences(out);
    } catch {
      out = `<pre>${escapeHtml(src)}</pre>`;
    }
    this._html = sanitize(out);
  }

  /** Upgrade ```lang fenced blocks to Shiki-highlighted markup (best-effort). */
  private async _highlightFences(rendered: string): Promise<string> {
    const re = /<pre><code class="language-([\w-]+)">([\s\S]*?)<\/code><\/pre>/g;
    const parts: string[] = [];
    const jobs: Promise<void>[] = [];
    let last = 0;
    let m: RegExpExecArray | null;
    while ((m = re.exec(rendered))) {
      const [full, lang, body] = m;
      parts.push(rendered.slice(last, m.index));
      const idx = parts.push(full) - 1; // placeholder = original block
      last = m.index + full.length;
      if (canHighlight(lang)) {
        const decoded = decodeEntities(body);
        jobs.push(
          highlightToHtml(decoded, lang)
            .then((code) => {
              parts[idx] = `<pre><code>${code}</code></pre>`;
            })
            // A lazily-loaded grammar chunk may be absent (e.g. the notebook
            // vendors only the main bundle). Degrade this one fence to marked's
            // default output instead of failing the whole document.
            .catch(() => {}),
        );
      }
    }
    parts.push(rendered.slice(last));
    await Promise.all(jobs);
    return parts.join('');
  }

  private _onSlot = (e: Event) => {
    this._slotText = (e.target as HTMLSlotElement)
      .assignedNodes({ flatten: true })
      .map((n) => n.textContent ?? '')
      .join('');
    if (!this.value) void this._render();
  };

  render() {
    return html`
      <div part="content" data-testid=${ifDefined(this.testid || undefined)}>${unsafeHTML(this._html)}</div>
      <slot @slotchange=${this._onSlot} hidden></slot>
    `;
  }
}

function decodeEntities(s: string): string {
  return s
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, '&');
}

/** Small allowlist sanitizer: strips <script>/<style>/embeds, event-handler attrs, javascript: URLs. */
function sanitize(dirty: string): string {
  const tpl = document.createElement('template');
  tpl.innerHTML = dirty;
  tpl.content.querySelectorAll('script, style, iframe, object, embed').forEach((n) => n.remove());
  tpl.content.querySelectorAll('*').forEach((el) => {
    for (const attr of Array.from(el.attributes)) {
      const name = attr.name.toLowerCase();
      const val = attr.value.trim().toLowerCase();
      if (name.startsWith('on')) el.removeAttribute(attr.name);
      else if ((name === 'href' || name === 'src') && val.startsWith('javascript:')) el.removeAttribute(attr.name);
    }
    if (el.tagName === 'A') {
      el.setAttribute('rel', 'noopener noreferrer');
      el.setAttribute('target', '_blank');
    }
  });
  return tpl.innerHTML;
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-markdown': SemaMarkdown;
  }
}
customElements.define('sema-markdown', SemaMarkdown);
