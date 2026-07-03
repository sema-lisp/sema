import { html, css } from 'lit';
import { property, state } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';
import './sema-code-editor.js';
import './sema-markdown.js';

/**
 * `<sema-editable-markdown>` — edit-in-place markdown. Shows rendered markdown;
 * click to edit (highlighted markdown source), blur or Shift+Enter to render.
 * Owns the view↔edit toggle so hosts only bind `value` + listen for `change`.
 *
 * Events: `input` (`{ value }`) per keystroke, `change` (`{ value }`) on commit
 * (blur / Shift+Enter).
 */
export class SemaEditableMarkdown extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: block;
      }
      .empty {
        color: var(--text-tertiary, #5a5448);
        font-style: italic;
        cursor: text;
        padding: 0.4em 0;
      }
    `,
  ];

  @property() value = '';
  @property() placeholder = 'Empty markdown — click to edit';
  @property({ type: Boolean, reflect: true }) readonly = false;
  @state() private _editing = false;
  private _initialized = false;

  willUpdate() {
    // Decide the initial mode once `value` has actually been assigned (it is set
    // after construction by the host), not at connect time when it's still ''.
    if (!this._initialized) {
      this._initialized = true;
      this._editing = this.value.trim() === '' && !this.readonly;
    }
  }

  private _edit = () => {
    if (this.readonly) return;
    this._editing = true;
    void this.updateComplete.then(() => {
      const ed = this.shadowRoot?.querySelector('sema-code-editor') as
        | (HTMLElement & { focus?: () => void })
        | null;
      ed?.focus?.();
    });
  };

  private _commit() {
    if (!this._editing) return;
    this._editing = false;
    this.dispatchEvent(
      new CustomEvent('change', { detail: { value: this.value }, bubbles: true, composed: true }),
    );
  }

  private _onInput = (e: Event) => {
    this.value = (e as CustomEvent<{ value: string }>).detail.value;
    this.dispatchEvent(
      new CustomEvent('input', { detail: { value: this.value }, bubbles: true, composed: true }),
    );
  };

  private _onFocusout = () => {
    // Render when leaving a non-empty editor; an empty editor stays open.
    if (this.value.trim()) this._commit();
  };

  private _onKeydown = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && e.shiftKey) {
      e.preventDefault();
      this._commit();
    }
  };

  private _onEmptyKeydown = (e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      this._edit();
    }
  };

  render() {
    if (this._editing) {
      return html`<sema-code-editor
        lang="markdown"
        autosize
        testid="cell-textarea"
        .value=${this.value}
        .placeholder=${this.placeholder}
        @input=${this._onInput}
        @focusout=${this._onFocusout}
        @keydown=${this._onKeydown}
      ></sema-code-editor>`;
    }
    if (!this.value.trim()) {
      return html`<div
        class="empty"
        data-testid="markdown-rendered"
        role="button"
        tabindex="0"
        @click=${this._edit}
        @keydown=${this._onEmptyKeydown}
      >
        ${this.placeholder}
      </div>`;
    }
    return html`<sema-markdown
      testid="markdown-rendered"
      .value=${this.value}
      @click=${this._edit}
    ></sema-markdown>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-editable-markdown': SemaEditableMarkdown;
  }
}
customElements.define('sema-editable-markdown', SemaEditableMarkdown);
