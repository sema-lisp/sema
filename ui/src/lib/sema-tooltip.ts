import { html, css } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';

export type TooltipPlacement = 'top' | 'bottom' | 'left' | 'right';

export class SemaTooltip extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        position: relative;
        display: inline-block;
      }

      .tooltip {
        position: absolute;
        z-index: 200;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xxs, 10px);
        line-height: 1.4;
        padding: 6px 10px;
        background: var(--tooltip-bg, #1a1a1a);
        color: var(--text-primary, #d8d0c0);
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-md, 4px);
        /* Short tips stay one line; longer ones WRAP within max-width rather than
           truncating with an ellipsis (a tooltip must show its full text). */
        white-space: normal;
        overflow-wrap: break-word;
        width: max-content;
        pointer-events: none;
        opacity: 0;
        transition: opacity 0.15s;
        max-width: 20em;
      }

      :host(:hover) .tooltip,
      :host(:focus-within) .tooltip {
        opacity: 1;
      }

      /* ── Fallback positioning (all browsers) ── */
      :host([placement="top"]) .tooltip {
        bottom: calc(100% + 6px);
        left: 50%;
        transform: translateX(-50%);
        margin-bottom: 0;
      }
      :host([placement="bottom"]) .tooltip {
        top: calc(100% + 6px);
        left: 50%;
        transform: translateX(-50%);
      }
      :host([placement="left"]) .tooltip {
        right: calc(100% + 6px);
        top: 50%;
        transform: translateY(-50%);
      }
      :host([placement="right"]) .tooltip {
        left: calc(100% + 6px);
        top: 50%;
        transform: translateY(-50%);
      }

      /* NOTE: CSS anchor positioning (position-area) was intentionally removed.
         An @supports (position-area) query returns true in browsers that only parse
         the property without implementing the layout, which silently overrode the
         fallback and left every tooltip stuck at its trigger's origin. The absolute
         fallback above positions correctly everywhere; z-index keeps it visible. */

      .tooltip-arrow {
        position: absolute;
        width: 6px;
        height: 6px;
        background: var(--tooltip-bg, #1a1a1a);
        border: 1px solid var(--border, #1e1e1e);
        border-top-color: transparent;
        border-left-color: transparent;
        rotate: 45deg;
      }

      :host([placement="top"]) .tooltip-arrow {
        bottom: -4px;
        left: calc(50% - 3px);
      }
      :host([placement="bottom"]) .tooltip-arrow {
        top: -4px;
        left: calc(50% - 3px);
      }
      :host([placement="left"]) .tooltip-arrow {
        right: -4px;
        top: calc(50% - 3px);
      }
      :host([placement="right"]) .tooltip-arrow {
        left: -4px;
        top: calc(50% - 3px);
      }
    `,
  ];

  @property({ reflect: true }) placement: TooltipPlacement = 'top';
  @property({ attribute: 'content' }) content?: string;

  render() {
    if (!this.content) {
      return html`<slot @slotchange=${this._onSlotChange}></slot>`;
    }
    return html`
      <div class="tooltip" role="tooltip" aria-label=${this.content}>${this.content}<div class="tooltip-arrow"></div></div>
      <slot @slotchange=${this._onSlotChange}></slot>
    `;
  }

  connectedCallback() {
    super.connectedCallback();
    this.addEventListener('keydown', this._onKeydown);
    // disconnectedCallback strips the trigger's aria-description and a reconnect
    // schedules no Lit update (and slotchange does not re-fire for an unchanged
    // assignment), so re-apply it here.
    if (this.hasUpdated) {
      this.updateComplete.then(() => {
        if (this.isConnected) this._applyDescription();
      });
    }
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    this.removeEventListener('keydown', this._onKeydown);
    this._describedTrigger?.removeAttribute('aria-description');
    this._describedTrigger = null;
  }

  firstUpdated() {
    this._applyDescription();
  }

  updated(changed: Map<string, unknown>) {
    if (changed.has('content')) this._applyDescription();
  }

  private _onSlotChange = () => this._applyDescription();

  private _describedTrigger: HTMLElement | null = null;

  private _slottedTrigger(): HTMLElement | null {
    const slot = this.shadowRoot?.querySelector('slot');
    const first = slot?.assignedElements({ flatten: true })[0];
    return first instanceof HTMLElement ? first : null;
  }

  // IDREF ARIA (aria-describedby) can't reach across the shadow boundary to the
  // slotted light-DOM trigger, so set the string-valued aria-description on it.
  private _applyDescription() {
    const trigger = this._slottedTrigger();
    if (this._describedTrigger && this._describedTrigger !== trigger) {
      this._describedTrigger.removeAttribute('aria-description');
    }
    this._describedTrigger = trigger;
    if (!trigger) return;
    if (this.content) trigger.setAttribute('aria-description', this.content);
    else trigger.removeAttribute('aria-description');
  }

  private _onKeydown = (e: KeyboardEvent) => {
    if (e.key !== 'Escape') return;
    // Blur the actually-focused element (possibly deep in a shadow root) so
    // :focus-within stops matching and the tooltip hides.
    let active: Element | null = document.activeElement;
    while (active?.shadowRoot?.activeElement) active = active.shadowRoot.activeElement;
    if (active instanceof HTMLElement) active.blur();
  };
}

declare global { interface HTMLElementTagNameMap { 'sema-tooltip': SemaTooltip } }
customElements.define('sema-tooltip', SemaTooltip);
