import { html, css, nothing } from 'lit';
import { property, state } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';
import type { SemaTreeSelectEventDetail } from './events.js';

export class SemaTree extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: block;
      }
    `,
  ];

  connectedCallback() {
    super.connectedCallback();
    this.setAttribute('role', 'tree');
    // Accessible-name fallback (mirrors sema-menu's 'Menu' default).
    if (!this.hasAttribute('aria-label') && !this.hasAttribute('aria-labelledby')) {
      this.setAttribute('aria-label', 'Tree');
    }
  }

  // Ensure exactly one tree item is a tab stop (roving tabindex).
  private _onSlotChange = () => {
    const items = Array.from(this.querySelectorAll('sema-tree-item')) as SemaTreeItem[];
    if (items.length && !items.some((i) => i.tabbable)) items[0].tabbable = true;
  };

  render() {
    return html`<slot @slotchange=${this._onSlotChange}></slot>`;
  }
}

export class SemaTreeItem extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: block;
      }
      .row {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 4px 12px;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xs, 11px);
        color: var(--text-secondary, #a09888);
        cursor: pointer;
        user-select: none;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        transition: color 0.1s, background 0.1s;
        outline: none;
      }
      .row:hover {
        color: var(--text-primary, #d8d0c0);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
      }
      .row:focus-visible {
        outline: 1px solid var(--gold-dim, rgba(200, 168, 85, 0.5));
        outline-offset: -1px;
      }
      :host([selected]) .row {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
      }

      /* Top-level parent items read as section headers (uppercased, dimmed,
         letter-spaced), distinct from leaves. Keeps the base --mono family so
         the header never picks up the consumer's ambient font (e.g. a serif
         page body). Gated to depth 0 so nested dirs stay normal. */
      :host([depth='0'][has-children]) .row {
        text-transform: uppercase;
        letter-spacing: 0.06em;
        font-size: var(--text-xxs, 10px);
        color: var(--text-tertiary, #5a5448);
      }

      .chevron {
        font-size: var(--text-xxs, 10px);
        width: 13px;
        text-align: center;
        flex-shrink: 0;
        color: var(--text-tertiary, #5a5448);
      }
      :host(:not([has-children])) .chevron {
        visibility: hidden;
      }
      :host([expanded]) .chevron {
        transform: rotate(0deg);
      }
      :host(:not([expanded])) .chevron {
        transform: rotate(-90deg);
      }

      .label {
        overflow: hidden;
        text-overflow: ellipsis;
      }

      .children {
        display: none;
      }
      :host([expanded]) .children {
        display: block;
      }
    `,
  ];

  @property({ reflect: true }) label?: string;
  @property({ type: Boolean, reflect: true }) expanded = false;
  @property({ type: Boolean, reflect: true }) selected = false;
  @property({ type: Boolean, reflect: true, attribute: 'has-children' }) hasChildren = false;
  @property({ type: Number, reflect: true }) depth = 0;
  /** Roving tab stop within the tree (managed by SemaTree / arrow navigation). */
  @property({ type: Boolean, reflect: true }) tabbable = false;

  @state() private _hasSlotChildren = false;

  render() {
    const padLeft = 12 + this.depth * 14;
    return html`
      <div class="row" part="row" role="treeitem" tabindex=${this.tabbable ? '0' : '-1'}
           style="padding-left:${padLeft}px;"
        aria-label=${this.label || nothing}
        aria-expanded=${this.hasChildren || this._hasSlotChildren ? String(this.expanded) : nothing}
        aria-selected=${String(this.selected)}
           aria-level=${this.depth + 1}
           @click=${this._onClick}
           @keydown=${this._onKeydown}>
        <span class="chevron">&#x25BE;</span>
        <span class="label" part="label">${this.label}<slot name="label"></slot></span>
      </div>
      <div class="children"><slot @slotchange=${this._onSlotChange}></slot></div>
    `;
  }

  connectedCallback() {
    super.connectedCallback();
    this._updateDepth();
  }

  private _updateDepth() {
    let parent = this.parentElement;
    let d = 0;
    while (parent) {
      if (parent instanceof SemaTreeItem) d++;
      if (parent instanceof SemaTree) break;
      parent = parent.parentElement;
      if (!parent?.parentElement) break;
    }
    this.depth = d;
  }

  private _onSlotChange() {
    this._hasSlotChildren = (this.shadowRoot?.querySelector('slot')?.assignedElements().length ?? 0) > 0;
  }

  private _onClick(e: Event) {
    e.stopPropagation();
    if (this._hasSlotChildren || this.hasChildren) {
      this.expanded = !this.expanded;
    }
    this._select();
    this._makeTabStop();
  }

  /** Become the single roving tab stop within the tree. */
  private _makeTabStop() {
    const tree = this.closest('sema-tree');
    tree?.querySelectorAll('sema-tree-item').forEach((it) => {
      (it as SemaTreeItem).tabbable = it === this;
    });
  }

  private _onKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      e.stopPropagation();
      this._onClick(e);
    } else if (e.key === 'ArrowRight' && !this.expanded) {
      e.preventDefault();
      e.stopPropagation();
      this.expanded = true;
    } else if (e.key === 'ArrowLeft' && this.expanded) {
      e.preventDefault();
      e.stopPropagation();
      this.expanded = false;
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      e.stopPropagation();
      this._focusAdjacent('next');
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      e.stopPropagation();
      this._focusAdjacent('prev');
    }
  }

  focus() {
    const row = this.shadowRoot?.querySelector('.row') as HTMLElement | null;
    row?.focus();
  }

  private _select() {
    this.dispatchEvent(new CustomEvent<SemaTreeSelectEventDetail>('sema-tree-select', {
      detail: { label: this.label, element: this },
      bubbles: true,
      composed: true,
    }));
  }

  private _focusAdjacent(dir: 'next' | 'prev') {
    const item = this.shadowRoot!.querySelector('.row') as HTMLElement;
    if (!item) return;

    // Flatten all visible tree items in the parent tree
    const tree = this.closest('sema-tree') as SemaTree | null;
    if (!tree) return;

    // Recursively collect visible items from tree content
    const collect = (root: Element): SemaTreeItem[] => {
      const result: SemaTreeItem[] = [];
      const children = root instanceof HTMLSlotElement
        ? root.assignedElements()
        : Array.from(root.children);
      for (const child of children) {
        if (child instanceof SemaTreeItem) {
          result.push(child);
          // Descend only into a truly-leaf node or an expanded one. Use BOTH the explicit
          // `has-children` attribute and the auto-detected slot flag, else we walk into the
          // collapsed (display:none) children of an auto-detected parent.
          const hasKids = child.hasChildren || child._hasSlotChildren;
          if (child.expanded || !hasKids) {
            const slot = child.shadowRoot?.querySelector('.children slot') as HTMLSlotElement | null;
            if (slot) {
              result.push(...collect(slot));
            }
          }
        } else {
          result.push(...collect(child));
        }
      }
      return result;
    };

    const allItems = collect(tree);
    const idx = allItems.indexOf(this);
    const nextIdx = dir === 'next' ? idx + 1 : idx - 1;
    if (nextIdx >= 0 && nextIdx < allItems.length) {
      const target = allItems[nextIdx];
      // Move the single tab stop to the newly focused item (roving tabindex).
      tree.querySelectorAll('sema-tree-item').forEach((it) => {
        (it as SemaTreeItem).tabbable = it === target;
      });
      target.focus();
    }
  }
}

declare global { interface HTMLElementTagNameMap { 'sema-tree': SemaTree; 'sema-tree-item': SemaTreeItem } }
customElements.define('sema-tree', SemaTree);
customElements.define('sema-tree-item', SemaTreeItem);
