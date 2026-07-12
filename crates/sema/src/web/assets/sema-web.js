// src/index.ts
import { SemaInterpreter } from "@sema-lang/sema";

// src/handles.ts
var SEMA_IDENT_RE = /^[a-zA-Z_][a-zA-Z0-9_/\-?!*><=+.]*$/;
var _handles = /* @__PURE__ */ new Map();
var _handleIds = /* @__PURE__ */ new WeakMap();
var _nextHandle = 1;
function getHandles(ctx) {
  return ctx ? ctx.handles : _handles;
}
function getHandleIds(ctx) {
  return ctx ? ctx.handleIds : _handleIds;
}
function allocHandle(ctx) {
  if (ctx) {
    return ctx.nextHandle++;
  }
  return _nextHandle++;
}
function storeHandle(obj, ctx) {
  if (obj == null) return null;
  const handles = getHandles(ctx);
  const handleIds = getHandleIds(ctx);
  const existing = handleIds.get(obj);
  if (existing != null && handles.get(existing) === obj) {
    return existing;
  }
  const id = allocHandle(ctx);
  handles.set(id, obj);
  handleIds.set(obj, id);
  return id;
}
function getElement(id, ctx) {
  const el = getHandles(ctx).get(id);
  if (!el || !(el instanceof Element)) {
    throw new Error(`Invalid element handle: ${id}`);
  }
  return el;
}
function getNode(id, ctx) {
  const node = getHandles(ctx).get(id);
  if (!node || !(node instanceof Element) && !(node instanceof Text)) {
    throw new Error(`Invalid node handle: ${id}`);
  }
  return node;
}
function getEvent(id, ctx) {
  const ev = getHandles(ctx).get(id);
  if (!ev || !(ev instanceof Event)) {
    throw new Error(`Invalid event handle: ${id}`);
  }
  return ev;
}
function releaseHandle(id, ctx) {
  const handles = getHandles(ctx);
  const value = handles.get(id);
  if (value) {
    getHandleIds(ctx).delete(value);
  }
  handles.delete(id);
}
function releaseHandlesForSubtree(root, ctx) {
  const handles = getHandles(ctx);
  const handleIds = getHandleIds(ctx);
  for (const [id, value] of handles) {
    if ((value instanceof Element || value instanceof Text) && (value === root || root.contains(value))) {
      handleIds.delete(value);
      handles.delete(id);
    }
  }
}

// src/callbacks.ts
function toInvokableCallback(value, interp, label) {
  if (typeof value === "function") {
    return value;
  }
  if (typeof value === "string" && SEMA_IDENT_RE.test(value)) {
    return ((...args) => interp.invokeGlobal(value, ...args));
  }
  throw new Error(`Invalid ${label}: expected function value or callback name`);
}
function releaseCallback(value) {
  if (typeof value === "function") {
    value.__semaRelease?.();
  }
}

// src/context.ts
var SemaWebContext = class {
  constructor() {
    /** DOM element/text/event handles */
    this.handles = /* @__PURE__ */ new Map();
    this.handleIds = /* @__PURE__ */ new WeakMap();
    this.nextHandle = 1;
    /** Reactive signals */
    this.signals = /* @__PURE__ */ new Map();
    this.nextSignalId = 1;
    /** Mounted components */
    this.mountedComponents = /* @__PURE__ */ new Map();
    this.mountedComponentsById = /* @__PURE__ */ new Map();
    this.nextComponentId = 1;
    /** Next capture ID for callComponent */
    this.nextCaptureId = 1;
    /** Component render context stack (per-instance for multi-instance isolation) */
    this.renderContextStack = [];
    /** Current execution owner stack for callbacks invoked outside render. */
    this.ownerStack = [];
    /** DOM event listeners registry */
    this.listeners = /* @__PURE__ */ new Map();
    /** Reactive watch cleanup callbacks */
    this.watchDisposers = /* @__PURE__ */ new Map();
    this.nextWatchId = 1;
    /** Browser interval handles */
    this.intervals = /* @__PURE__ */ new Map();
    /** Managed streaming resources keyed by signal id */
    this.streams = /* @__PURE__ */ new Map();
    /** Open WebSocket connections keyed by numeric handle */
    this.sockets = /* @__PURE__ */ new Map();
    this.nextSocketId = 1;
    /** Per-signal cleanup hooks (used for callback-backed computed signals, etc.) */
    this.signalFinalizers = /* @__PURE__ */ new Map();
    /** Runtime-level cleanup hooks */
    this.cleanupHooks = /* @__PURE__ */ new Set();
    /** Instance-owned scoped CSS stylesheet */
    this.styleEl = null;
    this.cssNamespace = Math.random().toString(36).slice(2, 10);
    this.nextCssClassId = 1;
    /** Error handler */
    this.onerror = (error, context) => {
      console.error(`[sema-web] Error in ${context}:`, error);
    };
  }
};
function getCurrentOwnerId(ctx) {
  const ownerId = ctx.ownerStack[ctx.ownerStack.length - 1];
  if (ownerId != null) return ownerId;
  const renderId = ctx.renderContextStack[ctx.renderContextStack.length - 1];
  return renderId != null ? renderId : null;
}
function withOwnerContext(ctx, ownerId, fn) {
  if (ownerId == null) return fn();
  ctx.ownerStack.push(ownerId);
  try {
    return fn();
  } finally {
    ctx.ownerStack.pop();
  }
}
function registerSignalFinalizer(ctx, signalId, finalizer) {
  const existing = ctx.signalFinalizers.get(signalId);
  if (existing) {
    try {
      existing();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `signal-finalizer:${signalId}`);
    }
  }
  ctx.signalFinalizers.set(signalId, finalizer);
}
function disposeSignal(ctx, signalId) {
  const finalizer = ctx.signalFinalizers.get(signalId);
  if (finalizer) {
    try {
      finalizer();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `signal-finalizer:${signalId}`);
    } finally {
      ctx.signalFinalizers.delete(signalId);
    }
  }
  ctx.signals.delete(signalId);
}
function disposeContextResources(ctx) {
  for (const { target, event, listener, callback } of ctx.listeners.values()) {
    try {
      target.removeEventListener(event, listener);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `listener-cleanup:${event}`);
    }
    releaseCallback(callback);
  }
  ctx.listeners.clear();
  for (const { dispose, callback } of ctx.watchDisposers.values()) {
    try {
      dispose();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), "watch-cleanup");
    }
    releaseCallback(callback);
  }
  ctx.watchDisposers.clear();
  for (const [id, { callback }] of ctx.intervals) {
    try {
      clearInterval(id);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `interval-cleanup:${id}`);
    }
    releaseCallback(callback);
  }
  ctx.intervals.clear();
  for (const stream of ctx.streams.values()) {
    try {
      stream.close();
    } catch (e) {
      ctx.onerror(
        e instanceof Error ? e : new Error(String(e)),
        `${stream.kind}-cleanup`
      );
    }
  }
  ctx.streams.clear();
  for (const { socket, callbacks } of ctx.sockets.values()) {
    try {
      socket.onopen = socket.onmessage = socket.onclose = socket.onerror = null;
      socket.close();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), "websocket-cleanup");
    }
    for (const cb of callbacks) releaseCallback(cb);
  }
  ctx.sockets.clear();
  for (const cleanup of ctx.cleanupHooks) {
    try {
      cleanup();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), "runtime-cleanup");
    }
  }
  ctx.cleanupHooks.clear();
  if (ctx.styleEl) {
    try {
      ctx.styleEl.remove();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), "css-cleanup");
    }
    ctx.styleEl = null;
  }
  ctx.handles.clear();
  for (const signalId of Array.from(ctx.signals.keys())) {
    disposeSignal(ctx, signalId);
  }
  ctx.mountedComponents.clear();
  ctx.mountedComponentsById.clear();
  ctx.renderContextStack.length = 0;
  ctx.ownerStack.length = 0;
}

// src/sip.ts
var SVG_NS = "http://www.w3.org/2000/svg";
var MATHML_NS = "http://www.w3.org/1998/Math/MathML";
var NS_ATTR_PREFIXES = {
  xlink: "http://www.w3.org/1999/xlink",
  xml: "http://www.w3.org/XML/1998/namespace",
  xmlns: "http://www.w3.org/2000/xmlns/"
};
var EVENT_NAME_RE = /^[a-zA-Z][a-zA-Z0-9_-]*$/;
function classListToString(values) {
  let joined = "";
  let hasToken = false;
  for (const value of values) {
    if (value === null || value === void 0 || value === false || value === "") {
      continue;
    }
    if (hasToken) joined += " ";
    joined += String(value);
    hasToken = true;
  }
  return joined;
}
var BOOLEAN_ATTRS = /* @__PURE__ */ new Set([
  "allowfullscreen",
  "async",
  "autofocus",
  "autoplay",
  "controls",
  "default",
  "defer",
  "disabled",
  "formnovalidate",
  "hidden",
  "inert",
  "ismap",
  "itemscope",
  "loop",
  "multiple",
  "muted",
  "nomodule",
  "novalidate",
  "open",
  "playsinline",
  "readonly",
  "required",
  "reversed",
  "selected"
]);
function renderSip(node, interp, ctx) {
  return renderSipNode(node, interp, ctx, null);
}
function renderSipNode(node, interp, ctx, namespaceURI) {
  if (node === null || node === void 0) {
    return document.createTextNode("");
  }
  if (typeof node === "string" || typeof node === "number" || typeof node === "boolean") {
    return document.createTextNode(String(node));
  }
  if (Array.isArray(node)) {
    if (node.length === 0) {
      return document.createTextNode("");
    }
    const tag = node[0];
    if (typeof tag !== "string") {
      const frag = document.createDocumentFragment();
      for (let i = 0; i < node.length; i++) {
        frag.appendChild(renderSipNode(node[i], interp, ctx, namespaceURI));
      }
      return frag;
    }
    const tagName = tag.startsWith(":") ? tag.slice(1) : tag;
    const lowerTag = tagName.toLowerCase();
    let elNamespace = namespaceURI;
    if (lowerTag === "svg") {
      elNamespace = SVG_NS;
    } else if (lowerTag === "math") {
      elNamespace = MATHML_NS;
    }
    let el;
    try {
      el = elNamespace ? document.createElementNS(elNamespace, tagName) : document.createElement(tagName);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `sip-render:invalid-tag:${tagName}`);
      return document.createTextNode("");
    }
    const childNamespace = lowerTag === "foreignobject" ? null : elNamespace;
    let childStart = 1;
    if (node.length > 1 && node[1] !== null && typeof node[1] === "object" && !Array.isArray(node[1])) {
      applyAttributes(el, node[1], interp, ctx);
      childStart = 2;
    }
    for (let i = childStart; i < node.length; i++) {
      el.appendChild(renderSipNode(node[i], interp, ctx, childNamespace));
    }
    return el;
  }
  try {
    return document.createTextNode(String(node));
  } catch (e) {
    ctx.onerror(e instanceof Error ? e : new Error(String(e)), "sip-render:text");
    return document.createTextNode("");
  }
}
function applyAttributes(el, attrs, interp, ctx) {
  try {
    for (const rawKey in attrs) {
      let key = rawKey;
      let value;
      try {
        if (!Object.prototype.hasOwnProperty.call(attrs, rawKey)) {
          continue;
        }
        value = attrs[rawKey];
      } catch (e) {
        ctx.onerror(e instanceof Error ? e : new Error(String(e)), `sip-render:attribute:${rawKey}`);
        continue;
      }
      if (key.startsWith(":")) {
        key = key.slice(1);
      }
      if (value === null || value === void 0) {
        continue;
      }
      try {
        if (key.startsWith("on-")) {
          const eventName = key.slice(3);
          if (!EVENT_NAME_RE.test(eventName)) {
            ctx.onerror(new Error(`Invalid event handler attribute: ${key}`), "sip-render:on-handler");
            continue;
          }
          if (typeof value === "string") {
            if (!SEMA_IDENT_RE.test(value)) {
              ctx.onerror(new Error(`Invalid event handler name: ${value}`), "sip-render:on-handler");
              continue;
            }
            el.setAttribute(`data-sema-on-${eventName}`, value);
          } else {
            ctx.onerror(
              new Error(`Event handler value for "${key}" must be a string function name, got: ${typeof value}`),
              "sip-render:on-handler"
            );
          }
        } else if (key === "style") {
          if (typeof value === "string") {
            el.setAttribute("style", value);
          } else if (typeof value === "object") {
            for (let [prop, val] of Object.entries(value)) {
              if (prop.startsWith(":")) prop = prop.slice(1);
              if (val === null || val === void 0) continue;
              el.style.setProperty(prop, String(val));
            }
          }
        } else if (key === "class") {
          if (value === false) {
          } else if (Array.isArray(value)) {
            const joined = classListToString(value);
            if (joined) el.setAttribute("class", joined);
          } else {
            el.setAttribute("class", String(value));
          }
        } else if (key === "value") {
          el.value = String(value);
        } else if (key === "checked") {
          el.checked = Boolean(value);
        } else if (key === "muted") {
          if (value) {
            el.setAttribute(key, "");
          } else {
            el.removeAttribute(key);
          }
          if ("defaultMuted" in el) {
            el.defaultMuted = Boolean(value);
          }
          if ("muted" in el) {
            el.muted = Boolean(value);
          }
        } else if (BOOLEAN_ATTRS.has(key)) {
          if (value) {
            el.setAttribute(key, "");
          } else {
            el.removeAttribute(key);
          }
        } else {
          if (key === "xmlns") {
            el.setAttributeNS(NS_ATTR_PREFIXES.xmlns, key, String(value));
            continue;
          }
          const colonIdx = key.indexOf(":");
          const prefix = colonIdx > 0 ? key.slice(0, colonIdx) : null;
          const ns = prefix ? NS_ATTR_PREFIXES[prefix] : void 0;
          if (ns) {
            el.setAttributeNS(ns, key, String(value));
          } else {
            el.setAttribute(key, String(value));
          }
        }
      } catch (e) {
        ctx.onerror(e instanceof Error ? e : new Error(String(e)), `sip-render:attribute:${key}`);
      }
    }
  } catch (e) {
    ctx.onerror(e instanceof Error ? e : new Error(String(e)), "sip-render:attributes");
  }
}
function registerSipBindings(interp, ctx) {
  interp.registerFunction("sip/render", (sipData) => {
    const node = renderSip(sipData, interp, ctx);
    if (node instanceof Element) {
      return storeHandle(node, ctx);
    }
    const wrapper = document.createElement("span");
    wrapper.appendChild(node);
    return storeHandle(wrapper, ctx);
  });
  interp.registerFunction("sip/render-into!", (selector, sipData) => {
    const target = document.querySelector(selector);
    if (!target) throw new Error(`sip/render-into!: target not found: ${selector}`);
    target.innerHTML = "";
    const node = renderSip(sipData, interp, ctx);
    target.appendChild(node);
    return null;
  });
  interp.registerFunction("hiccup/render", (sipData) => {
    const node = renderSip(sipData, interp, ctx);
    if (node instanceof Element) {
      return storeHandle(node, ctx);
    }
    const wrapper = document.createElement("span");
    wrapper.appendChild(node);
    return storeHandle(wrapper, ctx);
  });
  interp.registerFunction("hiccup/render-into!", (selector, sipData) => {
    const target = document.querySelector(selector);
    if (!target) throw new Error(`hiccup/render-into!: target not found: ${selector}`);
    target.innerHTML = "";
    const node = renderSip(sipData, interp, ctx);
    target.appendChild(node);
    return null;
  });
}

// src/dom.ts
function registerDomBindings(interp, ctx) {
  interp.registerFunction("dom/query", (selector) => {
    const el = document.querySelector(selector);
    return storeHandle(el, ctx);
  });
  interp.registerFunction("dom/query-all", (selector) => {
    const els = document.querySelectorAll(selector);
    const ids = [];
    els.forEach((el) => {
      const id = storeHandle(el, ctx);
      if (id != null) ids.push(id);
    });
    return ids;
  });
  interp.registerFunction("dom/get-id", (id) => {
    const el = document.getElementById(id);
    return storeHandle(el, ctx);
  });
  interp.registerFunction("dom/create-element", (tag) => {
    const el = document.createElement(tag);
    return storeHandle(el, ctx);
  });
  interp.registerFunction("dom/create-text", (content) => {
    const node = document.createTextNode(content);
    return storeHandle(node, ctx);
  });
  interp.registerFunction("dom/append-child!", (parentId, childId) => {
    const parent = getElement(parentId, ctx);
    const child = getNode(childId, ctx);
    parent.appendChild(child);
    return childId;
  });
  interp.registerFunction("dom/remove-child!", (parentId, childId) => {
    const parent = getElement(parentId, ctx);
    const child = getNode(childId, ctx);
    parent.removeChild(child);
    return childId;
  });
  interp.registerFunction("dom/remove!", (id) => {
    const el = getElement(id, ctx);
    el.remove();
    return null;
  });
  interp.registerFunction("dom/set-attribute!", (id, attr, val) => {
    getElement(id, ctx).setAttribute(attr, val);
    return null;
  });
  interp.registerFunction("dom/get-attribute", (id, attr) => {
    return getElement(id, ctx).getAttribute(attr);
  });
  interp.registerFunction("dom/remove-attribute!", (id, attr) => {
    getElement(id, ctx).removeAttribute(attr);
    return null;
  });
  interp.registerFunction("dom/add-class!", (id, ...classes) => {
    getElement(id, ctx).classList.add(...classes);
    return null;
  });
  interp.registerFunction("dom/remove-class!", (id, ...classes) => {
    getElement(id, ctx).classList.remove(...classes);
    return null;
  });
  interp.registerFunction("dom/toggle-class!", (id, cls) => {
    return getElement(id, ctx).classList.toggle(cls);
  });
  interp.registerFunction("dom/has-class?", (id, cls) => {
    return getElement(id, ctx).classList.contains(cls);
  });
  interp.registerFunction("dom/set-style!", (id, prop, val) => {
    const el = getElement(id, ctx);
    el.style.setProperty(prop, val);
    return null;
  });
  interp.registerFunction("dom/get-style", (id, prop) => {
    const el = getElement(id, ctx);
    return el.style.getPropertyValue(prop);
  });
  interp.registerFunction("dom/set-text!", (id, text) => {
    getElement(id, ctx).textContent = text;
    return null;
  });
  interp.registerFunction("dom/get-text", (id) => {
    return getElement(id, ctx).textContent;
  });
  interp.registerFunction("dom/set-html!", (id, html) => {
    getElement(id, ctx).innerHTML = html;
    return null;
  });
  interp.registerFunction("dom/get-html", (id) => {
    return getElement(id, ctx).innerHTML;
  });
  interp.registerFunction("dom/set-value!", (id, val) => {
    const el = getElement(id, ctx);
    el.value = val;
    return null;
  });
  interp.registerFunction("dom/get-value", (id) => {
    const el = getElement(id, ctx);
    return el.value;
  });
  interp.registerFunction("dom/on!", (id, event, callbackValue) => {
    const el = getElement(id, ctx);
    const callback = toInvokableCallback(callbackValue, interp, `dom/on! callback for "${event}"`);
    const ownerId = getCurrentOwnerId(ctx);
    const callbackKey = typeof callback.__semaCallbackHandle === "number" ? `handle:${callback.__semaCallbackHandle}` : typeof callbackValue === "string" ? `name:${callbackValue}` : `fn:${String(callback)}`;
    const key = `${id}:${event}:${callbackKey}`;
    const existing = ctx.listeners.get(key);
    if (existing) {
      existing.target.removeEventListener(existing.event, existing.listener);
      releaseCallback(existing.callback);
      ctx.listeners.delete(key);
      for (const component of ctx.mountedComponents.values()) {
        component.ownedListenerKeys.delete(key);
      }
    }
    const listener = (ev) => {
      const evHandle = storeHandle(ev, ctx);
      try {
        withOwnerContext(ctx, ownerId, () => callback(evHandle));
      } catch (e) {
        ctx.onerror(e instanceof Error ? e : new Error(String(e)), `event:${event}`);
      } finally {
        if (evHandle != null) releaseHandle(evHandle, ctx);
      }
    };
    ctx.listeners.set(key, { target: el, event, listener, callback });
    el.addEventListener(event, listener);
    const owner = ownerId != null ? ctx.mountedComponentsById.get(ownerId) ?? null : null;
    if (owner) owner.ownedListenerKeys.add(key);
    return null;
  });
  interp.registerFunction("dom/off!", (id, event, callbackValue) => {
    const callbackKey = typeof callbackValue === "function" && typeof callbackValue.__semaCallbackHandle === "number" ? `handle:${callbackValue.__semaCallbackHandle}` : typeof callbackValue === "string" ? `name:${callbackValue}` : `fn:${String(callbackValue)}`;
    const key = `${id}:${event}:${callbackKey}`;
    const registration = ctx.listeners.get(key);
    if (registration) {
      registration.target.removeEventListener(registration.event, registration.listener);
      releaseCallback(registration.callback);
      ctx.listeners.delete(key);
      for (const component of ctx.mountedComponents.values()) {
        component.ownedListenerKeys.delete(key);
      }
    }
    return null;
  });
  interp.registerFunction("dom/prevent-default!", (evId) => {
    getEvent(evId, ctx).preventDefault();
    return null;
  });
  interp.registerFunction("dom/stop-propagation!", (evId) => {
    const ev = getEvent(evId, ctx);
    ev.stopPropagation();
    ev.__sema_stop = true;
    return null;
  });
  interp.registerFunction("dom/event-value", (evId) => {
    const ev = getEvent(evId, ctx);
    const target = ev.target;
    if (target && "value" in target) {
      return target.value;
    }
    return null;
  });
  interp.registerFunction("dom/event-key", (evId) => {
    const ev = getEvent(evId, ctx);
    if ("key" in ev) {
      return ev.key;
    }
    return null;
  });
  interp.registerFunction("dom/event-target", (evId) => {
    const ev = getEvent(evId, ctx);
    const target = ev.target;
    if (target instanceof Element) {
      return storeHandle(target, ctx);
    }
    return null;
  });
  interp.registerFunction("dom/event-target-closest", (evId, selector) => {
    const ev = getEvent(evId, ctx);
    const target = ev.target;
    if (target instanceof Element) {
      const found = target.closest(selector);
      if (found) return storeHandle(found, ctx);
    }
    return null;
  });
  interp.registerFunction("dom/focus!", (id) => {
    const el = getElement(id, ctx);
    if ("focus" in el) el.focus();
    return null;
  });
  interp.registerFunction("dom/render", (sipData) => {
    const node = renderSip(sipData, interp, ctx);
    if (node instanceof Element) {
      return storeHandle(node, ctx);
    }
    const wrapper = document.createElement("span");
    wrapper.appendChild(node);
    return storeHandle(wrapper, ctx);
  });
  interp.registerFunction("dom/render-into!", (selector, sipData) => {
    const target = document.querySelector(selector);
    if (!target) throw new Error(`dom/render-into!: target not found: ${selector}`);
    target.innerHTML = "";
    const node = renderSip(sipData, interp, ctx);
    target.appendChild(node);
    return null;
  });
}

// src/store.ts
function registerStoreBindings(interp, ctx) {
  interp.registerFunction("store/get", (key) => {
    try {
      const val = localStorage.getItem(key);
      if (val === null) return null;
      return JSON.parse(val);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `store/get:${key}`);
      return null;
    }
  });
  interp.registerFunction("store/set!", (key, value) => {
    try {
      localStorage.setItem(key, JSON.stringify(value));
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `store/set!:${key}`);
    }
    return null;
  });
  interp.registerFunction("store/remove!", (key) => {
    localStorage.removeItem(key);
    return null;
  });
  interp.registerFunction("store/clear!", () => {
    localStorage.clear();
    return null;
  });
  interp.registerFunction("store/keys", () => {
    const keys = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key !== null) keys.push(key);
    }
    return keys;
  });
  interp.registerFunction("store/has?", (key) => {
    return localStorage.getItem(key) !== null;
  });
  interp.registerFunction("store/session-get", (key) => {
    try {
      const val = sessionStorage.getItem(key);
      if (val === null) return null;
      return JSON.parse(val);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `store/session-get:${key}`);
      return null;
    }
  });
  interp.registerFunction("store/session-set!", (key, value) => {
    try {
      sessionStorage.setItem(key, JSON.stringify(value));
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `store/session-set!:${key}`);
    }
    return null;
  });
  interp.registerFunction("store/session-remove!", (key) => {
    sessionStorage.removeItem(key);
    return null;
  });
  interp.registerFunction("store/session-clear!", () => {
    sessionStorage.clear();
    return null;
  });
}

// src/reactive.ts
import { signal, computed, effect, batch } from "@preact/signals-core";
function registerReactiveBindings(interp, ctx) {
  const getActiveComponent3 = () => {
    const componentId = getCurrentOwnerId(ctx);
    return componentId != null ? ctx.mountedComponentsById.get(componentId) ?? null : null;
  };
  interp.registerFunction("__state/create", (initialValue) => {
    const id = ctx.nextSignalId++;
    ctx.signals.set(id, signal(initialValue));
    return id;
  });
  interp.registerFunction("__state/deref", (signalId) => {
    const s = ctx.signals.get(signalId);
    if (!s) throw new Error(`Unknown state: ${signalId}`);
    return s.value;
  });
  interp.registerFunction("__state/put!", (signalId, newValue) => {
    const s = ctx.signals.get(signalId);
    if (!s) throw new Error(`Unknown state: ${signalId}`);
    s.value = newValue;
    return newValue;
  });
  interp.registerFunction("__state/computed-create", (callbackValue) => {
    const callback = toInvokableCallback(callbackValue, interp, "computed callback");
    const id = ctx.nextSignalId++;
    const owner = getActiveComponent3();
    const c = computed(() => {
      try {
        return withOwnerContext(ctx, owner?.instanceId ?? null, () => callback());
      } catch (e) {
        ctx.onerror(e instanceof Error ? e : new Error(String(e)), "computed");
        return void 0;
      }
    });
    ctx.signals.set(id, c);
    registerSignalFinalizer(ctx, id, () => {
      releaseCallback(callbackValue);
    });
    if (owner) owner.ownedSignalIds.add(id);
    return id;
  });
  interp.registerFunction("__state/batch-run", (callbackValue) => {
    const callback = toInvokableCallback(callbackValue, interp, "batch callback");
    let captured = void 0;
    try {
      batch(() => {
        try {
          captured = callback();
        } catch (e) {
          ctx.onerror(e instanceof Error ? e : new Error(String(e)), "batch");
        }
      });
    } finally {
      releaseCallback(callbackValue);
    }
    return captured;
  });
  interp.registerFunction("__state/watch", (signalId, callbackValue) => {
    const s = ctx.signals.get(signalId);
    if (!s) throw new Error(`Unknown state: ${signalId}`);
    const callback = toInvokableCallback(callbackValue, interp, "watch callback");
    const owner = getActiveComponent3();
    let prev = s.value;
    const watchId = ctx.nextWatchId++;
    const dispose = effect(() => {
      const current = s.value;
      if (prev !== current) {
        const oldVal = prev;
        prev = current;
        try {
          withOwnerContext(ctx, owner?.instanceId ?? null, () => callback(oldVal, current));
        } catch (e) {
          ctx.onerror(e instanceof Error ? e : new Error(String(e)), "watch");
        }
      }
    });
    ctx.watchDisposers.set(watchId, { dispose, callback });
    if (owner) owner.ownedWatchIds.add(watchId);
    return watchId;
  });
  interp.registerFunction("__state/unwatch", (watchId) => {
    const registration = ctx.watchDisposers.get(watchId);
    if (registration) {
      registration.dispose();
      ctx.watchDisposers.delete(watchId);
      releaseCallback(registration.callback);
      for (const component of ctx.mountedComponents.values()) {
        component.ownedWatchIds.delete(watchId);
      }
    }
    return null;
  });
  const semaResult = interp.evalStr(`
    (define (state val) (__state/create val))
    (define (deref ref) (__state/deref ref))
    (define (put! ref val) (__state/put! ref val))
    (define (update! ref f . args) (put! ref (apply f (cons (deref ref) args))))

    (defmacro computed (expr)
      \`(__state/computed-create (fn () ,expr)))

    (defmacro batch (. body)
      \`(__state/batch-run (fn () ,@body)))

    (define (watch ref fn) (__state/watch ref fn))
    (define (unwatch! watch-id) (__state/unwatch watch-id))
  `);
  if (semaResult.error) {
    throw new Error(`[sema-web] Failed to register reactive wrappers: ${semaResult.error}`);
  }
}

// src/component.ts
import morphdom from "morphdom";
import { signal as signal2, effect as effect2 } from "@preact/signals-core";
function withComponentContext(ctx, componentId, fn) {
  return withOwnerContext(ctx, componentId, () => {
    ctx.renderContextStack.push(componentId);
    try {
      return fn();
    } finally {
      ctx.renderContextStack.pop();
    }
  });
}
function callComponent(interp, component, ctx) {
  try {
    return withComponentContext(ctx, component.instanceId, () => interp.invokeGlobal(component.componentFn));
  } catch (e) {
    ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component:${component.componentFn}`);
    return null;
  }
}
function renderComponent(component, interp, ctx) {
  if (component.dispose) component.dispose();
  component.dispose = effect2(() => {
    const sipData = callComponent(interp, component, ctx);
    if (sipData == null) return;
    const clone = component.target.cloneNode(false);
    const sipNode = renderSip(sipData, interp, ctx);
    clone.appendChild(sipNode);
    const activeElement = document.activeElement;
    morphdom(component.target, clone, {
      childrenOnly: true,
      onBeforeElUpdated(fromEl, toEl) {
        if (fromEl === activeElement && (fromEl.tagName === "INPUT" || fromEl.tagName === "TEXTAREA" || fromEl.tagName === "SELECT")) {
          for (const attr of Array.from(toEl.attributes)) {
            if (attr.name !== "value") fromEl.setAttribute(attr.name, attr.value);
          }
          return false;
        }
        return true;
      },
      onNodeDiscarded(node) {
        releaseHandlesForSubtree(node, ctx);
      }
    });
  });
}
function getActiveComponent(ctx) {
  const componentId = ctx.renderContextStack[ctx.renderContextStack.length - 1];
  return componentId != null ? ctx.mountedComponentsById.get(componentId) ?? null : null;
}
function cleanupWatch(ctx, watchId) {
  const registration = ctx.watchDisposers.get(watchId);
  if (registration) {
    registration.dispose();
    ctx.watchDisposers.delete(watchId);
    releaseCallback(registration.callback);
  }
}
function cleanupInterval(ctx, intervalId) {
  clearInterval(intervalId);
  const registration = ctx.intervals.get(intervalId);
  if (registration) {
    releaseCallback(registration.callback);
  }
  ctx.intervals.delete(intervalId);
}
function cleanupStream(ctx, signalId) {
  const stream = ctx.streams.get(signalId);
  if (stream) {
    stream.close();
    ctx.streams.delete(signalId);
  }
}
function cleanupListener(ctx, key) {
  const registration = ctx.listeners.get(key);
  if (!registration) return;
  registration.target.removeEventListener(registration.event, registration.listener);
  releaseCallback(registration.callback);
  ctx.listeners.delete(key);
}
function destroyMountedComponent(selector, component, ctx) {
  if (component.pendingMount) {
    releaseCallback(component.pendingMount);
    component.pendingMount = null;
  }
  if (component.mountCleanup) {
    try {
      withComponentContext(ctx, component.instanceId, () => component.mountCleanup());
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `unmount-cleanup:${component.componentFn}`);
    } finally {
      component.mountCleanup = null;
    }
  }
  if (component.dispose) {
    try {
      component.dispose();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-dispose:${component.componentFn}`);
    }
    component.dispose = null;
  }
  if (component.eventCleanup) {
    try {
      component.eventCleanup();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-events:${component.componentFn}`);
    }
    component.eventCleanup = null;
  }
  for (const listenerKey of component.ownedListenerKeys) {
    try {
      cleanupListener(ctx, listenerKey);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-listener-cleanup:${component.componentFn}`);
    }
  }
  component.ownedListenerKeys.clear();
  for (const signalId of component.ownedSignalIds) {
    try {
      disposeSignal(ctx, signalId);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-signal-cleanup:${component.componentFn}`);
    }
  }
  component.ownedSignalIds.clear();
  for (const watchId of component.ownedWatchIds) {
    try {
      cleanupWatch(ctx, watchId);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-watch-cleanup:${component.componentFn}`);
    }
  }
  component.ownedWatchIds.clear();
  for (const intervalId of component.ownedIntervalIds) {
    try {
      cleanupInterval(ctx, intervalId);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-interval-cleanup:${component.componentFn}`);
    }
  }
  component.ownedIntervalIds.clear();
  for (const signalId of component.ownedStreamIds) {
    try {
      cleanupStream(ctx, signalId);
      disposeSignal(ctx, signalId);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-stream-cleanup:${component.componentFn}`);
    }
  }
  component.ownedStreamIds.clear();
  for (const signalId of component.localState.values()) {
    try {
      disposeSignal(ctx, signalId);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-local-state-cleanup:${component.componentFn}`);
    }
  }
  component.localState.clear();
  for (const child of Array.from(component.target.childNodes)) {
    releaseHandlesForSubtree(child, ctx);
  }
  component.target.innerHTML = "";
  ctx.mountedComponents.delete(selector);
  ctx.mountedComponentsById.delete(component.instanceId);
}
var EventDelegator = class {
  setup(component, target, interp, ctx) {
    const bubbling = [
      "click",
      "dblclick",
      "contextmenu",
      "input",
      "change",
      "submit",
      "keydown",
      "keyup",
      "keypress",
      "pointerdown",
      "pointerup",
      "pointermove",
      "focusin",
      "focusout"
    ];
    const listeners = [];
    for (const event of bubbling) {
      const attr = `data-sema-on-${event}`;
      const listener = (ev) => {
        const start = ev.target;
        if (!(start instanceof Element) || !target.contains(start)) {
          return;
        }
        let el = start;
        while (el) {
          if (el.hasAttribute(attr)) {
            const fn = el.getAttribute(attr);
            if (SEMA_IDENT_RE.test(fn)) {
              withOwnerContext(ctx, component.instanceId, () => {
                this.dispatchEvent(interp, ctx, fn, ev);
              });
              if (ev.cancelBubble || ev.__sema_stop) break;
            }
          }
          if (el === target) break;
          el = el.parentElement;
        }
      };
      target.addEventListener(event, listener);
      listeners.push({ event, listener });
    }
    const mouseOverListener = (ev) => {
      const mev = ev;
      const el = mev.target.closest?.("[data-sema-on-mouseenter]");
      if (!el || el.contains(mev.relatedTarget)) return;
      withOwnerContext(ctx, component.instanceId, () => {
        this.dispatchEvent(interp, ctx, el.getAttribute("data-sema-on-mouseenter"), mev);
      });
    };
    target.addEventListener("mouseover", mouseOverListener);
    listeners.push({ event: "mouseover", listener: mouseOverListener });
    const mouseOutListener = (ev) => {
      const mev = ev;
      const el = mev.target.closest?.("[data-sema-on-mouseleave]");
      if (!el || el.contains(mev.relatedTarget)) return;
      withOwnerContext(ctx, component.instanceId, () => {
        this.dispatchEvent(interp, ctx, el.getAttribute("data-sema-on-mouseleave"), mev);
      });
    };
    target.addEventListener("mouseout", mouseOutListener);
    listeners.push({ event: "mouseout", listener: mouseOutListener });
    return () => {
      for (const { event, listener } of listeners) {
        target.removeEventListener(event, listener);
      }
    };
  }
  dispatchEvent(interp, ctx, callbackName, ev) {
    const evHandle = storeHandle(ev, ctx);
    try {
      interp.invokeGlobal(callbackName, evHandle);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `event:${ev.type}:${callbackName}`);
    } finally {
      if (evHandle != null) releaseHandle(evHandle, ctx);
    }
  }
};
function registerComponentBindings(interp, ctx) {
  interp.registerFunction(
    "component/mount!",
    (selector, componentFn) => {
      if (!SEMA_IDENT_RE.test(componentFn)) {
        throw new Error(`Invalid component function name: ${componentFn}`);
      }
      const target = document.querySelector(selector);
      if (!target) throw new Error(`mount! target not found: ${selector}`);
      const existing = ctx.mountedComponents.get(selector);
      if (existing) {
        destroyMountedComponent(selector, existing, ctx);
      }
      const component = {
        instanceId: ctx.nextComponentId++,
        target,
        componentFn,
        dispose: null,
        eventCleanup: null,
        localState: /* @__PURE__ */ new Map(),
        mountCleanup: null,
        pendingMount: null,
        ownedSignalIds: /* @__PURE__ */ new Set(),
        ownedWatchIds: /* @__PURE__ */ new Set(),
        ownedIntervalIds: /* @__PURE__ */ new Set(),
        ownedStreamIds: /* @__PURE__ */ new Set(),
        ownedListenerKeys: /* @__PURE__ */ new Set()
      };
      const delegator = new EventDelegator();
      component.eventCleanup = delegator.setup(component, target, interp, ctx);
      ctx.mountedComponents.set(selector, component);
      ctx.mountedComponentsById.set(component.instanceId, component);
      renderComponent(component, interp, ctx);
      const pendingMount = component.pendingMount;
      if (pendingMount) {
        try {
          const mountCallback = toInvokableCallback(pendingMount, interp, "on-mount callback");
          const cleanupFn = withComponentContext(ctx, component.instanceId, () => mountCallback());
          if (typeof cleanupFn === "function") {
            const finalCleanupFn = cleanupFn;
            component.mountCleanup = () => {
              try {
                withComponentContext(ctx, component.instanceId, () => finalCleanupFn());
              } catch (e) {
                ctx.onerror(e instanceof Error ? e : new Error(String(e)), "unmount-cleanup");
              } finally {
                releaseCallback(finalCleanupFn);
              }
            };
          } else if (typeof cleanupFn === "string" && SEMA_IDENT_RE.test(cleanupFn)) {
            const finalCleanupName = cleanupFn;
            component.mountCleanup = () => {
              try {
                withComponentContext(ctx, component.instanceId, () => interp.invokeGlobal(finalCleanupName));
              } catch (e) {
                ctx.onerror(e instanceof Error ? e : new Error(String(e)), `unmount-cleanup:${finalCleanupName}`);
              }
            };
          }
          releaseCallback(mountCallback);
        } catch (e) {
          ctx.onerror(e instanceof Error ? e : new Error(String(e)), "on-mount");
        }
        component.pendingMount = null;
      }
      return null;
    }
  );
  interp.registerFunction("component/unmount!", (selector) => {
    const component = ctx.mountedComponents.get(selector);
    if (component) {
      destroyMountedComponent(selector, component, ctx);
    }
    return null;
  });
  interp.registerFunction("component/force-render!", (selector) => {
    const component = ctx.mountedComponents.get(selector);
    if (component) {
      renderComponent(component, interp, ctx);
    }
    return null;
  });
  interp.registerFunction("__component/current-id", () => {
    const stack = ctx.renderContextStack;
    return stack.length > 0 ? stack[stack.length - 1] : null;
  });
  interp.registerFunction("__component/local", (name, initialValue) => {
    const stack = ctx.renderContextStack;
    if (stack.length === 0) {
      throw new Error("(local) called outside of a component render context");
    }
    const component = getActiveComponent(ctx);
    if (!component) {
      throw new Error("(local) no active mounted component");
    }
    const existingId = component.localState.get(name);
    if (existingId != null) {
      return existingId;
    }
    const id = ctx.nextSignalId++;
    const s = signal2(initialValue);
    ctx.signals.set(id, s);
    component.localState.set(name, id);
    return id;
  });
  interp.registerFunction("__component/on-mount", (callbackValue) => {
    const stack = ctx.renderContextStack;
    if (stack.length === 0) {
      throw new Error("(on-mount) called outside of a component render context");
    }
    const component = getActiveComponent(ctx);
    if (!component) {
      throw new Error("(on-mount) no active mounted component");
    }
    releaseCallback(component.pendingMount);
    component.pendingMount = callbackValue;
    return null;
  });
  interp.registerFunction("js/set-interval", (callbackValue, ms) => {
    const callback = toInvokableCallback(callbackValue, interp, "interval callback");
    const ownerId = getCurrentOwnerId(ctx);
    const id = setInterval(() => {
      try {
        withOwnerContext(ctx, ownerId, () => callback());
      } catch (e) {
        ctx.onerror(e instanceof Error ? e : new Error(String(e)), "interval");
      }
    }, ms);
    ctx.intervals.set(id, { callback });
    const owner = ownerId != null ? ctx.mountedComponentsById.get(ownerId) ?? null : null;
    if (owner) owner.ownedIntervalIds.add(id);
    return id;
  });
  interp.registerFunction("js/clear-interval", (id) => {
    cleanupInterval(ctx, id);
    for (const component of ctx.mountedComponents.values()) {
      component.ownedIntervalIds.delete(id);
    }
    return null;
  });
  const semaResult = interp.evalStr(`
    (defmacro mount! (selector component-name)
      \`(component/mount! ,selector
         ,(if (symbol? component-name)
            (symbol->string component-name)
            component-name)))

    (define (local name initial) (__component/local name initial))

    (define (on-mount fn) (__component/on-mount fn))

    (defmacro defcomponent (name params . body)
      \`(define ,name
         (fn ,params ,@body)))
  `);
  if (semaResult.error) {
    throw new Error(`[sema-web] Failed to register component wrappers: ${semaResult.error}`);
  }
}
function disposeAllComponents(ctx) {
  for (const [selector, component] of Array.from(ctx.mountedComponents.entries())) {
    try {
      destroyMountedComponent(selector, component, ctx);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-dispose-all:${component.componentFn}`);
    }
  }
}

// src/llm.ts
import { signal as signal3 } from "@preact/signals-core";

// src/sse.ts
function createEventBuffer() {
  return {
    data: [],
    event: null,
    id: null,
    retry: null
  };
}
function emitBufferedEvent(buffer, onEvent) {
  if (buffer.data.length === 0 && buffer.event == null && buffer.id == null && buffer.retry == null) {
    return;
  }
  onEvent({
    data: buffer.data.join("\n"),
    event: buffer.event,
    id: buffer.id,
    retry: buffer.retry
  });
}
function openSseStream(opts) {
  const controller = new AbortController();
  const forwardedSignal = opts.signal;
  const abortForwarder = () => controller.abort(forwardedSignal?.reason);
  if (forwardedSignal) {
    if (forwardedSignal.aborted) {
      controller.abort(forwardedSignal.reason);
    } else {
      forwardedSignal.addEventListener("abort", abortForwarder, { once: true });
    }
  }
  const done = (async () => {
    try {
      const response = await fetch(opts.url, {
        method: opts.method ?? (opts.body != null ? "POST" : "GET"),
        headers: opts.headers,
        body: opts.body,
        credentials: opts.credentials,
        signal: controller.signal
      });
      if (!response.ok || !response.body) {
        throw new Error(`HTTP ${response.status}`);
      }
      opts.onOpen?.(response);
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let textBuffer = "";
      let eventBuffer = createEventBuffer();
      try {
        while (true) {
          const { done: done2, value } = await reader.read();
          if (done2) break;
          textBuffer += decoder.decode(value, { stream: true });
          const lines = textBuffer.split(/\r?\n/);
          textBuffer = lines.pop() ?? "";
          for (const line of lines) {
            if (line === "") {
              emitBufferedEvent(eventBuffer, opts.onEvent);
              eventBuffer = createEventBuffer();
              continue;
            }
            if (line.startsWith(":")) continue;
            const separator = line.indexOf(":");
            const field = separator === -1 ? line : line.slice(0, separator);
            let rawValue = separator === -1 ? "" : line.slice(separator + 1);
            if (rawValue.startsWith(" ")) rawValue = rawValue.slice(1);
            switch (field) {
              case "data":
                eventBuffer.data.push(rawValue);
                break;
              case "event":
                eventBuffer.event = rawValue || null;
                break;
              case "id":
                eventBuffer.id = rawValue || null;
                break;
              case "retry": {
                const parsed = Number.parseInt(rawValue, 10);
                eventBuffer.retry = Number.isFinite(parsed) ? parsed : null;
                break;
              }
              default:
                break;
            }
          }
        }
        textBuffer += decoder.decode();
        const finalLines = textBuffer.split(/\r?\n/);
        for (const line of finalLines) {
          if (!line) continue;
          if (line.startsWith(":")) continue;
          const separator = line.indexOf(":");
          const field = separator === -1 ? line : line.slice(0, separator);
          let rawValue = separator === -1 ? "" : line.slice(separator + 1);
          if (rawValue.startsWith(" ")) rawValue = rawValue.slice(1);
          if (field === "data") eventBuffer.data.push(rawValue);
          else if (field === "event") eventBuffer.event = rawValue || null;
          else if (field === "id") eventBuffer.id = rawValue || null;
          else if (field === "retry") {
            const parsed = Number.parseInt(rawValue, 10);
            eventBuffer.retry = Number.isFinite(parsed) ? parsed : null;
          }
        }
        emitBufferedEvent(eventBuffer, opts.onEvent);
      } finally {
        reader.releaseLock();
      }
    } catch (error) {
      if (!controller.signal.aborted) {
        opts.onError?.(error instanceof Error ? error : new Error(String(error)));
      }
    } finally {
      if (forwardedSignal) {
        forwardedSignal.removeEventListener("abort", abortForwarder);
      }
      opts.onClose?.();
    }
  })();
  return {
    close: () => controller.abort(),
    done
  };
}

// src/llm.ts
function registerLlmBindings(interp, opts, ctx) {
  const proxyUrl = opts.url.replace(/\/+$/, "");
  const headerPairs = [];
  headerPairs.push(`"Content-Type" "application/json"`);
  if (opts.token) {
    headerPairs.push(`"Authorization" "Bearer ${escapeSemaString(opts.token)}"`);
  }
  if (opts.headers) {
    for (const [k, v] of Object.entries(opts.headers)) {
      headerPairs.push(`"${escapeSemaString(k)}" "${escapeSemaString(v)}"`);
    }
  }
  const headersMap = `{${headerPairs.join(" ")}}`;
  interp.registerFunction("llm/proxy-url", () => proxyUrl);
  interp.registerFunction("__llm/chat-stream-raw", (messagesJson, optsJson) => {
    const messages = JSON.parse(messagesJson);
    const streamOpts = optsJson ? JSON.parse(optsJson) : {};
    const id = ctx.nextSignalId++;
    const s = signal3({
      text: "",
      done: false,
      error: null
    });
    ctx.signals.set(id, s);
    const headers = {
      "Content-Type": "application/json",
      ...opts.headers || {}
    };
    if (opts.token) {
      headers["Authorization"] = `Bearer ${opts.token}`;
    }
    function stripColonKeys3(obj) {
      if (Array.isArray(obj)) return obj.map(stripColonKeys3);
      if (obj && typeof obj === "object") {
        const out = {};
        for (const [k, v] of Object.entries(obj)) {
          out[k.startsWith(":") ? k.slice(1) : k] = stripColonKeys3(v);
        }
        return out;
      }
      return obj;
    }
    const body = JSON.stringify({
      messages: stripColonKeys3(messages),
      ...stripColonKeys3(streamOpts && typeof streamOpts === "object" ? streamOpts : {}),
      stream: true
    });
    let accumulated = "";
    const managedStream = openSseStream({
      url: `${proxyUrl}/stream`,
      method: "POST",
      headers,
      body,
      onEvent: (event) => {
        if (!event.data) return;
        try {
          const parsed = JSON.parse(event.data);
          if (parsed.type === "token" && typeof parsed.text === "string") {
            accumulated += parsed.text;
            s.value = { text: accumulated, done: false, error: null };
            return;
          }
          if (parsed.type === "done") {
            s.value = { text: accumulated, done: true, error: null };
            return;
          }
          if (parsed.type === "error") {
            s.value = {
              text: accumulated,
              done: true,
              error: typeof parsed.error === "string" ? parsed.error : "Stream error"
            };
          }
        } catch {
          s.value = { text: accumulated, done: true, error: "Invalid stream payload" };
        }
      },
      onError: (error) => {
        s.value = { text: accumulated, done: true, error: error.message };
      },
      onClose: () => {
        ctx.streams.delete(id);
        s.value = {
          text: accumulated,
          done: true,
          error: s.value.error
        };
      }
    });
    ctx.streams.set(id, {
      kind: "llm-stream",
      close: managedStream.close
    });
    const ownerId = getCurrentOwnerId(ctx);
    const owner = ownerId != null ? ctx.mountedComponentsById.get(ownerId) ?? null : null;
    if (owner) owner.ownedStreamIds.add(id);
    return id;
  });
  interp.registerFunction("__llm/close-stream", (signalId) => {
    const stream = ctx.streams.get(signalId);
    if (stream) {
      stream.close();
      ctx.streams.delete(signalId);
      for (const component of ctx.mountedComponents.values()) {
        component.ownedStreamIds.delete(signalId);
      }
    }
    const current = ctx.signals.get(signalId);
    if (current) {
      current.value = {
        ...current.value ?? {},
        done: true
      };
    }
    return null;
  });
  const semaCode = `
;; --- LLM proxy internals ---

(define __llm-proxy-url "${escapeSemaString(proxyUrl)}")
(define __llm-proxy-headers ${headersMap})

;; Helper: POST to the proxy and decode the JSON response body.
(define (__llm-proxy-post endpoint body-map)
  (let ((url (string-append __llm-proxy-url "/" endpoint))
        (resp (http/post url
                {:headers __llm-proxy-headers
                 :body (json/encode body-map)})))
    (if (and (map? resp) (:body resp))
      (json/decode (:body resp))
      resp)))

;; Helper: GET from the proxy.
(define (__llm-proxy-get endpoint)
  (let ((url (string-append __llm-proxy-url "/" endpoint))
        (resp (http/get url {:headers __llm-proxy-headers})))
    (if (and (map? resp) (:body resp))
      (json/decode (:body resp))
      resp)))

;; --- Public API ---

;; (message role content) \u2014 build a chat message map
(define (message role content)
  {:role (if (keyword? role) (keyword->string role) (->string role))
   :content content})

;; (llm/complete prompt) or (llm/complete prompt opts)
;; Send a simple prompt for completion.
(define (llm/complete prompt . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((body (merge {:prompt prompt} (if (map? opts) opts {}))))
      (let ((result (__llm-proxy-post "complete" body)))
        (if (map? result)
          (or (:content result) (:text result) result)
          result)))))

;; (llm/chat messages) or (llm/chat messages opts)
;; Chat with a list of message maps.
(define (llm/chat messages . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((body (merge {:messages messages} (if (map? opts) opts {}))))
      (let ((result (__llm-proxy-post "chat" body)))
        (if (map? result)
          (or (:content result) (:text result) result)
          result)))))

;; (llm/send prompt) or (llm/send prompt opts)
;; Send a prompt (list of messages or prompt object).
(define (llm/send prompt . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((messages (if (list? prompt) prompt (list prompt))))
      (let ((body (merge {:messages messages} (if (map? opts) opts {}))))
        (let ((result (__llm-proxy-post "chat" body)))
          (if (map? result)
            (or (:content result) (:text result) result)
            result))))))

;; (llm/extract schema text) or (llm/extract schema text opts)
;; Extract structured data from text.
(define (llm/extract schema text . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((body (merge {:schema schema :text text} (if (map? opts) opts {}))))
      (__llm-proxy-post "extract" body))))

;; (llm/classify categories text) or (llm/classify categories text opts)
;; Classify text into one of the given categories.
(define (llm/classify categories text . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((body (merge {:categories categories :text text} (if (map? opts) opts {}))))
      (__llm-proxy-post "classify" body))))

;; (llm/embed text) or (llm/embed text opts)
;; Get text embeddings.
(define (llm/embed text . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((body (merge {:text text} (if (map? opts) opts {}))))
      (__llm-proxy-post "embed" body))))

;; (llm/list-models)
;; List available models from the proxy.
(define (llm/list-models)
  (__llm-proxy-get "models"))

;; (llm/chat-stream messages) or (llm/chat-stream messages opts)
;; Streaming chat \u2014 returns a signal ID that updates as tokens arrive.
;; Signal value shape: {:text "" :done false :error nil}
(define (llm/chat-stream messages . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (__llm/chat-stream-raw
      (json/encode messages)
      (json/encode (if (map? opts) opts {})))))

(define (llm/close-stream signal-id)
  (__llm/close-stream signal-id))
`;
  const result = interp.evalStr(semaCode);
  if (result.error) {
    throw new Error(`[sema-web] Failed to register LLM bindings: ${result.error}`);
  }
}
function escapeSemaString(s) {
  return s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n").replace(/\r/g, "\\r").replace(/\t/g, "\\t");
}

// src/router.ts
import { signal as signal4 } from "@preact/signals-core";
function escapeRegexLiteral(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
function decodeRouteParam(value) {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}
function compileRoute(pattern) {
  const paramNames = [];
  let regexStr = "^";
  let lastIndex = 0;
  pattern.replace(/:([a-zA-Z_][a-zA-Z0-9_]*)/g, (match, name, offset) => {
    regexStr += escapeRegexLiteral(pattern.slice(lastIndex, offset));
    paramNames.push(name);
    regexStr += "([^/]+)";
    lastIndex = offset + match.length;
    return match;
  });
  regexStr += escapeRegexLiteral(pattern.slice(lastIndex));
  regexStr += "$";
  return { regex: new RegExp(regexStr), paramNames };
}
function registerRouterBindings(interp, ctx) {
  const routes = [];
  let removeHashChangeListener = null;
  const routeSignalId = ctx.nextSignalId++;
  const routeSignal = signal4(null);
  ctx.signals.set(routeSignalId, routeSignal);
  function matchRoute(path) {
    for (const route of routes) {
      const match = path.match(route.regex);
      if (match) {
        const params = {};
        route.paramNames.forEach((name, i) => {
          params[name] = decodeRouteParam(match[i + 1]);
        });
        return { path, params, handler: route.handler };
      }
    }
    return null;
  }
  function updateRoute() {
    const hash = window.location.hash.slice(1) || "/";
    routeSignal.value = matchRoute(hash);
  }
  interp.registerFunction("router/init!", (routeMap) => {
    routes.length = 0;
    for (let [pattern, handler] of Object.entries(routeMap)) {
      if (pattern.startsWith(":")) pattern = pattern.slice(1);
      if (typeof handler === "string" && handler.startsWith(":")) handler = handler.slice(1);
      const { regex, paramNames } = compileRoute(String(pattern));
      routes.push({ pattern: String(pattern), regex, paramNames, handler: String(handler) });
    }
    if (removeHashChangeListener) {
      removeHashChangeListener();
      ctx.cleanupHooks.delete(removeHashChangeListener);
    }
    window.addEventListener("hashchange", updateRoute);
    removeHashChangeListener = () => {
      window.removeEventListener("hashchange", updateRoute);
    };
    ctx.cleanupHooks.add(removeHashChangeListener);
    updateRoute();
    return null;
  });
  interp.registerFunction("router/push!", (path) => {
    window.location.hash = path;
    return null;
  });
  interp.registerFunction("router/replace!", (path) => {
    window.history.replaceState(null, "", `#${path}`);
    updateRoute();
    return null;
  });
  interp.registerFunction("router/back!", () => {
    window.history.back();
    return null;
  });
  interp.registerFunction("router/current", () => routeSignalId);
  const semaResult = interp.evalStr(`
    (define (router/current-route) (deref (router/current)))
  `);
  if (semaResult.error) {
    throw new Error(`[sema-web] Failed to register router wrappers: ${semaResult.error}`);
  }
}

// src/css.ts
function getStyleSheet(ctx) {
  if (!ctx.styleEl) {
    const styleEl = document.createElement("style");
    styleEl.setAttribute("data-sema-css", "");
    document.head.appendChild(styleEl);
    ctx.styleEl = styleEl;
  }
  return ctx.styleEl;
}
function generateRules(className, props, parentSelector) {
  const rules = [];
  const declarations = [];
  for (let [key, value] of Object.entries(props)) {
    if (key.startsWith(":")) key = key.slice(1);
    if (key.startsWith("&")) {
      const nestedSelector = parentSelector ? `${parentSelector}${key.slice(1)}` : `.${className}${key.slice(1)}`;
      if (typeof value === "object" && value !== null) {
        rules.push(...generateRules(className, value, nestedSelector));
      }
    } else {
      const cssProp = key.replace(/([A-Z])/g, "-$1").toLowerCase();
      declarations.push(`${cssProp}: ${value}`);
    }
  }
  if (declarations.length > 0) {
    const selector = parentSelector || `.${className}`;
    rules.push(`${selector} { ${declarations.join("; ")} }`);
  }
  return rules;
}
function registerCssBindings(interp, ctx) {
  interp.registerFunction("css/scoped", (props) => {
    const className = `sema-${ctx.cssNamespace}-${ctx.nextCssClassId++}`;
    const rules = generateRules(className, props);
    const sheet = getStyleSheet(ctx);
    for (const rule of rules) {
      sheet.sheet?.insertRule(rule, sheet.sheet.cssRules.length);
    }
    return className;
  });
  const semaResult = interp.evalStr(`
    (define (css props) (css/scoped props))
  `);
  if (semaResult.error) {
    throw new Error(`[sema-web] Failed to register css wrappers: ${semaResult.error}`);
  }
}

// src/http.ts
import { signal as signal5 } from "@preact/signals-core";
function getActiveComponent2(ctx) {
  const componentId = getCurrentOwnerId(ctx);
  return componentId != null ? ctx.mountedComponentsById.get(componentId) ?? null : null;
}
function stripColonKeys(obj) {
  if (Array.isArray(obj)) return obj.map(stripColonKeys);
  if (obj && typeof obj === "object") {
    const out = {};
    for (const [k, v] of Object.entries(obj)) {
      out[k.startsWith(":") ? k.slice(1) : k] = stripColonKeys(v);
    }
    return out;
  }
  return obj;
}
function normalizeStreamOptions(input, maybeOpts) {
  if (typeof input === "string") {
    const normalized2 = stripColonKeys(maybeOpts && typeof maybeOpts === "object" ? maybeOpts : {});
    return {
      url: input,
      method: normalized2.method,
      headers: normalized2.headers,
      body: normalized2.body,
      withCredentials: normalized2.withCredentials ?? normalized2["with-credentials"]
    };
  }
  const normalized = stripColonKeys(input && typeof input === "object" ? input : {});
  if (typeof normalized.url !== "string" || normalized.url.length === 0) {
    throw new Error("http/event-source expected a URL string or options map with :url");
  }
  return {
    url: normalized.url,
    method: normalized.method,
    headers: normalized.headers,
    body: normalized.body,
    withCredentials: normalized.withCredentials ?? normalized["with-credentials"]
  };
}
function closeManagedStream(ctx, signalId) {
  const stream = ctx.streams.get(signalId);
  if (!stream) return;
  stream.close();
  ctx.streams.delete(signalId);
  for (const component of ctx.mountedComponents.values()) {
    component.ownedStreamIds.delete(signalId);
  }
}
function registerHttpBindings(interp, ctx) {
  interp.registerFunction("http/event-source", (input, maybeOpts) => {
    const opts = normalizeStreamOptions(input, maybeOpts);
    const id = ctx.nextSignalId++;
    const s = signal5({
      data: null,
      event: null,
      id: null,
      retry: null,
      done: false,
      error: null,
      status: null,
      state: "connecting"
    });
    ctx.signals.set(id, s);
    const managedStream = openSseStream({
      url: opts.url,
      method: opts.method,
      headers: opts.headers,
      body: opts.body,
      credentials: opts.withCredentials ? "include" : "same-origin",
      onOpen: (response) => {
        s.value = {
          ...s.value,
          state: "open",
          status: response.status,
          error: null
        };
      },
      onEvent: (event) => {
        s.value = {
          data: event.data,
          event: event.event ?? "message",
          id: event.id,
          retry: event.retry,
          done: false,
          error: null,
          status: s.value.status,
          state: "open"
        };
      },
      onError: (error) => {
        s.value = {
          ...s.value,
          done: true,
          error: error.message,
          state: "closed"
        };
      },
      onClose: () => {
        ctx.streams.delete(id);
        s.value = {
          ...s.value,
          done: true,
          state: "closed"
        };
      }
    });
    ctx.streams.set(id, {
      kind: "event-source",
      close: managedStream.close
    });
    const owner = getActiveComponent2(ctx);
    if (owner) owner.ownedStreamIds.add(id);
    return id;
  });
  interp.registerFunction("http/close-event-source", (signalId) => {
    closeManagedStream(ctx, signalId);
    const current = ctx.signals.get(signalId);
    if (current) {
      current.value = {
        ...current.value ?? {},
        done: true,
        state: "closed"
      };
    }
    return null;
  });
  interp.registerFunction("http/close-stream", (signalId) => {
    closeManagedStream(ctx, signalId);
    const current = ctx.signals.get(signalId);
    if (current) {
      current.value = {
        ...current.value ?? {},
        done: true,
        state: "closed"
      };
    }
    return null;
  });
}

// src/ws.ts
function stripColonKeys2(obj) {
  if (Array.isArray(obj)) return obj.map(stripColonKeys2);
  if (obj && typeof obj === "object") {
    const out = {};
    for (const [k, v] of Object.entries(obj)) {
      out[k.startsWith(":") ? k.slice(1) : k] = stripColonKeys2(v);
    }
    return out;
  }
  return obj;
}
function mapGet(obj, key) {
  if (!obj || typeof obj !== "object") return void 0;
  return `:${key}` in obj ? obj[`:${key}`] : obj[key];
}
function getReg(ctx, handle) {
  const reg = typeof handle === "number" ? ctx.sockets.get(handle) : void 0;
  if (!reg) throw new Error("ws: invalid or closed connection handle");
  return reg;
}
function toBinary(value) {
  if (value instanceof Uint8Array) return value;
  if (ArrayBuffer.isView(value)) return value;
  if (Array.isArray(value) && value.every((n) => typeof n === "number")) {
    return Uint8Array.from(value);
  }
  return null;
}
function encodeSend(msg) {
  if (msg && typeof msg === "object" && !Array.isArray(msg) && !(msg instanceof Uint8Array)) {
    const text = mapGet(msg, "text");
    if (text !== void 0) return String(text);
    const bin2 = mapGet(msg, "binary");
    if (bin2 !== void 0) {
      const b = toBinary(bin2);
      if (b) return b;
      throw new Error("ws/send: {:binary \u2026} expects a bytevector");
    }
    const json = mapGet(msg, "json");
    if (json !== void 0) return JSON.stringify(stripColonKeys2(json));
    return JSON.stringify(stripColonKeys2(msg));
  }
  if (typeof msg === "string") return msg;
  const bin = toBinary(msg);
  if (bin) return bin;
  return JSON.stringify(stripColonKeys2(msg));
}
function registerWsBindings(interp, ctx) {
  interp.registerFunction("ws/connect", (url, opts) => {
    if (typeof url !== "string" || url.length === 0) {
      throw new Error("ws/connect expects a ws:// or wss:// URL string");
    }
    const subprotocols = opts ? mapGet(opts, "subprotocols") : void 0;
    const socket = subprotocols !== void 0 ? new WebSocket(url, subprotocols) : new WebSocket(url);
    socket.binaryType = "arraybuffer";
    const handle = ctx.nextSocketId++;
    ctx.sockets.set(handle, { socket, callbacks: [] });
    return handle;
  });
  interp.registerFunction("ws/send", (handle, msg) => {
    const { socket } = getReg(ctx, handle);
    if (socket.readyState !== WebSocket.OPEN) {
      throw new Error("ws/send: connection is not open");
    }
    socket.send(encodeSend(msg));
    return null;
  });
  interp.registerFunction("ws/connected?", (handle) => {
    const reg = typeof handle === "number" ? ctx.sockets.get(handle) : void 0;
    return !!reg && reg.socket.readyState === WebSocket.OPEN;
  });
  interp.registerFunction("ws/close", (handle, code, reason) => {
    const reg = typeof handle === "number" ? ctx.sockets.get(handle) : void 0;
    if (!reg) return null;
    const { socket } = reg;
    socket.onopen = null;
    socket.onmessage = null;
    socket.onerror = null;
    if (!socket.onclose) {
      socket.onclose = () => {
        if (typeof handle === "number") ctx.sockets.delete(handle);
        for (const cb of reg.callbacks) releaseCallback(cb);
      };
    }
    try {
      if (typeof code === "number") socket.close(code, reason);
      else socket.close();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), "ws/close");
    }
    return null;
  });
  interp.registerFunction(
    "__ws/listen",
    (handle, onOpenV, onMessageV, onCloseV, onErrorV) => {
      const reg = getReg(ctx, handle);
      const wire = (v, label) => {
        if (v === void 0 || v === null) return null;
        const cb = toInvokableCallback(v, interp, `ws/listen ${label}`);
        reg.callbacks.push(cb);
        return cb;
      };
      const onOpen = wire(onOpenV, "on-open");
      const onMessage = wire(onMessageV, "on-message");
      const onClose = wire(onCloseV, "on-close");
      const onError = wire(onErrorV, "on-error");
      const { socket } = reg;
      if (onOpen) {
        if (socket.readyState === WebSocket.OPEN) {
          try {
            onOpen(handle);
          } catch (e) {
            ctx.onerror(e instanceof Error ? e : new Error(String(e)), "ws/listen on-open");
          }
        } else {
          socket.onopen = () => onOpen(handle);
        }
      }
      if (onMessage) {
        socket.onmessage = (ev) => {
          const data = ev.data instanceof ArrayBuffer ? new Uint8Array(ev.data) : ev.data;
          onMessage(handle, data);
        };
      }
      socket.onclose = (ev) => {
        if (onClose) {
          onClose(handle, { ":code": ev.code, ":reason": ev.reason });
        }
        if (typeof handle === "number") ctx.sockets.delete(handle);
        for (const cb of reg.callbacks) releaseCallback(cb);
      };
      if (onError) {
        socket.onerror = () => onError(handle, "websocket error");
      }
      return handle;
    }
  );
  const wrapper = interp.evalStr(`
    (define ws/listen
      (fn (conn handlers)
        (__ws/listen conn
          (get handlers :on-open)
          (get handlers :on-message)
          (get handlers :on-close)
          (get handlers :on-error))))`);
  if (wrapper.error) {
    throw new Error(`ws/listen wrapper failed to install: ${wrapper.error}`);
  }
}

// src/loader.ts
async function loadScripts(interp, opts) {
  const mimeType = opts?.type ?? "text/sema";
  const scripts = document.querySelectorAll(`script[type="${mimeType}"]`);
  const results = [];
  for (const script of scripts) {
    const src = script.getAttribute("src");
    let code;
    if (src) {
      try {
        const resp = await fetch(src);
        if (!resp.ok) {
          const err = `Failed to fetch ${src}: ${resp.status} ${resp.statusText}`;
          console.error(`[sema-web] ${err}`);
          results.push({ value: null, output: [], error: err });
          continue;
        }
        const artifactKind = classifyExternalScript(src);
        if (artifactKind === "archive") {
          if (!interp.loadArchive || !interp.runEntry && !interp.runEntryAsync) {
            const err = `Runtime does not support compiled web archives: ${src}`;
            console.error(`[sema-web] ${err}`);
            results.push({ value: null, output: [], error: err });
            continue;
          }
          const bytes = new Uint8Array(await resp.arrayBuffer());
          let archiveInfo;
          try {
            archiveInfo = interp.loadArchive(bytes);
          } catch (e) {
            const err = `Failed to load archive ${src}: ${e instanceof Error ? e.message : String(e)}`;
            console.error(`[sema-web] ${err}`);
            results.push({ value: null, output: [], error: err });
            continue;
          }
          if (!archiveInfo.ok) {
            const err = archiveInfo.error || `Failed to load archive ${src}`;
            console.error(`[sema-web] ${err}`);
            results.push({ value: null, output: [], error: err });
            continue;
          }
          if (!archiveInfo.entryPoint) {
            const err = `Archive ${src} did not provide an entry point`;
            console.error(`[sema-web] ${err}`);
            results.push({ value: null, output: [], error: err });
            continue;
          }
          const result = interp.runEntryAsync ? await interp.runEntryAsync(archiveInfo.entryPoint) : interp.runEntry(archiveInfo.entryPoint);
          for (const line of result.output) {
            console.log(`[sema] ${line}`);
          }
          if (result.error) {
            console.error(`[sema-web] Error in ${src}: ${result.error}`);
          }
          results.push(result);
          continue;
        }
        code = await resp.text();
      } catch (e) {
        const err = `Failed to fetch ${src}: ${e instanceof Error ? e.message : String(e)}`;
        console.error(`[sema-web] ${err}`);
        results.push({ value: null, output: [], error: err });
        continue;
      }
    } else {
      code = script.textContent ?? "";
    }
    if (!code.trim()) {
      results.push({ value: null, output: [], error: null });
      continue;
    }
    try {
      const result = interp.evalStrAsync ? await interp.evalStrAsync(code) : interp.evalStr(code);
      for (const line of result.output) {
        console.log(`[sema] ${line}`);
      }
      if (result.error) {
        console.error(`[sema-web] Error in ${src ?? "inline script"}: ${result.error}`);
      }
      results.push(result);
    } catch (e) {
      const err = `Evaluation error: ${e instanceof Error ? e.message : String(e)}`;
      console.error(`[sema-web] ${err}`);
      results.push({ value: null, output: [], error: err });
    }
  }
  return results;
}
function classifyExternalScript(src) {
  const url = new URL(src, document.baseURI);
  if (url.pathname.endsWith(".vfs")) {
    return "archive";
  }
  return "source";
}

// src/index.ts
var SemaWeb = class _SemaWeb {
  constructor(interp, ctx) {
    this._interp = interp;
    this._ctx = ctx;
  }
  /**
   * Create a SemaWeb instance with browser bindings registered.
   *
   * @param opts - Configuration options
   * @returns A ready-to-use SemaWeb instance
   */
  static async create(opts) {
    const interp = await SemaInterpreter.create(opts);
    const ctx = new SemaWebContext();
    const web = new _SemaWeb(interp, ctx);
    if (opts?.dom !== false) {
      registerDomBindings(interp, ctx);
    }
    if (opts?.store !== false) {
      registerStoreBindings(interp, ctx);
    }
    if (opts?.console !== false) {
      registerConsoleBindings(interp);
    }
    if (opts?.reactive !== false || opts?.components !== false) {
      registerReactiveBindings(interp, ctx);
    }
    if (opts?.sip !== false || opts?.components !== false) {
      registerSipBindings(interp, ctx);
    }
    if (opts?.components !== false) {
      registerComponentBindings(interp, ctx);
    }
    if (opts?.router !== false) {
      registerRouterBindings(interp, ctx);
    }
    if (opts?.css !== false) {
      registerCssBindings(interp, ctx);
    }
    if (opts?.http !== false) {
      registerHttpBindings(interp, ctx);
    }
    if (opts?.websocket !== false) {
      registerWsBindings(interp, ctx);
    }
    if (opts?.llmProxy) {
      const proxyOpts = typeof opts.llmProxy === "string" ? { url: opts.llmProxy } : opts.llmProxy;
      registerLlmBindings(interp, proxyOpts, ctx);
    }
    if (opts?.autoLoad !== false) {
      await loadScripts(interp, opts?.loader);
    }
    return web;
  }
  /**
   * Convenience: create a SemaWeb instance with default options and auto-load scripts.
   *
   * Equivalent to `SemaWeb.create()` — discovers and evaluates all
   * `<script type="text/sema">` tags in the document.
   *
   * @param opts - Configuration options
   * @returns A ready-to-use SemaWeb instance
   */
  static async init(opts) {
    return _SemaWeb.create(opts);
  }
  /**
   * Evaluate a string of Sema code with browser bindings available.
   *
   * @param code - Sema source code
   * @returns The evaluation result
   */
  eval(code) {
    return this._interp.evalStr(code);
  }
  /**
   * Evaluate Sema code with async HTTP support.
   *
   * @param code - Sema source code
   * @returns The evaluation result
   */
  async evalAsync(code) {
    return this._interp.evalStrAsync(code);
  }
  /**
   * Register a JavaScript function callable from Sema code.
   *
   * @param name - Function name in Sema
   * @param fn - JavaScript function
   */
  registerFunction(name, fn) {
    this._interp.registerFunction(name, fn);
  }
  /**
   * Preload a Sema module so that `(import "name")` works.
   *
   * @param name - Module name
   * @param source - Sema source code
   */
  preloadModule(name, source) {
    this._interp.preloadModule(name, source);
  }
  /**
   * Get the underlying SemaInterpreter instance.
   *
   * Useful for advanced operations like VFS access.
   */
  get interpreter() {
    return this._interp;
  }
  /**
   * Get the SemaWebContext instance.
   *
   * Useful for advanced operations requiring direct context access.
   */
  get context() {
    return this._ctx;
  }
  /**
   * Get the Sema interpreter version.
   */
  version() {
    return this._interp.version();
  }
  /**
   * Free the interpreter's WASM memory.
   * The instance cannot be used after calling this method.
   */
  dispose() {
    disposeAllComponents(this._ctx);
    disposeContextResources(this._ctx);
    this._interp.dispose();
  }
};
function registerConsoleBindings(interp) {
  interp.registerFunction("console/log", (...args) => {
    console.log(...args);
    return null;
  });
  interp.registerFunction("console/warn", (...args) => {
    console.warn(...args);
    return null;
  });
  interp.registerFunction("console/error", (...args) => {
    console.error(...args);
    return null;
  });
  interp.registerFunction("console/info", (...args) => {
    console.info(...args);
    return null;
  });
  interp.registerFunction("console/debug", (...args) => {
    console.debug(...args);
    return null;
  });
  interp.registerFunction("console/clear", () => {
    console.clear();
    return null;
  });
  interp.registerFunction("console/time", (label) => {
    console.time(label);
    return null;
  });
  interp.registerFunction("console/time-end", (label) => {
    console.timeEnd(label);
    return null;
  });
}
export {
  SemaWeb,
  SemaWebContext,
  loadScripts,
  registerComponentBindings,
  registerCssBindings,
  registerDomBindings,
  registerHttpBindings,
  registerLlmBindings,
  registerReactiveBindings,
  registerRouterBindings,
  registerSipBindings,
  registerStoreBindings,
  renderSip
};
//# sourceMappingURL=index.js.map