import { html, css, nothing, LitElement } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'icon' | 'pill' | 'run' | 'debug' | 'action';
export type ButtonSize = 'sm' | 'md';

export class SemaButton extends SemaElement {
  static shadowRootOptions = {
    ...LitElement.shadowRootOptions,
    delegatesFocus: true,
  };
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: inline-block;
        vertical-align: middle;
      }
      :host([variant="icon"]) {
        display: inline-flex;
      }

      .button {
        font-family: var(--mono, 'JetBrains Mono', monospace);
        cursor: pointer;
        transition: color 0.15s, background 0.15s, border-color 0.15s, opacity 0.15s;
        line-height: 1;
        white-space: nowrap;
        text-decoration: none;
        border: none;
        background: transparent;
        color: inherit;
        -webkit-font-smoothing: antialiased;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        gap: 0.4em;
      }
      .button::-moz-focus-inner { border: 0; }
      .button:focus { outline: none; }
      .button:focus-visible {
        outline: var(--focus-ring-width, 1px) solid var(--focus-ring-color-subtle, rgba(200, 168, 85, 0.5));
        outline-offset: var(--focus-ring-offset, 1px);
        border-radius: 3px;
      }
      .button:disabled {
        opacity: 0.4;
        cursor: not-allowed;
        pointer-events: none;
      }

      /* ── primary ── */
      :host([variant="primary"]) .button {
        background: var(--gold, #c8a855);
        color: var(--bg, #0c0c0c);
        padding: 14px 35px;
        border-radius: 6px;
        font-size: var(--text-lg, 14px);
        font-weight: 500;
        letter-spacing: 0.04em;
      }
      :host([variant="primary"]) .button:hover:not(:disabled) { background: var(--gold-bright, #e3c878); opacity: 1; }
      :host([variant="primary"]) .button:active:not(:disabled) { opacity: 0.7; }
      :host([variant="primary"]) .button:focus-visible {
        outline: 2px solid var(--text-primary, #d8d0c0);
        outline-offset: 3px;
        border-radius: 6px;
      }

      /* ── secondary ── */
      :host([variant="secondary"]) .button {
        background: transparent;
        color: var(--text-primary, #d8d0c0);
        padding: 14px 35px;
        border-radius: 6px;
        font-size: var(--text-lg, 14px);
        letter-spacing: 0.04em;
        border: 1px solid var(--border, #1e1e1e);
      }
      :host([variant="secondary"]) .button:hover:not(:disabled) {
        border-color: var(--text-tertiary, #5a5448);
        color: var(--gold, #c8a855);
      }

      /* ── ghost ── */
      :host([variant="ghost"]) .button {
        background: transparent;
        color: var(--text-tertiary, #5a5448);
        padding: 14px 35px;
        border-radius: 6px;
        font-size: var(--text-lg, 14px);
        letter-spacing: 0.04em;
      }
      :host([variant="ghost"]) .button:hover:not(:disabled) { color: var(--text-primary, #d8d0c0); }

      /* ── icon ── */
      :host([variant="icon"]) {
        width: 32px;
        height: 32px;
      }
      :host([variant="icon"]) .button {
        width: 32px;
        height: 32px;
        border-radius: 4px;
        color: var(--text-tertiary, #5a5448);
        font-size: var(--text-md, 13px);
        padding: 0;
      }
      :host([variant="icon"]) .button:hover:not(:disabled) {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
      }

      /* ── pill ── */
      :host([variant="pill"]) .button {
        background: transparent;
        color: var(--gold, #c8a855);
        padding: 6px 16px;
        border: 1px solid var(--gold-dim, rgba(200, 168, 85, 0.5));
        border-radius: 20px;
        font-size: var(--text-sm, 12px);
        letter-spacing: 0.03em;
      }
      :host([variant="pill"]) .button:hover:not(:disabled) {
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
        border-color: var(--gold, #c8a855);
      }

      /* ── run ── */
      :host([variant="run"]) .button {
        background: var(--gold, #c8a855);
        color: var(--bg, #0c0c0c);
        padding: 5px 14px;
        border-radius: 3px;
        font-size: var(--text-xs, 11px);
        letter-spacing: 0.05em;
      }
      :host([variant="run"]) .button:hover:not(:disabled) { opacity: 0.85; }
      :host([variant="run"]) .button:active:not(:disabled) { opacity: 0.7; }
      :host([variant="run"]) .button:focus-visible {
        outline: 2px solid var(--text-primary, #d8d0c0);
        outline-offset: 3px;
        border-radius: 3px;
      }

      /* shortcut badge inside run */
      .shortcut {
        font-family: system-ui, -apple-system, sans-serif;
        font-size: var(--text-xxs, 10px);
        opacity: 0.7;
        margin-left: 8px;
        background: rgba(0, 0, 0, 0.2);
        font-weight: bold;
        line-height: 1;
        padding: 2px 6px;
        border-radius: 4px;
        pointer-events: none;
        white-space: nowrap;
      }

      /* ── debug ── */
      :host([variant="debug"]) .button {
        width: 28px;
        height: 24px;
        border-radius: 3px;
        border: 1px solid var(--border, #1e1e1e);
        color: var(--text-secondary, #a09888);
        font-family: system-ui, -apple-system, sans-serif;
        font-size: var(--text-md, 13px);
        background: transparent;
      }
      :host([variant="debug"]) .button:hover:not(:disabled) {
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
        color: var(--gold, #c8a855);
        border-color: var(--gold-dim, rgba(200, 168, 85, 0.5));
      }
      :host([variant="debug"]) .button:focus-visible {
        outline-offset: 0;
        border-radius: 3px;
      }
      :host([variant="debug"][danger]) .button:hover:not(:disabled) {
        color: var(--error, #c85555);
        border-color: var(--error, #c85555);
      }

      /* ── action ── */
      :host([variant="action"]) .button {
        width: 24px;
        height: 24px;
        border-radius: 3px;
        background: var(--bg-elevated, #141414);
        color: var(--text-tertiary, #5a5448);
        font-size: var(--text-xxs, 10px);
      }
      :host([variant="action"]) .button:hover:not(:disabled) {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
      }
      :host([variant="action"][danger]) .button:hover:not(:disabled) {
        color: var(--error, #c85555);
      }

      /* ── slot content layout ── */
      .button ::slotted(svg) {
        width: 16px;
        height: 16px;
        flex-shrink: 0;
      }
      :host([variant="action"]) .button ::slotted(svg) {
        width: 13px;
        height: 13px;
      }

      /* size=sm — compact toolbar metrics; placed last so it overrides the
         form-scale text variants (secondary/ghost/primary) on equal specificity. */
      :host([size="sm"]) .button {
        height: var(--control-height-sm, 22px);
        box-sizing: border-box;
        padding: 0 14px;
        font-size: var(--text-xs, 11px);
        border-radius: var(--radius-sm, 3px);
      }
      /* icon is a fixed square — sm shrinks the box to the shared control height. */
      :host([size="sm"][variant="icon"]) {
        width: var(--control-height-sm, 22px);
        height: var(--control-height-sm, 22px);
      }
      :host([size="sm"][variant="icon"]) .button {
        width: var(--control-height-sm, 22px);
        height: var(--control-height-sm, 22px);
        padding: 0;
      }
    `,
  ];

  @property({ reflect: true }) variant: ButtonVariant = 'primary';
  @property({ reflect: true }) size: ButtonSize = 'md';
  @property({ type: Boolean, reflect: true }) disabled = false;
  @property({ type: Boolean, reflect: true }) danger = false;
  @property({ attribute: 'shortcut' }) shortcut?: string;

  render() {
    const label = this.getAttribute('aria-label');
    return html`
      <button class="button" type="button" ?disabled=${this.disabled} part="button"
              aria-label=${label || nothing}>
        <slot></slot>
        ${this.shortcut ? html`<span class="shortcut">${this.shortcut}</span>` : ''}
      </button>
    `;
  }
}

declare global { interface HTMLElementTagNameMap { 'sema-button': SemaButton } }
customElements.define('sema-button', SemaButton);
