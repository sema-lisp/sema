import { html, css, nothing } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';

/**
 * Renders a keyboard key or shortcut combo as key-caps.
 *
 * Use slotted text for a single key (`<sema-kbd>Esc</sema-kbd>`), or the `keys`
 * attribute for a combo (`<sema-kbd keys="Cmd+Shift+P">`), which splits on `+`
 * and renders one cap per key joined by separators.
 *
 * @slot - a single key label (ignored when `keys` is set)
 * @csspart key - each key-cap element
 */
export class SemaKbd extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: inline-flex;
        align-items: center;
        gap: 0.25em;
        vertical-align: middle;
      }
      kbd {
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xs, 11px);
        line-height: 1;
        color: var(--text-secondary, #a09888);
        background: var(--bg-elevated, #141414);
        border: 1px solid var(--border, #1e1e1e);
        border-bottom-width: 2px;
        border-radius: var(--radius-sm, 3px);
        padding: 0.2em 0.4em;
        min-width: 1.4em;
        text-align: center;
        white-space: nowrap;
      }
      .sep {
        color: var(--text-tertiary, #5a5448);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xxs, 10px);
      }
    `,
  ];

  /** A `+`-separated shortcut combo, e.g. `Cmd+Shift+P`. Overrides the slot. */
  @property() keys?: string;

  private get _parts(): string[] {
    return (this.keys ?? '')
      .split('+')
      .map((k) => k.trim())
      .filter(Boolean);
  }

  render() {
    const parts = this._parts;
    if (parts.length === 0) {
      return html`<kbd part="key"><slot></slot></kbd>`;
    }
    return parts.map(
      (key, i) => html`
        ${i > 0 ? html`<span class="sep" aria-hidden="true">+</span>` : nothing}
        <kbd part="key">${key}</kbd>
      `,
    );
  }
}

declare global { interface HTMLElementTagNameMap { 'sema-kbd': SemaKbd } }
customElements.define('sema-kbd', SemaKbd);
