import { html, css, unsafeCSS } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';
import scrollbarStyles from '../styles/scrollbar.css?inline';

/**
 * `<sema-scroll-area>` — a scroll container with the shared themed thin scrollbar.
 *
 * Consolidates the `scrollbar-width: thin; scrollbar-color: …` styling that was
 * copy-pasted across surfaces. Give it a bounded size (height / max-height) and it
 * scrolls its slotted content; `orientation` picks the axis.
 *
 * ```html
 * <sema-scroll-area style="max-height: 192px;">…long content…</sema-scroll-area>
 * ```
 */
export class SemaScrollArea extends SemaElement {
  static styles = [
    SemaElement.base,
    unsafeCSS(scrollbarStyles),
    css`
      /* Flex sizes the viewport to the host's exact content box. The previous
         max-height:inherit approach overflowed the host whenever it had
         padding/border under border-box (inherit copies the full max-height,
         ignoring the insets). */
      :host {
        display: flex;
        flex-direction: column;
        min-height: 0;
      }
      .viewport {
        flex: 1 1 auto;
        min-height: 0;
        min-width: 0;
      }
      :host([orientation='vertical']) .viewport {
        overflow-y: auto;
        overflow-x: hidden;
      }
      :host([orientation='horizontal']) .viewport {
        overflow-x: auto;
        overflow-y: hidden;
      }
      :host([orientation='both']) .viewport {
        overflow: auto;
      }
    `,
  ];

  /** Which axis scrolls. */
  @property({ reflect: true }) orientation: 'vertical' | 'horizontal' | 'both' = 'vertical';

  render() {
    return html`<div class="viewport sema-scroll" part="viewport" tabindex="0">
      <slot></slot>
    </div>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-scroll-area': SemaScrollArea;
  }
}
customElements.define('sema-scroll-area', SemaScrollArea);
