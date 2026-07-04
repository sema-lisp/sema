import { html, css, nothing } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';
import { FocusTrapController } from '../internal/controllers/focus-trap.js';

export type DrawerPlacement = 'left' | 'right' | 'top' | 'bottom';

/**
 * Generic, dockable slide-over panel. Dock to any edge via `placement`.
 * Modal: traps focus, locks scroll, closes on backdrop click and Escape.
 *
 * Size the panel with the `--drawer-size` custom property
 * (width for left/right, height for top/bottom).
 *
 * @slot - drawer body content
 * @slot header - optional header content (rendered next to the close button)
 * @slot footer - optional footer content (e.g. actions)
 * @csspart backdrop - the scrim behind the panel
 * @csspart panel - the sliding panel
 * @csspart close - the built-in close button
 * @fires sema-drawer-open - when the drawer opens
 * @fires sema-drawer-close - when the drawer closes
 */
export class SemaDrawer extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: none;
        --drawer-size: 320px;
      }
      :host([open]) {
        display: block;
      }

      .backdrop {
        position: fixed;
        inset: 0;
        z-index: 500;
        background: rgba(0, 0, 0, 0.6);
        animation: fadeIn 0.15s ease;
      }

      .panel {
        position: fixed;
        z-index: 501;
        background: var(--bg-elevated, #141414);
        border: 1px solid var(--border, #1e1e1e);
        display: flex;
        flex-direction: column;
        box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
        overflow: hidden;
      }

      /* ── docking ── */
      :host([placement='right']) .panel,
      :host(:not([placement])) .panel {
        top: 0;
        bottom: 0;
        right: 0;
        width: var(--drawer-size);
        max-width: 100vw;
        border-width: 0 0 0 1px;
        animation: slideInRight 0.18s ease;
      }
      :host([placement='left']) .panel {
        top: 0;
        bottom: 0;
        left: 0;
        width: var(--drawer-size);
        max-width: 100vw;
        border-width: 0 1px 0 0;
        animation: slideInLeft 0.18s ease;
      }
      :host([placement='top']) .panel {
        left: 0;
        right: 0;
        top: 0;
        height: var(--drawer-size);
        max-height: 100vh;
        border-width: 0 0 1px 0;
        animation: slideInTop 0.18s ease;
      }
      :host([placement='bottom']) .panel {
        left: 0;
        right: 0;
        bottom: 0;
        height: var(--drawer-size);
        max-height: 100vh;
        border-width: 1px 0 0 0;
        animation: slideInBottom 0.18s ease;
      }

      .header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: var(--space-md, 16px);
        padding: var(--space-md, 16px) var(--space-lg, 24px);
        border-bottom: 1px solid var(--border, #1e1e1e);
      }
      .title {
        font-family: var(--serif, 'Cormorant', Georgia, serif);
        font-size: var(--text-3xl, 22px);
        font-weight: 400;
        color: var(--text-primary, #d8d0c0);
        margin: 0;
      }
      .close {
        flex-shrink: 0;
        width: 28px;
        height: 28px;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        background: transparent;
        border: none;
        border-radius: var(--radius-md, 4px);
        color: var(--text-tertiary, #5a5448);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xl, 16px);
        line-height: 1;
        cursor: pointer;
        transition: color 0.15s, background 0.15s;
      }
      .close:hover {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
      }
      .close:focus { outline: none; }
      .close:focus-visible {
        outline: var(--focus-ring-width, 1px) solid var(--focus-ring-color-subtle, rgba(200, 168, 85, 0.5));
        outline-offset: var(--focus-ring-offset, 1px);
      }

      .body {
        flex: 1;
        min-height: 0;
        overflow: auto;
        padding: var(--space-lg, 24px);
        color: var(--text-secondary, #a09888);
      }

      .footer {
        display: flex;
        justify-content: flex-end;
        gap: var(--space-md, 16px);
        padding: var(--space-md, 16px) var(--space-lg, 24px);
        border-top: 1px solid var(--border, #1e1e1e);
      }
      .footer.empty {
        display: none;
      }

      @keyframes fadeIn { from { opacity: 0; } to { opacity: 1; } }
      @keyframes slideInRight { from { transform: translateX(100%); } to { transform: translateX(0); } }
      @keyframes slideInLeft { from { transform: translateX(-100%); } to { transform: translateX(0); } }
      @keyframes slideInTop { from { transform: translateY(-100%); } to { transform: translateY(0); } }
      @keyframes slideInBottom { from { transform: translateY(100%); } to { transform: translateY(0); } }
    `,
  ];

  @property({ type: Boolean, reflect: true }) open = false;
  @property({ reflect: true }) placement: DrawerPlacement = 'right';
  @property({ attribute: 'label' }) label?: string;

  private _labelId = `sema-drawer-${Math.random().toString(36).slice(2, 8)}`;
  private _hasFooter = false;

  private _focusTrap = new FocusTrapController(this, {
    getContainer: (host): HTMLElement =>
      (host.shadowRoot?.querySelector('.panel') as HTMLElement | null) ?? host,
    isActive: (host) => host.open,
    lockScroll: true,
  });

  // Escape must close the drawer from anywhere focus may be.
  private _onDocKeydown = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      this.close();
    }
  };

  connectedCallback() {
    super.connectedCallback();
    if (this.open) document.addEventListener('keydown', this._onDocKeydown);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener('keydown', this._onDocKeydown);
  }

  private _onFooterSlotChange(e: Event) {
    const slot = e.target as HTMLSlotElement;
    const hasFooter = slot.assignedNodes({ flatten: true }).length > 0;
    if (hasFooter !== this._hasFooter) {
      this._hasFooter = hasFooter;
      this.requestUpdate();
    }
  }

  render() {
    if (!this.open) return html``;
    return html`
      <div class="backdrop" part="backdrop" role="presentation" @click=${this.close}></div>
      <div class="panel" part="panel" role="dialog" aria-modal="true"
           aria-labelledby=${this.label ? this._labelId : nothing}
           aria-label=${this.label ? nothing : this.getAttribute('aria-label') || 'Drawer'}>
        <div class="header">
          ${this.label
            ? html`<h2 class="title" id=${this._labelId}>${this.label}</h2>`
            : html`<slot name="header"></slot>`}
          <button class="close" part="close" type="button" aria-label="Close" @click=${this.close}>✕</button>
        </div>
        <div class="body"><slot></slot></div>
        <div class="footer ${this._hasFooter ? '' : 'empty'}">
          <slot name="footer" @slotchange=${this._onFooterSlotChange}></slot>
        </div>
      </div>
    `;
  }

  updated(changed: Map<string, unknown>) {
    if (changed.has('open')) {
      if (this.open) {
        document.addEventListener('keydown', this._onDocKeydown);
        this.dispatchEvent(new CustomEvent('sema-drawer-open', { bubbles: true, composed: true }));
      } else {
        document.removeEventListener('keydown', this._onDocKeydown);
        this.dispatchEvent(new CustomEvent('sema-drawer-close', { bubbles: true, composed: true }));
      }
    }
  }

  close = () => {
    this.open = false;
  };

  show() {
    this.open = true;
  }
}

declare global { interface HTMLElementTagNameMap { 'sema-drawer': SemaDrawer } }
customElements.define('sema-drawer', SemaDrawer);
