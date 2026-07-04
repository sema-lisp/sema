import { html, css, nothing } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';

type PageItem = number | 'start-ellipsis' | 'end-ellipsis';

function range(start: number, end: number): number[] {
  const out: number[] = [];
  for (let i = start; i <= end; i++) out.push(i);
  return out;
}

/**
 * Build the displayed page sequence with collapsed ellipses.
 * Mirrors the well-tested MUI `usePagination` truncation so the visible width
 * stays stable as the current page moves.
 */
export function paginationItems(
  page: number,
  count: number,
  siblings = 1,
  boundaries = 1,
): PageItem[] {
  if (count <= 0) return [];

  const startPages = range(1, Math.min(boundaries, count));
  const endPages = range(Math.max(count - boundaries + 1, boundaries + 1), count);

  const siblingsStart = Math.max(
    Math.min(page - siblings, count - boundaries - siblings * 2 - 1),
    boundaries + 2,
  );
  const siblingsEnd = Math.min(
    Math.max(page + siblings, boundaries + siblings * 2 + 2),
    endPages.length > 0 ? endPages[0] - 2 : count - 1,
  );

  return [
    ...startPages,
    ...(siblingsStart > boundaries + 2
      ? (['start-ellipsis'] as const)
      : boundaries + 1 < count - boundaries
        ? [boundaries + 1]
        : []),
    ...range(siblingsStart, siblingsEnd),
    ...(siblingsEnd < count - boundaries - 1
      ? (['end-ellipsis'] as const)
      : count - boundaries > boundaries
        ? [count - boundaries]
        : []),
    ...endPages,
  ];
}

/**
 * Page navigation with prev/next and collapsed ellipses.
 *
 * Selecting a page updates `page` and fires `sema-page-change`. Consumers may
 * treat it as controlled (re-set `page` from the event) or uncontrolled.
 *
 * @csspart nav - the nav landmark
 * @csspart item - any page / prev / next button
 * @csspart page - a numbered page button
 * @csspart current - the active page button
 * @csspart prev - the previous button
 * @csspart next - the next button
 * @fires sema-page-change - `{ page }` when a new page is selected
 */
export class SemaPagination extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: block;
      }
      nav {
        display: flex;
        align-items: center;
        gap: var(--space-xs, 4px);
      }
      button {
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xs, 11px);
        min-width: 30px;
        height: 30px;
        padding: 0 6px;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        background: transparent;
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-sm, 3px);
        color: var(--text-secondary, #a09888);
        cursor: pointer;
        transition: color 0.15s, background 0.15s, border-color 0.15s;
      }
      button:hover:not(:disabled):not([aria-current]) {
        color: var(--gold, #c8a855);
        border-color: var(--gold-dim, rgba(200, 168, 85, 0.5));
      }
      button:focus { outline: none; }
      button:focus-visible {
        outline: var(--focus-ring-width, 1px) solid var(--focus-ring-color-subtle, rgba(200, 168, 85, 0.5));
        outline-offset: var(--focus-ring-offset, 1px);
      }
      button:disabled {
        opacity: 0.35;
        cursor: not-allowed;
      }
      button[aria-current] {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
        border-color: var(--gold-dim, rgba(200, 168, 85, 0.5));
        cursor: default;
      }
      .ellipsis {
        min-width: 24px;
        text-align: center;
        color: var(--text-tertiary, #5a5448);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xs, 11px);
        user-select: none;
      }
    `,
  ];

  /** Current page (1-based). */
  @property({ type: Number, reflect: true }) page = 1;
  /** Total number of pages. */
  @property({ type: Number, reflect: true }) total = 1;
  /** Pages shown on each side of the current page. */
  @property({ type: Number }) siblings = 1;
  /** Pages always shown at the start and end. */
  @property({ type: Number }) boundaries = 1;

  private get _current(): number {
    if (this.total < 1) return 1;
    return Math.min(Math.max(this.page, 1), this.total);
  }

  private _go(target: number) {
    const next = Math.min(Math.max(target, 1), this.total);
    if (next === this._current) return;
    this.page = next;
    this.dispatchEvent(
      new CustomEvent('sema-page-change', {
        detail: { page: next },
        bubbles: true,
        composed: true,
      }),
    );
  }

  render() {
    if (this.total < 1) return html``;
    const current = this._current;
    const items = paginationItems(current, this.total, this.siblings, this.boundaries);

    return html`
      <nav part="nav" aria-label="Pagination">
        <button
          part="item prev"
          type="button"
          aria-label="Previous page"
          ?disabled=${current <= 1}
          @click=${() => this._go(current - 1)}
        >‹</button>

        ${items.map((item) =>
          typeof item === 'number'
            ? html`<button
                part=${item === current ? 'item page current' : 'item page'}
                type="button"
                aria-label=${`Page ${item}`}
                aria-current=${item === current ? 'page' : nothing}
                @click=${() => this._go(item)}
              >${item}</button>`
            : html`<span class="ellipsis" aria-hidden="true">…</span>`,
        )}

        <button
          part="item next"
          type="button"
          aria-label="Next page"
          ?disabled=${current >= this.total}
          @click=${() => this._go(current + 1)}
        >›</button>
      </nav>
    `;
  }
}

declare global { interface HTMLElementTagNameMap { 'sema-pagination': SemaPagination } }
customElements.define('sema-pagination', SemaPagination);
