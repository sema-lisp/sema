import { html, css } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';

export class SemaToggle extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: inline-block;
      }
      .toggle {
        display: flex;
        align-items: center;
        height: var(--control-height-sm, 22px);
        box-sizing: border-box;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xxs, 10px);
        letter-spacing: 0.04em;
        padding: 0 9px;
        border-radius: 3px;
        cursor: pointer;
        color: var(--text-tertiary, #5a5448);
        transition: color 0.15s, background 0.15s;
        user-select: none;
        white-space: nowrap;
      }
      .toggle:focus { outline: none; }
      .toggle:focus-visible {
        outline: var(--focus-ring-width, 1px) solid var(--focus-ring-color-subtle, rgba(200, 168, 85, 0.5));
        outline-offset: var(--focus-ring-offset, 1px);
      }
      .toggle:hover {
        color: var(--text-secondary, #a09888);
      }
      :host([selected]) .toggle {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
      }
    `,
  ];

  @property({ reflect: true }) value: string = '';
  @property({ type: Boolean, reflect: true }) selected = false;
  /** Group-assigned roving tab stop (decoupled from `selected` so the group can make
   *  a toggle focusable without selecting it). */
  @property({ type: Boolean, reflect: true }) tabbable = false;

  focus() {
    const el = this.shadowRoot?.querySelector('.toggle') as HTMLElement | null;
    el?.focus();
  }

  render() {
    return html`
      <div class="toggle" role="radio" aria-checked=${this.selected ? 'true' : 'false'} tabindex=${this.selected || this.tabbable ? '0' : '-1'}>
        <slot></slot>
      </div>
    `;
  }
}

declare global { interface HTMLElementTagNameMap { 'sema-toggle': SemaToggle } }
customElements.define('sema-toggle', SemaToggle);
