import { html, css } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';
import type { SemaToggle } from './sema-toggle.js';
import type { SemaChangeEventDetail } from './events.js';

/**
 * `<sema-toggle-group>` — a radiogroup of `<sema-toggle>` children; arrow keys
 * rove focus and Enter/Space selects (APG manual activation), emitting
 * `sema-change` with the chosen value.
 *
 * Boundary: `<sema-toggle-group>` picks a value (radiogroup — the app reacts to
 * sema-change); `<sema-tabs>` switches which content panel is visible
 * (tablist/tab/tabpanel — panel visibility is owned by the component).
 */
export class SemaToggleGroup extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: inline-flex;
        align-items: center;
        gap: 8px;
      }
      .group {
        display: flex;
        align-items: center;
        gap: 0;
      }
      .separator {
        width: 1px;
        height: 16px;
        background: var(--border, #1e1e1e);
        margin: 0 4px;
      }
    `,
  ];

  @property({ reflect: true }) value: string = '';

  /** All descendant toggles. A getter (not queryAssignedElements) so each toggle may
   *  be wrapped — e.g. in a `<sema-tooltip>` to explain the option — and still count. */
  private get _toggles(): SemaToggle[] {
    return [...this.querySelectorAll('sema-toggle')] as SemaToggle[];
  }

  render() {
    return html`
      <div class="group" role="radiogroup" @keydown=${this._onKeydown} @click=${this._onClick}>
        <slot @slotchange=${this._onSlotChange}></slot>
      </div>
    `;
  }

  // Keep the toggles in sync when `value` is set programmatically (controlled use,
  // e.g. restoring a saved selection) — not just on user interaction / slotchange.
  updated(changed: Map<string, unknown>) {
    if (changed.has('value')) this._updateSelection();
  }

  private _onSlotChange() {
    this._updateSelection();
  }

  private _onClick(e: Event) {
    const path = e.composedPath();
    for (const el of path) {
      if (el instanceof HTMLElement && el.matches('sema-toggle')) {
        this.value = (el as unknown as SemaToggle).value;
        this._emitChange();
        this._updateSelection();
        return;
      }
    }
  }

  private _onKeydown(e: KeyboardEvent) {
    const path = e.composedPath();
    let target: Element | null = null;
    for (const el of path) {
      if (el instanceof HTMLElement && el.matches('sema-toggle')) {
        target = el;
        break;
      }
    }
    if (!target) return;
    const toggle = target as unknown as SemaToggle;
    const idx = this._toggles.indexOf(toggle);
    if (idx < 0) return;

    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
      e.preventDefault();
      const next = (idx + 1) % this._toggles.length;
      this._setTabbable(next);
      this._toggles[next].focus();
    } else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
      e.preventDefault();
      const prev = (idx - 1 + this._toggles.length) % this._toggles.length;
      this._setTabbable(prev);
      this._toggles[prev].focus();
    } else if (e.key === ' ' || e.key === 'Enter') {
      e.preventDefault();
      this.value = toggle.value;
      this._emitChange();
      this._updateSelection();
    }
  }

  private _updateSelection() {
    let selectedIdx = -1;
    this._toggles.forEach((t, i) => {
      t.selected = t.value === this.value;
      if (t.selected) selectedIdx = i;
    });
    // Exactly one toggle must be a tab stop (WAI-ARIA radiogroup). When nothing is
    // selected, fall back to the first so the group is still keyboard-reachable.
    this._setTabbable(selectedIdx >= 0 ? selectedIdx : 0);
  }

  private _setTabbable(idx: number) {
    this._toggles.forEach((t, i) => {
      t.tabbable = i === idx;
    });
  }

  private _emitChange() {
    this.dispatchEvent(new CustomEvent<SemaChangeEventDetail>('sema-change', {
      detail: { value: this.value },
      bubbles: true,
      composed: true,
    }));
  }
}

declare global { interface HTMLElementTagNameMap { 'sema-toggle-group': SemaToggleGroup } }
customElements.define('sema-toggle-group', SemaToggleGroup);
