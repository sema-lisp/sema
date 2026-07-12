var Zl = Object.defineProperty;
var Xl = Object.getPrototypeOf;
var Jl = Reflect.get;
var Vo = (n) => {
  throw TypeError(n);
};
var Yl = (n, e, t) => e in n ? Zl(n, e, { enumerable: !0, configurable: !0, writable: !0, value: t }) : n[e] = t;
var g = (n, e, t) => Yl(n, typeof e != "symbol" ? e + "" : e, t), Os = (n, e, t) => e.has(n) || Vo("Cannot " + t);
var V = (n, e, t) => (Os(n, e, "read from private field"), t ? t.call(n) : e.get(n)), Pe = (n, e, t) => e.has(n) ? Vo("Cannot add the same private member more than once") : e instanceof WeakSet ? e.add(n) : e.set(n, t), he = (n, e, t, s) => (Os(n, e, "write to private field"), s ? s.call(n, t) : e.set(n, t), t), yt = (n, e, t) => (Os(n, e, "access private method"), t);
var Ko = (n, e, t) => Jl(Xl(n), t, e);
/**
 * @license
 * Copyright 2019 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const Fn = globalThis, Ir = Fn.ShadowRoot && (Fn.ShadyCSS === void 0 || Fn.ShadyCSS.nativeShadow) && "adoptedStyleSheets" in Document.prototype && "replace" in CSSStyleSheet.prototype, Pr = Symbol(), Zo = /* @__PURE__ */ new WeakMap();
let aa = class {
  constructor(e, t, s) {
    if (this._$cssResult$ = !0, s !== Pr) throw Error("CSSResult is not constructable. Use `unsafeCSS` or `css` instead.");
    this.cssText = e, this.t = t;
  }
  get styleSheet() {
    let e = this.o;
    const t = this.t;
    if (Ir && e === void 0) {
      const s = t !== void 0 && t.length === 1;
      s && (e = Zo.get(t)), e === void 0 && ((this.o = e = new CSSStyleSheet()).replaceSync(this.cssText), s && Zo.set(t, e));
    }
    return e;
  }
  toString() {
    return this.cssText;
  }
};
const q = (n) => new aa(typeof n == "string" ? n : n + "", void 0, Pr), I = (n, ...e) => {
  const t = n.length === 1 ? n[0] : e.reduce((s, r, o) => s + ((i) => {
    if (i._$cssResult$ === !0) return i.cssText;
    if (typeof i == "number") return i;
    throw Error("Value passed to 'css' function must be a 'css' function result: " + i + ". Use 'unsafeCSS' to pass non-literal values, but take care to ensure page security.");
  })(r) + n[o + 1], n[0]);
  return new aa(t, n, Pr);
}, ec = (n, e) => {
  if (Ir) n.adoptedStyleSheets = e.map((t) => t instanceof CSSStyleSheet ? t : t.styleSheet);
  else for (const t of e) {
    const s = document.createElement("style"), r = Fn.litNonce;
    r !== void 0 && s.setAttribute("nonce", r), s.textContent = t.cssText, n.appendChild(s);
  }
}, Xo = Ir ? (n) => n : (n) => n instanceof CSSStyleSheet ? ((e) => {
  let t = "";
  for (const s of e.cssRules) t += s.cssText;
  return q(t);
})(n) : n;
/**
 * @license
 * Copyright 2017 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const { is: tc, defineProperty: nc, getOwnPropertyDescriptor: sc, getOwnPropertyNames: rc, getOwnPropertySymbols: oc, getPrototypeOf: ic } = Object, De = globalThis, Jo = De.trustedTypes, ac = Jo ? Jo.emptyScript : "", zs = De.reactiveElementPolyfillSupport, Kt = (n, e) => n, qn = { toAttribute(n, e) {
  switch (e) {
    case Boolean:
      n = n ? ac : null;
      break;
    case Object:
    case Array:
      n = n == null ? n : JSON.stringify(n);
  }
  return n;
}, fromAttribute(n, e) {
  let t = n;
  switch (e) {
    case Boolean:
      t = n !== null;
      break;
    case Number:
      t = n === null ? null : Number(n);
      break;
    case Object:
    case Array:
      try {
        t = JSON.parse(n);
      } catch {
        t = null;
      }
  }
  return t;
} }, vs = (n, e) => !tc(n, e), Yo = { attribute: !0, type: String, converter: qn, reflect: !1, useDefault: !1, hasChanged: vs };
Symbol.metadata ?? (Symbol.metadata = Symbol("metadata")), De.litPropertyMetadata ?? (De.litPropertyMetadata = /* @__PURE__ */ new WeakMap());
let wt = class extends HTMLElement {
  static addInitializer(e) {
    this._$Ei(), (this.l ?? (this.l = [])).push(e);
  }
  static get observedAttributes() {
    return this.finalize(), this._$Eh && [...this._$Eh.keys()];
  }
  static createProperty(e, t = Yo) {
    if (t.state && (t.attribute = !1), this._$Ei(), this.prototype.hasOwnProperty(e) && ((t = Object.create(t)).wrapped = !0), this.elementProperties.set(e, t), !t.noAccessor) {
      const s = Symbol(), r = this.getPropertyDescriptor(e, s, t);
      r !== void 0 && nc(this.prototype, e, r);
    }
  }
  static getPropertyDescriptor(e, t, s) {
    const { get: r, set: o } = sc(this.prototype, e) ?? { get() {
      return this[t];
    }, set(i) {
      this[t] = i;
    } };
    return { get: r, set(i) {
      const a = r == null ? void 0 : r.call(this);
      o == null || o.call(this, i), this.requestUpdate(e, a, s);
    }, configurable: !0, enumerable: !0 };
  }
  static getPropertyOptions(e) {
    return this.elementProperties.get(e) ?? Yo;
  }
  static _$Ei() {
    if (this.hasOwnProperty(Kt("elementProperties"))) return;
    const e = ic(this);
    e.finalize(), e.l !== void 0 && (this.l = [...e.l]), this.elementProperties = new Map(e.elementProperties);
  }
  static finalize() {
    if (this.hasOwnProperty(Kt("finalized"))) return;
    if (this.finalized = !0, this._$Ei(), this.hasOwnProperty(Kt("properties"))) {
      const t = this.properties, s = [...rc(t), ...oc(t)];
      for (const r of s) this.createProperty(r, t[r]);
    }
    const e = this[Symbol.metadata];
    if (e !== null) {
      const t = litPropertyMetadata.get(e);
      if (t !== void 0) for (const [s, r] of t) this.elementProperties.set(s, r);
    }
    this._$Eh = /* @__PURE__ */ new Map();
    for (const [t, s] of this.elementProperties) {
      const r = this._$Eu(t, s);
      r !== void 0 && this._$Eh.set(r, t);
    }
    this.elementStyles = this.finalizeStyles(this.styles);
  }
  static finalizeStyles(e) {
    const t = [];
    if (Array.isArray(e)) {
      const s = new Set(e.flat(1 / 0).reverse());
      for (const r of s) t.unshift(Xo(r));
    } else e !== void 0 && t.push(Xo(e));
    return t;
  }
  static _$Eu(e, t) {
    const s = t.attribute;
    return s === !1 ? void 0 : typeof s == "string" ? s : typeof e == "string" ? e.toLowerCase() : void 0;
  }
  constructor() {
    super(), this._$Ep = void 0, this.isUpdatePending = !1, this.hasUpdated = !1, this._$Em = null, this._$Ev();
  }
  _$Ev() {
    var e;
    this._$ES = new Promise((t) => this.enableUpdating = t), this._$AL = /* @__PURE__ */ new Map(), this._$E_(), this.requestUpdate(), (e = this.constructor.l) == null || e.forEach((t) => t(this));
  }
  addController(e) {
    var t;
    (this._$EO ?? (this._$EO = /* @__PURE__ */ new Set())).add(e), this.renderRoot !== void 0 && this.isConnected && ((t = e.hostConnected) == null || t.call(e));
  }
  removeController(e) {
    var t;
    (t = this._$EO) == null || t.delete(e);
  }
  _$E_() {
    const e = /* @__PURE__ */ new Map(), t = this.constructor.elementProperties;
    for (const s of t.keys()) this.hasOwnProperty(s) && (e.set(s, this[s]), delete this[s]);
    e.size > 0 && (this._$Ep = e);
  }
  createRenderRoot() {
    const e = this.shadowRoot ?? this.attachShadow(this.constructor.shadowRootOptions);
    return ec(e, this.constructor.elementStyles), e;
  }
  connectedCallback() {
    var e;
    this.renderRoot ?? (this.renderRoot = this.createRenderRoot()), this.enableUpdating(!0), (e = this._$EO) == null || e.forEach((t) => {
      var s;
      return (s = t.hostConnected) == null ? void 0 : s.call(t);
    });
  }
  enableUpdating(e) {
  }
  disconnectedCallback() {
    var e;
    (e = this._$EO) == null || e.forEach((t) => {
      var s;
      return (s = t.hostDisconnected) == null ? void 0 : s.call(t);
    });
  }
  attributeChangedCallback(e, t, s) {
    this._$AK(e, s);
  }
  _$ET(e, t) {
    var o;
    const s = this.constructor.elementProperties.get(e), r = this.constructor._$Eu(e, s);
    if (r !== void 0 && s.reflect === !0) {
      const i = (((o = s.converter) == null ? void 0 : o.toAttribute) !== void 0 ? s.converter : qn).toAttribute(t, s.type);
      this._$Em = e, i == null ? this.removeAttribute(r) : this.setAttribute(r, i), this._$Em = null;
    }
  }
  _$AK(e, t) {
    var o, i;
    const s = this.constructor, r = s._$Eh.get(e);
    if (r !== void 0 && this._$Em !== r) {
      const a = s.getPropertyOptions(r), l = typeof a.converter == "function" ? { fromAttribute: a.converter } : ((o = a.converter) == null ? void 0 : o.fromAttribute) !== void 0 ? a.converter : qn;
      this._$Em = r;
      const c = l.fromAttribute(t, a.type);
      this[r] = c ?? ((i = this._$Ej) == null ? void 0 : i.get(r)) ?? c, this._$Em = null;
    }
  }
  requestUpdate(e, t, s, r = !1, o) {
    var i;
    if (e !== void 0) {
      const a = this.constructor;
      if (r === !1 && (o = this[e]), s ?? (s = a.getPropertyOptions(e)), !((s.hasChanged ?? vs)(o, t) || s.useDefault && s.reflect && o === ((i = this._$Ej) == null ? void 0 : i.get(e)) && !this.hasAttribute(a._$Eu(e, s)))) return;
      this.C(e, t, s);
    }
    this.isUpdatePending === !1 && (this._$ES = this._$EP());
  }
  C(e, t, { useDefault: s, reflect: r, wrapped: o }, i) {
    s && !(this._$Ej ?? (this._$Ej = /* @__PURE__ */ new Map())).has(e) && (this._$Ej.set(e, i ?? t ?? this[e]), o !== !0 || i !== void 0) || (this._$AL.has(e) || (this.hasUpdated || s || (t = void 0), this._$AL.set(e, t)), r === !0 && this._$Em !== e && (this._$Eq ?? (this._$Eq = /* @__PURE__ */ new Set())).add(e));
  }
  async _$EP() {
    this.isUpdatePending = !0;
    try {
      await this._$ES;
    } catch (t) {
      Promise.reject(t);
    }
    const e = this.scheduleUpdate();
    return e != null && await e, !this.isUpdatePending;
  }
  scheduleUpdate() {
    return this.performUpdate();
  }
  performUpdate() {
    var s;
    if (!this.isUpdatePending) return;
    if (!this.hasUpdated) {
      if (this.renderRoot ?? (this.renderRoot = this.createRenderRoot()), this._$Ep) {
        for (const [o, i] of this._$Ep) this[o] = i;
        this._$Ep = void 0;
      }
      const r = this.constructor.elementProperties;
      if (r.size > 0) for (const [o, i] of r) {
        const { wrapped: a } = i, l = this[o];
        a !== !0 || this._$AL.has(o) || l === void 0 || this.C(o, void 0, i, l);
      }
    }
    let e = !1;
    const t = this._$AL;
    try {
      e = this.shouldUpdate(t), e ? (this.willUpdate(t), (s = this._$EO) == null || s.forEach((r) => {
        var o;
        return (o = r.hostUpdate) == null ? void 0 : o.call(r);
      }), this.update(t)) : this._$EM();
    } catch (r) {
      throw e = !1, this._$EM(), r;
    }
    e && this._$AE(t);
  }
  willUpdate(e) {
  }
  _$AE(e) {
    var t;
    (t = this._$EO) == null || t.forEach((s) => {
      var r;
      return (r = s.hostUpdated) == null ? void 0 : r.call(s);
    }), this.hasUpdated || (this.hasUpdated = !0, this.firstUpdated(e)), this.updated(e);
  }
  _$EM() {
    this._$AL = /* @__PURE__ */ new Map(), this.isUpdatePending = !1;
  }
  get updateComplete() {
    return this.getUpdateComplete();
  }
  getUpdateComplete() {
    return this._$ES;
  }
  shouldUpdate(e) {
    return !0;
  }
  update(e) {
    this._$Eq && (this._$Eq = this._$Eq.forEach((t) => this._$ET(t, this[t]))), this._$EM();
  }
  updated(e) {
  }
  firstUpdated(e) {
  }
};
wt.elementStyles = [], wt.shadowRootOptions = { mode: "open" }, wt[Kt("elementProperties")] = /* @__PURE__ */ new Map(), wt[Kt("finalized")] = /* @__PURE__ */ new Map(), zs == null || zs({ ReactiveElement: wt }), (De.reactiveElementVersions ?? (De.reactiveElementVersions = [])).push("2.1.2");
/**
 * @license
 * Copyright 2017 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const Zt = globalThis, ei = (n) => n, Hn = Zt.trustedTypes, ti = Hn ? Hn.createPolicy("lit-html", { createHTML: (n) => n }) : void 0, la = "$lit$", Oe = `lit$${Math.random().toFixed(9).slice(2)}$`, ca = "?" + Oe, lc = `<${ca}>`, at = document, tn = () => at.createComment(""), nn = (n) => n === null || typeof n != "object" && typeof n != "function", Lr = Array.isArray, cc = (n) => Lr(n) || typeof (n == null ? void 0 : n[Symbol.iterator]) == "function", Bs = `[ 	
\f\r]`, Ut = /<(?:(!--|\/[^a-zA-Z])|(\/?[a-zA-Z][^>\s]*)|(\/?$))/g, ni = /-->/g, si = />/g, Ke = RegExp(`>|${Bs}(?:([^\\s"'>=/]+)(${Bs}*=${Bs}*(?:[^ 	
\f\r"'\`<>=]|("|')|))|$)`, "g"), ri = /'/g, oi = /"/g, ua = /^(?:script|style|textarea|title)$/i, uc = (n) => (e, ...t) => ({ _$litType$: n, strings: e, values: t }), w = uc(1), le = Symbol.for("lit-noChange"), R = Symbol.for("lit-nothing"), ii = /* @__PURE__ */ new WeakMap(), Ye = at.createTreeWalker(at, 129);
function ha(n, e) {
  if (!Lr(n) || !n.hasOwnProperty("raw")) throw Error("invalid template strings array");
  return ti !== void 0 ? ti.createHTML(e) : e;
}
const hc = (n, e) => {
  const t = n.length - 1, s = [];
  let r, o = e === 2 ? "<svg>" : e === 3 ? "<math>" : "", i = Ut;
  for (let a = 0; a < t; a++) {
    const l = n[a];
    let c, h, u = -1, p = 0;
    for (; p < l.length && (i.lastIndex = p, h = i.exec(l), h !== null); ) p = i.lastIndex, i === Ut ? h[1] === "!--" ? i = ni : h[1] !== void 0 ? i = si : h[2] !== void 0 ? (ua.test(h[2]) && (r = RegExp("</" + h[2], "g")), i = Ke) : h[3] !== void 0 && (i = Ke) : i === Ke ? h[0] === ">" ? (i = r ?? Ut, u = -1) : h[1] === void 0 ? u = -2 : (u = i.lastIndex - h[2].length, c = h[1], i = h[3] === void 0 ? Ke : h[3] === '"' ? oi : ri) : i === oi || i === ri ? i = Ke : i === ni || i === si ? i = Ut : (i = Ke, r = void 0);
    const d = i === Ke && n[a + 1].startsWith("/>") ? " " : "";
    o += i === Ut ? l + lc : u >= 0 ? (s.push(c), l.slice(0, u) + la + l.slice(u) + Oe + d) : l + Oe + (u === -2 ? a : d);
  }
  return [ha(n, o + (n[t] || "<?>") + (e === 2 ? "</svg>" : e === 3 ? "</math>" : "")), s];
};
let ar = class pa {
  constructor({ strings: e, _$litType$: t }, s) {
    let r;
    this.parts = [];
    let o = 0, i = 0;
    const a = e.length - 1, l = this.parts, [c, h] = hc(e, t);
    if (this.el = pa.createElement(c, s), Ye.currentNode = this.el.content, t === 2 || t === 3) {
      const u = this.el.content.firstChild;
      u.replaceWith(...u.childNodes);
    }
    for (; (r = Ye.nextNode()) !== null && l.length < a; ) {
      if (r.nodeType === 1) {
        if (r.hasAttributes()) for (const u of r.getAttributeNames()) if (u.endsWith(la)) {
          const p = h[i++], d = r.getAttribute(u).split(Oe), f = /([.?@])?(.*)/.exec(p);
          l.push({ type: 1, index: o, name: f[2], strings: d, ctor: f[1] === "." ? dc : f[1] === "?" ? fc : f[1] === "@" ? gc : ws }), r.removeAttribute(u);
        } else u.startsWith(Oe) && (l.push({ type: 6, index: o }), r.removeAttribute(u));
        if (ua.test(r.tagName)) {
          const u = r.textContent.split(Oe), p = u.length - 1;
          if (p > 0) {
            r.textContent = Hn ? Hn.emptyScript : "";
            for (let d = 0; d < p; d++) r.append(u[d], tn()), Ye.nextNode(), l.push({ type: 2, index: ++o });
            r.append(u[p], tn());
          }
        }
      } else if (r.nodeType === 8) if (r.data === ca) l.push({ type: 2, index: o });
      else {
        let u = -1;
        for (; (u = r.data.indexOf(Oe, u + 1)) !== -1; ) l.push({ type: 7, index: o }), u += Oe.length - 1;
      }
      o++;
    }
  }
  static createElement(e, t) {
    const s = at.createElement("template");
    return s.innerHTML = e, s;
  }
};
function St(n, e, t = n, s) {
  var i, a;
  if (e === le) return e;
  let r = s !== void 0 ? (i = t._$Co) == null ? void 0 : i[s] : t._$Cl;
  const o = nn(e) ? void 0 : e._$litDirective$;
  return (r == null ? void 0 : r.constructor) !== o && ((a = r == null ? void 0 : r._$AO) == null || a.call(r, !1), o === void 0 ? r = void 0 : (r = new o(n), r._$AT(n, t, s)), s !== void 0 ? (t._$Co ?? (t._$Co = []))[s] = r : t._$Cl = r), r !== void 0 && (e = St(n, r._$AS(n, e.values), r, s)), e;
}
let pc = class {
  constructor(e, t) {
    this._$AV = [], this._$AN = void 0, this._$AD = e, this._$AM = t;
  }
  get parentNode() {
    return this._$AM.parentNode;
  }
  get _$AU() {
    return this._$AM._$AU;
  }
  u(e) {
    const { el: { content: t }, parts: s } = this._$AD, r = ((e == null ? void 0 : e.creationScope) ?? at).importNode(t, !0);
    Ye.currentNode = r;
    let o = Ye.nextNode(), i = 0, a = 0, l = s[0];
    for (; l !== void 0; ) {
      if (i === l.index) {
        let c;
        l.type === 2 ? c = new _s(o, o.nextSibling, this, e) : l.type === 1 ? c = new l.ctor(o, l.name, l.strings, this, e) : l.type === 6 && (c = new mc(o, this, e)), this._$AV.push(c), l = s[++a];
      }
      i !== (l == null ? void 0 : l.index) && (o = Ye.nextNode(), i++);
    }
    return Ye.currentNode = at, r;
  }
  p(e) {
    let t = 0;
    for (const s of this._$AV) s !== void 0 && (s.strings !== void 0 ? (s._$AI(e, s, t), t += s.strings.length - 2) : s._$AI(e[t])), t++;
  }
}, _s = class da {
  get _$AU() {
    var e;
    return ((e = this._$AM) == null ? void 0 : e._$AU) ?? this._$Cv;
  }
  constructor(e, t, s, r) {
    this.type = 2, this._$AH = R, this._$AN = void 0, this._$AA = e, this._$AB = t, this._$AM = s, this.options = r, this._$Cv = (r == null ? void 0 : r.isConnected) ?? !0;
  }
  get parentNode() {
    let e = this._$AA.parentNode;
    const t = this._$AM;
    return t !== void 0 && (e == null ? void 0 : e.nodeType) === 11 && (e = t.parentNode), e;
  }
  get startNode() {
    return this._$AA;
  }
  get endNode() {
    return this._$AB;
  }
  _$AI(e, t = this) {
    e = St(this, e, t), nn(e) ? e === R || e == null || e === "" ? (this._$AH !== R && this._$AR(), this._$AH = R) : e !== this._$AH && e !== le && this._(e) : e._$litType$ !== void 0 ? this.$(e) : e.nodeType !== void 0 ? this.T(e) : cc(e) ? this.k(e) : this._(e);
  }
  O(e) {
    return this._$AA.parentNode.insertBefore(e, this._$AB);
  }
  T(e) {
    this._$AH !== e && (this._$AR(), this._$AH = this.O(e));
  }
  _(e) {
    this._$AH !== R && nn(this._$AH) ? this._$AA.nextSibling.data = e : this.T(at.createTextNode(e)), this._$AH = e;
  }
  $(e) {
    var o;
    const { values: t, _$litType$: s } = e, r = typeof s == "number" ? this._$AC(e) : (s.el === void 0 && (s.el = ar.createElement(ha(s.h, s.h[0]), this.options)), s);
    if (((o = this._$AH) == null ? void 0 : o._$AD) === r) this._$AH.p(t);
    else {
      const i = new pc(r, this), a = i.u(this.options);
      i.p(t), this.T(a), this._$AH = i;
    }
  }
  _$AC(e) {
    let t = ii.get(e.strings);
    return t === void 0 && ii.set(e.strings, t = new ar(e)), t;
  }
  k(e) {
    Lr(this._$AH) || (this._$AH = [], this._$AR());
    const t = this._$AH;
    let s, r = 0;
    for (const o of e) r === t.length ? t.push(s = new da(this.O(tn()), this.O(tn()), this, this.options)) : s = t[r], s._$AI(o), r++;
    r < t.length && (this._$AR(s && s._$AB.nextSibling, r), t.length = r);
  }
  _$AR(e = this._$AA.nextSibling, t) {
    var s;
    for ((s = this._$AP) == null ? void 0 : s.call(this, !1, !0, t); e !== this._$AB; ) {
      const r = ei(e).nextSibling;
      ei(e).remove(), e = r;
    }
  }
  setConnected(e) {
    var t;
    this._$AM === void 0 && (this._$Cv = e, (t = this._$AP) == null || t.call(this, e));
  }
}, ws = class {
  get tagName() {
    return this.element.tagName;
  }
  get _$AU() {
    return this._$AM._$AU;
  }
  constructor(e, t, s, r, o) {
    this.type = 1, this._$AH = R, this._$AN = void 0, this.element = e, this.name = t, this._$AM = r, this.options = o, s.length > 2 || s[0] !== "" || s[1] !== "" ? (this._$AH = Array(s.length - 1).fill(new String()), this.strings = s) : this._$AH = R;
  }
  _$AI(e, t = this, s, r) {
    const o = this.strings;
    let i = !1;
    if (o === void 0) e = St(this, e, t, 0), i = !nn(e) || e !== this._$AH && e !== le, i && (this._$AH = e);
    else {
      const a = e;
      let l, c;
      for (e = o[0], l = 0; l < o.length - 1; l++) c = St(this, a[s + l], t, l), c === le && (c = this._$AH[l]), i || (i = !nn(c) || c !== this._$AH[l]), c === R ? e = R : e !== R && (e += (c ?? "") + o[l + 1]), this._$AH[l] = c;
    }
    i && !r && this.j(e);
  }
  j(e) {
    e === R ? this.element.removeAttribute(this.name) : this.element.setAttribute(this.name, e ?? "");
  }
}, dc = class extends ws {
  constructor() {
    super(...arguments), this.type = 3;
  }
  j(e) {
    this.element[this.name] = e === R ? void 0 : e;
  }
}, fc = class extends ws {
  constructor() {
    super(...arguments), this.type = 4;
  }
  j(e) {
    this.element.toggleAttribute(this.name, !!e && e !== R);
  }
}, gc = class extends ws {
  constructor(e, t, s, r, o) {
    super(e, t, s, r, o), this.type = 5;
  }
  _$AI(e, t = this) {
    if ((e = St(this, e, t, 0) ?? R) === le) return;
    const s = this._$AH, r = e === R && s !== R || e.capture !== s.capture || e.once !== s.once || e.passive !== s.passive, o = e !== R && (s === R || r);
    r && this.element.removeEventListener(this.name, this, s), o && this.element.addEventListener(this.name, this, e), this._$AH = e;
  }
  handleEvent(e) {
    var t;
    typeof this._$AH == "function" ? this._$AH.call(((t = this.options) == null ? void 0 : t.host) ?? this.element, e) : this._$AH.handleEvent(e);
  }
}, mc = class {
  constructor(e, t, s) {
    this.element = e, this.type = 6, this._$AN = void 0, this._$AM = t, this.options = s;
  }
  get _$AU() {
    return this._$AM._$AU;
  }
  _$AI(e) {
    St(this, e);
  }
};
const bc = { I: _s }, Ds = Zt.litHtmlPolyfillSupport;
Ds == null || Ds(ar, _s), (Zt.litHtmlVersions ?? (Zt.litHtmlVersions = [])).push("3.3.3");
const yc = (n, e, t) => {
  const s = (t == null ? void 0 : t.renderBefore) ?? e;
  let r = s._$litPart$;
  if (r === void 0) {
    const o = (t == null ? void 0 : t.renderBefore) ?? null;
    s._$litPart$ = r = new _s(e.insertBefore(tn(), o), o, void 0, t ?? {});
  }
  return r._$AI(n), r;
};
/**
 * @license
 * Copyright 2017 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const rt = globalThis;
let Ct = class extends wt {
  constructor() {
    super(...arguments), this.renderOptions = { host: this }, this._$Do = void 0;
  }
  createRenderRoot() {
    var t;
    const e = super.createRenderRoot();
    return (t = this.renderOptions).renderBefore ?? (t.renderBefore = e.firstChild), e;
  }
  update(e) {
    const t = this.render();
    this.hasUpdated || (this.renderOptions.isConnected = this.isConnected), super.update(e), this._$Do = yc(t, this.renderRoot, this.renderOptions);
  }
  connectedCallback() {
    var e;
    super.connectedCallback(), (e = this._$Do) == null || e.setConnected(!0);
  }
  disconnectedCallback() {
    var e;
    super.disconnectedCallback(), (e = this._$Do) == null || e.setConnected(!1);
  }
  render() {
    return le;
  }
};
var ia;
Ct._$litElement$ = !0, Ct.finalized = !0, (ia = rt.litElementHydrateSupport) == null || ia.call(rt, { LitElement: Ct });
const Fs = rt.litElementPolyfillSupport;
Fs == null || Fs({ LitElement: Ct });
(rt.litElementVersions ?? (rt.litElementVersions = [])).push("4.2.2");
/**
 * @license
 * Copyright 2017 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const vc = { attribute: !0, type: String, converter: qn, reflect: !1, hasChanged: vs }, _c = (n = vc, e, t) => {
  const { kind: s, metadata: r } = t;
  let o = globalThis.litPropertyMetadata.get(r);
  if (o === void 0 && globalThis.litPropertyMetadata.set(r, o = /* @__PURE__ */ new Map()), s === "setter" && ((n = Object.create(n)).wrapped = !0), o.set(t.name, n), s === "accessor") {
    const { name: i } = t;
    return { set(a) {
      const l = e.get.call(this);
      e.set.call(this, a), this.requestUpdate(i, l, n, !0, a);
    }, init(a) {
      return a !== void 0 && this.C(i, void 0, n, a), a;
    } };
  }
  if (s === "setter") {
    const { name: i } = t;
    return function(a) {
      const l = this[i];
      e.call(this, a), this.requestUpdate(i, l, n, !0, a);
    };
  }
  throw Error("Unsupported decorator location: " + s);
};
function m(n) {
  return (e, t) => typeof t == "object" ? _c(n, e, t) : ((s, r, o) => {
    const i = r.hasOwnProperty(o);
    return r.constructor.createProperty(o, s), i ? Object.getOwnPropertyDescriptor(r, o) : void 0;
  })(n, e, t);
}
/**
 * @license
 * Copyright 2017 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
function Qe(n) {
  return m({ ...n, state: !0, attribute: !1 });
}
/**
 * @license
 * Copyright 2017 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const fa = (n, e, t) => (t.configurable = !0, t.enumerable = !0, Reflect.decorate && typeof e != "object" && Object.defineProperty(n, e, t), t);
/**
 * @license
 * Copyright 2017 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
function ga(n, e) {
  return (t, s, r) => {
    const o = (i) => {
      var a;
      return ((a = i.renderRoot) == null ? void 0 : a.querySelector(n)) ?? null;
    };
    return fa(t, s, { get() {
      return o(this);
    } });
  };
}
/**
 * @license
 * Copyright 2021 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
function Nr(n) {
  return (e, t) => {
    const { slot: s, selector: r } = n ?? {}, o = "slot" + (s ? `[name=${s}]` : ":not([name])");
    return fa(e, t, { get() {
      var l;
      const i = (l = this.renderRoot) == null ? void 0 : l.querySelector(o), a = (i == null ? void 0 : i.assignedElements(n)) ?? [];
      return r === void 0 ? a : a.filter((c) => c.matches(r));
    } });
  };
}
const bo = class bo extends Ct {
};
bo.base = I`
    :host {
      box-sizing: border-box;
    }
    :host *,
    :host *::before,
    :host *::after {
      box-sizing: border-box;
    }
    @media (prefers-reduced-motion: reduce) {
      /* Near-zero (not zero) so animationend/transitionend still fire. */
      :host,
      :host *,
      :host *::before,
      :host *::after {
        animation-duration: 0.001ms !important;
        transition-duration: 0.001ms !important;
        animation-iteration-count: 1 !important;
        scroll-behavior: auto;
      }
    }
  `;
let C = bo;
var wc = Object.defineProperty, vn = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && wc(e, t, r), r;
};
const gs = class gs extends C {
  constructor() {
    super(...arguments), this.variant = "primary", this.size = "md", this.disabled = !1, this.danger = !1;
  }
  render() {
    const e = this.getAttribute("aria-label");
    return w`
      <button class="button" type="button" ?disabled=${this.disabled} part="button"
              aria-label=${e || R}>
        <slot></slot>
        ${this.shortcut ? w`<span class="shortcut">${this.shortcut}</span>` : ""}
      </button>
    `;
  }
};
gs.shadowRootOptions = {
  ...Ct.shadowRootOptions,
  delegatesFocus: !0
}, gs.styles = [
  C.base,
  I`
      :host {
        display: inline-block;
        vertical-align: middle;
      }
      :host([variant="icon"]) {
        display: inline-flex;
      }

      .button {
        font-family: var(--mono, 'JetBrains Mono', monospace);
        cursor: pointer;
        transition: color 0.15s, background 0.15s, border-color 0.15s, opacity 0.15s;
        line-height: 1;
        white-space: nowrap;
        text-decoration: none;
        border: none;
        background: transparent;
        color: inherit;
        -webkit-font-smoothing: antialiased;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        gap: 0.4em;
      }
      .button::-moz-focus-inner { border: 0; }
      .button:focus { outline: none; }
      .button:focus-visible {
        outline: var(--focus-ring-width, 1px) solid var(--focus-ring-color-subtle, rgba(200, 168, 85, 0.5));
        outline-offset: var(--focus-ring-offset, 1px);
        border-radius: 3px;
      }
      .button:disabled {
        opacity: 0.4;
        cursor: not-allowed;
        pointer-events: none;
      }

      /* ── primary ── */
      :host([variant="primary"]) .button {
        background: var(--gold, #c8a855);
        color: var(--bg, #0c0c0c);
        padding: 14px 35px;
        border-radius: 6px;
        font-size: var(--text-lg, 14px);
        font-weight: 500;
        letter-spacing: 0.04em;
      }
      :host([variant="primary"]) .button:hover:not(:disabled) { background: var(--gold-bright, #e3c878); opacity: 1; }
      :host([variant="primary"]) .button:active:not(:disabled) { opacity: 0.7; }
      :host([variant="primary"]) .button:focus-visible {
        outline: 2px solid var(--text-primary, #d8d0c0);
        outline-offset: 3px;
        border-radius: 6px;
      }

      /* ── secondary ── */
      :host([variant="secondary"]) .button {
        background: transparent;
        color: var(--text-primary, #d8d0c0);
        padding: 14px 35px;
        border-radius: 6px;
        font-size: var(--text-lg, 14px);
        letter-spacing: 0.04em;
        border: 1px solid var(--border, #1e1e1e);
      }
      :host([variant="secondary"]) .button:hover:not(:disabled) {
        border-color: var(--text-tertiary, #5a5448);
        color: var(--gold, #c8a855);
      }

      /* ── ghost ── */
      :host([variant="ghost"]) .button {
        background: transparent;
        color: var(--text-tertiary, #5a5448);
        padding: 14px 35px;
        border-radius: 6px;
        font-size: var(--text-lg, 14px);
        letter-spacing: 0.04em;
      }
      :host([variant="ghost"]) .button:hover:not(:disabled) { color: var(--text-primary, #d8d0c0); }

      /* ── icon ── */
      :host([variant="icon"]) {
        width: 32px;
        height: 32px;
      }
      :host([variant="icon"]) .button {
        width: 32px;
        height: 32px;
        border-radius: 4px;
        color: var(--text-tertiary, #5a5448);
        font-size: var(--text-md, 13px);
        padding: 0;
      }
      :host([variant="icon"]) .button:hover:not(:disabled) {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
      }

      /* ── pill ── */
      :host([variant="pill"]) .button {
        background: transparent;
        color: var(--gold, #c8a855);
        padding: 6px 16px;
        border: 1px solid var(--gold-dim, rgba(200, 168, 85, 0.5));
        border-radius: 20px;
        font-size: var(--text-sm, 12px);
        letter-spacing: 0.03em;
      }
      :host([variant="pill"]) .button:hover:not(:disabled) {
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
        border-color: var(--gold, #c8a855);
      }

      /* ── run ── */
      :host([variant="run"]) .button {
        background: var(--gold, #c8a855);
        color: var(--bg, #0c0c0c);
        padding: 5px 14px;
        border-radius: 3px;
        font-size: var(--text-xs, 11px);
        letter-spacing: 0.05em;
      }
      :host([variant="run"]) .button:hover:not(:disabled) { opacity: 0.85; }
      :host([variant="run"]) .button:active:not(:disabled) { opacity: 0.7; }
      :host([variant="run"]) .button:focus-visible {
        outline: 2px solid var(--text-primary, #d8d0c0);
        outline-offset: 3px;
        border-radius: 3px;
      }
      /* run + danger: a destructive/stop state, so it must read as danger at rest —
         unlike the debug/action danger modifier below, which only tints on hover. */
      :host([variant="run"][danger]) .button {
        background: transparent;
        border: 1px solid var(--error, #c85555);
        color: var(--error, #c85555);
      }
      :host([variant="run"][danger]) .button:hover:not(:disabled) {
        background: var(--error-bg, rgba(200, 85, 85, 0.06));
        opacity: 1;
      }

      /* shortcut badge inside run */
      .shortcut {
        font-family: system-ui, -apple-system, sans-serif;
        font-size: var(--text-xxs, 10px);
        opacity: 0.7;
        margin-left: 8px;
        background: rgba(0, 0, 0, 0.2);
        font-weight: bold;
        line-height: 1;
        padding: 2px 6px;
        border-radius: 4px;
        pointer-events: none;
        white-space: nowrap;
      }

      /* ── debug ── */
      :host([variant="debug"]) .button {
        width: 28px;
        height: 24px;
        border-radius: 3px;
        border: 1px solid var(--border, #1e1e1e);
        color: var(--text-secondary, #a09888);
        font-family: system-ui, -apple-system, sans-serif;
        font-size: var(--text-md, 13px);
        background: transparent;
      }
      :host([variant="debug"]) .button:hover:not(:disabled) {
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
        color: var(--gold, #c8a855);
        border-color: var(--gold-dim, rgba(200, 168, 85, 0.5));
      }
      :host([variant="debug"]) .button:focus-visible {
        outline-offset: 0;
        border-radius: 3px;
      }
      :host([variant="debug"][danger]) .button:hover:not(:disabled) {
        color: var(--error, #c85555);
        border-color: var(--error, #c85555);
      }

      /* ── action ── */
      :host([variant="action"]) .button {
        width: 24px;
        height: 24px;
        border-radius: 3px;
        background: var(--bg-elevated, #141414);
        color: var(--text-tertiary, #5a5448);
        font-size: var(--text-xxs, 10px);
      }
      :host([variant="action"]) .button:hover:not(:disabled) {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
      }
      :host([variant="action"][danger]) .button:hover:not(:disabled) {
        color: var(--error, #c85555);
      }

      /* ── slot content layout ── */
      .button ::slotted(svg) {
        width: 16px;
        height: 16px;
        flex-shrink: 0;
      }
      :host([variant="action"]) .button ::slotted(svg) {
        width: 13px;
        height: 13px;
      }

      /* size=sm — compact toolbar metrics; placed last so it overrides the
         form-scale text variants (secondary/ghost/primary) on equal specificity. */
      :host([size="sm"]) .button {
        height: var(--control-height-sm, 22px);
        box-sizing: border-box;
        padding: 0 14px;
        font-size: var(--text-xs, 11px);
        border-radius: var(--radius-sm, 3px);
      }
      /* icon is a fixed square — sm shrinks the box to the shared control height. */
      :host([size="sm"][variant="icon"]) {
        width: var(--control-height-sm, 22px);
        height: var(--control-height-sm, 22px);
      }
      :host([size="sm"][variant="icon"]) .button {
        width: var(--control-height-sm, 22px);
        height: var(--control-height-sm, 22px);
        padding: 0;
      }
    `
];
let je = gs;
vn([
  m({ reflect: !0 })
], je.prototype, "variant");
vn([
  m({ reflect: !0 })
], je.prototype, "size");
vn([
  m({ type: Boolean, reflect: !0 })
], je.prototype, "disabled");
vn([
  m({ type: Boolean, reflect: !0 })
], je.prototype, "danger");
vn([
  m({ attribute: "shortcut" })
], je.prototype, "shortcut");
customElements.define("sema-button", je);
var xc = Object.defineProperty, Mr = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && xc(e, t, r), r;
};
const yo = class yo extends C {
  constructor() {
    super(...arguments), this.variant = "neutral", this.pill = !1, this.dot = !1;
  }
  render() {
    return w`
      <span class="badge" part="badge">
        ${this.dot ? w`<span class="dot" aria-hidden="true"></span>` : ""}
        <slot></slot>
      </span>
    `;
  }
};
yo.styles = [
  C.base,
  I`
      :host {
        display: inline-flex;
        vertical-align: middle;

        /* Per-variant palette, overridden by :host([variant=…]) below. */
        --_badge-bg: transparent;
        --_badge-border: var(--border, #1e1e1e);
        --_badge-fg: var(--text-secondary, #a09888);
      }

      :host([variant='gold']) {
        --_badge-bg: var(--gold-glow, rgba(200, 168, 85, 0.08));
        --_badge-border: var(--gold-dim, rgba(200, 168, 85, 0.5));
        --_badge-fg: var(--gold, #c8a855);
      }
      :host([variant='success']) {
        --_badge-bg: color-mix(in srgb, var(--success, #6a9955) 12%, transparent);
        --_badge-border: color-mix(in srgb, var(--success, #6a9955) 40%, transparent);
        --_badge-fg: var(--success, #6a9955);
      }
      :host([variant='error']) {
        --_badge-bg: var(--error-bg, rgba(200, 85, 85, 0.06));
        --_badge-border: color-mix(in srgb, var(--error, #c85555) 40%, transparent);
        --_badge-fg: var(--error, #c85555);
      }

      .badge {
        display: inline-flex;
        align-items: center;
        gap: 0.35em;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xxs, 10px);
        line-height: 1;
        letter-spacing: 0.04em;
        white-space: nowrap;
        padding: 4px 7px;
        border: 1px solid var(--_badge-border);
        border-radius: var(--radius-sm, 3px);
        background: var(--_badge-bg);
        color: var(--_badge-fg);
      }

      :host([pill]) .badge {
        padding: 4px 11px;
        border-radius: var(--radius-pill, 20px);
      }

      .dot {
        width: 0.4em;
        height: 0.4em;
        border-radius: var(--radius-full, 50%);
        background: currentColor;
        flex-shrink: 0;
      }
    `
];
let At = yo;
Mr([
  m({ reflect: !0 })
], At.prototype, "variant");
Mr([
  m({ type: Boolean, reflect: !0 })
], At.prototype, "pill");
Mr([
  m({ type: Boolean, reflect: !0 })
], At.prototype, "dot");
customElements.define("sema-badge", At);
var kc = Object.defineProperty, ma = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && kc(e, t, r), r;
};
const vo = class vo extends C {
  constructor() {
    super(...arguments), this.placement = "top", this._onSlotChange = () => this._applyDescription(), this._describedTrigger = null, this._onKeydown = (e) => {
      var s;
      if (e.key !== "Escape") return;
      let t = document.activeElement;
      for (; (s = t == null ? void 0 : t.shadowRoot) != null && s.activeElement; ) t = t.shadowRoot.activeElement;
      t instanceof HTMLElement && t.blur();
    };
  }
  render() {
    return this.content ? w`
      <div class="tooltip" role="tooltip" aria-label=${this.content}>${this.content}<div class="tooltip-arrow"></div></div>
      <slot @slotchange=${this._onSlotChange}></slot>
    ` : w`<slot @slotchange=${this._onSlotChange}></slot>`;
  }
  connectedCallback() {
    super.connectedCallback(), this.addEventListener("keydown", this._onKeydown), this.hasUpdated && this.updateComplete.then(() => {
      this.isConnected && this._applyDescription();
    });
  }
  disconnectedCallback() {
    var e;
    super.disconnectedCallback(), this.removeEventListener("keydown", this._onKeydown), (e = this._describedTrigger) == null || e.removeAttribute("aria-description"), this._describedTrigger = null;
  }
  firstUpdated() {
    this._applyDescription();
  }
  updated(e) {
    e.has("content") && this._applyDescription();
  }
  _slottedTrigger() {
    var s;
    const e = (s = this.shadowRoot) == null ? void 0 : s.querySelector("slot"), t = e == null ? void 0 : e.assignedElements({ flatten: !0 })[0];
    return t instanceof HTMLElement ? t : null;
  }
  // IDREF ARIA (aria-describedby) can't reach across the shadow boundary to the
  // slotted light-DOM trigger, so set the string-valued aria-description on it.
  _applyDescription() {
    const e = this._slottedTrigger();
    this._describedTrigger && this._describedTrigger !== e && this._describedTrigger.removeAttribute("aria-description"), this._describedTrigger = e, e && (this.content ? e.setAttribute("aria-description", this.content) : e.removeAttribute("aria-description"));
  }
};
vo.styles = [
  C.base,
  I`
      :host {
        position: relative;
        display: inline-block;
      }

      .tooltip {
        position: absolute;
        z-index: 200;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xxs, 10px);
        line-height: 1.4;
        padding: 6px 10px;
        background: var(--tooltip-bg, #1a1a1a);
        color: var(--text-primary, #d8d0c0);
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-md, 4px);
        /* Short tips stay one line; longer ones WRAP within max-width rather than
           truncating with an ellipsis (a tooltip must show its full text). */
        white-space: normal;
        overflow-wrap: break-word;
        width: max-content;
        pointer-events: none;
        opacity: 0;
        transition: opacity 0.15s;
        max-width: 20em;
      }

      :host(:hover) .tooltip,
      :host(:focus-within) .tooltip {
        opacity: 1;
      }

      /* ── Fallback positioning (all browsers) ── */
      :host([placement="top"]) .tooltip {
        bottom: calc(100% + 6px);
        left: 50%;
        transform: translateX(-50%);
        margin-bottom: 0;
      }
      :host([placement="bottom"]) .tooltip {
        top: calc(100% + 6px);
        left: 50%;
        transform: translateX(-50%);
      }
      :host([placement="left"]) .tooltip {
        right: calc(100% + 6px);
        top: 50%;
        transform: translateY(-50%);
      }
      :host([placement="right"]) .tooltip {
        left: calc(100% + 6px);
        top: 50%;
        transform: translateY(-50%);
      }

      /* NOTE: CSS anchor positioning (position-area) was intentionally removed.
         An @supports (position-area) query returns true in browsers that only parse
         the property without implementing the layout, which silently overrode the
         fallback and left every tooltip stuck at its trigger's origin. The absolute
         fallback above positions correctly everywhere; z-index keeps it visible. */

      .tooltip-arrow {
        position: absolute;
        width: 6px;
        height: 6px;
        background: var(--tooltip-bg, #1a1a1a);
        border: 1px solid var(--border, #1e1e1e);
        border-top-color: transparent;
        border-left-color: transparent;
        rotate: 45deg;
      }

      :host([placement="top"]) .tooltip-arrow {
        bottom: -4px;
        left: calc(50% - 3px);
      }
      :host([placement="bottom"]) .tooltip-arrow {
        top: -4px;
        left: calc(50% - 3px);
      }
      :host([placement="left"]) .tooltip-arrow {
        right: -4px;
        top: calc(50% - 3px);
      }
      :host([placement="right"]) .tooltip-arrow {
        left: -4px;
        top: calc(50% - 3px);
      }
    `
];
let sn = vo;
ma([
  m({ reflect: !0 })
], sn.prototype, "placement");
ma([
  m({ attribute: "content" })
], sn.prototype, "content");
customElements.define("sema-tooltip", sn);
var Cc = Object.defineProperty, Or = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Cc(e, t, r), r;
};
const _o = class _o extends C {
  constructor() {
    super(...arguments), this.value = "", this.selected = !1, this.tabbable = !1;
  }
  focus() {
    var t;
    const e = (t = this.shadowRoot) == null ? void 0 : t.querySelector(".toggle");
    e == null || e.focus();
  }
  render() {
    return w`
      <div class="toggle" role="radio" aria-checked=${this.selected ? "true" : "false"} tabindex=${this.selected || this.tabbable ? "0" : "-1"}>
        <slot></slot>
      </div>
    `;
  }
};
_o.styles = [
  C.base,
  I`
      :host {
        display: inline-block;
      }
      .toggle {
        display: flex;
        align-items: center;
        height: var(--control-height-sm, 22px);
        box-sizing: border-box;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xxs, 10px);
        letter-spacing: 0.04em;
        padding: 0 9px;
        border-radius: 3px;
        cursor: pointer;
        color: var(--text-tertiary, #5a5448);
        transition: color 0.15s, background 0.15s;
        user-select: none;
        white-space: nowrap;
      }
      .toggle:focus { outline: none; }
      .toggle:focus-visible {
        outline: var(--focus-ring-width, 1px) solid var(--focus-ring-color-subtle, rgba(200, 168, 85, 0.5));
        outline-offset: var(--focus-ring-offset, 1px);
      }
      .toggle:hover {
        color: var(--text-secondary, #a09888);
      }
      :host([selected]) .toggle {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
      }
    `
];
let Et = _o;
Or([
  m({ reflect: !0 })
], Et.prototype, "value");
Or([
  m({ type: Boolean, reflect: !0 })
], Et.prototype, "selected");
Or([
  m({ type: Boolean, reflect: !0 })
], Et.prototype, "tabbable");
customElements.define("sema-toggle", Et);
var $c = Object.defineProperty, Sc = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && $c(e, t, r), r;
};
const wo = class wo extends C {
  constructor() {
    super(...arguments), this.value = "";
  }
  /** All descendant toggles. A getter (not queryAssignedElements) so each toggle may
   *  be wrapped — e.g. in a `<sema-tooltip>` to explain the option — and still count. */
  get _toggles() {
    return [...this.querySelectorAll("sema-toggle")];
  }
  render() {
    return w`
      <div class="group" role="radiogroup" @keydown=${this._onKeydown} @click=${this._onClick}>
        <slot @slotchange=${this._onSlotChange}></slot>
      </div>
    `;
  }
  // Keep the toggles in sync when `value` is set programmatically (controlled use,
  // e.g. restoring a saved selection) — not just on user interaction / slotchange.
  updated(e) {
    e.has("value") && this._updateSelection();
  }
  _onSlotChange() {
    this._updateSelection();
  }
  _onClick(e) {
    const t = e.composedPath();
    for (const s of t)
      if (s instanceof HTMLElement && s.matches("sema-toggle")) {
        this.value = s.value, this._emitChange(), this._updateSelection();
        return;
      }
  }
  _onKeydown(e) {
    const t = e.composedPath();
    let s = null;
    for (const i of t)
      if (i instanceof HTMLElement && i.matches("sema-toggle")) {
        s = i;
        break;
      }
    if (!s) return;
    const r = s, o = this._toggles.indexOf(r);
    if (!(o < 0))
      if (e.key === "ArrowRight" || e.key === "ArrowDown") {
        e.preventDefault();
        const i = (o + 1) % this._toggles.length;
        this._setTabbable(i), this._toggles[i].focus();
      } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
        e.preventDefault();
        const i = (o - 1 + this._toggles.length) % this._toggles.length;
        this._setTabbable(i), this._toggles[i].focus();
      } else (e.key === " " || e.key === "Enter") && (e.preventDefault(), this.value = r.value, this._emitChange(), this._updateSelection());
  }
  _updateSelection() {
    let e = -1;
    this._toggles.forEach((t, s) => {
      t.selected = t.value === this.value, t.selected && (e = s);
    }), this._setTabbable(e >= 0 ? e : 0);
  }
  _setTabbable(e) {
    this._toggles.forEach((t, s) => {
      t.tabbable = s === e;
    });
  }
  _emitChange() {
    this.dispatchEvent(new CustomEvent("sema-change", {
      detail: { value: this.value },
      bubbles: !0,
      composed: !0
    }));
  }
};
wo.styles = [
  C.base,
  I`
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
    `
];
let Wn = wo;
Sc([
  m({ reflect: !0 })
], Wn.prototype, "value");
customElements.define("sema-toggle-group", Wn);
var Ac = Object.defineProperty, _n = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Ac(e, t, r), r;
};
const xo = class xo extends C {
  constructor() {
    super(...arguments), this.direction = "horizontal", this.min = 0, this.max = 1 / 0, this.step = 10, this.shiftStep = 50, this._startCoord = 0, this._endDrag = null, this._onPointerDown = (e) => {
      var i;
      e.preventDefault(), (i = this._endDrag) == null || i.call(this);
      const s = "touches" in e ? e.touches[0] : e;
      this._startCoord = this.direction === "horizontal" ? s.clientX : s.clientY, this.setAttribute("dragging", ""), document.body.style.cursor = this.direction === "horizontal" ? "col-resize" : "row-resize", document.body.style.userSelect = "none", this.dispatchEvent(new CustomEvent("sema-resize-start", { bubbles: !0, composed: !0 }));
      const r = (a) => {
        const l = "touches" in a ? a.touches[0] : a, c = (this.direction === "horizontal" ? l.clientX : l.clientY) - this._startCoord;
        this.dispatchEvent(new CustomEvent("sema-resize", {
          detail: { delta: c },
          bubbles: !0,
          composed: !0
        }));
      }, o = () => {
        this._endDrag = null, this.removeAttribute("dragging"), document.body.style.cursor = "", document.body.style.userSelect = "", document.removeEventListener("mousemove", r), document.removeEventListener("mouseup", o), document.removeEventListener("touchmove", r), document.removeEventListener("touchend", o), document.removeEventListener("touchcancel", o), window.removeEventListener("blur", o), this.isConnected && this.dispatchEvent(new CustomEvent("sema-resize-end", { bubbles: !0, composed: !0 }));
      };
      this._endDrag = o, document.addEventListener("mousemove", r), document.addEventListener("mouseup", o), document.addEventListener("touchmove", r), document.addEventListener("touchend", o), document.addEventListener("touchcancel", o), window.addEventListener("blur", o);
    }, this._onKeydown = (e) => {
      const t = this.direction === "horizontal";
      let s = 0;
      if (e.key === "Home") {
        e.preventDefault(), this.dispatchEvent(new CustomEvent("sema-resize-start", { bubbles: !0, composed: !0 })), this.dispatchEvent(new CustomEvent("sema-resize", {
          detail: { delta: this.min, absolute: !0 },
          bubbles: !0,
          composed: !0
        })), this.dispatchEvent(new CustomEvent("sema-resize-end", { bubbles: !0, composed: !0 }));
        return;
      }
      if (e.key === "End") {
        e.preventDefault(), this.dispatchEvent(new CustomEvent("sema-resize-start", { bubbles: !0, composed: !0 })), this.dispatchEvent(new CustomEvent("sema-resize", {
          detail: { delta: this.max, absolute: !0 },
          bubbles: !0,
          composed: !0
        })), this.dispatchEvent(new CustomEvent("sema-resize-end", { bubbles: !0, composed: !0 }));
        return;
      }
      if (t && e.key === "ArrowLeft" || !t && e.key === "ArrowUp")
        s = e.shiftKey ? -this.shiftStep : -this.step;
      else if (t && e.key === "ArrowRight" || !t && e.key === "ArrowDown")
        s = e.shiftKey ? this.shiftStep : this.step;
      else
        return;
      e.preventDefault(), this.dispatchEvent(new CustomEvent("sema-resize-start", { bubbles: !0, composed: !0 })), this.dispatchEvent(new CustomEvent("sema-resize", {
        detail: { delta: s, keyboard: !0 },
        bubbles: !0,
        composed: !0
      })), this.dispatchEvent(new CustomEvent("sema-resize-end", { bubbles: !0, composed: !0 }));
    };
  }
  connectedCallback() {
    super.connectedCallback(), this.setAttribute("role", "separator"), this.tabIndex = 0, this.hasAttribute("aria-label") || this.setAttribute("aria-label", "Resize"), this.addEventListener("mousedown", this._onPointerDown), this.addEventListener("touchstart", this._onPointerDown, { passive: !1 }), this.addEventListener("keydown", this._onKeydown);
  }
  disconnectedCallback() {
    var e;
    super.disconnectedCallback(), this.removeEventListener("mousedown", this._onPointerDown), this.removeEventListener("touchstart", this._onPointerDown), this.removeEventListener("keydown", this._onKeydown), (e = this._endDrag) == null || e.call(this);
  }
  updated(e) {
    e.has("direction") && this.setAttribute("aria-orientation", this.direction === "horizontal" ? "vertical" : "horizontal"), e.has("min") && this.setAttribute("aria-valuemin", String(this.min)), e.has("max") && (Number.isFinite(this.max) ? this.setAttribute("aria-valuemax", String(this.max)) : this.removeAttribute("aria-valuemax"));
  }
  /** Report the current pane size to assistive tech (aria-valuenow/-valuetext). */
  setValue(e, t) {
    this.setAttribute("aria-valuenow", String(e)), t !== void 0 ? this.setAttribute("aria-valuetext", t) : this.removeAttribute("aria-valuetext");
  }
  render() {
    return w``;
  }
};
xo.styles = [
  C.base,
  I`
      :host {
        display: block;
        flex-shrink: 0;
        background: var(--border, #1e1e1e);
        position: relative;
        z-index: 5;
        transition: background 0.15s;
        outline: none;
      }
      :host([direction="horizontal"]) {
        width: 4px;
        cursor: col-resize;
      }
      :host([direction="vertical"]) {
        height: 4px;
        cursor: row-resize;
      }
      /* Invisible expanded hit target — a 4px bar is too thin to grab reliably. */
      :host::after {
        content: '';
        position: absolute;
      }
      :host([direction="horizontal"])::after {
        inset: 0 -4px;
      }
      :host([direction="vertical"])::after {
        inset: -4px 0;
      }
      :host(:hover),
      :host([dragging]) {
        background: var(--gold-dim, rgba(200, 168, 85, 0.5));
      }
      :host(:focus-visible) {
        outline: var(--focus-ring-width, 1px) solid var(--focus-ring-color-subtle, rgba(200, 168, 85, 0.5));
        outline-offset: var(--focus-ring-offset, 1px);
      }
    `
];
let qe = xo;
_n([
  m({ reflect: !0 })
], qe.prototype, "direction");
_n([
  m({ type: Number })
], qe.prototype, "min");
_n([
  m({ type: Number })
], qe.prototype, "max");
_n([
  m({ type: Number })
], qe.prototype, "step");
_n([
  m({ type: Number })
], qe.prototype, "shiftStep");
customElements.define("sema-splitter", qe);
const Ec = 'a[href],button:not([disabled]),textarea:not([disabled]),input:not([disabled]),select:not([disabled]),[tabindex]:not([tabindex="-1"])';
function lr(n) {
  const e = [];
  function t(s) {
    if (s instanceof HTMLElement && s.matches(Ec) && !e.includes(s) && e.push(s), s.shadowRoot && s.shadowRoot.mode === "open")
      for (const r of s.shadowRoot.children)
        t(r);
    if (s instanceof HTMLSlotElement)
      for (const r of s.assignedElements({ flatten: !0 }))
        t(r);
    for (const r of s.children)
      t(r);
  }
  return t(n), e;
}
const xe = [];
let jt = 0;
class zr {
  constructor(e, t) {
    this._previouslyFocused = null, this._activated = !1, this._attached = !1, this._didLockScroll = !1, this._rafId = null, this._host = e, this._getContainer = t.getContainer, this._isActive = t.isActive, this._lockScroll = t.lockScroll ?? !1, this._initialFocus = t.initialFocus ?? "first-focusable", this._boundKeydown = this._onKeydown.bind(this), e.addController(this);
  }
  hostConnected() {
    this._isActive(this._host) && (this._activate(), this._rafId = requestAnimationFrame(() => {
      this._rafId = null, this._activated && this._host.isConnected && this._focusFirstTabbable();
    }));
  }
  hostUpdated() {
    const e = this._isActive(this._host);
    e && !this._activated && (this._activate(), this._focusFirstTabbable()), !e && this._activated && this._deactivate();
  }
  hostDisconnected() {
    this._cancelPendingFocus(), this._activated && this._deactivate();
  }
  _activate() {
    this._activated || (this._activated = !0, this._previouslyFocused = document.activeElement, xe.length > 0 && xe[xe.length - 1]._detach(), xe.push(this), this._getContainer(this._host) && this._attach(), this._lockScroll && this._lockBodyScroll());
  }
  _deactivate() {
    if (!this._activated) return;
    this._activated = !1, this._cancelPendingFocus(), this._detach();
    const e = xe.indexOf(this);
    e !== -1 && xe.splice(e, 1), xe.length > 0 && xe[xe.length - 1]._attach(), this._didLockScroll && this._unlockBodyScroll();
    const t = this._previouslyFocused;
    t instanceof HTMLElement && document.activeElement !== t && t.focus({ preventScroll: !0 });
  }
  _attach() {
    if (this._attached) return;
    const e = this._getContainer(this._host);
    e && (e.addEventListener("keydown", this._boundKeydown), this._attached = !0);
  }
  _detach() {
    if (!this._attached) return;
    const e = this._getContainer(this._host);
    e == null || e.removeEventListener("keydown", this._boundKeydown), this._attached = !1;
  }
  _cancelPendingFocus() {
    this._rafId !== null && (cancelAnimationFrame(this._rafId), this._rafId = null);
  }
  _focusFirstTabbable() {
    const e = this._getContainer(this._host);
    if (!e) return;
    if (this._initialFocus === "autofocus") {
      const s = e.querySelector("[autofocus]");
      if (s) {
        s.focus();
        return;
      }
    }
    const t = lr(e);
    t.length > 0 ? t[0].focus() : e.focus();
  }
  _onKeydown(e) {
    if (e.key !== "Tab") return;
    const t = this._getContainer(this._host);
    if (!t) return;
    const s = lr(t);
    if (s.length === 0) {
      e.preventDefault();
      return;
    }
    const r = e.composedPath(), o = s.findIndex((l) => r.includes(l));
    if (o === -1) return;
    const i = s[0], a = s[s.length - 1];
    e.shiftKey ? s[o] === i && (e.preventDefault(), a.focus()) : s[o] === a && (e.preventDefault(), i.focus());
  }
  _lockBodyScroll() {
    jt === 0 && (document.body.style.overflow = "hidden"), jt++, this._didLockScroll = !0;
  }
  _unlockBodyScroll() {
    this._didLockScroll && (jt = Math.max(0, jt - 1), jt === 0 && (document.body.style.overflow = ""), this._didLockScroll = !1);
  }
}
var Rc = Object.defineProperty, ba = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Rc(e, t, r), r;
};
const ko = class ko extends C {
  constructor() {
    super(...arguments), this.open = !1, this._labelId = `sema-dialog-${Math.random().toString(36).slice(2, 8)}`, this._bodyId = `sema-dialog-body-${Math.random().toString(36).slice(2, 8)}`, this._focusTrap = new zr(this, {
      getContainer: (e) => e,
      isActive: (e) => e.open,
      lockScroll: !0
    }), this._onDocKeydown = (e) => {
      e.key === "Escape" && (e.preventDefault(), this.close());
    };
  }
  connectedCallback() {
    super.connectedCallback(), this.tabIndex = -1, this.open && document.addEventListener("keydown", this._onDocKeydown);
  }
  disconnectedCallback() {
    super.disconnectedCallback(), document.removeEventListener("keydown", this._onDocKeydown);
  }
  render() {
    return this.open ? w`
      <div class="backdrop" role="presentation" @click=${this._onBackdropClick}>
        <div class="dialog" role="dialog" aria-modal="true"
             aria-labelledby=${this.label ? this._labelId : R}
             aria-label=${this.label ? R : this.getAttribute("aria-label") || "Dialog"}
             aria-describedby=${this._bodyId}>
          ${this.label ? w`<div class="header" id=${this._labelId}>${this.label}</div>` : ""}
          <div class="body" id=${this._bodyId}><slot></slot></div>
          <div class="footer"><slot name="footer"></slot></div>
        </div>
      </div>
    ` : w``;
  }
  updated(e) {
    e.has("open") && (this.open ? (document.addEventListener("keydown", this._onDocKeydown), this.dispatchEvent(new CustomEvent("sema-open", { bubbles: !0, composed: !0 }))) : (document.removeEventListener("keydown", this._onDocKeydown), this.dispatchEvent(new CustomEvent("sema-close", { bubbles: !0, composed: !0 }))));
  }
  close() {
    this.open = !1;
  }
  show() {
    this.open = !0;
  }
  _onBackdropClick(e) {
    e.target.classList.contains("backdrop") && this.close();
  }
};
ko.styles = [
  C.base,
  I`
      :host {
        display: none;
      }
      /* Give the open host a real box matching what is visually painted: its
         only child (.backdrop) is position:fixed and out of flow, so without
         this the host computes 0x0 and visibility checks (a11y tools,
         Playwright toBeVisible) report the shown dialog as hidden. */
      :host([open]) {
        display: block;
        position: fixed;
        inset: 0;
        z-index: 500;
      }

      .backdrop {
        position: fixed;
        inset: 0;
        z-index: 500;
        display: flex;
        align-items: center;
        justify-content: center;
        background: rgba(0, 0, 0, 0.6);
        animation: fadeIn 0.15s ease;
      }

      .dialog {
        background: var(--bg-elevated, #141414);
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-xl, 8px);
        min-width: 320px;
        max-width: 480px;
        width: 90vw;
        max-height: 80vh;
        display: flex;
        flex-direction: column;
        box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
        animation: slideUp 0.15s ease;
      }

      .header {
        font-family: var(--serif, 'Cormorant', Georgia, serif);
        font-size: var(--text-3xl, 22px);
        font-weight: 400;
        color: var(--text-primary, #d8d0c0);
        padding: var(--space-lg, 24px) var(--space-lg, 24px) 0;
      }

      .body {
        font-family: var(--serif, 'Cormorant', Georgia, serif);
        font-size: var(--text-2xl, 18px);
        line-height: 1.7;
        color: var(--text-secondary, #a09888);
        padding: var(--space-md, 16px) var(--space-lg, 24px);
        overflow-y: auto;
      }

      .footer {
        display: flex;
        justify-content: flex-end;
        gap: var(--space-lg, 24px);
        padding: 0 var(--space-lg, 24px) var(--space-lg, 24px);
        border-top: none;
      }

      @keyframes fadeIn {
        from { opacity: 0; }
        to { opacity: 1; }
      }
      @keyframes slideUp {
        from { opacity: 0; transform: translateY(8px); }
        to { opacity: 1; transform: translateY(0); }
      }
    `
];
let rn = ko;
ba([
  m({ type: Boolean, reflect: !0 })
], rn.prototype, "open");
ba([
  m({ attribute: "label" })
], rn.prototype, "label");
customElements.define("sema-dialog", rn);
var Tc = Object.defineProperty, Br = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Tc(e, t, r), r;
};
const Co = class Co extends C {
  constructor() {
    super(...arguments), this.open = !1, this.placement = "right", this._labelId = `sema-drawer-${Math.random().toString(36).slice(2, 8)}`, this._hasFooter = !1, this._focusTrap = new zr(this, {
      getContainer: (e) => {
        var t;
        return ((t = e.shadowRoot) == null ? void 0 : t.querySelector(".panel")) ?? e;
      },
      isActive: (e) => e.open,
      lockScroll: !0
    }), this._onDocKeydown = (e) => {
      e.key === "Escape" && (e.preventDefault(), this.close());
    }, this.close = () => {
      this.open = !1;
    };
  }
  connectedCallback() {
    super.connectedCallback(), this.open && document.addEventListener("keydown", this._onDocKeydown);
  }
  disconnectedCallback() {
    super.disconnectedCallback(), document.removeEventListener("keydown", this._onDocKeydown);
  }
  _onFooterSlotChange(e) {
    const s = e.target.assignedNodes({ flatten: !0 }).length > 0;
    s !== this._hasFooter && (this._hasFooter = s, this.requestUpdate());
  }
  render() {
    return this.open ? w`
      <div class="backdrop" part="backdrop" role="presentation" @click=${this.close}></div>
      <div class="panel" part="panel" role="dialog" aria-modal="true"
           aria-labelledby=${this.label ? this._labelId : R}
           aria-label=${this.label ? R : this.getAttribute("aria-label") || "Drawer"}>
        <div class="header">
          ${this.label ? w`<h2 class="title" id=${this._labelId}>${this.label}</h2>` : w`<slot name="header"></slot>`}
          <button class="close" part="close" type="button" aria-label="Close" @click=${this.close}>✕</button>
        </div>
        <div class="body"><slot></slot></div>
        <div class="footer ${this._hasFooter ? "" : "empty"}">
          <slot name="footer" @slotchange=${this._onFooterSlotChange}></slot>
        </div>
      </div>
    ` : w``;
  }
  updated(e) {
    e.has("open") && (this.open ? (document.addEventListener("keydown", this._onDocKeydown), this.dispatchEvent(new CustomEvent("sema-drawer-open", { bubbles: !0, composed: !0 }))) : (document.removeEventListener("keydown", this._onDocKeydown), this.dispatchEvent(new CustomEvent("sema-drawer-close", { bubbles: !0, composed: !0 }))));
  }
  show() {
    this.open = !0;
  }
};
Co.styles = [
  C.base,
  I`
      :host {
        display: none;
        --drawer-size: 320px;
      }
      :host([open]) {
        display: block;
      }

      .backdrop {
        position: fixed;
        inset: 0;
        z-index: 500;
        background: rgba(0, 0, 0, 0.6);
        animation: fadeIn 0.15s ease;
      }

      .panel {
        position: fixed;
        z-index: 501;
        background: var(--bg-elevated, #141414);
        border: 1px solid var(--border, #1e1e1e);
        display: flex;
        flex-direction: column;
        box-shadow: 0 8px 32px rgba(0, 0, 0, 0.4);
        overflow: hidden;
      }

      /* ── docking ── */
      :host([placement='right']) .panel,
      :host(:not([placement])) .panel {
        top: 0;
        bottom: 0;
        right: 0;
        width: var(--drawer-size);
        max-width: 100vw;
        border-width: 0 0 0 1px;
        animation: slideInRight 0.18s ease;
      }
      :host([placement='left']) .panel {
        top: 0;
        bottom: 0;
        left: 0;
        width: var(--drawer-size);
        max-width: 100vw;
        border-width: 0 1px 0 0;
        animation: slideInLeft 0.18s ease;
      }
      :host([placement='top']) .panel {
        left: 0;
        right: 0;
        top: 0;
        height: var(--drawer-size);
        max-height: 100vh;
        border-width: 0 0 1px 0;
        animation: slideInTop 0.18s ease;
      }
      :host([placement='bottom']) .panel {
        left: 0;
        right: 0;
        bottom: 0;
        height: var(--drawer-size);
        max-height: 100vh;
        border-width: 1px 0 0 0;
        animation: slideInBottom 0.18s ease;
      }

      .header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: var(--space-md, 16px);
        padding: var(--space-md, 16px) var(--space-lg, 24px);
        border-bottom: 1px solid var(--border, #1e1e1e);
      }
      .title {
        font-family: var(--serif, 'Cormorant', Georgia, serif);
        font-size: var(--text-3xl, 22px);
        font-weight: 400;
        color: var(--text-primary, #d8d0c0);
        margin: 0;
      }
      .close {
        flex-shrink: 0;
        width: 28px;
        height: 28px;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        background: transparent;
        border: none;
        border-radius: var(--radius-md, 4px);
        color: var(--text-tertiary, #5a5448);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xl, 16px);
        line-height: 1;
        cursor: pointer;
        transition: color 0.15s, background 0.15s;
      }
      .close:hover {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
      }
      .close:focus { outline: none; }
      .close:focus-visible {
        outline: var(--focus-ring-width, 1px) solid var(--focus-ring-color-subtle, rgba(200, 168, 85, 0.5));
        outline-offset: var(--focus-ring-offset, 1px);
      }

      .body {
        flex: 1;
        min-height: 0;
        overflow: auto;
        padding: var(--space-lg, 24px);
        color: var(--text-secondary, #a09888);
      }

      .footer {
        display: flex;
        justify-content: flex-end;
        gap: var(--space-md, 16px);
        padding: var(--space-md, 16px) var(--space-lg, 24px);
        border-top: 1px solid var(--border, #1e1e1e);
      }
      .footer.empty {
        display: none;
      }

      @keyframes fadeIn { from { opacity: 0; } to { opacity: 1; } }
      @keyframes slideInRight { from { transform: translateX(100%); } to { transform: translateX(0); } }
      @keyframes slideInLeft { from { transform: translateX(-100%); } to { transform: translateX(0); } }
      @keyframes slideInTop { from { transform: translateY(-100%); } to { transform: translateY(0); } }
      @keyframes slideInBottom { from { transform: translateY(100%); } to { transform: translateY(0); } }
    `
];
let Rt = Co;
Br([
  m({ type: Boolean, reflect: !0 })
], Rt.prototype, "open");
Br([
  m({ reflect: !0 })
], Rt.prototype, "placement");
Br([
  m({ attribute: "label" })
], Rt.prototype, "label");
customElements.define("sema-drawer", Rt);
var Ic = Object.defineProperty, pt = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Ic(e, t, r), r;
};
const $o = class $o extends C {
  constructor() {
    super(...arguments), this._onSlotChange = () => {
      const e = Array.from(this.querySelectorAll("sema-tree-item"));
      e.length && !e.some((t) => t.tabbable) && (e[0].tabbable = !0);
    };
  }
  connectedCallback() {
    super.connectedCallback(), this.setAttribute("role", "tree"), !this.hasAttribute("aria-label") && !this.hasAttribute("aria-labelledby") && this.setAttribute("aria-label", "Tree");
  }
  render() {
    return w`<slot @slotchange=${this._onSlotChange}></slot>`;
  }
};
$o.styles = [
  C.base,
  I`
      :host {
        display: block;
      }
    `
];
let Qn = $o;
var Ge;
const Ve = (Ge = class extends C {
  constructor() {
    super(...arguments), this.expanded = !1, this.selected = !1, this.hasChildren = !1, this.depth = 0, this.tabbable = !1, this._hasSlotChildren = !1;
  }
  render() {
    const e = 12 + this.depth * 14;
    return w`
      <div class="row" part="row" role="treeitem" tabindex=${this.tabbable ? "0" : "-1"}
           style="padding-left:${e}px;"
        aria-label=${this.label || R}
        aria-expanded=${this.hasChildren || this._hasSlotChildren ? String(this.expanded) : R}
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
    super.connectedCallback(), this._updateDepth();
  }
  _updateDepth() {
    let e = this.parentElement, t = 0;
    for (; e && (e instanceof Ge && t++, !(e instanceof Qn || (e = e.parentElement, !(e != null && e.parentElement)))); )
      ;
    this.depth = t;
  }
  _onSlotChange() {
    var e, t;
    this._hasSlotChildren = (((t = (e = this.shadowRoot) == null ? void 0 : e.querySelector("slot")) == null ? void 0 : t.assignedElements().length) ?? 0) > 0;
  }
  _onClick(e) {
    e.stopPropagation(), (this._hasSlotChildren || this.hasChildren) && (this.expanded = !this.expanded), this._select(), this._makeTabStop();
  }
  /** Become the single roving tab stop within the tree. */
  _makeTabStop() {
    const e = this.closest("sema-tree");
    e == null || e.querySelectorAll("sema-tree-item").forEach((t) => {
      t.tabbable = t === this;
    });
  }
  _onKeydown(e) {
    e.key === "Enter" || e.key === " " ? (e.preventDefault(), e.stopPropagation(), this._onClick(e)) : e.key === "ArrowRight" && !this.expanded ? (e.preventDefault(), e.stopPropagation(), this.expanded = !0) : e.key === "ArrowLeft" && this.expanded ? (e.preventDefault(), e.stopPropagation(), this.expanded = !1) : e.key === "ArrowDown" ? (e.preventDefault(), e.stopPropagation(), this._focusAdjacent("next")) : e.key === "ArrowUp" && (e.preventDefault(), e.stopPropagation(), this._focusAdjacent("prev"));
  }
  focus() {
    var t;
    const e = (t = this.shadowRoot) == null ? void 0 : t.querySelector(".row");
    e == null || e.focus();
  }
  _select() {
    this.dispatchEvent(new CustomEvent("sema-tree-select", {
      detail: { label: this.label, element: this },
      bubbles: !0,
      composed: !0
    }));
  }
  _focusAdjacent(e) {
    if (!this.shadowRoot.querySelector(".row")) return;
    const s = this.closest("sema-tree");
    if (!s) return;
    const r = (l) => {
      var u;
      const c = [], h = l instanceof HTMLSlotElement ? l.assignedElements() : Array.from(l.children);
      for (const p of h)
        if (p instanceof Ge) {
          c.push(p);
          const d = p.hasChildren || p._hasSlotChildren;
          if (p.expanded || !d) {
            const f = (u = p.shadowRoot) == null ? void 0 : u.querySelector(".children slot");
            f && c.push(...r(f));
          }
        } else
          c.push(...r(p));
      return c;
    }, o = r(s), i = o.indexOf(this), a = e === "next" ? i + 1 : i - 1;
    if (a >= 0 && a < o.length) {
      const l = o[a];
      s.querySelectorAll("sema-tree-item").forEach((c) => {
        c.tabbable = c === l;
      }), l.focus();
    }
  }
}, Ge.styles = [
  C.base,
  I`
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
    `
], Ge);
pt([
  m({ reflect: !0 })
], Ve.prototype, "label");
pt([
  m({ type: Boolean, reflect: !0 })
], Ve.prototype, "expanded");
pt([
  m({ type: Boolean, reflect: !0 })
], Ve.prototype, "selected");
pt([
  m({ type: Boolean, reflect: !0, attribute: "has-children" })
], Ve.prototype, "hasChildren");
pt([
  m({ type: Number, reflect: !0 })
], Ve.prototype, "depth");
pt([
  m({ type: Boolean, reflect: !0 })
], Ve.prototype, "tabbable");
pt([
  Qe()
], Ve.prototype, "_hasSlotChildren");
let Pc = Ve;
customElements.define("sema-tree", Qn);
customElements.define("sema-tree-item", Pc);
var Lc = Object.defineProperty, ya = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Lc(e, t, r), r;
};
const ai = "https://fonts.googleapis.com/css2?family=Cormorant:ital,wght@0,300;0,400;0,500;0,600;1,400&family=Inter:wght@300;400;500;600&family=JetBrains+Mono:wght@400;500&display=swap";
let Gs = !1;
const Nc = [
  "margin",
  "padding",
  "background",
  "color",
  "font-family",
  "font-size",
  "line-height",
  "-webkit-font-smoothing",
  "height",
  "min-height",
  "overflow",
  "display",
  "flex-direction"
];
let Tn = 0, In = null;
function Mc() {
  if (Gs) return;
  if (document.querySelector(`link[href="${ai}"]`)) {
    Gs = !0;
    return;
  }
  const n = document.createElement("link");
  n.rel = "stylesheet", n.href = ai, document.head.appendChild(n), Gs = !0;
}
const So = class So extends C {
  constructor() {
    super(...arguments), this.fullHeight = !1, this.flex = !1;
  }
  connectedCallback() {
    super.connectedCallback(), Mc(), this._ensureMeta("viewport", "width=device-width, initial-scale=1"), this._ensureMeta("theme-color", "#c8a855"), Tn === 0 && (In = new Map(
      Nc.map((e) => [e, document.body.style.getPropertyValue(e)])
    )), Tn++, this._applyBody();
  }
  disconnectedCallback() {
    if (super.disconnectedCallback(), Tn--, Tn === 0 && In) {
      for (const [e, t] of In)
        t ? document.body.style.setProperty(e, t) : document.body.style.removeProperty(e);
      In = null;
    }
  }
  updated(e) {
    this.isConnected && (e.has("fullHeight") || e.has("flex")) && this._applyBody();
  }
  _ensureMeta(e, t) {
    let s = document.querySelector(`meta[name="${e}"]`);
    s || (s = document.createElement("meta"), s.setAttribute("name", e), document.head.appendChild(s)), s.setAttribute("content", t);
  }
  _applyBody() {
    const e = document.body;
    e.style.margin = "0", e.style.padding = "0", e.style.background = "var(--bg, #0c0c0c)", e.style.color = "var(--text-secondary, #a09888)", e.style.fontFamily = "var(--sans, 'Inter', system-ui, -apple-system, sans-serif)", e.style.fontSize = "18px", e.style.lineHeight = "1.7", e.style.setProperty("-webkit-font-smoothing", "antialiased"), this.fullHeight ? (e.style.height = "100vh", e.style.minHeight = "", e.style.overflow = "hidden") : (e.style.height = "", e.style.minHeight = "100vh", e.style.overflow = ""), this.flex ? (e.style.display = "flex", e.style.flexDirection = "column") : (e.style.display = "", e.style.flexDirection = "");
  }
  render() {
    return w`<slot></slot>`;
  }
};
So.styles = [
  C.base,
  I`
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
        --sans: 'Inter', system-ui, -apple-system, sans-serif;
        --mono: 'JetBrains Mono', monospace;
      }
    `
];
let on = So;
ya([
  m({ type: Boolean, reflect: !0, attribute: "full-height" })
], on.prototype, "fullHeight");
ya([
  m({ type: Boolean, reflect: !0 })
], on.prototype, "flex");
customElements.define("sema-page", on);
/**
 * @license
 * Copyright 2017 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const Me = { ATTRIBUTE: 1, CHILD: 2, PROPERTY: 3, BOOLEAN_ATTRIBUTE: 4 }, Dr = (n) => (...e) => ({ _$litDirective$: n, values: e });
let Fr = class {
  constructor(e) {
  }
  get _$AU() {
    return this._$AM._$AU;
  }
  _$AT(e, t, s) {
    this._$Ct = e, this._$AM = t, this._$Ci = s;
  }
  _$AS(e, t) {
    return this.update(e, t);
  }
  update(e, t) {
    return this.render(...t);
  }
};
/**
 * @license
 * Copyright 2017 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
class cr extends Fr {
  constructor(e) {
    if (super(e), this.it = R, e.type !== Me.CHILD) throw Error(this.constructor.directiveName + "() can only be used in child bindings");
  }
  render(e) {
    if (e === R || e == null) return this._t = void 0, this.it = e;
    if (e === le) return e;
    if (typeof e != "string") throw Error(this.constructor.directiveName + "() called with a non-string value");
    if (e === this.it) return this._t;
    this.it = e;
    const t = [e];
    return t.raw = t, this._t = { _$litType$: this.constructor.resultType, strings: t, values: [] };
  }
}
cr.directiveName = "unsafeHTML", cr.resultType = 1;
const an = Dr(cr);
/**
 * @license
 * Copyright 2017 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const Oc = Symbol();
let va = class {
  get taskComplete() {
    return this.t || (this.i === 1 ? this.t = new Promise(((e, t) => {
      this.o = e, this.h = t;
    })) : this.i === 3 ? this.t = Promise.reject(this.l) : this.t = Promise.resolve(this.u)), this.t;
  }
  constructor(e, t, s) {
    var o;
    this.p = 0, this.i = 0, (this._ = e).addController(this);
    const r = typeof t == "object" ? t : { task: t, args: s };
    this.v = r.task, this.j = r.args, this.m = r.argsEqual ?? zc, this.k = r.onComplete, this.A = r.onError, this.autoRun = r.autoRun ?? !0, "initialValue" in r && (this.u = r.initialValue, this.i = 2, this.O = (o = this.T) == null ? void 0 : o.call(this));
  }
  hostUpdate() {
    this.autoRun === !0 && this.S();
  }
  hostUpdated() {
    this.autoRun === "afterUpdate" && this.S();
  }
  T() {
    if (this.j === void 0) return;
    const e = this.j();
    if (!Array.isArray(e)) throw Error("The args function must return an array");
    return e;
  }
  async S() {
    const e = this.T(), t = this.O;
    this.O = e, e === t || e === void 0 || t !== void 0 && this.m(t, e) || await this.run(e);
  }
  async run(e) {
    var i, a, l, c, h;
    let t, s;
    e ?? (e = this.T()), this.O = e, this.i === 1 ? (i = this.q) == null || i.abort() : (this.t = void 0, this.o = void 0, this.h = void 0), this.i = 1, this.autoRun === "afterUpdate" ? queueMicrotask((() => this._.requestUpdate())) : this._.requestUpdate();
    const r = ++this.p;
    this.q = new AbortController();
    let o = !1;
    try {
      t = await this.v(e, { signal: this.q.signal });
    } catch (u) {
      o = !0, s = u;
    }
    if (this.p === r) {
      if (t === Oc) this.i = 0;
      else {
        if (o === !1) {
          try {
            (a = this.k) == null || a.call(this, t);
          } catch {
          }
          this.i = 2, (l = this.o) == null || l.call(this, t);
        } else {
          try {
            (c = this.A) == null || c.call(this, s);
          } catch {
          }
          this.i = 3, (h = this.h) == null || h.call(this, s);
        }
        this.u = t, this.l = s;
      }
      this._.requestUpdate();
    }
  }
  abort(e) {
    var t;
    this.i === 1 && ((t = this.q) == null || t.abort(e));
  }
  get value() {
    return this.u;
  }
  get error() {
    return this.l;
  }
  get status() {
    return this.i;
  }
  render(e) {
    var t, s, r, o;
    switch (this.i) {
      case 0:
        return (t = e.initial) == null ? void 0 : t.call(e);
      case 1:
        return (s = e.pending) == null ? void 0 : s.call(e);
      case 2:
        return (r = e.complete) == null ? void 0 : r.call(e, this.value);
      case 3:
        return (o = e.error) == null ? void 0 : o.call(e, this.error);
      default:
        throw Error("Unexpected status: " + this.i);
    }
  }
};
const zc = (n, e) => n === e || n.length === e.length && n.every(((t, s) => !vs(t, e[s])));
function Vn(n) {
  if (!n) return "";
  const e = n.replace(/\r\n/g, `
`).replace(/^\n/, "").split(`
`);
  for (; e.length && e[0].trim() === ""; ) e.shift();
  for (; e.length && e[e.length - 1].trim() === ""; ) e.pop();
  if (e.length === 0) return "";
  let t = 1 / 0;
  for (const s of e) {
    if (s.trim() === "") continue;
    const r = s.match(/^[ \t]*/)[0].length;
    r < t && (t = r);
  }
  return !Number.isFinite(t) || t === 0 ? e.join(`
`) : e.map((s) => s.slice(t)).join(`
`);
}
var O = class extends Error {
  constructor(n) {
    super(n), this.name = "ShikiError";
  }
};
function Bc(n) {
  return Gr(n);
}
function Gr(n) {
  return Array.isArray(n) ? Dc(n) : n instanceof RegExp ? n : typeof n == "object" ? Fc(n) : n;
}
function Dc(n) {
  let e = [];
  for (let t = 0, s = n.length; t < s; t++)
    e[t] = Gr(n[t]);
  return e;
}
function Fc(n) {
  let e = {};
  for (let t in n)
    e[t] = Gr(n[t]);
  return e;
}
function _a(n, ...e) {
  return e.forEach((t) => {
    for (let s in t)
      n[s] = t[s];
  }), n;
}
function wa(n) {
  const e = ~n.lastIndexOf("/") || ~n.lastIndexOf("\\");
  return e === 0 ? n : ~e === n.length - 1 ? wa(n.substring(0, n.length - 1)) : n.substr(~e + 1);
}
var Us = /\$(\d+)|\${(\d+):\/(downcase|upcase)}/g, Pn = class {
  static hasCaptures(n) {
    return n === null ? !1 : (Us.lastIndex = 0, Us.test(n));
  }
  static replaceCaptures(n, e, t) {
    return n.replace(Us, (s, r, o, i) => {
      let a = t[parseInt(r || o, 10)];
      if (a) {
        let l = e.substring(a.start, a.end);
        for (; l[0] === "."; )
          l = l.substring(1);
        switch (i) {
          case "downcase":
            return l.toLowerCase();
          case "upcase":
            return l.toUpperCase();
          default:
            return l;
        }
      } else
        return s;
    });
  }
};
function xa(n, e) {
  return n < e ? -1 : n > e ? 1 : 0;
}
function ka(n, e) {
  if (n === null && e === null)
    return 0;
  if (!n)
    return -1;
  if (!e)
    return 1;
  let t = n.length, s = e.length;
  if (t === s) {
    for (let r = 0; r < t; r++) {
      let o = xa(n[r], e[r]);
      if (o !== 0)
        return o;
    }
    return 0;
  }
  return t - s;
}
function li(n) {
  return !!(/^#[0-9a-f]{6}$/i.test(n) || /^#[0-9a-f]{8}$/i.test(n) || /^#[0-9a-f]{3}$/i.test(n) || /^#[0-9a-f]{4}$/i.test(n));
}
function Ca(n) {
  return n.replace(/[\-\\\{\}\*\+\?\|\^\$\.\,\[\]\(\)\#\s]/g, "\\$&");
}
var $a = class {
  constructor(n) {
    g(this, "cache", /* @__PURE__ */ new Map());
    this.fn = n;
  }
  get(n) {
    if (this.cache.has(n))
      return this.cache.get(n);
    const e = this.fn(n);
    return this.cache.set(n, e), e;
  }
}, Kn = class {
  constructor(n, e, t) {
    g(this, "_cachedMatchRoot", new $a(
      (n) => this._root.match(n)
    ));
    this._colorMap = n, this._defaults = e, this._root = t;
  }
  static createFromRawTheme(n, e) {
    return this.createFromParsedTheme(jc(n), e);
  }
  static createFromParsedTheme(n, e) {
    return Hc(n, e);
  }
  getColorMap() {
    return this._colorMap.getColorMap();
  }
  getDefaults() {
    return this._defaults;
  }
  match(n) {
    if (n === null)
      return this._defaults;
    const e = n.scopeName, s = this._cachedMatchRoot.get(e).find(
      (r) => Gc(n.parent, r.parentScopes)
    );
    return s ? new Sa(
      s.fontStyle,
      s.foreground,
      s.background
    ) : null;
  }
}, js = class Gn {
  constructor(e, t) {
    this.parent = e, this.scopeName = t;
  }
  static push(e, t) {
    for (const s of t)
      e = new Gn(e, s);
    return e;
  }
  static from(...e) {
    let t = null;
    for (let s = 0; s < e.length; s++)
      t = new Gn(t, e[s]);
    return t;
  }
  push(e) {
    return new Gn(this, e);
  }
  getSegments() {
    let e = this;
    const t = [];
    for (; e; )
      t.push(e.scopeName), e = e.parent;
    return t.reverse(), t;
  }
  toString() {
    return this.getSegments().join(" ");
  }
  extends(e) {
    return this === e ? !0 : this.parent === null ? !1 : this.parent.extends(e);
  }
  getExtensionIfDefined(e) {
    const t = [];
    let s = this;
    for (; s && s !== e; )
      t.push(s.scopeName), s = s.parent;
    return s === e ? t.reverse() : void 0;
  }
};
function Gc(n, e) {
  if (e.length === 0)
    return !0;
  for (let t = 0; t < e.length; t++) {
    let s = e[t], r = !1;
    if (s === ">") {
      if (t === e.length - 1)
        return !1;
      s = e[++t], r = !0;
    }
    for (; n && !Uc(n.scopeName, s); ) {
      if (r)
        return !1;
      n = n.parent;
    }
    if (!n)
      return !1;
    n = n.parent;
  }
  return !0;
}
function Uc(n, e) {
  return e === n || n.startsWith(e) && n[e.length] === ".";
}
var Sa = class {
  constructor(n, e, t) {
    this.fontStyle = n, this.foregroundId = e, this.backgroundId = t;
  }
};
function jc(n) {
  if (!n)
    return [];
  if (!n.settings || !Array.isArray(n.settings))
    return [];
  let e = n.settings, t = [], s = 0;
  for (let r = 0, o = e.length; r < o; r++) {
    let i = e[r];
    if (!i.settings)
      continue;
    let a;
    if (typeof i.scope == "string") {
      let u = i.scope;
      u = u.replace(/^[,]+/, ""), u = u.replace(/[,]+$/, ""), a = u.split(",");
    } else Array.isArray(i.scope) ? a = i.scope : a = [""];
    let l = -1;
    if (typeof i.settings.fontStyle == "string") {
      l = 0;
      let u = i.settings.fontStyle.split(" ");
      for (let p = 0, d = u.length; p < d; p++)
        switch (u[p]) {
          case "italic":
            l = l | 1;
            break;
          case "bold":
            l = l | 2;
            break;
          case "underline":
            l = l | 4;
            break;
          case "strikethrough":
            l = l | 8;
            break;
        }
    }
    let c = null;
    typeof i.settings.foreground == "string" && li(i.settings.foreground) && (c = i.settings.foreground);
    let h = null;
    typeof i.settings.background == "string" && li(i.settings.background) && (h = i.settings.background);
    for (let u = 0, p = a.length; u < p; u++) {
      let f = a[u].trim().split(" "), b = f[f.length - 1], v = null;
      f.length > 1 && (v = f.slice(0, f.length - 1), v.reverse()), t[s++] = new qc(
        b,
        v,
        r,
        l,
        c,
        h
      );
    }
  }
  return t;
}
var qc = class {
  constructor(n, e, t, s, r, o) {
    this.scope = n, this.parentScopes = e, this.index = t, this.fontStyle = s, this.foreground = r, this.background = o;
  }
}, W = /* @__PURE__ */ ((n) => (n[n.NotSet = -1] = "NotSet", n[n.None = 0] = "None", n[n.Italic = 1] = "Italic", n[n.Bold = 2] = "Bold", n[n.Underline = 4] = "Underline", n[n.Strikethrough = 8] = "Strikethrough", n))(W || {});
function Hc(n, e) {
  n.sort((l, c) => {
    let h = xa(l.scope, c.scope);
    return h !== 0 || (h = ka(l.parentScopes, c.parentScopes), h !== 0) ? h : l.index - c.index;
  });
  let t = 0, s = "#000000", r = "#ffffff";
  for (; n.length >= 1 && n[0].scope === ""; ) {
    let l = n.shift();
    l.fontStyle !== -1 && (t = l.fontStyle), l.foreground !== null && (s = l.foreground), l.background !== null && (r = l.background);
  }
  let o = new Wc(e), i = new Sa(t, o.getId(s), o.getId(r)), a = new Vc(new ur(0, null, -1, 0, 0), []);
  for (let l = 0, c = n.length; l < c; l++) {
    let h = n[l];
    a.insert(0, h.scope, h.parentScopes, h.fontStyle, o.getId(h.foreground), o.getId(h.background));
  }
  return new Kn(o, i, a);
}
var Wc = class {
  constructor(n) {
    g(this, "_isFrozen");
    g(this, "_lastColorId");
    g(this, "_id2color");
    g(this, "_color2id");
    if (this._lastColorId = 0, this._id2color = [], this._color2id = /* @__PURE__ */ Object.create(null), Array.isArray(n)) {
      this._isFrozen = !0;
      for (let e = 0, t = n.length; e < t; e++)
        this._color2id[n[e]] = e, this._id2color[e] = n[e];
    } else
      this._isFrozen = !1;
  }
  getId(n) {
    if (n === null)
      return 0;
    n = n.toUpperCase();
    let e = this._color2id[n];
    if (e)
      return e;
    if (this._isFrozen)
      throw new Error(`Missing color in color map - ${n}`);
    return e = ++this._lastColorId, this._color2id[n] = e, this._id2color[e] = n, e;
  }
  getColorMap() {
    return this._id2color.slice(0);
  }
}, Qc = Object.freeze([]), ur = class Aa {
  constructor(e, t, s, r, o) {
    g(this, "scopeDepth");
    g(this, "parentScopes");
    g(this, "fontStyle");
    g(this, "foreground");
    g(this, "background");
    this.scopeDepth = e, this.parentScopes = t || Qc, this.fontStyle = s, this.foreground = r, this.background = o;
  }
  clone() {
    return new Aa(this.scopeDepth, this.parentScopes, this.fontStyle, this.foreground, this.background);
  }
  static cloneArr(e) {
    let t = [];
    for (let s = 0, r = e.length; s < r; s++)
      t[s] = e[s].clone();
    return t;
  }
  acceptOverwrite(e, t, s, r) {
    this.scopeDepth > e ? console.log("how did this happen?") : this.scopeDepth = e, t !== -1 && (this.fontStyle = t), s !== 0 && (this.foreground = s), r !== 0 && (this.background = r);
  }
}, Vc = class hr {
  constructor(e, t = [], s = {}) {
    g(this, "_rulesWithParentScopes");
    this._mainRule = e, this._children = s, this._rulesWithParentScopes = t;
  }
  static _cmpBySpecificity(e, t) {
    if (e.scopeDepth !== t.scopeDepth)
      return t.scopeDepth - e.scopeDepth;
    let s = 0, r = 0;
    for (; e.parentScopes[s] === ">" && s++, t.parentScopes[r] === ">" && r++, !(s >= e.parentScopes.length || r >= t.parentScopes.length); ) {
      const o = t.parentScopes[r].length - e.parentScopes[s].length;
      if (o !== 0)
        return o;
      s++, r++;
    }
    return t.parentScopes.length - e.parentScopes.length;
  }
  match(e) {
    if (e !== "") {
      let s = e.indexOf("."), r, o;
      if (s === -1 ? (r = e, o = "") : (r = e.substring(0, s), o = e.substring(s + 1)), this._children.hasOwnProperty(r))
        return this._children[r].match(o);
    }
    const t = this._rulesWithParentScopes.concat(this._mainRule);
    return t.sort(hr._cmpBySpecificity), t;
  }
  insert(e, t, s, r, o, i) {
    if (t === "") {
      this._doInsertHere(e, s, r, o, i);
      return;
    }
    let a = t.indexOf("."), l, c;
    a === -1 ? (l = t, c = "") : (l = t.substring(0, a), c = t.substring(a + 1));
    let h;
    this._children.hasOwnProperty(l) ? h = this._children[l] : (h = new hr(this._mainRule.clone(), ur.cloneArr(this._rulesWithParentScopes)), this._children[l] = h), h.insert(e + 1, c, s, r, o, i);
  }
  _doInsertHere(e, t, s, r, o) {
    if (t === null) {
      this._mainRule.acceptOverwrite(e, s, r, o);
      return;
    }
    for (let i = 0, a = this._rulesWithParentScopes.length; i < a; i++) {
      let l = this._rulesWithParentScopes[i];
      if (ka(l.parentScopes, t) === 0) {
        l.acceptOverwrite(e, s, r, o);
        return;
      }
    }
    s === -1 && (s = this._mainRule.fontStyle), r === 0 && (r = this._mainRule.foreground), o === 0 && (o = this._mainRule.background), this._rulesWithParentScopes.push(new ur(e, t, s, r, o));
  }
}, ot = class oe {
  static toBinaryStr(e) {
    return e.toString(2).padStart(32, "0");
  }
  static print(e) {
    const t = oe.getLanguageId(e), s = oe.getTokenType(e), r = oe.getFontStyle(e), o = oe.getForeground(e), i = oe.getBackground(e);
    console.log({
      languageId: t,
      tokenType: s,
      fontStyle: r,
      foreground: o,
      background: i
    });
  }
  static getLanguageId(e) {
    return (e & 255) >>> 0;
  }
  static getTokenType(e) {
    return (e & 768) >>> 8;
  }
  static containsBalancedBrackets(e) {
    return (e & 1024) !== 0;
  }
  static getFontStyle(e) {
    return (e & 30720) >>> 11;
  }
  static getForeground(e) {
    return (e & 16744448) >>> 15;
  }
  static getBackground(e) {
    return (e & 4278190080) >>> 24;
  }
  /**
   * Updates the fields in `metadata`.
   * A value of `0`, `NotSet` or `null` indicates that the corresponding field should be left as is.
   */
  static set(e, t, s, r, o, i, a) {
    let l = oe.getLanguageId(e), c = oe.getTokenType(e), h = oe.containsBalancedBrackets(e) ? 1 : 0, u = oe.getFontStyle(e), p = oe.getForeground(e), d = oe.getBackground(e);
    return t !== 0 && (l = t), s !== 8 && (c = s), r !== null && (h = r ? 1 : 0), o !== -1 && (u = o), i !== 0 && (p = i), a !== 0 && (d = a), (l << 0 | c << 8 | h << 10 | u << 11 | p << 15 | d << 24) >>> 0;
  }
};
function Zn(n, e) {
  const t = [], s = Kc(n);
  let r = s.next();
  for (; r !== null; ) {
    let l = 0;
    if (r.length === 2 && r.charAt(1) === ":") {
      switch (r.charAt(0)) {
        case "R":
          l = 1;
          break;
        case "L":
          l = -1;
          break;
        default:
          console.log(`Unknown priority ${r} in scope selector`);
      }
      r = s.next();
    }
    let c = i();
    if (t.push({ matcher: c, priority: l }), r !== ",")
      break;
    r = s.next();
  }
  return t;
  function o() {
    if (r === "-") {
      r = s.next();
      const l = o();
      return (c) => !!l && !l(c);
    }
    if (r === "(") {
      r = s.next();
      const l = a();
      return r === ")" && (r = s.next()), l;
    }
    if (ci(r)) {
      const l = [];
      do
        l.push(r), r = s.next();
      while (ci(r));
      return (c) => e(l, c);
    }
    return null;
  }
  function i() {
    const l = [];
    let c = o();
    for (; c; )
      l.push(c), c = o();
    return (h) => l.every((u) => u(h));
  }
  function a() {
    const l = [];
    let c = i();
    for (; c && (l.push(c), r === "|" || r === ","); ) {
      do
        r = s.next();
      while (r === "|" || r === ",");
      c = i();
    }
    return (h) => l.some((u) => u(h));
  }
}
function ci(n) {
  return !!n && !!n.match(/[\w\.:]+/);
}
function Kc(n) {
  let e = /([LR]:|[\w\.:][\w\.:\-]*|[\,\|\-\(\)])/g, t = e.exec(n);
  return {
    next: () => {
      if (!t)
        return null;
      const s = t[0];
      return t = e.exec(n), s;
    }
  };
}
function Ea(n) {
  typeof n.dispose == "function" && n.dispose();
}
var ln = class {
  constructor(n) {
    this.scopeName = n;
  }
  toKey() {
    return this.scopeName;
  }
}, Zc = class {
  constructor(n, e) {
    this.scopeName = n, this.ruleName = e;
  }
  toKey() {
    return `${this.scopeName}#${this.ruleName}`;
  }
}, Xc = class {
  constructor() {
    g(this, "_references", []);
    g(this, "_seenReferenceKeys", /* @__PURE__ */ new Set());
    g(this, "visitedRule", /* @__PURE__ */ new Set());
  }
  get references() {
    return this._references;
  }
  add(n) {
    const e = n.toKey();
    this._seenReferenceKeys.has(e) || (this._seenReferenceKeys.add(e), this._references.push(n));
  }
}, Jc = class {
  constructor(n, e) {
    g(this, "seenFullScopeRequests", /* @__PURE__ */ new Set());
    g(this, "seenPartialScopeRequests", /* @__PURE__ */ new Set());
    g(this, "Q");
    this.repo = n, this.initialScopeName = e, this.seenFullScopeRequests.add(this.initialScopeName), this.Q = [new ln(this.initialScopeName)];
  }
  processQueue() {
    const n = this.Q;
    this.Q = [];
    const e = new Xc();
    for (const t of n)
      Yc(t, this.initialScopeName, this.repo, e);
    for (const t of e.references)
      if (t instanceof ln) {
        if (this.seenFullScopeRequests.has(t.scopeName))
          continue;
        this.seenFullScopeRequests.add(t.scopeName), this.Q.push(t);
      } else {
        if (this.seenFullScopeRequests.has(t.scopeName) || this.seenPartialScopeRequests.has(t.toKey()))
          continue;
        this.seenPartialScopeRequests.add(t.toKey()), this.Q.push(t);
      }
  }
};
function Yc(n, e, t, s) {
  const r = t.lookup(n.scopeName);
  if (!r) {
    if (n.scopeName === e)
      throw new Error(`No grammar provided for <${e}>`);
    return;
  }
  const o = t.lookup(e);
  n instanceof ln ? Un({ baseGrammar: o, selfGrammar: r }, s) : pr(
    n.ruleName,
    { baseGrammar: o, selfGrammar: r, repository: r.repository },
    s
  );
  const i = t.injections(n.scopeName);
  if (i)
    for (const a of i)
      s.add(new ln(a));
}
function pr(n, e, t) {
  if (e.repository && e.repository[n]) {
    const s = e.repository[n];
    Xn([s], e, t);
  }
}
function Un(n, e) {
  n.selfGrammar.patterns && Array.isArray(n.selfGrammar.patterns) && Xn(
    n.selfGrammar.patterns,
    { ...n, repository: n.selfGrammar.repository },
    e
  ), n.selfGrammar.injections && Xn(
    Object.values(n.selfGrammar.injections),
    { ...n, repository: n.selfGrammar.repository },
    e
  );
}
function Xn(n, e, t) {
  for (const s of n) {
    if (t.visitedRule.has(s))
      continue;
    t.visitedRule.add(s);
    const r = s.repository ? _a({}, e.repository, s.repository) : e.repository;
    Array.isArray(s.patterns) && Xn(s.patterns, { ...e, repository: r }, t);
    const o = s.include;
    if (!o)
      continue;
    const i = Ra(o);
    switch (i.kind) {
      case 0:
        Un({ ...e, selfGrammar: e.baseGrammar }, t);
        break;
      case 1:
        Un(e, t);
        break;
      case 2:
        pr(i.ruleName, { ...e, repository: r }, t);
        break;
      case 3:
      case 4:
        const a = i.scopeName === e.selfGrammar.scopeName ? e.selfGrammar : i.scopeName === e.baseGrammar.scopeName ? e.baseGrammar : void 0;
        if (a) {
          const l = { baseGrammar: e.baseGrammar, selfGrammar: a, repository: r };
          i.kind === 4 ? pr(i.ruleName, l, t) : Un(l, t);
        } else
          i.kind === 4 ? t.add(new Zc(i.scopeName, i.ruleName)) : t.add(new ln(i.scopeName));
        break;
    }
  }
}
var eu = class {
  constructor() {
    g(this, "kind", 0);
  }
}, tu = class {
  constructor() {
    g(this, "kind", 1);
  }
}, nu = class {
  constructor(n) {
    g(this, "kind", 2);
    this.ruleName = n;
  }
}, su = class {
  constructor(n) {
    g(this, "kind", 3);
    this.scopeName = n;
  }
}, ru = class {
  constructor(n, e) {
    g(this, "kind", 4);
    this.scopeName = n, this.ruleName = e;
  }
};
function Ra(n) {
  if (n === "$base")
    return new eu();
  if (n === "$self")
    return new tu();
  const e = n.indexOf("#");
  if (e === -1)
    return new su(n);
  if (e === 0)
    return new nu(n.substring(1));
  {
    const t = n.substring(0, e), s = n.substring(e + 1);
    return new ru(t, s);
  }
}
var ou = /\\(\d+)/, ui = /\\(\d+)/g, iu = -1, Ta = -2;
var wn = class {
  constructor(n, e, t, s) {
    g(this, "$location");
    g(this, "id");
    g(this, "_nameIsCapturing");
    g(this, "_name");
    g(this, "_contentNameIsCapturing");
    g(this, "_contentName");
    this.$location = n, this.id = e, this._name = t || null, this._nameIsCapturing = Pn.hasCaptures(this._name), this._contentName = s || null, this._contentNameIsCapturing = Pn.hasCaptures(this._contentName);
  }
  get debugName() {
    const n = this.$location ? `${wa(this.$location.filename)}:${this.$location.line}` : "unknown";
    return `${this.constructor.name}#${this.id} @ ${n}`;
  }
  getName(n, e) {
    return !this._nameIsCapturing || this._name === null || n === null || e === null ? this._name : Pn.replaceCaptures(this._name, n, e);
  }
  getContentName(n, e) {
    return !this._contentNameIsCapturing || this._contentName === null ? this._contentName : Pn.replaceCaptures(this._contentName, n, e);
  }
}, au = class extends wn {
  constructor(e, t, s, r, o) {
    super(e, t, s, r);
    g(this, "retokenizeCapturedWithRuleId");
    this.retokenizeCapturedWithRuleId = o;
  }
  dispose() {
  }
  collectPatterns(e, t) {
    throw new Error("Not supported!");
  }
  compile(e, t) {
    throw new Error("Not supported!");
  }
  compileAG(e, t, s, r) {
    throw new Error("Not supported!");
  }
}, lu = class extends wn {
  constructor(e, t, s, r, o) {
    super(e, t, s, null);
    g(this, "_match");
    g(this, "captures");
    g(this, "_cachedCompiledPatterns");
    this._match = new cn(r, this.id), this.captures = o, this._cachedCompiledPatterns = null;
  }
  dispose() {
    this._cachedCompiledPatterns && (this._cachedCompiledPatterns.dispose(), this._cachedCompiledPatterns = null);
  }
  get debugMatchRegExp() {
    return `${this._match.source}`;
  }
  collectPatterns(e, t) {
    t.push(this._match);
  }
  compile(e, t) {
    return this._getCachedCompiledPatterns(e).compile(e);
  }
  compileAG(e, t, s, r) {
    return this._getCachedCompiledPatterns(e).compileAG(e, s, r);
  }
  _getCachedCompiledPatterns(e) {
    return this._cachedCompiledPatterns || (this._cachedCompiledPatterns = new un(), this.collectPatterns(e, this._cachedCompiledPatterns)), this._cachedCompiledPatterns;
  }
}, hi = class extends wn {
  constructor(e, t, s, r, o) {
    super(e, t, s, r);
    g(this, "hasMissingPatterns");
    g(this, "patterns");
    g(this, "_cachedCompiledPatterns");
    this.patterns = o.patterns, this.hasMissingPatterns = o.hasMissingPatterns, this._cachedCompiledPatterns = null;
  }
  dispose() {
    this._cachedCompiledPatterns && (this._cachedCompiledPatterns.dispose(), this._cachedCompiledPatterns = null);
  }
  collectPatterns(e, t) {
    for (const s of this.patterns)
      e.getRule(s).collectPatterns(e, t);
  }
  compile(e, t) {
    return this._getCachedCompiledPatterns(e).compile(e);
  }
  compileAG(e, t, s, r) {
    return this._getCachedCompiledPatterns(e).compileAG(e, s, r);
  }
  _getCachedCompiledPatterns(e) {
    return this._cachedCompiledPatterns || (this._cachedCompiledPatterns = new un(), this.collectPatterns(e, this._cachedCompiledPatterns)), this._cachedCompiledPatterns;
  }
}, dr = class extends wn {
  constructor(e, t, s, r, o, i, a, l, c, h) {
    super(e, t, s, r);
    g(this, "_begin");
    g(this, "beginCaptures");
    g(this, "_end");
    g(this, "endHasBackReferences");
    g(this, "endCaptures");
    g(this, "applyEndPatternLast");
    g(this, "hasMissingPatterns");
    g(this, "patterns");
    g(this, "_cachedCompiledPatterns");
    this._begin = new cn(o, this.id), this.beginCaptures = i, this._end = new cn(a || "￿", -1), this.endHasBackReferences = this._end.hasBackReferences, this.endCaptures = l, this.applyEndPatternLast = c || !1, this.patterns = h.patterns, this.hasMissingPatterns = h.hasMissingPatterns, this._cachedCompiledPatterns = null;
  }
  dispose() {
    this._cachedCompiledPatterns && (this._cachedCompiledPatterns.dispose(), this._cachedCompiledPatterns = null);
  }
  get debugBeginRegExp() {
    return `${this._begin.source}`;
  }
  get debugEndRegExp() {
    return `${this._end.source}`;
  }
  getEndWithResolvedBackReferences(e, t) {
    return this._end.resolveBackReferences(e, t);
  }
  collectPatterns(e, t) {
    t.push(this._begin);
  }
  compile(e, t) {
    return this._getCachedCompiledPatterns(e, t).compile(e);
  }
  compileAG(e, t, s, r) {
    return this._getCachedCompiledPatterns(e, t).compileAG(e, s, r);
  }
  _getCachedCompiledPatterns(e, t) {
    if (!this._cachedCompiledPatterns) {
      this._cachedCompiledPatterns = new un();
      for (const s of this.patterns)
        e.getRule(s).collectPatterns(e, this._cachedCompiledPatterns);
      this.applyEndPatternLast ? this._cachedCompiledPatterns.push(this._end.hasBackReferences ? this._end.clone() : this._end) : this._cachedCompiledPatterns.unshift(this._end.hasBackReferences ? this._end.clone() : this._end);
    }
    return this._end.hasBackReferences && (this.applyEndPatternLast ? this._cachedCompiledPatterns.setSource(this._cachedCompiledPatterns.length() - 1, t) : this._cachedCompiledPatterns.setSource(0, t)), this._cachedCompiledPatterns;
  }
}, Jn = class extends wn {
  constructor(e, t, s, r, o, i, a, l, c) {
    super(e, t, s, r);
    g(this, "_begin");
    g(this, "beginCaptures");
    g(this, "whileCaptures");
    g(this, "_while");
    g(this, "whileHasBackReferences");
    g(this, "hasMissingPatterns");
    g(this, "patterns");
    g(this, "_cachedCompiledPatterns");
    g(this, "_cachedCompiledWhilePatterns");
    this._begin = new cn(o, this.id), this.beginCaptures = i, this.whileCaptures = l, this._while = new cn(a, Ta), this.whileHasBackReferences = this._while.hasBackReferences, this.patterns = c.patterns, this.hasMissingPatterns = c.hasMissingPatterns, this._cachedCompiledPatterns = null, this._cachedCompiledWhilePatterns = null;
  }
  dispose() {
    this._cachedCompiledPatterns && (this._cachedCompiledPatterns.dispose(), this._cachedCompiledPatterns = null), this._cachedCompiledWhilePatterns && (this._cachedCompiledWhilePatterns.dispose(), this._cachedCompiledWhilePatterns = null);
  }
  get debugBeginRegExp() {
    return `${this._begin.source}`;
  }
  get debugWhileRegExp() {
    return `${this._while.source}`;
  }
  getWhileWithResolvedBackReferences(e, t) {
    return this._while.resolveBackReferences(e, t);
  }
  collectPatterns(e, t) {
    t.push(this._begin);
  }
  compile(e, t) {
    return this._getCachedCompiledPatterns(e).compile(e);
  }
  compileAG(e, t, s, r) {
    return this._getCachedCompiledPatterns(e).compileAG(e, s, r);
  }
  _getCachedCompiledPatterns(e) {
    if (!this._cachedCompiledPatterns) {
      this._cachedCompiledPatterns = new un();
      for (const t of this.patterns)
        e.getRule(t).collectPatterns(e, this._cachedCompiledPatterns);
    }
    return this._cachedCompiledPatterns;
  }
  compileWhile(e, t) {
    return this._getCachedCompiledWhilePatterns(e, t).compile(e);
  }
  compileWhileAG(e, t, s, r) {
    return this._getCachedCompiledWhilePatterns(e, t).compileAG(e, s, r);
  }
  _getCachedCompiledWhilePatterns(e, t) {
    return this._cachedCompiledWhilePatterns || (this._cachedCompiledWhilePatterns = new un(), this._cachedCompiledWhilePatterns.push(this._while.hasBackReferences ? this._while.clone() : this._while)), this._while.hasBackReferences && this._cachedCompiledWhilePatterns.setSource(0, t || "￿"), this._cachedCompiledWhilePatterns;
  }
}, Ia = class H {
  static createCaptureRule(e, t, s, r, o) {
    return e.registerRule((i) => new au(t, i, s, r, o));
  }
  static getCompiledRuleId(e, t, s) {
    return e.id || t.registerRule((r) => {
      if (e.id = r, e.match)
        return new lu(
          e.$vscodeTextmateLocation,
          e.id,
          e.name,
          e.match,
          H._compileCaptures(e.captures, t, s)
        );
      if (typeof e.begin > "u") {
        e.repository && (s = _a({}, s, e.repository));
        let o = e.patterns;
        return typeof o > "u" && e.include && (o = [{ include: e.include }]), new hi(
          e.$vscodeTextmateLocation,
          e.id,
          e.name,
          e.contentName,
          H._compilePatterns(o, t, s)
        );
      }
      return e.while ? new Jn(
        e.$vscodeTextmateLocation,
        e.id,
        e.name,
        e.contentName,
        e.begin,
        H._compileCaptures(e.beginCaptures || e.captures, t, s),
        e.while,
        H._compileCaptures(e.whileCaptures || e.captures, t, s),
        H._compilePatterns(e.patterns, t, s)
      ) : new dr(
        e.$vscodeTextmateLocation,
        e.id,
        e.name,
        e.contentName,
        e.begin,
        H._compileCaptures(e.beginCaptures || e.captures, t, s),
        e.end,
        H._compileCaptures(e.endCaptures || e.captures, t, s),
        e.applyEndPatternLast,
        H._compilePatterns(e.patterns, t, s)
      );
    }), e.id;
  }
  static _compileCaptures(e, t, s) {
    let r = [];
    if (e) {
      let o = 0;
      for (const i in e) {
        if (i === "$vscodeTextmateLocation")
          continue;
        const a = parseInt(i, 10);
        a > o && (o = a);
      }
      for (let i = 0; i <= o; i++)
        r[i] = null;
      for (const i in e) {
        if (i === "$vscodeTextmateLocation")
          continue;
        const a = parseInt(i, 10);
        let l = 0;
        e[i].patterns && (l = H.getCompiledRuleId(e[i], t, s)), r[a] = H.createCaptureRule(t, e[i].$vscodeTextmateLocation, e[i].name, e[i].contentName, l);
      }
    }
    return r;
  }
  static _compilePatterns(e, t, s) {
    let r = [];
    if (e)
      for (let o = 0, i = e.length; o < i; o++) {
        const a = e[o];
        let l = -1;
        if (a.include) {
          const c = Ra(a.include);
          switch (c.kind) {
            case 0:
            case 1:
              l = H.getCompiledRuleId(s[a.include], t, s);
              break;
            case 2:
              let h = s[c.ruleName];
              h && (l = H.getCompiledRuleId(h, t, s));
              break;
            case 3:
            case 4:
              const u = c.scopeName, p = c.kind === 4 ? c.ruleName : null, d = t.getExternalGrammar(u, s);
              if (d)
                if (p) {
                  let f = d.repository[p];
                  f && (l = H.getCompiledRuleId(f, t, d.repository));
                } else
                  l = H.getCompiledRuleId(d.repository.$self, t, d.repository);
              break;
          }
        } else
          l = H.getCompiledRuleId(a, t, s);
        if (l !== -1) {
          const c = t.getRule(l);
          let h = !1;
          if ((c instanceof hi || c instanceof dr || c instanceof Jn) && c.hasMissingPatterns && c.patterns.length === 0 && (h = !0), h)
            continue;
          r.push(l);
        }
      }
    return {
      patterns: r,
      hasMissingPatterns: (e ? e.length : 0) !== r.length
    };
  }
}, cn = class Pa {
  constructor(e, t) {
    g(this, "source");
    g(this, "ruleId");
    g(this, "hasAnchor");
    g(this, "hasBackReferences");
    g(this, "_anchorCache");
    if (e && typeof e == "string") {
      const s = e.length;
      let r = 0, o = [], i = !1;
      for (let a = 0; a < s; a++)
        if (e.charAt(a) === "\\" && a + 1 < s) {
          const c = e.charAt(a + 1);
          c === "z" ? (o.push(e.substring(r, a)), o.push("$(?!\\n)(?<!\\n)"), r = a + 2) : (c === "A" || c === "G") && (i = !0), a++;
        }
      this.hasAnchor = i, r === 0 ? this.source = e : (o.push(e.substring(r, s)), this.source = o.join(""));
    } else
      this.hasAnchor = !1, this.source = e;
    this.hasAnchor ? this._anchorCache = this._buildAnchorCache() : this._anchorCache = null, this.ruleId = t, typeof this.source == "string" ? this.hasBackReferences = ou.test(this.source) : this.hasBackReferences = !1;
  }
  clone() {
    return new Pa(this.source, this.ruleId);
  }
  setSource(e) {
    this.source !== e && (this.source = e, this.hasAnchor && (this._anchorCache = this._buildAnchorCache()));
  }
  resolveBackReferences(e, t) {
    if (typeof this.source != "string")
      throw new Error("This method should only be called if the source is a string");
    let s = t.map((r) => e.substring(r.start, r.end));
    return ui.lastIndex = 0, this.source.replace(ui, (r, o) => Ca(s[parseInt(o, 10)] || ""));
  }
  _buildAnchorCache() {
    if (typeof this.source != "string")
      throw new Error("This method should only be called if the source is a string");
    let e = [], t = [], s = [], r = [], o, i, a, l;
    for (o = 0, i = this.source.length; o < i; o++)
      a = this.source.charAt(o), e[o] = a, t[o] = a, s[o] = a, r[o] = a, a === "\\" && o + 1 < i && (l = this.source.charAt(o + 1), l === "A" ? (e[o + 1] = "￿", t[o + 1] = "￿", s[o + 1] = "A", r[o + 1] = "A") : l === "G" ? (e[o + 1] = "￿", t[o + 1] = "G", s[o + 1] = "￿", r[o + 1] = "G") : (e[o + 1] = l, t[o + 1] = l, s[o + 1] = l, r[o + 1] = l), o++);
    return {
      A0_G0: e.join(""),
      A0_G1: t.join(""),
      A1_G0: s.join(""),
      A1_G1: r.join("")
    };
  }
  resolveAnchors(e, t) {
    return !this.hasAnchor || !this._anchorCache || typeof this.source != "string" ? this.source : e ? t ? this._anchorCache.A1_G1 : this._anchorCache.A1_G0 : t ? this._anchorCache.A0_G1 : this._anchorCache.A0_G0;
  }
}, un = class {
  constructor() {
    g(this, "_items");
    g(this, "_hasAnchors");
    g(this, "_cached");
    g(this, "_anchorCache");
    this._items = [], this._hasAnchors = !1, this._cached = null, this._anchorCache = {
      A0_G0: null,
      A0_G1: null,
      A1_G0: null,
      A1_G1: null
    };
  }
  dispose() {
    this._disposeCaches();
  }
  _disposeCaches() {
    this._cached && (this._cached.dispose(), this._cached = null), this._anchorCache.A0_G0 && (this._anchorCache.A0_G0.dispose(), this._anchorCache.A0_G0 = null), this._anchorCache.A0_G1 && (this._anchorCache.A0_G1.dispose(), this._anchorCache.A0_G1 = null), this._anchorCache.A1_G0 && (this._anchorCache.A1_G0.dispose(), this._anchorCache.A1_G0 = null), this._anchorCache.A1_G1 && (this._anchorCache.A1_G1.dispose(), this._anchorCache.A1_G1 = null);
  }
  push(n) {
    this._items.push(n), this._hasAnchors = this._hasAnchors || n.hasAnchor;
  }
  unshift(n) {
    this._items.unshift(n), this._hasAnchors = this._hasAnchors || n.hasAnchor;
  }
  length() {
    return this._items.length;
  }
  setSource(n, e) {
    this._items[n].source !== e && (this._disposeCaches(), this._items[n].setSource(e));
  }
  compile(n) {
    if (!this._cached) {
      let e = this._items.map((t) => t.source);
      this._cached = new pi(n, e, this._items.map((t) => t.ruleId));
    }
    return this._cached;
  }
  compileAG(n, e, t) {
    return this._hasAnchors ? e ? t ? (this._anchorCache.A1_G1 || (this._anchorCache.A1_G1 = this._resolveAnchors(n, e, t)), this._anchorCache.A1_G1) : (this._anchorCache.A1_G0 || (this._anchorCache.A1_G0 = this._resolveAnchors(n, e, t)), this._anchorCache.A1_G0) : t ? (this._anchorCache.A0_G1 || (this._anchorCache.A0_G1 = this._resolveAnchors(n, e, t)), this._anchorCache.A0_G1) : (this._anchorCache.A0_G0 || (this._anchorCache.A0_G0 = this._resolveAnchors(n, e, t)), this._anchorCache.A0_G0) : this.compile(n);
  }
  _resolveAnchors(n, e, t) {
    let s = this._items.map((r) => r.resolveAnchors(e, t));
    return new pi(n, s, this._items.map((r) => r.ruleId));
  }
}, pi = class {
  constructor(n, e, t) {
    g(this, "scanner");
    this.regExps = e, this.rules = t, this.scanner = n.createOnigScanner(e);
  }
  dispose() {
    typeof this.scanner.dispose == "function" && this.scanner.dispose();
  }
  toString() {
    const n = [];
    for (let e = 0, t = this.rules.length; e < t; e++)
      n.push("   - " + this.rules[e] + ": " + this.regExps[e]);
    return n.join(`
`);
  }
  findNextMatchSync(n, e, t) {
    const s = this.scanner.findNextMatchSync(n, e, t);
    return s ? {
      ruleId: this.rules[s.index],
      captureIndices: s.captureIndices
    } : null;
  }
}, qs = class {
  constructor(n, e) {
    this.languageId = n, this.tokenType = e;
  }
}, Se, cu = (Se = class {
  constructor(e, t) {
    g(this, "_defaultAttributes");
    g(this, "_embeddedLanguagesMatcher");
    g(this, "_getBasicScopeAttributes", new $a((e) => {
      const t = this._scopeToLanguage(e), s = this._toStandardTokenType(e);
      return new qs(t, s);
    }));
    this._defaultAttributes = new qs(
      e,
      8
      /* NotSet */
    ), this._embeddedLanguagesMatcher = new uu(Object.entries(t || {}));
  }
  getDefaultAttributes() {
    return this._defaultAttributes;
  }
  getBasicScopeAttributes(e) {
    return e === null ? Se._NULL_SCOPE_METADATA : this._getBasicScopeAttributes.get(e);
  }
  /**
   * Given a produced TM scope, return the language that token describes or null if unknown.
   * e.g. source.html => html, source.css.embedded.html => css, punctuation.definition.tag.html => null
   */
  _scopeToLanguage(e) {
    return this._embeddedLanguagesMatcher.match(e) || 0;
  }
  _toStandardTokenType(e) {
    const t = e.match(Se.STANDARD_TOKEN_TYPE_REGEXP);
    if (!t)
      return 8;
    switch (t[1]) {
      case "comment":
        return 1;
      case "string":
        return 2;
      case "regex":
        return 3;
      case "meta.embedded":
        return 0;
    }
    throw new Error("Unexpected match for standard token type!");
  }
}, g(Se, "_NULL_SCOPE_METADATA", new qs(0, 0)), g(Se, "STANDARD_TOKEN_TYPE_REGEXP", /\b(comment|string|regex|meta\.embedded)\b/), Se), uu = class {
  constructor(n) {
    g(this, "values");
    g(this, "scopesRegExp");
    if (n.length === 0)
      this.values = null, this.scopesRegExp = null;
    else {
      this.values = new Map(n);
      const e = n.map(
        ([t, s]) => Ca(t)
      );
      e.sort(), e.reverse(), this.scopesRegExp = new RegExp(
        `^((${e.join(")|(")}))($|\\.)`,
        ""
      );
    }
  }
  match(n) {
    if (!this.scopesRegExp)
      return;
    const e = n.match(this.scopesRegExp);
    if (e)
      return this.values.get(e[1]);
  }
};
typeof process < "u" && process.env.VSCODE_TEXTMATE_DEBUG;
var di = class {
  constructor(n, e) {
    this.stack = n, this.stoppedEarly = e;
  }
};
function La(n, e, t, s, r, o, i, a) {
  const l = e.content.length;
  let c = !1, h = -1;
  if (i) {
    const d = hu(
      n,
      e,
      t,
      s,
      r,
      o
    );
    r = d.stack, s = d.linePos, t = d.isFirstLine, h = d.anchorPosition;
  }
  const u = Date.now();
  for (; !c; ) {
    if (a !== 0 && Date.now() - u > a)
      return new di(r, !0);
    p();
  }
  return new di(r, !1);
  function p() {
    const d = pu(
      n,
      e,
      t,
      s,
      r,
      h
    );
    if (!d) {
      o.produce(r, l), c = !0;
      return;
    }
    const f = d.captureIndices, b = d.matchedRuleId, v = f && f.length > 0 ? f[0].end > s : !1;
    if (b === iu) {
      const k = r.getRule(n);
      o.produce(r, f[0].start), r = r.withContentNameScopesList(r.nameScopesList), Vt(
        n,
        e,
        t,
        r,
        o,
        k.endCaptures,
        f
      ), o.produce(r, f[0].end);
      const _ = r;
      if (r = r.parent, h = _.getAnchorPos(), !v && _.getEnterPos() === s) {
        r = _, o.produce(r, l), c = !0;
        return;
      }
    } else {
      const k = n.getRule(b);
      o.produce(r, f[0].start);
      const _ = r, x = k.getName(e.content, f), $ = r.contentNameScopesList.pushAttributed(
        x,
        n
      );
      if (r = r.push(
        b,
        s,
        h,
        f[0].end === l,
        null,
        $,
        $
      ), k instanceof dr) {
        const A = k;
        Vt(
          n,
          e,
          t,
          r,
          o,
          A.beginCaptures,
          f
        ), o.produce(r, f[0].end), h = f[0].end;
        const T = A.getContentName(
          e.content,
          f
        ), M = $.pushAttributed(
          T,
          n
        );
        if (r = r.withContentNameScopesList(M), A.endHasBackReferences && (r = r.withEndRule(
          A.getEndWithResolvedBackReferences(
            e.content,
            f
          )
        )), !v && _.hasSameRuleAs(r)) {
          r = r.pop(), o.produce(r, l), c = !0;
          return;
        }
      } else if (k instanceof Jn) {
        const A = k;
        Vt(
          n,
          e,
          t,
          r,
          o,
          A.beginCaptures,
          f
        ), o.produce(r, f[0].end), h = f[0].end;
        const T = A.getContentName(
          e.content,
          f
        ), M = $.pushAttributed(
          T,
          n
        );
        if (r = r.withContentNameScopesList(M), A.whileHasBackReferences && (r = r.withEndRule(
          A.getWhileWithResolvedBackReferences(
            e.content,
            f
          )
        )), !v && _.hasSameRuleAs(r)) {
          r = r.pop(), o.produce(r, l), c = !0;
          return;
        }
      } else if (Vt(
        n,
        e,
        t,
        r,
        o,
        k.captures,
        f
      ), o.produce(r, f[0].end), r = r.pop(), !v) {
        r = r.safePop(), o.produce(r, l), c = !0;
        return;
      }
    }
    f[0].end > s && (s = f[0].end, t = !1);
  }
}
function hu(n, e, t, s, r, o) {
  let i = r.beginRuleCapturedEOL ? 0 : -1;
  const a = [];
  for (let l = r; l; l = l.pop()) {
    const c = l.getRule(n);
    c instanceof Jn && a.push({
      rule: c,
      stack: l
    });
  }
  for (let l = a.pop(); l; l = a.pop()) {
    const { ruleScanner: c, findOptions: h } = gu(l.rule, n, l.stack.endRule, t, s === i), u = c.findNextMatchSync(e, s, h);
    if (u) {
      if (u.ruleId !== Ta) {
        r = l.stack.pop();
        break;
      }
      u.captureIndices && u.captureIndices.length && (o.produce(l.stack, u.captureIndices[0].start), Vt(n, e, t, l.stack, o, l.rule.whileCaptures, u.captureIndices), o.produce(l.stack, u.captureIndices[0].end), i = u.captureIndices[0].end, u.captureIndices[0].end > s && (s = u.captureIndices[0].end, t = !1));
    } else {
      r = l.stack.pop();
      break;
    }
  }
  return { stack: r, linePos: s, anchorPosition: i, isFirstLine: t };
}
function pu(n, e, t, s, r, o) {
  const i = du(n, e, t, s, r, o), a = n.getInjections();
  if (a.length === 0)
    return i;
  const l = fu(a, n, e, t, s, r, o);
  if (!l)
    return i;
  if (!i)
    return l;
  const c = i.captureIndices[0].start, h = l.captureIndices[0].start;
  return h < c || l.priorityMatch && h === c ? l : i;
}
function du(n, e, t, s, r, o) {
  const i = r.getRule(n), { ruleScanner: a, findOptions: l } = Na(i, n, r.endRule, t, s === o), c = a.findNextMatchSync(e, s, l);
  return c ? {
    captureIndices: c.captureIndices,
    matchedRuleId: c.ruleId
  } : null;
}
function fu(n, e, t, s, r, o, i) {
  let a = Number.MAX_VALUE, l = null, c, h = 0;
  const u = o.contentNameScopesList.getScopeNames();
  for (let p = 0, d = n.length; p < d; p++) {
    const f = n[p];
    if (!f.matcher(u))
      continue;
    const b = e.getRule(f.ruleId), { ruleScanner: v, findOptions: k } = Na(b, e, null, s, r === i), _ = v.findNextMatchSync(t, r, k);
    if (!_)
      continue;
    const x = _.captureIndices[0].start;
    if (!(x >= a) && (a = x, l = _.captureIndices, c = _.ruleId, h = f.priority, a === r))
      break;
  }
  return l ? {
    priorityMatch: h === -1,
    captureIndices: l,
    matchedRuleId: c
  } : null;
}
function Na(n, e, t, s, r) {
  return {
    ruleScanner: n.compileAG(e, t, s, r),
    findOptions: 0
    /* None */
  };
}
function gu(n, e, t, s, r) {
  return {
    ruleScanner: n.compileWhileAG(e, t, s, r),
    findOptions: 0
    /* None */
  };
}
function Vt(n, e, t, s, r, o, i) {
  if (o.length === 0)
    return;
  const a = e.content, l = Math.min(o.length, i.length), c = [], h = i[0].end;
  for (let u = 0; u < l; u++) {
    const p = o[u];
    if (p === null)
      continue;
    const d = i[u];
    if (d.length === 0)
      continue;
    if (d.start > h)
      break;
    for (; c.length > 0 && c[c.length - 1].endPos <= d.start; )
      r.produceFromScopes(c[c.length - 1].scopes, c[c.length - 1].endPos), c.pop();
    if (c.length > 0 ? r.produceFromScopes(c[c.length - 1].scopes, d.start) : r.produce(s, d.start), p.retokenizeCapturedWithRuleId) {
      const b = p.getName(a, i), v = s.contentNameScopesList.pushAttributed(b, n), k = p.getContentName(a, i), _ = v.pushAttributed(k, n), x = s.push(p.retokenizeCapturedWithRuleId, d.start, -1, !1, null, v, _), $ = n.createOnigString(a.substring(0, d.end));
      La(
        n,
        $,
        t && d.start === 0,
        d.start,
        x,
        r,
        !1,
        /* no time limit */
        0
      ), Ea($);
      continue;
    }
    const f = p.getName(a, i);
    if (f !== null) {
      const v = (c.length > 0 ? c[c.length - 1].scopes : s.contentNameScopesList).pushAttributed(f, n);
      c.push(new mu(v, d.end));
    }
  }
  for (; c.length > 0; )
    r.produceFromScopes(c[c.length - 1].scopes, c[c.length - 1].endPos), c.pop();
}
var mu = class {
  constructor(n, e) {
    g(this, "scopes");
    g(this, "endPos");
    this.scopes = n, this.endPos = e;
  }
};
function bu(n, e, t, s, r, o, i, a) {
  return new vu(
    n,
    e,
    t,
    s,
    r,
    o,
    i,
    a
  );
}
function fi(n, e, t, s, r) {
  const o = Zn(e, Yn), i = Ia.getCompiledRuleId(t, s, r.repository);
  for (const a of o)
    n.push({
      debugSelector: e,
      matcher: a.matcher,
      ruleId: i,
      grammar: r,
      priority: a.priority
    });
}
function Yn(n, e) {
  if (e.length < n.length)
    return !1;
  let t = 0;
  return n.every((s) => {
    for (let r = t; r < e.length; r++)
      if (yu(e[r], s))
        return t = r + 1, !0;
    return !1;
  });
}
function yu(n, e) {
  if (!n)
    return !1;
  if (n === e)
    return !0;
  const t = e.length;
  return n.length > t && n.substr(0, t) === e && n[t] === ".";
}
var vu = class {
  constructor(n, e, t, s, r, o, i, a) {
    g(this, "_rootId");
    g(this, "_lastRuleId");
    g(this, "_ruleId2desc");
    g(this, "_includedGrammars");
    g(this, "_grammarRepository");
    g(this, "_grammar");
    g(this, "_injections");
    g(this, "_basicScopeAttributesProvider");
    g(this, "_tokenTypeMatchers");
    if (this._rootScopeName = n, this.balancedBracketSelectors = o, this._onigLib = a, this._basicScopeAttributesProvider = new cu(
      t,
      s
    ), this._rootId = -1, this._lastRuleId = 0, this._ruleId2desc = [null], this._includedGrammars = {}, this._grammarRepository = i, this._grammar = gi(e, null), this._injections = null, this._tokenTypeMatchers = [], r)
      for (const l of Object.keys(r)) {
        const c = Zn(l, Yn);
        for (const h of c)
          this._tokenTypeMatchers.push({
            matcher: h.matcher,
            type: r[l]
          });
      }
  }
  get themeProvider() {
    return this._grammarRepository;
  }
  dispose() {
    for (const n of this._ruleId2desc)
      n && n.dispose();
  }
  createOnigScanner(n) {
    return this._onigLib.createOnigScanner(n);
  }
  createOnigString(n) {
    return this._onigLib.createOnigString(n);
  }
  getMetadataForScope(n) {
    return this._basicScopeAttributesProvider.getBasicScopeAttributes(n);
  }
  _collectInjections() {
    const n = {
      lookup: (r) => r === this._rootScopeName ? this._grammar : this.getExternalGrammar(r),
      injections: (r) => this._grammarRepository.injections(r)
    }, e = [], t = this._rootScopeName, s = n.lookup(t);
    if (s) {
      const r = s.injections;
      if (r)
        for (let i in r)
          fi(
            e,
            i,
            r[i],
            this,
            s
          );
      const o = this._grammarRepository.injections(t);
      o && o.forEach((i) => {
        const a = this.getExternalGrammar(i);
        if (a) {
          const l = a.injectionSelector;
          l && fi(
            e,
            l,
            a,
            this,
            a
          );
        }
      });
    }
    return e.sort((r, o) => r.priority - o.priority), e;
  }
  getInjections() {
    return this._injections === null && (this._injections = this._collectInjections()), this._injections;
  }
  registerRule(n) {
    const e = ++this._lastRuleId, t = n(e);
    return this._ruleId2desc[e] = t, t;
  }
  getRule(n) {
    return this._ruleId2desc[n];
  }
  getExternalGrammar(n, e) {
    if (this._includedGrammars[n])
      return this._includedGrammars[n];
    if (this._grammarRepository) {
      const t = this._grammarRepository.lookup(n);
      if (t)
        return this._includedGrammars[n] = gi(
          t,
          e && e.$base
        ), this._includedGrammars[n];
    }
  }
  tokenizeLine(n, e, t = 0) {
    const s = this._tokenize(n, e, !1, t);
    return {
      tokens: s.lineTokens.getResult(s.ruleStack, s.lineLength),
      ruleStack: s.ruleStack,
      stoppedEarly: s.stoppedEarly
    };
  }
  tokenizeLine2(n, e, t = 0) {
    const s = this._tokenize(n, e, !0, t);
    return {
      tokens: s.lineTokens.getBinaryResult(s.ruleStack, s.lineLength),
      ruleStack: s.ruleStack,
      stoppedEarly: s.stoppedEarly
    };
  }
  _tokenize(n, e, t, s) {
    this._rootId === -1 && (this._rootId = Ia.getCompiledRuleId(
      this._grammar.repository.$self,
      this,
      this._grammar.repository
    ), this.getInjections());
    let r;
    if (!e || e === fr.NULL) {
      r = !0;
      const c = this._basicScopeAttributesProvider.getDefaultAttributes(), h = this.themeProvider.getDefaults(), u = ot.set(
        0,
        c.languageId,
        c.tokenType,
        null,
        h.fontStyle,
        h.foregroundId,
        h.backgroundId
      ), p = this.getRule(this._rootId).getName(
        null,
        null
      );
      let d;
      p ? d = Xt.createRootAndLookUpScopeName(
        p,
        u,
        this
      ) : d = Xt.createRoot(
        "unknown",
        u
      ), e = new fr(
        null,
        this._rootId,
        -1,
        -1,
        !1,
        null,
        d,
        d
      );
    } else
      r = !1, e.reset();
    n = n + `
`;
    const o = this.createOnigString(n), i = o.content.length, a = new wu(
      t,
      n,
      this._tokenTypeMatchers,
      this.balancedBracketSelectors
    ), l = La(
      this,
      o,
      r,
      0,
      e,
      a,
      !0,
      s
    );
    return Ea(o), {
      lineLength: i,
      lineTokens: a,
      ruleStack: l.stack,
      stoppedEarly: l.stoppedEarly
    };
  }
};
function gi(n, e) {
  return n = Bc(n), n.repository = n.repository || {}, n.repository.$self = {
    $vscodeTextmateLocation: n.$vscodeTextmateLocation,
    patterns: n.patterns,
    name: n.scopeName
  }, n.repository.$base = e || n.repository.$self, n;
}
var Xt = class me {
  /**
   * Invariant:
   * ```
   * if (parent && !scopePath.extends(parent.scopePath)) {
   * 	throw new Error();
   * }
   * ```
   */
  constructor(e, t, s) {
    this.parent = e, this.scopePath = t, this.tokenAttributes = s;
  }
  static fromExtension(e, t) {
    let s = e, r = (e == null ? void 0 : e.scopePath) ?? null;
    for (const o of t)
      r = js.push(r, o.scopeNames), s = new me(s, r, o.encodedTokenAttributes);
    return s;
  }
  static createRoot(e, t) {
    return new me(null, new js(null, e), t);
  }
  static createRootAndLookUpScopeName(e, t, s) {
    const r = s.getMetadataForScope(e), o = new js(null, e), i = s.themeProvider.themeMatch(o), a = me.mergeAttributes(
      t,
      r,
      i
    );
    return new me(null, o, a);
  }
  get scopeName() {
    return this.scopePath.scopeName;
  }
  toString() {
    return this.getScopeNames().join(" ");
  }
  equals(e) {
    return me.equals(this, e);
  }
  static equals(e, t) {
    do {
      if (e === t || !e && !t)
        return !0;
      if (!e || !t || e.scopeName !== t.scopeName || e.tokenAttributes !== t.tokenAttributes)
        return !1;
      e = e.parent, t = t.parent;
    } while (!0);
  }
  static mergeAttributes(e, t, s) {
    let r = -1, o = 0, i = 0;
    return s !== null && (r = s.fontStyle, o = s.foregroundId, i = s.backgroundId), ot.set(
      e,
      t.languageId,
      t.tokenType,
      null,
      r,
      o,
      i
    );
  }
  pushAttributed(e, t) {
    if (e === null)
      return this;
    if (e.indexOf(" ") === -1)
      return me._pushAttributed(this, e, t);
    const s = e.split(/ /g);
    let r = this;
    for (const o of s)
      r = me._pushAttributed(r, o, t);
    return r;
  }
  static _pushAttributed(e, t, s) {
    const r = s.getMetadataForScope(t), o = e.scopePath.push(t), i = s.themeProvider.themeMatch(o), a = me.mergeAttributes(
      e.tokenAttributes,
      r,
      i
    );
    return new me(e, o, a);
  }
  getScopeNames() {
    return this.scopePath.getSegments();
  }
  getExtensionIfDefined(e) {
    var r;
    const t = [];
    let s = this;
    for (; s && s !== e; )
      t.push({
        encodedTokenAttributes: s.tokenAttributes,
        scopeNames: s.scopePath.getExtensionIfDefined(((r = s.parent) == null ? void 0 : r.scopePath) ?? null)
      }), s = s.parent;
    return s === e ? t.reverse() : void 0;
  }
}, ie, fr = (ie = class {
  /**
   * Invariant:
   * ```
   * if (contentNameScopesList !== nameScopesList && contentNameScopesList?.parent !== nameScopesList) {
   * 	throw new Error();
   * }
   * if (this.parent && !nameScopesList.extends(this.parent.contentNameScopesList)) {
   * 	throw new Error();
   * }
   * ```
   */
  constructor(e, t, s, r, o, i, a, l) {
    g(this, "_stackElementBrand");
    /**
     * The position on the current line where this state was pushed.
     * This is relevant only while tokenizing a line, to detect endless loops.
     * Its value is meaningless across lines.
     */
    g(this, "_enterPos");
    /**
     * The captured anchor position when this stack element was pushed.
     * This is relevant only while tokenizing a line, to restore the anchor position when popping.
     * Its value is meaningless across lines.
     */
    g(this, "_anchorPos");
    /**
     * The depth of the stack.
     */
    g(this, "depth");
    this.parent = e, this.ruleId = t, this.beginRuleCapturedEOL = o, this.endRule = i, this.nameScopesList = a, this.contentNameScopesList = l, this.depth = this.parent ? this.parent.depth + 1 : 1, this._enterPos = s, this._anchorPos = r;
  }
  equals(e) {
    return e === null ? !1 : ie._equals(this, e);
  }
  static _equals(e, t) {
    return e === t ? !0 : this._structuralEquals(e, t) ? Xt.equals(e.contentNameScopesList, t.contentNameScopesList) : !1;
  }
  /**
   * A structural equals check. Does not take into account `scopes`.
   */
  static _structuralEquals(e, t) {
    do {
      if (e === t || !e && !t)
        return !0;
      if (!e || !t || e.depth !== t.depth || e.ruleId !== t.ruleId || e.endRule !== t.endRule)
        return !1;
      e = e.parent, t = t.parent;
    } while (!0);
  }
  clone() {
    return this;
  }
  static _reset(e) {
    for (; e; )
      e._enterPos = -1, e._anchorPos = -1, e = e.parent;
  }
  reset() {
    ie._reset(this);
  }
  pop() {
    return this.parent;
  }
  safePop() {
    return this.parent ? this.parent : this;
  }
  push(e, t, s, r, o, i, a) {
    return new ie(
      this,
      e,
      t,
      s,
      r,
      o,
      i,
      a
    );
  }
  getEnterPos() {
    return this._enterPos;
  }
  getAnchorPos() {
    return this._anchorPos;
  }
  getRule(e) {
    return e.getRule(this.ruleId);
  }
  toString() {
    const e = [];
    return this._writeString(e, 0), "[" + e.join(",") + "]";
  }
  _writeString(e, t) {
    var s, r;
    return this.parent && (t = this.parent._writeString(e, t)), e[t++] = `(${this.ruleId}, ${(s = this.nameScopesList) == null ? void 0 : s.toString()}, ${(r = this.contentNameScopesList) == null ? void 0 : r.toString()})`, t;
  }
  withContentNameScopesList(e) {
    return this.contentNameScopesList === e ? this : this.parent.push(
      this.ruleId,
      this._enterPos,
      this._anchorPos,
      this.beginRuleCapturedEOL,
      this.endRule,
      this.nameScopesList,
      e
    );
  }
  withEndRule(e) {
    return this.endRule === e ? this : new ie(
      this.parent,
      this.ruleId,
      this._enterPos,
      this._anchorPos,
      this.beginRuleCapturedEOL,
      e,
      this.nameScopesList,
      this.contentNameScopesList
    );
  }
  // Used to warn of endless loops
  hasSameRuleAs(e) {
    let t = this;
    for (; t && t._enterPos === e._enterPos; ) {
      if (t.ruleId === e.ruleId)
        return !0;
      t = t.parent;
    }
    return !1;
  }
  toStateStackFrame() {
    var e, t, s;
    return {
      ruleId: this.ruleId,
      beginRuleCapturedEOL: this.beginRuleCapturedEOL,
      endRule: this.endRule,
      nameScopesList: ((t = this.nameScopesList) == null ? void 0 : t.getExtensionIfDefined(((e = this.parent) == null ? void 0 : e.nameScopesList) ?? null)) ?? [],
      contentNameScopesList: ((s = this.contentNameScopesList) == null ? void 0 : s.getExtensionIfDefined(this.nameScopesList)) ?? []
    };
  }
  static pushFrame(e, t) {
    const s = Xt.fromExtension((e == null ? void 0 : e.nameScopesList) ?? null, t.nameScopesList);
    return new ie(
      e,
      t.ruleId,
      t.enterPos ?? -1,
      t.anchorPos ?? -1,
      t.beginRuleCapturedEOL,
      t.endRule,
      s,
      Xt.fromExtension(s, t.contentNameScopesList)
    );
  }
}, // TODO remove me
g(ie, "NULL", new ie(
  null,
  0,
  0,
  0,
  !1,
  null,
  null,
  null
)), ie), _u = class {
  constructor(n, e) {
    g(this, "balancedBracketScopes");
    g(this, "unbalancedBracketScopes");
    g(this, "allowAny", !1);
    this.balancedBracketScopes = n.flatMap(
      (t) => t === "*" ? (this.allowAny = !0, []) : Zn(t, Yn).map((s) => s.matcher)
    ), this.unbalancedBracketScopes = e.flatMap(
      (t) => Zn(t, Yn).map((s) => s.matcher)
    );
  }
  get matchesAlways() {
    return this.allowAny && this.unbalancedBracketScopes.length === 0;
  }
  get matchesNever() {
    return this.balancedBracketScopes.length === 0 && !this.allowAny;
  }
  match(n) {
    for (const e of this.unbalancedBracketScopes)
      if (e(n))
        return !1;
    for (const e of this.balancedBracketScopes)
      if (e(n))
        return !0;
    return this.allowAny;
  }
}, wu = class {
  constructor(n, e, t, s) {
    g(this, "_emitBinaryTokens");
    /**
     * defined only if `false`.
     */
    g(this, "_lineText");
    /**
     * used only if `_emitBinaryTokens` is false.
     */
    g(this, "_tokens");
    /**
     * used only if `_emitBinaryTokens` is true.
     */
    g(this, "_binaryTokens");
    g(this, "_lastTokenEndIndex");
    g(this, "_tokenTypeOverrides");
    this.balancedBracketSelectors = s, this._emitBinaryTokens = n, this._tokenTypeOverrides = t, this._lineText = null, this._tokens = [], this._binaryTokens = [], this._lastTokenEndIndex = 0;
  }
  produce(n, e) {
    this.produceFromScopes(n.contentNameScopesList, e);
  }
  produceFromScopes(n, e) {
    var s;
    if (this._lastTokenEndIndex >= e)
      return;
    if (this._emitBinaryTokens) {
      let r = (n == null ? void 0 : n.tokenAttributes) ?? 0, o = !1;
      if ((s = this.balancedBracketSelectors) != null && s.matchesAlways && (o = !0), this._tokenTypeOverrides.length > 0 || this.balancedBracketSelectors && !this.balancedBracketSelectors.matchesAlways && !this.balancedBracketSelectors.matchesNever) {
        const i = (n == null ? void 0 : n.getScopeNames()) ?? [];
        for (const a of this._tokenTypeOverrides)
          a.matcher(i) && (r = ot.set(
            r,
            0,
            a.type,
            null,
            -1,
            0,
            0
          ));
        this.balancedBracketSelectors && (o = this.balancedBracketSelectors.match(i));
      }
      if (o && (r = ot.set(
        r,
        0,
        8,
        o,
        -1,
        0,
        0
      )), this._binaryTokens.length > 0 && this._binaryTokens[this._binaryTokens.length - 1] === r) {
        this._lastTokenEndIndex = e;
        return;
      }
      this._binaryTokens.push(this._lastTokenEndIndex), this._binaryTokens.push(r), this._lastTokenEndIndex = e;
      return;
    }
    const t = (n == null ? void 0 : n.getScopeNames()) ?? [];
    this._tokens.push({
      startIndex: this._lastTokenEndIndex,
      endIndex: e,
      // value: lineText.substring(lastTokenEndIndex, endIndex),
      scopes: t
    }), this._lastTokenEndIndex = e;
  }
  getResult(n, e) {
    return this._tokens.length > 0 && this._tokens[this._tokens.length - 1].startIndex === e - 1 && this._tokens.pop(), this._tokens.length === 0 && (this._lastTokenEndIndex = -1, this.produce(n, e), this._tokens[this._tokens.length - 1].startIndex = 0), this._tokens;
  }
  getBinaryResult(n, e) {
    this._binaryTokens.length > 0 && this._binaryTokens[this._binaryTokens.length - 2] === e - 1 && (this._binaryTokens.pop(), this._binaryTokens.pop()), this._binaryTokens.length === 0 && (this._lastTokenEndIndex = -1, this.produce(n, e), this._binaryTokens[this._binaryTokens.length - 2] = 0);
    const t = new Uint32Array(this._binaryTokens.length);
    for (let s = 0, r = this._binaryTokens.length; s < r; s++)
      t[s] = this._binaryTokens[s];
    return t;
  }
}, xu = class {
  constructor(n, e) {
    g(this, "_grammars", /* @__PURE__ */ new Map());
    g(this, "_rawGrammars", /* @__PURE__ */ new Map());
    g(this, "_injectionGrammars", /* @__PURE__ */ new Map());
    g(this, "_theme");
    this._onigLib = e, this._theme = n;
  }
  dispose() {
    for (const n of this._grammars.values())
      n.dispose();
  }
  setTheme(n) {
    this._theme = n;
  }
  getColorMap() {
    return this._theme.getColorMap();
  }
  /**
   * Add `grammar` to registry and return a list of referenced scope names
   */
  addGrammar(n, e) {
    this._rawGrammars.set(n.scopeName, n), e && this._injectionGrammars.set(n.scopeName, e);
  }
  /**
   * Lookup a raw grammar.
   */
  lookup(n) {
    return this._rawGrammars.get(n);
  }
  /**
   * Returns the injections for the given grammar
   */
  injections(n) {
    return this._injectionGrammars.get(n);
  }
  /**
   * Get the default theme settings
   */
  getDefaults() {
    return this._theme.getDefaults();
  }
  /**
   * Match a scope in the theme.
   */
  themeMatch(n) {
    return this._theme.match(n);
  }
  /**
   * Lookup a grammar.
   */
  grammarForScopeName(n, e, t, s, r) {
    if (!this._grammars.has(n)) {
      let o = this._rawGrammars.get(n);
      if (!o)
        return null;
      this._grammars.set(n, bu(
        n,
        o,
        e,
        t,
        s,
        r,
        this,
        this._onigLib
      ));
    }
    return this._grammars.get(n);
  }
}, ku = class {
  constructor(e) {
    g(this, "_options");
    g(this, "_syncRegistry");
    g(this, "_ensureGrammarCache");
    this._options = e, this._syncRegistry = new xu(
      Kn.createFromRawTheme(e.theme, e.colorMap),
      e.onigLib
    ), this._ensureGrammarCache = /* @__PURE__ */ new Map();
  }
  dispose() {
    this._syncRegistry.dispose();
  }
  /**
   * Change the theme. Once called, no previous `ruleStack` should be used anymore.
   */
  setTheme(e, t) {
    this._syncRegistry.setTheme(Kn.createFromRawTheme(e, t));
  }
  /**
   * Returns a lookup array for color ids.
   */
  getColorMap() {
    return this._syncRegistry.getColorMap();
  }
  /**
   * Load the grammar for `scopeName` and all referenced included grammars asynchronously.
   * Please do not use language id 0.
   */
  loadGrammarWithEmbeddedLanguages(e, t, s) {
    return this.loadGrammarWithConfiguration(e, t, { embeddedLanguages: s });
  }
  /**
   * Load the grammar for `scopeName` and all referenced included grammars asynchronously.
   * Please do not use language id 0.
   */
  loadGrammarWithConfiguration(e, t, s) {
    return this._loadGrammar(
      e,
      t,
      s.embeddedLanguages,
      s.tokenTypes,
      new _u(
        s.balancedBracketSelectors || [],
        s.unbalancedBracketSelectors || []
      )
    );
  }
  /**
   * Load the grammar for `scopeName` and all referenced included grammars asynchronously.
   */
  loadGrammar(e) {
    return this._loadGrammar(e, 0, null, null, null);
  }
  _loadGrammar(e, t, s, r, o) {
    const i = new Jc(this._syncRegistry, e);
    for (; i.Q.length > 0; )
      i.Q.map((a) => this._loadSingleGrammar(a.scopeName)), i.processQueue();
    return this._grammarForScopeName(
      e,
      t,
      s,
      r,
      o
    );
  }
  _loadSingleGrammar(e) {
    this._ensureGrammarCache.has(e) || (this._doLoadSingleGrammar(e), this._ensureGrammarCache.set(e, !0));
  }
  _doLoadSingleGrammar(e) {
    const t = this._options.loadGrammar(e);
    if (t) {
      const s = typeof this._options.getInjections == "function" ? this._options.getInjections(e) : void 0;
      this._syncRegistry.addGrammar(t, s);
    }
  }
  /**
   * Adds a rawGrammar.
   */
  addGrammar(e, t = [], s = 0, r = null) {
    return this._syncRegistry.addGrammar(e, t), this._grammarForScopeName(e.scopeName, s, r);
  }
  /**
   * Get the grammar for `scopeName`. The grammar must first be created via `loadGrammar` or `addGrammar`.
   */
  _grammarForScopeName(e, t = 0, s = null, r = null, o = null) {
    return this._syncRegistry.grammarForScopeName(
      e,
      t,
      s,
      r,
      o
    );
  }
}, gr = fr.NULL;
function es(n, e) {
  const t = typeof n == "string" ? {} : { ...n.colorReplacements }, s = typeof n == "string" ? n : n.name;
  for (const [r, o] of Object.entries((e == null ? void 0 : e.colorReplacements) || {})) typeof o == "string" ? t[r] = o : r === s && Object.assign(t, o);
  return t;
}
function Fe(n, e) {
  return n && ((e == null ? void 0 : e[n == null ? void 0 : n.toLowerCase()]) || n);
}
function Cu(n) {
  return Array.isArray(n) ? n : [n];
}
async function Ma(n) {
  return Promise.resolve(typeof n == "function" ? n() : n).then((e) => e.default || e);
}
function xs(n) {
  return !n || [
    "plaintext",
    "txt",
    "text",
    "plain"
  ].includes(n);
}
function $u(n) {
  return n === "ansi" || xs(n);
}
function ks(n) {
  return n === "none";
}
function Su(n) {
  return ks(n);
}
const Au = /(\r?\n)/g;
function Cs(n, e = !1) {
  var o;
  if (n.length === 0) return [["", 0]];
  const t = n.split(Au);
  let s = 0;
  const r = [];
  for (let i = 0; i < t.length; i += 2) {
    const a = e ? t[i] + (t[i + 1] || "") : t[i];
    r.push([a, s]), s += t[i].length, s += ((o = t[i + 1]) == null ? void 0 : o.length) || 0;
  }
  return r;
}
const mi = {
  light: "#333333",
  dark: "#bbbbbb"
}, bi = {
  light: "#fffffe",
  dark: "#1e1e1e"
}, yi = "__shiki_resolved";
function Ur(n) {
  var a, l, c, h, u;
  if (n != null && n[yi]) return n;
  const e = { ...n };
  e.tokenColors && !e.settings && (e.settings = e.tokenColors, delete e.tokenColors), e.type || (e.type = "dark"), e.colorReplacements = { ...e.colorReplacements }, e.settings || (e.settings = []);
  let { bg: t, fg: s } = e;
  if (!t || !s) {
    const p = e.settings ? e.settings.find((d) => !d.name && !d.scope) : void 0;
    (a = p == null ? void 0 : p.settings) != null && a.foreground && (s = p.settings.foreground), (l = p == null ? void 0 : p.settings) != null && l.background && (t = p.settings.background), !s && ((c = e == null ? void 0 : e.colors) != null && c["editor.foreground"]) && (s = e.colors["editor.foreground"]), !t && ((h = e == null ? void 0 : e.colors) != null && h["editor.background"]) && (t = e.colors["editor.background"]), s || (s = e.type === "light" ? mi.light : mi.dark), t || (t = e.type === "light" ? bi.light : bi.dark), e.fg = s, e.bg = t;
  }
  e.settings[0] && e.settings[0].settings && !e.settings[0].scope || e.settings.unshift({ settings: {
    foreground: e.fg,
    background: e.bg
  } });
  let r = 0;
  const o = /* @__PURE__ */ new Map();
  function i(p) {
    var f;
    if (o.has(p)) return o.get(p);
    r += 1;
    const d = `#${r.toString(16).padStart(8, "0").toLowerCase()}`;
    return (f = e.colorReplacements) != null && f[`#${d}`] ? i(p) : (o.set(p, d), d);
  }
  e.settings = e.settings.map((p) => {
    var v, k;
    const d = ((v = p.settings) == null ? void 0 : v.foreground) && !p.settings.foreground.startsWith("#"), f = ((k = p.settings) == null ? void 0 : k.background) && !p.settings.background.startsWith("#");
    if (!d && !f) return p;
    const b = {
      ...p,
      settings: { ...p.settings }
    };
    if (d) {
      const _ = i(p.settings.foreground);
      e.colorReplacements[_] = p.settings.foreground, b.settings.foreground = _;
    }
    if (f) {
      const _ = i(p.settings.background);
      e.colorReplacements[_] = p.settings.background, b.settings.background = _;
    }
    return b;
  });
  for (const p of Object.keys(e.colors || {})) if ((p === "editor.foreground" || p === "editor.background" || p.startsWith("terminal.ansi")) && !((u = e.colors[p]) != null && u.startsWith("#"))) {
    const d = i(e.colors[p]);
    e.colorReplacements[d] = e.colors[p], e.colors[p] = d;
  }
  return Object.defineProperty(e, yi, {
    enumerable: !1,
    writable: !1,
    value: !0
  }), e;
}
async function Oa(n) {
  return [...new Set((await Promise.all(n.filter((e) => !$u(e)).map(async (e) => await Ma(e).then((t) => Array.isArray(t) ? t : [t])))).flat())];
}
async function za(n) {
  return (await Promise.all(n.map(async (e) => Su(e) ? null : Ur(await Ma(e))))).filter((e) => !!e);
}
function Ba(n, e) {
  if (!e) return n;
  if (e[n]) {
    const t = /* @__PURE__ */ new Set([n]);
    for (; e[n]; ) {
      if (n = e[n], t.has(n)) throw new O(`Circular alias \`${[...t].join(" -> ")} -> ${n}\``);
      t.add(n);
    }
  }
  return n;
}
var Eu = class extends ku {
  constructor(e, t, s, r = {}) {
    super(e);
    g(this, "_resolver");
    g(this, "_themes");
    g(this, "_langs");
    g(this, "_alias");
    g(this, "_resolvedThemes", /* @__PURE__ */ new Map());
    g(this, "_resolvedGrammars", /* @__PURE__ */ new Map());
    g(this, "_langMap", /* @__PURE__ */ new Map());
    g(this, "_langGraph", /* @__PURE__ */ new Map());
    g(this, "_textmateThemeCache", /* @__PURE__ */ new WeakMap());
    g(this, "_loadedThemesCache", null);
    g(this, "_loadedLanguagesCache", null);
    this._resolver = e, this._themes = t, this._langs = s, this._alias = r, this._themes.map((o) => this.loadTheme(o)), this.loadLanguages(this._langs);
  }
  getTheme(e) {
    return typeof e == "string" ? this._resolvedThemes.get(e) : this.loadTheme(e);
  }
  loadTheme(e) {
    const t = Ur(e);
    return t.name && (this._resolvedThemes.set(t.name, t), this._loadedThemesCache = null), t;
  }
  getLoadedThemes() {
    return this._loadedThemesCache || (this._loadedThemesCache = [...this._resolvedThemes.keys()]), this._loadedThemesCache;
  }
  setTheme(e) {
    let t = this._textmateThemeCache.get(e);
    t || (t = Kn.createFromRawTheme(e), this._textmateThemeCache.set(e, t)), this._syncRegistry.setTheme(t);
  }
  getGrammar(e) {
    return e = Ba(e, this._alias), this._resolvedGrammars.get(e);
  }
  loadLanguage(e) {
    var o, i, a, l;
    if (this.getGrammar(e.name)) return;
    const t = new Set([...this._langMap.values()].filter((c) => {
      var h;
      return (h = c.embeddedLangsLazy) == null ? void 0 : h.includes(e.name);
    }));
    this._resolver.addLanguage(e);
    const s = {
      balancedBracketSelectors: e.balancedBracketSelectors || ["*"],
      unbalancedBracketSelectors: e.unbalancedBracketSelectors || []
    };
    this._syncRegistry._rawGrammars.set(e.scopeName, e);
    const r = this.loadGrammarWithConfiguration(e.scopeName, 1, s);
    if (r.name = e.name, this._resolvedGrammars.set(e.name, r), e.aliases && e.aliases.forEach((c) => {
      this._alias[c] = e.name;
    }), this._loadedLanguagesCache = null, t.size) for (const c of t)
      this._resolvedGrammars.delete(c.name), this._loadedLanguagesCache = null, (i = (o = this._syncRegistry) == null ? void 0 : o._injectionGrammars) == null || i.delete(c.scopeName), (l = (a = this._syncRegistry) == null ? void 0 : a._grammars) == null || l.delete(c.scopeName), this.loadLanguage(this._langMap.get(c.name));
  }
  dispose() {
    super.dispose(), this._resolvedThemes.clear(), this._resolvedGrammars.clear(), this._langMap.clear(), this._langGraph.clear(), this._loadedThemesCache = null;
  }
  loadLanguages(e) {
    for (const r of e) this.resolveEmbeddedLanguages(r);
    const t = [...this._langGraph.entries()], s = t.filter(([r, o]) => !o);
    if (s.length) {
      const r = t.filter(([o, i]) => {
        var a;
        return i ? (a = i.embeddedLanguages || i.embeddedLangs) == null ? void 0 : a.some((l) => s.map(([c]) => c).includes(l)) : !1;
      }).filter((o) => !s.includes(o));
      throw new O(`Missing languages ${s.map(([o]) => `\`${o}\``).join(", ")}, required by ${r.map(([o]) => `\`${o}\``).join(", ")}`);
    }
    for (const [r, o] of t) this._resolver.addLanguage(o);
    for (const [r, o] of t) this.loadLanguage(o);
  }
  getLoadedLanguages() {
    return this._loadedLanguagesCache || (this._loadedLanguagesCache = [.../* @__PURE__ */ new Set([...this._resolvedGrammars.keys(), ...Object.keys(this._alias)])]), this._loadedLanguagesCache;
  }
  resolveEmbeddedLanguages(e) {
    this._langMap.set(e.name, e), this._langGraph.set(e.name, e);
    const t = e.embeddedLanguages ?? e.embeddedLangs;
    if (t) for (const s of t) this._langGraph.set(s, this._langMap.get(s));
  }
}, Ru = class {
  constructor(n, e) {
    g(this, "_langs", /* @__PURE__ */ new Map());
    g(this, "_scopeToLang", /* @__PURE__ */ new Map());
    g(this, "_injections", /* @__PURE__ */ new Map());
    g(this, "_onigLib");
    this._onigLib = {
      createOnigScanner: (t) => n.createScanner(t),
      createOnigString: (t) => n.createString(t)
    }, e.forEach((t) => this.addLanguage(t));
  }
  get onigLib() {
    return this._onigLib;
  }
  getLangRegistration(n) {
    return this._langs.get(n);
  }
  loadGrammar(n) {
    return this._scopeToLang.get(n);
  }
  addLanguage(n) {
    this._langs.set(n.name, n), n.aliases && n.aliases.forEach((e) => {
      this._langs.set(e, n);
    }), this._scopeToLang.set(n.scopeName, n), n.injectTo && n.injectTo.forEach((e) => {
      this._injections.get(e) || this._injections.set(e, []), this._injections.get(e).push(n.scopeName);
    });
  }
  getInjections(n) {
    const e = n.split(".");
    let t = [];
    for (let s = 1; s <= e.length; s++) {
      const r = e.slice(0, s).join(".");
      t = [...t, ...this._injections.get(r) || []];
    }
    return t;
  }
};
let qt = 0;
function Tu(n) {
  qt += 1, n.warnings !== !1 && qt >= 10 && qt % 10 === 0 && console.warn(`[Shiki] ${qt} instances have been created. Shiki is supposed to be used as a singleton, consider refactoring your code to cache your highlighter instance; Or call \`highlighter.dispose()\` to release unused instances.`);
  let e = !1;
  if (!n.engine) throw new O("`engine` option is required for synchronous mode");
  const t = (n.langs || []).flat(1), s = (n.themes || []).flat(1).map(Ur), r = new Eu(new Ru(n.engine, t), s, t, n.langAlias);
  let o;
  function i(_) {
    return Ba(_, n.langAlias);
  }
  function a(_) {
    v();
    const x = r.getGrammar(typeof _ == "string" ? _ : _.name);
    if (!x) throw new O(`Language \`${_}\` not found, you may need to load it first`);
    return x;
  }
  function l(_) {
    if (_ === "none") return {
      bg: "",
      fg: "",
      name: "none",
      settings: [],
      type: "dark"
    };
    v();
    const x = r.getTheme(_);
    if (!x) throw new O(`Theme \`${_}\` not found, you may need to load it first`);
    return x;
  }
  function c(_) {
    v();
    const x = l(_);
    return o !== _ && (r.setTheme(x), o = _), {
      theme: x,
      colorMap: r.getColorMap()
    };
  }
  function h() {
    return v(), r.getLoadedThemes();
  }
  function u() {
    return v(), r.getLoadedLanguages();
  }
  function p(..._) {
    v(), r.loadLanguages(_.flat(1));
  }
  async function d(..._) {
    return p(await Oa(_));
  }
  function f(..._) {
    v();
    for (const x of _.flat(1)) r.loadTheme(x);
  }
  async function b(..._) {
    return v(), f(await za(_));
  }
  function v() {
    if (e) throw new O("Shiki instance has been disposed");
  }
  function k() {
    e || (e = !0, r.dispose(), qt -= 1);
  }
  return {
    setTheme: c,
    getTheme: l,
    getLanguage: a,
    getLoadedThemes: h,
    getLoadedLanguages: u,
    resolveLangAlias: i,
    loadLanguage: d,
    loadLanguageSync: p,
    loadTheme: b,
    loadThemeSync: f,
    dispose: k,
    [Symbol.dispose]: k
  };
}
async function Iu(n) {
  n.engine || console.warn("`engine` option is required. Use `createOnigurumaEngine` or `createJavaScriptRegexEngine` to create an engine.");
  const [e, t, s] = await Promise.all([
    za(n.themes || []),
    Oa(n.langs || []),
    n.engine
  ]);
  return Tu({
    ...n,
    themes: e,
    langs: t,
    engine: s
  });
}
const Da = /* @__PURE__ */ new WeakMap();
function $s(n, e) {
  Da.set(n, e);
}
function hn(n) {
  return Da.get(n);
}
var Ss = class Fa {
  constructor(...e) {
    /**
    * Theme to Stack mapping
    */
    g(this, "_stacks", {});
    g(this, "lang");
    if (e.length === 2) {
      const [t, s] = e;
      this.lang = s, this._stacks = t;
    } else {
      const [t, s, r] = e;
      this.lang = s, this._stacks = { [r]: t };
    }
  }
  get themes() {
    return Object.keys(this._stacks);
  }
  get theme() {
    return this.themes[0];
  }
  get _stack() {
    return this._stacks[this.theme];
  }
  /**
  * Static method to create a initial grammar state.
  */
  static initial(e, t) {
    return new Fa(Object.fromEntries(Cu(t).map((s) => [s, gr])), e);
  }
  /**
  * Get the internal stack object.
  * @internal
  */
  getInternalStack(e = this.theme) {
    return this._stacks[e];
  }
  getScopes(e = this.theme) {
    return Pu(this._stacks[e]);
  }
  toJSON() {
    return {
      lang: this.lang,
      theme: this.theme,
      themes: this.themes,
      scopes: this.getScopes()
    };
  }
};
function Pu(n) {
  const e = [], t = /* @__PURE__ */ new Set();
  function s(r) {
    var i;
    if (t.has(r)) return;
    t.add(r);
    const o = (i = r == null ? void 0 : r.nameScopesList) == null ? void 0 : i.scopeName;
    o && e.push(o), r.parent && s(r.parent);
  }
  return s(n), e;
}
function Lu(n, e) {
  if (!(n instanceof Ss)) throw new O("Invalid grammar state");
  return n.getInternalStack(e);
}
const Nu = /,/, Mu = / /;
function Ga(n, e, t = {}) {
  const { theme: s = n.getLoadedThemes()[0] } = t;
  if (xs(n.resolveLangAlias(t.lang || "text")) || ks(s)) return Cs(e).map((a) => [{
    content: a[0],
    offset: a[1]
  }]);
  const { theme: r, colorMap: o } = n.setTheme(s), i = n.getLanguage(t.lang || "text");
  if (t.grammarState) {
    if (t.grammarState.lang !== i.name) throw new O(`Grammar state language "${t.grammarState.lang}" does not match highlight language "${i.name}"`);
    if (!t.grammarState.themes.includes(r.name)) throw new O(`Grammar state themes "${t.grammarState.themes}" do not contain highlight theme "${r.name}"`);
  }
  return zu(e, i, r, o, t);
}
function Ou(...n) {
  if (n.length === 2) return hn(n[1]);
  const [e, t, s = {}] = n, { lang: r = "text", theme: o = e.getLoadedThemes()[0] } = s;
  if (xs(r) || ks(o)) throw new O("Plain language does not have grammar state");
  if (r === "ansi") throw new O("ANSI language does not have grammar state");
  const { theme: i, colorMap: a } = e.setTheme(o), l = e.getLanguage(r);
  return new Ss(jr(t, l, i, a, s).stateStack, l.name, i.name);
}
function zu(n, e, t, s, r) {
  const o = jr(n, e, t, s, r), i = new Ss(o.stateStack, e.name, t.name);
  return $s(o.tokens, i), o.tokens;
}
function jr(n, e, t, s, r) {
  const o = es(t, r), { tokenizeMaxLineLength: i = 0, tokenizeTimeLimit: a = 500, includeExplanation: l = !1 } = r, c = Cs(n);
  let h = r.grammarState ? Lu(r.grammarState, t.name) ?? gr : r.grammarContextCode != null ? jr(r.grammarContextCode, e, t, s, {
    ...r,
    grammarState: void 0,
    grammarContextCode: void 0
  }).stateStack : gr, u = [];
  const p = [];
  for (let d = 0, f = c.length; d < f; d++) {
    const [b, v] = c[d];
    if (b === "") {
      u = [], p.push([]);
      continue;
    }
    if (i > 0 && b.length >= i) {
      u = [], p.push([{
        content: b,
        offset: v,
        color: "",
        fontStyle: 0
      }]);
      continue;
    }
    let k, _, x;
    l && l !== "tokenType" && (k = e.tokenizeLine(b, h, a), _ = k.tokens, x = 0);
    const $ = e.tokenizeLine2(b, h, a), A = $.tokens.length / 2;
    for (let T = 0; T < A; T++) {
      const M = $.tokens[2 * T], ee = T + 1 < A ? $.tokens[2 * T + 2] : b.length;
      if (M === ee) continue;
      const j = $.tokens[2 * T + 1], ge = Fe(s[ot.getForeground(j)], o), mt = ot.getFontStyle(j), Z = {
        content: b.substring(M, ee),
        offset: v + M,
        color: ge,
        fontStyle: mt
      };
      if (l === "tokenType") Z.type = ot.getTokenType(j);
      else if (l) {
        const Wo = [];
        if (l !== "scopeName") for (const we of t.settings) {
          let bt;
          switch (typeof we.scope) {
            case "string":
              bt = we.scope.split(Nu).map((Ms) => Ms.trim());
              break;
            case "object":
              bt = we.scope;
              break;
            default:
              continue;
          }
          Wo.push({
            settings: we,
            selectors: bt.map((Ms) => Ms.split(Mu))
          });
        }
        Z.explanation = [];
        let Qo = 0;
        for (; M + Qo < ee; ) {
          const we = _[x], bt = b.substring(we.startIndex, we.endIndex);
          Qo += bt.length, Z.explanation.push({
            content: bt,
            scopes: l === "scopeName" ? Bu(we.scopes) : Du(Wo, we.scopes)
          }), x += 1;
        }
      }
      u.push(Z);
    }
    p.push(u), u = [], h = $.ruleStack;
  }
  return {
    tokens: p,
    stateStack: h
  };
}
function Bu(n) {
  return n.map((e) => ({ scopeName: e }));
}
function Du(n, e) {
  const t = [];
  for (let s = 0, r = e.length; s < r; s++) {
    const o = e[s];
    t[s] = {
      scopeName: o,
      themeMatches: Gu(n, o, e.slice(0, s))
    };
  }
  return t;
}
function vi(n, e) {
  return n === e || e.substring(0, n.length) === n && e[n.length] === ".";
}
function Fu(n, e, t) {
  if (!vi(n.at(-1), e)) return !1;
  let s = n.length - 2, r = t.length - 1;
  for (; s >= 0 && r >= 0; )
    vi(n[s], t[r]) && (s -= 1), r -= 1;
  return s === -1;
}
function Gu(n, e, t) {
  const s = [];
  for (const { selectors: r, settings: o } of n) for (const i of r) if (Fu(i, e, t)) {
    s.push(o);
    break;
  }
  return s;
}
function Ua(n, e, t, s = Ga) {
  const r = Object.entries(t.themes).filter((c) => c[1]).map((c) => ({
    color: c[0],
    theme: c[1]
  })), o = r.map((c) => {
    const h = s(n, e, {
      ...t,
      theme: c.theme
    });
    return {
      tokens: h,
      state: hn(h),
      theme: typeof c.theme == "string" ? c.theme : c.theme.name
    };
  }), i = Uu(...o.map((c) => c.tokens)), a = i[0].map((c, h) => c.map((u, p) => {
    const d = {
      content: u.content,
      variants: {},
      offset: u.offset
    };
    return "includeExplanation" in t && t.includeExplanation && (d.explanation = u.explanation), i.forEach((f, b) => {
      const { content: v, explanation: k, offset: _, ...x } = f[h][p];
      d.variants[r[b].color] = x;
    }), d;
  })), l = o[0].state ? new Ss(Object.fromEntries(o.map((c) => {
    var h;
    return [c.theme, (h = c.state) == null ? void 0 : h.getInternalStack(c.theme)];
  })), o[0].state.lang) : void 0;
  return l && $s(a, l), a;
}
function Uu(...n) {
  const e = n.map(() => []), t = n.length;
  for (let s = 0; s < n[0].length; s++) {
    const r = n.map((l) => l[s]), o = e.map(() => []);
    e.forEach((l, c) => l.push(o[c]));
    const i = r.map(() => 0), a = r.map((l) => l[0]);
    for (; a.every((l) => l); ) {
      const l = Math.min(...a.map((c) => c.content.length));
      for (let c = 0; c < t; c++) {
        const h = a[c];
        h.content.length === l ? (o[c].push(h), i[c] += 1, a[c] = r[c][i[c]]) : (o[c].push({
          ...h,
          content: h.content.slice(0, l)
        }), a[c] = {
          ...h,
          content: h.content.slice(l),
          offset: h.offset + l
        });
      }
    }
  }
  return e;
}
const ju = [
  "area",
  "base",
  "basefont",
  "bgsound",
  "br",
  "col",
  "command",
  "embed",
  "frame",
  "hr",
  "image",
  "img",
  "input",
  "keygen",
  "link",
  "meta",
  "param",
  "source",
  "track",
  "wbr"
];
class xn {
  /**
   * @param {SchemaType['property']} property
   *   Property.
   * @param {SchemaType['normal']} normal
   *   Normal.
   * @param {Space | undefined} [space]
   *   Space.
   * @returns
   *   Schema.
   */
  constructor(e, t, s) {
    this.normal = t, this.property = e, s && (this.space = s);
  }
}
xn.prototype.normal = {};
xn.prototype.property = {};
xn.prototype.space = void 0;
function ja(n, e) {
  const t = {}, s = {};
  for (const r of n)
    Object.assign(t, r.property), Object.assign(s, r.normal);
  return new xn(t, s, e);
}
function mr(n) {
  return n.toLowerCase();
}
class Y {
  /**
   * @param {string} property
   *   Property.
   * @param {string} attribute
   *   Attribute.
   * @returns
   *   Info.
   */
  constructor(e, t) {
    this.attribute = t, this.property = e;
  }
}
Y.prototype.attribute = "";
Y.prototype.booleanish = !1;
Y.prototype.boolean = !1;
Y.prototype.commaOrSpaceSeparated = !1;
Y.prototype.commaSeparated = !1;
Y.prototype.defined = !1;
Y.prototype.mustUseProperty = !1;
Y.prototype.number = !1;
Y.prototype.overloadedBoolean = !1;
Y.prototype.property = "";
Y.prototype.spaceSeparated = !1;
Y.prototype.space = void 0;
let qu = 0;
const S = dt(), B = dt(), br = dt(), y = dt(), N = dt(), it = dt(), te = dt();
function dt() {
  return 2 ** ++qu;
}
const yr = /* @__PURE__ */ Object.freeze(/* @__PURE__ */ Object.defineProperty({
  __proto__: null,
  boolean: S,
  booleanish: B,
  commaOrSpaceSeparated: te,
  commaSeparated: it,
  number: y,
  overloadedBoolean: br,
  spaceSeparated: N
}, Symbol.toStringTag, { value: "Module" })), Hs = (
  /** @type {ReadonlyArray<keyof typeof types>} */
  Object.keys(yr)
);
class qr extends Y {
  /**
   * @constructor
   * @param {string} property
   *   Property.
   * @param {string} attribute
   *   Attribute.
   * @param {number | null | undefined} [mask]
   *   Mask.
   * @param {Space | undefined} [space]
   *   Space.
   * @returns
   *   Info.
   */
  constructor(e, t, s, r) {
    let o = -1;
    if (super(e, t), _i(this, "space", r), typeof s == "number")
      for (; ++o < Hs.length; ) {
        const i = Hs[o];
        _i(this, Hs[o], (s & yr[i]) === yr[i]);
      }
  }
}
qr.prototype.defined = !0;
function _i(n, e, t) {
  t && (n[e] = t);
}
function zt(n) {
  const e = {}, t = {};
  for (const [s, r] of Object.entries(n.properties)) {
    const o = new qr(
      s,
      n.transform(n.attributes || {}, s),
      r,
      n.space
    );
    n.mustUseProperty && n.mustUseProperty.includes(s) && (o.mustUseProperty = !0), e[s] = o, t[mr(s)] = s, t[mr(o.attribute)] = s;
  }
  return new xn(e, t, n.space);
}
const qa = zt({
  properties: {
    ariaActiveDescendant: null,
    ariaAtomic: B,
    ariaAutoComplete: null,
    ariaBusy: B,
    ariaChecked: B,
    ariaColCount: y,
    ariaColIndex: y,
    ariaColSpan: y,
    ariaControls: N,
    ariaCurrent: null,
    ariaDescribedBy: N,
    ariaDetails: null,
    ariaDisabled: B,
    ariaDropEffect: N,
    ariaErrorMessage: null,
    ariaExpanded: B,
    ariaFlowTo: N,
    ariaGrabbed: B,
    ariaHasPopup: null,
    ariaHidden: B,
    ariaInvalid: null,
    ariaKeyShortcuts: null,
    ariaLabel: null,
    ariaLabelledBy: N,
    ariaLevel: y,
    ariaLive: null,
    ariaModal: B,
    ariaMultiLine: B,
    ariaMultiSelectable: B,
    ariaOrientation: null,
    ariaOwns: N,
    ariaPlaceholder: null,
    ariaPosInSet: y,
    ariaPressed: B,
    ariaReadOnly: B,
    ariaRelevant: null,
    ariaRequired: B,
    ariaRoleDescription: N,
    ariaRowCount: y,
    ariaRowIndex: y,
    ariaRowSpan: y,
    ariaSelected: B,
    ariaSetSize: y,
    ariaSort: null,
    ariaValueMax: y,
    ariaValueMin: y,
    ariaValueNow: y,
    ariaValueText: null,
    role: null
  },
  transform(n, e) {
    return e === "role" ? e : "aria-" + e.slice(4).toLowerCase();
  }
});
function Ha(n, e) {
  return e in n ? n[e] : e;
}
function Wa(n, e) {
  return Ha(n, e.toLowerCase());
}
const Hu = zt({
  attributes: {
    acceptcharset: "accept-charset",
    classname: "class",
    htmlfor: "for",
    httpequiv: "http-equiv"
  },
  mustUseProperty: ["checked", "multiple", "muted", "selected"],
  properties: {
    // Standard Properties.
    abbr: null,
    accept: it,
    acceptCharset: N,
    accessKey: N,
    action: null,
    allow: null,
    allowFullScreen: S,
    allowPaymentRequest: S,
    allowUserMedia: S,
    alpha: S,
    alt: null,
    as: null,
    async: S,
    autoCapitalize: null,
    autoComplete: N,
    autoFocus: S,
    autoPlay: S,
    blocking: N,
    capture: null,
    charSet: null,
    checked: S,
    cite: null,
    className: N,
    closedBy: null,
    colorSpace: null,
    cols: y,
    colSpan: y,
    command: null,
    commandFor: null,
    content: null,
    contentEditable: B,
    controls: S,
    controlsList: N,
    coords: y | it,
    crossOrigin: null,
    data: null,
    dateTime: null,
    decoding: null,
    default: S,
    defer: S,
    dir: null,
    dirName: null,
    disabled: S,
    download: br,
    draggable: B,
    encType: null,
    enterKeyHint: null,
    fetchPriority: null,
    form: null,
    formAction: null,
    formEncType: null,
    formMethod: null,
    formNoValidate: S,
    formTarget: null,
    headers: N,
    height: y,
    hidden: br,
    high: y,
    href: null,
    hrefLang: null,
    htmlFor: N,
    httpEquiv: N,
    id: null,
    imageSizes: null,
    imageSrcSet: null,
    inert: S,
    inputMode: null,
    integrity: null,
    is: null,
    isMap: S,
    itemId: null,
    itemProp: N,
    itemRef: N,
    itemScope: S,
    itemType: N,
    kind: null,
    label: null,
    lang: null,
    language: null,
    list: null,
    loading: null,
    loop: S,
    low: y,
    manifest: null,
    max: null,
    maxLength: y,
    media: null,
    method: null,
    min: null,
    minLength: y,
    multiple: S,
    muted: S,
    name: null,
    nonce: null,
    noModule: S,
    noValidate: S,
    onAbort: null,
    onAfterPrint: null,
    onAuxClick: null,
    onBeforeMatch: null,
    onBeforePrint: null,
    onBeforeToggle: null,
    onBeforeUnload: null,
    onBlur: null,
    onCancel: null,
    onCanPlay: null,
    onCanPlayThrough: null,
    onChange: null,
    onClick: null,
    onClose: null,
    onContextLost: null,
    onContextMenu: null,
    onContextRestored: null,
    onCopy: null,
    onCueChange: null,
    onCut: null,
    onDblClick: null,
    onDrag: null,
    onDragEnd: null,
    onDragEnter: null,
    onDragExit: null,
    onDragLeave: null,
    onDragOver: null,
    onDragStart: null,
    onDrop: null,
    onDurationChange: null,
    onEmptied: null,
    onEnded: null,
    onError: null,
    onFocus: null,
    onFormData: null,
    onHashChange: null,
    onInput: null,
    onInvalid: null,
    onKeyDown: null,
    onKeyPress: null,
    onKeyUp: null,
    onLanguageChange: null,
    onLoad: null,
    onLoadedData: null,
    onLoadedMetadata: null,
    onLoadEnd: null,
    onLoadStart: null,
    onMessage: null,
    onMessageError: null,
    onMouseDown: null,
    onMouseEnter: null,
    onMouseLeave: null,
    onMouseMove: null,
    onMouseOut: null,
    onMouseOver: null,
    onMouseUp: null,
    onOffline: null,
    onOnline: null,
    onPageHide: null,
    onPageShow: null,
    onPaste: null,
    onPause: null,
    onPlay: null,
    onPlaying: null,
    onPopState: null,
    onProgress: null,
    onRateChange: null,
    onRejectionHandled: null,
    onReset: null,
    onResize: null,
    onScroll: null,
    onScrollEnd: null,
    onSecurityPolicyViolation: null,
    onSeeked: null,
    onSeeking: null,
    onSelect: null,
    onSlotChange: null,
    onStalled: null,
    onStorage: null,
    onSubmit: null,
    onSuspend: null,
    onTimeUpdate: null,
    onToggle: null,
    onUnhandledRejection: null,
    onUnload: null,
    onVolumeChange: null,
    onWaiting: null,
    onWheel: null,
    open: S,
    optimum: y,
    pattern: null,
    ping: N,
    placeholder: null,
    playsInline: S,
    popover: null,
    popoverTarget: null,
    popoverTargetAction: null,
    poster: null,
    preload: null,
    readOnly: S,
    referrerPolicy: null,
    rel: N,
    required: S,
    reversed: S,
    rows: y,
    rowSpan: y,
    sandbox: N,
    scope: null,
    scoped: S,
    seamless: S,
    selected: S,
    shadowRootClonable: S,
    shadowRootCustomElementRegistry: S,
    shadowRootDelegatesFocus: S,
    shadowRootMode: null,
    shadowRootSerializable: S,
    shape: null,
    size: y,
    sizes: null,
    slot: null,
    span: y,
    spellCheck: B,
    src: null,
    srcDoc: null,
    srcLang: null,
    srcSet: null,
    start: y,
    step: null,
    style: null,
    tabIndex: y,
    target: null,
    title: null,
    translate: null,
    type: null,
    typeMustMatch: S,
    useMap: null,
    value: B,
    width: y,
    wrap: null,
    writingSuggestions: null,
    // Legacy.
    // See: https://html.spec.whatwg.org/#other-elements,-attributes-and-apis
    align: null,
    // Several. Use CSS `text-align` instead,
    aLink: null,
    // `<body>`. Use CSS `a:active {color}` instead
    archive: N,
    // `<object>`. List of URIs to archives
    axis: null,
    // `<td>` and `<th>`. Use `scope` on `<th>`
    background: null,
    // `<body>`. Use CSS `background-image` instead
    bgColor: null,
    // `<body>` and table elements. Use CSS `background-color` instead
    border: y,
    // `<table>`. Use CSS `border-width` instead,
    borderColor: null,
    // `<table>`. Use CSS `border-color` instead,
    bottomMargin: y,
    // `<body>`
    cellPadding: null,
    // `<table>`
    cellSpacing: null,
    // `<table>`
    char: null,
    // Several table elements. When `align=char`, sets the character to align on
    charOff: null,
    // Several table elements. When `char`, offsets the alignment
    classId: null,
    // `<object>`
    clear: null,
    // `<br>`. Use CSS `clear` instead
    code: null,
    // `<object>`
    codeBase: null,
    // `<object>`
    codeType: null,
    // `<object>`
    color: null,
    // `<font>` and `<hr>`. Use CSS instead
    compact: S,
    // Lists. Use CSS to reduce space between items instead
    declare: S,
    // `<object>`
    event: null,
    // `<script>`
    face: null,
    // `<font>`. Use CSS instead
    frame: null,
    // `<table>`
    frameBorder: null,
    // `<iframe>`. Use CSS `border` instead
    hSpace: y,
    // `<img>` and `<object>`
    leftMargin: y,
    // `<body>`
    link: null,
    // `<body>`. Use CSS `a:link {color: *}` instead
    longDesc: null,
    // `<frame>`, `<iframe>`, and `<img>`. Use an `<a>`
    lowSrc: null,
    // `<img>`. Use a `<picture>`
    marginHeight: y,
    // `<body>`
    marginWidth: y,
    // `<body>`
    noResize: S,
    // `<frame>`
    noHref: S,
    // `<area>`. Use no href instead of an explicit `nohref`
    noShade: S,
    // `<hr>`. Use background-color and height instead of borders
    noWrap: S,
    // `<td>` and `<th>`
    object: null,
    // `<applet>`
    profile: null,
    // `<head>`
    prompt: null,
    // `<isindex>`
    rev: null,
    // `<link>`
    rightMargin: y,
    // `<body>`
    rules: null,
    // `<table>`
    scheme: null,
    // `<meta>`
    scrolling: B,
    // `<frame>`. Use overflow in the child context
    standby: null,
    // `<object>`
    summary: null,
    // `<table>`
    text: null,
    // `<body>`. Use CSS `color` instead
    topMargin: y,
    // `<body>`
    valueType: null,
    // `<param>`
    version: null,
    // `<html>`. Use a doctype.
    vAlign: null,
    // Several. Use CSS `vertical-align` instead
    vLink: null,
    // `<body>`. Use CSS `a:visited {color}` instead
    vSpace: y,
    // `<img>` and `<object>`
    // Non-standard Properties.
    allowTransparency: null,
    autoCorrect: null,
    autoSave: null,
    credentialless: S,
    disablePictureInPicture: S,
    disableRemotePlayback: S,
    exportParts: it,
    part: N,
    prefix: null,
    property: null,
    results: y,
    security: null,
    unselectable: null
  },
  space: "html",
  transform: Wa
}), Wu = zt({
  attributes: {
    accentHeight: "accent-height",
    alignmentBaseline: "alignment-baseline",
    arabicForm: "arabic-form",
    baselineShift: "baseline-shift",
    capHeight: "cap-height",
    className: "class",
    clipPath: "clip-path",
    clipRule: "clip-rule",
    colorInterpolation: "color-interpolation",
    colorInterpolationFilters: "color-interpolation-filters",
    colorProfile: "color-profile",
    colorRendering: "color-rendering",
    crossOrigin: "crossorigin",
    dataType: "datatype",
    dominantBaseline: "dominant-baseline",
    enableBackground: "enable-background",
    fillOpacity: "fill-opacity",
    fillRule: "fill-rule",
    floodColor: "flood-color",
    floodOpacity: "flood-opacity",
    fontFamily: "font-family",
    fontSize: "font-size",
    fontSizeAdjust: "font-size-adjust",
    fontStretch: "font-stretch",
    fontStyle: "font-style",
    fontVariant: "font-variant",
    fontWeight: "font-weight",
    glyphName: "glyph-name",
    glyphOrientationHorizontal: "glyph-orientation-horizontal",
    glyphOrientationVertical: "glyph-orientation-vertical",
    hrefLang: "hreflang",
    horizAdvX: "horiz-adv-x",
    horizOriginX: "horiz-origin-x",
    horizOriginY: "horiz-origin-y",
    imageRendering: "image-rendering",
    letterSpacing: "letter-spacing",
    lightingColor: "lighting-color",
    markerEnd: "marker-end",
    markerMid: "marker-mid",
    markerStart: "marker-start",
    maskType: "mask-type",
    navDown: "nav-down",
    navDownLeft: "nav-down-left",
    navDownRight: "nav-down-right",
    navLeft: "nav-left",
    navNext: "nav-next",
    navPrev: "nav-prev",
    navRight: "nav-right",
    navUp: "nav-up",
    navUpLeft: "nav-up-left",
    navUpRight: "nav-up-right",
    onAbort: "onabort",
    onActivate: "onactivate",
    onAfterPrint: "onafterprint",
    onBeforePrint: "onbeforeprint",
    onBegin: "onbegin",
    onCancel: "oncancel",
    onCanPlay: "oncanplay",
    onCanPlayThrough: "oncanplaythrough",
    onChange: "onchange",
    onClick: "onclick",
    onClose: "onclose",
    onCopy: "oncopy",
    onCueChange: "oncuechange",
    onCut: "oncut",
    onDblClick: "ondblclick",
    onDrag: "ondrag",
    onDragEnd: "ondragend",
    onDragEnter: "ondragenter",
    onDragExit: "ondragexit",
    onDragLeave: "ondragleave",
    onDragOver: "ondragover",
    onDragStart: "ondragstart",
    onDrop: "ondrop",
    onDurationChange: "ondurationchange",
    onEmptied: "onemptied",
    onEnd: "onend",
    onEnded: "onended",
    onError: "onerror",
    onFocus: "onfocus",
    onFocusIn: "onfocusin",
    onFocusOut: "onfocusout",
    onHashChange: "onhashchange",
    onInput: "oninput",
    onInvalid: "oninvalid",
    onKeyDown: "onkeydown",
    onKeyPress: "onkeypress",
    onKeyUp: "onkeyup",
    onLoad: "onload",
    onLoadedData: "onloadeddata",
    onLoadedMetadata: "onloadedmetadata",
    onLoadStart: "onloadstart",
    onMessage: "onmessage",
    onMouseDown: "onmousedown",
    onMouseEnter: "onmouseenter",
    onMouseLeave: "onmouseleave",
    onMouseMove: "onmousemove",
    onMouseOut: "onmouseout",
    onMouseOver: "onmouseover",
    onMouseUp: "onmouseup",
    onMouseWheel: "onmousewheel",
    onOffline: "onoffline",
    onOnline: "ononline",
    onPageHide: "onpagehide",
    onPageShow: "onpageshow",
    onPaste: "onpaste",
    onPause: "onpause",
    onPlay: "onplay",
    onPlaying: "onplaying",
    onPopState: "onpopstate",
    onProgress: "onprogress",
    onRateChange: "onratechange",
    onRepeat: "onrepeat",
    onReset: "onreset",
    onResize: "onresize",
    onScroll: "onscroll",
    onSeeked: "onseeked",
    onSeeking: "onseeking",
    onSelect: "onselect",
    onShow: "onshow",
    onStalled: "onstalled",
    onStorage: "onstorage",
    onSubmit: "onsubmit",
    onSuspend: "onsuspend",
    onTimeUpdate: "ontimeupdate",
    onToggle: "ontoggle",
    onUnload: "onunload",
    onVolumeChange: "onvolumechange",
    onWaiting: "onwaiting",
    onZoom: "onzoom",
    overlinePosition: "overline-position",
    overlineThickness: "overline-thickness",
    paintOrder: "paint-order",
    panose1: "panose-1",
    pointerEvents: "pointer-events",
    referrerPolicy: "referrerpolicy",
    renderingIntent: "rendering-intent",
    shapeRendering: "shape-rendering",
    stopColor: "stop-color",
    stopOpacity: "stop-opacity",
    strikethroughPosition: "strikethrough-position",
    strikethroughThickness: "strikethrough-thickness",
    strokeDashArray: "stroke-dasharray",
    strokeDashOffset: "stroke-dashoffset",
    strokeLineCap: "stroke-linecap",
    strokeLineJoin: "stroke-linejoin",
    strokeMiterLimit: "stroke-miterlimit",
    strokeOpacity: "stroke-opacity",
    strokeWidth: "stroke-width",
    tabIndex: "tabindex",
    textAnchor: "text-anchor",
    textDecoration: "text-decoration",
    textRendering: "text-rendering",
    transformOrigin: "transform-origin",
    typeOf: "typeof",
    underlinePosition: "underline-position",
    underlineThickness: "underline-thickness",
    unicodeBidi: "unicode-bidi",
    unicodeRange: "unicode-range",
    unitsPerEm: "units-per-em",
    vAlphabetic: "v-alphabetic",
    vHanging: "v-hanging",
    vIdeographic: "v-ideographic",
    vMathematical: "v-mathematical",
    vectorEffect: "vector-effect",
    vertAdvY: "vert-adv-y",
    vertOriginX: "vert-origin-x",
    vertOriginY: "vert-origin-y",
    wordSpacing: "word-spacing",
    writingMode: "writing-mode",
    xHeight: "x-height",
    // These were camelcased in Tiny. Now lowercased in SVG 2
    playbackOrder: "playbackorder",
    timelineBegin: "timelinebegin"
  },
  properties: {
    about: te,
    accentHeight: y,
    accumulate: null,
    additive: null,
    alignmentBaseline: null,
    alphabetic: y,
    amplitude: y,
    arabicForm: null,
    ascent: y,
    attributeName: null,
    attributeType: null,
    azimuth: y,
    bandwidth: null,
    baselineShift: null,
    baseFrequency: null,
    baseProfile: null,
    bbox: null,
    begin: null,
    bias: y,
    by: null,
    calcMode: null,
    capHeight: y,
    className: N,
    clip: null,
    clipPath: null,
    clipPathUnits: null,
    clipRule: null,
    color: null,
    colorInterpolation: null,
    colorInterpolationFilters: null,
    colorProfile: null,
    colorRendering: null,
    content: null,
    contentScriptType: null,
    contentStyleType: null,
    crossOrigin: null,
    cursor: null,
    cx: null,
    cy: null,
    d: null,
    dataType: null,
    defaultAction: null,
    descent: y,
    diffuseConstant: y,
    direction: null,
    display: null,
    dur: null,
    divisor: y,
    dominantBaseline: null,
    download: S,
    dx: null,
    dy: null,
    edgeMode: null,
    editable: null,
    elevation: y,
    enableBackground: null,
    end: null,
    event: null,
    exponent: y,
    externalResourcesRequired: null,
    fill: null,
    fillOpacity: y,
    fillRule: null,
    filter: null,
    filterRes: null,
    filterUnits: null,
    floodColor: null,
    floodOpacity: null,
    focusable: null,
    focusHighlight: null,
    fontFamily: null,
    fontSize: null,
    fontSizeAdjust: null,
    fontStretch: null,
    fontStyle: null,
    fontVariant: null,
    fontWeight: null,
    format: null,
    fr: null,
    from: null,
    fx: null,
    fy: null,
    g1: it,
    g2: it,
    glyphName: it,
    glyphOrientationHorizontal: null,
    glyphOrientationVertical: null,
    glyphRef: null,
    gradientTransform: null,
    gradientUnits: null,
    handler: null,
    hanging: y,
    hatchContentUnits: null,
    hatchUnits: null,
    height: null,
    href: null,
    hrefLang: null,
    horizAdvX: y,
    horizOriginX: y,
    horizOriginY: y,
    id: null,
    ideographic: y,
    imageRendering: null,
    initialVisibility: null,
    in: null,
    in2: null,
    intercept: y,
    k: y,
    k1: y,
    k2: y,
    k3: y,
    k4: y,
    kernelMatrix: te,
    kernelUnitLength: null,
    keyPoints: null,
    // SEMI_COLON_SEPARATED
    keySplines: null,
    // SEMI_COLON_SEPARATED
    keyTimes: null,
    // SEMI_COLON_SEPARATED
    kerning: null,
    lang: null,
    lengthAdjust: null,
    letterSpacing: null,
    lightingColor: null,
    limitingConeAngle: y,
    local: null,
    markerEnd: null,
    markerMid: null,
    markerStart: null,
    markerHeight: null,
    markerUnits: null,
    markerWidth: null,
    mask: null,
    maskContentUnits: null,
    maskType: null,
    maskUnits: null,
    mathematical: null,
    max: null,
    media: null,
    mediaCharacterEncoding: null,
    mediaContentEncodings: null,
    mediaSize: y,
    mediaTime: null,
    method: null,
    min: null,
    mode: null,
    name: null,
    navDown: null,
    navDownLeft: null,
    navDownRight: null,
    navLeft: null,
    navNext: null,
    navPrev: null,
    navRight: null,
    navUp: null,
    navUpLeft: null,
    navUpRight: null,
    numOctaves: null,
    observer: null,
    offset: null,
    onAbort: null,
    onActivate: null,
    onAfterPrint: null,
    onBeforePrint: null,
    onBegin: null,
    onCancel: null,
    onCanPlay: null,
    onCanPlayThrough: null,
    onChange: null,
    onClick: null,
    onClose: null,
    onCopy: null,
    onCueChange: null,
    onCut: null,
    onDblClick: null,
    onDrag: null,
    onDragEnd: null,
    onDragEnter: null,
    onDragExit: null,
    onDragLeave: null,
    onDragOver: null,
    onDragStart: null,
    onDrop: null,
    onDurationChange: null,
    onEmptied: null,
    onEnd: null,
    onEnded: null,
    onError: null,
    onFocus: null,
    onFocusIn: null,
    onFocusOut: null,
    onHashChange: null,
    onInput: null,
    onInvalid: null,
    onKeyDown: null,
    onKeyPress: null,
    onKeyUp: null,
    onLoad: null,
    onLoadedData: null,
    onLoadedMetadata: null,
    onLoadStart: null,
    onMessage: null,
    onMouseDown: null,
    onMouseEnter: null,
    onMouseLeave: null,
    onMouseMove: null,
    onMouseOut: null,
    onMouseOver: null,
    onMouseUp: null,
    onMouseWheel: null,
    onOffline: null,
    onOnline: null,
    onPageHide: null,
    onPageShow: null,
    onPaste: null,
    onPause: null,
    onPlay: null,
    onPlaying: null,
    onPopState: null,
    onProgress: null,
    onRateChange: null,
    onRepeat: null,
    onReset: null,
    onResize: null,
    onScroll: null,
    onSeeked: null,
    onSeeking: null,
    onSelect: null,
    onShow: null,
    onStalled: null,
    onStorage: null,
    onSubmit: null,
    onSuspend: null,
    onTimeUpdate: null,
    onToggle: null,
    onUnload: null,
    onVolumeChange: null,
    onWaiting: null,
    onZoom: null,
    opacity: null,
    operator: null,
    order: null,
    orient: null,
    orientation: null,
    origin: null,
    overflow: null,
    overlay: null,
    overlinePosition: y,
    overlineThickness: y,
    paintOrder: null,
    panose1: null,
    path: null,
    pathLength: y,
    patternContentUnits: null,
    patternTransform: null,
    patternUnits: null,
    phase: null,
    ping: N,
    pitch: null,
    playbackOrder: null,
    pointerEvents: null,
    points: null,
    pointsAtX: y,
    pointsAtY: y,
    pointsAtZ: y,
    preserveAlpha: null,
    preserveAspectRatio: null,
    primitiveUnits: null,
    propagate: null,
    property: te,
    r: null,
    radius: null,
    referrerPolicy: null,
    refX: null,
    refY: null,
    rel: te,
    rev: te,
    renderingIntent: null,
    repeatCount: null,
    repeatDur: null,
    requiredExtensions: te,
    requiredFeatures: te,
    requiredFonts: te,
    requiredFormats: te,
    resource: null,
    restart: null,
    result: null,
    rotate: null,
    rx: null,
    ry: null,
    scale: null,
    seed: null,
    shapeRendering: null,
    side: null,
    slope: null,
    snapshotTime: null,
    specularConstant: y,
    specularExponent: y,
    spreadMethod: null,
    spacing: null,
    startOffset: null,
    stdDeviation: null,
    stemh: null,
    stemv: null,
    stitchTiles: null,
    stopColor: null,
    stopOpacity: null,
    strikethroughPosition: y,
    strikethroughThickness: y,
    string: null,
    stroke: null,
    strokeDashArray: te,
    strokeDashOffset: null,
    strokeLineCap: null,
    strokeLineJoin: null,
    strokeMiterLimit: y,
    strokeOpacity: y,
    strokeWidth: null,
    style: null,
    surfaceScale: y,
    syncBehavior: null,
    syncBehaviorDefault: null,
    syncMaster: null,
    syncTolerance: null,
    syncToleranceDefault: null,
    systemLanguage: te,
    tabIndex: y,
    tableValues: null,
    target: null,
    targetX: y,
    targetY: y,
    textAnchor: null,
    textDecoration: null,
    textRendering: null,
    textLength: null,
    timelineBegin: null,
    title: null,
    transformBehavior: null,
    type: null,
    typeOf: te,
    to: null,
    transform: null,
    transformOrigin: null,
    u1: null,
    u2: null,
    underlinePosition: y,
    underlineThickness: y,
    unicode: null,
    unicodeBidi: null,
    unicodeRange: null,
    unitsPerEm: y,
    values: null,
    vAlphabetic: y,
    vMathematical: y,
    vectorEffect: null,
    vHanging: y,
    vIdeographic: y,
    version: null,
    vertAdvY: y,
    vertOriginX: y,
    vertOriginY: y,
    viewBox: null,
    viewTarget: null,
    visibility: null,
    width: null,
    widths: null,
    wordSpacing: null,
    writingMode: null,
    x: null,
    x1: null,
    x2: null,
    xChannelSelector: null,
    xHeight: y,
    y: null,
    y1: null,
    y2: null,
    yChannelSelector: null,
    z: null,
    zoomAndPan: null
  },
  space: "svg",
  transform: Ha
}), Qa = zt({
  properties: {
    xLinkActuate: null,
    xLinkArcRole: null,
    xLinkHref: null,
    xLinkRole: null,
    xLinkShow: null,
    xLinkTitle: null,
    xLinkType: null
  },
  space: "xlink",
  transform(n, e) {
    return "xlink:" + e.slice(5).toLowerCase();
  }
}), Va = zt({
  attributes: { xmlnsxlink: "xmlns:xlink" },
  properties: { xmlnsXLink: null, xmlns: null },
  space: "xmlns",
  transform: Wa
}), Ka = zt({
  properties: { xmlBase: null, xmlLang: null, xmlSpace: null },
  space: "xml",
  transform(n, e) {
    return "xml:" + e.slice(3).toLowerCase();
  }
}), Qu = /[A-Z]/g, wi = /-[a-z]/g, Vu = /^data[-\w.:]+$/i;
function Ku(n, e) {
  const t = mr(e);
  let s = e, r = Y;
  if (t in n.normal)
    return n.property[n.normal[t]];
  if (t.length > 4 && t.slice(0, 4) === "data" && Vu.test(e)) {
    if (e.charAt(4) === "-") {
      const o = e.slice(5).replace(wi, Xu);
      s = "data" + o.charAt(0).toUpperCase() + o.slice(1);
    } else {
      const o = e.slice(4);
      if (!wi.test(o)) {
        let i = o.replace(Qu, Zu);
        i.charAt(0) !== "-" && (i = "-" + i), e = "data" + i;
      }
    }
    r = qr;
  }
  return new r(s, e);
}
function Zu(n) {
  return "-" + n.toLowerCase();
}
function Xu(n) {
  return n.charAt(1).toUpperCase();
}
const Ju = ja([qa, Hu, Qa, Va, Ka], "html"), Za = ja([qa, Wu, Qa, Va, Ka], "svg"), xi = {}.hasOwnProperty;
function Yu(n, e) {
  const t = e || {};
  function s(r, ...o) {
    let i = s.invalid;
    const a = s.handlers;
    if (r && xi.call(r, n)) {
      const l = String(r[n]);
      i = xi.call(a, l) ? a[l] : s.unknown;
    }
    if (i)
      return i.call(this, r, ...o);
  }
  return s.handlers = t.handlers || {}, s.invalid = t.invalid, s.unknown = t.unknown, s;
}
const eh = /["&'<>`]/g, th = /[\uD800-\uDBFF][\uDC00-\uDFFF]/g, nh = (
  // eslint-disable-next-line no-control-regex, unicorn/no-hex-escape
  /[\x01-\t\v\f\x0E-\x1F\x7F\x81\x8D\x8F\x90\x9D\xA0-\uFFFF]/g
), sh = /[|\\{}()[\]^$+*?.]/g, ki = /* @__PURE__ */ new WeakMap();
function rh(n, e) {
  if (n = n.replace(
    e.subset ? oh(e.subset) : eh,
    s
  ), e.subset || e.escapeOnly)
    return n;
  return n.replace(th, t).replace(nh, s);
  function t(r, o, i) {
    return e.format(
      (r.charCodeAt(0) - 55296) * 1024 + r.charCodeAt(1) - 56320 + 65536,
      i.charCodeAt(o + 2),
      e
    );
  }
  function s(r, o, i) {
    return e.format(
      r.charCodeAt(0),
      i.charCodeAt(o + 1),
      e
    );
  }
}
function oh(n) {
  let e = ki.get(n);
  return e || (e = ih(n), ki.set(n, e)), e;
}
function ih(n) {
  const e = [];
  let t = -1;
  for (; ++t < n.length; )
    e.push(n[t].replace(sh, "\\$&"));
  return new RegExp("(?:" + e.join("|") + ")", "g");
}
const ah = /[\dA-Fa-f]/;
function lh(n, e, t) {
  const s = "&#x" + n.toString(16).toUpperCase();
  return t && e && !ah.test(String.fromCharCode(e)) ? s : s + ";";
}
const ch = /\d/;
function uh(n, e, t) {
  const s = "&#" + String(n);
  return t && e && !ch.test(String.fromCharCode(e)) ? s : s + ";";
}
const hh = [
  "AElig",
  "AMP",
  "Aacute",
  "Acirc",
  "Agrave",
  "Aring",
  "Atilde",
  "Auml",
  "COPY",
  "Ccedil",
  "ETH",
  "Eacute",
  "Ecirc",
  "Egrave",
  "Euml",
  "GT",
  "Iacute",
  "Icirc",
  "Igrave",
  "Iuml",
  "LT",
  "Ntilde",
  "Oacute",
  "Ocirc",
  "Ograve",
  "Oslash",
  "Otilde",
  "Ouml",
  "QUOT",
  "REG",
  "THORN",
  "Uacute",
  "Ucirc",
  "Ugrave",
  "Uuml",
  "Yacute",
  "aacute",
  "acirc",
  "acute",
  "aelig",
  "agrave",
  "amp",
  "aring",
  "atilde",
  "auml",
  "brvbar",
  "ccedil",
  "cedil",
  "cent",
  "copy",
  "curren",
  "deg",
  "divide",
  "eacute",
  "ecirc",
  "egrave",
  "eth",
  "euml",
  "frac12",
  "frac14",
  "frac34",
  "gt",
  "iacute",
  "icirc",
  "iexcl",
  "igrave",
  "iquest",
  "iuml",
  "laquo",
  "lt",
  "macr",
  "micro",
  "middot",
  "nbsp",
  "not",
  "ntilde",
  "oacute",
  "ocirc",
  "ograve",
  "ordf",
  "ordm",
  "oslash",
  "otilde",
  "ouml",
  "para",
  "plusmn",
  "pound",
  "quot",
  "raquo",
  "reg",
  "sect",
  "shy",
  "sup1",
  "sup2",
  "sup3",
  "szlig",
  "thorn",
  "times",
  "uacute",
  "ucirc",
  "ugrave",
  "uml",
  "uuml",
  "yacute",
  "yen",
  "yuml"
], Ws = {
  nbsp: " ",
  iexcl: "¡",
  cent: "¢",
  pound: "£",
  curren: "¤",
  yen: "¥",
  brvbar: "¦",
  sect: "§",
  uml: "¨",
  copy: "©",
  ordf: "ª",
  laquo: "«",
  not: "¬",
  shy: "­",
  reg: "®",
  macr: "¯",
  deg: "°",
  plusmn: "±",
  sup2: "²",
  sup3: "³",
  acute: "´",
  micro: "µ",
  para: "¶",
  middot: "·",
  cedil: "¸",
  sup1: "¹",
  ordm: "º",
  raquo: "»",
  frac14: "¼",
  frac12: "½",
  frac34: "¾",
  iquest: "¿",
  Agrave: "À",
  Aacute: "Á",
  Acirc: "Â",
  Atilde: "Ã",
  Auml: "Ä",
  Aring: "Å",
  AElig: "Æ",
  Ccedil: "Ç",
  Egrave: "È",
  Eacute: "É",
  Ecirc: "Ê",
  Euml: "Ë",
  Igrave: "Ì",
  Iacute: "Í",
  Icirc: "Î",
  Iuml: "Ï",
  ETH: "Ð",
  Ntilde: "Ñ",
  Ograve: "Ò",
  Oacute: "Ó",
  Ocirc: "Ô",
  Otilde: "Õ",
  Ouml: "Ö",
  times: "×",
  Oslash: "Ø",
  Ugrave: "Ù",
  Uacute: "Ú",
  Ucirc: "Û",
  Uuml: "Ü",
  Yacute: "Ý",
  THORN: "Þ",
  szlig: "ß",
  agrave: "à",
  aacute: "á",
  acirc: "â",
  atilde: "ã",
  auml: "ä",
  aring: "å",
  aelig: "æ",
  ccedil: "ç",
  egrave: "è",
  eacute: "é",
  ecirc: "ê",
  euml: "ë",
  igrave: "ì",
  iacute: "í",
  icirc: "î",
  iuml: "ï",
  eth: "ð",
  ntilde: "ñ",
  ograve: "ò",
  oacute: "ó",
  ocirc: "ô",
  otilde: "õ",
  ouml: "ö",
  divide: "÷",
  oslash: "ø",
  ugrave: "ù",
  uacute: "ú",
  ucirc: "û",
  uuml: "ü",
  yacute: "ý",
  thorn: "þ",
  yuml: "ÿ",
  fnof: "ƒ",
  Alpha: "Α",
  Beta: "Β",
  Gamma: "Γ",
  Delta: "Δ",
  Epsilon: "Ε",
  Zeta: "Ζ",
  Eta: "Η",
  Theta: "Θ",
  Iota: "Ι",
  Kappa: "Κ",
  Lambda: "Λ",
  Mu: "Μ",
  Nu: "Ν",
  Xi: "Ξ",
  Omicron: "Ο",
  Pi: "Π",
  Rho: "Ρ",
  Sigma: "Σ",
  Tau: "Τ",
  Upsilon: "Υ",
  Phi: "Φ",
  Chi: "Χ",
  Psi: "Ψ",
  Omega: "Ω",
  alpha: "α",
  beta: "β",
  gamma: "γ",
  delta: "δ",
  epsilon: "ε",
  zeta: "ζ",
  eta: "η",
  theta: "θ",
  iota: "ι",
  kappa: "κ",
  lambda: "λ",
  mu: "μ",
  nu: "ν",
  xi: "ξ",
  omicron: "ο",
  pi: "π",
  rho: "ρ",
  sigmaf: "ς",
  sigma: "σ",
  tau: "τ",
  upsilon: "υ",
  phi: "φ",
  chi: "χ",
  psi: "ψ",
  omega: "ω",
  thetasym: "ϑ",
  upsih: "ϒ",
  piv: "ϖ",
  bull: "•",
  hellip: "…",
  prime: "′",
  Prime: "″",
  oline: "‾",
  frasl: "⁄",
  weierp: "℘",
  image: "ℑ",
  real: "ℜ",
  trade: "™",
  alefsym: "ℵ",
  larr: "←",
  uarr: "↑",
  rarr: "→",
  darr: "↓",
  harr: "↔",
  crarr: "↵",
  lArr: "⇐",
  uArr: "⇑",
  rArr: "⇒",
  dArr: "⇓",
  hArr: "⇔",
  forall: "∀",
  part: "∂",
  exist: "∃",
  empty: "∅",
  nabla: "∇",
  isin: "∈",
  notin: "∉",
  ni: "∋",
  prod: "∏",
  sum: "∑",
  minus: "−",
  lowast: "∗",
  radic: "√",
  prop: "∝",
  infin: "∞",
  ang: "∠",
  and: "∧",
  or: "∨",
  cap: "∩",
  cup: "∪",
  int: "∫",
  there4: "∴",
  sim: "∼",
  cong: "≅",
  asymp: "≈",
  ne: "≠",
  equiv: "≡",
  le: "≤",
  ge: "≥",
  sub: "⊂",
  sup: "⊃",
  nsub: "⊄",
  sube: "⊆",
  supe: "⊇",
  oplus: "⊕",
  otimes: "⊗",
  perp: "⊥",
  sdot: "⋅",
  lceil: "⌈",
  rceil: "⌉",
  lfloor: "⌊",
  rfloor: "⌋",
  lang: "〈",
  rang: "〉",
  loz: "◊",
  spades: "♠",
  clubs: "♣",
  hearts: "♥",
  diams: "♦",
  quot: '"',
  amp: "&",
  lt: "<",
  gt: ">",
  OElig: "Œ",
  oelig: "œ",
  Scaron: "Š",
  scaron: "š",
  Yuml: "Ÿ",
  circ: "ˆ",
  tilde: "˜",
  ensp: " ",
  emsp: " ",
  thinsp: " ",
  zwnj: "‌",
  zwj: "‍",
  lrm: "‎",
  rlm: "‏",
  ndash: "–",
  mdash: "—",
  lsquo: "‘",
  rsquo: "’",
  sbquo: "‚",
  ldquo: "“",
  rdquo: "”",
  bdquo: "„",
  dagger: "†",
  Dagger: "‡",
  permil: "‰",
  lsaquo: "‹",
  rsaquo: "›",
  euro: "€"
}, ph = [
  "cent",
  "copy",
  "divide",
  "gt",
  "lt",
  "not",
  "para",
  "times"
], Xa = {}.hasOwnProperty, vr = {};
let Ln;
for (Ln in Ws)
  Xa.call(Ws, Ln) && (vr[Ws[Ln]] = Ln);
const dh = /[^\dA-Za-z]/;
function fh(n, e, t, s) {
  const r = String.fromCharCode(n);
  if (Xa.call(vr, r)) {
    const o = vr[r], i = "&" + o;
    return t && hh.includes(o) && !ph.includes(o) && (!s || e && e !== 61 && dh.test(String.fromCharCode(e))) ? i : i + ";";
  }
  return "";
}
function gh(n, e, t) {
  let s = lh(n, e, t.omitOptionalSemicolons), r;
  if ((t.useNamedReferences || t.useShortestReferences) && (r = fh(
    n,
    e,
    t.omitOptionalSemicolons,
    t.attribute
  )), (t.useShortestReferences || !r) && t.useShortestReferences) {
    const o = uh(n, e, t.omitOptionalSemicolons);
    o.length < s.length && (s = o);
  }
  return r && (!t.useShortestReferences || r.length < s.length) ? r : s;
}
function $t(n, e) {
  return rh(n, Object.assign({ format: gh }, e));
}
const mh = /^>|^->|<!--|-->|--!>|<!-$/g, bh = [">"], yh = ["<", ">"];
function vh(n, e, t, s) {
  return s.settings.bogusComments ? "<?" + $t(
    n.value,
    Object.assign({}, s.settings.characterReferences, {
      subset: bh
    })
  ) + ">" : "<!--" + n.value.replace(mh, r) + "-->";
  function r(o) {
    return $t(
      o,
      Object.assign({}, s.settings.characterReferences, {
        subset: yh
      })
    );
  }
}
function _h(n, e, t, s) {
  return "<!" + (s.settings.upperDoctype ? "DOCTYPE" : "doctype") + (s.settings.tightDoctype ? "" : " ") + "html>";
}
function Ci(n, e) {
  const t = String(n);
  if (typeof e != "string")
    throw new TypeError("Expected character");
  let s = 0, r = t.indexOf(e);
  for (; r !== -1; )
    s++, r = t.indexOf(e, r + e.length);
  return s;
}
function wh(n, e) {
  const t = e || {};
  return (n[n.length - 1] === "" ? [...n, ""] : n).join(
    (t.padRight ? " " : "") + "," + (t.padLeft === !1 ? "" : " ")
  ).trim();
}
function xh(n) {
  return n.join(" ").trim();
}
const kh = /[ \t\n\f\r]/g;
function Hr(n) {
  return typeof n == "object" ? n.type === "text" ? $i(n.value) : !1 : $i(n);
}
function $i(n) {
  return n.replace(kh, "") === "";
}
const G = Ya(1), Ja = Ya(-1), Ch = [];
function Ya(n) {
  return e;
  function e(t, s, r) {
    const o = t ? t.children : Ch;
    let i = (s || 0) + n, a = o[i];
    if (!r)
      for (; a && Hr(a); )
        i += n, a = o[i];
    return a;
  }
}
const $h = {}.hasOwnProperty;
function el(n) {
  return e;
  function e(t, s, r) {
    return $h.call(n, t.tagName) && n[t.tagName](t, s, r);
  }
}
const Wr = el({
  body: Ah,
  caption: Qs,
  colgroup: Qs,
  dd: Ih,
  dt: Th,
  head: Qs,
  html: Sh,
  li: Rh,
  optgroup: Ph,
  option: Lh,
  p: Eh,
  rp: Si,
  rt: Si,
  tbody: Mh,
  td: Ai,
  tfoot: Oh,
  th: Ai,
  thead: Nh,
  tr: zh
});
function Qs(n, e, t) {
  const s = G(t, e, !0);
  return !s || s.type !== "comment" && !(s.type === "text" && Hr(s.value.charAt(0)));
}
function Sh(n, e, t) {
  const s = G(t, e);
  return !s || s.type !== "comment";
}
function Ah(n, e, t) {
  const s = G(t, e);
  return !s || s.type !== "comment";
}
function Eh(n, e, t) {
  const s = G(t, e);
  return s ? s.type === "element" && (s.tagName === "address" || s.tagName === "article" || s.tagName === "aside" || s.tagName === "blockquote" || s.tagName === "details" || s.tagName === "div" || s.tagName === "dl" || s.tagName === "fieldset" || s.tagName === "figcaption" || s.tagName === "figure" || s.tagName === "footer" || s.tagName === "form" || s.tagName === "h1" || s.tagName === "h2" || s.tagName === "h3" || s.tagName === "h4" || s.tagName === "h5" || s.tagName === "h6" || s.tagName === "header" || s.tagName === "hgroup" || s.tagName === "hr" || s.tagName === "main" || s.tagName === "menu" || s.tagName === "nav" || s.tagName === "ol" || s.tagName === "p" || s.tagName === "pre" || s.tagName === "section" || s.tagName === "table" || s.tagName === "ul") : !t || // Confusing parent.
  !(t.type === "element" && (t.tagName === "a" || t.tagName === "audio" || t.tagName === "del" || t.tagName === "ins" || t.tagName === "map" || t.tagName === "noscript" || t.tagName === "video"));
}
function Rh(n, e, t) {
  const s = G(t, e);
  return !s || s.type === "element" && s.tagName === "li";
}
function Th(n, e, t) {
  const s = G(t, e);
  return !!(s && s.type === "element" && (s.tagName === "dt" || s.tagName === "dd"));
}
function Ih(n, e, t) {
  const s = G(t, e);
  return !s || s.type === "element" && (s.tagName === "dt" || s.tagName === "dd");
}
function Si(n, e, t) {
  const s = G(t, e);
  return !s || s.type === "element" && (s.tagName === "rp" || s.tagName === "rt");
}
function Ph(n, e, t) {
  const s = G(t, e);
  return !s || s.type === "element" && s.tagName === "optgroup";
}
function Lh(n, e, t) {
  const s = G(t, e);
  return !s || s.type === "element" && (s.tagName === "option" || s.tagName === "optgroup");
}
function Nh(n, e, t) {
  const s = G(t, e);
  return !!(s && s.type === "element" && (s.tagName === "tbody" || s.tagName === "tfoot"));
}
function Mh(n, e, t) {
  const s = G(t, e);
  return !s || s.type === "element" && (s.tagName === "tbody" || s.tagName === "tfoot");
}
function Oh(n, e, t) {
  return !G(t, e);
}
function zh(n, e, t) {
  const s = G(t, e);
  return !s || s.type === "element" && s.tagName === "tr";
}
function Ai(n, e, t) {
  const s = G(t, e);
  return !s || s.type === "element" && (s.tagName === "td" || s.tagName === "th");
}
const Bh = el({
  body: Gh,
  colgroup: Uh,
  head: Fh,
  html: Dh,
  tbody: jh
});
function Dh(n) {
  const e = G(n, -1);
  return !e || e.type !== "comment";
}
function Fh(n) {
  const e = /* @__PURE__ */ new Set();
  for (const s of n.children)
    if (s.type === "element" && (s.tagName === "base" || s.tagName === "title")) {
      if (e.has(s.tagName)) return !1;
      e.add(s.tagName);
    }
  const t = n.children[0];
  return !t || t.type === "element";
}
function Gh(n) {
  const e = G(n, -1, !0);
  return !e || e.type !== "comment" && !(e.type === "text" && Hr(e.value.charAt(0))) && !(e.type === "element" && (e.tagName === "meta" || e.tagName === "link" || e.tagName === "script" || e.tagName === "style" || e.tagName === "template"));
}
function Uh(n, e, t) {
  const s = Ja(t, e), r = G(n, -1, !0);
  return t && s && s.type === "element" && s.tagName === "colgroup" && Wr(s, t.children.indexOf(s), t) ? !1 : !!(r && r.type === "element" && r.tagName === "col");
}
function jh(n, e, t) {
  const s = Ja(t, e), r = G(n, -1);
  return t && s && s.type === "element" && (s.tagName === "thead" || s.tagName === "tbody") && Wr(s, t.children.indexOf(s), t) ? !1 : !!(r && r.type === "element" && r.tagName === "tr");
}
const Nn = {
  // See: <https://html.spec.whatwg.org/#attribute-name-state>.
  name: [
    [`	
\f\r &/=>`.split(""), `	
\f\r "&'/=>\``.split("")],
    [`\0	
\f\r "&'/<=>`.split(""), `\0	
\f\r "&'/<=>\``.split("")]
  ],
  // See: <https://html.spec.whatwg.org/#attribute-value-(unquoted)-state>.
  unquoted: [
    [`	
\f\r &>`.split(""), `\0	
\f\r "&'<=>\``.split("")],
    [`\0	
\f\r "&'<=>\``.split(""), `\0	
\f\r "&'<=>\``.split("")]
  ],
  // See: <https://html.spec.whatwg.org/#attribute-value-(single-quoted)-state>.
  single: [
    ["&'".split(""), "\"&'`".split("")],
    ["\0&'".split(""), "\0\"&'`".split("")]
  ],
  // See: <https://html.spec.whatwg.org/#attribute-value-(double-quoted)-state>.
  double: [
    ['"&'.split(""), "\"&'`".split("")],
    ['\0"&'.split(""), "\0\"&'`".split("")]
  ]
};
function qh(n, e, t, s) {
  const r = s.schema, o = r.space === "svg" ? !1 : s.settings.omitOptionalTags;
  let i = r.space === "svg" ? s.settings.closeEmptyElements : s.settings.voids.includes(n.tagName.toLowerCase());
  const a = [];
  let l;
  r.space === "html" && n.tagName === "svg" && (s.schema = Za);
  const c = Hh(s, n.properties), h = s.all(
    r.space === "html" && n.tagName === "template" ? n.content : n
  );
  return s.schema = r, h && (i = !1), (c || !o || !Bh(n, e, t)) && (a.push("<", n.tagName, c ? " " + c : ""), i && (r.space === "svg" || s.settings.closeSelfClosing) && (l = c.charAt(c.length - 1), (!s.settings.tightSelfClosing || l === "/" || l && l !== '"' && l !== "'") && a.push(" "), a.push("/")), a.push(">")), a.push(h), !i && (!o || !Wr(n, e, t)) && a.push("</" + n.tagName + ">"), a.join("");
}
function Hh(n, e) {
  const t = [];
  let s = -1, r;
  if (e) {
    for (r in e)
      if (e[r] !== null && e[r] !== void 0) {
        const o = Wh(n, r, e[r]);
        o && t.push(o);
      }
  }
  for (; ++s < t.length; ) {
    const o = n.settings.tightAttributes ? t[s].charAt(t[s].length - 1) : void 0;
    s !== t.length - 1 && o !== '"' && o !== "'" && (t[s] += " ");
  }
  return t.join("");
}
function Wh(n, e, t) {
  const s = Ku(n.schema, e), r = n.settings.allowParseErrors && n.schema.space === "html" ? 0 : 1, o = n.settings.allowDangerousCharacters ? 0 : 1;
  let i = n.quote, a;
  if (s.overloadedBoolean && (t === s.attribute || t === "") ? t = !0 : (s.boolean || s.overloadedBoolean) && (typeof t != "string" || t === s.attribute || t === "") && (t = !!t), t == null || t === !1 || typeof t == "number" && Number.isNaN(t))
    return "";
  const l = $t(
    s.attribute,
    Object.assign({}, n.settings.characterReferences, {
      // Always encode without parse errors in non-HTML.
      subset: Nn.name[r][o]
    })
  );
  return t === !0 || (t = Array.isArray(t) ? (s.commaSeparated ? wh : xh)(t, {
    padLeft: !n.settings.tightCommaSeparatedLists
  }) : String(t), n.settings.collapseEmptyAttributes && !t) ? l : (n.settings.preferUnquoted && (a = $t(
    t,
    Object.assign({}, n.settings.characterReferences, {
      attribute: !0,
      subset: Nn.unquoted[r][o]
    })
  )), a !== t && (n.settings.quoteSmart && Ci(t, i) > Ci(t, n.alternative) && (i = n.alternative), a = i + $t(
    t,
    Object.assign({}, n.settings.characterReferences, {
      // Always encode without parse errors in non-HTML.
      subset: (i === "'" ? Nn.single : Nn.double)[r][o],
      attribute: !0
    })
  ) + i), l + (a && "=" + a));
}
const Qh = ["<", "&"];
function tl(n, e, t, s) {
  return t && t.type === "element" && (t.tagName === "script" || t.tagName === "style") ? n.value : $t(
    n.value,
    Object.assign({}, s.settings.characterReferences, {
      subset: Qh
    })
  );
}
function Vh(n, e, t, s) {
  return s.settings.allowDangerousHtml ? n.value : tl(n, e, t, s);
}
function Kh(n, e, t, s) {
  return s.all(n);
}
const Zh = Yu("type", {
  invalid: Xh,
  unknown: Jh,
  handlers: { comment: vh, doctype: _h, element: qh, raw: Vh, root: Kh, text: tl }
});
function Xh(n) {
  throw new Error("Expected node, not `" + n + "`");
}
function Jh(n) {
  const e = (
    /** @type {Nodes} */
    n
  );
  throw new Error("Cannot compile unknown node `" + e.type + "`");
}
const Yh = {}, ep = {}, tp = [];
function np(n, e) {
  const t = e || Yh, s = t.quote || '"', r = s === '"' ? "'" : '"';
  if (s !== '"' && s !== "'")
    throw new Error("Invalid quote `" + s + "`, expected `'` or `\"`");
  return {
    one: sp,
    all: rp,
    settings: {
      omitOptionalTags: t.omitOptionalTags || !1,
      allowParseErrors: t.allowParseErrors || !1,
      allowDangerousCharacters: t.allowDangerousCharacters || !1,
      quoteSmart: t.quoteSmart || !1,
      preferUnquoted: t.preferUnquoted || !1,
      tightAttributes: t.tightAttributes || !1,
      upperDoctype: t.upperDoctype || !1,
      tightDoctype: t.tightDoctype || !1,
      bogusComments: t.bogusComments || !1,
      tightCommaSeparatedLists: t.tightCommaSeparatedLists || !1,
      tightSelfClosing: t.tightSelfClosing || !1,
      collapseEmptyAttributes: t.collapseEmptyAttributes || !1,
      allowDangerousHtml: t.allowDangerousHtml || !1,
      voids: t.voids || ju,
      characterReferences: t.characterReferences || ep,
      closeSelfClosing: t.closeSelfClosing || !1,
      closeEmptyElements: t.closeEmptyElements || !1
    },
    schema: t.space === "svg" ? Za : Ju,
    quote: s,
    alternative: r
  }.one(
    Array.isArray(n) ? { type: "root", children: n } : n,
    void 0,
    void 0
  );
}
function sp(n, e, t) {
  return Zh(n, e, t, this);
}
function rp(n) {
  const e = [], t = n && n.children || tp;
  let s = -1;
  for (; ++s < t.length; )
    e[s] = this.one(t[s], s, n);
  return e.join("");
}
const Ei = /\s+/g;
function nl(n, e) {
  var s;
  if (!e) return n;
  n.properties || (n.properties = {}), (s = n.properties).class || (s.class = []), typeof n.properties.class == "string" && (n.properties.class = n.properties.class.split(Ei)), Array.isArray(n.properties.class) || (n.properties.class = []);
  const t = Array.isArray(e) ? e : e.split(Ei);
  for (const r of t) r && !n.properties.class.includes(r) && n.properties.class.push(r);
  return n;
}
function op(n) {
  const e = Cs(n, !0).map(([r]) => r);
  function t(r) {
    if (r === n.length) return {
      line: e.length - 1,
      character: e.at(-1).length
    };
    let o = r, i = 0;
    for (const a of e) {
      if (o < a.length) break;
      o -= a.length, i++;
    }
    return {
      line: i,
      character: o
    };
  }
  function s(r, o) {
    let i = 0;
    for (let a = 0; a < r; a++) i += e[a].length;
    return i += o, i;
  }
  return {
    lines: e,
    indexToPos: t,
    posToIndex: s
  };
}
const ip = ["color", "background-color"];
function ap(n, e) {
  let t = 0;
  const s = [];
  for (const r of e)
    r > t && s.push({
      ...n,
      content: n.content.slice(t, r),
      offset: n.offset + t
    }), t = r;
  return t < n.content.length && s.push({
    ...n,
    content: n.content.slice(t),
    offset: n.offset + t
  }), s;
}
function lp(n, e) {
  const t = [...e instanceof Set ? e : new Set(e)].sort((s, r) => s - r);
  return t.length ? n.map((s) => s.flatMap((r) => {
    const o = t.filter((i) => r.offset < i && i < r.offset + r.content.length).map((i) => i - r.offset).sort((i, a) => i - a);
    return o.length ? ap(r, o) : r;
  })) : n;
}
function cp(n, e, t, s, r = "css-vars") {
  const o = {
    content: n.content,
    explanation: n.explanation,
    offset: n.offset
  }, i = e.map((h) => ts(n.variants[h])), a = new Set(i.flatMap((h) => Object.keys(h))), l = {}, c = (h, u) => {
    const p = u === "color" ? "" : u === "background-color" ? "-bg" : `-${u}`;
    return t + e[h] + (u === "color" ? "" : p);
  };
  return i.forEach((h, u) => {
    for (const p of a) {
      const d = h[p] || "inherit";
      if (u === 0 && s && ip.includes(p)) if (s === "light-dark()" && i.length > 1) {
        const f = e.findIndex((v) => v === "light"), b = e.findIndex((v) => v === "dark");
        if (f === -1 || b === -1) throw new O('When using `defaultColor: "light-dark()"`, you must provide both `light` and `dark` themes');
        l[p] = `light-dark(${i[f][p] || "inherit"}, ${i[b][p] || "inherit"})`, r === "css-vars" && (l[c(u, p)] = d);
      } else l[p] = d;
      else r === "css-vars" && (l[c(u, p)] = d);
    }
  }), o.htmlStyle = l, o;
}
function ts(n) {
  const e = {};
  if (n.color && (e.color = n.color), n.bgColor && (e["background-color"] = n.bgColor), n.fontStyle) {
    n.fontStyle & W.Italic && (e["font-style"] = "italic"), n.fontStyle & W.Bold && (e["font-weight"] = "bold");
    const t = [];
    n.fontStyle & W.Underline && t.push("underline"), n.fontStyle & W.Strikethrough && t.push("line-through"), t.length && (e["text-decoration"] = t.join(" "));
  }
  return e;
}
function _r(n) {
  return typeof n == "string" ? n : Object.entries(n).map(([e, t]) => `${e}:${t}`).join(";");
}
function up() {
  const n = /* @__PURE__ */ new WeakMap();
  function e(t) {
    if (!n.has(t.meta)) {
      let r = function(i) {
        if (typeof i == "number") {
          if (i < 0 || i > t.source.length) throw new O(`Invalid decoration offset: ${i}. Code length: ${t.source.length}`);
          return {
            ...s.indexToPos(i),
            offset: i
          };
        } else {
          const a = s.lines[i.line];
          if (a === void 0) throw new O(`Invalid decoration position ${JSON.stringify(i)}. Lines length: ${s.lines.length}`);
          let l = i.character;
          if (l < 0 && (l = a.length + l), l < 0 || l > a.length) throw new O(`Invalid decoration position ${JSON.stringify(i)}. Line ${i.line} length: ${a.length}`);
          return {
            ...i,
            character: l,
            offset: s.posToIndex(i.line, l)
          };
        }
      };
      const s = op(t.source), o = (t.options.decorations || []).map((i) => ({
        ...i,
        start: r(i.start),
        end: r(i.end)
      }));
      hp(o), n.set(t.meta, {
        decorations: o,
        converter: s,
        source: t.source
      });
    }
    return n.get(t.meta);
  }
  return {
    name: "shiki:decorations",
    tokens(t) {
      var s;
      if ((s = this.options.decorations) != null && s.length)
        return lp(t, e(this).decorations.flatMap((r) => [r.start.offset, r.end.offset]));
    },
    code(t) {
      var h;
      if (!((h = this.options.decorations) != null && h.length)) return;
      const s = e(this), r = [...t.children].filter((u) => u.type === "element" && u.tagName === "span");
      if (r.length !== s.converter.lines.length) throw new O(`Number of lines in code element (${r.length}) does not match the number of lines in the source (${s.converter.lines.length}). Failed to apply decorations.`);
      function o(u, p, d, f) {
        const b = r[u];
        let v = "", k = -1, _ = -1;
        if (p === 0 && (k = 0), d === 0 && (_ = 0), d === Number.POSITIVE_INFINITY && (_ = b.children.length), k === -1 || _ === -1) for (let $ = 0; $ < b.children.length; $++)
          v += sl(b.children[$]), k === -1 && v.length === p && (k = $ + 1), _ === -1 && v.length === d && (_ = $ + 1);
        if (k === -1) throw new O(`Failed to find start index for decoration ${JSON.stringify(f.start)}`);
        if (_ === -1) throw new O(`Failed to find end index for decoration ${JSON.stringify(f.end)}`);
        const x = b.children.slice(k, _);
        if (!f.alwaysWrap && x.length === b.children.length) a(b, f, "line");
        else if (!f.alwaysWrap && x.length === 1 && x[0].type === "element") a(x[0], f, "token");
        else {
          const $ = {
            type: "element",
            tagName: "span",
            properties: {},
            children: x
          };
          a($, f, "wrapper"), b.children.splice(k, x.length, $);
        }
      }
      function i(u, p) {
        r[u] = a(r[u], p, "line");
      }
      function a(u, p, d) {
        var v;
        const f = p.properties || {}, b = p.transform || ((k) => k);
        return u.tagName = p.tagName || "span", u.properties = {
          ...u.properties,
          ...f,
          class: u.properties.class
        }, (v = p.properties) != null && v.class && nl(u, p.properties.class), u = b(u, d) || u, u;
      }
      const l = [], c = s.decorations.sort((u, p) => p.start.offset - u.start.offset || u.end.offset - p.end.offset);
      for (const u of c) {
        const { start: p, end: d } = u;
        if (p.line === d.line) o(p.line, p.character, d.character, u);
        else if (p.line < d.line) {
          o(p.line, p.character, Number.POSITIVE_INFINITY, u);
          for (let f = p.line + 1; f < d.line; f++) l.unshift(() => i(f, u));
          o(d.line, 0, d.character, u);
        }
      }
      l.forEach((u) => u());
    }
  };
}
function hp(n) {
  for (let e = 0; e < n.length; e++) {
    const t = n[e];
    if (t.start.offset > t.end.offset) throw new O(`Invalid decoration range: ${JSON.stringify(t.start)} - ${JSON.stringify(t.end)}`);
    for (let s = e + 1; s < n.length; s++) {
      const r = n[s], o = t.start.offset <= r.start.offset && r.start.offset < t.end.offset, i = t.start.offset < r.end.offset && r.end.offset <= t.end.offset, a = r.start.offset <= t.start.offset && t.start.offset < r.end.offset, l = r.start.offset < t.end.offset && t.end.offset <= r.end.offset;
      if (o || i || a || l) {
        if (o && i || a && l || a && t.start.offset === t.end.offset || i && r.start.offset === r.end.offset) continue;
        throw new O(`Decorations ${JSON.stringify(t.start)} and ${JSON.stringify(r.start)} intersect.`);
      }
    }
  }
}
function sl(n) {
  return n.type === "text" ? n.value : n.type === "element" ? n.children.map(sl).join("") : "";
}
const pp = [/* @__PURE__ */ up()];
function ns(n) {
  const e = dp(n.transformers || []);
  return [
    ...e.pre,
    ...e.normal,
    ...e.post,
    ...pp
  ];
}
function dp(n) {
  const e = [], t = [], s = [];
  for (const r of n) switch (r.enforce) {
    case "pre":
      e.push(r);
      break;
    case "post":
      t.push(r);
      break;
    default:
      s.push(r);
  }
  return {
    pre: e,
    post: t,
    normal: s
  };
}
var et = [
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "brightBlack",
  "brightRed",
  "brightGreen",
  "brightYellow",
  "brightBlue",
  "brightMagenta",
  "brightCyan",
  "brightWhite"
], Vs = {
  1: "bold",
  2: "dim",
  3: "italic",
  4: "underline",
  7: "reverse",
  8: "hidden",
  9: "strikethrough"
};
function fp(n, e) {
  const t = n.indexOf("\x1B", e);
  if (t !== -1 && n[t + 1] === "[") {
    const s = n.indexOf("m", t);
    if (s !== -1) return {
      sequence: n.substring(t + 2, s).split(";"),
      startPosition: t,
      position: s + 1
    };
  }
  return { position: n.length };
}
function Ri(n) {
  const e = n.shift();
  if (e === "2") {
    const t = n.splice(0, 3).map((s) => Number.parseInt(s));
    return t.length !== 3 || t.some((s) => Number.isNaN(s)) ? void 0 : {
      type: "rgb",
      rgb: t
    };
  } else if (e === "5") {
    const t = n.shift();
    if (t) return {
      type: "table",
      index: Number(t)
    };
  }
}
function gp(n) {
  const e = [];
  for (; n.length > 0; ) {
    const t = n.shift();
    if (!t) continue;
    const s = Number.parseInt(t);
    if (!Number.isNaN(s))
      if (s === 0) e.push({ type: "resetAll" });
      else if (s <= 9)
        Vs[s] && e.push({
          type: "setDecoration",
          value: Vs[s]
        });
      else if (s <= 29) {
        const r = Vs[s - 20];
        r && (e.push({
          type: "resetDecoration",
          value: r
        }), r === "dim" && e.push({
          type: "resetDecoration",
          value: "bold"
        }));
      } else if (s <= 37) e.push({
        type: "setForegroundColor",
        value: {
          type: "named",
          name: et[s - 30]
        }
      });
      else if (s === 38) {
        const r = Ri(n);
        r && e.push({
          type: "setForegroundColor",
          value: r
        });
      } else if (s === 39) e.push({ type: "resetForegroundColor" });
      else if (s <= 47) e.push({
        type: "setBackgroundColor",
        value: {
          type: "named",
          name: et[s - 40]
        }
      });
      else if (s === 48) {
        const r = Ri(n);
        r && e.push({
          type: "setBackgroundColor",
          value: r
        });
      } else s === 49 ? e.push({ type: "resetBackgroundColor" }) : s === 53 ? e.push({
        type: "setDecoration",
        value: "overline"
      }) : s === 55 ? e.push({
        type: "resetDecoration",
        value: "overline"
      }) : s >= 90 && s <= 97 ? e.push({
        type: "setForegroundColor",
        value: {
          type: "named",
          name: et[s - 90 + 8]
        }
      }) : s >= 100 && s <= 107 && e.push({
        type: "setBackgroundColor",
        value: {
          type: "named",
          name: et[s - 100 + 8]
        }
      });
  }
  return e;
}
function mp() {
  let n = null, e = null, t = /* @__PURE__ */ new Set();
  return { parse(s) {
    const r = [];
    let o = 0;
    do {
      const i = fp(s, o), a = i.sequence ? s.substring(o, i.startPosition) : s.substring(o);
      if (a.length > 0 && r.push({
        value: a,
        foreground: n,
        background: e,
        decorations: new Set(t)
      }), i.sequence) {
        const l = gp(i.sequence);
        for (const c of l) c.type === "resetAll" ? (n = null, e = null, t.clear()) : c.type === "resetForegroundColor" ? n = null : c.type === "resetBackgroundColor" ? e = null : c.type === "resetDecoration" && t.delete(c.value);
        for (const c of l) c.type === "setForegroundColor" ? n = c.value : c.type === "setBackgroundColor" ? e = c.value : c.type === "setDecoration" && t.add(c.value);
      }
      o = i.position;
    } while (o < s.length);
    return r;
  } };
}
var bp = {
  black: "#000000",
  red: "#bb0000",
  green: "#00bb00",
  yellow: "#bbbb00",
  blue: "#0000bb",
  magenta: "#ff00ff",
  cyan: "#00bbbb",
  white: "#eeeeee",
  brightBlack: "#555555",
  brightRed: "#ff5555",
  brightGreen: "#00ff00",
  brightYellow: "#ffff55",
  brightBlue: "#5555ff",
  brightMagenta: "#ff55ff",
  brightCyan: "#55ffff",
  brightWhite: "#ffffff"
};
function yp(n = bp) {
  function e(a) {
    return n[a];
  }
  function t(a) {
    return `#${a.map((l) => Math.max(0, Math.min(l, 255)).toString(16).padStart(2, "0")).join("")}`;
  }
  let s;
  function r() {
    if (s) return s;
    s = [];
    for (let c = 0; c < et.length; c++) s.push(e(et[c]));
    let a = [
      0,
      95,
      135,
      175,
      215,
      255
    ];
    for (let c = 0; c < 6; c++) for (let h = 0; h < 6; h++) for (let u = 0; u < 6; u++) s.push(t([
      a[c],
      a[h],
      a[u]
    ]));
    let l = 8;
    for (let c = 0; c < 24; c++, l += 10) s.push(t([
      l,
      l,
      l
    ]));
    return s;
  }
  function o(a) {
    return r()[a];
  }
  function i(a) {
    switch (a.type) {
      case "named":
        return e(a.name);
      case "rgb":
        return t(a.rgb);
      case "table":
        return o(a.index);
    }
  }
  return { value: i };
}
const vp = /#([0-9a-f]{3,8})/i, _p = /var\((--[\w-]+-ansi-[\w-]+)\)/, wp = {
  black: "#000000",
  red: "#cd3131",
  green: "#0DBC79",
  yellow: "#E5E510",
  blue: "#2472C8",
  magenta: "#BC3FBC",
  cyan: "#11A8CD",
  white: "#E5E5E5",
  brightBlack: "#666666",
  brightRed: "#F14C4C",
  brightGreen: "#23D18B",
  brightYellow: "#F5F543",
  brightBlue: "#3B8EEA",
  brightMagenta: "#D670D6",
  brightCyan: "#29B8DB",
  brightWhite: "#FFFFFF"
};
function xp(n, e, t) {
  const s = es(n, t), r = Cs(e), o = yp(Object.fromEntries(et.map((a) => {
    var c;
    const l = `terminal.ansi${a[0].toUpperCase()}${a.substring(1)}`;
    return [a, ((c = n.colors) == null ? void 0 : c[l]) || wp[a]];
  }))), i = mp();
  return r.map((a) => i.parse(a[0]).map((l) => {
    let c, h;
    l.decorations.has("reverse") ? (c = l.background ? o.value(l.background) : n.bg, h = l.foreground ? o.value(l.foreground) : n.fg) : (c = l.foreground ? o.value(l.foreground) : n.fg, h = l.background ? o.value(l.background) : void 0), c = Fe(c, s), h = Fe(h, s), l.decorations.has("dim") && (c = kp(c));
    let u = W.None;
    return l.decorations.has("bold") && (u |= W.Bold), l.decorations.has("italic") && (u |= W.Italic), l.decorations.has("underline") && (u |= W.Underline), l.decorations.has("strikethrough") && (u |= W.Strikethrough), {
      content: l.value,
      offset: a[1],
      color: c,
      bgColor: h,
      fontStyle: u
    };
  }));
}
function kp(n) {
  const e = n.match(vp);
  if (e) {
    const s = e[1];
    if (s.length === 8) {
      const r = Math.round(Number.parseInt(s.slice(6, 8), 16) / 2).toString(16).padStart(2, "0");
      return `#${s.slice(0, 6)}${r}`;
    } else {
      if (s.length === 6) return `#${s}80`;
      if (s.length === 4) {
        const r = s[0], o = s[1], i = s[2], a = s[3];
        return `#${r}${r}${o}${o}${i}${i}${Math.round(Number.parseInt(`${a}${a}`, 16) / 2).toString(16).padStart(2, "0")}`;
      } else if (s.length === 3) {
        const r = s[0], o = s[1], i = s[2];
        return `#${r}${r}${o}${o}${i}${i}80`;
      }
    }
  }
  const t = n.match(_p);
  return t ? `var(${t[1]}-dim)` : n;
}
function wr(n, e, t = {}) {
  const s = n.resolveLangAlias(t.lang || "text"), { theme: r = n.getLoadedThemes()[0] } = t;
  if (!xs(s) && !ks(r) && s === "ansi") {
    const { theme: o } = n.setTheme(r);
    return xp(o, e, t);
  }
  return Ga(n, e, t);
}
function ss(n, e, t) {
  let s, r, o, i, a, l;
  if ("themes" in t) {
    const { defaultColor: c = "light", cssVariablePrefix: h = "--shiki-", colorsRendering: u = "css-vars" } = t, p = Object.entries(t.themes).filter((k) => k[1]).map((k) => ({
      color: k[0],
      theme: k[1]
    })).sort((k, _) => k.color === c ? -1 : _.color === c ? 1 : 0);
    if (p.length === 0) throw new O("`themes` option must not be empty");
    const d = Ua(n, e, t, wr);
    if (l = hn(d), c && c !== "light-dark()" && !p.some((k) => k.color === c)) throw new O(`\`themes\` option must contain the defaultColor key \`${c}\``);
    const f = p.map((k) => n.getTheme(k.theme)), b = p.map((k) => k.color);
    o = d.map((k) => k.map((_) => cp(_, b, h, c, u))), l && $s(o, l);
    const v = p.map((k) => es(k.theme, t));
    r = Ti(p, f, v, h, c, "fg", u), s = Ti(p, f, v, h, c, "bg", u), i = `shiki-themes ${f.map((k) => k.name).join(" ")}`, a = c ? void 0 : [r, s].join(";");
  } else if ("theme" in t) {
    const c = es(t.theme, t);
    o = wr(n, e, t);
    const h = n.getTheme(t.theme);
    s = Fe(h.bg, c), r = Fe(h.fg, c), i = h.name, l = hn(o);
  } else throw new O("Invalid options, either `theme` or `themes` must be provided");
  return {
    tokens: o,
    fg: r,
    bg: s,
    themeName: i,
    rootStyle: a,
    grammarState: l
  };
}
function Ti(n, e, t, s, r, o, i) {
  return n.map((a, l) => {
    const c = Fe(e[l][o], t[l]) || "inherit", h = `${s + a.color}${o === "bg" ? "-bg" : ""}:${c}`;
    if (l === 0 && r) {
      if (r === "light-dark()" && n.length > 1) {
        const u = n.findIndex((d) => d.color === "light"), p = n.findIndex((d) => d.color === "dark");
        if (u === -1 || p === -1) throw new O('When using `defaultColor: "light-dark()"`, you must provide both `light` and `dark` themes');
        return `light-dark(${Fe(e[u][o], t[u]) || "inherit"}, ${Fe(e[p][o], t[p]) || "inherit"});${h}`;
      }
      return c;
    }
    return i === "css-vars" ? h : null;
  }).filter((a) => !!a).join(";");
}
const rl = /^\s+$/, Cp = /^(\s*)(.*?)(\s*)$/;
function rs(n, e, t, s = {
  meta: {},
  options: t,
  codeToHast: (r, o) => rs(n, r, o),
  codeToTokens: (r, o) => ss(n, r, o)
}) {
  var f, b;
  let r = e;
  for (const v of ns(t)) r = ((f = v.preprocess) == null ? void 0 : f.call(s, r, t)) || r;
  let { tokens: o, fg: i, bg: a, themeName: l, rootStyle: c, grammarState: h } = ss(n, r, t);
  const { mergeWhitespaces: u = !0, mergeSameStyleTokens: p = !1 } = t;
  u === !0 ? o = Sp(o) : u === "never" && (o = Ap(o)), p && (o = Ep(o));
  const d = {
    ...s,
    get source() {
      return r;
    }
  };
  for (const v of ns(t)) o = ((b = v.tokens) == null ? void 0 : b.call(d, o)) || o;
  return $p(o, {
    ...t,
    fg: i,
    bg: a,
    themeName: l,
    rootStyle: t.rootStyle === !1 ? !1 : t.rootStyle ?? c
  }, d, h);
}
function $p(n, e, t, s = hn(n)) {
  var b, v, k, _;
  const r = ns(e), o = [], i = {
    type: "root",
    children: []
  }, { structure: a = "classic", tabindex: l = "0" } = e, c = { class: `shiki ${e.themeName || ""}` };
  e.rootStyle !== !1 && (e.rootStyle != null ? c.style = e.rootStyle : c.style = `background-color:${e.bg};color:${e.fg}`), l !== !1 && l != null && (c.tabindex = l.toString());
  for (const [x, $] of Object.entries(e.meta || {})) x.startsWith("_") || (c[x] = $);
  let h = {
    type: "element",
    tagName: "pre",
    properties: c,
    children: [],
    data: e.data
  }, u = {
    type: "element",
    tagName: "code",
    properties: {},
    children: o
  };
  const p = [], d = {
    ...t,
    structure: a,
    addClassToHast: nl,
    get source() {
      return t.source;
    },
    get tokens() {
      return n;
    },
    get options() {
      return e;
    },
    get root() {
      return i;
    },
    get pre() {
      return h;
    },
    get code() {
      return u;
    },
    get lines() {
      return p;
    }
  };
  if (n.forEach((x, $) => {
    var M, ee;
    $ && (a === "inline" ? i.children.push({
      type: "element",
      tagName: "br",
      properties: {},
      children: []
    }) : a === "classic" && o.push({
      type: "text",
      value: `
`
    }));
    let A = {
      type: "element",
      tagName: "span",
      properties: { class: "line" },
      children: []
    }, T = 0;
    for (const j of x) {
      let ge = {
        type: "element",
        tagName: "span",
        properties: { ...j.htmlAttrs },
        children: [{
          type: "text",
          value: j.content
        }]
      };
      const mt = _r(j.htmlStyle || ts(j));
      mt && (ge.properties.style = mt);
      for (const Z of r) ge = ((M = Z == null ? void 0 : Z.span) == null ? void 0 : M.call(d, ge, $ + 1, T, A, j)) || ge;
      a === "inline" ? i.children.push(ge) : a === "classic" && A.children.push(ge), T += j.content.length;
    }
    if (a === "classic") {
      for (const j of r) A = ((ee = j == null ? void 0 : j.line) == null ? void 0 : ee.call(d, A, $ + 1)) || A;
      p.push(A), o.push(A);
    } else a === "inline" && p.push(A);
  }), a === "classic") {
    for (const x of r) u = ((b = x == null ? void 0 : x.code) == null ? void 0 : b.call(d, u)) || u;
    h.children.push(u);
    for (const x of r) h = ((v = x == null ? void 0 : x.pre) == null ? void 0 : v.call(d, h)) || h;
    i.children.push(h);
  } else if (a === "inline") {
    const x = [];
    let $ = {
      type: "element",
      tagName: "span",
      properties: { class: "line" },
      children: []
    };
    for (const T of i.children) T.type === "element" && T.tagName === "br" ? (x.push($), $ = {
      type: "element",
      tagName: "span",
      properties: { class: "line" },
      children: []
    }) : (T.type === "element" || T.type === "text") && $.children.push(T);
    x.push($);
    let A = {
      type: "element",
      tagName: "code",
      properties: {},
      children: x
    };
    for (const T of r) A = ((k = T == null ? void 0 : T.code) == null ? void 0 : k.call(d, A)) || A;
    i.children = [];
    for (let T = 0; T < A.children.length; T++) {
      T > 0 && i.children.push({
        type: "element",
        tagName: "br",
        properties: {},
        children: []
      });
      const M = A.children[T];
      M.type === "element" && i.children.push(...M.children);
    }
  }
  let f = i;
  for (const x of r) f = ((_ = x == null ? void 0 : x.root) == null ? void 0 : _.call(d, f)) || f;
  return s && $s(f, s), f;
}
function Sp(n) {
  return n.map((e) => {
    const t = [];
    let s = "", r;
    return e.forEach((o, i) => {
      const a = !(o.fontStyle && (o.fontStyle & W.Underline || o.fontStyle & W.Strikethrough));
      a && rl.test(o.content) && e[i + 1] ? (r === void 0 && (r = o.offset), s += o.content) : s ? (a ? t.push({
        ...o,
        offset: r,
        content: s + o.content
      }) : t.push({
        content: s,
        offset: r
      }, o), r = void 0, s = "") : t.push(o);
    }), t;
  });
}
function Ap(n) {
  return n.map((e) => e.flatMap((t) => {
    if (rl.test(t.content)) return t;
    const s = t.content.match(Cp);
    if (!s) return t;
    const [, r, o, i] = s;
    if (!r && !i) return t;
    const a = [{
      ...t,
      offset: t.offset + r.length,
      content: o
    }];
    return r && a.unshift({
      content: r,
      offset: t.offset
    }), i && a.push({
      content: i,
      offset: t.offset + r.length + o.length
    }), a;
  }));
}
function Ep(n) {
  return n.map((e) => {
    const t = [];
    for (const s of e) {
      if (t.length === 0) {
        t.push({ ...s });
        continue;
      }
      const r = t.at(-1), o = _r(r.htmlStyle || ts(r)), i = _r(s.htmlStyle || ts(s)), a = r.fontStyle && (r.fontStyle & W.Underline || r.fontStyle & W.Strikethrough), l = s.fontStyle && (s.fontStyle & W.Underline || s.fontStyle & W.Strikethrough);
      !a && !l && o === i ? r.content += s.content : t.push({ ...s });
    }
    return t;
  });
}
const Rp = np;
function Tp(n, e, t) {
  var o;
  const s = {
    meta: {},
    options: t,
    codeToHast: (i, a) => rs(n, i, a),
    codeToTokens: (i, a) => ss(n, i, a)
  };
  let r = Rp(rs(n, e, t, s));
  for (const i of ns(t)) r = ((o = i.postprocess) == null ? void 0 : o.call(s, r, t)) || r;
  return r;
}
async function Ip(n) {
  const e = await Iu(n);
  return {
    getLastGrammarState: (...t) => Ou(e, ...t),
    codeToTokensBase: (t, s) => wr(e, t, s),
    codeToTokensWithThemes: (t, s) => Ua(e, t, s),
    codeToTokens: (t, s) => ss(e, t, s),
    codeToHast: (t, s) => rs(e, t, s),
    codeToHtml: (t, s) => Tp(e, t, s),
    getBundledLanguages: () => ({}),
    getBundledThemes: () => ({}),
    ...e,
    getInternalContext: () => e
  };
}
const Ii = 4294967295;
var Pp = class {
  constructor(n, e = {}) {
    g(this, "patterns");
    g(this, "options");
    g(this, "regexps");
    this.patterns = n, this.options = e;
    const { forgiving: t = !1, cache: s, regexConstructor: r } = e;
    if (!r) throw new Error("Option `regexConstructor` is not provided");
    this.regexps = n.map((o) => {
      if (typeof o != "string") return o;
      const i = s == null ? void 0 : s.get(o);
      if (i) {
        if (i instanceof RegExp) return i;
        if (t) return null;
        throw i;
      }
      try {
        const a = r(o);
        return s == null || s.set(o, a), a;
      } catch (a) {
        if (s == null || s.set(o, a), t) return null;
        throw a;
      }
    });
  }
  findNextMatchSync(n, e, t) {
    const s = typeof n == "string" ? n : n.content, r = [];
    function o(i, a, l = 0) {
      return {
        index: i,
        captureIndices: a.indices.map((c) => c == null ? {
          start: Ii,
          end: Ii,
          length: 0
        } : {
          start: c[0] + l,
          end: c[1] + l,
          length: c[1] - c[0]
        })
      };
    }
    for (let i = 0; i < this.regexps.length; i++) {
      const a = this.regexps[i];
      if (a)
        try {
          a.lastIndex = e;
          const l = a.exec(s);
          if (!l) continue;
          if (l.index === e) return o(i, l, 0);
          r.push([
            i,
            l,
            0
          ]);
        } catch (l) {
          if (this.options.forgiving) continue;
          throw l;
        }
    }
    if (r.length) {
      const i = Math.min(...r.map((a) => a[1].index));
      for (const [a, l, c] of r) if (l.index === i) return o(a, l, c);
    }
    return null;
  }
};
function Bt(n) {
  if ([...n].length !== 1) throw new Error(`Expected "${n}" to be a single code point`);
  return n.codePointAt(0);
}
function Lp(n, e, t) {
  return n.has(e) || n.set(e, t), n.get(e);
}
const Qr = /* @__PURE__ */ new Set(["alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower", "print", "punct", "space", "upper", "word", "xdigit"]), U = String.raw;
function Dt(n, e) {
  if (n == null) throw new Error(e ?? "Value expected");
  return n;
}
const ol = U`\[\^?`, il = `c.? | C(?:-.?)?|${U`[pP]\{(?:\^?[-\x20_]*[A-Za-z][-\x20\w]*\})?`}|${U`x[89A-Fa-f]\p{AHex}(?:\\x[89A-Fa-f]\p{AHex})*`}|${U`u(?:\p{AHex}{4})? | x\{[^\}]*\}? | x\p{AHex}{0,2}`}|${U`o\{[^\}]*\}?`}|${U`\d{1,3}`}`, Vr = /[?*+][?+]?|\{(?:\d+(?:,\d*)?|,\d+)\}\??/, Mn = new RegExp(U`
  \\ (?:
    ${il}
    | [gk]<[^>]*>?
    | [gk]'[^']*'?
    | .
  )
  | \( (?:
    \? (?:
      [:=!>({]
      | <[=!]
      | <[^>]*>
      | '[^']*'
      | ~\|?
      | #(?:[^)\\]|\\.?)*
      | [^:)]*[:)]
    )?
    | \*[^\)]*\)?
  )?
  | (?:${Vr.source})+
  | ${ol}
  | .
`.replace(/\s+/g, ""), "gsu"), Ks = new RegExp(U`
  \\ (?:
    ${il}
    | .
  )
  | \[:(?:\^?\p{Alpha}+|\^):\]
  | ${ol}
  | &&
  | .
`.replace(/\s+/g, ""), "gsu");
function Np(n, e = {}) {
  const t = { flags: "", ...e, rules: { captureGroup: !1, singleline: !1, ...e.rules } };
  if (typeof n != "string") throw new Error("String expected as pattern");
  const s = Yp(t.flags), r = [s.extended], o = { captureGroup: t.rules.captureGroup, getCurrentModX() {
    return r.at(-1);
  }, numOpenGroups: 0, popModX() {
    r.pop();
  }, pushModX(u) {
    r.push(u);
  }, replaceCurrentModX(u) {
    r[r.length - 1] = u;
  }, singleline: t.rules.singleline };
  let i = [], a;
  for (Mn.lastIndex = 0; a = Mn.exec(n); ) {
    const u = Mp(o, n, a[0], Mn.lastIndex);
    u.tokens ? i.push(...u.tokens) : u.token && i.push(u.token), u.lastIndex !== void 0 && (Mn.lastIndex = u.lastIndex);
  }
  const l = [];
  let c = 0;
  i.filter((u) => u.type === "GroupOpen").forEach((u) => {
    u.kind === "capturing" ? u.number = ++c : u.raw === "(" && l.push(u);
  }), c || l.forEach((u, p) => {
    u.kind = "capturing", u.number = p + 1;
  });
  const h = c || l.length;
  return { tokens: i.map((u) => u.type === "EscapedNumber" ? td(u, h) : u).flat(), flags: s };
}
function Mp(n, e, t, s) {
  const [r, o] = t;
  if (t === "[" || t === "[^") {
    const i = Op(e, t, s);
    return { tokens: i.tokens, lastIndex: i.lastIndex };
  }
  if (r === "\\") {
    if ("AbBGyYzZ".includes(o)) return { token: Pi(t, t) };
    if (/^\\g[<']/.test(t)) {
      if (!/^\\g(?:<[^>]+>|'[^']+')$/.test(t)) throw new Error(`Invalid group name "${t}"`);
      return { token: Wp(t) };
    }
    if (/^\\k[<']/.test(t)) {
      if (!/^\\k(?:<[^>]+>|'[^']+')$/.test(t)) throw new Error(`Invalid group name "${t}"`);
      return { token: ll(t) };
    }
    if (o === "K") return { token: cl("keep", t) };
    if (o === "N" || o === "R") return { token: tt("newline", t, { negate: o === "N" }) };
    if (o === "O") return { token: tt("any", t) };
    if (o === "X") return { token: tt("text_segment", t) };
    const i = al(t, { inCharClass: !1 });
    return Array.isArray(i) ? { tokens: i } : { token: i };
  }
  if (r === "(") {
    if (o === "*") return { token: Zp(t) };
    if (t === "(?{") throw new Error(`Unsupported callout "${t}"`);
    if (t.startsWith("(?#")) {
      if (e[s] !== ")") throw new Error('Unclosed comment group "(?#"');
      return { lastIndex: s + 1 };
    }
    if (/^\(\?[-imx]+[:)]$/.test(t)) return { token: Kp(t, n) };
    if (n.pushModX(n.getCurrentModX()), n.numOpenGroups++, t === "(" && !n.captureGroup || t === "(?:") return { token: xt("group", t) };
    if (t === "(?>") return { token: xt("atomic", t) };
    if (t === "(?=" || t === "(?!" || t === "(?<=" || t === "(?<!") return { token: xt(t[2] === "<" ? "lookbehind" : "lookahead", t, { negate: t.endsWith("!") }) };
    if (t === "(" && n.captureGroup || t.startsWith("(?<") && t.endsWith(">") || t.startsWith("(?'") && t.endsWith("'")) return { token: xt("capturing", t, { ...t !== "(" && { name: t.slice(3, -1) } }) };
    if (t.startsWith("(?~")) {
      if (t === "(?~|") throw new Error(`Unsupported absence function kind "${t}"`);
      return { token: xt("absence_repeater", t) };
    }
    throw t === "(?(" ? new Error(`Unsupported conditional "${t}"`) : new Error(`Invalid or unsupported group option "${t}"`);
  }
  if (t === ")") {
    if (n.popModX(), n.numOpenGroups--, n.numOpenGroups < 0) throw new Error('Unmatched ")"');
    return { token: jp(t) };
  }
  if (n.getCurrentModX()) {
    if (t === "#") {
      const i = e.indexOf(`
`, s);
      return { lastIndex: i === -1 ? e.length : i };
    }
    if (/^\s$/.test(t)) {
      const i = /\s+/y;
      return i.lastIndex = s, { lastIndex: i.exec(e) ? i.lastIndex : s };
    }
  }
  if (t === ".") return { token: tt("dot", t) };
  if (t === "^" || t === "$") {
    const i = n.singleline ? { "^": U`\A`, $: U`\Z` }[t] : t;
    return { token: Pi(i, t) };
  }
  return t === "|" ? { token: Bp(t) } : Vr.test(t) ? { tokens: nd(t) } : { token: Ae(Bt(t), t) };
}
function Op(n, e, t) {
  const s = [Li(e[1] === "^", e)];
  let r = 1, o;
  for (Ks.lastIndex = t; o = Ks.exec(n); ) {
    const i = o[0];
    if (i[0] === "[" && i[1] !== ":") r++, s.push(Li(i[1] === "^", i));
    else if (i === "]") {
      if (s.at(-1).type === "CharacterClassOpen") s.push(Ae(93, i));
      else if (r--, s.push(Dp(i)), !r) break;
    } else {
      const a = zp(i);
      Array.isArray(a) ? s.push(...a) : s.push(a);
    }
  }
  return { tokens: s, lastIndex: Ks.lastIndex || n.length };
}
function zp(n) {
  if (n[0] === "\\") return al(n, { inCharClass: !0 });
  if (n[0] === "[") {
    const e = /\[:(?<negate>\^?)(?<name>[a-z]+):\]/.exec(n);
    if (!e || !Qr.has(e.groups.name)) throw new Error(`Invalid POSIX class "${n}"`);
    return tt("posix", n, { value: e.groups.name, negate: !!e.groups.negate });
  }
  return n === "-" ? Fp(n) : n === "&&" ? Gp(n) : Ae(Bt(n), n);
}
function al(n, { inCharClass: e }) {
  const t = n[1];
  if (t === "c" || t === "C") return Vp(n);
  if ("dDhHsSwW".includes(t)) return Xp(n);
  if (n.startsWith(U`\o{`)) throw new Error(`Incomplete, invalid, or unsupported octal code point "${n}"`);
  if (/^\\[pP]\{/.test(n)) {
    if (n.length === 3) throw new Error(`Incomplete or invalid Unicode property "${n}"`);
    return Jp(n);
  }
  if (new RegExp("^\\\\x[89A-Fa-f]\\p{AHex}", "u").test(n)) try {
    const s = n.split(/\\x/).slice(1).map((i) => parseInt(i, 16)), r = new TextDecoder("utf-8", { ignoreBOM: !0, fatal: !0 }).decode(new Uint8Array(s)), o = new TextEncoder();
    return [...r].map((i) => {
      const a = [...o.encode(i)].map((l) => `\\x${l.toString(16)}`).join("");
      return Ae(Bt(i), a);
    });
  } catch {
    throw new Error(`Multibyte code "${n}" incomplete or invalid in Oniguruma`);
  }
  if (t === "u" || t === "x") return Ae(ed(n), n);
  if (Ni.has(t)) return Ae(Ni.get(t), n);
  if (/\d/.test(t)) return Up(e, n);
  if (n === "\\") throw new Error(U`Incomplete escape "\"`);
  if (t === "M") throw new Error(`Unsupported meta "${n}"`);
  if ([...n].length === 2) return Ae(n.codePointAt(1), n);
  throw new Error(`Unexpected escape "${n}"`);
}
function Bp(n) {
  return { type: "Alternator", raw: n };
}
function Pi(n, e) {
  return { type: "Assertion", kind: n, raw: e };
}
function ll(n) {
  return { type: "Backreference", raw: n };
}
function Ae(n, e) {
  return { type: "Character", value: n, raw: e };
}
function Dp(n) {
  return { type: "CharacterClassClose", raw: n };
}
function Fp(n) {
  return { type: "CharacterClassHyphen", raw: n };
}
function Gp(n) {
  return { type: "CharacterClassIntersector", raw: n };
}
function Li(n, e) {
  return { type: "CharacterClassOpen", negate: n, raw: e };
}
function tt(n, e, t = {}) {
  return { type: "CharacterSet", kind: n, ...t, raw: e };
}
function cl(n, e, t = {}) {
  return n === "keep" ? { type: "Directive", kind: n, raw: e } : { type: "Directive", kind: n, flags: Dt(t.flags), raw: e };
}
function Up(n, e) {
  return { type: "EscapedNumber", inCharClass: n, raw: e };
}
function jp(n) {
  return { type: "GroupClose", raw: n };
}
function xt(n, e, t = {}) {
  return { type: "GroupOpen", kind: n, ...t, raw: e };
}
function qp(n, e, t, s) {
  return { type: "NamedCallout", kind: n, tag: e, arguments: t, raw: s };
}
function Hp(n, e, t, s) {
  return { type: "Quantifier", kind: n, min: e, max: t, raw: s };
}
function Wp(n) {
  return { type: "Subroutine", raw: n };
}
const Qp = /* @__PURE__ */ new Set(["COUNT", "CMP", "ERROR", "FAIL", "MAX", "MISMATCH", "SKIP", "TOTAL_COUNT"]), Ni = /* @__PURE__ */ new Map([["a", 7], ["b", 8], ["e", 27], ["f", 12], ["n", 10], ["r", 13], ["t", 9], ["v", 11]]);
function Vp(n) {
  const e = n[1] === "c" ? n[2] : n[3];
  if (!e || !/[A-Za-z]/.test(e)) throw new Error(`Unsupported control character "${n}"`);
  return Ae(Bt(e.toUpperCase()) - 64, n);
}
function Kp(n, e) {
  let { on: t, off: s } = /^\(\?(?<on>[imx]*)(?:-(?<off>[-imx]*))?/.exec(n).groups;
  s ?? (s = "");
  const r = (e.getCurrentModX() || t.includes("x")) && !s.includes("x"), o = Oi(t), i = Oi(s), a = {};
  if (o && (a.enable = o), i && (a.disable = i), n.endsWith(")")) return e.replaceCurrentModX(r), cl("flags", n, { flags: a });
  if (n.endsWith(":")) return e.pushModX(r), e.numOpenGroups++, xt("group", n, { ...(o || i) && { flags: a } });
  throw new Error(`Unexpected flag modifier "${n}"`);
}
function Zp(n) {
  const e = /\(\*(?<name>[A-Za-z_]\w*)?(?:\[(?<tag>(?:[A-Za-z_]\w*)?)\])?(?:\{(?<args>[^}]*)\})?\)/.exec(n);
  if (!e) throw new Error(`Incomplete or invalid named callout "${n}"`);
  const { name: t, tag: s, args: r } = e.groups;
  if (!t) throw new Error(`Invalid named callout "${n}"`);
  if (s === "") throw new Error(`Named callout tag with empty value not allowed "${n}"`);
  const o = r ? r.split(",").filter((h) => h !== "").map((h) => /^[+-]?\d+$/.test(h) ? +h : h) : [], [i, a, l] = o, c = Qp.has(t) ? t.toLowerCase() : "custom";
  switch (c) {
    case "fail":
    case "mismatch":
    case "skip":
      if (o.length > 0) throw new Error(`Named callout arguments not allowed "${o}"`);
      break;
    case "error":
      if (o.length > 1) throw new Error(`Named callout allows only one argument "${o}"`);
      if (typeof i == "string") throw new Error(`Named callout argument must be a number "${i}"`);
      break;
    case "max":
      if (!o.length || o.length > 2) throw new Error(`Named callout must have one or two arguments "${o}"`);
      if (typeof i == "string" && !/^[A-Za-z_]\w*$/.test(i)) throw new Error(`Named callout argument one must be a tag or number "${i}"`);
      if (o.length === 2 && (typeof a == "number" || !/^[<>X]$/.test(a))) throw new Error(`Named callout optional argument two must be '<', '>', or 'X' "${a}"`);
      break;
    case "count":
    case "total_count":
      if (o.length > 1) throw new Error(`Named callout allows only one argument "${o}"`);
      if (o.length === 1 && (typeof i == "number" || !/^[<>X]$/.test(i))) throw new Error(`Named callout optional argument must be '<', '>', or 'X' "${i}"`);
      break;
    case "cmp":
      if (o.length !== 3) throw new Error(`Named callout must have three arguments "${o}"`);
      if (typeof i == "string" && !/^[A-Za-z_]\w*$/.test(i)) throw new Error(`Named callout argument one must be a tag or number "${i}"`);
      if (typeof a == "number" || !/^(?:[<>!=]=|[<>])$/.test(a)) throw new Error(`Named callout argument two must be '==', '!=', '>', '<', '>=', or '<=' "${a}"`);
      if (typeof l == "string" && !/^[A-Za-z_]\w*$/.test(l)) throw new Error(`Named callout argument three must be a tag or number "${l}"`);
      break;
    case "custom":
      throw new Error(`Undefined callout name "${t}"`);
    default:
      throw new Error(`Unexpected named callout kind "${c}"`);
  }
  return qp(c, s ?? null, (r == null ? void 0 : r.split(",")) ?? null, n);
}
function Mi(n) {
  let e = null, t, s;
  if (n[0] === "{") {
    const { minStr: r, maxStr: o } = /^\{(?<minStr>\d*)(?:,(?<maxStr>\d*))?/.exec(n).groups, i = 1e5;
    if (+r > i || o && +o > i) throw new Error("Quantifier value unsupported in Oniguruma");
    if (t = +r, s = o === void 0 ? +r : o === "" ? 1 / 0 : +o, t > s && (e = "possessive", [t, s] = [s, t]), n.endsWith("?")) {
      if (e === "possessive") throw new Error('Unsupported possessive interval quantifier chain with "?"');
      e = "lazy";
    } else e || (e = "greedy");
  } else t = n[0] === "+" ? 1 : 0, s = n[0] === "?" ? 1 : 1 / 0, e = n[1] === "+" ? "possessive" : n[1] === "?" ? "lazy" : "greedy";
  return Hp(e, t, s, n);
}
function Xp(n) {
  const e = n[1].toLowerCase();
  return tt({ d: "digit", h: "hex", s: "space", w: "word" }[e], n, { negate: n[1] !== e });
}
function Jp(n) {
  const { p: e, neg: t, value: s } = /^\\(?<p>[pP])\{(?<neg>\^?)(?<value>[^}]+)/.exec(n).groups;
  return tt("property", n, { value: s, negate: e === "P" && !t || e === "p" && !!t });
}
function Oi(n) {
  const e = {};
  return n.includes("i") && (e.ignoreCase = !0), n.includes("m") && (e.dotAll = !0), n.includes("x") && (e.extended = !0), Object.keys(e).length ? e : null;
}
function Yp(n) {
  const e = { ignoreCase: !1, dotAll: !1, extended: !1, digitIsAscii: !1, posixIsAscii: !1, spaceIsAscii: !1, wordIsAscii: !1, textSegmentMode: null };
  for (let t = 0; t < n.length; t++) {
    const s = n[t];
    if (!"imxDPSWy".includes(s)) throw new Error(`Invalid flag "${s}"`);
    if (s === "y") {
      if (!/^y{[gw]}/.test(n.slice(t))) throw new Error('Invalid or unspecified flag "y" mode');
      e.textSegmentMode = n[t + 2] === "g" ? "grapheme" : "word", t += 3;
      continue;
    }
    e[{ i: "ignoreCase", m: "dotAll", x: "extended", D: "digitIsAscii", P: "posixIsAscii", S: "spaceIsAscii", W: "wordIsAscii" }[s]] = !0;
  }
  return e;
}
function ed(n) {
  if (new RegExp("^(?:\\\\u(?!\\p{AHex}{4})|\\\\x(?!\\p{AHex}{1,2}|\\{\\p{AHex}{1,8}\\}))", "u").test(n)) throw new Error(`Incomplete or invalid escape "${n}"`);
  const e = n[2] === "{" ? new RegExp("^\\\\x\\{\\s*(?<hex>\\p{AHex}+)", "u").exec(n).groups.hex : n.slice(2);
  return parseInt(e, 16);
}
function td(n, e) {
  const { raw: t, inCharClass: s } = n, r = t.slice(1);
  if (!s && (r !== "0" && r.length === 1 || r[0] !== "0" && +r <= e)) return [ll(t)];
  const o = [], i = r.match(/^[0-7]+|\d/g);
  for (let a = 0; a < i.length; a++) {
    const l = i[a];
    let c;
    if (a === 0 && l !== "8" && l !== "9") {
      if (c = parseInt(l, 8), c > 127) throw new Error(U`Octal encoded byte above 177 unsupported "${t}"`);
    } else c = Bt(l);
    o.push(Ae(c, (a === 0 ? "\\" : "") + l));
  }
  return o;
}
function nd(n) {
  const e = [], t = new RegExp(Vr, "gy");
  let s;
  for (; s = t.exec(n); ) {
    const r = s[0];
    if (r[0] === "{") {
      const o = /^\{(?<min>\d+),(?<max>\d+)\}\??$/.exec(r);
      if (o) {
        const { min: i, max: a } = o.groups;
        if (+i > +a && r.endsWith("?")) {
          t.lastIndex--, e.push(Mi(r.slice(0, -1)));
          continue;
        }
      }
    }
    e.push(Mi(r));
  }
  return e;
}
function ul(n, e) {
  if (!Array.isArray(n.body)) throw new Error("Expected node with body array");
  if (n.body.length !== 1) return !1;
  const t = n.body[0];
  return !e || Object.keys(e).every((s) => e[s] === t[s]);
}
function sd(n) {
  return rd.has(n.type);
}
const rd = /* @__PURE__ */ new Set(["AbsenceFunction", "Backreference", "CapturingGroup", "Character", "CharacterClass", "CharacterSet", "Group", "Quantifier", "Subroutine"]);
function hl(n, e = {}) {
  const t = { flags: "", normalizeUnknownPropertyNames: !1, skipBackrefValidation: !1, skipLookbehindValidation: !1, skipPropertyNameValidation: !1, unicodePropertyMap: null, ...e, rules: { captureGroup: !1, singleline: !1, ...e.rules } }, s = Np(n, { flags: t.flags, rules: { captureGroup: t.rules.captureGroup, singleline: t.rules.singleline } }), r = (p, d) => {
    const f = s.tokens[o.nextIndex];
    switch (o.parent = p, o.nextIndex++, f.type) {
      case "Alternator":
        return lt();
      case "Assertion":
        return od(f);
      case "Backreference":
        return id(f, o);
      case "Character":
        return As(f.value, { useLastValid: !!d.isCheckingRangeEnd });
      case "CharacterClassHyphen":
        return ad(f, o, d);
      case "CharacterClassOpen":
        return ld(f, o, d);
      case "CharacterSet":
        return cd(f, o);
      case "Directive":
        return gd(f.kind, { flags: f.flags });
      case "GroupOpen":
        return ud(f, o, d);
      case "NamedCallout":
        return bd(f.kind, f.tag, f.arguments);
      case "Quantifier":
        return hd(f, o);
      case "Subroutine":
        return pd(f, o);
      default:
        throw new Error(`Unexpected token type "${f.type}"`);
    }
  }, o = { capturingGroups: [], hasNumberedRef: !1, namedGroupsByName: /* @__PURE__ */ new Map(), nextIndex: 0, normalizeUnknownPropertyNames: t.normalizeUnknownPropertyNames, parent: null, skipBackrefValidation: t.skipBackrefValidation, skipLookbehindValidation: t.skipLookbehindValidation, skipPropertyNameValidation: t.skipPropertyNameValidation, subroutines: [], tokens: s.tokens, unicodePropertyMap: t.unicodePropertyMap, walk: r }, i = vd(md(s.flags));
  let a = i.body[0];
  for (; o.nextIndex < s.tokens.length; ) {
    const p = r(a, {});
    p.type === "Alternative" ? (i.body.push(p), a = p) : a.body.push(p);
  }
  const { capturingGroups: l, hasNumberedRef: c, namedGroupsByName: h, subroutines: u } = o;
  if (c && h.size && !t.rules.captureGroup) throw new Error("Numbered backref/subroutine not allowed when using named capture");
  for (const { ref: p } of u) if (typeof p == "number") {
    if (p > l.length) throw new Error("Subroutine uses a group number that's not defined");
    p && (l[p - 1].isSubroutined = !0);
  } else if (h.has(p)) {
    if (h.get(p).length > 1) throw new Error(U`Subroutine uses a duplicate group name "\g<${p}>"`);
    h.get(p)[0].isSubroutined = !0;
  } else throw new Error(U`Subroutine uses a group name that's not defined "\g<${p}>"`);
  return i;
}
function od({ kind: n }) {
  return xr(Dt({ "^": "line_start", $: "line_end", "\\A": "string_start", "\\b": "word_boundary", "\\B": "word_boundary", "\\G": "search_start", "\\y": "text_segment_boundary", "\\Y": "text_segment_boundary", "\\z": "string_end", "\\Z": "string_end_newline" }[n], `Unexpected assertion kind "${n}"`), { negate: n === U`\B` || n === U`\Y` });
}
function id({ raw: n }, e) {
  const t = /^\\k[<']/.test(n), s = t ? n.slice(3, -1) : n.slice(1), r = (o, i = !1) => {
    const a = e.capturingGroups.length;
    let l = !1;
    if (o > a) if (e.skipBackrefValidation) l = !0;
    else throw new Error(`Not enough capturing groups defined to the left "${n}"`);
    return e.hasNumberedRef = !0, kr(i ? a + 1 - o : o, { orphan: l });
  };
  if (t) {
    const o = /^(?<sign>-?)0*(?<num>[1-9]\d*)$/.exec(s);
    if (o) return r(+o.groups.num, !!o.groups.sign);
    if (/[-+]/.test(s)) throw new Error(`Invalid backref name "${n}"`);
    if (!e.namedGroupsByName.has(s)) throw new Error(`Group name not defined to the left "${n}"`);
    return kr(s);
  }
  return r(+s);
}
function ad(n, e, t) {
  const { tokens: s, walk: r } = e, o = e.parent, i = o.body.at(-1), a = s[e.nextIndex];
  if (!t.isCheckingRangeEnd && i && i.type !== "CharacterClass" && i.type !== "CharacterClassRange" && a && a.type !== "CharacterClassOpen" && a.type !== "CharacterClassClose" && a.type !== "CharacterClassIntersector") {
    const l = r(o, { ...t, isCheckingRangeEnd: !0 });
    if (i.type === "Character" && l.type === "Character") return o.body.pop(), fd(i, l);
    throw new Error("Invalid character class range");
  }
  return As(Bt("-"));
}
function ld({ negate: n }, e, t) {
  const { tokens: s, walk: r } = e, o = [jn()], i = s[e.nextIndex];
  let a = Di(i);
  for (; a.type !== "CharacterClassClose"; ) {
    if (a.type === "CharacterClassIntersector") o.push(jn()), e.nextIndex++;
    else {
      const c = o.at(-1);
      c.body.push(r(c, t));
    }
    a = Di(s[e.nextIndex], i);
  }
  const l = jn({ negate: n });
  return o.length === 1 ? l.body = o[0].body : (l.kind = "intersection", l.body = o.map((c) => c.body.length === 1 ? c.body[0] : c)), e.nextIndex++, l;
}
function cd({ kind: n, negate: e, value: t }, s) {
  const { normalizeUnknownPropertyNames: r, skipPropertyNameValidation: o, unicodePropertyMap: i } = s;
  if (n === "property") {
    const a = Es(t);
    if (Qr.has(a) && !(i != null && i.has(a))) n = "posix", t = a;
    else return kt(t, { negate: e, normalizeUnknownPropertyNames: r, skipPropertyNameValidation: o, unicodePropertyMap: i });
  }
  return n === "posix" ? yd(t, { negate: e }) : Cr(n, { negate: e });
}
function ud(n, e, t) {
  const { tokens: s, capturingGroups: r, namedGroupsByName: o, skipLookbehindValidation: i, walk: a } = e, l = _d(n), c = l.type === "AbsenceFunction", h = Bi(l), u = h && l.negate;
  if (l.type === "CapturingGroup" && (r.push(l), l.name && Lp(o, l.name, []).push(l)), c && t.isInAbsenceFunction) throw new Error("Nested absence function not supported by Oniguruma");
  let p = Fi(s[e.nextIndex]);
  for (; p.type !== "GroupClose"; ) {
    if (p.type === "Alternator") l.body.push(lt()), e.nextIndex++;
    else {
      const d = l.body.at(-1), f = a(d, { ...t, isInAbsenceFunction: t.isInAbsenceFunction || c, isInLookbehind: t.isInLookbehind || h, isInNegLookbehind: t.isInNegLookbehind || u });
      if (d.body.push(f), (h || t.isInLookbehind) && !i) {
        const b = "Lookbehind includes a pattern not allowed by Oniguruma";
        if (u || t.isInNegLookbehind) {
          if (zi(f) || f.type === "CapturingGroup") throw new Error(b);
        } else if (zi(f) || Bi(f) && f.negate) throw new Error(b);
      }
    }
    p = Fi(s[e.nextIndex]);
  }
  return e.nextIndex++, l;
}
function hd({ kind: n, min: e, max: t }, s) {
  const r = s.parent, o = r.body.at(-1);
  if (!o || !sd(o)) throw new Error("Quantifier requires a repeatable token");
  const i = dl(n, e, t, o);
  return r.body.pop(), i;
}
function pd({ raw: n }, e) {
  const { capturingGroups: t, subroutines: s } = e;
  let r = n.slice(3, -1);
  const o = /^(?<sign>[-+]?)0*(?<num>[1-9]\d*)$/.exec(r);
  if (o) {
    const a = +o.groups.num, l = t.length;
    if (e.hasNumberedRef = !0, r = { "": a, "+": l + a, "-": l + 1 - a }[o.groups.sign], r < 1) throw new Error("Invalid subroutine number");
  } else r === "0" && (r = 0);
  const i = fl(r);
  return s.push(i), i;
}
function dd(n, e) {
  return { type: "AbsenceFunction", kind: n, body: kn(e == null ? void 0 : e.body) };
}
function lt(n) {
  return { type: "Alternative", body: gl(n == null ? void 0 : n.body) };
}
function xr(n, e) {
  const t = { type: "Assertion", kind: n };
  return (n === "word_boundary" || n === "text_segment_boundary") && (t.negate = !!(e != null && e.negate)), t;
}
function kr(n, e) {
  const t = !!(e != null && e.orphan);
  return { type: "Backreference", ref: n, ...t && { orphan: t } };
}
function pl(n, e) {
  const t = { name: void 0, isSubroutined: !1, ...e };
  if (t.name !== void 0 && !wd(t.name)) throw new Error(`Group name "${t.name}" invalid in Oniguruma`);
  return { type: "CapturingGroup", number: n, ...t.name && { name: t.name }, ...t.isSubroutined && { isSubroutined: t.isSubroutined }, body: kn(e == null ? void 0 : e.body) };
}
function As(n, e) {
  const t = { useLastValid: !1, ...e };
  if (n > 1114111) {
    const s = n.toString(16);
    if (t.useLastValid) n = 1114111;
    else throw n > 1310719 ? new Error(`Invalid code point out of range "\\x{${s}}"`) : new Error(`Invalid code point out of range in JS "\\x{${s}}"`);
  }
  return { type: "Character", value: n };
}
function jn(n) {
  const e = { kind: "union", negate: !1, ...n };
  return { type: "CharacterClass", kind: e.kind, negate: e.negate, body: gl(n == null ? void 0 : n.body) };
}
function fd(n, e) {
  if (e.value < n.value) throw new Error("Character class range out of order");
  return { type: "CharacterClassRange", min: n, max: e };
}
function Cr(n, e) {
  const t = !!(e != null && e.negate), s = { type: "CharacterSet", kind: n };
  return (n === "digit" || n === "hex" || n === "newline" || n === "space" || n === "word") && (s.negate = t), (n === "text_segment" || n === "newline" && !t) && (s.variableLength = !0), s;
}
function gd(n, e = {}) {
  if (n === "keep") return { type: "Directive", kind: n };
  if (n === "flags") return { type: "Directive", kind: n, flags: Dt(e.flags) };
  throw new Error(`Unexpected directive kind "${n}"`);
}
function md(n) {
  return { type: "Flags", ...n };
}
function pe(n) {
  const e = n == null ? void 0 : n.atomic, t = n == null ? void 0 : n.flags;
  if (e && t) throw new Error("Atomic group cannot have flags");
  return { type: "Group", ...e && { atomic: e }, ...t && { flags: t }, body: kn(n == null ? void 0 : n.body) };
}
function Je(n) {
  const e = { behind: !1, negate: !1, ...n };
  return { type: "LookaroundAssertion", kind: e.behind ? "lookbehind" : "lookahead", negate: e.negate, body: kn(n == null ? void 0 : n.body) };
}
function bd(n, e, t) {
  return { type: "NamedCallout", kind: n, tag: e, arguments: t };
}
function yd(n, e) {
  const t = !!(e != null && e.negate);
  if (!Qr.has(n)) throw new Error(`Invalid POSIX class "${n}"`);
  return { type: "CharacterSet", kind: "posix", value: n, negate: t };
}
function dl(n, e, t, s) {
  if (e > t) throw new Error("Invalid reversed quantifier range");
  return { type: "Quantifier", kind: n, min: e, max: t, body: s };
}
function vd(n, e) {
  return { type: "Regex", body: kn(e == null ? void 0 : e.body), flags: n };
}
function fl(n) {
  return { type: "Subroutine", ref: n };
}
function kt(n, e) {
  var r;
  const t = { negate: !1, normalizeUnknownPropertyNames: !1, skipPropertyNameValidation: !1, unicodePropertyMap: null, ...e };
  let s = (r = t.unicodePropertyMap) == null ? void 0 : r.get(Es(n));
  if (!s) {
    if (t.normalizeUnknownPropertyNames) s = xd(n);
    else if (t.unicodePropertyMap && !t.skipPropertyNameValidation) throw new Error(U`Invalid Unicode property "\p{${n}}"`);
  }
  return { type: "CharacterSet", kind: "property", value: s ?? n, negate: t.negate };
}
function _d({ flags: n, kind: e, name: t, negate: s, number: r }) {
  switch (e) {
    case "absence_repeater":
      return dd("repeater");
    case "atomic":
      return pe({ atomic: !0 });
    case "capturing":
      return pl(r, { name: t });
    case "group":
      return pe({ flags: n });
    case "lookahead":
    case "lookbehind":
      return Je({ behind: e === "lookbehind", negate: s });
    default:
      throw new Error(`Unexpected group kind "${e}"`);
  }
}
function kn(n) {
  if (n === void 0) n = [lt()];
  else if (!Array.isArray(n) || !n.length || !n.every((e) => e.type === "Alternative")) throw new Error("Invalid body; expected array of one or more Alternative nodes");
  return n;
}
function gl(n) {
  if (n === void 0) n = [];
  else if (!Array.isArray(n) || !n.every((e) => !!e.type)) throw new Error("Invalid body; expected array of nodes");
  return n;
}
function zi(n) {
  return n.type === "LookaroundAssertion" && n.kind === "lookahead";
}
function Bi(n) {
  return n.type === "LookaroundAssertion" && n.kind === "lookbehind";
}
function wd(n) {
  return /^[\p{Alpha}\p{Pc}][^)]*$/u.test(n);
}
function xd(n) {
  return n.trim().replace(/[- _]+/g, "_").replace(/[A-Z][a-z]+(?=[A-Z])/g, "$&_").replace(/[A-Za-z]+/g, (e) => e[0].toUpperCase() + e.slice(1).toLowerCase());
}
function Es(n) {
  return n.replace(/[- _]+/g, "").toLowerCase();
}
function Di(n, e) {
  const t = e;
  return Dt(n, `Unclosed character class${(t == null ? void 0 : t.type) === "Character" && t.value === 93 && t.raw === "]" ? ' (started with "]")' : ""}`);
}
function Fi(n) {
  return Dt(n, "Unclosed group");
}
function Jt(n, e, t = null) {
  function s(o, i) {
    for (let a = 0; a < o.length; a++) {
      const l = r(o[a], i, a, o);
      a = Math.max(-1, a + l);
    }
  }
  function r(o, i = null, a = null, l = null) {
    var k, _;
    let c = 0, h = !1;
    const u = { node: o, parent: i, key: a, container: l, root: n, remove() {
      On(l).splice(Math.max(0, vt(a) + c), 1), c--, h = !0;
    }, removeAllNextSiblings() {
      return On(l).splice(vt(a) + 1);
    }, removeAllPrevSiblings() {
      const x = vt(a) + c;
      return c -= x, On(l).splice(0, Math.max(0, x));
    }, replaceWith(x, $ = {}) {
      const A = !!$.traverse;
      l ? l[Math.max(0, vt(a) + c)] = x : Dt(i, "Can't replace root node")[a] = x, A && r(x, i, a, l), h = !0;
    }, replaceWithMultiple(x, $ = {}) {
      const A = !!$.traverse;
      if (On(l).splice(Math.max(0, vt(a) + c), 1, ...x), c += x.length - 1, A) {
        let T = 0;
        for (let M = 0; M < x.length; M++) T += r(x[M], i, vt(a) + M + T, l);
      }
      h = !0;
    }, skip() {
      h = !0;
    } }, { type: p } = o, d = e["*"], f = e[p], b = typeof d == "function" ? d : d == null ? void 0 : d.enter, v = typeof f == "function" ? f : f == null ? void 0 : f.enter;
    if (b == null || b(u, t), v == null || v(u, t), !h) switch (p) {
      case "AbsenceFunction":
      case "Alternative":
      case "CapturingGroup":
      case "CharacterClass":
      case "Group":
      case "LookaroundAssertion":
        s(o.body, o);
        break;
      case "Assertion":
      case "Backreference":
      case "Character":
      case "CharacterSet":
      case "Directive":
      case "Flags":
      case "NamedCallout":
      case "Subroutine":
        break;
      case "CharacterClassRange":
        r(o.min, o, "min"), r(o.max, o, "max");
        break;
      case "Quantifier":
        r(o.body, o, "body");
        break;
      case "Regex":
        s(o.body, o), r(o.flags, o, "flags");
        break;
      default:
        throw new Error(`Unexpected node type "${p}"`);
    }
    return (k = f == null ? void 0 : f.exit) == null || k.call(f, u, t), (_ = d == null ? void 0 : d.exit) == null || _.call(d, u, t), c;
  }
  return r(n), n;
}
function On(n) {
  if (!Array.isArray(n)) throw new Error("Container expected");
  return n;
}
function vt(n) {
  if (typeof n != "number") throw new Error("Numeric key expected");
  return n;
}
const kd = String.raw`\(\?(?:[:=!>A-Za-z\-]|<[=!]|\(DEFINE\))`;
function Cd(n, e) {
  for (let t = 0; t < n.length; t++)
    n[t] >= e && n[t]++;
}
function $d(n, e, t, s) {
  return n.slice(0, e) + s + n.slice(e + t.length);
}
const ce = Object.freeze({
  DEFAULT: "DEFAULT",
  CHAR_CLASS: "CHAR_CLASS"
});
function Kr(n, e, t, s) {
  const r = new RegExp(String.raw`${e}|(?<$skip>\[\^?|\\?.)`, "gsu"), o = [!1];
  let i = 0, a = "";
  for (const l of n.matchAll(r)) {
    const { 0: c, groups: { $skip: h } } = l;
    if (!h && (!s || s === ce.DEFAULT == !i)) {
      t instanceof Function ? a += t(l, {
        context: i ? ce.CHAR_CLASS : ce.DEFAULT,
        negated: o[o.length - 1]
      }) : a += t;
      continue;
    }
    c[0] === "[" ? (i++, o.push(c[1] === "^")) : c === "]" && i && (i--, o.pop()), a += c;
  }
  return a;
}
function ml(n, e, t, s) {
  Kr(n, e, t, s);
}
function Sd(n, e, t = 0, s) {
  if (!new RegExp(e, "su").test(n))
    return null;
  const r = new RegExp(`${e}|(?<$skip>\\\\?.)`, "gsu");
  r.lastIndex = t;
  let o = 0, i;
  for (; i = r.exec(n); ) {
    const { 0: a, groups: { $skip: l } } = i;
    if (!l && (!s || s === ce.DEFAULT == !o))
      return i;
    a === "[" ? o++ : a === "]" && o && o--, r.lastIndex == i.index && r.lastIndex++;
  }
  return null;
}
function zn(n, e, t) {
  return !!Sd(n, e, 0, t);
}
function Ad(n, e) {
  const t = /\\?./gsu;
  t.lastIndex = e;
  let s = n.length, r = 0, o = 1, i;
  for (; i = t.exec(n); ) {
    const [a] = i;
    if (a === "[")
      r++;
    else if (r)
      a === "]" && r--;
    else if (a === "(")
      o++;
    else if (a === ")" && (o--, !o)) {
      s = i.index;
      break;
    }
  }
  return n.slice(e, s);
}
const Gi = new RegExp(String.raw`(?<noncapturingStart>${kd})|(?<capturingStart>\((?:\?<[^>]+>)?)|\\?.`, "gsu");
function Ed(n, e) {
  const t = (e == null ? void 0 : e.hiddenCaptures) ?? [];
  let s = (e == null ? void 0 : e.captureTransfers) ?? /* @__PURE__ */ new Map();
  if (!/\(\?>/.test(n))
    return {
      pattern: n,
      captureTransfers: s,
      hiddenCaptures: t
    };
  const r = "(?>", o = "(?:(?=(", i = [0], a = [];
  let l = 0, c = 0, h = NaN, u;
  do {
    u = !1;
    let p = 0, d = 0, f = !1, b;
    for (Gi.lastIndex = Number.isNaN(h) ? 0 : h + o.length; b = Gi.exec(n); ) {
      const { 0: v, index: k, groups: { capturingStart: _, noncapturingStart: x } } = b;
      if (v === "[")
        p++;
      else if (p)
        v === "]" && p--;
      else if (v === r && !f)
        h = k, f = !0;
      else if (f && x)
        d++;
      else if (_)
        f ? d++ : (l++, i.push(l + c));
      else if (v === ")" && f) {
        if (!d) {
          c++;
          const $ = l + c;
          if (n = `${n.slice(0, h)}${o}${n.slice(h + r.length, k)}))<$$${$}>)${n.slice(k + 1)}`, u = !0, a.push($), Cd(t, $), s.size) {
            const A = /* @__PURE__ */ new Map();
            s.forEach((T, M) => {
              A.set(
                M >= $ ? M + 1 : M,
                T.map((ee) => ee >= $ ? ee + 1 : ee)
              );
            }), s = A;
          }
          break;
        }
        d--;
      }
    }
  } while (u);
  return t.push(...a), n = Kr(
    n,
    String.raw`\\(?<backrefNum>[1-9]\d*)|<\$\$(?<wrappedBackrefNum>\d+)>`,
    ({ 0: p, groups: { backrefNum: d, wrappedBackrefNum: f } }) => {
      if (d) {
        const b = +d;
        if (b > i.length - 1)
          throw new Error(`Backref "${p}" greater than number of captures`);
        return `\\${i[b]}`;
      }
      return `\\${f}`;
    },
    ce.DEFAULT
  ), {
    pattern: n,
    captureTransfers: s,
    hiddenCaptures: t
  };
}
const bl = String.raw`(?:[?*+]|\{\d+(?:,\d*)?\})`, Zs = new RegExp(String.raw`
\\(?: \d+
  | c[A-Za-z]
  | [gk]<[^>]+>
  | [pPu]\{[^\}]+\}
  | u[A-Fa-f\d]{4}
  | x[A-Fa-f\d]{2}
  )
| \((?: \? (?: [:=!>]
  | <(?:[=!]|[^>]+>)
  | [A-Za-z\-]+:
  | \(DEFINE\)
  ))?
| (?<qBase>${bl})(?<qMod>[?+]?)(?<invalidQ>[?*+\{]?)
| \\?.
`.replace(/\s+/g, ""), "gsu");
function Rd(n) {
  if (!new RegExp(`${bl}\\+`).test(n))
    return {
      pattern: n
    };
  const e = [];
  let t = null, s = null, r = "", o = 0, i;
  for (Zs.lastIndex = 0; i = Zs.exec(n); ) {
    const { 0: a, index: l, groups: { qBase: c, qMod: h, invalidQ: u } } = i;
    if (a === "[")
      o || (s = l), o++;
    else if (a === "]")
      o ? o-- : s = null;
    else if (!o)
      if (h === "+" && r && !r.startsWith("(")) {
        if (u)
          throw new Error(`Invalid quantifier "${a}"`);
        let p = -1;
        if (/^\{\d+\}$/.test(c))
          n = $d(n, l + c.length, h, "");
        else {
          if (r === ")" || r === "]") {
            const d = r === ")" ? t : s;
            if (d === null)
              throw new Error(`Invalid unmatched "${r}"`);
            n = `${n.slice(0, d)}(?>${n.slice(d, l)}${c})${n.slice(l + a.length)}`;
          } else
            n = `${n.slice(0, l - r.length)}(?>${r}${c})${n.slice(l + a.length)}`;
          p += 4;
        }
        Zs.lastIndex += p;
      } else a[0] === "(" ? e.push(l) : a === ")" && (t = e.length ? e.pop() : null);
    r = a;
  }
  return {
    pattern: n
  };
}
const ae = String.raw, Td = ae`\\g<(?<gRNameOrNum>[^>&]+)&R=(?<gRDepth>[^>]+)>`, $r = ae`\(\?R=(?<rDepth>[^\)]+)\)|${Td}`, Rs = ae`\(\?<(?![=!])(?<captureName>[^>]+)>`, yl = ae`${Rs}|(?<unnamed>\()(?!\?)`, Ze = new RegExp(ae`${Rs}|${$r}|\(\?|\\?.`, "gsu"), Xs = "Cannot use multiple overlapping recursions";
function Id(n, e) {
  const { hiddenCaptures: t, mode: s } = {
    hiddenCaptures: [],
    mode: "plugin",
    ...e
  };
  let r = (e == null ? void 0 : e.captureTransfers) ?? /* @__PURE__ */ new Map();
  if (!new RegExp($r, "su").test(n))
    return {
      pattern: n,
      captureTransfers: r,
      hiddenCaptures: t
    };
  if (s === "plugin" && zn(n, ae`\(\?\(DEFINE\)`, ce.DEFAULT))
    throw new Error("DEFINE groups cannot be used with recursion");
  const o = [], i = zn(n, ae`\\[1-9]`, ce.DEFAULT), a = /* @__PURE__ */ new Map(), l = [];
  let c = !1, h = 0, u = 0, p;
  for (Ze.lastIndex = 0; p = Ze.exec(n); ) {
    const { 0: d, groups: { captureName: f, rDepth: b, gRNameOrNum: v, gRDepth: k } } = p;
    if (d === "[")
      h++;
    else if (h)
      d === "]" && h--;
    else if (b) {
      if (Ui(b), c)
        throw new Error(Xs);
      if (i)
        throw new Error(
          // When used in `external` mode by transpilers other than Regex+, backrefs might have
          // gone through conversion from named to numbered, so avoid a misleading error
          `${s === "external" ? "Backrefs" : "Numbered backrefs"} cannot be used with global recursion`
        );
      const _ = n.slice(0, p.index), x = n.slice(Ze.lastIndex);
      if (zn(x, $r, ce.DEFAULT))
        throw new Error(Xs);
      const $ = +b - 1;
      n = ji(
        _,
        x,
        $,
        !1,
        t,
        o,
        u
      ), r = Hi(
        r,
        _,
        $,
        o.length,
        0,
        u
      );
      break;
    } else if (v) {
      Ui(k);
      let _ = !1;
      for (const Z of l)
        if (Z.name === v || Z.num === +v) {
          if (_ = !0, Z.hasRecursedWithin)
            throw new Error(Xs);
          break;
        }
      if (!_)
        throw new Error(ae`Recursive \g cannot be used outside the referenced group "${s === "external" ? v : ae`\g<${v}&R=${k}>`}"`);
      const x = a.get(v), $ = Ad(n, x);
      if (i && zn($, ae`${Rs}|\((?!\?)`, ce.DEFAULT))
        throw new Error(
          // When used in `external` mode by transpilers other than Regex+, backrefs might have
          // gone through conversion from named to numbered, so avoid a misleading error
          `${s === "external" ? "Backrefs" : "Numbered backrefs"} cannot be used with recursion of capturing groups`
        );
      const A = n.slice(x, p.index), T = $.slice(A.length + d.length), M = o.length, ee = +k - 1, j = ji(
        A,
        T,
        ee,
        !0,
        t,
        o,
        u
      );
      r = Hi(
        r,
        A,
        ee,
        o.length - M,
        M,
        u
      );
      const ge = n.slice(0, x), mt = n.slice(x + $.length);
      n = `${ge}${j}${mt}`, Ze.lastIndex += j.length - d.length - A.length - T.length, l.forEach((Z) => Z.hasRecursedWithin = !0), c = !0;
    } else if (f)
      u++, a.set(String(u), Ze.lastIndex), a.set(f, Ze.lastIndex), l.push({
        num: u,
        name: f
      });
    else if (d[0] === "(") {
      const _ = d === "(";
      _ && (u++, a.set(String(u), Ze.lastIndex)), l.push(_ ? { num: u } : {});
    } else d === ")" && l.pop();
  }
  return t.push(...o), {
    pattern: n,
    captureTransfers: r,
    hiddenCaptures: t
  };
}
function Ui(n) {
  const e = `Max depth must be integer between 2 and 100; used ${n}`;
  if (!/^[1-9]\d*$/.test(n))
    throw new Error(e);
  if (n = +n, n < 2 || n > 100)
    throw new Error(e);
}
function ji(n, e, t, s, r, o, i) {
  const a = /* @__PURE__ */ new Set();
  s && ml(n + e, Rs, ({ groups: { captureName: c } }) => {
    a.add(c);
  }, ce.DEFAULT);
  const l = [
    t,
    s ? a : null,
    r,
    o,
    i
  ];
  return `${n}${qi(`(?:${n}`, "forward", ...l)}(?:)${qi(`${e})`, "backward", ...l)}${e}`;
}
function qi(n, e, t, s, r, o, i) {
  const l = (h) => e === "forward" ? h + 2 : t - h + 2 - 1;
  let c = "";
  for (let h = 0; h < t; h++) {
    const u = l(h);
    c += Kr(
      n,
      ae`${yl}|\\k<(?<backref>[^>]+)>`,
      ({ 0: p, groups: { captureName: d, unnamed: f, backref: b } }) => {
        if (b && s && !s.has(b))
          return p;
        const v = `_$${u}`;
        if (f || d) {
          const k = i + o.length + 1;
          return o.push(k), Pd(r, k), f ? p : `(?<${d}${v}>`;
        }
        return ae`\k<${b}${v}>`;
      },
      ce.DEFAULT
    );
  }
  return c;
}
function Pd(n, e) {
  for (let t = 0; t < n.length; t++)
    n[t] >= e && n[t]++;
}
function Hi(n, e, t, s, r, o) {
  if (n.size && s) {
    let i = 0;
    ml(e, yl, () => i++, ce.DEFAULT);
    const a = o - i + r, l = /* @__PURE__ */ new Map();
    return n.forEach((c, h) => {
      const u = (s - i * t) / t, p = i * t, d = h > a + i ? h + s : h, f = [];
      for (const b of c)
        if (b <= a)
          f.push(b);
        else if (b > a + i + u)
          f.push(b + s);
        else if (b <= a + i)
          for (let v = 0; v <= t; v++)
            f.push(b + i * v);
        else
          for (let v = 0; v <= t; v++)
            f.push(b + p + u * v);
      l.set(d, f);
    }), l;
  }
  return n;
}
var D = String.fromCodePoint, E = String.raw, de = {}, Ts = globalThis.RegExp;
de.flagGroups = (() => {
  try {
    new Ts("(?i:)");
  } catch {
    return !1;
  }
  return !0;
})();
de.unicodeSets = (() => {
  try {
    new Ts("[[]]", "v");
  } catch {
    return !1;
  }
  return !0;
})();
de.bugFlagVLiteralHyphenIsRange = de.unicodeSets ? (() => {
  try {
    new Ts(E`[\d\-a]`, "v");
  } catch {
    return !0;
  }
  return !1;
})() : !1;
de.bugNestedClassIgnoresNegation = de.unicodeSets && new Ts("[[^a]]", "v").test("a");
function os(n, { enable: e, disable: t }) {
  return {
    dotAll: !(t != null && t.dotAll) && !!(e != null && e.dotAll || n.dotAll),
    ignoreCase: !(t != null && t.ignoreCase) && !!(e != null && e.ignoreCase || n.ignoreCase)
  };
}
function pn(n, e, t) {
  return n.has(e) || n.set(e, t), n.get(e);
}
function Sr(n, e) {
  return Wi[n] >= Wi[e];
}
function Ld(n, e) {
  if (n == null)
    throw new Error(e ?? "Value expected");
  return n;
}
var Wi = {
  ES2025: 2025,
  ES2024: 2024,
  ES2018: 2018
}, Nd = (
  /** @type {const} */
  {
    auto: "auto",
    ES2025: "ES2025",
    ES2024: "ES2024",
    ES2018: "ES2018"
  }
);
function vl(n = {}) {
  if ({}.toString.call(n) !== "[object Object]")
    throw new Error("Unexpected options");
  if (n.target !== void 0 && !Nd[n.target])
    throw new Error(`Unexpected target "${n.target}"`);
  const e = {
    // Sets the level of emulation rigor/strictness.
    accuracy: "default",
    // Disables advanced emulation that relies on returning a `RegExp` subclass, resulting in
    // certain patterns not being emulatable.
    avoidSubclass: !1,
    // Oniguruma flags; a string with `i`, `m`, `x`, `D`, `S`, `W`, `y{g}` in any order (all
    // optional). Oniguruma's `m` is equivalent to JavaScript's `s` (`dotAll`).
    flags: "",
    // Include JavaScript flag `g` (`global`) in the result.
    global: !1,
    // Include JavaScript flag `d` (`hasIndices`) in the result.
    hasIndices: !1,
    // Delay regex construction until first use if the transpiled pattern is at least this length.
    lazyCompileLength: 1 / 0,
    // JavaScript version used for generated regexes. Using `auto` detects the best value based on
    // your environment. Later targets allow faster processing, simpler generated source, and
    // support for additional features.
    target: "auto",
    // Disables minifications that simplify the pattern without changing the meaning.
    verbose: !1,
    ...n,
    // Advanced options that override standard behavior, error checking, and flags when enabled.
    rules: {
      // Useful with TextMate grammars that merge backreferences across patterns.
      allowOrphanBackrefs: !1,
      // Use ASCII `\b` and `\B`, which increases search performance of generated regexes.
      asciiWordBoundaries: !1,
      // Allow unnamed captures and numbered calls (backreferences and subroutines) when using
      // named capture. This is Oniguruma option `ONIG_OPTION_CAPTURE_GROUP`; on by default in
      // `vscode-oniguruma`.
      captureGroup: !1,
      // Change the recursion depth limit from Oniguruma's `20` to an integer `2`–`20`.
      recursionLimit: 20,
      // `^` as `\A`; `$` as`\Z`. Improves search performance of generated regexes without changing
      // the meaning if searching line by line. This is Oniguruma option `ONIG_OPTION_SINGLELINE`.
      singleline: !1,
      ...n.rules
    }
  };
  return e.target === "auto" && (e.target = de.flagGroups ? "ES2025" : de.unicodeSets ? "ES2024" : "ES2018"), e;
}
var Md = "[	-\r ]", Od = /* @__PURE__ */ new Set([
  D(304),
  // İ
  D(305)
  // ı
]), ke = E`[\p{L}\p{M}\p{N}\p{Pc}]`;
function _l(n) {
  if (Od.has(n))
    return [n];
  const e = /* @__PURE__ */ new Set(), t = n.toLowerCase(), s = t.toUpperCase(), r = Dd.get(t), o = zd.get(t), i = Bd.get(t);
  return [...s].length === 1 && e.add(s), i && e.add(i), r && e.add(r), e.add(t), o && e.add(o), [...e];
}
var Zr = /* @__PURE__ */ new Map(
  `C Other
Cc Control cntrl
Cf Format
Cn Unassigned
Co Private_Use
Cs Surrogate
L Letter
LC Cased_Letter
Ll Lowercase_Letter
Lm Modifier_Letter
Lo Other_Letter
Lt Titlecase_Letter
Lu Uppercase_Letter
M Mark Combining_Mark
Mc Spacing_Mark
Me Enclosing_Mark
Mn Nonspacing_Mark
N Number
Nd Decimal_Number digit
Nl Letter_Number
No Other_Number
P Punctuation punct
Pc Connector_Punctuation
Pd Dash_Punctuation
Pe Close_Punctuation
Pf Final_Punctuation
Pi Initial_Punctuation
Po Other_Punctuation
Ps Open_Punctuation
S Symbol
Sc Currency_Symbol
Sk Modifier_Symbol
Sm Math_Symbol
So Other_Symbol
Z Separator
Zl Line_Separator
Zp Paragraph_Separator
Zs Space_Separator
ASCII
ASCII_Hex_Digit AHex
Alphabetic Alpha
Any
Assigned
Bidi_Control Bidi_C
Bidi_Mirrored Bidi_M
Case_Ignorable CI
Cased
Changes_When_Casefolded CWCF
Changes_When_Casemapped CWCM
Changes_When_Lowercased CWL
Changes_When_NFKC_Casefolded CWKCF
Changes_When_Titlecased CWT
Changes_When_Uppercased CWU
Dash
Default_Ignorable_Code_Point DI
Deprecated Dep
Diacritic Dia
Emoji
Emoji_Component EComp
Emoji_Modifier EMod
Emoji_Modifier_Base EBase
Emoji_Presentation EPres
Extended_Pictographic ExtPict
Extender Ext
Grapheme_Base Gr_Base
Grapheme_Extend Gr_Ext
Hex_Digit Hex
IDS_Binary_Operator IDSB
IDS_Trinary_Operator IDST
ID_Continue IDC
ID_Start IDS
Ideographic Ideo
Join_Control Join_C
Logical_Order_Exception LOE
Lowercase Lower
Math
Noncharacter_Code_Point NChar
Pattern_Syntax Pat_Syn
Pattern_White_Space Pat_WS
Quotation_Mark QMark
Radical
Regional_Indicator RI
Sentence_Terminal STerm
Soft_Dotted SD
Terminal_Punctuation Term
Unified_Ideograph UIdeo
Uppercase Upper
Variation_Selector VS
White_Space space
XID_Continue XIDC
XID_Start XIDS`.split(/\s/).map((n) => [Es(n), n])
), zd = /* @__PURE__ */ new Map([
  ["s", D(383)],
  // s, ſ
  [D(383), "s"]
  // ſ, s
]), Bd = /* @__PURE__ */ new Map([
  [D(223), D(7838)],
  // ß, ẞ
  [D(107), D(8490)],
  // k, K (Kelvin)
  [D(229), D(8491)],
  // å, Å (Angstrom)
  [D(969), D(8486)]
  // ω, Ω (Ohm)
]), Dd = new Map([
  Le(453),
  Le(456),
  Le(459),
  Le(498),
  ...Js(8072, 8079),
  ...Js(8088, 8095),
  ...Js(8104, 8111),
  Le(8124),
  Le(8140),
  Le(8188)
]), Fd = /* @__PURE__ */ new Map([
  ["alnum", E`[\p{Alpha}\p{Nd}]`],
  ["alpha", E`\p{Alpha}`],
  ["ascii", E`\p{ASCII}`],
  ["blank", E`[\p{Zs}\t]`],
  ["cntrl", E`\p{Cc}`],
  ["digit", E`\p{Nd}`],
  ["graph", E`[\P{space}&&\P{Cc}&&\P{Cn}&&\P{Cs}]`],
  ["lower", E`\p{Lower}`],
  ["print", E`[[\P{space}&&\P{Cc}&&\P{Cn}&&\P{Cs}]\p{Zs}]`],
  ["punct", E`[\p{P}\p{S}]`],
  // Updated value from Onig 6.9.9; changed from Unicode `\p{punct}`
  ["space", E`\p{space}`],
  ["upper", E`\p{Upper}`],
  ["word", E`[\p{Alpha}\p{M}\p{Nd}\p{Pc}]`],
  ["xdigit", E`\p{AHex}`]
]);
function Gd(n, e) {
  const t = [];
  for (let s = n; s <= e; s++)
    t.push(s);
  return t;
}
function Le(n) {
  const e = D(n);
  return [e.toLowerCase(), e];
}
function Js(n, e) {
  return Gd(n, e).map((t) => Le(t));
}
var wl = /* @__PURE__ */ new Set([
  "Lower",
  "Lowercase",
  "Upper",
  "Uppercase",
  "Ll",
  "Lowercase_Letter",
  "Lt",
  "Titlecase_Letter",
  "Lu",
  "Uppercase_Letter"
  // The `Changes_When_*` properties (and their aliases) could be included, but they're very rare.
  // Some other properties include a handful of chars with specific cases only, but these chars are
  // generally extreme edge cases and using such properties case insensitively generally produces
  // undesired behavior anyway
]);
function Ud(n, e) {
  const t = {
    // A couple edge cases exist where options `accuracy` and `bestEffortTarget` are used:
    // - `CharacterSet` kind `text_segment` (`\X`): An exact representation would require heavy
    //   Unicode data; a best-effort approximation requires knowing the target.
    // - `CharacterSet` kind `posix` with values `graph` and `print`: Their complex Unicode
    //   representations would be hard to change to ASCII versions after the fact in the generator
    //   based on `target`/`accuracy`, so produce the appropriate structure here.
    accuracy: "default",
    asciiWordBoundaries: !1,
    avoidSubclass: !1,
    bestEffortTarget: "ES2025",
    ...e
  };
  xl(n);
  const s = {
    accuracy: t.accuracy,
    asciiWordBoundaries: t.asciiWordBoundaries,
    avoidSubclass: t.avoidSubclass,
    flagDirectivesByAlt: /* @__PURE__ */ new Map(),
    jsGroupNameMap: /* @__PURE__ */ new Map(),
    minTargetEs2024: Sr(t.bestEffortTarget, "ES2024"),
    passedLookbehind: !1,
    strategy: null,
    // Subroutines can appear before the groups they ref, so collect reffed nodes for a second pass 
    subroutineRefMap: /* @__PURE__ */ new Map(),
    supportedGNodes: /* @__PURE__ */ new Set(),
    digitIsAscii: n.flags.digitIsAscii,
    spaceIsAscii: n.flags.spaceIsAscii,
    wordIsAscii: n.flags.wordIsAscii
  };
  Jt(n, jd, s);
  const r = {
    dotAll: n.flags.dotAll,
    ignoreCase: n.flags.ignoreCase
  }, o = {
    currentFlags: r,
    prevFlags: null,
    globalFlags: r,
    groupOriginByCopy: /* @__PURE__ */ new Map(),
    groupsByName: /* @__PURE__ */ new Map(),
    multiplexCapturesToLeftByRef: /* @__PURE__ */ new Map(),
    openRefs: /* @__PURE__ */ new Map(),
    reffedNodesByReferencer: /* @__PURE__ */ new Map(),
    subroutineRefMap: s.subroutineRefMap
  };
  Jt(n, qd, o);
  const i = {
    groupsByName: o.groupsByName,
    highestOrphanBackref: 0,
    numCapturesToLeft: 0,
    reffedNodesByReferencer: o.reffedNodesByReferencer
  };
  return Jt(n, Hd, i), n._originMap = o.groupOriginByCopy, n._strategy = s.strategy, n;
}
var jd = {
  AbsenceFunction({ node: n, parent: e, replaceWith: t }) {
    const { body: s, kind: r } = n;
    if (r === "repeater") {
      const o = pe();
      o.body[0].body.push(
        // Insert own alts as `body`
        Je({ negate: !0, body: s }),
        kt("Any")
      );
      const i = pe();
      i.body[0].body.push(
        dl("greedy", 0, 1 / 0, o)
      ), t(z(i, e), { traverse: !0 });
    } else
      throw new Error('Unsupported absence function "(?~|"');
  },
  Alternative: {
    enter({ node: n, parent: e, key: t }, { flagDirectivesByAlt: s }) {
      const r = n.body.filter((o) => o.kind === "flags");
      for (let o = t + 1; o < e.body.length; o++) {
        const i = e.body[o];
        pn(s, i, []).push(...r);
      }
    },
    exit({ node: n }, { flagDirectivesByAlt: e }) {
      var t;
      if ((t = e.get(n)) != null && t.length) {
        const s = Cl(e.get(n));
        if (s) {
          const r = pe({ flags: s });
          r.body[0].body = n.body, n.body = [z(r, n)];
        }
      }
    }
  },
  Assertion({ node: n, parent: e, key: t, container: s, root: r, remove: o, replaceWith: i }, a) {
    const { kind: l, negate: c } = n, { asciiWordBoundaries: h, avoidSubclass: u, supportedGNodes: p, wordIsAscii: d } = a;
    if (l === "text_segment_boundary")
      throw new Error(`Unsupported text segment boundary "\\${c ? "Y" : "y"}"`);
    if (l === "line_end")
      i(z(Je({ body: [
        lt({ body: [xr("string_end")] }),
        lt({ body: [As(10)] })
        // `\n`
      ] }), e));
    else if (l === "line_start")
      i(z(Ce(E`(?<=\A|\n(?!\z))`, { skipLookbehindValidation: !0 }), e));
    else if (l === "search_start")
      if (p.has(n))
        r.flags.sticky = !0, o();
      else {
        const f = s[t - 1];
        if (f && Xd(f))
          i(z(Je({ negate: !0 }), e));
        else {
          if (u)
            throw new Error(E`Uses "\G" in a way that requires a subclass`);
          i(Ne(xr("string_start"), e)), a.strategy = "clip_search";
        }
      }
    else if (!(l === "string_end" || l === "string_start")) if (l === "string_end_newline")
      i(z(Ce(E`(?=\n?\z)`), e));
    else if (l === "word_boundary") {
      if (!d && !h) {
        const f = `(?:(?<=${ke})(?!${ke})|(?<!${ke})(?=${ke}))`, b = `(?:(?<=${ke})(?=${ke})|(?<!${ke})(?!${ke}))`;
        i(z(Ce(c ? b : f), e));
      }
    } else
      throw new Error(`Unexpected assertion kind "${l}"`);
  },
  Backreference({ node: n }, { jsGroupNameMap: e }) {
    let { ref: t } = n;
    typeof t == "string" && !er(t) && (t = Ys(t, e), n.ref = t);
  },
  CapturingGroup({ node: n }, { jsGroupNameMap: e, subroutineRefMap: t }) {
    let { name: s } = n;
    s && !er(s) && (s = Ys(s, e), n.name = s), t.set(n.number, n), s && t.set(s, n);
  },
  CharacterClassRange({ node: n, parent: e, replaceWith: t }) {
    if (e.kind === "intersection") {
      const s = jn({ body: [n] });
      t(z(s, e), { traverse: !0 });
    }
  },
  CharacterSet({ node: n, parent: e, replaceWith: t }, { accuracy: s, minTargetEs2024: r, digitIsAscii: o, spaceIsAscii: i, wordIsAscii: a }) {
    const { kind: l, negate: c, value: h } = n;
    if (o && (l === "digit" || h === "digit")) {
      t(Ne(Cr("digit", { negate: c }), e));
      return;
    }
    if (i && (l === "space" || h === "space")) {
      t(z(tr(Ce(Md), c), e));
      return;
    }
    if (a && (l === "word" || h === "word")) {
      t(Ne(Cr("word", { negate: c }), e));
      return;
    }
    if (l === "any")
      t(Ne(kt("Any"), e));
    else if (l === "digit")
      t(Ne(kt("Nd", { negate: c }), e));
    else if (l !== "dot") if (l === "text_segment") {
      if (s === "strict")
        throw new Error(E`Use of "\X" requires non-strict accuracy`);
      const u = "\\p{Emoji}(?:\\p{EMod}|\\uFE0F\\u20E3?|[\\x{E0020}-\\x{E007E}]+\\x{E007F})?", p = E`\p{RI}{2}|${u}(?:\u200D${u})*`;
      t(z(Ce(
        // Close approximation of an extended grapheme cluster; see <unicode.org/reports/tr29/>
        E`(?>\r\n|${r ? E`\p{RGI_Emoji}` : p}|\P{M}\p{M}*)`,
        // Allow JS property `RGI_Emoji` through
        { skipPropertyNameValidation: !0 }
      ), e));
    } else if (l === "hex")
      t(Ne(kt("AHex", { negate: c }), e));
    else if (l === "newline")
      t(z(Ce(c ? `[^
]` : `(?>\r
?|[
\v\f\u2028\u2029])`), e));
    else if (l === "posix")
      if (!r && (h === "graph" || h === "print")) {
        if (s === "strict")
          throw new Error(`POSIX class "${h}" requires min target ES2024 or non-strict accuracy`);
        let u = {
          graph: "!-~",
          print: " -~"
        }[h];
        c && (u = `\0-${D(u.codePointAt(0) - 1)}${D(u.codePointAt(2) + 1)}-􏿿`), t(z(Ce(`[${u}]`), e));
      } else
        t(z(tr(Ce(Fd.get(h)), c), e));
    else if (l === "property")
      Zr.has(Es(h)) || (n.key = "sc");
    else if (l === "space")
      t(Ne(kt("space", { negate: c }), e));
    else if (l === "word")
      t(z(tr(Ce(ke), c), e));
    else
      throw new Error(`Unexpected character set kind "${l}"`);
  },
  Directive({ node: n, parent: e, root: t, remove: s, replaceWith: r, removeAllPrevSiblings: o, removeAllNextSiblings: i }) {
    const { kind: a, flags: l } = n;
    if (a === "flags")
      if (!l.enable && !l.disable)
        s();
      else {
        const c = pe({ flags: l });
        c.body[0].body = i(), r(z(c, e), { traverse: !0 });
      }
    else if (a === "keep") {
      const c = t.body[0], u = t.body.length === 1 && // Not emulatable if within a `CapturingGroup`
      ul(c, { type: "Group" }) && c.body[0].body.length === 1 ? c.body[0] : t;
      if (e.parent !== u || u.body.length > 1)
        throw new Error(E`Uses "\K" in a way that's unsupported`);
      const p = Je({ behind: !0 });
      p.body[0].body = o(), r(z(p, e));
    } else
      throw new Error(`Unexpected directive kind "${a}"`);
  },
  Flags({ node: n, parent: e }) {
    if (n.posixIsAscii)
      throw new Error('Unsupported flag "P"');
    if (n.textSegmentMode === "word")
      throw new Error('Unsupported flag "y{w}"');
    [
      "digitIsAscii",
      // Flag D
      "extended",
      // Flag x
      "posixIsAscii",
      // Flag P
      "spaceIsAscii",
      // Flag S
      "wordIsAscii",
      // Flag W
      "textSegmentMode"
      // Flag y{g} or y{w}
    ].forEach((t) => delete n[t]), Object.assign(n, {
      // JS flag g; no Onig equiv
      global: !1,
      // JS flag d; no Onig equiv
      hasIndices: !1,
      // JS flag m; no Onig equiv but its behavior is always on in Onig. Onig's only line break
      // char is line feed, unlike JS, so this flag isn't used since it would produce inaccurate
      // results (also allows `^` and `$` to be used in the generator for string start and end)
      multiline: !1,
      // JS flag y; no Onig equiv, but used for `\G` emulation
      sticky: n.sticky ?? !1
      // Note: Regex+ doesn't allow explicitly adding flags it handles implicitly, so leave out
      // properties `unicode` (JS flag u) and `unicodeSets` (JS flag v). Keep the existing values
      // for `ignoreCase` (flag i) and `dotAll` (JS flag s, but Onig flag m)
    }), e.options = {
      disable: {
        // Onig uses different rules for flag x than Regex+, so disable the implicit flag
        x: !0,
        // Onig has no flag to control "named capture only" mode but contextually applies its
        // behavior when named capturing is used, so disable Regex+'s implicit flag for it
        n: !0
      },
      force: {
        // Always add flag v because we're generating an AST that relies on it (it enables JS
        // support for Onig features nested classes, intersection, Unicode properties, etc.).
        // However, the generator might disable flag v based on its `target` option
        v: !0
      }
    };
  },
  Group({ node: n }) {
    if (!n.flags)
      return;
    const { enable: e, disable: t } = n.flags;
    e != null && e.extended && delete e.extended, t != null && t.extended && delete t.extended, e != null && e.dotAll && (t != null && t.dotAll) && delete e.dotAll, e != null && e.ignoreCase && (t != null && t.ignoreCase) && delete e.ignoreCase, e && !Object.keys(e).length && delete n.flags.enable, t && !Object.keys(t).length && delete n.flags.disable, !n.flags.enable && !n.flags.disable && delete n.flags;
  },
  LookaroundAssertion({ node: n }, e) {
    const { kind: t } = n;
    t === "lookbehind" && (e.passedLookbehind = !0);
  },
  NamedCallout({ node: n, parent: e, replaceWith: t }) {
    const { kind: s } = n;
    if (s === "fail")
      t(z(Je({ negate: !0 }), e));
    else
      throw new Error(`Unsupported named callout "(*${s.toUpperCase()}"`);
  },
  Quantifier({ node: n }) {
    if (n.body.type === "Quantifier") {
      const e = pe();
      e.body[0].body.push(n.body), n.body = z(e, n);
    }
  },
  Regex: {
    enter({ node: n }, { supportedGNodes: e }) {
      const t = [];
      let s = !1, r = !1;
      for (const o of n.body)
        if (o.body.length === 1 && o.body[0].kind === "search_start")
          o.body.pop();
        else {
          const i = Sl(o.body);
          i ? (s = !0, Array.isArray(i) ? t.push(...i) : t.push(i)) : r = !0;
        }
      s && !r && t.forEach((o) => e.add(o));
    },
    exit(n, { accuracy: e, passedLookbehind: t, strategy: s }) {
      if (e === "strict" && t && s)
        throw new Error(E`Uses "\G" in a way that requires non-strict accuracy`);
    }
  },
  Subroutine({ node: n }, { jsGroupNameMap: e }) {
    let { ref: t } = n;
    typeof t == "string" && !er(t) && (t = Ys(t, e), n.ref = t);
  }
}, qd = {
  Backreference({ node: n }, { multiplexCapturesToLeftByRef: e, reffedNodesByReferencer: t }) {
    const { orphan: s, ref: r } = n;
    s || t.set(n, [...e.get(r).map(({ node: o }) => o)]);
  },
  CapturingGroup: {
    enter({
      node: n,
      parent: e,
      replaceWith: t,
      skip: s
    }, {
      groupOriginByCopy: r,
      groupsByName: o,
      multiplexCapturesToLeftByRef: i,
      openRefs: a,
      reffedNodesByReferencer: l
    }) {
      const c = r.get(n);
      if (c && a.has(n.number)) {
        const u = Ne(Qi(n.number), e);
        l.set(u, a.get(n.number)), t(u);
        return;
      }
      a.set(n.number, n), i.set(n.number, []), n.name && pn(i, n.name, []);
      const h = i.get(n.name ?? n.number);
      for (let u = 0; u < h.length; u++) {
        const p = h[u];
        if (
          // This group is from subroutine expansion, and there's a multiplex value from either the
          // origin node or a prior subroutine expansion group with the same origin
          c === p.node || c && c === p.origin || // This group is not from subroutine expansion, and it comes after a subroutine expansion
          // group that refers to this group
          n === p.origin
        ) {
          h.splice(u, 1);
          break;
        }
      }
      if (i.get(n.number).push({ node: n, origin: c }), n.name && i.get(n.name).push({ node: n, origin: c }), n.name) {
        const u = pn(o, n.name, /* @__PURE__ */ new Map());
        let p = !1;
        if (c)
          p = !0;
        else
          for (const d of u.values())
            if (!d.hasDuplicateNameToRemove) {
              p = !0;
              break;
            }
        o.get(n.name).set(n, { node: n, hasDuplicateNameToRemove: p });
      }
    },
    exit({ node: n }, { openRefs: e }) {
      e.get(n.number) === n && e.delete(n.number);
    }
  },
  Group: {
    enter({ node: n }, e) {
      e.prevFlags = e.currentFlags, n.flags && (e.currentFlags = os(e.currentFlags, n.flags));
    },
    exit(n, e) {
      e.currentFlags = e.prevFlags;
    }
  },
  Subroutine({ node: n, parent: e, replaceWith: t }, s) {
    const { isRecursive: r, ref: o } = n;
    if (r) {
      let h = e;
      for (; (h = h.parent) && !(h.type === "CapturingGroup" && (h.name === o || h.number === o)); )
        ;
      s.reffedNodesByReferencer.set(n, h);
      return;
    }
    const i = s.subroutineRefMap.get(o), a = o === 0, l = a ? Qi(0) : (
      // The reffed group might itself contain subroutines, which are expanded during sub-traversal
      kl(i, s.groupOriginByCopy, null)
    );
    let c = l;
    if (!a) {
      const h = Cl(Vd(
        i,
        (p) => p.type === "Group" && !!p.flags
      )), u = h ? os(s.globalFlags, h) : s.globalFlags;
      Wd(u, s.currentFlags) || (c = pe({
        flags: Kd(u)
      }), c.body[0].body.push(l));
    }
    t(z(c, e), { traverse: !a });
  }
}, Hd = {
  Backreference({ node: n, parent: e, replaceWith: t }, s) {
    if (n.orphan) {
      s.highestOrphanBackref = Math.max(s.highestOrphanBackref, n.ref);
      return;
    }
    const o = s.reffedNodesByReferencer.get(n).filter((i) => Qd(i, n));
    if (!o.length)
      t(z(Je({ negate: !0 }), e));
    else if (o.length > 1) {
      const i = pe({
        atomic: !0,
        body: o.reverse().map((a) => lt({
          body: [kr(a.number)]
        }))
      });
      t(z(i, e));
    } else
      n.ref = o[0].number;
  },
  CapturingGroup({ node: n }, e) {
    n.number = ++e.numCapturesToLeft, n.name && e.groupsByName.get(n.name).get(n).hasDuplicateNameToRemove && delete n.name;
  },
  Regex: {
    exit({ node: n }, e) {
      const t = Math.max(e.highestOrphanBackref - e.numCapturesToLeft, 0);
      for (let s = 0; s < t; s++) {
        const r = pl();
        n.body.at(-1).body.push(r);
      }
    }
  },
  Subroutine({ node: n }, e) {
    !n.isRecursive || n.ref === 0 || (n.ref = e.reffedNodesByReferencer.get(n).number);
  }
};
function xl(n) {
  Jt(n, {
    "*"({ node: e, parent: t }) {
      e.parent = t;
    }
  });
}
function Wd(n, e) {
  return n.dotAll === e.dotAll && n.ignoreCase === e.ignoreCase;
}
function Qd(n, e) {
  let t = e;
  do {
    if (t.type === "Regex")
      return !1;
    if (t.type === "Alternative")
      continue;
    if (t === n)
      return !1;
    const s = $l(t.parent);
    for (const r of s) {
      if (r === t)
        break;
      if (r === n || Al(r, n))
        return !0;
    }
  } while (t = t.parent);
  throw new Error("Unexpected path");
}
function kl(n, e, t, s) {
  const r = Array.isArray(n) ? [] : {};
  for (const [o, i] of Object.entries(n))
    o === "parent" ? r.parent = Array.isArray(t) ? s : t : i && typeof i == "object" ? r[o] = kl(i, e, r, t) : (o === "type" && i === "CapturingGroup" && e.set(r, e.get(n) ?? n), r[o] = i);
  return r;
}
function Qi(n) {
  const e = fl(n);
  return e.isRecursive = !0, e;
}
function Vd(n, e) {
  const t = [];
  for (; n = n.parent; )
    (!e || e(n)) && t.push(n);
  return t;
}
function Ys(n, e) {
  if (e.has(n))
    return e.get(n);
  const t = `$${e.size}_${n.replace(/^[^$_\p{IDS}]|[^$\u200C\u200D\p{IDC}]/ug, "_")}`;
  return e.set(n, t), t;
}
function Cl(n) {
  const e = ["dotAll", "ignoreCase"], t = { enable: {}, disable: {} };
  return n.forEach(({ flags: s }) => {
    e.forEach((r) => {
      var o, i;
      (o = s.enable) != null && o[r] && (delete t.disable[r], t.enable[r] = !0), (i = s.disable) != null && i[r] && (t.disable[r] = !0);
    });
  }), Object.keys(t.enable).length || delete t.enable, Object.keys(t.disable).length || delete t.disable, t.enable || t.disable ? t : null;
}
function Kd({ dotAll: n, ignoreCase: e }) {
  const t = {};
  return (n || e) && (t.enable = {}, n && (t.enable.dotAll = !0), e && (t.enable.ignoreCase = !0)), (!n || !e) && (t.disable = {}, !n && (t.disable.dotAll = !0), !e && (t.disable.ignoreCase = !0)), t;
}
function $l(n) {
  if (!n)
    throw new Error("Node expected");
  const { body: e } = n;
  return Array.isArray(e) ? e : e ? [e] : null;
}
function Sl(n) {
  const e = n.find((t) => t.kind === "search_start" || Jd(t, { negate: !1 }) || !Zd(t));
  if (!e)
    return null;
  if (e.kind === "search_start")
    return e;
  if (e.type === "LookaroundAssertion")
    return e.body[0].body[0];
  if (e.type === "CapturingGroup" || e.type === "Group") {
    const t = [];
    for (const s of e.body) {
      const r = Sl(s.body);
      if (!r)
        return null;
      Array.isArray(r) ? t.push(...r) : t.push(r);
    }
    return t;
  }
  return null;
}
function Al(n, e) {
  const t = $l(n) ?? [];
  for (const s of t)
    if (s === e || Al(s, e))
      return !0;
  return !1;
}
function Zd({ type: n }) {
  return n === "Assertion" || n === "Directive" || n === "LookaroundAssertion";
}
function Xd(n) {
  const e = [
    "Character",
    "CharacterClass",
    "CharacterSet"
  ];
  return e.includes(n.type) || n.type === "Quantifier" && n.min && e.includes(n.body.type);
}
function Jd(n, e) {
  const t = {
    negate: null,
    ...e
  };
  return n.type === "LookaroundAssertion" && (t.negate === null || n.negate === t.negate) && n.body.length === 1 && ul(n.body[0], {
    type: "Assertion",
    kind: "search_start"
  });
}
function er(n) {
  return /^[$_\p{IDS}][$\u200C\u200D\p{IDC}]*$/u.test(n);
}
function Ce(n, e) {
  const s = hl(n, {
    ...e,
    // Providing a custom set of Unicode property names avoids converting some JS Unicode
    // properties (ex: `\p{Alpha}`) to Onig POSIX classes
    unicodePropertyMap: Zr
  }).body;
  return s.length > 1 || s[0].body.length > 1 ? pe({ body: s }) : s[0].body[0];
}
function tr(n, e) {
  return n.negate = e, n;
}
function Ne(n, e) {
  return n.parent = e, n;
}
function z(n, e) {
  return xl(n), n.parent = e, n;
}
function Yd(n, e) {
  const t = vl(e), s = Sr(t.target, "ES2024"), r = Sr(t.target, "ES2025"), o = t.rules.recursionLimit;
  if (!Number.isInteger(o) || o < 2 || o > 20)
    throw new Error("Invalid recursionLimit; use 2-20");
  let i = null, a = null;
  if (!r) {
    const d = [n.flags.ignoreCase];
    Jt(n, ef, {
      getCurrentModI: () => d.at(-1),
      popModI() {
        d.pop();
      },
      pushModI(f) {
        d.push(f);
      },
      setHasCasedChar() {
        d.at(-1) ? i = !0 : a = !0;
      }
    });
  }
  const l = {
    dotAll: n.flags.dotAll,
    // - Turn global flag i on if a case insensitive node was used and no case sensitive nodes were
    //   used (to avoid unnecessary node expansion).
    // - Turn global flag i off if a case sensitive node was used (since case sensitivity can't be
    //   forced without the use of ES2025 flag groups)
    ignoreCase: !!((n.flags.ignoreCase || i) && !a)
  };
  let c = n;
  const h = {
    accuracy: t.accuracy,
    appliedGlobalFlags: l,
    captureMap: /* @__PURE__ */ new Map(),
    currentFlags: {
      dotAll: n.flags.dotAll,
      ignoreCase: n.flags.ignoreCase
    },
    inCharClass: !1,
    lastNode: c,
    originMap: n._originMap,
    recursionLimit: o,
    useAppliedIgnoreCase: !!(!r && i && a),
    useFlagMods: r,
    useFlagV: s,
    verbose: t.verbose
  };
  function u(d) {
    return h.lastNode = c, c = d, Ld(tf[d.type], `Unexpected node type "${d.type}"`)(d, h, u);
  }
  const p = {
    pattern: n.body.map(u).join("|"),
    // Could reset `lastNode` at this point via `lastNode = ast`, but it isn't needed by flags
    flags: u(n.flags),
    options: { ...n.options }
  };
  return s || (delete p.options.force.v, p.options.disable.v = !0, p.options.unicodeSetsPlugin = null), p._captureTransfers = /* @__PURE__ */ new Map(), p._hiddenCaptures = [], h.captureMap.forEach((d, f) => {
    d.hidden && p._hiddenCaptures.push(f), d.transferTo && pn(p._captureTransfers, d.transferTo, []).push(f);
  }), p;
}
var ef = {
  "*": {
    enter({ node: n }, e) {
      if (Ki(n)) {
        const t = e.getCurrentModI();
        e.pushModI(
          n.flags ? os({ ignoreCase: t }, n.flags).ignoreCase : t
        );
      }
    },
    exit({ node: n }, e) {
      Ki(n) && e.popModI();
    }
  },
  Backreference(n, e) {
    e.setHasCasedChar();
  },
  Character({ node: n }, e) {
    Xr(D(n.value)) && e.setHasCasedChar();
  },
  CharacterClassRange({ node: n, skip: e }, t) {
    e(), El(n, { firstOnly: !0 }).length && t.setHasCasedChar();
  },
  CharacterSet({ node: n }, e) {
    n.kind === "property" && wl.has(n.value) && e.setHasCasedChar();
  }
}, tf = {
  /**
  @param {AlternativeNode} node
  */
  Alternative({ body: n }, e, t) {
    return n.map(t).join("");
  },
  /**
  @param {AssertionNode} node
  */
  Assertion({ kind: n, negate: e }) {
    if (n === "string_end")
      return "$";
    if (n === "string_start")
      return "^";
    if (n === "word_boundary")
      return e ? E`\B` : E`\b`;
    throw new Error(`Unexpected assertion kind "${n}"`);
  },
  /**
  @param {BackreferenceNode} node
  */
  Backreference({ ref: n }, e) {
    if (typeof n != "number")
      throw new Error("Unexpected named backref in transformed AST");
    if (!e.useFlagMods && e.accuracy === "strict" && e.currentFlags.ignoreCase && !e.captureMap.get(n).ignoreCase)
      throw new Error("Use of case-insensitive backref to case-sensitive group requires target ES2025 or non-strict accuracy");
    return "\\" + n;
  },
  /**
  @param {CapturingGroupNode} node
  */
  CapturingGroup(n, e, t) {
    const { body: s, name: r, number: o } = n, i = { ignoreCase: e.currentFlags.ignoreCase }, a = e.originMap.get(n);
    return a && (i.hidden = !0, o > a.number && (i.transferTo = a.number)), e.captureMap.set(o, i), `(${r ? `?<${r}>` : ""}${s.map(t).join("|")})`;
  },
  /**
  @param {CharacterNode} node
  */
  Character({ value: n }, e) {
    const t = D(n), s = _t(n, {
      escDigit: e.lastNode.type === "Backreference",
      inCharClass: e.inCharClass,
      useFlagV: e.useFlagV
    });
    if (s !== t)
      return s;
    if (e.useAppliedIgnoreCase && e.currentFlags.ignoreCase && Xr(t)) {
      const r = _l(t);
      return e.inCharClass ? r.join("") : r.length > 1 ? `[${r.join("")}]` : r[0];
    }
    return t;
  },
  /**
  @param {CharacterClassNode} node
  */
  CharacterClass(n, e, t) {
    const { kind: s, negate: r, parent: o } = n;
    let { body: i } = n;
    if (s === "intersection" && !e.useFlagV)
      throw new Error("Use of character class intersection requires min target ES2024");
    de.bugFlagVLiteralHyphenIsRange && e.useFlagV && i.some(Zi) && (i = [As(45), ...i.filter((c) => !Zi(c))]);
    const a = () => `[${r ? "^" : ""}${i.map(t).join(s === "intersection" ? "&&" : "")}]`;
    if (!e.inCharClass) {
      if (
        // Already established `kind !== 'intersection'` if `!state.useFlagV`; don't check again
        (!e.useFlagV || de.bugNestedClassIgnoresNegation) && !r
      ) {
        const h = i.filter(
          (u) => u.type === "CharacterClass" && u.kind === "union" && u.negate
        );
        if (h.length) {
          const u = pe(), p = u.body[0];
          return u.parent = o, p.parent = u, i = i.filter((d) => !h.includes(d)), n.body = i, i.length ? (n.parent = p, p.body.push(n)) : u.body.pop(), h.forEach((d) => {
            const f = lt({ body: [d] });
            d.parent = f, f.parent = u, u.body.push(f);
          }), t(u);
        }
      }
      e.inCharClass = !0;
      const c = a();
      return e.inCharClass = !1, c;
    }
    const l = i[0];
    if (
      // Already established that the parent is a char class via `inCharClass`; don't check again
      s === "union" && !r && l && // Allows many nested classes to work with `target` ES2018 which doesn't support nesting
      ((!e.useFlagV || !e.verbose) && o.kind === "union" && !(de.bugFlagVLiteralHyphenIsRange && e.useFlagV) || !e.verbose && o.kind === "intersection" && // JS doesn't allow intersection with union or ranges
      i.length === 1 && l.type !== "CharacterClassRange")
    )
      return i.map(t).join("");
    if (!e.useFlagV && o.type === "CharacterClass")
      throw new Error("Uses nested character class in a way that requires min target ES2024");
    return a();
  },
  /**
  @param {CharacterClassRangeNode} node
  */
  CharacterClassRange(n, e) {
    const t = n.min.value, s = n.max.value, r = {
      escDigit: !1,
      inCharClass: !0,
      useFlagV: e.useFlagV
    }, o = _t(t, r), i = _t(s, r), a = /* @__PURE__ */ new Set();
    if (e.useAppliedIgnoreCase && e.currentFlags.ignoreCase) {
      const l = El(n);
      af(l).forEach((h) => {
        a.add(
          Array.isArray(h) ? `${_t(h[0], r)}-${_t(h[1], r)}` : _t(h, r)
        );
      });
    }
    return `${o}-${i}${[...a].join("")}`;
  },
  /**
  @param {CharacterSetNode} node
  */
  CharacterSet({ kind: n, negate: e, value: t, key: s }, r) {
    if (n === "dot")
      return r.currentFlags.dotAll ? r.appliedGlobalFlags.dotAll || r.useFlagMods ? "." : "[^]" : (
        // Onig's only line break char is line feed, unlike JS
        E`[^\n]`
      );
    if (n === "digit")
      return e ? E`\D` : E`\d`;
    if (n === "property") {
      if (r.useAppliedIgnoreCase && r.currentFlags.ignoreCase && wl.has(t))
        throw new Error(`Unicode property "${t}" can't be case-insensitive when other chars have specific case`);
      return `${e ? E`\P` : E`\p`}{${s ? `${s}=` : ""}${t}}`;
    }
    if (n === "word")
      return e ? E`\W` : E`\w`;
    throw new Error(`Unexpected character set kind "${n}"`);
  },
  /**
  @param {FlagsNode} node
  */
  Flags(n, e) {
    return (
      // The transformer should never turn on the properties for flags d, g, m since Onig doesn't
      // have equivs. Flag m is never used since Onig uses different line break chars than JS
      // (node.hasIndices ? 'd' : '') +
      // (node.global ? 'g' : '') +
      // (node.multiline ? 'm' : '') +
      (e.appliedGlobalFlags.ignoreCase ? "i" : "") + (n.dotAll ? "s" : "") + (n.sticky ? "y" : "")
    );
  },
  /**
  @param {GroupNode} node
  */
  Group({ atomic: n, body: e, flags: t, parent: s }, r, o) {
    const i = r.currentFlags;
    t && (r.currentFlags = os(i, t));
    const a = e.map(o).join("|"), l = !r.verbose && e.length === 1 && // Single alt
    s.type !== "Quantifier" && !n && (!r.useFlagMods || !t) ? a : `(?${lf(n, t, r.useFlagMods)}${a})`;
    return r.currentFlags = i, l;
  },
  /**
  @param {LookaroundAssertionNode} node
  */
  LookaroundAssertion({ body: n, kind: e, negate: t }, s, r) {
    return `(?${`${e === "lookahead" ? "" : "<"}${t ? "!" : "="}`}${n.map(r).join("|")})`;
  },
  /**
  @param {QuantifierNode} node
  */
  Quantifier(n, e, t) {
    return t(n.body) + cf(n);
  },
  /**
  @param {SubroutineNode & {isRecursive: true}} node
  */
  Subroutine({ isRecursive: n, ref: e }, t) {
    if (!n)
      throw new Error("Unexpected non-recursive subroutine in transformed AST");
    const s = t.recursionLimit;
    return e === 0 ? `(?R=${s})` : E`\g<${e}&R=${s}>`;
  }
}, nf = /* @__PURE__ */ new Set([
  "$",
  "(",
  ")",
  "*",
  "+",
  ".",
  "?",
  "[",
  "\\",
  "]",
  "^",
  "{",
  "|",
  "}"
]), sf = /* @__PURE__ */ new Set([
  "-",
  "\\",
  "]",
  "^",
  // Literal `[` doesn't require escaping with flag u, but this can help work around regex source
  // linters and regex syntax processors that expect unescaped `[` to create a nested class
  "["
]), rf = /* @__PURE__ */ new Set([
  "(",
  ")",
  "-",
  "/",
  "[",
  "\\",
  "]",
  "^",
  "{",
  "|",
  "}",
  // Double punctuators; also includes already-listed `-` and `^`
  "!",
  "#",
  "$",
  "%",
  "&",
  "*",
  "+",
  ",",
  ".",
  ":",
  ";",
  "<",
  "=",
  ">",
  "?",
  "@",
  "`",
  "~"
]), Vi = /* @__PURE__ */ new Map([
  [9, E`\t`],
  // horizontal tab
  [10, E`\n`],
  // line feed
  [11, E`\v`],
  // vertical tab
  [12, E`\f`],
  // form feed
  [13, E`\r`],
  // carriage return
  [8232, E`\u2028`],
  // line separator
  [8233, E`\u2029`],
  // paragraph separator
  [65279, E`\uFEFF`]
  // ZWNBSP/BOM
]), of = new RegExp("^\\p{Cased}$", "u");
function Xr(n) {
  return of.test(n);
}
function El(n, e) {
  const t = !!(e != null && e.firstOnly), s = n.min.value, r = n.max.value, o = [];
  if (s < 65 && (r === 65535 || r >= 131071) || s === 65536 && r >= 131071)
    return o;
  for (let i = s; i <= r; i++) {
    const a = D(i);
    if (!Xr(a))
      continue;
    const l = _l(a).filter((c) => {
      const h = c.codePointAt(0);
      return h < s || h > r;
    });
    if (l.length && (o.push(...l), t))
      break;
  }
  return o;
}
function _t(n, { escDigit: e, inCharClass: t, useFlagV: s }) {
  if (Vi.has(n))
    return Vi.get(n);
  if (
    // Control chars, etc.; condition modeled on the Chrome developer console's display for strings
    n < 32 || n > 126 && n < 160 || // Unicode planes 4-16; unassigned, special purpose, and private use area
    n > 262143 || // Avoid corrupting a preceding backref by immediately following it with a literal digit
    e && uf(n)
  )
    return n > 255 ? `\\u{${n.toString(16).toUpperCase()}}` : `\\x${n.toString(16).toUpperCase().padStart(2, "0")}`;
  const r = t ? s ? rf : sf : nf, o = D(n);
  return (r.has(o) ? "\\" : "") + o;
}
function af(n) {
  const e = n.map((r) => r.codePointAt(0)).sort((r, o) => r - o), t = [];
  let s = null;
  for (let r = 0; r < e.length; r++)
    e[r + 1] === e[r] + 1 ? s ?? (s = e[r]) : s === null ? t.push(e[r]) : (t.push([s, e[r]]), s = null);
  return t;
}
function lf(n, e, t) {
  if (n)
    return ">";
  let s = "";
  if (e && t) {
    const { enable: r, disable: o } = e;
    s = (r != null && r.ignoreCase ? "i" : "") + (r != null && r.dotAll ? "s" : "") + (o ? "-" : "") + (o != null && o.ignoreCase ? "i" : "") + (o != null && o.dotAll ? "s" : "");
  }
  return `${s}:`;
}
function cf({ kind: n, max: e, min: t }) {
  let s;
  return !t && e === 1 ? s = "?" : !t && e === 1 / 0 ? s = "*" : t === 1 && e === 1 / 0 ? s = "+" : t === e ? s = `{${t}}` : s = `{${t},${e === 1 / 0 ? "" : e}}`, s + {
    greedy: "",
    lazy: "?",
    possessive: "+"
  }[n];
}
function Ki({ type: n }) {
  return n === "CapturingGroup" || n === "Group" || n === "LookaroundAssertion";
}
function uf(n) {
  return n > 47 && n < 58;
}
function Zi({ type: n, value: e }) {
  return n === "Character" && e === 45;
}
var ze, $e, nt, Be, st, yn, Ar, Ue, hf = (Ue = class extends RegExp {
  /**
  @overload
  @param {string} pattern
  @param {string} [flags]
  @param {EmulatedRegExpOptions} [options]
  */
  /**
  @overload
  @param {EmulatedRegExp} pattern
  @param {string} [flags]
  */
  constructor(t, s, r) {
    var e = (...Xm) => (super(...Xm), Pe(this, yn), /**
    @type {Map<number, {
      hidden?: true;
      transferTo?: number;
    }>}
    */
    Pe(this, ze, /* @__PURE__ */ new Map()), /**
    @type {RegExp | EmulatedRegExp | null}
    */
    Pe(this, $e, null), /**
    @type {string}
    */
    Pe(this, nt), /**
    @type {Map<number, string>?}
    */
    Pe(this, Be, null), /**
    @type {string?}
    */
    Pe(this, st, null), /**
    Can be used to serialize the instance.
    @type {EmulatedRegExpOptions}
    */
    g(this, "rawOptions", {}), this);
    const o = !!(r != null && r.lazyCompile);
    if (t instanceof RegExp) {
      if (r)
        throw new Error("Cannot provide options when copying a regexp");
      const i = t;
      e(i, s), he(this, nt, i.source), i instanceof Ue && (he(this, ze, V(i, ze)), he(this, Be, V(i, Be)), he(this, st, V(i, st)), this.rawOptions = i.rawOptions);
    } else {
      const i = {
        hiddenCaptures: [],
        strategy: null,
        transfers: [],
        ...r
      };
      e(o ? "" : t, s), he(this, nt, t), he(this, ze, df(i.hiddenCaptures, i.transfers)), he(this, st, i.strategy), this.rawOptions = r ?? {};
    }
    o || he(this, $e, this);
  }
  // Override the getter with one that works with lazy-compiled regexes
  get source() {
    return V(this, nt) || "(?:)";
  }
  /**
  Called internally by all String/RegExp methods that use regexes.
  @override
  @param {string} str
  @returns {RegExpExecArray?}
  */
  exec(t) {
    if (!V(this, $e)) {
      const { lazyCompile: o, ...i } = this.rawOptions;
      he(this, $e, new Ue(V(this, nt), this.flags, i));
    }
    const s = this.global || this.sticky, r = this.lastIndex;
    if (V(this, st) === "clip_search" && s && r) {
      this.lastIndex = 0;
      const o = yt(this, yn, Ar).call(this, t.slice(r));
      return o && (pf(o, r, t, this.hasIndices), this.lastIndex += r), o;
    }
    return yt(this, yn, Ar).call(this, t);
  }
}, ze = new WeakMap(), $e = new WeakMap(), nt = new WeakMap(), Be = new WeakMap(), st = new WeakMap(), yn = new WeakSet(), /**
Adds support for hidden and transfer captures.
@param {string} str
@returns
*/
Ar = function(t) {
  V(this, $e).lastIndex = this.lastIndex;
  const s = Ko(Ue.prototype, this, "exec").call(V(this, $e), t);
  if (this.lastIndex = V(this, $e).lastIndex, !s || !V(this, ze).size)
    return s;
  const r = [...s];
  s.length = 1;
  let o;
  this.hasIndices && (o = [...s.indices], s.indices.length = 1);
  const i = [0];
  for (let a = 1; a < r.length; a++) {
    const { hidden: l, transferTo: c } = V(this, ze).get(a) ?? {};
    if (l ? i.push(null) : (i.push(s.length), s.push(r[a]), this.hasIndices && s.indices.push(o[a])), c && r[a] !== void 0) {
      const h = i[c];
      if (!h)
        throw new Error(`Invalid capture transfer to "${h}"`);
      if (s[h] = r[a], this.hasIndices && (s.indices[h] = o[a]), s.groups) {
        V(this, Be) || he(this, Be, ff(this.source));
        const u = V(this, Be).get(c);
        u && (s.groups[u] = r[a], this.hasIndices && (s.indices.groups[u] = o[a]));
      }
    }
  }
  return s;
}, Ue);
function pf(n, e, t, s) {
  if (n.index += e, n.input = t, s) {
    const r = n.indices;
    for (let i = 0; i < r.length; i++) {
      const a = r[i];
      a && (r[i] = [a[0] + e, a[1] + e]);
    }
    const o = r.groups;
    o && Object.keys(o).forEach((i) => {
      const a = o[i];
      a && (o[i] = [a[0] + e, a[1] + e]);
    });
  }
}
function df(n, e) {
  const t = /* @__PURE__ */ new Map();
  for (const s of n)
    t.set(s, {
      hidden: !0
    });
  for (const [s, r] of e)
    for (const o of r)
      pn(t, o, {}).transferTo = s;
  return t;
}
function ff(n) {
  const e = /(?<capture>\((?:\?<(?![=!])(?<name>[^>]+)>|(?!\?)))|\\?./gsu, t = /* @__PURE__ */ new Map();
  let s = 0, r = 0, o;
  for (; o = e.exec(n); ) {
    const { 0: i, groups: { capture: a, name: l } } = o;
    i === "[" ? s++ : s ? i === "]" && s-- : a && (r++, l && t.set(r, l));
  }
  return t;
}
function gf(n, e) {
  const t = mf(n, e);
  return t.options ? new hf(t.pattern, t.flags, t.options) : new RegExp(t.pattern, t.flags);
}
function mf(n, e) {
  const t = vl(e), s = hl(n, {
    flags: t.flags,
    normalizeUnknownPropertyNames: !0,
    rules: {
      captureGroup: t.rules.captureGroup,
      singleline: t.rules.singleline
    },
    skipBackrefValidation: t.rules.allowOrphanBackrefs,
    unicodePropertyMap: Zr
  }), r = Ud(s, {
    accuracy: t.accuracy,
    asciiWordBoundaries: t.rules.asciiWordBoundaries,
    avoidSubclass: t.avoidSubclass,
    bestEffortTarget: t.target
  }), o = Yd(r, t), i = Id(o.pattern, {
    captureTransfers: o._captureTransfers,
    hiddenCaptures: o._hiddenCaptures,
    mode: "external"
  }), a = Rd(i.pattern), l = Ed(a.pattern, {
    captureTransfers: i.captureTransfers,
    hiddenCaptures: i.hiddenCaptures
  }), c = {
    pattern: l.pattern,
    flags: `${t.hasIndices ? "d" : ""}${t.global ? "g" : ""}${o.flags}${o.options.disable.v ? "u" : "v"}`
  };
  if (t.avoidSubclass) {
    if (t.lazyCompileLength !== 1 / 0)
      throw new Error("Lazy compilation requires subclass");
  } else {
    const h = l.hiddenCaptures.sort((f, b) => f - b), u = Array.from(l.captureTransfers), p = r._strategy, d = c.pattern.length >= t.lazyCompileLength;
    (h.length || u.length || p || d) && (c.options = {
      ...h.length && { hiddenCaptures: h },
      ...u.length && { transfers: u },
      ...p && { strategy: p },
      ...d && { lazyCompile: d }
    });
  }
  return c;
}
function bf(n, e) {
  return gf(n, {
    global: !0,
    hasIndices: !0,
    lazyCompileLength: 3e3,
    rules: {
      allowOrphanBackrefs: !0,
      asciiWordBoundaries: !0,
      captureGroup: !0,
      recursionLimit: 5,
      singleline: !0
    },
    ...e
  });
}
function yf(n = {}) {
  const e = {
    target: "auto",
    cache: /* @__PURE__ */ new Map(),
    ...n
  };
  return e.regexConstructor || (e.regexConstructor = (t) => bf(t, { target: e.target })), {
    createScanner(t) {
      return new Pp(t, e);
    },
    createString(t) {
      return { content: t };
    }
  };
}
const vf = "https://raw.githubusercontent.com/martinring/tmlanguage/master/tmlanguage.json", _f = "Sema", wf = "source.sema", xf = [{ include: "#block-comment" }, { include: "#comment" }, { include: "#string" }, { include: "#quoted-expression" }, { include: "#definition" }, { include: "#special-form" }, { include: "#threading-macro" }, { include: "#builtin" }, { include: "#operator" }, { include: "#keyword-literal" }, { include: "#character" }, { include: "#boolean" }, { include: "#nil" }, { include: "#number" }, { include: "#paren" }], kf = /* @__PURE__ */ JSON.parse('{"block-comment":{"name":"comment.block.sema","begin":"#\\\\|","end":"\\\\|#","patterns":[{"include":"#block-comment"}]},"comment":{"name":"comment.line.semicolon.sema","match":";.*$"},"string":{"name":"string.quoted.double.sema","begin":"\\"","end":"\\"","patterns":[{"name":"constant.character.escape.sema","match":"\\\\\\\\(?:[\\\\\\\\\\"nrt0]|x[0-9a-fA-F]+;|u[0-9a-fA-F]{4}|U[0-9a-fA-F]{8})"}]},"keyword-literal":{"name":"constant.other.keyword.sema","match":"(?<=[(\\\\[\\\\s,{]):[\\\\w!$%&*+\\\\-./:<=>?@^~][\\\\w!$%&*+\\\\-./:<=>?@^~0-9]*|^:[\\\\w!$%&*+\\\\-./:<=>?@^~][\\\\w!$%&*+\\\\-./:<=>?@^~0-9]*"},"character":{"patterns":[{"name":"constant.character.named.sema","match":"(?<=[\\\\s(\\\\[{])#\\\\\\\\(?:space|newline|tab|return|nul|alarm|backspace|delete|escape)(?=[\\\\s)\\\\]}]|$)|^#\\\\\\\\(?:space|newline|tab|return|nul|alarm|backspace|delete|escape)(?=[\\\\s)\\\\]}]|$)"},{"name":"constant.character.sema","match":"(?<=[\\\\s(\\\\[{])#\\\\\\\\.(?=[\\\\s)\\\\]}]|$)|^#\\\\\\\\.(?=[\\\\s)\\\\]}]|$)"}]},"boolean":{"name":"constant.language.boolean.sema","match":"(?<=[\\\\s(\\\\[{])(?:#t|#f|true|false)(?=[\\\\s)\\\\]}]|$)|^(?:#t|#f|true|false)(?=[\\\\s)\\\\]}]|$)"},"nil":{"name":"constant.language.nil.sema","match":"(?<=[\\\\s(\\\\[{])nil(?=[\\\\s)\\\\]}]|$)|^nil(?=[\\\\s)\\\\]}]|$)"},"number":{"name":"constant.numeric.sema","match":"(?<=[\\\\s(\\\\[{])-?(?:0[xX][0-9a-fA-F]+|0[bB][01]+|0[oO][0-7]+|[0-9]+(?:\\\\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)(?=[\\\\s)\\\\]}]|$)|^-?(?:0[xX][0-9a-fA-F]+|0[bB][01]+|0[oO][0-7]+|[0-9]+(?:\\\\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)(?=[\\\\s)\\\\]}]|$)"},"definition":{"patterns":[{"comment":"define with parens: (define (name args...) ...) — must come before simple define","match":"(\\\\()\\\\s*(define)\\\\s+(\\\\()\\\\s*([\\\\w!$%&*+\\\\-./:<=>?@^~][\\\\w!$%&*+\\\\-./:<=>?@^~0-9]*)","captures":{"1":{"name":"punctuation.paren.open.sema"},"2":{"name":"keyword.control.definition.sema"},"3":{"name":"punctuation.paren.open.sema"},"4":{"name":"entity.name.function.sema"}}},{"comment":"define with a simple name: (define name ...)","match":"(\\\\()\\\\s*(define)\\\\s+([\\\\w!$%&*+\\\\-./:<=>?@^~][\\\\w!$%&*+\\\\-./:<=>?@^~0-9]*)","captures":{"1":{"name":"punctuation.paren.open.sema"},"2":{"name":"keyword.control.definition.sema"},"3":{"name":"variable.other.sema"}}},{"comment":"defun/defmacro/defmulti/defmethod/defagent/deftool name","match":"(\\\\()\\\\s*(defun|defmacro|defmulti|defmethod|defagent|deftool|define-record-type)\\\\s+([\\\\w!$%&*+\\\\-./:<=>?@^~][\\\\w!$%&*+\\\\-./:<=>?@^~0-9]*)","captures":{"1":{"name":"punctuation.paren.open.sema"},"2":{"name":"keyword.control.definition.sema"},"3":{"name":"entity.name.function.sema"}}},{"comment":"set! target: (set! name value)","match":"(\\\\()\\\\s*(set!)\\\\s+([\\\\w!$%&*+\\\\-./:<=>?@^~][\\\\w!$%&*+\\\\-./:<=>?@^~0-9]*)","captures":{"1":{"name":"punctuation.paren.open.sema"},"2":{"name":"keyword.control.sema"},"3":{"name":"variable.other.sema"}}}]},"special-form":{"match":"(?<=[\\\\s(\\\\[{])(?:define|defun|lambda|fn|if|if-let|cond|case|when|when-let|unless|let\\\\*?|letrec|begin|do|and|or|set!|quote|quasiquote|unquote-splicing|unquote|define-record-type|defmacro|defmethod|defmulti|defagent|deftool|try|catch|throw|import|module|export|load|delay|force|eval|macroexpand|else|prompt|message|match)(?=[\\\\s)\\\\]}]|$)","name":"keyword.control.sema"},"operator":{"match":"(?<=[\\\\s(\\\\[{])(?:\\\\+|\\\\*|/|%|>=|<=|>(?!=)|<(?!=)|=|-|eqv\\\\?)(?=[\\\\s)\\\\]}]|$)","name":"keyword.operator.arithmetic.sema"},"threading-macro":{"match":"(?<=[\\\\s(\\\\[{])(?:->>?|as->|some->)(?=[\\\\s)\\\\]}]|$)","name":"keyword.operator.threading.sema"},"builtin":{"patterns":[{"comment":"Higher-order and core functions","match":"(?<=[\\\\s(\\\\[{])(?:map|filter|foldl|foldr|reduce|for-each|apply|gensym|not|error|assert|assert=|deep-merge|get-in|assoc-in|update-in|tap|spy|pprint|retry|time)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"List functions","match":"(?<=[\\\\s(\\\\[{])(?:list|cons|car|cdr|first|rest|nth|append|reverse|length|null\\\\?|list\\\\?|member|vector|sort|sort-by|take|drop|zip|flatten|range|make-list|flat-map|take-while|drop-while|every|any|partition|last|iota)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"ca*r/cd*r variants","match":"(?<=[\\\\s(\\\\[{])(?:caar|cadr|cdar|cddr|caaar|caadr|cadar|caddr|cdaar|cdadr|cddar|cdddr)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"list/* namespaced functions","match":"(?<=[\\\\s(\\\\[{])(?:list/chunk|list/dedupe|list/drop-while|list/group-by|list/index-of|list/interleave|list/max|list/min|list/pick|list/repeat|list/shuffle|list/split-at|list/sum|list/take-while|list/unique|list->bytevector|list->string|list->vector|list/avg|list/cross-join|list/diff|list/duplicates|list/find|list/intersect|list/join|list/key-by|list/median|list/mode|list/pad|list/page|list/pluck|list/reject|list/sliding|list/sole|list/times)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Additional list functions","match":"(?<=[\\\\s(\\\\[{])(?:assoc|assq|assv|flatten-deep|frequencies|interpose|vector->list)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Map functions","match":"(?<=[\\\\s(\\\\[{])(?:hash-map|get|dissoc|merge|keys|vals|contains\\\\?|count|empty\\\\?)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"map/* namespaced functions","match":"(?<=[\\\\s(\\\\[{])(?:map/entries|map/filter|map/from-entries|map/map-keys|map/map-vals|map/select-keys|map/update|map/except|map/sort-keys|map/zip)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"hashmap/* functions","match":"(?<=[\\\\s(\\\\[{])(?:hashmap/new|hashmap/get|hashmap/assoc|hashmap/keys|hashmap/contains\\\\?|hashmap/to-map)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"String functions","match":"(?<=[\\\\s(\\\\[{])(?:string-append|string/join|string/split|string/trim|string/upper|string/lower|string/replace|string/contains\\\\?|string/starts-with\\\\?|string/ends-with\\\\?|string/capitalize|string/empty\\\\?|string/index-of|string/reverse|string/repeat|string/pad-left|string/pad-right|str|substring|string-length|string-ref|string/byte-length|string/chars|string/codepoints|string/foldcase|string/from-codepoints|string/last-index-of|string/map|string/normalize|string/number\\\\?|string/title-case|string/trim-left|string/trim-right|string-ci=\\\\?|string/after|string/after-last|string/before|string/before-last|string/between|string/camel-case|string/chop-end|string/chop-start|string/ensure-end|string/ensure-start|string/headline|string/intern|string/kebab-case|string/pascal-case|string/remove|string/replace-first|string/replace-last|string/snake-case|string/take|string/unwrap|string/words|string/wrap)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"String conversions","match":"(?<=[\\\\s(\\\\[{])(?:string->char|string->float|string->list|string->utf8)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Char functions","match":"(?<=[\\\\s(\\\\[{])(?:char->integer|char->string|integer->char|char-alphabetic\\\\?|char-ci<\\\\?|char-ci<=\\\\?|char-ci=\\\\?|char-ci>\\\\?|char-ci>=\\\\?|char-downcase|char-lower-case\\\\?|char-numeric\\\\?|char-upcase|char-upper-case\\\\?|char-whitespace\\\\?|char<\\\\?|char<=\\\\?|char=\\\\?|char>\\\\?|char>=\\\\?)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"IO and display","match":"(?<=[\\\\s(\\\\[{])(?:display|print|println|newline|format|read|read-line|read-many|print-error|println-error|read-stdin)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Type predicates","match":"(?<=[\\\\s(\\\\[{])(?:number\\\\?|string\\\\?|symbol\\\\?|pair\\\\?|boolean\\\\?|procedure\\\\?|integer\\\\?|float\\\\?|keyword\\\\?|nil\\\\?|fn\\\\?|map\\\\?|record\\\\?|equal\\\\?|eq\\\\?|zero\\\\?|positive\\\\?|negative\\\\?|even\\\\?|odd\\\\?|type|bool\\\\?|bytevector\\\\?|char\\\\?|vector\\\\?|promise\\\\?|agent\\\\?|conversation\\\\?|message\\\\?|prompt\\\\?|tool\\\\?|promise-forced\\\\?)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Type conversions","match":"(?<=[\\\\s(\\\\[{])(?:string->number|number->string|string->symbol|symbol->string|string->keyword|keyword->string)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Math functions and constants","match":"(?<=[\\\\s(\\\\[{])(?:abs|min|max|round|floor|ceiling|sqrt|expt|pow|log|sin|cos|ceil|int|float|truncate|mod|modulo|math/remainder|math/gcd|math/lcm|math/pow|math/tan|math/random|math/random-int|math/clamp|math/sign|math/exp|math/log10|math/log2|math/acos|math/asin|math/atan|math/atan2|math/cosh|math/degrees->radians|math/infinite\\\\?|math/lerp|math/map-range|math/nan\\\\?|math/quotient|math/radians->degrees|math/sinh|math/tanh|math/infinity|math/nan|pi|e)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"File functions","match":"(?<=[\\\\s(\\\\[{])(?:file/read|file/write|file/exists\\\\?|file/delete|file/list|file/append|file/rename|file/mkdir|file/info|file/read-lines|file/write-lines|file/is-directory\\\\?|file/is-file\\\\?|file/copy|file/fold-lines|file/for-each-line|file/is-symlink\\\\?|file/glob|file/read-bytes|file/write-bytes)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Path functions","match":"(?<=[\\\\s(\\\\[{])(?:path/absolute|path/absolute\\\\?|path/basename|path/dir|path/dirname|path/ext|path/extension|path/filename|path/join|path/stem)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"PDF functions","match":"(?<=[\\\\s(\\\\[{])(?:pdf/extract-text|pdf/extract-text-pages|pdf/metadata|pdf/page-count)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"JSON and TOML functions","match":"(?<=[\\\\s(\\\\[{])(?:json/decode|json/encode|json/encode-pretty|toml/decode|toml/encode)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"HTTP client functions","match":"(?<=[\\\\s(\\\\[{])(?:http/get|http/post|http/put|http/delete|http/request)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"HTTP server functions","match":"(?<=[\\\\s(\\\\[{])(?:http/serve|http/router|http/router/dispatch|http/file|http/ok|http/created|http/no-content|http/not-found|http/error|http/redirect|http/html|http/text|http/stream|http/stream/send|http/websocket)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"WebSocket functions","match":"(?<=[\\\\s(\\\\[{])(?:ws/send|ws/recv|ws/close)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Regex functions","match":"(?<=[\\\\s(\\\\[{])(?:regex/match\\\\?|regex/match|regex/find-all|regex/replace|regex/replace-all|regex/split)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Crypto and encoding functions","match":"(?<=[\\\\s(\\\\[{])(?:uuid/v4|base64/encode|base64/decode|base64/encode-bytes|base64/decode-bytes|hash/md5|hash/sha256|hash/hmac-sha256)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"DateTime functions","match":"(?<=[\\\\s(\\\\[{])(?:time/now|time/format|time/parse|time/date-parts|time/add|time/diff|time-ms)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"CSV functions","match":"(?<=[\\\\s(\\\\[{])(?:csv/parse|csv/parse-maps|csv/encode)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Bitwise functions","match":"(?<=[\\\\s(\\\\[{])(?:bit/and|bit/or|bit/xor|bit/not|bit/shift-left|bit/shift-right)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Terminal functions","match":"(?<=[\\\\s(\\\\[{])(?:term/style|term/strip|term/rgb|term/spinner-start|term/spinner-stop|term/spinner-update|term/black|term/blue|term/bold|term/cyan|term/dim|term/gray|term/green|term/inverse|term/italic|term/magenta|term/red|term/strikethrough|term/underline|term/white|term/yellow)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Bytevector functions","match":"(?<=[\\\\s(\\\\[{])(?:bytevector|make-bytevector|bytevector-length|bytevector-u8-ref|bytevector-u8-set!|bytevector-copy|bytevector-append|bytevector->list|utf8->string)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"System functions","match":"(?<=[\\\\s(\\\\[{])(?:env|shell|exit|sleep|sys/args|sys/cwd|sys/platform|sys/set-env|sys/env-all|sys/arch|sys/elapsed|sys/home-dir|sys/hostname|sys/interactive\\\\?|sys/os|sys/pid|sys/temp-dir|sys/tty|sys/user|sys/which|sys/interner-stats|sys/sema-home)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Context functions","match":"(?<=[\\\\s(\\\\[{])(?:context/get|context/set|context/has\\\\?|context/remove|context/all|context/clear|context/merge|context/with|context/push|context/pop|context/pull|context/stack|context/get-hidden|context/set-hidden|context/has-hidden\\\\?)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Key-value store functions","match":"(?<=[\\\\s(\\\\[{])(?:kv/open|kv/close|kv/get|kv/set|kv/delete|kv/keys)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Text processing functions","match":"(?<=[\\\\s(\\\\[{])(?:text/chunk|text/chunk-by-separator|text/clean-whitespace|text/excerpt|text/normalize-newlines|text/split-sentences|text/strip-html|text/trim-indent|text/truncate|text/word-count)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Document functions","match":"(?<=[\\\\s(\\\\[{])(?:document/create|document/chunk|document/text|document/metadata)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Log functions","match":"(?<=[\\\\s(\\\\[{])(?:log/debug|log/info|log/warn|log/error)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Prompt template functions","match":"(?<=[\\\\s(\\\\[{])(?:prompt/render|prompt/template|prompt/concat|prompt/fill|prompt/slots)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"LLM functions","match":"(?<=[\\\\s(\\\\[{])(?:conversation/new|conversation/say|conversation/say-as|conversation/messages|conversation/last-reply|conversation/fork|conversation/add-message|conversation/model|conversation/set-system|conversation/system|conversation/cost|conversation/filter|conversation/map|conversation/token-count|llm/complete|llm/chat|llm/stream|llm/send|llm/extract|llm/extract-from-image|llm/classify|llm/batch|llm/pmap|llm/embed|llm/auto-configure|llm/configure|llm/set-budget|llm/budget-remaining|llm/define-provider|llm/last-usage|llm/session-usage|llm/similarity|llm/clear-budget|llm/configure-embeddings|llm/current-provider|llm/default-provider|llm/list-providers|llm/providers|llm/pricing-status|llm/reset-usage|llm/set-default|llm/set-pricing|llm/compare|llm/summarize|llm/token-count|llm/token-estimate|llm/with-budget|llm/with-cache|llm/with-fallback|llm/with-rate-limit|llm/cache-clear|llm/cache-key|llm/cache-stats|prompt/append|prompt/messages|prompt/set-system|message/role|message/content|message/with-image|agent/run|agent/max-turns|agent/model|agent/name|agent/system|agent/tools)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Embedding functions","match":"(?<=[\\\\s(\\\\[{])(?:embedding/->list|embedding/length|embedding/list->embedding|embedding/ref)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Vector math functions","match":"(?<=[\\\\s(\\\\[{])(?:vector/cosine-similarity|vector/distance|vector/dot-product|vector/normalize)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Vector store functions","match":"(?<=[\\\\s(\\\\[{])(?:vector-store/create|vector-store/open|vector-store/save|vector-store/add|vector-store/search|vector-store/count|vector-store/delete)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"},{"comment":"Tool query functions","match":"(?<=[\\\\s(\\\\[{])(?:tool/name|tool/description|tool/parameters)(?=[\\\\s)\\\\]}]|$)","name":"support.function.sema"}]},"quoted-expression":{"patterns":[{"name":"keyword.operator.quote.sema","match":"(?<=[\\\\s(\\\\[{])\'|^\'"},{"name":"keyword.operator.quasiquote.sema","match":"(?<=[\\\\s(\\\\[{])`|^`"},{"name":"keyword.operator.unquote-splicing.sema","match":"(?<=[\\\\s(\\\\[{]),@|^,@"},{"name":"keyword.operator.unquote.sema","match":"(?<=[\\\\s(\\\\[{]),|^,"}]},"paren":{"patterns":[{"match":"[()\\\\[\\\\]{}]","name":"punctuation.bracket.sema"}]}}'), Cf = {
  $schema: vf,
  name: _f,
  scopeName: wf,
  patterns: xf,
  repository: kf
}, $f = { ...Cf, name: "sema" }, Rl = {
  json: () => import("./json-DxJze_jm.js"),
  shellscript: () => import("./shellscript-InADTalH.js"),
  javascript: () => import("./javascript-C25yR2R2.js"),
  typescript: () => import("./typescript-RycA9KXf.js"),
  html: () => import("./html-CPZ3oZQ7.js"),
  css: () => import("./css-M7EaDHN_.js"),
  toml: () => import("./toml-DY62mUL_.js"),
  markdown: () => import("./markdown-CrScaQ96.js"),
  rust: () => import("./rust-CLzF9zIN.js"),
  yaml: () => import("./yaml-DaO7k5B1.js"),
  diff: () => import("./diff-BxzP2J8R.js")
}, Sf = {
  sh: "shellscript",
  bash: "shellscript",
  shell: "shellscript",
  zsh: "shellscript",
  js: "javascript",
  ts: "typescript",
  md: "markdown",
  rs: "rust",
  yml: "yaml"
}, is = /* @__PURE__ */ new Map(), Jr = /* @__PURE__ */ new Map();
function Cn(n) {
  return Sf[n] ?? n;
}
function Ft(n) {
  const e = Cn(n);
  return e === "sema" || e in Rl || is.has(e) || Jr.has(e);
}
function Af(n, e) {
  typeof n == "string" ? e && Jr.set(n, e) : is.set(n.name, n);
}
const Ef = [
  ["constant.character.escape", "tok-escape"],
  ["constant.numeric", "tok-number"],
  ["constant.language", "tok-boolean"],
  ["constant.character", "tok-boolean"],
  ["constant.other.keyword", "tok-keyword-lit"],
  ["support.type.property-name", "tok-property"],
  // JSON object keys
  ["meta.object-literal.key", "tok-property"],
  // JS/TS object keys
  ["entity.name.tag", "tok-keyword"],
  // HTML/XML tags
  ["entity.other.attribute-name", "tok-property"],
  // HTML/XML attributes
  ["comment", "tok-comment"],
  ["string", "tok-string"],
  ["keyword.operator", "tok-operator"],
  ["keyword", "tok-keyword"],
  ["entity.name.function", "tok-function"],
  ["support.function", "tok-builtin"],
  ["variable", "tok-variable"],
  ["punctuation.definition.comment", "tok-comment"],
  // the `#`/`//` delimiter is part of the comment
  ["punctuation", "tok-punctuation"]
];
function Tl(n) {
  for (const [e, t] of Ef)
    if (n === e || n.startsWith(e + ".")) return t;
  return null;
}
function K(n) {
  return n.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
const Rf = {
  name: "sema-noop",
  settings: [{ settings: { foreground: "#d8d0c0", background: "#0a0a0a" } }]
};
let nr = null, as = null;
function Is() {
  return nr || (nr = Ip({
    themes: [Rf],
    langs: [$f],
    engine: yf({ forgiving: !0 })
  })), nr;
}
const dn = /* @__PURE__ */ new Set(["sema"]), sr = /* @__PURE__ */ new Map();
async function Yr(n) {
  if (dn.has(n)) return;
  const e = sr.get(n);
  if (e) return e;
  const t = (async () => {
    const s = await Is();
    if (is.has(n))
      await s.loadLanguage(is.get(n));
    else {
      const r = Jr.get(n) ?? Rl[n];
      if (!r) return;
      const o = await r(), i = o && typeof o == "object" && "default" in o ? o.default : o;
      await s.loadLanguage(i);
    }
    dn.add(n);
  })();
  sr.set(n, t);
  try {
    await t;
  } finally {
    sr.delete(n);
  }
}
function Il(n, e, t) {
  return n.codeToTokensBase(e, { lang: t, theme: "sema-noop", includeExplanation: !0 }).map(
    (r) => r.map((o) => (o.explanation ?? [{ content: o.content, scopes: [] }]).map((a) => {
      var u;
      const l = ((u = a.scopes[a.scopes.length - 1]) == null ? void 0 : u.scopeName) ?? "", c = Tl(l), h = K(a.content);
      return c ? `<span class="${c}">${h}</span>` : h;
    }).join("")).join("")
  ).join(`
`);
}
async function Tf(n) {
  const e = Cn(n);
  as = await Is(), Ft(e) && await Yr(e);
}
function If(n, e) {
  const t = Cn(e);
  if (!as || !dn.has(t)) return K(n);
  try {
    return Il(as, n, t);
  } catch {
    return K(n);
  }
}
async function Pl(n, e) {
  const t = Cn(e);
  if (!Ft(t) || (await Yr(t), !dn.has(t))) return K(n);
  const s = await Is();
  as = s;
  try {
    return Il(s, n, t);
  } catch {
    return K(n);
  }
}
async function Pf(n, e) {
  const t = () => n ? [{ text: n, cls: "" }] : [], s = Cn(e);
  if (!Ft(s) || (await Yr(s), !dn.has(s))) return t();
  const r = await Is();
  let o;
  try {
    o = r.codeToTokensBase(n, { lang: s, theme: "sema-noop", includeExplanation: !0 });
  } catch {
    return t();
  }
  const i = [];
  return o.forEach((a, l) => {
    var c;
    l > 0 && i.push({ text: `
`, cls: "" });
    for (const h of a) {
      const u = h.explanation ?? [{ content: h.content, scopes: [] }];
      for (const p of u) {
        if (!p.content) continue;
        const d = ((c = p.scopes[p.scopes.length - 1]) == null ? void 0 : c.scopeName) ?? "";
        i.push({ text: p.content, cls: Tl(d) ?? "" });
      }
    }
  }), i;
}
class Ll {
  constructor(e, t, s = 1500) {
    this._timer = null, this.copied = !1, this.copy = async () => {
      try {
        await navigator.clipboard.writeText(this._getText());
      } catch {
        return;
      }
      this.copied = !0, this._host.requestUpdate(), this._clear(), this._timer = setTimeout(() => {
        this.copied = !1, this._timer = null, this._host.requestUpdate();
      }, this._resetMs);
    }, this._host = e, this._getText = t, this._resetMs = s, e.addController(this);
  }
  hostDisconnected() {
    this._clear();
  }
  _clear() {
    this._timer && (clearTimeout(this._timer), this._timer = null);
  }
}
const $n = ".tok-comment{color:var(--syntax-comment, #5a5448);font-style:italic}.tok-keyword{color:var(--syntax-keyword, #c8a855)}.tok-string{color:var(--syntax-string, #a8c47a)}.tok-number{color:var(--syntax-number, #d19a66)}.tok-boolean{color:var(--syntax-boolean, #d19a66)}.tok-keyword-lit{color:var(--syntax-keyword-lit, #7aacb8)}.tok-builtin{color:var(--syntax-builtin, #88a8b8)}.tok-function{color:var(--syntax-function, #88a8b8)}.tok-variable{color:var(--syntax-variable, #b898c8)}.tok-punctuation{color:var(--syntax-punctuation, #6a6258)}.tok-operator{color:var(--syntax-operator, #c8a855)}.tok-escape{color:var(--syntax-escape, #c8d8a8)}.tok-property{color:var(--syntax-property, #7aacb8)}", Nl = '.copy{position:absolute;z-index:2;top:var(--space-sm, 8px);right:var(--space-sm, 8px);font-family:var(--mono, "JetBrains Mono", monospace);font-size:var(--text-xxs, 10px);letter-spacing:.04em;padding:5px 10px;border:1px solid var(--border, #1e1e1e);border-radius:var(--radius-sm, 3px);background:var(--bg-elevated, #141414);color:var(--text-tertiary, #5a5448);cursor:pointer;opacity:0;transition:opacity .15s,color .15s,border-color .15s}.wrap:hover .copy,.copy:focus-visible{opacity:1}.copy:hover{color:var(--gold, #c8a855);border-color:var(--gold-dim, rgba(200, 168, 85, .5))}.copy:focus-visible{outline:var(--focus-ring-width, 1px) solid var(--focus-ring-color-subtle, rgba(200, 168, 85, .5));outline-offset:var(--focus-ring-offset, 1px)}.copy.copied{color:var(--success, #6a9955);border-color:var(--success, #6a9955);opacity:1}', Sn = ".sema-scroll{scrollbar-width:thin;scrollbar-color:var(--border, #1e1e1e) transparent}";
var Lf = Object.defineProperty, Gt = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Lf(e, t, r), r;
}, ne;
const ft = (ne = class extends C {
  constructor() {
    super(...arguments), this.lang = "sema", this.noDedent = !1, this.noHighlight = !1, this.format = !1, this.copy = !1, this.lines = !1, this._raw = "", this._code = "", this._copy = new Ll(this, () => this._code), this._highlightTask = new va(this, {
      task: async ([e, t, s, r, o]) => {
        const i = s ? e.replace(/^\n/, "").replace(/\s+$/, "") : Vn(e);
        let a = i;
        return o && (ne.formatter ? a = await ne.formatter(i, { lang: t }) : ne._warnNoFormatter()), this._code = a, !r && Ft(t) ? Pl(a, t) : K(a);
      },
      args: () => [this._raw, this.lang, this.noDedent, this.noHighlight, this.format]
    }), this._onSlotChange = (e) => {
      const t = e.target;
      this._raw = t.assignedNodes({ flatten: !0 }).map((s) => s.textContent ?? "").join(""), this.requestUpdate();
    };
  }
  static _warnNoFormatter() {
    ne._warned || (ne._warned = !0, console.warn(
      "[sema-code] `format` set but no formatter registered. Set SemaCode.formatter to enable formatting."
    ));
  }
  /** Dedented (unformatted) source for the pre-highlight / fallback render. */
  _plainCode() {
    return this.noDedent ? this._raw.replace(/^\n/, "").replace(/\s+$/, "") : Vn(this._raw);
  }
  _lines(e) {
    return e.split(`
`).map((t) => w`<span class="cl">${an(t === "" ? "​" : t)}</span>`);
  }
  render() {
    return w`
      <div class="wrap">
        ${this.copy ? w`<button
              class="copy ${this._copy.copied ? "copied" : ""}"
              part="copy-button"
              type="button"
              aria-label="Copy code"
              @click=${this._copy.copy}
            >
              ${this._copy.copied ? "Copied" : "Copy"}
            </button>` : R}
        <pre class="sema-scroll" part="pre"><code part="code" aria-label=${`${this.lang} code`}>${this._highlightTask.render(
      {
        initial: () => this._lines(K(this._plainCode())),
        pending: () => this._lines(this._highlightTask.value ?? K(this._plainCode())),
        complete: (e) => this._lines(e),
        error: () => this._lines(K(this._plainCode()))
      }
    )}</code></pre>
        <slot @slotchange=${this._onSlotChange}></slot>
      </div>
    `;
  }
}, ne.styles = [
  C.base,
  q($n),
  q(Nl),
  q(Sn),
  I`
      :host {
        display: block;
      }
      .wrap {
        position: relative;
      }
      pre {
        margin: 0;
        padding: var(--space-md, 16px);
        background: var(--bg-editor, #0a0a0a);
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-lg, 6px);
        overflow-x: auto;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-md, 13px);
        line-height: 1.7;
        color: var(--text-primary, #d8d0c0);
        tab-size: 2;
      }
      code {
        font: inherit;
        color: inherit;
      }
      .cl {
        display: block;
        white-space: pre;
      }
      /* line numbers */
      :host([lines]) code {
        counter-reset: ln;
      }
      :host([lines]) .cl {
        padding-left: 3em;
        position: relative;
      }
      :host([lines]) .cl::before {
        counter-increment: ln;
        content: counter(ln);
        position: absolute;
        left: 0;
        width: 2.2em;
        text-align: right;
        color: var(--text-tertiary, #5a5448);
        user-select: none;
      }
      slot {
        display: none;
      }
    `
], ne.registerLanguage = Af, ne._warned = !1, ne);
Gt([
  m({ reflect: !0 })
], ft.prototype, "lang");
Gt([
  m({ type: Boolean, reflect: !0, attribute: "no-dedent" })
], ft.prototype, "noDedent");
Gt([
  m({ type: Boolean, reflect: !0, attribute: "no-highlight" })
], ft.prototype, "noHighlight");
Gt([
  m({ type: Boolean, reflect: !0 })
], ft.prototype, "format");
Gt([
  m({ type: Boolean, reflect: !0 })
], ft.prototype, "copy");
Gt([
  m({ type: Boolean, reflect: !0 })
], ft.prototype, "lines");
let Nf = ft;
customElements.define("sema-code", Nf);
/**
 * @license
 * Copyright 2020 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const { I: Mf } = bc, Xi = (n) => n, Of = (n) => n.strings === void 0, Ji = () => document.createComment(""), Ht = (n, e, t) => {
  var o;
  const s = n._$AA.parentNode, r = e === void 0 ? n._$AB : e._$AA;
  if (t === void 0) {
    const i = s.insertBefore(Ji(), r), a = s.insertBefore(Ji(), r);
    t = new Mf(i, a, n, n.options);
  } else {
    const i = t._$AB.nextSibling, a = t._$AM, l = a !== n;
    if (l) {
      let c;
      (o = t._$AQ) == null || o.call(t, n), t._$AM = n, t._$AP !== void 0 && (c = n._$AU) !== a._$AU && t._$AP(c);
    }
    if (i !== r || l) {
      let c = t._$AA;
      for (; c !== i; ) {
        const h = Xi(c).nextSibling;
        Xi(s).insertBefore(c, r), c = h;
      }
    }
  }
  return t;
}, Xe = (n, e, t = n) => (n._$AI(e, t), n), zf = {}, Ml = (n, e = zf) => n._$AH = e, Bf = (n) => n._$AH, rr = (n) => {
  n._$AR(), n._$AA.remove();
};
/**
 * @license
 * Copyright 2020 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const Ps = Dr(class extends Fr {
  constructor(n) {
    if (super(n), n.type !== Me.PROPERTY && n.type !== Me.ATTRIBUTE && n.type !== Me.BOOLEAN_ATTRIBUTE) throw Error("The `live` directive is not allowed on child or event bindings");
    if (!Of(n)) throw Error("`live` bindings can only contain a single expression");
  }
  render(n) {
    return n;
  }
  update(n, [e]) {
    if (e === le || e === R) return e;
    const t = n.element, s = n.name;
    if (n.type === Me.PROPERTY) {
      if (e === t[s]) return le;
    } else if (n.type === Me.BOOLEAN_ATTRIBUTE) {
      if (!!e === t.hasAttribute(s)) return le;
    } else if (n.type === Me.ATTRIBUTE && t.getAttribute(s) === e + "") return le;
    return Ml(n), e;
  }
});
/**
 * @license
 * Copyright 2018 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const X = (n) => n ?? R;
class Df {
  constructor(e, { max: t = 200, mergeDelay: s = 600, onChange: r = null } = {}) {
    this._applying = !1, this._inTransaction = 0, this._suppress = !1, this._lastInputType = null, this._lastPushAt = 0, this._lastKind = null, this._composing = !1, this._forceNew = !1, this.ta = e, this.max = t, this.mergeDelay = s, this.onChange = r, this.stack = [this._read()], this.index = 0, e.addEventListener("beforeinput", (o) => {
      this._lastInputType = o.inputType || null;
    }), e.addEventListener("compositionstart", () => {
      this._composing = !0;
    }), e.addEventListener("compositionend", () => {
      this._composing = !1, this._forceNew = !0;
    }), e.addEventListener("input", () => {
      this._applying || this._suppress || this._inTransaction || this._composing || this._record();
    }), e.addEventListener("keydown", (o) => {
      const i = o.metaKey || o.ctrlKey;
      i && !o.altKey && o.key.toLowerCase() === "z" ? (o.preventDefault(), o.shiftKey ? this.redo() : this.undo()) : i && !o.altKey && o.key.toLowerCase() === "y" && (o.preventDefault(), this.redo());
    });
  }
  _read() {
    return { value: this.ta.value, start: this.ta.selectionStart ?? 0, end: this.ta.selectionEnd ?? 0 };
  }
  undo() {
    this.index > 0 && (this.index--, this._apply(this.stack[this.index]));
  }
  redo() {
    this.index < this.stack.length - 1 && (this.index++, this._apply(this.stack[this.index]));
  }
  transact(e) {
    this._inTransaction++;
    try {
      e();
    } finally {
      this._inTransaction--, this._inTransaction === 0 && this._record(!0);
    }
  }
  reset() {
    this.stack = [this._read()], this.index = 0, this._lastPushAt = 0, this._lastKind = null;
  }
  _record(e = !1) {
    const t = this._read(), s = this.stack[this.index];
    if (s.value === t.value && s.start === t.start && s.end === t.end) return;
    const r = performance.now(), o = this._lastInputType, i = o != null && o.startsWith("insert") ? "insert" : o != null && o.startsWith("delete") ? "delete" : "other", a = o === "insertFromPaste" || o === "insertFromDrop" || o === "deleteByCut";
    let l = !1;
    if (!e && !this._forceNew && !a && (l = r - this._lastPushAt <= this.mergeDelay && i === this._lastKind && s.start === s.end && t.start === t.end && (i === "insert" || i === "delete")), this._forceNew = !1, l)
      this.stack[this.index] = t;
    else if (this.stack.splice(this.index + 1), this.stack.push(t), this.index++, this.stack.length > this.max) {
      const c = this.stack.length - this.max;
      this.stack.splice(0, c), this.index = Math.max(0, this.index - c);
    }
    this._lastPushAt = r, this._lastKind = i;
  }
  _apply(e) {
    this._applying = !0, this.ta.value = e.value, this.ta.setSelectionRange(e.start, e.end), this.onChange ? this.onChange() : (this._suppress = !0, this.ta.dispatchEvent(new Event("input", { bubbles: !0 })), this._suppress = !1), this._applying = !1;
  }
}
var Ff = Object.defineProperty, fe = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Ff(e, t, r), r;
};
const or = typeof CSS < "u" && typeof CSS.supports == "function" && CSS.supports("field-sizing", "content"), Ao = class Ao extends C {
  constructor() {
    super(...arguments), this.value = "", this.lang = "sema", this.placeholder = "", this.readonly = !1, this.autosize = !1, this.tabSize = 2, this.testid = "", this.lineNumbers = !1, this.breakpoints = [], this.currentLine = 0, this._lines = [""], this._onInput = (e) => {
      e == null || e.stopPropagation();
      const t = this._ta;
      t && (this.value = t.value, this.autosize && !or && this._grow(), this.dispatchEvent(
        new CustomEvent("input", { detail: { value: this.value }, bubbles: !0, composed: !0 })
      ));
    }, this._onChange = (e) => {
      e == null || e.stopPropagation(), this.dispatchEvent(
        new CustomEvent("change", { detail: { value: this.value }, bubbles: !0, composed: !0 })
      );
    }, this._onScroll = () => {
      var r, o;
      const e = this._ta, t = (r = this.shadowRoot) == null ? void 0 : r.querySelector(".hl"), s = (o = this.shadowRoot) == null ? void 0 : o.querySelector(".gutter");
      e && t && (t.scrollTop = e.scrollTop, t.scrollLeft = e.scrollLeft), e && s && (s.scrollTop = e.scrollTop);
    }, this._onKeydown = (e) => {
      e.key === "Tab" && !e.metaKey && !e.ctrlKey && !e.altKey && (e.preventDefault(), e.shiftKey ? this._dedent() : this._indent());
    };
  }
  get _ta() {
    var e;
    return ((e = this.shadowRoot) == null ? void 0 : e.querySelector("textarea")) ?? null;
  }
  connectedCallback() {
    super.connectedCallback(), this._warm();
  }
  async _warm() {
    await Tf(this.lang), this._relight();
  }
  /** Recompute the per-line highlighted overlay. Shiki tokenizes per line and never
   * spans a newline, so splitting the joined output yields valid per-line HTML. */
  _relight() {
    const e = If(this.value, this.lang);
    this._lines = e.length ? e.split(`
`) : [""];
  }
  willUpdate(e) {
    (e.has("value") || e.has("lang")) && (this._relight(), e.has("lang") && this._warm());
  }
  firstUpdated() {
    const e = this._ta;
    e && (this._undo = new Df(e, { onChange: () => this._onInput() })), this.autosize && !or && this._grow(), this.hasAttribute("autofocus") && this.focus();
  }
  updated(e) {
    e.has("value") && this.autosize && !or && this._grow();
  }
  /** Clear the undo/redo history — call after loading unrelated content. */
  resetHistory() {
    var e;
    (e = this._undo) == null || e.reset();
  }
  /** Focus delegates to the inner textarea (the host itself isn't focusable). */
  focus() {
    var e;
    (e = this._ta) == null || e.focus();
  }
  /** Blur delegates to the inner textarea. */
  blur() {
    var e;
    (e = this._ta) == null || e.blur();
  }
  /** Scroll so 1-based `line` is vertically centered (e.g. a debugger's current line). */
  scrollToLine(e) {
    const t = this._ta;
    if (!t) return;
    const s = getComputedStyle(t), r = parseFloat(s.lineHeight) || 22, o = parseFloat(s.paddingTop) || 0;
    t.scrollTop = Math.max(0, o + (e - 1) * r - t.clientHeight / 2 + r);
  }
  _grow() {
    const e = this._ta;
    e && (e.style.height = "auto", e.style.height = `${e.scrollHeight}px`);
  }
  /** Tab: insert spaces at the cursor, or indent every line in a multi-line selection. */
  _indent() {
    const e = this._ta;
    if (!e) return;
    const { selectionStart: t, selectionEnd: s, value: r } = e, o = " ".repeat(this.tabSize);
    if (t === s || !r.slice(t, s).includes(`
`))
      e.value = r.slice(0, t) + o + r.slice(s), e.selectionStart = e.selectionEnd = t + o.length;
    else {
      const i = r.lastIndexOf(`
`, t - 1) + 1, a = r.slice(i, s), l = a.replace(/^/gm, o);
      e.value = r.slice(0, i) + l + r.slice(s), e.selectionStart = i, e.selectionEnd = s + (l.length - a.length);
    }
    e.dispatchEvent(new Event("input", { bubbles: !0 }));
  }
  /** Shift+Tab: remove up to `tab-size` leading spaces from each line in range. */
  _dedent() {
    const e = this._ta;
    if (!e) return;
    const { selectionStart: t, selectionEnd: s, value: r } = e, o = r.lastIndexOf(`
`, t - 1) + 1, i = r.slice(o, s);
    let a = 0, l = 0;
    const c = i.split(`
`).map((h, u) => {
      const p = /^ */.exec(h)[0].length, d = Math.min(p, this.tabSize);
      return u === 0 && (a = d), l += d, h.slice(d);
    });
    l !== 0 && (e.value = r.slice(0, o) + c.join(`
`) + r.slice(s), e.selectionStart = Math.max(o, t - a), e.selectionEnd = s - l, e.dispatchEvent(new Event("input", { bubbles: !0 })));
  }
  _gutterClick(e) {
    this.dispatchEvent(
      new CustomEvent("gutter-click", { detail: { line: e }, bubbles: !0, composed: !0 })
    );
  }
  _onGutterKeydown(e, t) {
    (e.key === "Enter" || e.key === " ") && (e.preventDefault(), this._gutterClick(t));
  }
  render() {
    const e = new Set(this.breakpoints), t = this.currentLine, s = this._lines.map(
      (r, o) => w`<div class="ln ${o + 1 === t ? "cur" : ""}" part="line">${an(r === "" ? "​" : r)}</div>`
    );
    return w`
      <div class="wrap">
        ${this.lineNumbers ? w`<div class="gutter" part="gutter">
              ${this._lines.map((r, o) => {
      const i = o + 1;
      return w`<div
                  class="gl ${e.has(i) ? "bp" : ""} ${i === t ? "cur" : ""}"
                  part="gutter-line${e.has(i) ? " breakpoint" : ""}${i === t ? " current" : ""}"
                  role="button"
                  tabindex="0"
                  aria-label=${`Toggle breakpoint on line ${i}`}
                  @click=${() => this._gutterClick(i)}
                  @keydown=${(a) => this._onGutterKeydown(a, i)}
                >
                  ${i}
                </div>`;
    })}
            </div>` : R}
        <div class="stack">
          <div class="hl sema-scroll" part="highlight" aria-hidden="true">${s}</div>
          <textarea
            class="sema-scroll"
            part="textarea"
            data-testid=${X(this.testid || void 0)}
            .value=${Ps(this.value)}
            ?readonly=${this.readonly}
            placeholder=${this.placeholder}
            spellcheck="false"
            autocapitalize="off"
            autocomplete="off"
            @input=${this._onInput}
            @change=${this._onChange}
            @scroll=${this._onScroll}
            @keydown=${this._onKeydown}
          ></textarea>
        </div>
      </div>
    `;
  }
};
Ao.styles = [
  C.base,
  q($n),
  q(Sn),
  I`
      :host {
        display: block;
      }
      .wrap {
        position: relative;
        display: flex;
        height: 100%;
        background: var(--bg-editor, #0a0a0a);
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-sm, 4px);
        overflow: hidden;
      }
      :host([autosize]) .wrap {
        height: auto;
      }
      /* gutter */
      .gutter {
        flex: 0 0 auto;
        overflow: hidden;
        user-select: none;
        background: var(--bg-editor, #0a0a0a);
        color: var(--text-tertiary, #5a5448);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-md, 13px);
        line-height: 1.7;
        padding: var(--space-sm, 8px) 0;
        text-align: right;
        border-right: 1px solid var(--border, #1e1e1e);
      }
      .gl {
        position: relative;
        padding: 0 0.55em 0 1.4em;
        cursor: pointer;
      }
      .gl:hover {
        color: var(--text-secondary, #a89f8c);
      }
      .gl.cur {
        color: var(--gold, #d4a537);
        background: var(--gold-glow, rgba(200, 168, 85, 0.15));
      }
      .gl.bp::before {
        content: '';
        position: absolute;
        left: 0.45em;
        top: 50%;
        transform: translateY(-50%);
        width: 0.55em;
        height: 0.55em;
        border-radius: 50%;
        background: var(--danger, #e5484d);
      }
      /* editor stack */
      .stack {
        position: relative;
        flex: 1 1 auto;
        overflow: hidden;
        min-height: 1.7em;
      }
      .hl,
      textarea {
        margin: 0;
        padding: var(--space-sm, 8px) var(--space-md, 12px);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-md, 13px);
        line-height: 1.7;
        tab-size: 2;
        white-space: pre;
        border: 0;
        box-sizing: border-box;
        letter-spacing: normal;
      }
      :host([autosize]) .hl,
      :host([autosize]) textarea {
        white-space: pre-wrap;
        word-break: break-word;
        overflow-wrap: break-word;
      }
      .hl {
        position: absolute;
        inset: 0;
        overflow: hidden;
        pointer-events: none;
        color: var(--text-primary, #d8d0c0);
      }
      .ln {
        display: block;
        position: relative;
      }
      .ln.cur {
        background: var(--bg-line-highlight, rgba(200, 168, 85, 0.1));
      }
      /* Gold accent bar at the gutter/code boundary for the current/debug line.
         .ln sits inside .hl's left padding, so reach back to the code-area edge. */
      .ln.cur::before {
        content: '';
        position: absolute;
        top: 0;
        bottom: 0;
        left: calc(-1 * var(--space-md, 12px));
        width: 2px;
        background: var(--gold, #d4a537);
      }
      textarea {
        position: relative;
        display: block;
        width: 100%;
        height: 100%;
        resize: none;
        background: transparent;
        color: transparent;
        caret-color: var(--text-primary, #d8d0c0);
        outline: none;
        overflow: auto;
      }
      :host([autosize]) textarea {
        /* Grow with content via CSS where supported (no measure-timing race);
           the scrollHeight fallback in _grow() covers browsers without it. */
        field-sizing: content;
        height: auto;
        min-height: 1.7em;
        max-height: none;
        overflow: hidden;
      }
      textarea::selection {
        background: var(--gold-dim, #3a3320);
        color: transparent;
      }
    `
];
let J = Ao;
fe([
  m()
], J.prototype, "value");
fe([
  m({ reflect: !0 })
], J.prototype, "lang");
fe([
  m()
], J.prototype, "placeholder");
fe([
  m({ type: Boolean, reflect: !0 })
], J.prototype, "readonly");
fe([
  m({ type: Boolean, reflect: !0 })
], J.prototype, "autosize");
fe([
  m({ type: Number, attribute: "tab-size" })
], J.prototype, "tabSize");
fe([
  m()
], J.prototype, "testid");
fe([
  m({ type: Boolean, reflect: !0, attribute: "line-numbers" })
], J.prototype, "lineNumbers");
fe([
  m({ attribute: !1 })
], J.prototype, "breakpoints");
fe([
  m({ type: Number, attribute: "current-line" })
], J.prototype, "currentLine");
fe([
  Qe()
], J.prototype, "_lines");
customElements.define("sema-editor", J);
function eo() {
  return {
    async: !1,
    breaks: !1,
    extensions: null,
    gfm: !0,
    hooks: null,
    pedantic: !1,
    renderer: null,
    silent: !1,
    tokenizer: null,
    walkTokens: null
  };
}
let gt = eo();
function Ol(n) {
  gt = n;
}
const zl = /[&<>"']/, Gf = new RegExp(zl.source, "g"), Bl = /[<>"']|&(?!(#\d{1,7}|#[Xx][a-fA-F0-9]{1,6}|\w+);)/, Uf = new RegExp(Bl.source, "g"), jf = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;"
}, Yi = (n) => jf[n];
function se(n, e) {
  if (e) {
    if (zl.test(n))
      return n.replace(Gf, Yi);
  } else if (Bl.test(n))
    return n.replace(Uf, Yi);
  return n;
}
const qf = /&(#(?:\d+)|(?:#x[0-9A-Fa-f]+)|(?:\w+));?/ig;
function Hf(n) {
  return n.replace(qf, (e, t) => (t = t.toLowerCase(), t === "colon" ? ":" : t.charAt(0) === "#" ? t.charAt(1) === "x" ? String.fromCharCode(parseInt(t.substring(2), 16)) : String.fromCharCode(+t.substring(1)) : ""));
}
const Wf = /(^|[^\[])\^/g;
function L(n, e) {
  let t = typeof n == "string" ? n : n.source;
  e = e || "";
  const s = {
    replace: (r, o) => {
      let i = typeof o == "string" ? o : o.source;
      return i = i.replace(Wf, "$1"), t = t.replace(r, i), s;
    },
    getRegex: () => new RegExp(t, e)
  };
  return s;
}
function ea(n) {
  try {
    n = encodeURI(n).replace(/%25/g, "%");
  } catch {
    return null;
  }
  return n;
}
const Yt = { exec: () => null };
function ta(n, e) {
  const t = n.replace(/\|/g, (o, i, a) => {
    let l = !1, c = i;
    for (; --c >= 0 && a[c] === "\\"; )
      l = !l;
    return l ? "|" : " |";
  }), s = t.split(/ \|/);
  let r = 0;
  if (s[0].trim() || s.shift(), s.length > 0 && !s[s.length - 1].trim() && s.pop(), e)
    if (s.length > e)
      s.splice(e);
    else
      for (; s.length < e; )
        s.push("");
  for (; r < s.length; r++)
    s[r] = s[r].trim().replace(/\\\|/g, "|");
  return s;
}
function Bn(n, e, t) {
  const s = n.length;
  if (s === 0)
    return "";
  let r = 0;
  for (; r < s && n.charAt(s - r - 1) === e; )
    r++;
  return n.slice(0, s - r);
}
function Qf(n, e) {
  if (n.indexOf(e[1]) === -1)
    return -1;
  let t = 0;
  for (let s = 0; s < n.length; s++)
    if (n[s] === "\\")
      s++;
    else if (n[s] === e[0])
      t++;
    else if (n[s] === e[1] && (t--, t < 0))
      return s;
  return -1;
}
function na(n, e, t, s) {
  const r = e.href, o = e.title ? se(e.title) : null, i = n[1].replace(/\\([\[\]])/g, "$1");
  if (n[0].charAt(0) !== "!") {
    s.state.inLink = !0;
    const a = {
      type: "link",
      raw: t,
      href: r,
      title: o,
      text: i,
      tokens: s.inlineTokens(i)
    };
    return s.state.inLink = !1, a;
  }
  return {
    type: "image",
    raw: t,
    href: r,
    title: o,
    text: se(i)
  };
}
function Vf(n, e) {
  const t = n.match(/^(\s+)(?:```)/);
  if (t === null)
    return e;
  const s = t[1];
  return e.split(`
`).map((r) => {
    const o = r.match(/^\s+/);
    if (o === null)
      return r;
    const [i] = o;
    return i.length >= s.length ? r.slice(s.length) : r;
  }).join(`
`);
}
class ls {
  // set by the lexer
  constructor(e) {
    g(this, "options");
    g(this, "rules");
    // set by the lexer
    g(this, "lexer");
    this.options = e || gt;
  }
  space(e) {
    const t = this.rules.block.newline.exec(e);
    if (t && t[0].length > 0)
      return {
        type: "space",
        raw: t[0]
      };
  }
  code(e) {
    const t = this.rules.block.code.exec(e);
    if (t) {
      const s = t[0].replace(/^ {1,4}/gm, "");
      return {
        type: "code",
        raw: t[0],
        codeBlockStyle: "indented",
        text: this.options.pedantic ? s : Bn(s, `
`)
      };
    }
  }
  fences(e) {
    const t = this.rules.block.fences.exec(e);
    if (t) {
      const s = t[0], r = Vf(s, t[3] || "");
      return {
        type: "code",
        raw: s,
        lang: t[2] ? t[2].trim().replace(this.rules.inline.anyPunctuation, "$1") : t[2],
        text: r
      };
    }
  }
  heading(e) {
    const t = this.rules.block.heading.exec(e);
    if (t) {
      let s = t[2].trim();
      if (/#$/.test(s)) {
        const r = Bn(s, "#");
        (this.options.pedantic || !r || / $/.test(r)) && (s = r.trim());
      }
      return {
        type: "heading",
        raw: t[0],
        depth: t[1].length,
        text: s,
        tokens: this.lexer.inline(s)
      };
    }
  }
  hr(e) {
    const t = this.rules.block.hr.exec(e);
    if (t)
      return {
        type: "hr",
        raw: t[0]
      };
  }
  blockquote(e) {
    const t = this.rules.block.blockquote.exec(e);
    if (t) {
      let s = t[0].replace(/\n {0,3}((?:=+|-+) *)(?=\n|$)/g, `
    $1`);
      s = Bn(s.replace(/^ *>[ \t]?/gm, ""), `
`);
      const r = this.lexer.state.top;
      this.lexer.state.top = !0;
      const o = this.lexer.blockTokens(s);
      return this.lexer.state.top = r, {
        type: "blockquote",
        raw: t[0],
        tokens: o,
        text: s
      };
    }
  }
  list(e) {
    let t = this.rules.block.list.exec(e);
    if (t) {
      let s = t[1].trim();
      const r = s.length > 1, o = {
        type: "list",
        raw: "",
        ordered: r,
        start: r ? +s.slice(0, -1) : "",
        loose: !1,
        items: []
      };
      s = r ? `\\d{1,9}\\${s.slice(-1)}` : `\\${s}`, this.options.pedantic && (s = r ? s : "[*+-]");
      const i = new RegExp(`^( {0,3}${s})((?:[	 ][^\\n]*)?(?:\\n|$))`);
      let a = "", l = "", c = !1;
      for (; e; ) {
        let h = !1;
        if (!(t = i.exec(e)) || this.rules.block.hr.test(e))
          break;
        a = t[0], e = e.substring(a.length);
        let u = t[2].split(`
`, 1)[0].replace(/^\t+/, (k) => " ".repeat(3 * k.length)), p = e.split(`
`, 1)[0], d = 0;
        this.options.pedantic ? (d = 2, l = u.trimStart()) : (d = t[2].search(/[^ ]/), d = d > 4 ? 1 : d, l = u.slice(d), d += t[1].length);
        let f = !1;
        if (!u && /^ *$/.test(p) && (a += p + `
`, e = e.substring(p.length + 1), h = !0), !h) {
          const k = new RegExp(`^ {0,${Math.min(3, d - 1)}}(?:[*+-]|\\d{1,9}[.)])((?:[ 	][^\\n]*)?(?:\\n|$))`), _ = new RegExp(`^ {0,${Math.min(3, d - 1)}}((?:- *){3,}|(?:_ *){3,}|(?:\\* *){3,})(?:\\n+|$)`), x = new RegExp(`^ {0,${Math.min(3, d - 1)}}(?:\`\`\`|~~~)`), $ = new RegExp(`^ {0,${Math.min(3, d - 1)}}#`);
          for (; e; ) {
            const A = e.split(`
`, 1)[0];
            if (p = A, this.options.pedantic && (p = p.replace(/^ {1,4}(?=( {4})*[^ ])/g, "  ")), x.test(p) || $.test(p) || k.test(p) || _.test(e))
              break;
            if (p.search(/[^ ]/) >= d || !p.trim())
              l += `
` + p.slice(d);
            else {
              if (f || u.search(/[^ ]/) >= 4 || x.test(u) || $.test(u) || _.test(u))
                break;
              l += `
` + p;
            }
            !f && !p.trim() && (f = !0), a += A + `
`, e = e.substring(A.length + 1), u = p.slice(d);
          }
        }
        o.loose || (c ? o.loose = !0 : /\n *\n *$/.test(a) && (c = !0));
        let b = null, v;
        this.options.gfm && (b = /^\[[ xX]\] /.exec(l), b && (v = b[0] !== "[ ] ", l = l.replace(/^\[[ xX]\] +/, ""))), o.items.push({
          type: "list_item",
          raw: a,
          task: !!b,
          checked: v,
          loose: !1,
          text: l,
          tokens: []
        }), o.raw += a;
      }
      o.items[o.items.length - 1].raw = a.trimEnd(), o.items[o.items.length - 1].text = l.trimEnd(), o.raw = o.raw.trimEnd();
      for (let h = 0; h < o.items.length; h++)
        if (this.lexer.state.top = !1, o.items[h].tokens = this.lexer.blockTokens(o.items[h].text, []), !o.loose) {
          const u = o.items[h].tokens.filter((d) => d.type === "space"), p = u.length > 0 && u.some((d) => /\n.*\n/.test(d.raw));
          o.loose = p;
        }
      if (o.loose)
        for (let h = 0; h < o.items.length; h++)
          o.items[h].loose = !0;
      return o;
    }
  }
  html(e) {
    const t = this.rules.block.html.exec(e);
    if (t)
      return {
        type: "html",
        block: !0,
        raw: t[0],
        pre: t[1] === "pre" || t[1] === "script" || t[1] === "style",
        text: t[0]
      };
  }
  def(e) {
    const t = this.rules.block.def.exec(e);
    if (t) {
      const s = t[1].toLowerCase().replace(/\s+/g, " "), r = t[2] ? t[2].replace(/^<(.*)>$/, "$1").replace(this.rules.inline.anyPunctuation, "$1") : "", o = t[3] ? t[3].substring(1, t[3].length - 1).replace(this.rules.inline.anyPunctuation, "$1") : t[3];
      return {
        type: "def",
        tag: s,
        raw: t[0],
        href: r,
        title: o
      };
    }
  }
  table(e) {
    const t = this.rules.block.table.exec(e);
    if (!t || !/[:|]/.test(t[2]))
      return;
    const s = ta(t[1]), r = t[2].replace(/^\||\| *$/g, "").split("|"), o = t[3] && t[3].trim() ? t[3].replace(/\n[ \t]*$/, "").split(`
`) : [], i = {
      type: "table",
      raw: t[0],
      header: [],
      align: [],
      rows: []
    };
    if (s.length === r.length) {
      for (const a of r)
        /^ *-+: *$/.test(a) ? i.align.push("right") : /^ *:-+: *$/.test(a) ? i.align.push("center") : /^ *:-+ *$/.test(a) ? i.align.push("left") : i.align.push(null);
      for (const a of s)
        i.header.push({
          text: a,
          tokens: this.lexer.inline(a)
        });
      for (const a of o)
        i.rows.push(ta(a, i.header.length).map((l) => ({
          text: l,
          tokens: this.lexer.inline(l)
        })));
      return i;
    }
  }
  lheading(e) {
    const t = this.rules.block.lheading.exec(e);
    if (t)
      return {
        type: "heading",
        raw: t[0],
        depth: t[2].charAt(0) === "=" ? 1 : 2,
        text: t[1],
        tokens: this.lexer.inline(t[1])
      };
  }
  paragraph(e) {
    const t = this.rules.block.paragraph.exec(e);
    if (t) {
      const s = t[1].charAt(t[1].length - 1) === `
` ? t[1].slice(0, -1) : t[1];
      return {
        type: "paragraph",
        raw: t[0],
        text: s,
        tokens: this.lexer.inline(s)
      };
    }
  }
  text(e) {
    const t = this.rules.block.text.exec(e);
    if (t)
      return {
        type: "text",
        raw: t[0],
        text: t[0],
        tokens: this.lexer.inline(t[0])
      };
  }
  escape(e) {
    const t = this.rules.inline.escape.exec(e);
    if (t)
      return {
        type: "escape",
        raw: t[0],
        text: se(t[1])
      };
  }
  tag(e) {
    const t = this.rules.inline.tag.exec(e);
    if (t)
      return !this.lexer.state.inLink && /^<a /i.test(t[0]) ? this.lexer.state.inLink = !0 : this.lexer.state.inLink && /^<\/a>/i.test(t[0]) && (this.lexer.state.inLink = !1), !this.lexer.state.inRawBlock && /^<(pre|code|kbd|script)(\s|>)/i.test(t[0]) ? this.lexer.state.inRawBlock = !0 : this.lexer.state.inRawBlock && /^<\/(pre|code|kbd|script)(\s|>)/i.test(t[0]) && (this.lexer.state.inRawBlock = !1), {
        type: "html",
        raw: t[0],
        inLink: this.lexer.state.inLink,
        inRawBlock: this.lexer.state.inRawBlock,
        block: !1,
        text: t[0]
      };
  }
  link(e) {
    const t = this.rules.inline.link.exec(e);
    if (t) {
      const s = t[2].trim();
      if (!this.options.pedantic && /^</.test(s)) {
        if (!/>$/.test(s))
          return;
        const i = Bn(s.slice(0, -1), "\\");
        if ((s.length - i.length) % 2 === 0)
          return;
      } else {
        const i = Qf(t[2], "()");
        if (i > -1) {
          const l = (t[0].indexOf("!") === 0 ? 5 : 4) + t[1].length + i;
          t[2] = t[2].substring(0, i), t[0] = t[0].substring(0, l).trim(), t[3] = "";
        }
      }
      let r = t[2], o = "";
      if (this.options.pedantic) {
        const i = /^([^'"]*[^\s])\s+(['"])(.*)\2/.exec(r);
        i && (r = i[1], o = i[3]);
      } else
        o = t[3] ? t[3].slice(1, -1) : "";
      return r = r.trim(), /^</.test(r) && (this.options.pedantic && !/>$/.test(s) ? r = r.slice(1) : r = r.slice(1, -1)), na(t, {
        href: r && r.replace(this.rules.inline.anyPunctuation, "$1"),
        title: o && o.replace(this.rules.inline.anyPunctuation, "$1")
      }, t[0], this.lexer);
    }
  }
  reflink(e, t) {
    let s;
    if ((s = this.rules.inline.reflink.exec(e)) || (s = this.rules.inline.nolink.exec(e))) {
      const r = (s[2] || s[1]).replace(/\s+/g, " "), o = t[r.toLowerCase()];
      if (!o) {
        const i = s[0].charAt(0);
        return {
          type: "text",
          raw: i,
          text: i
        };
      }
      return na(s, o, s[0], this.lexer);
    }
  }
  emStrong(e, t, s = "") {
    let r = this.rules.inline.emStrongLDelim.exec(e);
    if (!r || r[3] && s.match(/[\p{L}\p{N}]/u))
      return;
    if (!(r[1] || r[2] || "") || !s || this.rules.inline.punctuation.exec(s)) {
      const i = [...r[0]].length - 1;
      let a, l, c = i, h = 0;
      const u = r[0][0] === "*" ? this.rules.inline.emStrongRDelimAst : this.rules.inline.emStrongRDelimUnd;
      for (u.lastIndex = 0, t = t.slice(-1 * e.length + i); (r = u.exec(t)) != null; ) {
        if (a = r[1] || r[2] || r[3] || r[4] || r[5] || r[6], !a)
          continue;
        if (l = [...a].length, r[3] || r[4]) {
          c += l;
          continue;
        } else if ((r[5] || r[6]) && i % 3 && !((i + l) % 3)) {
          h += l;
          continue;
        }
        if (c -= l, c > 0)
          continue;
        l = Math.min(l, l + c + h);
        const p = [...r[0]][0].length, d = e.slice(0, i + r.index + p + l);
        if (Math.min(i, l) % 2) {
          const b = d.slice(1, -1);
          return {
            type: "em",
            raw: d,
            text: b,
            tokens: this.lexer.inlineTokens(b)
          };
        }
        const f = d.slice(2, -2);
        return {
          type: "strong",
          raw: d,
          text: f,
          tokens: this.lexer.inlineTokens(f)
        };
      }
    }
  }
  codespan(e) {
    const t = this.rules.inline.code.exec(e);
    if (t) {
      let s = t[2].replace(/\n/g, " ");
      const r = /[^ ]/.test(s), o = /^ /.test(s) && / $/.test(s);
      return r && o && (s = s.substring(1, s.length - 1)), s = se(s, !0), {
        type: "codespan",
        raw: t[0],
        text: s
      };
    }
  }
  br(e) {
    const t = this.rules.inline.br.exec(e);
    if (t)
      return {
        type: "br",
        raw: t[0]
      };
  }
  del(e) {
    const t = this.rules.inline.del.exec(e);
    if (t)
      return {
        type: "del",
        raw: t[0],
        text: t[2],
        tokens: this.lexer.inlineTokens(t[2])
      };
  }
  autolink(e) {
    const t = this.rules.inline.autolink.exec(e);
    if (t) {
      let s, r;
      return t[2] === "@" ? (s = se(t[1]), r = "mailto:" + s) : (s = se(t[1]), r = s), {
        type: "link",
        raw: t[0],
        text: s,
        href: r,
        tokens: [
          {
            type: "text",
            raw: s,
            text: s
          }
        ]
      };
    }
  }
  url(e) {
    var s;
    let t;
    if (t = this.rules.inline.url.exec(e)) {
      let r, o;
      if (t[2] === "@")
        r = se(t[0]), o = "mailto:" + r;
      else {
        let i;
        do
          i = t[0], t[0] = ((s = this.rules.inline._backpedal.exec(t[0])) == null ? void 0 : s[0]) ?? "";
        while (i !== t[0]);
        r = se(t[0]), t[1] === "www." ? o = "http://" + t[0] : o = t[0];
      }
      return {
        type: "link",
        raw: t[0],
        text: r,
        href: o,
        tokens: [
          {
            type: "text",
            raw: r,
            text: r
          }
        ]
      };
    }
  }
  inlineText(e) {
    const t = this.rules.inline.text.exec(e);
    if (t) {
      let s;
      return this.lexer.state.inRawBlock ? s = t[0] : s = se(t[0]), {
        type: "text",
        raw: t[0],
        text: s
      };
    }
  }
}
const Kf = /^(?: *(?:\n|$))+/, Zf = /^( {4}[^\n]+(?:\n(?: *(?:\n|$))*)?)+/, Xf = /^ {0,3}(`{3,}(?=[^`\n]*(?:\n|$))|~{3,})([^\n]*)(?:\n|$)(?:|([\s\S]*?)(?:\n|$))(?: {0,3}\1[~`]* *(?=\n|$)|$)/, An = /^ {0,3}((?:-[\t ]*){3,}|(?:_[ \t]*){3,}|(?:\*[ \t]*){3,})(?:\n+|$)/, Jf = /^ {0,3}(#{1,6})(?=\s|$)(.*)(?:\n+|$)/, Dl = /(?:[*+-]|\d{1,9}[.)])/, Fl = L(/^(?!bull |blockCode|fences|blockquote|heading|html)((?:.|\n(?!\s*?\n|bull |blockCode|fences|blockquote|heading|html))+?)\n {0,3}(=+|-+) *(?:\n+|$)/).replace(/bull/g, Dl).replace(/blockCode/g, / {4}/).replace(/fences/g, / {0,3}(?:`{3,}|~{3,})/).replace(/blockquote/g, / {0,3}>/).replace(/heading/g, / {0,3}#{1,6}/).replace(/html/g, / {0,3}<[^\n>]+>\n/).getRegex(), to = /^([^\n]+(?:\n(?!hr|heading|lheading|blockquote|fences|list|html|table| +\n)[^\n]+)*)/, Yf = /^[^\n]+/, no = /(?!\s*\])(?:\\.|[^\[\]\\])+/, eg = L(/^ {0,3}\[(label)\]: *(?:\n *)?([^<\s][^\s]*|<.*?>)(?:(?: +(?:\n *)?| *\n *)(title))? *(?:\n+|$)/).replace("label", no).replace("title", /(?:"(?:\\"?|[^"\\])*"|'[^'\n]*(?:\n[^'\n]+)*\n?'|\([^()]*\))/).getRegex(), tg = L(/^( {0,3}bull)([ \t][^\n]+?)?(?:\n|$)/).replace(/bull/g, Dl).getRegex(), Ls = "address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|meta|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul", so = /<!--(?:-?>|[\s\S]*?(?:-->|$))/, ng = L("^ {0,3}(?:<(script|pre|style|textarea)[\\s>][\\s\\S]*?(?:</\\1>[^\\n]*\\n+|$)|comment[^\\n]*(\\n+|$)|<\\?[\\s\\S]*?(?:\\?>\\n*|$)|<![A-Z][\\s\\S]*?(?:>\\n*|$)|<!\\[CDATA\\[[\\s\\S]*?(?:\\]\\]>\\n*|$)|</?(tag)(?: +|\\n|/?>)[\\s\\S]*?(?:(?:\\n *)+\\n|$)|<(?!script|pre|style|textarea)([a-z][\\w-]*)(?:attribute)*? */?>(?=[ \\t]*(?:\\n|$))[\\s\\S]*?(?:(?:\\n *)+\\n|$)|</(?!script|pre|style|textarea)[a-z][\\w-]*\\s*>(?=[ \\t]*(?:\\n|$))[\\s\\S]*?(?:(?:\\n *)+\\n|$))", "i").replace("comment", so).replace("tag", Ls).replace("attribute", / +[a-zA-Z:_][\w.:-]*(?: *= *"[^"\n]*"| *= *'[^'\n]*'| *= *[^\s"'=<>`]+)?/).getRegex(), Gl = L(to).replace("hr", An).replace("heading", " {0,3}#{1,6}(?:\\s|$)").replace("|lheading", "").replace("|table", "").replace("blockquote", " {0,3}>").replace("fences", " {0,3}(?:`{3,}(?=[^`\\n]*\\n)|~{3,})[^\\n]*\\n").replace("list", " {0,3}(?:[*+-]|1[.)]) ").replace("html", "</?(?:tag)(?: +|\\n|/?>)|<(?:script|pre|style|textarea|!--)").replace("tag", Ls).getRegex(), sg = L(/^( {0,3}> ?(paragraph|[^\n]*)(?:\n|$))+/).replace("paragraph", Gl).getRegex(), ro = {
  blockquote: sg,
  code: Zf,
  def: eg,
  fences: Xf,
  heading: Jf,
  hr: An,
  html: ng,
  lheading: Fl,
  list: tg,
  newline: Kf,
  paragraph: Gl,
  table: Yt,
  text: Yf
}, sa = L("^ *([^\\n ].*)\\n {0,3}((?:\\| *)?:?-+:? *(?:\\| *:?-+:? *)*(?:\\| *)?)(?:\\n((?:(?! *\\n|hr|heading|blockquote|code|fences|list|html).*(?:\\n|$))*)\\n*|$)").replace("hr", An).replace("heading", " {0,3}#{1,6}(?:\\s|$)").replace("blockquote", " {0,3}>").replace("code", " {4}[^\\n]").replace("fences", " {0,3}(?:`{3,}(?=[^`\\n]*\\n)|~{3,})[^\\n]*\\n").replace("list", " {0,3}(?:[*+-]|1[.)]) ").replace("html", "</?(?:tag)(?: +|\\n|/?>)|<(?:script|pre|style|textarea|!--)").replace("tag", Ls).getRegex(), rg = {
  ...ro,
  table: sa,
  paragraph: L(to).replace("hr", An).replace("heading", " {0,3}#{1,6}(?:\\s|$)").replace("|lheading", "").replace("table", sa).replace("blockquote", " {0,3}>").replace("fences", " {0,3}(?:`{3,}(?=[^`\\n]*\\n)|~{3,})[^\\n]*\\n").replace("list", " {0,3}(?:[*+-]|1[.)]) ").replace("html", "</?(?:tag)(?: +|\\n|/?>)|<(?:script|pre|style|textarea|!--)").replace("tag", Ls).getRegex()
}, og = {
  ...ro,
  html: L(`^ *(?:comment *(?:\\n|\\s*$)|<(tag)[\\s\\S]+?</\\1> *(?:\\n{2,}|\\s*$)|<tag(?:"[^"]*"|'[^']*'|\\s[^'"/>\\s]*)*?/?> *(?:\\n{2,}|\\s*$))`).replace("comment", so).replace(/tag/g, "(?!(?:a|em|strong|small|s|cite|q|dfn|abbr|data|time|code|var|samp|kbd|sub|sup|i|b|u|mark|ruby|rt|rp|bdi|bdo|span|br|wbr|ins|del|img)\\b)\\w+(?!:|[^\\w\\s@]*@)\\b").getRegex(),
  def: /^ *\[([^\]]+)\]: *<?([^\s>]+)>?(?: +(["(][^\n]+[")]))? *(?:\n+|$)/,
  heading: /^(#{1,6})(.*)(?:\n+|$)/,
  fences: Yt,
  // fences not supported
  lheading: /^(.+?)\n {0,3}(=+|-+) *(?:\n+|$)/,
  paragraph: L(to).replace("hr", An).replace("heading", ` *#{1,6} *[^
]`).replace("lheading", Fl).replace("|table", "").replace("blockquote", " {0,3}>").replace("|fences", "").replace("|list", "").replace("|html", "").replace("|tag", "").getRegex()
}, Ul = /^\\([!"#$%&'()*+,\-./:;<=>?@\[\]\\^_`{|}~])/, ig = /^(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)/, jl = /^( {2,}|\\)\n(?!\s*$)/, ag = /^(`+|[^`])(?:(?= {2,}\n)|[\s\S]*?(?:(?=[\\<!\[`*_]|\b_|$)|[^ ](?= {2,}\n)))/, En = "\\p{P}\\p{S}", lg = L(/^((?![*_])[\spunctuation])/, "u").replace(/punctuation/g, En).getRegex(), cg = /\[[^[\]]*?\]\([^\(\)]*?\)|`[^`]*?`|<[^<>]*?>/g, ug = L(/^(?:\*+(?:((?!\*)[punct])|[^\s*]))|^_+(?:((?!_)[punct])|([^\s_]))/, "u").replace(/punct/g, En).getRegex(), hg = L("^[^_*]*?__[^_*]*?\\*[^_*]*?(?=__)|[^*]+(?=[^*])|(?!\\*)[punct](\\*+)(?=[\\s]|$)|[^punct\\s](\\*+)(?!\\*)(?=[punct\\s]|$)|(?!\\*)[punct\\s](\\*+)(?=[^punct\\s])|[\\s](\\*+)(?!\\*)(?=[punct])|(?!\\*)[punct](\\*+)(?!\\*)(?=[punct])|[^punct\\s](\\*+)(?=[^punct\\s])", "gu").replace(/punct/g, En).getRegex(), pg = L("^[^_*]*?\\*\\*[^_*]*?_[^_*]*?(?=\\*\\*)|[^_]+(?=[^_])|(?!_)[punct](_+)(?=[\\s]|$)|[^punct\\s](_+)(?!_)(?=[punct\\s]|$)|(?!_)[punct\\s](_+)(?=[^punct\\s])|[\\s](_+)(?!_)(?=[punct])|(?!_)[punct](_+)(?!_)(?=[punct])", "gu").replace(/punct/g, En).getRegex(), dg = L(/\\([punct])/, "gu").replace(/punct/g, En).getRegex(), fg = L(/^<(scheme:[^\s\x00-\x1f<>]*|email)>/).replace("scheme", /[a-zA-Z][a-zA-Z0-9+.-]{1,31}/).replace("email", /[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+(@)[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)+(?![-_])/).getRegex(), gg = L(so).replace("(?:-->|$)", "-->").getRegex(), mg = L("^comment|^</[a-zA-Z][\\w:-]*\\s*>|^<[a-zA-Z][\\w-]*(?:attribute)*?\\s*/?>|^<\\?[\\s\\S]*?\\?>|^<![a-zA-Z]+\\s[\\s\\S]*?>|^<!\\[CDATA\\[[\\s\\S]*?\\]\\]>").replace("comment", gg).replace("attribute", /\s+[a-zA-Z:_][\w.:-]*(?:\s*=\s*"[^"]*"|\s*=\s*'[^']*'|\s*=\s*[^\s"'=<>`]+)?/).getRegex(), cs = /(?:\[(?:\\.|[^\[\]\\])*\]|\\.|`[^`]*`|[^\[\]\\`])*?/, bg = L(/^!?\[(label)\]\(\s*(href)(?:\s+(title))?\s*\)/).replace("label", cs).replace("href", /<(?:\\.|[^\n<>\\])+>|[^\s\x00-\x1f]*/).replace("title", /"(?:\\"?|[^"\\])*"|'(?:\\'?|[^'\\])*'|\((?:\\\)?|[^)\\])*\)/).getRegex(), ql = L(/^!?\[(label)\]\[(ref)\]/).replace("label", cs).replace("ref", no).getRegex(), Hl = L(/^!?\[(ref)\](?:\[\])?/).replace("ref", no).getRegex(), yg = L("reflink|nolink(?!\\()", "g").replace("reflink", ql).replace("nolink", Hl).getRegex(), oo = {
  _backpedal: Yt,
  // only used for GFM url
  anyPunctuation: dg,
  autolink: fg,
  blockSkip: cg,
  br: jl,
  code: ig,
  del: Yt,
  emStrongLDelim: ug,
  emStrongRDelimAst: hg,
  emStrongRDelimUnd: pg,
  escape: Ul,
  link: bg,
  nolink: Hl,
  punctuation: lg,
  reflink: ql,
  reflinkSearch: yg,
  tag: mg,
  text: ag,
  url: Yt
}, vg = {
  ...oo,
  link: L(/^!?\[(label)\]\((.*?)\)/).replace("label", cs).getRegex(),
  reflink: L(/^!?\[(label)\]\s*\[([^\]]*)\]/).replace("label", cs).getRegex()
}, Er = {
  ...oo,
  escape: L(Ul).replace("])", "~|])").getRegex(),
  url: L(/^((?:ftp|https?):\/\/|www\.)(?:[a-zA-Z0-9\-]+\.?)+[^\s<]*|^email/, "i").replace("email", /[A-Za-z0-9._+-]+(@)[a-zA-Z0-9-_]+(?:\.[a-zA-Z0-9-_]*[a-zA-Z0-9])+(?![-_])/).getRegex(),
  _backpedal: /(?:[^?!.,:;*_'"~()&]+|\([^)]*\)|&(?![a-zA-Z0-9]+;$)|[?!.,:;*_'"~)]+(?!$))+/,
  del: /^(~~?)(?=[^\s~])([\s\S]*?[^\s~])\1(?=[^~]|$)/,
  text: /^([`~]+|[^`~])(?:(?= {2,}\n)|(?=[a-zA-Z0-9.!#$%&'*+\/=?_`{\|}~-]+@)|[\s\S]*?(?:(?=[\\<!\[`*~_]|\b_|https?:\/\/|ftp:\/\/|www\.|$)|[^ ](?= {2,}\n)|[^a-zA-Z0-9.!#$%&'*+\/=?_`{\|}~-](?=[a-zA-Z0-9.!#$%&'*+\/=?_`{\|}~-]+@)))/
}, _g = {
  ...Er,
  br: L(jl).replace("{2,}", "*").getRegex(),
  text: L(Er.text).replace("\\b_", "\\b_| {2,}\\n").replace(/\{2,\}/g, "*").getRegex()
}, Dn = {
  normal: ro,
  gfm: rg,
  pedantic: og
}, Wt = {
  normal: oo,
  gfm: Er,
  breaks: _g,
  pedantic: vg
};
class be {
  constructor(e) {
    g(this, "tokens");
    g(this, "options");
    g(this, "state");
    g(this, "tokenizer");
    g(this, "inlineQueue");
    this.tokens = [], this.tokens.links = /* @__PURE__ */ Object.create(null), this.options = e || gt, this.options.tokenizer = this.options.tokenizer || new ls(), this.tokenizer = this.options.tokenizer, this.tokenizer.options = this.options, this.tokenizer.lexer = this, this.inlineQueue = [], this.state = {
      inLink: !1,
      inRawBlock: !1,
      top: !0
    };
    const t = {
      block: Dn.normal,
      inline: Wt.normal
    };
    this.options.pedantic ? (t.block = Dn.pedantic, t.inline = Wt.pedantic) : this.options.gfm && (t.block = Dn.gfm, this.options.breaks ? t.inline = Wt.breaks : t.inline = Wt.gfm), this.tokenizer.rules = t;
  }
  /**
   * Expose Rules
   */
  static get rules() {
    return {
      block: Dn,
      inline: Wt
    };
  }
  /**
   * Static Lex Method
   */
  static lex(e, t) {
    return new be(t).lex(e);
  }
  /**
   * Static Lex Inline Method
   */
  static lexInline(e, t) {
    return new be(t).inlineTokens(e);
  }
  /**
   * Preprocessing
   */
  lex(e) {
    e = e.replace(/\r\n|\r/g, `
`), this.blockTokens(e, this.tokens);
    for (let t = 0; t < this.inlineQueue.length; t++) {
      const s = this.inlineQueue[t];
      this.inlineTokens(s.src, s.tokens);
    }
    return this.inlineQueue = [], this.tokens;
  }
  blockTokens(e, t = []) {
    this.options.pedantic ? e = e.replace(/\t/g, "    ").replace(/^ +$/gm, "") : e = e.replace(/^( *)(\t+)/gm, (a, l, c) => l + "    ".repeat(c.length));
    let s, r, o, i;
    for (; e; )
      if (!(this.options.extensions && this.options.extensions.block && this.options.extensions.block.some((a) => (s = a.call({ lexer: this }, e, t)) ? (e = e.substring(s.raw.length), t.push(s), !0) : !1))) {
        if (s = this.tokenizer.space(e)) {
          e = e.substring(s.raw.length), s.raw.length === 1 && t.length > 0 ? t[t.length - 1].raw += `
` : t.push(s);
          continue;
        }
        if (s = this.tokenizer.code(e)) {
          e = e.substring(s.raw.length), r = t[t.length - 1], r && (r.type === "paragraph" || r.type === "text") ? (r.raw += `
` + s.raw, r.text += `
` + s.text, this.inlineQueue[this.inlineQueue.length - 1].src = r.text) : t.push(s);
          continue;
        }
        if (s = this.tokenizer.fences(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.heading(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.hr(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.blockquote(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.list(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.html(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.def(e)) {
          e = e.substring(s.raw.length), r = t[t.length - 1], r && (r.type === "paragraph" || r.type === "text") ? (r.raw += `
` + s.raw, r.text += `
` + s.raw, this.inlineQueue[this.inlineQueue.length - 1].src = r.text) : this.tokens.links[s.tag] || (this.tokens.links[s.tag] = {
            href: s.href,
            title: s.title
          });
          continue;
        }
        if (s = this.tokenizer.table(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.lheading(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (o = e, this.options.extensions && this.options.extensions.startBlock) {
          let a = 1 / 0;
          const l = e.slice(1);
          let c;
          this.options.extensions.startBlock.forEach((h) => {
            c = h.call({ lexer: this }, l), typeof c == "number" && c >= 0 && (a = Math.min(a, c));
          }), a < 1 / 0 && a >= 0 && (o = e.substring(0, a + 1));
        }
        if (this.state.top && (s = this.tokenizer.paragraph(o))) {
          r = t[t.length - 1], i && r.type === "paragraph" ? (r.raw += `
` + s.raw, r.text += `
` + s.text, this.inlineQueue.pop(), this.inlineQueue[this.inlineQueue.length - 1].src = r.text) : t.push(s), i = o.length !== e.length, e = e.substring(s.raw.length);
          continue;
        }
        if (s = this.tokenizer.text(e)) {
          e = e.substring(s.raw.length), r = t[t.length - 1], r && r.type === "text" ? (r.raw += `
` + s.raw, r.text += `
` + s.text, this.inlineQueue.pop(), this.inlineQueue[this.inlineQueue.length - 1].src = r.text) : t.push(s);
          continue;
        }
        if (e) {
          const a = "Infinite loop on byte: " + e.charCodeAt(0);
          if (this.options.silent) {
            console.error(a);
            break;
          } else
            throw new Error(a);
        }
      }
    return this.state.top = !0, t;
  }
  inline(e, t = []) {
    return this.inlineQueue.push({ src: e, tokens: t }), t;
  }
  /**
   * Lexing/Compiling
   */
  inlineTokens(e, t = []) {
    let s, r, o, i = e, a, l, c;
    if (this.tokens.links) {
      const h = Object.keys(this.tokens.links);
      if (h.length > 0)
        for (; (a = this.tokenizer.rules.inline.reflinkSearch.exec(i)) != null; )
          h.includes(a[0].slice(a[0].lastIndexOf("[") + 1, -1)) && (i = i.slice(0, a.index) + "[" + "a".repeat(a[0].length - 2) + "]" + i.slice(this.tokenizer.rules.inline.reflinkSearch.lastIndex));
    }
    for (; (a = this.tokenizer.rules.inline.blockSkip.exec(i)) != null; )
      i = i.slice(0, a.index) + "[" + "a".repeat(a[0].length - 2) + "]" + i.slice(this.tokenizer.rules.inline.blockSkip.lastIndex);
    for (; (a = this.tokenizer.rules.inline.anyPunctuation.exec(i)) != null; )
      i = i.slice(0, a.index) + "++" + i.slice(this.tokenizer.rules.inline.anyPunctuation.lastIndex);
    for (; e; )
      if (l || (c = ""), l = !1, !(this.options.extensions && this.options.extensions.inline && this.options.extensions.inline.some((h) => (s = h.call({ lexer: this }, e, t)) ? (e = e.substring(s.raw.length), t.push(s), !0) : !1))) {
        if (s = this.tokenizer.escape(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.tag(e)) {
          e = e.substring(s.raw.length), r = t[t.length - 1], r && s.type === "text" && r.type === "text" ? (r.raw += s.raw, r.text += s.text) : t.push(s);
          continue;
        }
        if (s = this.tokenizer.link(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.reflink(e, this.tokens.links)) {
          e = e.substring(s.raw.length), r = t[t.length - 1], r && s.type === "text" && r.type === "text" ? (r.raw += s.raw, r.text += s.text) : t.push(s);
          continue;
        }
        if (s = this.tokenizer.emStrong(e, i, c)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.codespan(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.br(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.del(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (s = this.tokenizer.autolink(e)) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (!this.state.inLink && (s = this.tokenizer.url(e))) {
          e = e.substring(s.raw.length), t.push(s);
          continue;
        }
        if (o = e, this.options.extensions && this.options.extensions.startInline) {
          let h = 1 / 0;
          const u = e.slice(1);
          let p;
          this.options.extensions.startInline.forEach((d) => {
            p = d.call({ lexer: this }, u), typeof p == "number" && p >= 0 && (h = Math.min(h, p));
          }), h < 1 / 0 && h >= 0 && (o = e.substring(0, h + 1));
        }
        if (s = this.tokenizer.inlineText(o)) {
          e = e.substring(s.raw.length), s.raw.slice(-1) !== "_" && (c = s.raw.slice(-1)), l = !0, r = t[t.length - 1], r && r.type === "text" ? (r.raw += s.raw, r.text += s.text) : t.push(s);
          continue;
        }
        if (e) {
          const h = "Infinite loop on byte: " + e.charCodeAt(0);
          if (this.options.silent) {
            console.error(h);
            break;
          } else
            throw new Error(h);
        }
      }
    return t;
  }
}
class us {
  constructor(e) {
    g(this, "options");
    this.options = e || gt;
  }
  code(e, t, s) {
    var o;
    const r = (o = (t || "").match(/^\S*/)) == null ? void 0 : o[0];
    return e = e.replace(/\n$/, "") + `
`, r ? '<pre><code class="language-' + se(r) + '">' + (s ? e : se(e, !0)) + `</code></pre>
` : "<pre><code>" + (s ? e : se(e, !0)) + `</code></pre>
`;
  }
  blockquote(e) {
    return `<blockquote>
${e}</blockquote>
`;
  }
  html(e, t) {
    return e;
  }
  heading(e, t, s) {
    return `<h${t}>${e}</h${t}>
`;
  }
  hr() {
    return `<hr>
`;
  }
  list(e, t, s) {
    const r = t ? "ol" : "ul", o = t && s !== 1 ? ' start="' + s + '"' : "";
    return "<" + r + o + `>
` + e + "</" + r + `>
`;
  }
  listitem(e, t, s) {
    return `<li>${e}</li>
`;
  }
  checkbox(e) {
    return "<input " + (e ? 'checked="" ' : "") + 'disabled="" type="checkbox">';
  }
  paragraph(e) {
    return `<p>${e}</p>
`;
  }
  table(e, t) {
    return t && (t = `<tbody>${t}</tbody>`), `<table>
<thead>
` + e + `</thead>
` + t + `</table>
`;
  }
  tablerow(e) {
    return `<tr>
${e}</tr>
`;
  }
  tablecell(e, t) {
    const s = t.header ? "th" : "td";
    return (t.align ? `<${s} align="${t.align}">` : `<${s}>`) + e + `</${s}>
`;
  }
  /**
   * span level renderer
   */
  strong(e) {
    return `<strong>${e}</strong>`;
  }
  em(e) {
    return `<em>${e}</em>`;
  }
  codespan(e) {
    return `<code>${e}</code>`;
  }
  br() {
    return "<br>";
  }
  del(e) {
    return `<del>${e}</del>`;
  }
  link(e, t, s) {
    const r = ea(e);
    if (r === null)
      return s;
    e = r;
    let o = '<a href="' + e + '"';
    return t && (o += ' title="' + t + '"'), o += ">" + s + "</a>", o;
  }
  image(e, t, s) {
    const r = ea(e);
    if (r === null)
      return s;
    e = r;
    let o = `<img src="${e}" alt="${s}"`;
    return t && (o += ` title="${t}"`), o += ">", o;
  }
  text(e) {
    return e;
  }
}
class io {
  // no need for block level renderers
  strong(e) {
    return e;
  }
  em(e) {
    return e;
  }
  codespan(e) {
    return e;
  }
  del(e) {
    return e;
  }
  html(e) {
    return e;
  }
  text(e) {
    return e;
  }
  link(e, t, s) {
    return "" + s;
  }
  image(e, t, s) {
    return "" + s;
  }
  br() {
    return "";
  }
}
class ye {
  constructor(e) {
    g(this, "options");
    g(this, "renderer");
    g(this, "textRenderer");
    this.options = e || gt, this.options.renderer = this.options.renderer || new us(), this.renderer = this.options.renderer, this.renderer.options = this.options, this.textRenderer = new io();
  }
  /**
   * Static Parse Method
   */
  static parse(e, t) {
    return new ye(t).parse(e);
  }
  /**
   * Static Parse Inline Method
   */
  static parseInline(e, t) {
    return new ye(t).parseInline(e);
  }
  /**
   * Parse Loop
   */
  parse(e, t = !0) {
    let s = "";
    for (let r = 0; r < e.length; r++) {
      const o = e[r];
      if (this.options.extensions && this.options.extensions.renderers && this.options.extensions.renderers[o.type]) {
        const i = o, a = this.options.extensions.renderers[i.type].call({ parser: this }, i);
        if (a !== !1 || !["space", "hr", "heading", "code", "table", "blockquote", "list", "html", "paragraph", "text"].includes(i.type)) {
          s += a || "";
          continue;
        }
      }
      switch (o.type) {
        case "space":
          continue;
        case "hr": {
          s += this.renderer.hr();
          continue;
        }
        case "heading": {
          const i = o;
          s += this.renderer.heading(this.parseInline(i.tokens), i.depth, Hf(this.parseInline(i.tokens, this.textRenderer)));
          continue;
        }
        case "code": {
          const i = o;
          s += this.renderer.code(i.text, i.lang, !!i.escaped);
          continue;
        }
        case "table": {
          const i = o;
          let a = "", l = "";
          for (let h = 0; h < i.header.length; h++)
            l += this.renderer.tablecell(this.parseInline(i.header[h].tokens), { header: !0, align: i.align[h] });
          a += this.renderer.tablerow(l);
          let c = "";
          for (let h = 0; h < i.rows.length; h++) {
            const u = i.rows[h];
            l = "";
            for (let p = 0; p < u.length; p++)
              l += this.renderer.tablecell(this.parseInline(u[p].tokens), { header: !1, align: i.align[p] });
            c += this.renderer.tablerow(l);
          }
          s += this.renderer.table(a, c);
          continue;
        }
        case "blockquote": {
          const i = o, a = this.parse(i.tokens);
          s += this.renderer.blockquote(a);
          continue;
        }
        case "list": {
          const i = o, a = i.ordered, l = i.start, c = i.loose;
          let h = "";
          for (let u = 0; u < i.items.length; u++) {
            const p = i.items[u], d = p.checked, f = p.task;
            let b = "";
            if (p.task) {
              const v = this.renderer.checkbox(!!d);
              c ? p.tokens.length > 0 && p.tokens[0].type === "paragraph" ? (p.tokens[0].text = v + " " + p.tokens[0].text, p.tokens[0].tokens && p.tokens[0].tokens.length > 0 && p.tokens[0].tokens[0].type === "text" && (p.tokens[0].tokens[0].text = v + " " + p.tokens[0].tokens[0].text)) : p.tokens.unshift({
                type: "text",
                text: v + " "
              }) : b += v + " ";
            }
            b += this.parse(p.tokens, c), h += this.renderer.listitem(b, f, !!d);
          }
          s += this.renderer.list(h, a, l);
          continue;
        }
        case "html": {
          const i = o;
          s += this.renderer.html(i.text, i.block);
          continue;
        }
        case "paragraph": {
          const i = o;
          s += this.renderer.paragraph(this.parseInline(i.tokens));
          continue;
        }
        case "text": {
          let i = o, a = i.tokens ? this.parseInline(i.tokens) : i.text;
          for (; r + 1 < e.length && e[r + 1].type === "text"; )
            i = e[++r], a += `
` + (i.tokens ? this.parseInline(i.tokens) : i.text);
          s += t ? this.renderer.paragraph(a) : a;
          continue;
        }
        default: {
          const i = 'Token with "' + o.type + '" type was not found.';
          if (this.options.silent)
            return console.error(i), "";
          throw new Error(i);
        }
      }
    }
    return s;
  }
  /**
   * Parse Inline Tokens
   */
  parseInline(e, t) {
    t = t || this.renderer;
    let s = "";
    for (let r = 0; r < e.length; r++) {
      const o = e[r];
      if (this.options.extensions && this.options.extensions.renderers && this.options.extensions.renderers[o.type]) {
        const i = this.options.extensions.renderers[o.type].call({ parser: this }, o);
        if (i !== !1 || !["escape", "html", "link", "image", "strong", "em", "codespan", "br", "del", "text"].includes(o.type)) {
          s += i || "";
          continue;
        }
      }
      switch (o.type) {
        case "escape": {
          const i = o;
          s += t.text(i.text);
          break;
        }
        case "html": {
          const i = o;
          s += t.html(i.text);
          break;
        }
        case "link": {
          const i = o;
          s += t.link(i.href, i.title, this.parseInline(i.tokens, t));
          break;
        }
        case "image": {
          const i = o;
          s += t.image(i.href, i.title, i.text);
          break;
        }
        case "strong": {
          const i = o;
          s += t.strong(this.parseInline(i.tokens, t));
          break;
        }
        case "em": {
          const i = o;
          s += t.em(this.parseInline(i.tokens, t));
          break;
        }
        case "codespan": {
          const i = o;
          s += t.codespan(i.text);
          break;
        }
        case "br": {
          s += t.br();
          break;
        }
        case "del": {
          const i = o;
          s += t.del(this.parseInline(i.tokens, t));
          break;
        }
        case "text": {
          const i = o;
          s += t.text(i.text);
          break;
        }
        default: {
          const i = 'Token with "' + o.type + '" type was not found.';
          if (this.options.silent)
            return console.error(i), "";
          throw new Error(i);
        }
      }
    }
    return s;
  }
}
class en {
  constructor(e) {
    g(this, "options");
    this.options = e || gt;
  }
  /**
   * Process markdown before marked
   */
  preprocess(e) {
    return e;
  }
  /**
   * Process HTML after marked is finished
   */
  postprocess(e) {
    return e;
  }
  /**
   * Process all tokens before walk tokens
   */
  processAllTokens(e) {
    return e;
  }
}
g(en, "passThroughHooks", /* @__PURE__ */ new Set([
  "preprocess",
  "postprocess",
  "processAllTokens"
]));
var ht, Rr, Wl;
class wg {
  constructor(...e) {
    Pe(this, ht);
    g(this, "defaults", eo());
    g(this, "options", this.setOptions);
    g(this, "parse", yt(this, ht, Rr).call(this, be.lex, ye.parse));
    g(this, "parseInline", yt(this, ht, Rr).call(this, be.lexInline, ye.parseInline));
    g(this, "Parser", ye);
    g(this, "Renderer", us);
    g(this, "TextRenderer", io);
    g(this, "Lexer", be);
    g(this, "Tokenizer", ls);
    g(this, "Hooks", en);
    this.use(...e);
  }
  /**
   * Run callback for every token
   */
  walkTokens(e, t) {
    var r, o;
    let s = [];
    for (const i of e)
      switch (s = s.concat(t.call(this, i)), i.type) {
        case "table": {
          const a = i;
          for (const l of a.header)
            s = s.concat(this.walkTokens(l.tokens, t));
          for (const l of a.rows)
            for (const c of l)
              s = s.concat(this.walkTokens(c.tokens, t));
          break;
        }
        case "list": {
          const a = i;
          s = s.concat(this.walkTokens(a.items, t));
          break;
        }
        default: {
          const a = i;
          (o = (r = this.defaults.extensions) == null ? void 0 : r.childTokens) != null && o[a.type] ? this.defaults.extensions.childTokens[a.type].forEach((l) => {
            const c = a[l].flat(1 / 0);
            s = s.concat(this.walkTokens(c, t));
          }) : a.tokens && (s = s.concat(this.walkTokens(a.tokens, t)));
        }
      }
    return s;
  }
  use(...e) {
    const t = this.defaults.extensions || { renderers: {}, childTokens: {} };
    return e.forEach((s) => {
      const r = { ...s };
      if (r.async = this.defaults.async || r.async || !1, s.extensions && (s.extensions.forEach((o) => {
        if (!o.name)
          throw new Error("extension name required");
        if ("renderer" in o) {
          const i = t.renderers[o.name];
          i ? t.renderers[o.name] = function(...a) {
            let l = o.renderer.apply(this, a);
            return l === !1 && (l = i.apply(this, a)), l;
          } : t.renderers[o.name] = o.renderer;
        }
        if ("tokenizer" in o) {
          if (!o.level || o.level !== "block" && o.level !== "inline")
            throw new Error("extension level must be 'block' or 'inline'");
          const i = t[o.level];
          i ? i.unshift(o.tokenizer) : t[o.level] = [o.tokenizer], o.start && (o.level === "block" ? t.startBlock ? t.startBlock.push(o.start) : t.startBlock = [o.start] : o.level === "inline" && (t.startInline ? t.startInline.push(o.start) : t.startInline = [o.start]));
        }
        "childTokens" in o && o.childTokens && (t.childTokens[o.name] = o.childTokens);
      }), r.extensions = t), s.renderer) {
        const o = this.defaults.renderer || new us(this.defaults);
        for (const i in s.renderer) {
          if (!(i in o))
            throw new Error(`renderer '${i}' does not exist`);
          if (i === "options")
            continue;
          const a = i, l = s.renderer[a], c = o[a];
          o[a] = (...h) => {
            let u = l.apply(o, h);
            return u === !1 && (u = c.apply(o, h)), u || "";
          };
        }
        r.renderer = o;
      }
      if (s.tokenizer) {
        const o = this.defaults.tokenizer || new ls(this.defaults);
        for (const i in s.tokenizer) {
          if (!(i in o))
            throw new Error(`tokenizer '${i}' does not exist`);
          if (["options", "rules", "lexer"].includes(i))
            continue;
          const a = i, l = s.tokenizer[a], c = o[a];
          o[a] = (...h) => {
            let u = l.apply(o, h);
            return u === !1 && (u = c.apply(o, h)), u;
          };
        }
        r.tokenizer = o;
      }
      if (s.hooks) {
        const o = this.defaults.hooks || new en();
        for (const i in s.hooks) {
          if (!(i in o))
            throw new Error(`hook '${i}' does not exist`);
          if (i === "options")
            continue;
          const a = i, l = s.hooks[a], c = o[a];
          en.passThroughHooks.has(i) ? o[a] = (h) => {
            if (this.defaults.async)
              return Promise.resolve(l.call(o, h)).then((p) => c.call(o, p));
            const u = l.call(o, h);
            return c.call(o, u);
          } : o[a] = (...h) => {
            let u = l.apply(o, h);
            return u === !1 && (u = c.apply(o, h)), u;
          };
        }
        r.hooks = o;
      }
      if (s.walkTokens) {
        const o = this.defaults.walkTokens, i = s.walkTokens;
        r.walkTokens = function(a) {
          let l = [];
          return l.push(i.call(this, a)), o && (l = l.concat(o.call(this, a))), l;
        };
      }
      this.defaults = { ...this.defaults, ...r };
    }), this;
  }
  setOptions(e) {
    return this.defaults = { ...this.defaults, ...e }, this;
  }
  lexer(e, t) {
    return be.lex(e, t ?? this.defaults);
  }
  parser(e, t) {
    return ye.parse(e, t ?? this.defaults);
  }
}
ht = new WeakSet(), Rr = function(e, t) {
  return (s, r) => {
    const o = { ...r }, i = { ...this.defaults, ...o };
    this.defaults.async === !0 && o.async === !1 && (i.silent || console.warn("marked(): The async option was set to true by an extension. The async: false option sent to parse will be ignored."), i.async = !0);
    const a = yt(this, ht, Wl).call(this, !!i.silent, !!i.async);
    if (typeof s > "u" || s === null)
      return a(new Error("marked(): input parameter is undefined or null"));
    if (typeof s != "string")
      return a(new Error("marked(): input parameter is of type " + Object.prototype.toString.call(s) + ", string expected"));
    if (i.hooks && (i.hooks.options = i), i.async)
      return Promise.resolve(i.hooks ? i.hooks.preprocess(s) : s).then((l) => e(l, i)).then((l) => i.hooks ? i.hooks.processAllTokens(l) : l).then((l) => i.walkTokens ? Promise.all(this.walkTokens(l, i.walkTokens)).then(() => l) : l).then((l) => t(l, i)).then((l) => i.hooks ? i.hooks.postprocess(l) : l).catch(a);
    try {
      i.hooks && (s = i.hooks.preprocess(s));
      let l = e(s, i);
      i.hooks && (l = i.hooks.processAllTokens(l)), i.walkTokens && this.walkTokens(l, i.walkTokens);
      let c = t(l, i);
      return i.hooks && (c = i.hooks.postprocess(c)), c;
    } catch (l) {
      return a(l);
    }
  };
}, Wl = function(e, t) {
  return (s) => {
    if (s.message += `
Please report this to https://github.com/markedjs/marked.`, e) {
      const r = "<p>An error occurred:</p><pre>" + se(s.message + "", !0) + "</pre>";
      return t ? Promise.resolve(r) : r;
    }
    if (t)
      return Promise.reject(s);
    throw s;
  };
};
const ct = new wg();
function P(n, e) {
  return ct.parse(n, e);
}
P.options = P.setOptions = function(n) {
  return ct.setOptions(n), P.defaults = ct.defaults, Ol(P.defaults), P;
};
P.getDefaults = eo;
P.defaults = gt;
P.use = function(...n) {
  return ct.use(...n), P.defaults = ct.defaults, Ol(P.defaults), P;
};
P.walkTokens = function(n, e) {
  return ct.walkTokens(n, e);
};
P.parseInline = ct.parseInline;
P.Parser = ye;
P.parser = ye.parse;
P.Renderer = us;
P.TextRenderer = io;
P.Lexer = be;
P.lexer = be.lex;
P.Tokenizer = ls;
P.Hooks = en;
P.parse = P;
P.options;
P.setOptions;
P.use;
P.walkTokens;
P.parseInline;
ye.parse;
be.lex;
var xg = Object.defineProperty, ao = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && xg(e, t, r), r;
};
const Eo = class Eo extends C {
  constructor() {
    super(...arguments), this.value = "", this.testid = "", this._html = "", this._slotText = "", this._onSlot = (e) => {
      this._slotText = e.target.assignedNodes({ flatten: !0 }).map((t) => t.textContent ?? "").join(""), this.value || this._render();
    };
  }
  updated(e) {
    e.has("value") && this._render();
  }
  async _render() {
    const e = this.value || this._slotText;
    let t;
    try {
      t = await P.parse(e, { gfm: !0 }), t = await this._highlightFences(t);
    } catch {
      t = `<pre>${K(e)}</pre>`;
    }
    this._html = Cg(t);
  }
  /** Upgrade ```lang fenced blocks to Shiki-highlighted markup (best-effort). */
  async _highlightFences(e) {
    const t = /<pre><code class="language-([\w-]+)">([\s\S]*?)<\/code><\/pre>/g, s = [], r = [];
    let o = 0, i;
    for (; i = t.exec(e); ) {
      const [a, l, c] = i;
      s.push(e.slice(o, i.index));
      const h = s.push(a) - 1;
      if (o = i.index + a.length, Ft(l)) {
        const u = kg(c);
        r.push(
          Pl(u, l).then((p) => {
            s[h] = `<pre><code>${p}</code></pre>`;
          }).catch(() => {
          })
        );
      }
    }
    return s.push(e.slice(o)), await Promise.all(r), s.join("");
  }
  render() {
    return w`
      <div part="content" data-testid=${X(this.testid || void 0)}>${an(this._html)}</div>
      <slot @slotchange=${this._onSlot} hidden></slot>
    `;
  }
};
Eo.styles = [
  C.base,
  q($n),
  I`
      :host {
        display: block;
        color: var(--text-primary, #d8d0c0);
      }
      [part='content'] {
        font-family: var(--sans, system-ui, sans-serif);
        line-height: 1.6;
      }
      [part='content'] > :first-child {
        margin-top: 0;
      }
      [part='content'] > :last-child {
        margin-bottom: 0;
      }
      h1,
      h2,
      h3,
      h4 {
        font-family: var(--serif, 'Cormorant', Georgia, serif);
        line-height: 1.2;
        font-weight: 600;
      }
      code {
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: 0.9em;
        background: var(--bg-editor, #0a0a0a);
        padding: 0.1em 0.3em;
        border-radius: 3px;
      }
      pre {
        background: var(--bg-editor, #0a0a0a);
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-sm, 4px);
        padding: var(--space-md, 12px);
        overflow-x: auto;
      }
      pre code {
        background: none;
        padding: 0;
        font-size: var(--text-md, 13px);
      }
      a {
        color: var(--gold, #d4a537);
      }
      table {
        border-collapse: collapse;
      }
      th,
      td {
        border: 1px solid var(--border, #1e1e1e);
        padding: 0.3em 0.6em;
      }
      blockquote {
        margin: 0.6em 0;
        padding-left: 0.9em;
        border-left: 2px solid var(--border, #1e1e1e);
        color: var(--text-secondary, #a89f8c);
      }
    `
];
let Tt = Eo;
ao([
  m()
], Tt.prototype, "value");
ao([
  m()
], Tt.prototype, "testid");
ao([
  Qe()
], Tt.prototype, "_html");
function kg(n) {
  return n.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&amp;/g, "&");
}
function Cg(n) {
  const e = document.createElement("template");
  return e.innerHTML = n, e.content.querySelectorAll("script, style, iframe, object, embed").forEach((t) => t.remove()), e.content.querySelectorAll("*").forEach((t) => {
    for (const s of Array.from(t.attributes)) {
      const r = s.name.toLowerCase(), o = s.value.trim().toLowerCase();
      (r.startsWith("on") || (r === "href" || r === "src") && o.startsWith("javascript:")) && t.removeAttribute(s.name);
    }
    t.tagName === "A" && (t.setAttribute("rel", "noopener noreferrer"), t.setAttribute("target", "_blank"));
  }), e.innerHTML;
}
customElements.define("sema-markdown", Tt);
var $g = Object.defineProperty, Q = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && $g(e, t, r), r;
};
const Sg = w`<svg
  class="logo-svg"
  viewBox="0 0 366 132"
  role="img"
  aria-label="Sema"
  xmlns="http://www.w3.org/2000/svg"
>
  <path
    class="logo-bracket"
    d="M48.5 104.3L48.5 114Q34 110.7 26.05 100.5Q18.1 90.3 18.1 75L18.1 57Q18.1 41.7 26.05 31.5Q34 21.3 48.5 18L48.5 27.6Q42.2 29.1 37.6 33.15Q33 37.2 30.5 43.3Q28 49.4 28 57L28 75Q28 82.6 30.5 88.65Q33 94.7 37.6 98.75Q42.2 102.8 48.5 104.3"
  />
  <path
    class="logo-word"
    d="M93.2 102.8L88.8 102.8Q79.4 102.8 74.2 98.6Q69 94.4 69 86.8L78.8 86.8Q78.8 90.4 81.45 92.45Q84.1 94.5 88.8 94.5L93.2 94.5Q98.1 94.5 100.75 92.4Q103.4 90.3 103.4 86.5Q103.4 79.8 96.8 79L82 76.9Q76.1 76 72.9 72.05Q69.7 68.1 69.7 61.8Q69.7 54.4 74.7 50.3Q79.7 46.2 88.7 46.2L93.1 46.2Q101.5 46.2 106.7 50.2Q111.9 54.2 112.2 60.8L102.2 60.8Q102 58 99.6 56.15Q97.2 54.3 93.1 54.3L88.7 54.3Q84.2 54.3 81.75 56.3Q79.3 58.3 79.3 61.7Q79.3 67.2 84.8 67.9L98.7 69.9Q113 71.8 113 86.5Q113 94.3 107.85 98.55Q102.7 102.8 93.2 102.8 M152 103Q142.1 103 136.05 97.1Q130 91.2 130 81L130 68Q130 57.8 136.05 51.9Q142.1 46 152 46Q158.6 46 163.55 48.65Q168.5 51.3 171.25 56.05Q174 60.8 174 67.1L174 77L139.7 77L139.7 81.8Q139.7 87.8 143 91.2Q146.3 94.6 152 94.6Q156.8 94.6 159.9 92.8Q163 91 163.6 87.8L173.5 87.8Q172.5 94.8 166.6 98.9Q160.7 103 152 103M139.7 67.1L139.7 69.7L164.3 69.7L164.3 67.1Q164.3 60.8 161.1 57.4Q157.9 54 152 54Q146.1 54 142.9 57.4Q139.7 60.8 139.7 67.1 M197.7 102L188.7 102L188.7 47L197.1 47L197.1 54.5L197.4 54.5Q197.8 50.7 200.25 48.35Q202.7 46 206.5 46Q210.2 46 212.7 48.2Q215.2 50.4 216.3 54.1Q216.9 50.3 219.4 48.15Q221.9 46 225.7 46Q230.9 46 234.1 49.95Q237.3 53.9 237.3 60.2L237.3 102L228.3 102L228.3 60.3Q228.3 57.2 226.75 55.35Q225.2 53.5 222.6 53.5Q220 53.5 218.5 55.3Q217 57.1 217 60.3L217 102L209 102L209 60.3Q209 57.2 207.5 55.35Q206 53.5 203.4 53.5Q200.8 53.5 199.25 55.3Q197.7 57.1 197.7 60.3 M268.9 103Q260.4 103 255.45 98.25Q250.5 93.5 250.5 85.8Q250.5 78.1 255.65 73.4Q260.8 68.7 269.2 68.7L285.3 68.7L285.3 64.5Q285.3 54.4 274.1 54.4Q269.1 54.4 266.05 56.25Q263 58.1 262.8 61.4L253 61.4Q253.5 54.7 259.1 50.35Q264.7 46 274.1 46Q284.2 46 289.7 50.8Q295.2 55.6 295.2 64.3L295.2 102L285.5 102L285.5 91.9L285.3 91.9Q284.6 97 280.25 100Q275.9 103 268.9 103M271.5 94.7Q277.8 94.7 281.55 91.65Q285.3 88.6 285.3 83.3L285.3 76L270.1 76Q265.8 76 263.15 78.55Q260.5 81.1 260.5 85.3Q260.5 89.6 263.4 92.15Q266.3 94.7 271.5 94.7"
  />
  <path
    class="logo-bracket"
    d="M316.5 114L316.5 104.3Q322.8 102.8 327.4 98.75Q332 94.7 334.55 88.65Q337.1 82.6 337.1 75L337.1 57Q337.1 49.4 334.55 43.3Q332 37.2 327.4 33.15Q322.8 29.1 316.5 27.6L316.5 18Q331 21.3 339 31.5Q347 41.7 347 57L347 75Q347 90.3 339 100.5Q331 110.7 316.5 114"
  />
</svg>`;
function Ag(n, e) {
  const t = [[]];
  let s = 0;
  for (const r of n) {
    if (s >= e) break;
    const o = r.text.length > e - s ? r.text.slice(0, e - s) : r.text;
    s += o.length, o.split(`
`).forEach((a, l) => {
      l > 0 && t.push([]), a && t[t.length - 1].push({ text: a, cls: r.cls });
    });
  }
  return t;
}
const Ro = class Ro extends C {
  constructor() {
    super(...arguments), this.lang = "sema", this.cps = 45, this.startDelay = 400, this.loop = !1, this.loopDelay = 1500, this.autoplay = !0, this.frame = !1, this.logo = !1, this.lineNumbers = !1, this.status = !1, this.filename = "", this.rows = 0, this.noDedent = !1, this._revealed = 0, this._index = 0, this._raw = "", this._code = "", this._total = 0, this._raf = 0, this._timer = 0, this._startAt = 0, this._playing = !1, this._reduce = !1, this._tokensTask = new va(this, {
      task: async ([e, t, s]) => {
        const r = s ? e.replace(/^\n/, "").replace(/\s+$/, "") : Vn(e);
        this._code = r, this._total = r.length;
        const o = Ft(t) ? await Pf(r, t) : r ? [{ text: r, cls: "" }] : [];
        return this._begin(), o;
      },
      args: () => [this._sources()[this._index % this._sources().length] ?? "", this.lang, this.noDedent]
    }), this._onSlotChange = (e) => {
      const t = e.target;
      this._raw = t.assignedNodes({ flatten: !0 }).map((s) => s.textContent ?? "").join(""), this._index = 0, this.requestUpdate();
    }, this._tick = (e) => {
      if (!this._playing) return;
      const t = Math.max(0, e - this._startAt) / 1e3;
      if (this._revealed = Math.min(this._total, Math.floor(t * this.cps)), this._revealed >= this._total) {
        this._playing = !1, this.dispatchEvent(new CustomEvent("sema-typer-done", { bubbles: !0, composed: !0 })), this._afterComplete();
        return;
      }
      this._raf = requestAnimationFrame(this._tick);
    };
  }
  connectedCallback() {
    super.connectedCallback(), this._reduce = typeof matchMedia < "u" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  }
  disconnectedCallback() {
    super.disconnectedCallback(), this._stop(), clearTimeout(this._timer);
  }
  _sources() {
    var e;
    return (e = this.snippets) != null && e.length ? this.snippets : [this._raw];
  }
  // --- typing engine ---
  _begin() {
    if (this._stop(), this._total === 0) {
      this._revealed = 0;
      return;
    }
    if (this._reduce) {
      this._revealed = this._total;
      return;
    }
    this._revealed = 0, this.autoplay && (this._startAt = performance.now() + this.startDelay, this._playing = !0, this._raf = requestAnimationFrame(this._tick));
  }
  _afterComplete() {
    const e = this._sources();
    if (e.length > 1) {
      const t = this._index + 1;
      (t < e.length || this.loop) && (this._timer = window.setTimeout(() => {
        this._index = t % e.length;
      }, this.loopDelay));
    } else this.loop && (this._timer = window.setTimeout(() => this._begin(), this.loopDelay));
  }
  _stop() {
    cancelAnimationFrame(this._raf), this._raf = 0, this._playing = !1;
  }
  // --- imperative API ---
  /** Total character count of the active (dedented) snippet — available once tokenized. */
  get total() {
    return this._total;
  }
  /** Resume typing from the current caret. */
  play() {
    this._playing || this._revealed >= this._total || (this._startAt = performance.now() - this._revealed / this.cps * 1e3, this._playing = !0, this._raf = requestAnimationFrame(this._tick));
  }
  /** Pause typing. */
  pause() {
    this._stop();
  }
  /** Reset to the start of the current snippet and play. */
  restart() {
    this._stop(), this._revealed = 0, this._startAt = performance.now() + this.startDelay, this._playing = !0, this._raf = requestAnimationFrame(this._tick);
  }
  /** Jump to character `n` (clamped) and pause there. */
  seek(e) {
    this._stop(), this._revealed = Math.max(0, Math.min(this._total, Math.floor(e)));
  }
  _pos() {
    var r;
    const e = this._code.slice(0, this._revealed), t = e.lastIndexOf(`
`);
    return { ln: (((r = e.match(/\n/g)) == null ? void 0 : r.length) ?? 0) + 1, col: this._revealed - (t + 1) + 1 };
  }
  updated() {
    if (this.rows > 0) {
      const e = this.renderRoot.querySelector(".viewport");
      e && (e.scrollTop = e.scrollHeight);
    }
  }
  _renderEditor() {
    const e = this._tokensTask.value ?? [], t = Ag(e, this._reduce ? this._total : this._revealed), s = this.rows > 0 ? `height:${this.rows * 1.5}em` : "";
    return w`<div class="viewport" style=${s}><code class="code" part="code" aria-label=${`${this.lang} code`}
      >${t.map(
      (r, o) => w`<span class="cl"
            >${r.map((i) => i.cls ? w`<span class="${i.cls}">${i.text}</span>` : i.text)}${o === t.length - 1 ? w`<span class="cursor" aria-hidden="true"></span>` : R}</span
          >`
    )}</code></div>`;
  }
  render() {
    const e = w`<slot @slotchange=${this._onSlotChange}></slot>`;
    if (!this.frame) return w`${this._renderEditor()}${e}`;
    const { ln: t, col: s } = this._pos();
    return w`
      <div class="frame ${this.logo ? "has-logo" : ""}">
        <span class="legend" part="legend"
          ><slot name="legend"
            >${this.logo ? Sg : w`<span class="lpar">(</span> <span class="lname">sema</span> <span class="lpar">)</span>`}</slot
          ></span>
        ${this._renderEditor()}
        ${this.status ? w`<div class="status" part="status">
              <slot name="status"
                ><span class="mode">EDIT</span><span class="fname">${this.filename}</span
                ><span class="spacer"></span><span class="pos">${t}:${s}</span><span class="seg">LF</span></slot
              >
            </div>` : R}
        ${e}
      </div>
    `;
  }
};
Ro.styles = [
  C.base,
  q($n),
  I`
      :host {
        display: block;
      }
      /* The legend straddles the top border (negative top); reserve space above the
         frame so it's never clipped — including in element screenshots / GIF export. */
      :host([frame]) {
        padding-top: 0.9em;
      }
      :host([frame][logo]) {
        padding-top: 1.35em;
      }
      .frame {
        position: relative;
        border: 1px solid var(--syntax-punctuation, #6a6258);
        border-radius: var(--radius-lg, 6px);
        background: var(--bg, #131110);
        padding: 16px 14px 9px;
      }
      .legend {
        position: absolute;
        top: -0.72em;
        left: 16px;
        padding: 0 9px;
        background: var(--bg, #131110);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-md, 13px);
        font-weight: 600;
        line-height: 1;
      }
      .lpar {
        color: var(--gold, #c8a855);
      }
      .lname {
        color: var(--text-primary, #e9e3d6);
      }
      .legend .logo-svg {
        display: block;
        height: 1.25em;
        width: auto;
      }
      /* Wordmark colors come from tokens (gold brackets, light wordmark). */
      .logo-bracket {
        fill: var(--gold, #c8a855);
      }
      .logo-word {
        fill: var(--logo-fg, #ffffff);
      }
      .frame.has-logo .legend {
        top: -0.95em;
        padding: 0 10px;
      }
      /* let a custom status slot's children participate in the status flex row */
      .status slot {
        display: contents;
      }
      .viewport {
        overflow: hidden;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-md, 13px);
        line-height: 1.5;
        color: var(--text-primary, #e9e3d6);
        tab-size: 2;
      }
      .code {
        display: block;
      }
      .cl {
        display: block;
        white-space: pre;
        min-height: 1.5em;
      }
      :host([line-numbers]) .code {
        counter-reset: ln;
      }
      :host([line-numbers]) .cl {
        padding-left: 3em;
        position: relative;
      }
      :host([line-numbers]) .cl::before {
        counter-increment: ln;
        content: counter(ln);
        position: absolute;
        left: 0;
        width: 2.2em;
        text-align: right;
        color: var(--text-tertiary, #5a5448);
        user-select: none;
      }
      .cursor {
        display: inline-block;
        width: 0.6ch;
        height: 1.05em;
        vertical-align: text-bottom;
        margin-left: 1px;
        background: var(--gold, #c8a855);
        animation: blink 1s steps(1) infinite;
      }
      @keyframes blink {
        50% {
          opacity: 0;
        }
      }
      .status {
        display: flex;
        align-items: center;
        margin-top: 8px;
        padding-top: 6px;
        border-top: 1px solid var(--border, #2b2620);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xs, 11px);
        color: var(--text-secondary, #968c79);
      }
      .status .mode {
        color: var(--gold, #c8a855);
        font-weight: 600;
        margin-right: 10px;
      }
      .status .spacer {
        flex: 1;
      }
      .status .seg,
      .status .pos {
        margin-left: 12px;
      }
      slot:not([name]) {
        display: none;
      }
    `
];
let F = Ro;
Q([
  m({ reflect: !0 })
], F.prototype, "lang");
Q([
  m({ type: Number })
], F.prototype, "cps");
Q([
  m({ type: Number, attribute: "start-delay" })
], F.prototype, "startDelay");
Q([
  m({ type: Boolean, reflect: !0 })
], F.prototype, "loop");
Q([
  m({ type: Number, attribute: "loop-delay" })
], F.prototype, "loopDelay");
Q([
  m({ type: Boolean, reflect: !0 })
], F.prototype, "autoplay");
Q([
  m({ type: Boolean, reflect: !0 })
], F.prototype, "frame");
Q([
  m({ type: Boolean, reflect: !0 })
], F.prototype, "logo");
Q([
  m({ type: Boolean, reflect: !0, attribute: "line-numbers" })
], F.prototype, "lineNumbers");
Q([
  m({ type: Boolean, reflect: !0 })
], F.prototype, "status");
Q([
  m({ reflect: !0 })
], F.prototype, "filename");
Q([
  m({ type: Number })
], F.prototype, "rows");
Q([
  m({ type: Boolean, reflect: !0, attribute: "no-dedent" })
], F.prototype, "noDedent");
Q([
  m({ attribute: !1 })
], F.prototype, "snippets");
Q([
  Qe()
], F.prototype, "_revealed");
Q([
  Qe()
], F.prototype, "_index");
customElements.define("sema-code-typer", F);
var Eg = Object.defineProperty, lo = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Eg(e, t, r), r;
};
const To = class To extends C {
  constructor() {
    super(...arguments), this.prompt = "$", this.autoPrompt = !1, this.copy = !1, this._raw = "", this._copy = new Ll(this, () => this._commands()), this._onSlotChange = (e) => {
      const t = e.target;
      this._raw = t.assignedNodes({ flatten: !0 }).map((s) => s.textContent ?? "").join(""), this.requestUpdate();
    };
  }
  _renderCommand(e) {
    const t = e.match(/^(.*?)(\s+)(#\s.*)$/), s = t ? t[1] : e, r = t ? t[2] : "", o = t ? t[3] : "", i = K(s).replace(
      /("[^"]*"|'[^']*')/g,
      '<span class="tok-string">$1</span>'
    );
    return { html: o ? `${i}${K(r)}<span class="tok-comment">${K(o)}</span>` : i, copyText: s };
  }
  _parse() {
    const e = Vn(this._raw);
    if (!e) return [];
    const t = this.prompt || "$";
    return e.split(`
`).map((s) => {
      if (s.trim() === "") return { kind: "blank" };
      if (/^\s*#/.test(s)) return { kind: "comment", html: `<span class="tok-comment">${K(s)}</span>` };
      let r = null;
      return this.autoPrompt ? r = s : s.startsWith(t + " ") ? r = s.slice(t.length + 1) : s === t && (r = ""), r === null ? { kind: "output", text: s } : { kind: "command", prompt: t, ...this._renderCommand(r) };
    });
  }
  /** The command lines (prompt + trailing comments stripped) — what copy writes. */
  _commands() {
    return this._parse().filter((e) => e.kind === "command").map((e) => e.copyText).join(`
`);
  }
  render() {
    const e = this._parse();
    return w`
      <div class="wrap">
        ${this.copy ? w`<button
              class="copy ${this._copy.copied ? "copied" : ""}"
              part="copy-button"
              type="button"
              aria-label="Copy commands"
              @click=${this._copy.copy}
            >
              ${this._copy.copied ? "Copied" : "Copy"}
            </button>` : R}
        <pre class="sema-scroll" part="pre"><code part="code">${e.map((t) => {
      switch (t.kind) {
        case "command":
          return w`<span class="term-line"><span class="term-prompt" part="prompt"
                  >${t.prompt}</span
                > ${an(t.html)}</span>`;
        case "comment":
          return w`<span class="term-line">${an(t.html)}</span>`;
        case "output":
          return w`<span class="term-line term-output" part="output">${t.text}</span>`;
        case "blank":
          return w`<span class="term-line"> </span>`;
      }
    })}</code></pre>
        <slot @slotchange=${this._onSlotChange}></slot>
      </div>
    `;
  }
};
To.styles = [
  C.base,
  q($n),
  q(Nl),
  q(Sn),
  I`
      :host {
        display: block;
      }
      .wrap {
        position: relative;
      }
      pre {
        margin: 0;
        padding: var(--space-md, 16px);
        background: var(--bg-editor, #0a0a0a);
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-lg, 6px);
        overflow-x: auto;
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-md, 13px);
        line-height: 1.7;
        color: var(--text-primary, #d8d0c0);
      }
      code {
        font: inherit;
        color: inherit;
      }
      .term-line {
        display: block;
        white-space: pre;
      }
      .term-prompt {
        color: var(--gold, #c8a855);
        user-select: none;
      }
      .term-output {
        color: var(--text-tertiary, #5a5448);
      }
      slot {
        display: none;
      }
    `
];
let It = To;
lo([
  m({ reflect: !0 })
], It.prototype, "prompt");
lo([
  m({ type: Boolean, reflect: !0, attribute: "prefix" })
], It.prototype, "autoPrompt");
lo([
  m({ type: Boolean, reflect: !0 })
], It.prototype, "copy");
customElements.define("sema-terminal", It);
var Rg = Object.defineProperty, Rn = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Rg(e, t, r), r;
};
const Io = class Io extends C {
  constructor() {
    super(...arguments), this.open = !1, this.placement = "bottom-start", this.openOn = "click", this.modal = !1, this._triggerEl = null, this._focusTrap = new zr(this, {
      getContainer: () => this._panel,
      isActive: () => this.open && this.modal
    }), this._onDocPointer = (e) => {
      e.composedPath().includes(this) || this.hide(!1);
    }, this._onViewportChange = (e) => {
      e.composedPath().includes(this) || this.hide(!1);
    }, this._onTriggerClick = () => {
      this.openOn === "click" && this.toggle();
    }, this._onPointerEnter = () => {
      this.openOn === "hover" && this.show();
    }, this._onPointerLeave = () => {
      this.openOn === "hover" && this.hide(!1);
    }, this._onKeydown = (e) => {
      this.open && (e.key === "Escape" ? (e.stopPropagation(), e.preventDefault(), this.hide(!0)) : e.key === "Tab" && !this.modal && this.hide(!0));
    }, this._onFocusOut = (e) => {
      if (this.modal || !this.open) return;
      const t = e.relatedTarget;
      t && this.contains(t) || this.hide(!1);
    }, this._onSelect = () => this.hide(!0);
  }
  disconnectedCallback() {
    var e;
    super.disconnectedCallback(), document.removeEventListener("pointerdown", this._onDocPointer, !0), window.removeEventListener("scroll", this._onViewportChange, !0), window.removeEventListener("resize", this._onViewportChange), this.open && ((e = this._triggerEl) == null || e.setAttribute("aria-expanded", "false"), this.open = !1), this._triggerEl = null;
  }
  get _trigger() {
    return this.querySelector('[slot="trigger"]');
  }
  /** Measure the trigger and place the (fixed) panel at viewport-relative
   * coordinates, flipping vertically when the preferred side doesn't fit but the
   * opposite side does. Called once the panel has laid out (post-`updateComplete`),
   * since it needs the panel's real dimensions. */
  _reposition() {
    const e = this._triggerEl, t = this._panel;
    if (!e || !t) return;
    const s = 4, r = e.getBoundingClientRect(), o = t.getBoundingClientRect(), i = this.placement.startsWith("top"), a = r.top - s - o.height >= 0, l = r.bottom + s + o.height <= window.innerHeight;
    let c = i;
    i && !a && l && (c = !1), !i && !l && a && (c = !0);
    let h, u;
    switch (this.placement) {
      case "left":
        h = r.top, u = r.left - s - o.width;
        break;
      case "right":
        h = r.top, u = r.right + s;
        break;
      default:
        h = c ? r.top - s - o.height : r.bottom + s, u = this.placement.endsWith("end") ? r.right - o.width : r.left;
    }
    u = Math.max(s, Math.min(u, window.innerWidth - o.width - s)), t.style.top = `${h}px`, t.style.left = `${u}px`;
  }
  show() {
    if (this.open) return;
    this._triggerEl = this._trigger, this.open = !0;
    const e = this._triggerEl;
    e && (e.setAttribute("aria-expanded", "true"), e.setAttribute(
      "aria-haspopup",
      this.querySelector("sema-menu") ? "menu" : this.modal ? "dialog" : "true"
    )), document.addEventListener("pointerdown", this._onDocPointer, !0), window.addEventListener("scroll", this._onViewportChange, !0), window.addEventListener("resize", this._onViewportChange), this.dispatchEvent(new CustomEvent("sema-open", { bubbles: !0, composed: !0 })), this.updateComplete.then(() => {
      var s;
      if (!this.open) return;
      this._reposition();
      const t = this.querySelector("sema-menu");
      t != null && t.focusFirst ? t.focusFirst() : (s = lr(this._panel)[0]) == null || s.focus();
    });
  }
  /** Close the popover. By default returns focus to the trigger (Esc/Tab/select). */
  hide(e = !0) {
    if (!this.open) return;
    this.open = !1, document.removeEventListener("pointerdown", this._onDocPointer, !0), window.removeEventListener("scroll", this._onViewportChange, !0), window.removeEventListener("resize", this._onViewportChange);
    const t = this._trigger ?? this._triggerEl;
    t == null || t.setAttribute("aria-expanded", "false"), e && (t == null || t.focus({ preventScroll: !0 })), this.dispatchEvent(new CustomEvent("sema-close", { bubbles: !0, composed: !0 }));
  }
  toggle() {
    this.open ? this.hide() : this.show();
  }
  // child menu chose an item
  render() {
    return w`
      <span
        class="trigger"
        part="trigger"
        @click=${this._onTriggerClick}
        @pointerenter=${this._onPointerEnter}
        @pointerleave=${this._onPointerLeave}
        @keydown=${this._onKeydown}
      >
        <slot name="trigger"></slot>
      </span>
      <div
        class="panel"
        part="panel"
        role="presentation"
        ?hidden=${!this.open}
        @keydown=${this._onKeydown}
        @focusout=${this._onFocusOut}
        @sema-select=${this._onSelect}
        @pointerenter=${this._onPointerEnter}
        @pointerleave=${this._onPointerLeave}
      >
        <slot></slot>
      </div>
    `;
  }
};
Io.styles = [
  C.base,
  I`
      :host {
        display: inline-block;
        position: relative;
      }
      /* Fixed (not absolute) so the panel escapes overflow-clipping ancestors — a
         plain overflow:hidden/auto container does not clip a fixed descendant.
         top/left are set inline by _reposition() (measured from the trigger's
         getBoundingClientRect() once the panel has laid out); left unset here,
         the panel sits at its default fixed origin only for the single
         pre-measurement frame right after open flips true. */
      .panel {
        position: fixed;
        z-index: 300;
        min-width: max-content;
        background: var(--bg-elevated, #141414);
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-md, 4px);
        padding: var(--space-xs, 4px);
        box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
      }
      .panel[hidden] {
        display: none;
      }
    `
];
let He = Io;
Rn([
  m({ type: Boolean, reflect: !0 })
], He.prototype, "open");
Rn([
  m({ reflect: !0 })
], He.prototype, "placement");
Rn([
  m({ attribute: "open-on" })
], He.prototype, "openOn");
Rn([
  m({ type: Boolean, reflect: !0 })
], He.prototype, "modal");
Rn([
  ga(".panel")
], He.prototype, "_panel");
customElements.define("sema-popover", He);
var Tg = Object.defineProperty, co = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Tg(e, t, r), r;
};
const Po = class Po extends C {
  constructor() {
    super(...arguments), this._onKeydown = (e) => {
      switch (e.key) {
        case "ArrowDown":
          e.preventDefault(), this._focusAt(this._activeIndex() + 1);
          break;
        case "ArrowUp":
          e.preventDefault(), this._focusAt(this._activeIndex() - 1);
          break;
        case "Home":
          e.preventDefault(), this._focusAt(0);
          break;
        case "End":
          e.preventDefault(), this._focusAt(this._enabled.length - 1);
          break;
        case "Enter":
        case " ": {
          const t = this.querySelector("sema-menu-item:focus");
          t && (e.preventDefault(), this._select(t));
          break;
        }
      }
    }, this._onClick = (e) => {
      const t = e.target.closest("sema-menu-item");
      t && !t.disabled && this._select(t);
    };
  }
  get _enabled() {
    return this._items.filter((e) => !e.disabled);
  }
  /** Focus the first enabled item (called by the popover on open). */
  focusFirst() {
    var e;
    (e = this._enabled[0]) == null || e.focus();
  }
  _activeIndex() {
    const e = this._enabled, t = this.querySelector("sema-menu-item:focus");
    return t ? e.indexOf(t) : -1;
  }
  _focusAt(e) {
    const t = this._enabled;
    if (t.length === 0) return;
    const s = (e + t.length) % t.length;
    t[s].focus();
  }
  _select(e) {
    this.dispatchEvent(
      new CustomEvent("sema-select", {
        detail: { value: e.value, item: e },
        bubbles: !0,
        composed: !0
      })
    );
  }
  render() {
    return w`<div
      role="menu"
      aria-label=${this.getAttribute("aria-label") || "Menu"}
      @keydown=${this._onKeydown}
      @click=${this._onClick}
    >
      <slot></slot>
    </div>`;
  }
};
Po.styles = [
  C.base,
  I`
      :host {
        display: block;
        min-width: 160px;
      }
      [role='menu'] {
        display: flex;
        flex-direction: column;
      }
    `
];
let hs = Po;
co([
  Nr({ selector: "sema-menu-item" })
], hs.prototype, "_items");
const ms = class ms extends C {
  constructor() {
    super(...arguments), this.value = "", this.disabled = !1;
  }
  focus() {
    var e, t;
    (t = (e = this.shadowRoot) == null ? void 0 : e.querySelector(".item")) == null || t.focus();
  }
  render() {
    return w`<button
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
};
ms.shadowRootOptions = { ...C.shadowRootOptions, delegatesFocus: !0 }, ms.styles = [
  C.base,
  I`
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
    `
];
let fn = ms;
co([
  m()
], fn.prototype, "value");
co([
  m({ type: Boolean, reflect: !0 })
], fn.prototype, "disabled");
customElements.define("sema-menu", hs);
customElements.define("sema-menu-item", fn);
const uo = '.control{width:100%;box-sizing:border-box;font-family:var(--mono, "JetBrains Mono", monospace);font-size:var(--text-md, 13px);line-height:1.5;padding:8px 11px;background:var(--bg-editor, #0a0a0a);color:var(--text-primary, #d8d0c0);border:1px solid var(--border, #1e1e1e);border-radius:var(--radius-sm, 3px);outline:none;caret-color:var(--gold, #c8a855);transition:border-color .15s,box-shadow .15s}.control::placeholder{color:var(--text-tertiary, #5a5448)}.control:focus-visible{border-color:var(--gold-dim, rgba(200, 168, 85, .5));box-shadow:0 0 0 1px var(--gold-dim, rgba(200, 168, 85, .5))}.control:disabled{opacity:.5;cursor:not-allowed}';
var Ig = Object.defineProperty, Re = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Ig(e, t, r), r;
};
const bs = class bs extends C {
  constructor() {
    super(...arguments), this.value = "", this.type = "text", this.placeholder = "", this.name = "", this.disabled = !1, this.required = !1, this.readonly = !1, this.testid = "", this._internals = this.attachInternals(), this._onInput = (e) => {
      this.value = e.target.value, this._internals.setFormValue(this.value);
    }, this._onChange = () => {
      this.dispatchEvent(new Event("change", { bubbles: !0, composed: !0 }));
    }, this._onKeydown = (e) => {
      var t;
      e.key === "Enter" && !e.isComposing && ((t = this._internals.form) == null || t.requestSubmit());
    };
  }
  // Host aria-* attributes (set e.g. by <sema-field>) must be mirrored onto the
  // inner control, where AT computes name/description — re-render when they change.
  static get observedAttributes() {
    return [...super.observedAttributes, "aria-label", "aria-description", "aria-invalid"];
  }
  attributeChangedCallback(e, t, s) {
    super.attributeChangedCallback(e, t, s), e.startsWith("aria-") && this.requestUpdate();
  }
  updated(e) {
    e.has("value") && this._internals.setFormValue(this.value);
  }
  formResetCallback() {
    this.value = "", this._internals.setFormValue("");
  }
  render() {
    return w`<input
      class="control"
      part="control"
      data-testid=${X(this.testid || void 0)}
      .value=${Ps(this.value)}
      type=${this.type}
      placeholder=${this.placeholder}
      ?disabled=${this.disabled}
      ?required=${this.required}
      ?readonly=${this.readonly}
      maxlength=${X(this.maxlength)}
      aria-label=${this.getAttribute("aria-label") || this.name || "input"}
      aria-description=${X(this.getAttribute("aria-description") ?? void 0)}
      aria-invalid=${X(this.getAttribute("aria-invalid") ?? void 0)}
      @input=${this._onInput}
      @change=${this._onChange}
      @keydown=${this._onKeydown}
    />`;
  }
};
bs.formAssociated = !0, bs.styles = [
  C.base,
  q(uo),
  I`
      :host {
        display: block;
      }
      :host([readonly]) .control {
        opacity: 0.6;
        cursor: default;
      }
    `
];
let ue = bs;
Re([
  m()
], ue.prototype, "value");
Re([
  m()
], ue.prototype, "type");
Re([
  m()
], ue.prototype, "placeholder");
Re([
  m()
], ue.prototype, "name");
Re([
  m({ type: Boolean, reflect: !0 })
], ue.prototype, "disabled");
Re([
  m({ type: Boolean, reflect: !0 })
], ue.prototype, "required");
Re([
  m({ type: Boolean, reflect: !0 })
], ue.prototype, "readonly");
Re([
  m({ type: Number })
], ue.prototype, "maxlength");
Re([
  m()
], ue.prototype, "testid");
customElements.define("sema-input", ue);
var Pg = Object.defineProperty, ve = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Pg(e, t, r), r;
};
const ra = typeof CSS < "u" && typeof CSS.supports == "function" && CSS.supports("field-sizing", "content"), ys = class ys extends C {
  constructor() {
    super(...arguments), this.value = "", this.placeholder = "", this.name = "", this.rows = 4, this.disabled = !1, this.required = !1, this.readonly = !1, this.autosize = !1, this.testid = "", this._internals = this.attachInternals(), this._onInput = (e) => {
      this.value = e.target.value, this._internals.setFormValue(this.value), this.autosize && !ra && this._autoGrow();
    }, this._onChange = () => {
      this.dispatchEvent(new Event("change", { bubbles: !0, composed: !0 }));
    };
  }
  get _ta() {
    var e;
    return ((e = this.shadowRoot) == null ? void 0 : e.querySelector("textarea")) ?? null;
  }
  // Host aria-* attributes (set e.g. by <sema-field>) must be mirrored onto the
  // inner control, where AT computes name/description — re-render when they change.
  static get observedAttributes() {
    return [...super.observedAttributes, "aria-label", "aria-description", "aria-invalid"];
  }
  attributeChangedCallback(e, t, s) {
    super.attributeChangedCallback(e, t, s), e.startsWith("aria-") && this.requestUpdate();
  }
  updated(e) {
    e.has("value") && this._internals.setFormValue(this.value), this.autosize && !ra && this._autoGrow();
  }
  formResetCallback() {
    this.value = "", this._internals.setFormValue("");
  }
  /** scrollHeight fallback for browsers without CSS `field-sizing`. */
  _autoGrow() {
    const e = this._ta;
    e && (e.style.height = "auto", e.style.height = `${e.scrollHeight + 2}px`);
  }
  render() {
    return w`<textarea
      class="control sema-scroll"
      part="control"
      data-testid=${X(this.testid || void 0)}
      .value=${Ps(this.value)}
      rows=${this.rows}
      placeholder=${this.placeholder}
      ?disabled=${this.disabled}
      ?required=${this.required}
      ?readonly=${this.readonly}
      maxlength=${X(this.maxlength)}
      aria-label=${this.getAttribute("aria-label") || this.name || "textarea"}
      aria-description=${X(this.getAttribute("aria-description") ?? void 0)}
      aria-invalid=${X(this.getAttribute("aria-invalid") ?? void 0)}
      @input=${this._onInput}
      @change=${this._onChange}
    ></textarea>`;
  }
};
ys.formAssociated = !0, ys.styles = [
  C.base,
  q(uo),
  q(Sn),
  I`
      :host {
        display: block;
      }
      .control {
        resize: vertical;
        min-height: 4em;
      }
      :host([autosize]) .control {
        field-sizing: content;
        resize: none;
        overflow: hidden;
        min-height: 3lh;
        max-height: 16lh;
      }
      :host([readonly]) .control {
        opacity: 0.6;
        cursor: default;
        resize: none;
      }
    `
];
let re = ys;
ve([
  m()
], re.prototype, "value");
ve([
  m()
], re.prototype, "placeholder");
ve([
  m()
], re.prototype, "name");
ve([
  m({ type: Number })
], re.prototype, "rows");
ve([
  m({ type: Boolean, reflect: !0 })
], re.prototype, "disabled");
ve([
  m({ type: Boolean, reflect: !0 })
], re.prototype, "required");
ve([
  m({ type: Boolean, reflect: !0 })
], re.prototype, "readonly");
ve([
  m({ type: Number })
], re.prototype, "maxlength");
ve([
  m({ type: Boolean, reflect: !0 })
], re.prototype, "autosize");
ve([
  m()
], re.prototype, "testid");
customElements.define("sema-textarea", re);
var Lg = Object.defineProperty, Te = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Lg(e, t, r), r;
}, Ee;
const _e = (Ee = class extends C {
  constructor() {
    super(...arguments), this.value = "", this.name = "", this.placeholder = "Select…", this.disabled = !1, this.required = !1, this.native = !1, this._entries = [], this._open = !1, this._listboxId = `sema-listbox-${++Ee._uid}`, this._internals = this.attachInternals(), this._onOpen = () => {
      this._open = !0, requestAnimationFrame(() => {
        var t, s;
        if (!this._open) return;
        (s = ((t = this.shadowRoot) == null ? void 0 : t.querySelector(
          `.option[data-value="${CSS.escape(this.value)}"]:not([disabled])`
        )) ?? this._enabledOptions()[0]) == null || s.focus();
      });
    }, this._sync = () => {
      const e = [];
      for (const t of Array.from(this.children))
        t instanceof HTMLOptGroupElement ? e.push({
          label: t.label,
          options: Array.from(t.querySelectorAll("option")).map((s) => this._readOption(s))
        }) : t instanceof HTMLOptionElement && e.push(this._readOption(t));
      this._entries = e, this.value || (this.value = this._firstValue()), this._internals.setFormValue(this.value), this._syncValidity();
    }, this._onTriggerKeydown = (e) => {
      var t;
      (e.key === "ArrowDown" || e.key === "ArrowUp") && (e.preventDefault(), (t = this._pop) == null || t.show());
    }, this._onListKeydown = (e) => {
      var o;
      const t = this._enabledOptions();
      if (t.length === 0) return;
      const s = (o = this.shadowRoot) == null ? void 0 : o.activeElement, r = s ? t.indexOf(s) : -1;
      switch (e.key) {
        case "ArrowDown":
          e.preventDefault(), t[(r + 1 + t.length) % t.length].focus();
          break;
        case "ArrowUp":
          e.preventDefault(), t[(r - 1 + t.length) % t.length].focus();
          break;
        case "Home":
          e.preventDefault(), t[0].focus();
          break;
        case "End":
          e.preventDefault(), t[t.length - 1].focus();
          break;
        case "Enter":
        case " ":
          e.preventDefault(), (s ?? t[0]).click();
          break;
      }
    }, this._onNativeChange = (e) => {
      this.value = e.target.value, this._internals.setFormValue(this.value), this.dispatchEvent(new Event("change", { bubbles: !0, composed: !0 }));
    };
  }
  firstUpdated() {
    this._sync();
  }
  // Host aria-* attributes (set e.g. by <sema-field>) must be mirrored onto the
  // inner control, where AT computes name/description — re-render when they change.
  static get observedAttributes() {
    return [...super.observedAttributes, "aria-label", "aria-description", "aria-invalid"];
  }
  attributeChangedCallback(e, t, s) {
    super.attributeChangedCallback(e, t, s), e.startsWith("aria-") && this.requestUpdate();
  }
  updated(e) {
    var t;
    if (e.has("value") && this._internals.setFormValue(this.value), (e.has("value") || e.has("required")) && this._syncValidity(), this.native) {
      const s = (t = this.shadowRoot) == null ? void 0 : t.querySelector("select");
      s && s.value !== this.value && (s.value = this.value);
    }
  }
  formResetCallback() {
    this.value = this._firstValue(), this._internals.setFormValue(this.value), this._syncValidity();
  }
  _syncValidity() {
    var e;
    if (this.required && !this.value) {
      const t = ((e = this.shadowRoot) == null ? void 0 : e.querySelector(this.native ? "select" : ".trigger")) ?? void 0;
      this._internals.setValidity({ valueMissing: !0 }, "Please select an option", t);
    } else
      this._internals.setValidity({});
  }
  _flat() {
    return this._entries.flatMap((e) => "options" in e ? e.options : [e]);
  }
  _firstValue() {
    var e;
    return ((e = this._flat()[0]) == null ? void 0 : e.value) ?? "";
  }
  _labelFor(e) {
    var t;
    return ((t = this._flat().find((s) => s.value === e)) == null ? void 0 : t.label) ?? null;
  }
  _readOption(e) {
    return { value: e.value, label: e.textContent ?? "", disabled: e.disabled };
  }
  _select(e) {
    var t;
    this.value = e, this._internals.setFormValue(e), this._syncValidity(), (t = this._pop) == null || t.hide(), this.dispatchEvent(new Event("change", { bubbles: !0, composed: !0 }));
  }
  _enabledOptions() {
    var e;
    return Array.from(((e = this.shadowRoot) == null ? void 0 : e.querySelectorAll(".option:not([disabled])")) ?? []);
  }
  _optionTpl(e) {
    const t = e.value === this.value;
    return w`<button
      class="option"
      role="option"
      type="button"
      tabindex="-1"
      data-value=${e.value}
      aria-selected=${String(t)}
      ?disabled=${e.disabled}
      @click=${() => this._select(e.value)}
    >
      <span class="check">${t ? "✓" : ""}</span><span>${e.label}</span>
    </button>`;
  }
  _renderCustom() {
    const e = this._labelFor(this.value);
    return w`
      <sema-popover
        placement="bottom-start"
        @sema-open=${this._onOpen}
        @sema-close=${() => this._open = !1}
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
          aria-label=${this.getAttribute("aria-label") || this.name || "select"}
          aria-description=${X(this.getAttribute("aria-description") ?? void 0)}
          aria-invalid=${X(this.getAttribute("aria-invalid") ?? void 0)}
          @keydown=${this._onTriggerKeydown}
        >
          <span class="label ${e === null ? "placeholder" : ""}">${e ?? this.placeholder}</span>
          <span class="chevron" aria-hidden="true">▾</span>
        </button>
        <div
          class="listbox"
          id=${this._listboxId}
          role="listbox"
          aria-label=${this.getAttribute("aria-label") || this.name || "options"}
          @keydown=${this._onListKeydown}
        >
          ${this._entries.map(
      (t) => "options" in t ? w`<div class="group-label" role="presentation">${t.label}</div>
                  ${t.options.map((s) => this._optionTpl(s))}` : this._optionTpl(t)
    )}
        </div>
      </sema-popover>
      <slot @slotchange=${this._sync}></slot>
    `;
  }
  _renderNative() {
    return w`
      <select
        class="control"
        part="control"
        .value=${Ps(this.value)}
        ?disabled=${this.disabled}
        ?required=${this.required}
        aria-label=${this.getAttribute("aria-label") || this.name || "select"}
        aria-description=${X(this.getAttribute("aria-description") ?? void 0)}
        aria-invalid=${X(this.getAttribute("aria-invalid") ?? void 0)}
        @change=${this._onNativeChange}
      >
        ${this._entries.map(
      (e) => "options" in e ? w`<optgroup label=${e.label}>
                ${e.options.map((t) => w`<option value=${t.value} ?disabled=${t.disabled}>${t.label}</option>`)}
              </optgroup>` : w`<option value=${e.value} ?disabled=${e.disabled}>${e.label}</option>`
    )}
      </select>
      <slot @slotchange=${this._sync}></slot>
    `;
  }
  render() {
    return this.native ? this._renderNative() : this._renderCustom();
  }
}, Ee.formAssociated = !0, Ee.styles = [
  C.base,
  q(uo),
  I`
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
    `
], Ee._uid = 0, Ee);
Te([
  m()
], _e.prototype, "value");
Te([
  m()
], _e.prototype, "name");
Te([
  m()
], _e.prototype, "placeholder");
Te([
  m({ type: Boolean, reflect: !0 })
], _e.prototype, "disabled");
Te([
  m({ type: Boolean, reflect: !0 })
], _e.prototype, "required");
Te([
  m({ type: Boolean, reflect: !0 })
], _e.prototype, "native");
Te([
  Qe()
], _e.prototype, "_entries");
Te([
  Qe()
], _e.prototype, "_open");
Te([
  ga("sema-popover")
], _e.prototype, "_pop");
let Ng = _e;
customElements.define("sema-select", Ng);
var Mg = Object.defineProperty, ho = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Mg(e, t, r), r;
};
const Lo = class Lo extends C {
  constructor() {
    super(...arguments), this.label = "", this.hint = "", this.error = "", this._control = null, this._onSlotChange = (e) => {
      var r;
      const t = e.target.assignedElements({ flatten: !0 }), s = t.find((o) => o.matches("input, textarea, select, sema-input, sema-textarea, sema-select")) ?? t[0] ?? null;
      if (s !== this._control) {
        for (const o of ["aria-label", "aria-description", "aria-invalid"]) (r = this._control) == null || r.removeAttribute(o);
        this._control = s;
      }
      this._applyA11y();
    };
  }
  updated(e) {
    (e.has("label") || e.has("hint") || e.has("error")) && this._applyA11y();
  }
  // Shadow boundaries rule out IDREF associations (aria-labelledby/-describedby),
  // so mirror label/hint/error onto the control as plain string aria attributes.
  _applyA11y() {
    const e = this._control;
    if (!e) return;
    this.label ? e.setAttribute("aria-label", this.label) : e.removeAttribute("aria-label");
    const t = this.error || this.hint;
    t ? e.setAttribute("aria-description", t) : e.removeAttribute("aria-description"), this.error ? e.setAttribute("aria-invalid", "true") : e.removeAttribute("aria-invalid");
  }
  render() {
    const e = this.error || this.hint;
    return w`
      <label class="field" part="field">
        ${this.label ? w`<span class="label" part="label">${this.label}</span>` : R}
        <slot @slotchange=${this._onSlotChange}></slot>
        ${e ? w`<span class="msg ${this.error ? "error" : ""}" part="message">${e}</span>` : R}
      </label>
    `;
  }
};
Lo.styles = [
  C.base,
  I`
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
    `
];
let Pt = Lo;
ho([
  m()
], Pt.prototype, "label");
ho([
  m()
], Pt.prototype, "hint");
ho([
  m()
], Pt.prototype, "error");
customElements.define("sema-field", Pt);
var Og = Object.defineProperty, zg = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Og(e, t, r), r;
};
const No = class No extends C {
  constructor() {
    super(...arguments), this.orientation = "vertical";
  }
  render() {
    return w`<div class="viewport sema-scroll" part="viewport" tabindex="0">
      <slot></slot>
    </div>`;
  }
};
No.styles = [
  C.base,
  q(Sn),
  I`
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
    `
];
let ps = No;
zg([
  m({ reflect: !0 })
], ps.prototype, "orientation");
customElements.define("sema-scroll-area", ps);
var Bg = Object.defineProperty, Ns = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Bg(e, t, r), r;
};
function ir(n, e) {
  const t = [];
  for (let s = n; s <= e; s++) t.push(s);
  return t;
}
function Dg(n, e, t = 1, s = 1) {
  if (e <= 0) return [];
  const r = ir(1, Math.min(s, e)), o = ir(Math.max(e - s + 1, s + 1), e), i = Math.max(
    Math.min(n - t, e - s - t * 2 - 1),
    s + 2
  ), a = Math.min(
    Math.max(n + t, s + t * 2 + 2),
    o.length > 0 ? o[0] - 2 : e - 1
  );
  return [
    ...r,
    ...i > s + 2 ? ["start-ellipsis"] : s + 1 < e - s ? [s + 1] : [],
    ...ir(i, a),
    ...a < e - s - 1 ? ["end-ellipsis"] : e - s > s ? [e - s] : [],
    ...o
  ];
}
const Mo = class Mo extends C {
  constructor() {
    super(...arguments), this.page = 1, this.total = 1, this.siblings = 1, this.boundaries = 1;
  }
  get _current() {
    return this.total < 1 ? 1 : Math.min(Math.max(this.page, 1), this.total);
  }
  _go(e) {
    const t = Math.min(Math.max(e, 1), this.total);
    t !== this._current && (this.page = t, this.dispatchEvent(
      new CustomEvent("sema-page-change", {
        detail: { page: t },
        bubbles: !0,
        composed: !0
      })
    ));
  }
  render() {
    if (this.total < 1) return w``;
    const e = this._current, t = Dg(e, this.total, this.siblings, this.boundaries);
    return w`
      <nav part="nav" aria-label="Pagination">
        <button
          part="item prev"
          type="button"
          aria-label="Previous page"
          ?disabled=${e <= 1}
          @click=${() => this._go(e - 1)}
        >‹</button>

        ${t.map(
      (s) => typeof s == "number" ? w`<button
                part=${s === e ? "item page current" : "item page"}
                type="button"
                aria-label=${`Page ${s}`}
                aria-current=${s === e ? "page" : R}
                @click=${() => this._go(s)}
              >${s}</button>` : w`<span class="ellipsis" aria-hidden="true">…</span>`
    )}

        <button
          part="item next"
          type="button"
          aria-label="Next page"
          ?disabled=${e >= this.total}
          @click=${() => this._go(e + 1)}
        >›</button>
      </nav>
    `;
  }
};
Mo.styles = [
  C.base,
  I`
      :host {
        display: block;
      }
      nav {
        display: flex;
        align-items: center;
        gap: var(--space-xs, 4px);
      }
      button {
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xs, 11px);
        min-width: 30px;
        height: 30px;
        padding: 0 6px;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        background: transparent;
        border: 1px solid var(--border, #1e1e1e);
        border-radius: var(--radius-sm, 3px);
        color: var(--text-secondary, #a09888);
        cursor: pointer;
        transition: color 0.15s, background 0.15s, border-color 0.15s;
      }
      button:hover:not(:disabled):not([aria-current]) {
        color: var(--gold, #c8a855);
        border-color: var(--gold-dim, rgba(200, 168, 85, 0.5));
      }
      button:focus { outline: none; }
      button:focus-visible {
        outline: var(--focus-ring-width, 1px) solid var(--focus-ring-color-subtle, rgba(200, 168, 85, 0.5));
        outline-offset: var(--focus-ring-offset, 1px);
      }
      button:disabled {
        opacity: 0.35;
        cursor: not-allowed;
      }
      button[aria-current] {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
        border-color: var(--gold-dim, rgba(200, 168, 85, 0.5));
        cursor: default;
      }
      .ellipsis {
        min-width: 24px;
        text-align: center;
        color: var(--text-tertiary, #5a5448);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xs, 11px);
        user-select: none;
      }
    `
];
let ut = Mo;
Ns([
  m({ type: Number, reflect: !0 })
], ut.prototype, "page");
Ns([
  m({ type: Number, reflect: !0 })
], ut.prototype, "total");
Ns([
  m({ type: Number })
], ut.prototype, "siblings");
Ns([
  m({ type: Number })
], ut.prototype, "boundaries");
customElements.define("sema-pagination", ut);
var Fg = Object.defineProperty, Ql = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Fg(e, t, r), r;
};
const Oo = class Oo extends C {
  constructor() {
    super(...arguments), this.size = "md", this.label = "Loading";
  }
  render() {
    return w`
      <span class="spinner" part="spinner" aria-hidden="true"></span>
      <span class="visually-hidden" role="status">${this.label}</span>
    `;
  }
};
Oo.styles = [
  C.base,
  I`
      :host {
        display: inline-flex;
        align-items: center;
        vertical-align: middle;
        color: var(--text-secondary, #a09888);

        --_size: 16px;
        --_border: 2px;
      }
      :host([size='sm']) { --_size: 12px; --_border: 1.5px; }
      :host([size='lg']) { --_size: 24px; --_border: 2.5px; }

      .spinner {
        width: var(--_size);
        height: var(--_size);
        border: var(--_border) solid var(--border-focus, #333);
        border-top-color: var(--gold, #c8a855);
        border-radius: var(--radius-full, 50%);
        animation: spin 0.7s linear infinite;
      }

      @keyframes spin {
        to { transform: rotate(360deg); }
      }

      .visually-hidden {
        position: absolute;
        width: 1px;
        height: 1px;
        padding: 0;
        margin: -1px;
        overflow: hidden;
        clip: rect(0, 0, 0, 0);
        white-space: nowrap;
        border: 0;
      }
    `
];
let gn = Oo;
Ql([
  m({ reflect: !0 })
], gn.prototype, "size");
Ql([
  m()
], gn.prototype, "label");
customElements.define("sema-spinner", gn);
var Gg = Object.defineProperty, Ug = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Gg(e, t, r), r;
};
const zo = class zo extends C {
  get _parts() {
    return (this.keys ?? "").split("+").map((e) => e.trim()).filter(Boolean);
  }
  render() {
    const e = this._parts;
    return e.length === 0 ? w`<kbd part="key"><slot></slot></kbd>` : e.map(
      (t, s) => w`
        ${s > 0 ? w`<span class="sep" aria-hidden="true">+</span>` : R}
        <kbd part="key">${t}</kbd>
      `
    );
  }
};
zo.styles = [
  C.base,
  I`
      :host {
        display: inline-flex;
        align-items: center;
        gap: 0.25em;
        vertical-align: middle;
      }
      kbd {
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xs, 11px);
        line-height: 1;
        color: var(--text-secondary, #a09888);
        background: var(--bg-elevated, #141414);
        border: 1px solid var(--border, #1e1e1e);
        border-bottom-width: 2px;
        border-radius: var(--radius-sm, 3px);
        padding: 0.2em 0.4em;
        min-width: 1.4em;
        text-align: center;
        white-space: nowrap;
      }
      .sep {
        color: var(--text-tertiary, #5a5448);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-xxs, 10px);
      }
    `
];
let ds = zo;
Ug([
  m()
], ds.prototype, "keys");
customElements.define("sema-kbd", ds);
const jg = ["none", "xs", "sm", "md", "lg", "xl", "2xl", "3xl", "4xl"], qg = {
  xs: "4px",
  sm: "8px",
  md: "16px",
  lg: "24px",
  xl: "32px",
  "2xl": "48px",
  "3xl": "64px",
  "4xl": "96px"
};
function po(n, e) {
  return q(
    jg.map((t) => {
      const s = t === "none" ? "0" : `var(--space-${t}, ${qg[t]})`;
      return `:host([${n}='${t}']) { ${e}: ${s}; }`;
    }).join(`
`)
  );
}
function Tr(n, e, t) {
  e == null ? n.style.removeProperty(t) : n.style.setProperty(t, e);
}
var Hg = Object.defineProperty, Vl = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Hg(e, t, r), r;
};
const Bo = class Bo extends C {
  render() {
    return w`<slot></slot>`;
  }
};
Bo.styles = [
  C.base,
  I`
      :host {
        display: block;
        --_max: var(--sema-container-max, var(--container-lg, 1200px));
        --_gutter: var(--sema-container-gutter, clamp(var(--space-lg, 24px), 4vw, var(--space-xl, 32px)));
        max-inline-size: var(--_max);
        margin-inline: auto;
        padding-inline: var(--_gutter);
      }
      /* :host([size='…']) and :host([gutter='…']) overwrite --_max / --_gutter.
         These match author-set attributes only — defaults never reflect, so a
         bare element falls through to the --sema-* tier above. */
      :host([size='md']) {
        --_max: var(--container-md, 1000px);
      }
      :host([size='lg']) {
        --_max: var(--container-lg, 1200px);
      }
      :host([size='full']) {
        --_max: none;
      }
    `,
  po("gutter", "--_gutter")
];
let mn = Bo;
Vl([
  m({ reflect: !0 })
], mn.prototype, "size");
Vl([
  m({ reflect: !0 })
], mn.prototype, "gutter");
customElements.define("sema-container", mn);
var Wg = Object.defineProperty, fo = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Wg(e, t, r), r;
};
const Do = class Do extends C {
  willUpdate(e) {
    e.has("min") && Tr(this, this.min, "--_min");
  }
  render() {
    return w`<div class="grid" part="grid"><slot></slot></div>`;
  }
};
Do.styles = [
  C.base,
  I`
      :host {
        display: block;
        --_min: var(--sema-grid-min, 240px);
        --_gap: var(--sema-grid-gap, var(--space-md, 16px));
      }
      /* Named inline-size container, and only when cols is set: auto mode
         needs no containment, and the name prevents accidental matches
         against consumer-defined ancestor containers. */
      :host([cols]) {
        container: sema-grid / inline-size;
      }
      /* Inner wrapper because container queries can't style the container's
         own box — shadow content queries the host. */
      .grid {
        display: grid;
        gap: var(--_gap);
        grid-template-columns: repeat(auto-fill, minmax(min(var(--_min), 100%), 1fr));
      }
      :host([cols='2']) .grid {
        grid-template-columns: repeat(2, minmax(0, 1fr));
      }
      /* The collapse must beat :host([cols='2']) above regardless of source
         order: the doubled [cols][cols] bumps specificity so a refactor (or
         codegen emitting blocks in a different order) can't silently break
         the narrow-container collapse. Threshold 700px ≈ content width of the
         default container at the page's historic 768px viewport breakpoint
         (768 − 2×32px gutters), so top-level grids collapse at the same point
         they did before migration. */
      @container sema-grid (inline-size < 700px) {
        :host([cols][cols]) .grid {
          grid-template-columns: minmax(0, 1fr);
        }
      }
      /* Code blocks / long URLs can't blow out tracks. */
      ::slotted(*) {
        min-inline-size: 0;
      }
    `,
  po("gap", "--_gap")
];
let Lt = Do;
fo([
  m({ reflect: !0 })
], Lt.prototype, "min");
fo([
  m({ type: Number, reflect: !0 })
], Lt.prototype, "cols");
fo([
  m({ reflect: !0 })
], Lt.prototype, "gap");
customElements.define("sema-grid", Lt);
var Qg = Object.defineProperty, go = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Qg(e, t, r), r;
};
const Fo = class Fo extends C {
  willUpdate(e) {
    e.has("sideWidth") && Tr(this, this.sideWidth, "--_side"), e.has("contentMin") && Tr(this, this.contentMin, "--_content-min");
  }
  render() {
    return w`<slot name="aside"></slot><slot></slot>`;
  }
};
Fo.styles = [
  C.base,
  I`
      :host {
        display: flex;
        flex-wrap: wrap;
        --_side: var(--sema-sidebar-side, 288px);
        --_content-min: var(--sema-sidebar-content-min, 50%);
        --_gap: var(--sema-sidebar-gap, var(--space-md, 16px));
        gap: var(--_gap);
      }
      ::slotted([slot='aside']) {
        flex-basis: var(--_side);
        flex-grow: 1;
      }
      /* Grow asymmetry + min-inline-size: when the content pane can't hold
         its minimum share of the row, the panes wrap to a stack. Single
         compound selector — valid in ::slotted(). */
      ::slotted(:not([slot])) {
        flex-basis: 0;
        flex-grow: 999;
        min-inline-size: var(--_content-min);
      }
    `,
  po("gap", "--_gap")
];
let Nt = Fo;
go([
  m({ reflect: !0, attribute: "side-width" })
], Nt.prototype, "sideWidth");
go([
  m({ reflect: !0, attribute: "content-min" })
], Nt.prototype, "contentMin");
go([
  m({ reflect: !0 })
], Nt.prototype, "gap");
customElements.define("sema-sidebar", Nt);
var Vg = Object.defineProperty, Ie = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Vg(e, t, r), r;
};
let Kg = 0, Zg = 0;
const Go = class Go extends C {
  constructor() {
    super(...arguments), this.value = "", this.activation = "auto", this.hashSync = !1, this._wired = !1, this._syncQueued = !1, this._warnedValue = null, this._sync = () => {
      var o, i;
      const e = this._tabs, t = this._panels;
      for (const a of e)
        a.id || (a.id = `sema-tab-${++Kg}`), a.setAttribute("role", "tab");
      for (const a of t)
        a.id || (a.id = `sema-tab-panel-${++Zg}`), a.setAttribute("role", "tabpanel");
      for (const a of e) {
        const l = this._panelFor(a.value);
        l ? (a.setAttribute("aria-controls", l.id), l.setAttribute("aria-labelledby", a.id)) : a.removeAttribute("aria-controls");
      }
      const s = this._enabledTabs(), r = (a) => a !== "" && s.some((l) => l.value === a);
      if (this._wired)
        r(this.value) || (this.value = ((i = s[0]) == null ? void 0 : i.value) ?? "");
      else {
        const a = this.hashSync ? window.location.hash.slice(1) : "";
        if (r(a))
          this.value = a;
        else if (!r(this.value)) {
          this.value && this._warnedValue !== this.value && (this._warnedValue = this.value, console.warn(`<sema-tabs> value="${this.value}" matches no enabled tab`));
          const l = s.find((c) => c.selected);
          this.value = ((o = l ?? s[0]) == null ? void 0 : o.value) ?? "";
        }
        this._wired = e.length > 0;
      }
      this._applySelection();
    }, this._onClick = (e) => {
      const t = this._tabFromEvent(e);
      t && this._activate(t);
    }, this._onKeydown = (e) => {
      const t = this._tabFromEvent(e);
      if (!t) return;
      const s = this._enabledTabs(), r = s.indexOf(t);
      let o;
      if (e.key === "ArrowRight") o = s[(r + 1) % s.length];
      else if (e.key === "ArrowLeft") o = s[(r - 1 + s.length) % s.length];
      else if (e.key === "Home") o = s[0];
      else if (e.key === "End") o = s[s.length - 1];
      else if (e.key === "Enter" || e.key === " ") {
        e.preventDefault(), this._activate(t);
        return;
      } else
        return;
      e.preventDefault(), !(!o || r < 0) && (this._roveTo(o), this.activation === "auto" && this._activate(o));
    }, this._onBeforeMatch = (e) => {
      const t = e.target;
      if (!(t instanceof HTMLElement) || !t.matches("sema-tab-panel")) return;
      const s = this._enabledTabs().find((r) => r.value === t.value);
      s && this._activate(s);
    }, this._onHashChange = () => {
      if (!this.hashSync) return;
      const e = this._enabledTabs().find((t) => t.value === window.location.hash.slice(1));
      e && this._activate(e);
    };
  }
  connectedCallback() {
    super.connectedCallback(), this.addEventListener("beforematch", this._onBeforeMatch), window.addEventListener("hashchange", this._onHashChange), this.hasUpdated && this.updateComplete.then(() => {
      this.isConnected && this._sync();
    });
  }
  disconnectedCallback() {
    super.disconnectedCallback(), this.removeEventListener("beforematch", this._onBeforeMatch), window.removeEventListener("hashchange", this._onHashChange);
  }
  updated(e) {
    e.has("value") && this._wired && this._applySelection();
  }
  render() {
    return w`
      <div
        class="tablist"
        role="tablist"
        part="tablist"
        aria-label=${this.getAttribute("aria-label") || "Tabs"}
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
    this._syncQueued || (this._syncQueued = !0, queueMicrotask(() => {
      this._syncQueued = !1, this.isConnected && this._sync();
    }));
  }
  _enabledTabs() {
    return this._tabs.filter((e) => !e.disabled);
  }
  _panelFor(e) {
    return this._panels.find((t) => t.value === e);
  }
  _applySelection() {
    const e = this._tabs, t = e.find((r) => !r.disabled && r.value === this.value && this.value !== ""), s = t ?? this._enabledTabs()[0] ?? e[0];
    for (const r of e)
      r.selected = r === t, r.setAttribute("aria-selected", String(r === t)), r.setAttribute("tabindex", r === s ? "0" : "-1"), r.disabled ? r.setAttribute("aria-disabled", "true") : r.removeAttribute("aria-disabled");
    for (const r of this._panels)
      t && r.value === this.value ? (r.removeAttribute("hidden"), this._setPanelFocusability(r)) : (r.setAttribute("hidden", "until-found"), r.removeAttribute("tabindex"));
  }
  // APG: a tabpanel with no focusable content is itself a tab stop.
  _setPanelFocusability(e) {
    e.querySelector(
      "a[href], button:not([disabled]), input:not([disabled]), select, textarea, [tabindex], audio[controls], video[controls], sema-button, sema-input, sema-textarea, sema-select"
    ) ? e.removeAttribute("tabindex") : e.setAttribute("tabindex", "0");
  }
  _tabFromEvent(e) {
    for (const t of e.composedPath())
      if (t instanceof HTMLElement && t.matches("sema-tab")) return t;
    return null;
  }
  _activate(e) {
    e.disabled || e.value === this.value || (this.value = e.value, this._applySelection(), this.hashSync && history.replaceState(null, "", "#" + this.value), e.scrollIntoView({ block: "nearest", inline: "nearest" }), this.dispatchEvent(new CustomEvent("sema-change", {
      detail: { value: this.value },
      bubbles: !0,
      composed: !0
    })));
  }
  /** Move the roving tab stop (and focus) without selecting. */
  _roveTo(e) {
    for (const t of this._tabs) t.setAttribute("tabindex", t === e ? "0" : "-1");
    e.focus();
  }
};
Go.styles = [
  C.base,
  I`
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
    `
];
let We = Go;
Ie([
  m({ reflect: !0 })
], We.prototype, "value");
Ie([
  m({ reflect: !0 })
], We.prototype, "activation");
Ie([
  m({ type: Boolean, reflect: !0, attribute: "hash-sync" })
], We.prototype, "hashSync");
Ie([
  Nr({ slot: "nav", selector: "sema-tab" })
], We.prototype, "_tabs");
Ie([
  Nr({ selector: "sema-tab-panel" })
], We.prototype, "_panels");
const Uo = class Uo extends C {
  constructor() {
    super(...arguments), this.value = "", this.selected = !1, this.disabled = !1;
  }
  connectedCallback() {
    super.connectedCallback(), this.slot || (this.slot = "nav");
  }
  updated(e) {
    var t;
    (e.has("disabled") || e.has("value")) && ((t = this.closest("sema-tabs")) == null || t._requestSync());
  }
  render() {
    return w`<slot></slot>`;
  }
};
Uo.styles = [
  C.base,
  I`
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
    `
];
let Mt = Uo;
Ie([
  m({ reflect: !0 })
], Mt.prototype, "value");
Ie([
  m({ type: Boolean, reflect: !0 })
], Mt.prototype, "selected");
Ie([
  m({ type: Boolean, reflect: !0 })
], Mt.prototype, "disabled");
const jo = class jo extends C {
  constructor() {
    super(...arguments), this.value = "";
  }
  updated(e) {
    var t;
    e.has("value") && ((t = this.closest("sema-tabs")) == null || t._requestSync());
  }
  render() {
    return w`<div class="panel" part="base"><slot></slot></div>`;
  }
};
jo.styles = [
  C.base,
  I`
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
    `
];
let fs = jo;
Ie([
  m({ reflect: !0 })
], fs.prototype, "value");
customElements.define("sema-tabs", We);
customElements.define("sema-tab", Mt);
customElements.define("sema-tab-panel", fs);
var Xg = Object.defineProperty, Kl = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Xg(e, t, r), r;
};
const qo = class qo extends C {
  constructor() {
    super(...arguments), this.variant = "info", this.dismissible = !0, this._dismiss = () => {
      this.dispatchEvent(new CustomEvent("sema-dismiss", { bubbles: !0, composed: !0 }));
    };
  }
  render() {
    const e = this.variant === "error" || this.variant === "warning" ? "alert" : "status";
    return w`<div class="toast" role=${e} part="toast">
      <span class="msg" part="message"><slot></slot></span>
      ${this.dismissible ? w`<button
            class="close"
            part="close"
            type="button"
            aria-label="Dismiss"
            @click=${this._dismiss}
          >
            ✕
          </button>` : R}
    </div>`;
  }
};
qo.styles = [
  C.base,
  I`
      :host {
        display: block;
        pointer-events: auto;
      }
      .toast {
        display: flex;
        align-items: flex-start;
        gap: 10px;
        min-width: 240px;
        max-width: 384px;
        padding: 11px 13px;
        background: var(--bg-elevated, #141414);
        border: 1px solid var(--border, #1e1e1e);
        border-left: 3px solid var(--accent, var(--text-secondary, #a09888));
        border-radius: var(--radius-md, 4px);
        box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
        font-family: var(--mono, 'JetBrains Mono', monospace);
        font-size: var(--text-sm, 12px);
        line-height: 1.5;
        color: var(--text-primary, #d8d0c0);
        animation: toast-in 0.18s ease;
      }
      :host([variant='success']) .toast {
        --accent: var(--success, #6a9955);
      }
      :host([variant='error']) .toast {
        --accent: var(--error, #c85555);
      }
      :host([variant='warning']) .toast {
        --accent: var(--gold, #c8a855);
      }
      :host([variant='info']) .toast {
        --accent: var(--text-secondary, #a09888);
      }
      .msg {
        flex: 1;
        min-width: 0;
      }
      .close {
        flex-shrink: 0;
        width: 19px;
        height: 19px;
        display: flex;
        align-items: center;
        justify-content: center;
        border: none;
        border-radius: var(--radius-sm, 3px);
        background: transparent;
        color: var(--text-tertiary, #5a5448);
        font-size: var(--text-lg, 14px);
        line-height: 1;
        cursor: pointer;
        transition: color 0.15s, background 0.15s;
      }
      .close:hover,
      .close:focus-visible {
        color: var(--gold, #c8a855);
        background: var(--gold-glow, rgba(200, 168, 85, 0.08));
        outline: none;
      }
      @keyframes toast-in {
        from {
          opacity: 0;
          transform: translateY(-6px);
        }
        to {
          opacity: 1;
          transform: translateY(0);
        }
      }
    `
];
let bn = qo;
Kl([
  m({ reflect: !0 })
], bn.prototype, "variant");
Kl([
  m({ type: Boolean, reflect: !0 })
], bn.prototype, "dismissible");
customElements.define("sema-toast", bn);
/**
 * @license
 * Copyright 2017 Google LLC
 * SPDX-License-Identifier: BSD-3-Clause
 */
const oa = (n, e, t) => {
  const s = /* @__PURE__ */ new Map();
  for (let r = e; r <= t; r++) s.set(n[r], r);
  return s;
}, Jg = Dr(class extends Fr {
  constructor(n) {
    if (super(n), n.type !== Me.CHILD) throw Error("repeat() can only be used in text expressions");
  }
  dt(n, e, t) {
    let s;
    t === void 0 ? t = e : e !== void 0 && (s = e);
    const r = [], o = [];
    let i = 0;
    for (const a of n) r[i] = s ? s(a, i) : i, o[i] = t(a, i), i++;
    return { values: o, keys: r };
  }
  render(n, e, t) {
    return this.dt(n, e, t).values;
  }
  update(n, [e, t, s]) {
    const r = Bf(n), { values: o, keys: i } = this.dt(e, t, s);
    if (!Array.isArray(r)) return this.ut = i, o;
    const a = this.ut ?? (this.ut = []), l = [];
    let c, h, u = 0, p = r.length - 1, d = 0, f = o.length - 1;
    for (; u <= p && d <= f; ) if (r[u] === null) u++;
    else if (r[p] === null) p--;
    else if (a[u] === i[d]) l[d] = Xe(r[u], o[d]), u++, d++;
    else if (a[p] === i[f]) l[f] = Xe(r[p], o[f]), p--, f--;
    else if (a[u] === i[f]) l[f] = Xe(r[u], o[f]), Ht(n, l[f + 1], r[u]), u++, f--;
    else if (a[p] === i[d]) l[d] = Xe(r[p], o[d]), Ht(n, r[u], r[p]), p--, d++;
    else if (c === void 0 && (c = oa(i, d, f), h = oa(a, u, p)), c.has(a[u])) if (c.has(a[p])) {
      const b = h.get(i[d]), v = b !== void 0 ? r[b] : null;
      if (v === null) {
        const k = Ht(n, r[u]);
        Xe(k, o[d]), l[d] = k;
      } else l[d] = Xe(v, o[d]), Ht(n, r[u], v), r[b] = null;
      d++;
    } else rr(r[p]), p--;
    else rr(r[u]), u++;
    for (; d <= f; ) {
      const b = Ht(n, l[f + 1]);
      Xe(b, o[d]), l[d++] = b;
    }
    for (; u <= p; ) {
      const b = r[u++];
      b !== null && rr(b);
    }
    return this.ut = i, Ml(n, l), le;
  }
});
var Yg = Object.defineProperty, mo = (n, e, t, s) => {
  for (var r = void 0, o = n.length - 1, i; o >= 0; o--)
    (i = n[o]) && (r = i(e, t, r) || r);
  return r && Yg(e, t, r), r;
};
let em = 0;
const Ho = class Ho extends C {
  constructor() {
    super(...arguments), this.position = "top-right", this.maxVisible = 5, this._toasts = [], this._timers = /* @__PURE__ */ new Map(), this._paused = !1, this._pause = () => {
      if (!this._paused) {
        this._paused = !0;
        for (const e of this._timers.values())
          clearTimeout(e.handle), e.remaining = Math.max(0, e.remaining - (performance.now() - e.startedAt));
      }
    }, this._resume = () => {
      if (this._paused) {
        this._paused = !1;
        for (const [e, t] of this._timers)
          clearTimeout(t.handle), t.startedAt = performance.now(), t.handle = window.setTimeout(() => this.dismiss(e), t.remaining);
      }
    };
  }
  disconnectedCallback() {
    super.disconnectedCallback();
    for (const e of this._timers.values()) clearTimeout(e.handle);
    this._timers.clear();
  }
  /** Show a toast; returns a handle to dismiss/update it. */
  show(e, t = {}) {
    const s = ++em, r = {
      id: s,
      message: e,
      variant: t.variant ?? "info",
      duration: t.duration === void 0 ? 4e3 : t.duration,
      dismissible: t.dismissible ?? !0
    };
    for (this._toasts = [r, ...this._toasts]; this._toasts.length > this.maxVisible; )
      this.dismiss(this._toasts[this._toasts.length - 1].id);
    return this._arm(r), {
      id: s,
      dismiss: () => this.dismiss(s),
      update: (o, i) => this.updateToast(s, o, i)
    };
  }
  updateToast(e, t, s = {}) {
    let r;
    this._toasts = this._toasts.map((o) => o.id !== e ? o : (r = {
      ...o,
      message: t,
      variant: s.variant ?? o.variant,
      dismissible: s.dismissible ?? o.dismissible,
      duration: s.duration === void 0 ? o.duration : s.duration
    }, r)), r && (this._disarm(e), this._arm(r));
  }
  dismiss(e) {
    this._disarm(e), this._toasts = this._toasts.filter((t) => t.id !== e);
  }
  dismissAll() {
    for (const e of [...this._toasts]) this.dismiss(e.id);
  }
  _arm(e) {
    if (!e.duration || e.duration <= 0) return;
    if (this._paused) {
      this._timers.set(e.id, { handle: 0, remaining: e.duration, startedAt: performance.now() });
      return;
    }
    const t = window.setTimeout(() => this.dismiss(e.id), e.duration);
    this._timers.set(e.id, { handle: t, remaining: e.duration, startedAt: performance.now() });
  }
  _disarm(e) {
    const t = this._timers.get(e);
    t && (clearTimeout(t.handle), this._timers.delete(e));
  }
  render() {
    return w`<div
      class="region"
      role="region"
      aria-live="polite"
      aria-label="Notifications"
      @pointerenter=${this._pause}
      @pointerleave=${this._resume}
    >
      ${Jg(
      this._toasts,
      (e) => e.id,
      (e) => w`<sema-toast
            .variant=${e.variant}
            ?dismissible=${e.dismissible}
            @sema-dismiss=${() => this.dismiss(e.id)}
            >${e.message}</sema-toast
          >`
    )}
    </div>`;
  }
};
Ho.styles = [
  C.base,
  I`
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
    `
];
let Ot = Ho;
mo([
  m({ reflect: !0 })
], Ot.prototype, "position");
mo([
  m({ type: Number, attribute: "max-visible" })
], Ot.prototype, "maxVisible");
mo([
  Qe()
], Ot.prototype, "_toasts");
customElements.define("sema-toaster", Ot);
function Qt() {
  const n = document.querySelector("sema-toaster");
  if (n) return n;
  const e = document.createElement("sema-toaster");
  return document.body.appendChild(e), e;
}
const fm = Object.assign(
  (n, e) => Qt().show(n, e),
  {
    success: (n, e) => Qt().show(n, { ...e, variant: "success" }),
    error: (n, e) => Qt().show(n, { ...e, variant: "error" }),
    warning: (n, e) => Qt().show(n, { ...e, variant: "warning" }),
    info: (n, e) => Qt().show(n, { ...e, variant: "info" }),
    dismissAll: () => {
      var n;
      (n = document.querySelector("sema-toaster")) == null || n.dismissAll();
    }
  }
);
export {
  At as SemaBadge,
  je as SemaButton,
  Nf as SemaCode,
  F as SemaCodeTyper,
  mn as SemaContainer,
  rn as SemaDialog,
  Rt as SemaDrawer,
  J as SemaEditor,
  C as SemaElement,
  Pt as SemaField,
  Lt as SemaGrid,
  ue as SemaInput,
  ds as SemaKbd,
  Tt as SemaMarkdown,
  hs as SemaMenu,
  fn as SemaMenuItem,
  on as SemaPage,
  ut as SemaPagination,
  He as SemaPopover,
  ps as SemaScrollArea,
  Ng as SemaSelect,
  Nt as SemaSidebar,
  gn as SemaSpinner,
  qe as SemaSplitter,
  Mt as SemaTab,
  fs as SemaTabPanel,
  We as SemaTabs,
  It as SemaTerminal,
  re as SemaTextarea,
  bn as SemaToast,
  Ot as SemaToaster,
  Et as SemaToggle,
  Wn as SemaToggleGroup,
  sn as SemaTooltip,
  Qn as SemaTree,
  Pc as SemaTreeItem,
  Dg as paginationItems,
  Af as registerLanguage,
  fm as toast
};
