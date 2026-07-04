import { html, css } from 'lit';
import { property } from 'lit/decorators.js';
import { SemaElement } from '../internal/sema-element.js';

const FONT_URL = 'https://fonts.googleapis.com/css2?family=Cormorant:ital,wght@0,300;0,400;0,500;0,600;1,400&family=JetBrains+Mono:wght@400;500&display=swap';

let _fontsLoaded = false;

// Inline body styles every _applyBody() call touches, in CSS-property form.
const BODY_STYLE_PROPS = [
  'margin',
  'padding',
  'background',
  'color',
  'font-family',
  'font-size',
  'line-height',
  '-webkit-font-smoothing',
  'height',
  'min-height',
  'overflow',
  'display',
  'flex-direction',
];

// Body styles are shared global state: one snapshot for the first mounted
// page, restored only when the last page unmounts (per-instance snapshots
// corrupt under multiple pages).
let _pageMounts = 0;
let _bodyStyleSnapshot: Map<string, string> | null = null;

function injectFontLink() {
  if (_fontsLoaded) return;
  if (document.querySelector(`link[href="${FONT_URL}"]`)) {
    _fontsLoaded = true;
    return;
  }
  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.href = FONT_URL;
  document.head.appendChild(link);
  _fontsLoaded = true;
}

export class SemaPage extends SemaElement {
  static styles = [
    SemaElement.base,
    css`
      :host {
        display: contents;
        --bg: #0c0c0c;
        --bg-elevated: #141414;
        --bg-editor: #0a0a0a;
        --bg-output: #080808;
        --bg-toolbar: #111;
        --gold: #c8a855;
        --gold-dim: rgba(200, 168, 85, 0.5);
        --gold-glow: rgba(200, 168, 85, 0.08);
        --gold-soft: rgba(200, 168, 85, 0.14);
        --text-primary: #d8d0c0;
        --text-secondary: #a09888;
        --text-tertiary: #5a5448;
        --success: #6a9955;
        --error: #c85555;
        --error-bg: rgba(200, 85, 85, 0.06);
        --border: #1e1e1e;
        --border-focus: #333;
        --tooltip-bg: #1a1a1a;
        --serif: 'Cormorant', Georgia, serif;
        --mono: 'JetBrains Mono', monospace;
      }
    `,
  ];

  @property({ type: Boolean, reflect: true, attribute: 'full-height' }) fullHeight = false;
  @property({ type: Boolean, reflect: true }) flex = false;

  connectedCallback() {
    super.connectedCallback();
    injectFontLink();
    this._ensureMeta('viewport', 'width=device-width, initial-scale=1');
    this._ensureMeta('theme-color', '#c8a855');
    if (_pageMounts === 0) {
      _bodyStyleSnapshot = new Map(
        BODY_STYLE_PROPS.map((prop) => [prop, document.body.style.getPropertyValue(prop)]),
      );
    }
    _pageMounts++;
    this._applyBody();
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    _pageMounts--;
    if (_pageMounts === 0 && _bodyStyleSnapshot) {
      for (const [prop, value] of _bodyStyleSnapshot) {
        if (value) document.body.style.setProperty(prop, value);
        else document.body.style.removeProperty(prop);
      }
      _bodyStyleSnapshot = null;
    }
  }

  updated(changed: Map<string, unknown>) {
    // Lit runs updates while detached; only a connected page may own body styles.
    if (this.isConnected && (changed.has('fullHeight') || changed.has('flex'))) {
      this._applyBody();
    }
  }

  private _ensureMeta(name: string, content: string) {
    let meta = document.querySelector(`meta[name="${name}"]`);
    if (!meta) {
      meta = document.createElement('meta');
      meta.setAttribute('name', name);
      document.head.appendChild(meta);
    }
    meta.setAttribute('content', content);
  }

  private _applyBody() {
    const body = document.body;
    body.style.margin = '0';
    body.style.padding = '0';
    body.style.background = 'var(--bg, #0c0c0c)';
    body.style.color = 'var(--text-secondary, #a09888)';
    body.style.fontFamily = 'var(--serif)';
    body.style.fontSize = '18px';
    body.style.lineHeight = '1.7';
    body.style.setProperty('-webkit-font-smoothing', 'antialiased');

    if (this.fullHeight) {
      body.style.height = '100vh';
      body.style.minHeight = '';
      body.style.overflow = 'hidden';
    } else {
      body.style.height = '';
      body.style.minHeight = '100vh';
      body.style.overflow = '';
    }
    if (this.flex) {
      body.style.display = 'flex';
      body.style.flexDirection = 'column';
    } else {
      body.style.display = '';
      body.style.flexDirection = '';
    }
  }

  render() {
    return html`<slot></slot>`;
  }
}

declare global { interface HTMLElementTagNameMap { 'sema-page': SemaPage } }
customElements.define('sema-page', SemaPage);
