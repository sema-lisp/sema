import { html, css } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';

export type BadgeVariant = 'neutral' | 'gold' | 'success' | 'error';

/**
 * Small status/label chip. Color comes from `variant`; shape from `pill`.
 * The `pill` shape + a wrapping container covers the old Provider Pill List.
 *
 * @slot - the badge label
 * @csspart badge - the inner chip element
 */
export class SemaBadge extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: inline-flex;
        vertical-align: middle;

        /* Per-variant palette, overridden by :host([variant=…]) below. */
        --_badge-bg: transparent;
        --_badge-border: var(--border, #1e1e1e);
        --_badge-fg: var(--text-secondary, #a09888);
      }

      :host([variant='gold']) {
        --_badge-bg: var(--gold-glow, rgba(200, 168, 85, 0.08));
        --_badge-border: var(--gold-dim, rgba(200, 168, 85, 0.5));
        --_badge-fg: var(--gold, #c8a855);
      }
      :host([variant='success']) {
        --_badge-bg: color-mix(in srgb, var(--success, #6a9955) 12%, transparent);
        --_badge-border: color-mix(in srgb, var(--success, #6a9955) 40%, transparent);
        --_badge-fg: var(--success, #6a9955);
      }
      :host([variant='error']) {
        --_badge-bg: var(--error-bg, rgba(200, 85, 85, 0.06));
        --_badge-border: color-mix(in srgb, var(--error, #c85555) 40%, transparent);
        --_badge-fg: var(--error, #c85555);
      }

      .badge {
        display: inline-flex;
        align-items: center;
        gap: 0.35em;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xxs, 10px);
        line-height: 1;
        letter-spacing: 0.04em;
        white-space: nowrap;
        padding: 4px 7px;
        border: 1px solid var(--_badge-border);
        border-radius: var(--radius-sm, 3px);
        background: var(--_badge-bg);
        color: var(--_badge-fg);
      }

      :host([pill]) .badge {
        padding: 4px 11px;
        border-radius: var(--radius-pill, 20px);
      }

      .dot {
        width: 0.4em;
        height: 0.4em;
        border-radius: var(--radius-full, 50%);
        background: currentColor;
        flex-shrink: 0;
      }
    `,
  ];

  @property({ reflect: true }) variant: BadgeVariant = 'neutral';
  /** Rounded "pill" shape — use inside a wrapping flex container for tag lists. */
  @property({ type: Boolean, reflect: true }) pill = false;
  /** Show a leading status dot in the current variant color. */
  @property({ type: Boolean, reflect: true }) dot = false;

  render() {
    return html`
      <span class="badge" part="badge">
        ${this.dot ? html`<span class="dot" aria-hidden="true"></span>` : ''}
        <slot></slot>
      </span>
    `;
  }
}

declare global { interface HTMLElementTagNameMap { 'sema-badge': SemaBadge } }
customElements.define('sema-badge', SemaBadge);
