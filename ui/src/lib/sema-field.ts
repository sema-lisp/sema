import { html, css, nothing } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';

/**
 * `<sema-field>` — a label + control + hint/error wrapper.
 *
 * Wrap any control (`sema-input`, `sema-textarea`, `sema-select`, or a native one):
 *
 * ```html
 * <sema-field label="Email" hint="We never share it.">
 *   <sema-input type="email" name="email"></sema-input>
 * </sema-field>
 * ```
 *
 * `error` (when set) replaces the hint and turns the message red.
 */
export class SemaField extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: block;
      }
      .field {
        display: flex;
        flex-direction: column;
        gap: var(--space-xs, 4px);
      }
      .label {
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xs, 11px);
        letter-spacing: 0.04em;
        color: var(--text-secondary, #a09888);
      }
      .msg {
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xxs, 10px);
        color: var(--text-tertiary, #5a5448);
      }
      .msg.error {
        color: var(--error, #c85555);
      }
    `,
  ];

  @property() label = '';
  @property() hint = '';
  @property() error = '';

  private _control: Element | null = null;

  updated(changed: Map<string, unknown>) {
    if (changed.has('label') || changed.has('hint') || changed.has('error')) this._applyA11y();
  }

  private _onSlotChange = (e: Event) => {
    const assigned = (e.target as HTMLSlotElement).assignedElements({ flatten: true });
    const control =
      assigned.find((el) => el.matches('input, textarea, select, sema-input, sema-textarea, sema-select')) ??
      assigned[0] ??
      null;
    if (control !== this._control) {
      // A swapped-out control would otherwise keep this field's stale attributes.
      for (const attr of ['aria-label', 'aria-description', 'aria-invalid']) this._control?.removeAttribute(attr);
      this._control = control;
    }
    this._applyA11y();
  };

  // Shadow boundaries rule out IDREF associations (aria-labelledby/-describedby),
  // so mirror label/hint/error onto the control as plain string aria attributes.
  private _applyA11y() {
    const control = this._control;
    if (!control) return;
    if (this.label) control.setAttribute('aria-label', this.label);
    else control.removeAttribute('aria-label');
    const description = this.error || this.hint;
    if (description) control.setAttribute('aria-description', description);
    else control.removeAttribute('aria-description');
    if (this.error) control.setAttribute('aria-invalid', 'true');
    else control.removeAttribute('aria-invalid');
  }

  render() {
    const msg = this.error || this.hint;
    return html`
      <label class="field" part="field">
        ${this.label ? html`<span class="label" part="label">${this.label}</span>` : nothing}
        <slot @slotchange=${this._onSlotChange}></slot>
        ${msg
          ? html`<span class="msg ${this.error ? 'error' : ''}" part="message">${msg}</span>`
          : nothing}
      </label>
    `;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-field': SemaField;
  }
}
customElements.define('sema-field', SemaField);
