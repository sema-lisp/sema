import { html, css, nothing } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';

export type ToastVariant = 'info' | 'success' | 'error' | 'warning';

/**
 * `<sema-toast>` — a single toast notification (usually created by the `toast()` API
 * via `<sema-toaster>`, but usable standalone). The message is the default slot.
 * Emits `sema-dismiss` when the close button is pressed.
 */
export class SemaToast extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: block;
        pointer-events: auto;
      }
      .toast {
        display: flex;
        align-items: flex-start;
        gap: 10px;
        min-width: 240px;
        max-width: 384px;
        padding: 11px 13px;
        background: var(--bg-elevated, #141414);
        border: 1px solid var(--border, #1e1e1e);
        border-left: 3px solid var(--accent, var(--text-secondary, #a09888));
        border-radius: var(--radius-md, 4px);
        box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-sm, 12px);
        line-height: 1.5;
        color: var(--text-primary, #d8d0c0);
        animation: toast-in 0.18s ease;
      }
      :host([variant='success']) .toast {
        --accent: var(--success, #6a9955);
      }
      :host([variant='error']) .toast {
        --accent: var(--error, #c85555);
      }
      :host([variant='warning']) .toast {
        --accent: var(--gold, #c8a855);
      }
      :host([variant='info']) .toast {
        --accent: var(--text-secondary, #a09888);
      }
      .msg {
        flex: 1;
        min-width: 0;
      }
      .close {
        flex-shrink: 0;
        width: 19px;
        height: 19px;
        display: flex;
        align-items: center;
        justify-content: center;
        border: none;
        border-radius: var(--radius-sm, 3px);
        background: transparent;
        color: var(--text-tertiary, #5a5448);
        font-size: var(--text-lg, 14px);
        line-height: 1;
        cursor: pointer;
        transition: color 0.15s, background 0.15s;
      }
      .close:hover,
      .close:focus-visible {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
        outline: none;
      }
      @keyframes toast-in {
        from {
          opacity: 0;
          transform: translateY(-6px);
        }
        to {
          opacity: 1;
          transform: translateY(0);
        }
      }
    `,
  ];

  @property({ reflect: true }) variant: ToastVariant = 'info';
  @property({ type: Boolean, reflect: true }) dismissible = true;

  private _dismiss = () => {
    this.dispatchEvent(new CustomEvent('sema-dismiss', { bubbles: true, composed: true }));
  };

  render() {
    // error/warning interrupt assistive tech (alert); info/success announce politely (status)
    const role = this.variant === 'error' || this.variant === 'warning' ? 'alert' : 'status';
    return html`<div class="toast" role=${role} part="toast">
      <span class="msg" part="message"><slot></slot></span>
      ${this.dismissible
        ? html`<button
            class="close"
            part="close"
            type="button"
            aria-label="Dismiss"
            @click=${this._dismiss}
          >
            ✕
          </button>`
        : nothing}
    </div>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-toast': SemaToast;
  }
}
customElements.define('sema-toast', SemaToast);
