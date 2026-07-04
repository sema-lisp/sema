import { html, css } from 'lit';
import { property, queryAssignedElements } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';
import type { SemaChangeEventDetail } from './events.js';

export type TabsActivation = 'auto' | 'manual';

let _tabUid = 0;
let _panelUid = 0;

/**
 * `<sema-tabs>` — accessible panel tabs (WAI-ARIA tablist/tab/tabpanel).
 *
 * Author `<sema-tab>` and `<sema-tab-panel>` children paired by `value`; tabs
 * auto-assign themselves into the `nav` slot, so no `slot` attributes are
 * needed and source order is free:
 *
 * ```html
 * <sema-tabs value="versions">
 *   <sema-tab value="readme">Readme</sema-tab>
 *   <sema-tab value="versions">Versions <span>12</span></sema-tab>
 *   <sema-tab-panel value="readme">…</sema-tab-panel>
 *   <sema-tab-panel value="versions">…</sema-tab-panel>
 * </sema-tabs>
 * ```
 *
 * Boundary: `<sema-toggle-group>` picks a value (radiogroup — the app reacts to
 * sema-change); `<sema-tabs>` switches which content panel is visible
 * (tablist/tab/tabpanel — panel visibility is owned by the component).
 * Activation contrast is deliberate per APG: toggle-group is manual (arrows
 * move focus without selecting — radiogroup convention); tabs default to
 * automatic (arrows select), with `activation="manual"` as the opt-out for
 * panels whose reveal does work (e.g. lazy syntax highlighting).
 *
 * Selection is the `value` attribute/property — setting it from code updates
 * the UI without emitting; user activation (click, keys, find-in-page
 * `beforematch`, `hash-sync` navigation) emits `sema-change`. Inactive panels
 * get `hidden="until-found"` so find-in-page can reveal them. Add `hash-sync`
 * to adopt `location.hash` on load and mirror activation into the URL fragment
 * (via `replaceState`; one such instance per page).
 */
export class SemaTabs extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: block;
      }
      .tablist {
        display: flex;
        gap: var(--space-lg, 24px);
        overflow-x: auto;
        border-block-end: 1px solid var(--border, #1e1e1e);
        scrollbar-width: thin;
        scrollbar-color: var(--border, #1e1e1e) transparent;
      }
    `,
  ];

  /** Value of the active tab/panel pair. Programmatic writes do not emit `sema-change`. */
  @property({ reflect: true }) value = '';
  /** `auto`: arrow focus also selects (APG default). `manual`: arrows move focus; Enter/Space selects. */
  @property({ reflect: true }) activation: TabsActivation = 'auto';
  /** Opt-in deep-linking: adopt `location.hash` on connect, mirror activation via `replaceState`. */
  @property({ type: Boolean, reflect: true, attribute: 'hash-sync' }) hashSync = false;

  @queryAssignedElements({ slot: 'nav', selector: 'sema-tab' }) private _tabs!: SemaTab[];
  @queryAssignedElements({ selector: 'sema-tab-panel' }) private _panels!: SemaTabPanel[];

  private _wired = false;
  private _syncQueued = false;
  private _warnedValue: string | null = null;

  connectedCallback() {
    super.connectedCallback();
    // Find-in-page into a hidden="until-found" panel: activate its tab.
    this.addEventListener('beforematch', this._onBeforeMatch);
    window.addEventListener('hashchange', this._onHashChange);
    // slotchange does not re-fire for an unchanged assignment on reconnect.
    if (this.hasUpdated) this.updateComplete.then(() => { if (this.isConnected) this._sync(); });
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    this.removeEventListener('beforematch', this._onBeforeMatch);
    window.removeEventListener('hashchange', this._onHashChange);
  }

  updated(changed: Map<string, unknown>) {
    if (changed.has('value') && this._wired) this._applySelection();
  }

  render() {
    return html`
      <div
        class="tablist"
        role="tablist"
        part="tablist"
        aria-label=${this.getAttribute('aria-label') || 'Tabs'}
        @keydown=${this._onKeydown}
        @click=${this._onClick}
      >
        <slot name="nav" @slotchange=${this._sync}></slot>
      </div>
      <slot @slotchange=${this._sync}></slot>
    `;
  }

  /** Coalesced re-wire, used by child tabs/panels when their props flip. */
  _requestSync() {
    if (this._syncQueued) return;
    this._syncQueued = true;
    queueMicrotask(() => {
      this._syncQueued = false;
      if (this.isConnected) this._sync();
    });
  }

  private _enabledTabs(): SemaTab[] {
    return this._tabs.filter((t) => !t.disabled);
  }

  private _panelFor(value: string): SemaTabPanel | undefined {
    return this._panels.find((p) => p.value === value);
  }

  // Wire ids/roles/IDREF pairs on the light-DOM hosts (IDREFs only resolve
  // within one tree scope, so none of this can live in shadow DOM), then
  // resolve/repair selection.
  private _sync = () => {
    const tabs = this._tabs;
    const panels = this._panels;

    for (const tab of tabs) {
      if (!tab.id) tab.id = `sema-tab-${++_tabUid}`;
      tab.setAttribute('role', 'tab');
    }
    for (const panel of panels) {
      if (!panel.id) panel.id = `sema-tab-panel-${++_panelUid}`;
      panel.setAttribute('role', 'tabpanel');
    }
    for (const tab of tabs) {
      const panel = this._panelFor(tab.value);
      if (panel) {
        tab.setAttribute('aria-controls', panel.id);
        panel.setAttribute('aria-labelledby', tab.id);
      } else {
        tab.removeAttribute('aria-controls');
      }
    }

    const enabled = this._enabledTabs();
    const valid = (v: string) => v !== '' && enabled.some((t) => t.value === v);

    if (!this._wired) {
      // Initial precedence: hash (when hash-sync) > value attr > pre-set selected > first enabled.
      const hash = this.hashSync ? window.location.hash.slice(1) : '';
      if (valid(hash)) {
        this.value = hash;
      } else if (!valid(this.value)) {
        if (this.value && this._warnedValue !== this.value) {
          this._warnedValue = this.value;
          console.warn(`<sema-tabs> value="${this.value}" matches no enabled tab`);
        }
        const preset = enabled.find((t) => t.selected);
        this.value = (preset ?? enabled[0])?.value ?? '';
      }
      this._wired = tabs.length > 0;
    } else if (!valid(this.value)) {
      // Repair (active tab removed/disabled): not user intent, no event.
      this.value = enabled[0]?.value ?? '';
    }

    this._applySelection();
  };

  private _applySelection() {
    const tabs = this._tabs;
    const selected = tabs.find((t) => !t.disabled && t.value === this.value && this.value !== '');
    // Exactly one tab is a tab stop: the selection, else the first tab so the
    // tablist stays keyboard-reachable even with nothing selectable.
    const stop = selected ?? this._enabledTabs()[0] ?? tabs[0];

    for (const tab of tabs) {
      tab.selected = tab === selected;
      tab.setAttribute('aria-selected', String(tab === selected));
      tab.setAttribute('tabindex', tab === stop ? '0' : '-1');
      if (tab.disabled) tab.setAttribute('aria-disabled', 'true');
      else tab.removeAttribute('aria-disabled');
    }

    for (const panel of this._panels) {
      if (selected && panel.value === this.value) {
        panel.removeAttribute('hidden');
        this._setPanelFocusability(panel);
      } else {
        panel.setAttribute('hidden', 'until-found');
        panel.removeAttribute('tabindex');
      }
    }
  }

  // APG: a tabpanel with no focusable content is itself a tab stop.
  private _setPanelFocusability(panel: SemaTabPanel) {
    const focusable = panel.querySelector(
      'a[href], button:not([disabled]), input:not([disabled]), select, textarea, [tabindex], audio[controls], video[controls], sema-button, sema-input, sema-textarea, sema-select',
    );
    if (focusable) panel.removeAttribute('tabindex');
    else panel.setAttribute('tabindex', '0');
  }

  private _tabFromEvent(e: Event): SemaTab | null {
    for (const el of e.composedPath()) {
      if (el instanceof HTMLElement && el.matches('sema-tab')) return el as SemaTab;
    }
    return null;
  }

  private _activate(tab: SemaTab) {
    if (tab.disabled || tab.value === this.value) return;
    this.value = tab.value;
    this._applySelection();
    if (this.hashSync) history.replaceState(null, '', '#' + this.value);
    tab.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    this.dispatchEvent(new CustomEvent<SemaChangeEventDetail>('sema-change', {
      detail: { value: this.value },
      bubbles: true,
      composed: true,
    }));
  }

  /** Move the roving tab stop (and focus) without selecting. */
  private _roveTo(tab: SemaTab) {
    for (const t of this._tabs) t.setAttribute('tabindex', t === tab ? '0' : '-1');
    tab.focus();
  }

  private _onClick = (e: Event) => {
    const tab = this._tabFromEvent(e);
    if (tab) this._activate(tab);
  };

  private _onKeydown = (e: KeyboardEvent) => {
    const tab = this._tabFromEvent(e);
    if (!tab) return;
    const enabled = this._enabledTabs();
    const idx = enabled.indexOf(tab);

    let target: SemaTab | undefined;
    if (e.key === 'ArrowRight') target = enabled[(idx + 1) % enabled.length];
    else if (e.key === 'ArrowLeft') target = enabled[(idx - 1 + enabled.length) % enabled.length];
    else if (e.key === 'Home') target = enabled[0];
    else if (e.key === 'End') target = enabled[enabled.length - 1];
    else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      this._activate(tab);
      return;
    } else {
      return; // ArrowUp/Down deliberately unbound: horizontal tablist, let the page scroll
    }

    e.preventDefault();
    if (!target || idx < 0) return;
    this._roveTo(target);
    if (this.activation === 'auto') this._activate(target);
  };

  private _onBeforeMatch = (e: Event) => {
    const panel = e.target as Element | null;
    if (!(panel instanceof HTMLElement) || !panel.matches('sema-tab-panel')) return;
    const tab = this._enabledTabs().find((t) => t.value === (panel as SemaTabPanel).value);
    if (tab) this._activate(tab);
  };

  private _onHashChange = () => {
    if (!this.hashSync) return;
    const tab = this._enabledTabs().find((t) => t.value === window.location.hash.slice(1));
    if (tab) this._activate(tab);
  };
}

/**
 * `<sema-tab>` — one tab in a `<sema-tabs>` tablist. The host is the
 * interactive element (role/tabindex/ARIA are written onto it by the group so
 * its IDREFs resolve in the light DOM); the default slot takes arbitrary
 * inline label content (text, count badges, …).
 */
export class SemaTab extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xs, 11px);
        letter-spacing: 0.02em;
        padding: var(--space-sm, 8px) var(--space-xs, 4px);
        cursor: pointer;
        white-space: nowrap;
        user-select: none;
        color: var(--text-tertiary, #5a5448);
        /* Indicator overlaps the tablist's 1px bottom border. */
        border-block-end: 2px solid transparent;
        margin-block-end: -1px;
        transition: color 0.15s, border-color 0.15s;
      }
      :host(:hover) {
        color: var(--text-secondary, #a09888);
      }
      :host([selected]) {
        color: var(--gold, #c8a855);
        border-block-end-color: var(--gold, #c8a855);
      }
      :host([disabled]) {
        color: var(--text-tertiary, #5a5448);
        opacity: 0.5;
        cursor: not-allowed;
      }
      :host(:focus) {
        outline: none;
      }
      :host(:focus-visible) {
        outline: var(--focus-ring-width, 1px) solid var(--focus-ring-color-subtle, rgba(200, 168, 85, 0.5));
        outline-offset: var(--focus-ring-offset, 1px);
      }
    `,
  ];

  /** Pairs this tab with the `<sema-tab-panel>` of the same value. */
  @property({ reflect: true }) value = '';
  /** Group-managed; may be pre-set by the author as an initial-selection hint. */
  @property({ type: Boolean, reflect: true }) selected = false;
  /** Visible but skipped by arrows and never activatable. */
  @property({ type: Boolean, reflect: true }) disabled = false;

  connectedCallback() {
    super.connectedCallback();
    // Tabs live in the nav slot; panels in the default slot — authors never
    // write slot attributes themselves.
    if (!this.slot) this.slot = 'nav';
  }

  updated(changed: Map<string, unknown>) {
    if (changed.has('disabled') || changed.has('value')) {
      (this.closest('sema-tabs') as SemaTabs | null)?._requestSync();
    }
  }

  render() {
    return html`<slot></slot>`;
  }
}

/**
 * `<sema-tab-panel>` — the content pane paired with a `<sema-tab>` by `value`.
 * Visibility is group-managed via `hidden="until-found"`, so find-in-page can
 * reveal inactive panels; style the active state with `:host(:not([hidden]))`.
 */
export class SemaTabPanel extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: block;
      }
      /* :host { display } defeats the UA [hidden] rule, so restore it — but not
         for until-found, which must stay laid out (content-visibility: hidden)
         or find-in-page can never match it. */
      :host([hidden]:not([hidden='until-found'])) {
        display: none !important;
      }
      .panel {
        padding-block-start: var(--space-md, 16px);
      }
    `,
  ];

  /** Pairs this panel with the `<sema-tab>` of the same value. */
  @property({ reflect: true }) value = '';

  updated(changed: Map<string, unknown>) {
    if (changed.has('value')) {
      (this.closest('sema-tabs') as SemaTabs | null)?._requestSync();
    }
  }

  render() {
    return html`<div class="panel" part="base"><slot></slot></div>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-tabs': SemaTabs;
    'sema-tab': SemaTab;
    'sema-tab-panel': SemaTabPanel;
  }
}
customElements.define('sema-tabs', SemaTabs);
customElements.define('sema-tab', SemaTab);
customElements.define('sema-tab-panel', SemaTabPanel);
