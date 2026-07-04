import { html, css, unsafeCSS } from 'lit';
import { property, state, query } from 'lit/decorators.js';
import { ifDefined } from 'lit/directives/if-defined.js';
import { live } from 'lit/directives/live.js';
import { SemaElement } from '../internal/sema-element.js';
import './sema-popover.js';
import type { SemaPopover } from './sema-popover.js';
import controlStyles from '../styles/control.css?inline';

type Opt = { value: string; label: string; disabled: boolean };
type Group = { label: string; options: Opt[] };
type Entry = Opt | Group;

/**
 * `<sema-select>` — a themed, form-associated select.
 *
 * By default renders a **custom dropdown** (a themeable `role="listbox"` on
 * `<sema-popover>`). Add the **`native`** flag to fall back to a styled native
 * `<select>` (OS-rendered list; useful on mobile or when full native behavior is
 * preferred). Author options as light-DOM `<option>` / `<optgroup>` children either way.
 *
 * ```html
 * <sema-select name="engine" value="vm">
 *   <option value="tw">Tree-walker</option>
 *   <option value="vm">Bytecode VM</option>
 * </sema-select>
 * <sema-select native> … </sema-select>
 * ```
 */
export class SemaSelect extends SemaElement {
  static formAssociated = true;
  static styles = [
    SemaElement.base,
    unsafeCSS(controlStyles),
    css`
      :host {
        display: block;
      }
      /* custom trigger */
      .trigger {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        cursor: pointer;
        text-align: left;
      }
      .trigger .label {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
      }
      .placeholder {
        color: var(--text-tertiary, #5a5448);
      }
      .chevron {
        flex-shrink: 0;
        font-size: 0.7em;
        color: var(--text-tertiary, #5a5448);
        transition: transform 0.15s;
      }
      .trigger[aria-expanded='true'] .chevron {
        transform: rotate(180deg);
      }
      /* custom listbox */
      .listbox {
        display: flex;
        flex-direction: column;
        min-width: 160px;
        max-height: 256px;
        overflow-y: auto;
        scrollbar-width: thin;
        scrollbar-color: var(--border, #1e1e1e) transparent;
      }
      .group-label {
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xxs, 10px);
        text-transform: uppercase;
        letter-spacing: 0.06em;
        color: var(--text-tertiary, #5a5448);
        padding: 6px 11px 3px;
      }
      .option {
        display: flex;
        align-items: center;
        gap: 8px;
        width: 100%;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-sm, 12px);
        text-align: left;
        padding: 6px 11px;
        border: none;
        border-radius: var(--radius-sm, 3px);
        background: transparent;
        color: var(--text-primary, #d8d0c0);
        cursor: pointer;
        white-space: nowrap;
      }
      .option:hover:not([disabled]),
      .option:focus-visible {
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
        color: var(--gold, #c8a855);
        outline: none;
      }
      .option:focus-visible {
        box-shadow: inset 0 0 0 1px var(--gold-dim, rgba(200, 168, 85, 0.5));
      }
      .option[aria-selected='true'] {
        color: var(--gold, #c8a855);
      }
      .option[disabled] {
        color: var(--text-tertiary, #5a5448);
        cursor: not-allowed;
      }
      .check {
        width: 1em;
        text-align: center;
      }
      select.control {
        cursor: pointer;
      }
      slot {
        display: none;
      }
    `,
  ];

  @property() value = '';
  @property() name = '';
  @property() placeholder = 'Select…';
  @property({ type: Boolean, reflect: true }) disabled = false;
  @property({ type: Boolean, reflect: true }) required = false;
  /** Use a styled native `<select>` instead of the custom dropdown. */
  @property({ type: Boolean, reflect: true }) native = false;

  @state() private _entries: Entry[] = [];
  @state() private _open = false;
  @query('sema-popover') private _pop?: SemaPopover;
  private static _uid = 0;
  private _listboxId = `sema-listbox-${++SemaSelect._uid}`;
  private _internals = this.attachInternals();

  // Open: focus the selected option (or first enabled). rAF so this runs AFTER the
  // popover's own first-focusable focus, so the *selected* option wins.
  private _onOpen = () => {
    this._open = true;
    requestAnimationFrame(() => {
      if (!this._open) return;
      const selected = this.shadowRoot?.querySelector<HTMLElement>(
        `.option[data-value="${CSS.escape(this.value)}"]:not([disabled])`,
      );
      (selected ?? this._enabledOptions()[0])?.focus();
    });
  };

  firstUpdated() {
    this._sync();
  }

  // Host aria-* attributes (set e.g. by <sema-field>) must be mirrored onto the
  // inner control, where AT computes name/description — re-render when they change.
  static get observedAttributes() {
    return [...super.observedAttributes, 'aria-label', 'aria-description', 'aria-invalid'];
  }

  attributeChangedCallback(name: string, old: string | null, value: string | null) {
    super.attributeChangedCallback(name, old, value);
    if (name.startsWith('aria-')) this.requestUpdate();
  }

  updated(changed: Map<string, unknown>) {
    if (changed.has('value')) this._internals.setFormValue(this.value);
    if (changed.has('value') || changed.has('required')) this._syncValidity();
    if (this.native) {
      const select = this.shadowRoot?.querySelector('select');
      if (select && select.value !== this.value) select.value = this.value;
    }
  }

  formResetCallback() {
    this.value = this._firstValue();
    this._internals.setFormValue(this.value);
    this._syncValidity();
  }

  private _syncValidity() {
    if (this.required && !this.value) {
      const anchor = this.shadowRoot?.querySelector<HTMLElement>(this.native ? 'select' : '.trigger') ?? undefined;
      this._internals.setValidity({ valueMissing: true }, 'Please select an option', anchor);
    } else {
      this._internals.setValidity({});
    }
  }

  private _flat(): Opt[] {
    return this._entries.flatMap((e) => ('options' in e ? e.options : [e]));
  }
  private _firstValue(): string {
    return this._flat()[0]?.value ?? '';
  }
  private _labelFor(v: string): string | null {
    return this._flat().find((o) => o.value === v)?.label ?? null;
  }

  private _readOption(o: HTMLOptionElement): Opt {
    return { value: o.value, label: o.textContent ?? '', disabled: o.disabled };
  }

  private _sync = () => {
    const entries: Entry[] = [];
    for (const child of Array.from(this.children)) {
      if (child instanceof HTMLOptGroupElement) {
        entries.push({
          label: child.label,
          options: Array.from(child.querySelectorAll('option')).map((o) => this._readOption(o)),
        });
      } else if (child instanceof HTMLOptionElement) {
        entries.push(this._readOption(child));
      }
    }
    this._entries = entries;
    if (!this.value) this.value = this._firstValue();
    this._internals.setFormValue(this.value);
    this._syncValidity();
  };

  private _select(value: string) {
    this.value = value;
    this._internals.setFormValue(value);
    this._syncValidity();
    this._pop?.hide();
    this.dispatchEvent(new Event('change', { bubbles: true, composed: true }));
  }

  private _enabledOptions(): HTMLButtonElement[] {
    return Array.from(this.shadowRoot?.querySelectorAll<HTMLButtonElement>('.option:not([disabled])') ?? []);
  }

  private _onTriggerKeydown = (e: KeyboardEvent) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      this._pop?.show();
    }
  };

  private _onListKeydown = (e: KeyboardEvent) => {
    const opts = this._enabledOptions();
    if (opts.length === 0) return;
    const active = this.shadowRoot?.activeElement as HTMLButtonElement | null;
    const i = active ? opts.indexOf(active) : -1;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        opts[(i + 1 + opts.length) % opts.length].focus();
        break;
      case 'ArrowUp':
        e.preventDefault();
        opts[(i - 1 + opts.length) % opts.length].focus();
        break;
      case 'Home':
        e.preventDefault();
        opts[0].focus();
        break;
      case 'End':
        e.preventDefault();
        opts[opts.length - 1].focus();
        break;
      case 'Enter':
      case ' ':
        e.preventDefault();
        (active ?? opts[0]).click();
        break;
    }
  };

  private _onNativeChange = (e: Event) => {
    this.value = (e.target as HTMLSelectElement).value;
    this._internals.setFormValue(this.value);
    this.dispatchEvent(new Event('change', { bubbles: true, composed: true }));
  };

  private _optionTpl(o: Opt) {
    const selected = o.value === this.value;
    return html`<button
      class="option"
      role="option"
      type="button"
      tabindex="-1"
      data-value=${o.value}
      aria-selected=${String(selected)}
      ?disabled=${o.disabled}
      @click=${() => this._select(o.value)}
    >
      <span class="check">${selected ? '✓' : ''}</span><span>${o.label}</span>
    </button>`;
  }

  private _renderCustom() {
    const label = this._labelFor(this.value);
    return html`
      <sema-popover
        placement="bottom-start"
        @sema-open=${this._onOpen}
        @sema-close=${() => (this._open = false)}
      >
        <button
          slot="trigger"
          class="control trigger"
          part="control"
          type="button"
          ?disabled=${this.disabled}
          aria-haspopup="listbox"
          aria-expanded=${String(this._open)}
          aria-controls=${this._listboxId}
          aria-label=${this.getAttribute('aria-label') || this.name || 'select'}
          aria-description=${ifDefined(this.getAttribute('aria-description') ?? undefined)}
          aria-invalid=${ifDefined(this.getAttribute('aria-invalid') ?? undefined)}
          @keydown=${this._onTriggerKeydown}
        >
          <span class="label ${label === null ? 'placeholder' : ''}">${label ?? this.placeholder}</span>
          <span class="chevron" aria-hidden="true">▾</span>
        </button>
        <div
          class="listbox"
          id=${this._listboxId}
          role="listbox"
          aria-label=${this.getAttribute('aria-label') || this.name || 'options'}
          @keydown=${this._onListKeydown}
        >
          ${this._entries.map((e) =>
            'options' in e
              ? html`<div class="group-label" role="presentation">${e.label}</div>
                  ${e.options.map((o) => this._optionTpl(o))}`
              : this._optionTpl(e),
          )}
        </div>
      </sema-popover>
      <slot @slotchange=${this._sync}></slot>
    `;
  }

  private _renderNative() {
    return html`
      <select
        class="control"
        part="control"
        .value=${live(this.value)}
        ?disabled=${this.disabled}
        ?required=${this.required}
        aria-label=${this.getAttribute('aria-label') || this.name || 'select'}
        aria-description=${ifDefined(this.getAttribute('aria-description') ?? undefined)}
        aria-invalid=${ifDefined(this.getAttribute('aria-invalid') ?? undefined)}
        @change=${this._onNativeChange}
      >
        ${this._entries.map((e) =>
          'options' in e
            ? html`<optgroup label=${e.label}>
                ${e.options.map((o) => html`<option value=${o.value} ?disabled=${o.disabled}>${o.label}</option>`)}
              </optgroup>`
            : html`<option value=${e.value} ?disabled=${e.disabled}>${e.label}</option>`,
        )}
      </select>
      <slot @slotchange=${this._sync}></slot>
    `;
  }

  render() {
    return this.native ? this._renderNative() : this._renderCustom();
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-select': SemaSelect;
  }
}
customElements.define('sema-select', SemaSelect);
