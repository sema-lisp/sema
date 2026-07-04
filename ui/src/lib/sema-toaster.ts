import { html, css } from 'lit';
import { property, state } from 'lit/decorators.js';
import { repeat } from 'lit/directives/repeat.js';
import { SemaElement } from '../internal/sema-element.js';
import './sema-toast.js';
import type { ToastVariant } from './sema-toast.js';

export type ToasterPosition =
  | 'top-left'
  | 'top-center'
  | 'top-right'
  | 'bottom-left'
  | 'bottom-center'
  | 'bottom-right';

export interface ToastOptions {
  variant?: ToastVariant;
  /** Auto-dismiss after N ms. `null`/`0` = sticky (manual dismiss only). Default 4000. */
  duration?: number | null;
  dismissible?: boolean;
}

export interface ToastHandle {
  readonly id: number;
  dismiss(): void;
  update(message: string, opts?: ToastOptions): void;
}

interface ToastItem {
  id: number;
  message: string;
  variant: ToastVariant;
  duration: number | null;
  dismissible: boolean;
}

interface Timer {
  handle: number;
  remaining: number;
  startedAt: number;
}

let _nextId = 0;

/**
 * `<sema-toaster>` — the toast region/coordinator. Holds the stack, positions it in a
 * corner, auto-dismisses (pausing on hover), and caps visible toasts. Usually created
 * automatically by the imperative `toast()` API, but can be placed manually to control
 * `position` / `max-visible`.
 */
export class SemaToaster extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        position: fixed;
        z-index: 1000;
        pointer-events: none;
      }
      :host([position^='top']) {
        top: var(--space-md, 16px);
      }
      :host([position^='bottom']) {
        bottom: var(--space-md, 16px);
      }
      :host([position$='left']) {
        left: var(--space-md, 16px);
      }
      :host([position$='right']) {
        right: var(--space-md, 16px);
      }
      :host([position$='center']) {
        left: 50%;
        transform: translateX(-50%);
      }
      .region {
        display: flex;
        flex-direction: column;
        gap: var(--space-sm, 8px);
        width: max-content;
        max-width: min(384px, 90vw);
        pointer-events: auto;
      }
      :host([position^='bottom']) .region {
        flex-direction: column-reverse;
      }
    `,
  ];

  @property({ reflect: true }) position: ToasterPosition = 'top-right';
  @property({ type: Number, attribute: 'max-visible' }) maxVisible = 5;

  @state() private _toasts: ToastItem[] = [];
  private _timers = new Map<number, Timer>();
  private _paused = false;

  disconnectedCallback() {
    super.disconnectedCallback();
    for (const t of this._timers.values()) clearTimeout(t.handle);
    this._timers.clear();
  }

  /** Show a toast; returns a handle to dismiss/update it. */
  show(message: string, opts: ToastOptions = {}): ToastHandle {
    const id = ++_nextId;
    const item: ToastItem = {
      id,
      message,
      variant: opts.variant ?? 'info',
      duration: opts.duration === undefined ? 4000 : opts.duration,
      dismissible: opts.dismissible ?? true,
    };
    this._toasts = [item, ...this._toasts]; // newest first
    while (this._toasts.length > this.maxVisible) {
      this.dismiss(this._toasts[this._toasts.length - 1].id);
    }
    this._arm(item);
    return {
      id,
      dismiss: () => this.dismiss(id),
      update: (m, o) => this.updateToast(id, m, o),
    };
  }

  updateToast(id: number, message: string, opts: ToastOptions = {}) {
    let updated: ToastItem | undefined;
    this._toasts = this._toasts.map((t) => {
      if (t.id !== id) return t;
      updated = {
        ...t,
        message,
        variant: opts.variant ?? t.variant,
        dismissible: opts.dismissible ?? t.dismissible,
        duration: opts.duration === undefined ? t.duration : opts.duration,
      };
      return updated;
    });
    if (updated) {
      this._disarm(id);
      this._arm(updated);
    }
  }

  dismiss(id: number) {
    this._disarm(id);
    this._toasts = this._toasts.filter((t) => t.id !== id);
  }

  dismissAll() {
    for (const t of [...this._toasts]) this.dismiss(t.id);
  }

  private _arm(item: ToastItem) {
    if (!item.duration || item.duration <= 0) return;
    if (this._paused) {
      // armed while hovered: record the remaining time only; _resume starts the real timer
      this._timers.set(item.id, { handle: 0, remaining: item.duration, startedAt: performance.now() });
      return;
    }
    const handle = window.setTimeout(() => this.dismiss(item.id), item.duration);
    this._timers.set(item.id, { handle, remaining: item.duration, startedAt: performance.now() });
  }
  private _disarm(id: number) {
    const t = this._timers.get(id);
    if (t) {
      clearTimeout(t.handle);
      this._timers.delete(id);
    }
  }
  private _pause = () => {
    if (this._paused) return;
    this._paused = true;
    for (const t of this._timers.values()) {
      clearTimeout(t.handle);
      t.remaining = Math.max(0, t.remaining - (performance.now() - t.startedAt));
    }
  };
  private _resume = () => {
    if (!this._paused) return;
    this._paused = false;
    for (const [id, t] of this._timers) {
      clearTimeout(t.handle);
      t.startedAt = performance.now();
      t.handle = window.setTimeout(() => this.dismiss(id), t.remaining);
    }
  };

  render() {
    return html`<div
      class="region"
      role="region"
      aria-live="polite"
      aria-label="Notifications"
      @pointerenter=${this._pause}
      @pointerleave=${this._resume}
    >
      ${repeat(
        this._toasts,
        (t) => t.id,
        (t) =>
          html`<sema-toast
            .variant=${t.variant}
            ?dismissible=${t.dismissible}
            @sema-dismiss=${() => this.dismiss(t.id)}
            >${t.message}</sema-toast
          >`,
      )}
    </div>`;
  }
}

declare global {
  interface HTMLElementTagNameMap {
    'sema-toaster': SemaToaster;
  }
}
customElements.define('sema-toaster', SemaToaster);
