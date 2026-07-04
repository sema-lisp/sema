import { html, css } from 'lit';
import { property, queryAssignedElements } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';
import type { SemaSelectEventDetail } from './events.js';

/**
 * `<sema-menu>` + `<sema-menu-item>` — a keyboard-navigable menu list.
 *
 * Usually placed inside a `<sema-popover>`. Arrow keys rove focus, Home/End jump,
 * Enter/Space or click selects. Selecting emits `sema-select` with `{ value, item }`
 * (which a parent popover listens for to auto-close).
 *
 * ```html
 * <sema-menu>
 *   <sema-menu-item value="code">Code cell</sema-menu-item>
 *   <sema-menu-item value="md">Markdown cell</sema-menu-item>
 * </sema-menu>
 * ```
 */
export class SemaMenu extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: block;
        min-width: 160px;
      }
      [role='menu'] {
        display: flex;
        flex-direction: column;
      }
    `,
  ];

  @queryAssignedElements({ selector: 'sema-menu-item' })
  private _items!: SemaMenuItem[];

  private get _enabled(): SemaMenuItem[] {
    return this._items.filter((i) => !i.disabled);
  }

  /** Focus the first enabled item (called by the popover on open). */
  focusFirst() {
    this._enabled[0]?.focus();
  }

  private _activeIndex(): number {
    const items = this._enabled;
    const active = this.querySelector('sema-menu-item:focus') as SemaMenuItem | null;
    return active ? items.indexOf(active) : -1;
  }

  private _focusAt(i: number) {
    const items = this._enabled;
    if (items.length === 0) return;
    const idx = (i + items.length) % items.length;
    items[idx].focus();
  }

  private _onKeydown = (e: KeyboardEvent) => {
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        this._focusAt(this._activeIndex() + 1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        this._focusAt(this._activeIndex() - 1);
        break;
      case 'Home':
        e.preventDefault();
        this._focusAt(0);
        break;
      case 'End':
        e.preventDefault();
        this._focusAt(this._enabled.length - 1);
        break;
      case 'Enter':
      case ' ': {
        const active = this.querySelector('sema-menu-item:focus') as SemaMenuItem | null;
        if (active) {
          e.preventDefault();
          this._select(active);
        }
        break;
      }
    }
  };

  private _onClick = (e: Event) => {
    const item = (e.target as HTMLElement).closest('sema-menu-item') as SemaMenuItem | null;
    if (item && !item.disabled) this._select(item);
  };

  private _select(item: SemaMenuItem) {
    this.dispatchEvent(
      new CustomEvent<SemaSelectEventDetail>('sema-select', {
        detail: { value: item.value, item },
        bubbles: true,
        composed: true,
      }),
    );
  }

  render() {
    return html`<div
      role="menu"
      aria-label=${this.getAttribute('aria-label') || 'Menu'}
      @keydown=${this._onKeydown}
      @click=${this._onClick}
    >
      <slot></slot>
    </div>`;
  }
}

export class SemaMenuItem extends SemaElement {
  // delegatesFocus so the host matches :focus when its inner button is focused —
  // required for the menu's roving keyboard nav (which detects the active item via :focus).
  static shadowRootOptions = { ...SemaElement.shadowRootOptions, delegatesFocus: true };
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: block;
      }
      .item {
        display: flex;
        align-items: center;
        gap: var(--space-sm, 8px);
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
      .item:hover:not([disabled]),
      .item:focus-visible {
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
        color: var(--gold, #c8a855);
        outline: none;
      }
      .item:focus-visible {
        box-shadow: inset 0 0 0 1px var(--gold-dim, rgba(200, 168, 85, 0.5));
      }
      .item[disabled] {
        color: var(--text-tertiary, #5a5448);
        cursor: not-allowed;
      }
    `,
  ];

  /** Value reported in the menu's `sema-select` event. */
  @property() value = '';
  @property({ type: Boolean, reflect: true }) disabled = false;

  focus() {
    this.shadowRoot?.querySelector<HTMLElement>('.item')?.focus();
  }

  render() {
    return html`<button
      class="item"
      part="item"
      role="menuitem"
      type="button"
      tabindex="-1"
      ?disabled=${this.disabled}
    >
      <slot></slot>
    </button>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-menu': SemaMenu;
    'sema-menu-item': SemaMenuItem;
  }
}
customElements.define('sema-menu', SemaMenu);
customElements.define('sema-menu-item', SemaMenuItem);
