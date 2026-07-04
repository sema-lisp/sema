import { html, css } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';
import { bridgeLengthAttr, gapAttrStyles } from '../internal/layout.js';
import type { GapToken } from '../internal/layout.js';

/**
 * `<sema-sidebar>` — static two-pane sidebar layout: fixed-basis aside +
 * growing content, stacking intrinsically on narrow containers. It never
 * resizes interactively and emits no events. For drag-resizable panes, use
 * `<sema-splitter>` between app-managed panes.
 *
 * Exactly two panes: the `aside` slot and the default slot (the content
 * pane); more is undefined behavior. Slot assignment, not source position,
 * determines which pane is which — author the light DOM in reading order.
 * The host is the flex container, so both panes are directly stylable from
 * page CSS; the stacking "breakpoint" is derived from available space
 * (`content-min`, effective default 50%), no media or container queries.
 * Panes stretch to equal height by default; set `align-items: flex-start` on
 * the host from page CSS for top alignment. Semantics-neutral: no ARIA
 * roles — consumers add landmarks (`<nav slot="aside">`).
 *
 * ```html
 * <sema-sidebar side-width="256px" gap="xl">
 *   <nav slot="aside" aria-label="Docs">…</nav>
 *   <article>…</article>
 * </sema-sidebar>
 * ```
 */
export class SemaSidebar extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: flex;
        flex-wrap: wrap;
        --_side: var(--sema-sidebar-side, 288px);
        --_content-min: var(--sema-sidebar-content-min, 50%);
        --_gap: var(--sema-sidebar-gap, var(--space-md, 16px));
        gap: var(--_gap);
      }
      ::slotted([slot='aside']) {
        flex-basis: var(--_side);
        flex-grow: 1;
      }
      /* Grow asymmetry + min-inline-size: when the content pane can't hold
         its minimum share of the row, the panes wrap to a stack. Single
         compound selector — valid in ::slotted(). */
      ::slotted(:not([slot])) {
        flex-basis: 0;
        flex-grow: 999;
        min-inline-size: var(--_content-min);
      }
    `,
    gapAttrStyles('gap', '--_gap'),
  ];

  /** Aside flex-basis (any CSS length). Unset → 288px. */
  @property({ reflect: true, attribute: 'side-width' }) sideWidth?: string;
  /** Content pane min-inline-size (percentage) — the stacking threshold. Unset → 50%. */
  @property({ reflect: true, attribute: 'content-min' }) contentMin?: string;
  /** Gap between panes (also the stacked gap) from the gap scale; `none` for border-separated panes. Unset → `md`. */
  @property({ reflect: true }) gap?: GapToken;

  willUpdate(changed: Map<string, unknown>) {
    if (changed.has('sideWidth')) bridgeLengthAttr(this, this.sideWidth, '--_side');
    if (changed.has('contentMin')) bridgeLengthAttr(this, this.contentMin, '--_content-min');
  }

  render() {
    return html`<slot name="aside"></slot><slot></slot>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-sidebar': SemaSidebar;
  }
}
customElements.define('sema-sidebar', SemaSidebar);
