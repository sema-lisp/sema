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
var owners = /* @__PURE__ */ new WeakMap();
function retainCallback(value) {
  if (typeof value !== "function") return;
  const callback = value;
  if (!callback.__semaRelease) return;
  owners.set(callback, (owners.get(callback) ?? 0) + 1);
}
function toInvokableCallback(value, interp, label) {
  if (typeof value === "function") {
    retainCallback(value);
    return value;
  }
  if (typeof value === "string" && SEMA_IDENT_RE.test(value)) {
    return ((...args) => interp.invokeGlobal(value, ...args));
  }
  throw new Error(`Invalid ${label}: expected function value or callback name`);
}
function releaseCallback(value) {
  if (typeof value !== "function") return;
  const callback = value;
  const held = owners.get(callback) ?? 0;
  if (held > 1) {
    owners.set(callback, held - 1);
    return;
  }
  owners.delete(callback);
  callback.__semaRelease?.();
}

// src/diagnostics.ts
var DEFAULT_DEV_LIMIT = 100;
var MAX_DETAIL_CHARS = 1024;
function boundDetail(detail) {
  if (detail.length <= MAX_DETAIL_CHARS) return detail;
  const dropped = detail.length - MAX_DETAIL_CHARS;
  return `${detail.slice(0, MAX_DETAIL_CHARS)}\u2026 (+${dropped} chars)`;
}
var DEFAULT_SLOW_RENDER_MS = 16;
function now() {
  return typeof performance !== "undefined" && typeof performance.now === "function" ? performance.now() : Date.now();
}
var Diagnostics = class {
  constructor(opts) {
    this.entries = [];
    this.listeners = /* @__PURE__ */ new Set();
    this.droppedCount = 0;
    this.enabled = opts?.enabled ?? false;
    const limit = opts?.limit;
    this.limit = typeof limit === "number" && limit > 0 ? Math.floor(limit) : DEFAULT_DEV_LIMIT;
    this.slowRenderMs = opts?.slowRenderMs ?? DEFAULT_SLOW_RENDER_MS;
  }
  /**
   * Record an event.
   *
   * The entry is passed as a thunk so that nothing is allocated — no object,
   * no string interpolation for `detail` — when recording is disabled, which
   * is the production default.
   */
  record(build) {
    if (!this.enabled) return;
    let entry;
    try {
      entry = build();
    } catch {
      return;
    }
    if (typeof entry.detail === "string") entry.detail = boundDetail(entry.detail);
    this.entries.push(entry);
    if (this.entries.length > this.limit) {
      this.entries.shift();
      this.droppedCount++;
    }
    for (const listener of this.listeners) {
      try {
        listener(entry);
      } catch {
      }
    }
  }
  /** All retained entries, oldest first. */
  all() {
    return this.entries.slice();
  }
  /** Retained entries of one kind, oldest first. */
  byKind(kind) {
    return this.entries.filter((e) => e.kind === kind);
  }
  /** Retained render entries at or over {@link slowRenderMs}. */
  slowRenders() {
    return this.entries.filter(
      (e) => e.kind === "render" && (e.durationMs ?? 0) >= this.slowRenderMs
    );
  }
  /** How many entries have been evicted by the size bound so far. */
  get dropped() {
    return this.droppedCount;
  }
  /** Drop all retained entries. Does not reset {@link dropped}. */
  clear() {
    this.entries.length = 0;
  }
  /**
   * Observe entries as they are recorded. Returns an unsubscribe function.
   *
   * Subscribing does not replay history — read {@link all} for that.
   */
  subscribe(listener) {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }
  /** Drop every subscriber. Called on context teardown. */
  clearListeners() {
    this.listeners.clear();
  }
};
function resolveDevOptions(dev) {
  if (!dev) {
    return {
      enabled: false,
      limit: DEFAULT_DEV_LIMIT,
      overlay: false,
      slowRenderMs: DEFAULT_SLOW_RENDER_MS
    };
  }
  const opts = dev === true ? {} : dev;
  const limit = opts.limit;
  return {
    enabled: true,
    limit: typeof limit === "number" && limit > 0 ? Math.floor(limit) : DEFAULT_DEV_LIMIT,
    overlay: opts.overlay === true,
    slowRenderMs: opts.slowRenderMs ?? DEFAULT_SLOW_RENDER_MS
  };
}

// src/context.ts
function createRenderScopedReactive() {
  return {
    computeds: [],
    watches: /* @__PURE__ */ new Map(),
    claimed: /* @__PURE__ */ new Set(),
    occurrences: /* @__PURE__ */ new Map(),
    reported: /* @__PURE__ */ new Set()
  };
}
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
    /**
     * Event names some render has emitted a `.capture` modifier for.
     *
     * Permission to do work, not a source of truth: the delegator skips its
     * capture-phase ancestor walk entirely for events not listed here, so an app
     * with no `.capture` anywhere pays one Set lookup per event instead of a
     * second walk. A stale entry costs one wasted walk; it can never lose a
     * handler, because entries are only ever added.
     *
     * Per-instance — a module-level Set would leak `.capture` from one SemaWeb
     * instance into another's dispatch cost.
     */
    this.captureEvents = /* @__PURE__ */ new Set();
    /** Current execution owner stack for callbacks invoked outside render. */
    this.ownerStack = [];
    /**
     * Composed-child scope chain for the render in flight, innermost last.
     *
     * `component/render` invokes a child as an ordinary Sema call inside the
     * parent's render, so the render-context stack still names the *mounting*
     * component. Without a second axis every child's `effect` / `on-unmount` /
     * `local` / `resource` lands in the mounting component's bucket, matched
     * positionally — so switching a branch keeps the old child's effect running,
     * removing a keyed row tears down a different row's, and every row of a list
     * shares one `local` cell.
     *
     * Each entry is `name#key`, where the key is the child's `:key` prop when it
     * has one and its ordinal among same-named siblings otherwise. Joining the
     * stack gives a path that is stable across renders for the same child
     * instance — the same identity that already governs DOM identity.
     */
    this.childScopeStack = [];
    /**
     * Per-parent-scope counters for un-keyed children, reset each render.
     *
     * A child with no `:key` gets its ordinal among same-named siblings. Stable
     * as long as the sibling set is, which is exactly the condition under which
     * an un-keyed list is stable anyway.
     */
    this.childScopeCounters = /* @__PURE__ */ new Map();
    /** DOM event listeners registry */
    this.listeners = /* @__PURE__ */ new Map();
    /** Reactive watch cleanup callbacks */
    this.watchDisposers = /* @__PURE__ */ new Map();
    this.nextWatchId = 1;
    /** Browser interval handles */
    this.intervals = /* @__PURE__ */ new Map();
    /** Managed streaming resources keyed by signal id */
    this.streams = /* @__PURE__ */ new Map();
    /**
     * Async resources keyed by signal id.
     *
     * Drained by each resource's signal finalizer, not by a defensive `clear()`
     * in {@link disposeContextResources}: a leftover entry after dispose means a
     * finalizer did not run, which also means a Sema callback handle leaked, and
     * clearing the map here would hide exactly that.
     */
    this.resources = /* @__PURE__ */ new Map();
    /** Named resources, `<ownerId|global>:<scope path>:<name>` to signal id. */
    this.resourcesByKey = /* @__PURE__ */ new Map();
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
    /**
     * Bounded dev-mode event recorder. Disabled unless `SemaWeb.create({dev})`
     * turned it on, in which case `record()` is a no-op that allocates nothing.
     */
    this.diagnostics = new Diagnostics();
    this._onerror = (error, context) => {
      console.error(`[sema-web] Error in ${context}:`, error);
    };
  }
  /**
   * Error handler invoked for every caught failure — render, event listener,
   * cleanup, stream, loader.
   *
   * This is an accessor rather than a plain field on purpose. Reading it
   * returns a wrapper that records the error into {@link diagnostics} *before*
   * delegating to the installed handler, so every existing `ctx.onerror(...)`
   * call site is covered without being rewritten, and diagnostics survive an
   * app (or a test) replacing the handler with its own. Assigning replaces only
   * the delegate; the recording layer cannot be assigned away.
   */
  get onerror() {
    return (error, context) => {
      this.diagnostics.record(() => ({
        kind: "error",
        at: Date.now(),
        context,
        detail: error?.message ?? String(error)
      }));
      this._onerror(error, context);
    };
  }
  set onerror(handler) {
    this._onerror = handler;
  }
};
function registerStream(ctx, id, registration) {
  ctx.streams.set(id, registration);
  ctx.diagnostics.record(() => ({
    kind: "stream",
    at: Date.now(),
    context: `${registration.kind}:${id}`,
    detail: "open"
  }));
}
function unregisterStream(ctx, id) {
  const registration = ctx.streams.get(id);
  if (!registration) return;
  ctx.streams.delete(id);
  ctx.diagnostics.record(() => ({
    kind: "stream",
    at: Date.now(),
    context: `${registration.kind}:${id}`,
    detail: "close"
  }));
}
function currentScopePath(ctx) {
  return ctx.childScopeStack.join("/");
}
function safely(component, ctx, fn) {
  try {
    fn();
  } catch (e) {
    ctx.onerror(
      e instanceof Error ? e : new Error(String(e)),
      `render-reactive-cleanup:${component.componentFn}`
    );
  }
}
function beginRenderScopedReactive(component, ctx) {
  const owned = component.renderReactive;
  for (const dispose of owned.computeds.splice(0)) safely(component, ctx, dispose);
  owned.claimed.clear();
  owned.occurrences.clear();
}
function endRenderScopedReactive(component, ctx) {
  const owned = component.renderReactive;
  for (const [key, watch] of owned.watches) {
    if (owned.claimed.has(key)) continue;
    owned.watches.delete(key);
    safely(component, ctx, watch.dispose);
  }
}
function localScopes(component) {
  const scopes = /* @__PURE__ */ new Set();
  for (const key of component.localState.keys()) {
    scopes.add(key.slice(0, key.indexOf("\0")));
  }
  return scopes;
}
function disposeLocalScope(component, ctx, scope) {
  const prefix = `${scope}\0`;
  for (const [key, signalId] of component.localState) {
    if (!key.startsWith(prefix)) continue;
    component.localState.delete(key);
    try {
      disposeSignal(ctx, signalId);
    } catch (e) {
      ctx.onerror(
        e instanceof Error ? e : new Error(String(e)),
        `component-local-state-cleanup:${component.componentFn}`
      );
    }
  }
}
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
  ctx.diagnostics.clearListeners();
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
var DELEGATED_EVENTS = [
  // Mouse
  "click",
  "dblclick",
  "auxclick",
  "contextmenu",
  "mousedown",
  "mouseup",
  "mousemove",
  "mouseover",
  "mouseout",
  "wheel",
  // Pointer
  "pointerdown",
  "pointerup",
  "pointermove",
  "pointerover",
  "pointerout",
  "pointercancel",
  // Touch
  "touchstart",
  "touchend",
  "touchmove",
  "touchcancel",
  // Keyboard
  "keydown",
  "keyup",
  "keypress",
  // Form
  "input",
  "change",
  "submit",
  "reset",
  "select",
  // Focus (the bubbling pair)
  "focusin",
  "focusout",
  // Clipboard
  "copy",
  "cut",
  "paste",
  // Drag and drop
  "drag",
  "dragstart",
  "dragend",
  "dragenter",
  "dragleave",
  "dragover",
  "drop",
  // Animation and transition
  "animationstart",
  "animationend",
  "transitionend"
];
var SYNTHETIC_EVENTS = ["mouseenter", "mouseleave"];
var ROUTABLE_EVENTS = /* @__PURE__ */ new Set([
  ...DELEGATED_EVENTS,
  ...SYNTHETIC_EVENTS
]);
var NON_BUBBLING_ALTERNATIVES = {
  focus: "focusin",
  blur: "focusout"
};
function editDistance(a, b) {
  const previous = new Array(b.length + 1);
  for (let j = 0; j <= b.length; j++) previous[j] = j;
  for (let i = 1; i <= a.length; i++) {
    let diagonal = previous[0];
    previous[0] = i;
    for (let j = 1; j <= b.length; j++) {
      const candidate = Math.min(
        previous[j] + 1,
        previous[j - 1] + 1,
        diagonal + (a[i - 1] === b[j - 1] ? 0 : 1)
      );
      diagonal = previous[j];
      previous[j] = candidate;
    }
  }
  return previous[b.length];
}
function nearestRoutableEvent(name) {
  const lower = name.toLowerCase();
  let best = null;
  let bestDistance = Infinity;
  for (const candidate of ROUTABLE_EVENTS) {
    const distance = editDistance(lower, candidate);
    if (distance < bestDistance) {
      bestDistance = distance;
      best = candidate;
    }
  }
  return best !== null && bestDistance <= 2 && bestDistance < lower.length ? best : null;
}
function delegationError(event, key) {
  if (ROUTABLE_EVENTS.has(event)) return null;
  const alternative = NON_BUBBLING_ALTERNATIVES[event];
  if (alternative) {
    return `Event "${event}" in attribute: ${key} is not delegated because it does not bubble to the mount root. Use "${alternative}", which does, or attach it directly with (dom/on! \u2026) from an on-mount callback.`;
  }
  const suggestion = nearestRoutableEvent(event);
  return `Unknown event "${event}" in attribute: ${key}. ` + (suggestion ? `Did you mean "${suggestion}"? ` : "") + "SIP delegates a fixed set of bubbling DOM events; for anything else (a custom element's event, or a non-bubbling one like scroll) attach the listener directly with (dom/on! \u2026) from an on-mount callback.";
}
var EVENT_MODIFIERS = ["prevent", "stop", "once", "capture", "self"];
var NO_EVENT_MODIFIERS = Object.freeze({
  prevent: false,
  stop: false,
  once: false,
  capture: false,
  self: false
});
var SIP_EVENT_MODS_PREFIX = "data-sema-mods-";
function eventModsAttr(event) {
  return `${SIP_EVENT_MODS_PREFIX}${event}`;
}
function parseEventAttrKey(key) {
  const body = key.slice(3);
  const dot = body.indexOf(".");
  const event = dot === -1 ? body : body.slice(0, dot);
  if (!EVENT_NAME_RE.test(event)) {
    return { ok: false, error: `Invalid event handler attribute: ${key}` };
  }
  if (dot === -1) {
    return { ok: true, event, modifiers: [] };
  }
  const seen = /* @__PURE__ */ new Set();
  for (const name of body.slice(dot + 1).split(".")) {
    if (!EVENT_MODIFIERS.includes(name)) {
      return {
        ok: false,
        error: `Invalid event modifier ${JSON.stringify(name)} in attribute: ${key} (supported: ${EVENT_MODIFIERS.join(", ")})`
      };
    }
    seen.add(name);
  }
  return { ok: true, event, modifiers: EVENT_MODIFIERS.filter((name) => seen.has(name)) };
}
function readEventModifiers(el, event) {
  const encoded = el.getAttribute(eventModsAttr(event));
  if (!encoded) return NO_EVENT_MODIFIERS;
  const mods = {
    prevent: false,
    stop: false,
    once: false,
    capture: false,
    self: false
  };
  for (const name of encoded.split(" ")) {
    if (Object.prototype.hasOwnProperty.call(mods, name)) {
      mods[name] = true;
    }
  }
  return mods;
}
var SIP_KEY_ATTR = "data-sema-key";
function sipNodeKey(node) {
  if (node.nodeType !== 1) return void 0;
  const el = node;
  return el.getAttribute(SIP_KEY_ATTR) || el.id || void 0;
}
function reportDuplicateKeys(el, ctx, tagName) {
  const seen = /* @__PURE__ */ new Set();
  for (const child of Array.from(el.children)) {
    const key = child.getAttribute(SIP_KEY_ATTR);
    if (key == null) continue;
    if (seen.has(key)) {
      ctx.onerror(
        new Error(
          `Duplicate SIP :key ${JSON.stringify(key)} among the children of <${tagName}>. Keyed siblings must be unique or DOM state will move between them.`
        ),
        `sip-render:duplicate-key:${tagName}`
      );
      continue;
    }
    seen.add(key);
  }
}
function declaresOnce(el) {
  for (const attr of Array.from(el.attributes)) {
    if (!attr.name.startsWith(SIP_EVENT_MODS_PREFIX)) continue;
    if (attr.value.split(" ").includes("once")) return true;
  }
  return false;
}
function reportKeylessOnce(el, ctx, tagName) {
  const children = Array.from(el.children);
  if (children.length < 2) return;
  const unkeyedTags = /* @__PURE__ */ new Map();
  for (const child of children) {
    if (child.hasAttribute(SIP_KEY_ATTR) || child.id) continue;
    unkeyedTags.set(child.tagName, (unkeyedTags.get(child.tagName) ?? 0) + 1);
  }
  for (const child of children) {
    if (child.hasAttribute(SIP_KEY_ATTR) || child.id) continue;
    if ((unkeyedTags.get(child.tagName) ?? 0) < 2) continue;
    if (!declaresOnce(child)) continue;
    ctx.onerror(
      new Error(
        `A .once handler on an unkeyed <${child.tagName.toLowerCase()}> with unkeyed siblings of the same tag, among the children of <${tagName}>. .once is spent per DOM element and morphdom matches unkeyed siblings by position, so reordering moves the spent handler onto a different item. Give each sibling a :key.`
      ),
      `sip-render:once-without-key:${tagName}`
    );
    return;
  }
}
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
    if (ctx.diagnostics.enabled) {
      reportDuplicateKeys(el, ctx, tagName);
      reportKeylessOnce(el, ctx, tagName);
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
          const parsed = parseEventAttrKey(key);
          if (!parsed.ok) {
            ctx.onerror(new Error(parsed.error), "sip-render:on-handler");
            continue;
          }
          const eventName = parsed.event;
          const unroutable = delegationError(eventName, key);
          if (unroutable) {
            ctx.onerror(new Error(unroutable), "sip-render:on-handler");
            continue;
          }
          if (typeof value === "string") {
            if (!SEMA_IDENT_RE.test(value)) {
              ctx.onerror(new Error(`Invalid event handler name: ${value}`), "sip-render:on-handler");
              continue;
            }
            el.setAttribute(`data-sema-on-${eventName}`, value);
            if (parsed.modifiers.length > 0) {
              el.setAttribute(eventModsAttr(eventName), parsed.modifiers.join(" "));
              if (parsed.modifiers.includes("capture")) ctx.captureEvents.add(eventName);
            }
          } else {
            ctx.onerror(
              new Error(`Event handler value for "${key}" must be a string function name, got: ${typeof value}`),
              "sip-render:on-handler"
            );
          }
        } else if (key === "key") {
          el.setAttribute(SIP_KEY_ATTR, String(value));
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
function resolveForm(node) {
  if (!(node instanceof Element)) return null;
  if (node.tagName === "FORM") return node;
  const owner = node.form;
  if (owner) return owner;
  return node.closest("form");
}
function formDataToFields(fd) {
  const out = /* @__PURE__ */ Object.create(null);
  for (const [name, entry] of fd.entries()) {
    const value = typeof entry === "string" ? entry : { name: entry.name, size: entry.size, type: entry.type };
    if (!(name in out)) {
      out[name] = value;
    } else if (Array.isArray(out[name])) {
      out[name].push(value);
    } else {
      out[name] = [out[name], value];
    }
  }
  return out;
}
function formFields(form, submitter) {
  if (submitter) {
    try {
      return formDataToFields(new FormData(form, submitter));
    } catch {
    }
  }
  return formDataToFields(new FormData(form));
}
function selectedValues(el) {
  const options = el.selectedOptions;
  if (!options) return [];
  return Array.from(options).map((option) => option.value);
}
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
  interp.registerFunction("dom/event-current-target", (evId) => {
    const ev = getEvent(evId, ctx);
    const declaring = ev.__sema_current_target;
    if (declaring instanceof Element) return storeHandle(declaring, ctx);
    const current = ev.currentTarget;
    return current instanceof Element ? storeHandle(current, ctx) : null;
  });
  interp.registerFunction("dom/event-form-data", (evId) => {
    const ev = getEvent(evId, ctx);
    const form = resolveForm(ev.target);
    if (!form) return null;
    return formFields(form, ev.submitter);
  });
  interp.registerFunction("dom/form-data", (id) => {
    const form = resolveForm(getElement(id, ctx));
    if (!form) return null;
    return formFields(form);
  });
  interp.registerFunction("dom/event-form", (evId) => {
    const ev = getEvent(evId, ctx);
    return storeHandle(resolveForm(ev.target), ctx);
  });
  interp.registerFunction("dom/event-checked", (evId) => {
    const ev = getEvent(evId, ctx);
    const target = ev.target;
    if (target && "checked" in target) return Boolean(target.checked);
    return null;
  });
  interp.registerFunction("dom/checked?", (id) => {
    const el = getElement(id, ctx);
    return "checked" in el ? Boolean(el.checked) : false;
  });
  interp.registerFunction("dom/selected-values", (id) => {
    return selectedValues(getElement(id, ctx));
  });
  interp.registerFunction("dom/event-selected-values", (evId) => {
    const ev = getEvent(evId, ctx);
    const target = ev.target;
    if (!(target instanceof Element)) return null;
    return selectedValues(target);
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
  const guarded = (context, fallback, run) => {
    try {
      return run();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), context);
      return fallback;
    }
  };
  interp.registerFunction(
    "store/get",
    (key) => guarded(`store/get:${key}`, null, () => {
      const val = localStorage.getItem(key);
      if (val === null) return null;
      return JSON.parse(val);
    })
  );
  interp.registerFunction(
    "store/set!",
    (key, value) => guarded(`store/set!:${key}`, null, () => {
      localStorage.setItem(key, JSON.stringify(value));
      return null;
    })
  );
  interp.registerFunction(
    "store/remove!",
    (key) => guarded(`store/remove!:${key}`, null, () => {
      localStorage.removeItem(key);
      return null;
    })
  );
  interp.registerFunction(
    "store/clear!",
    () => guarded("store/clear!", null, () => {
      localStorage.clear();
      return null;
    })
  );
  interp.registerFunction(
    "store/keys",
    () => guarded("store/keys", [], () => {
      const keys = [];
      for (let i = 0; i < localStorage.length; i++) {
        const key = localStorage.key(i);
        if (key !== null) keys.push(key);
      }
      return keys;
    })
  );
  interp.registerFunction(
    "store/has?",
    (key) => guarded(`store/has?:${key}`, false, () => localStorage.getItem(key) !== null)
  );
  interp.registerFunction(
    "store/session-get",
    (key) => guarded(`store/session-get:${key}`, null, () => {
      const val = sessionStorage.getItem(key);
      if (val === null) return null;
      return JSON.parse(val);
    })
  );
  interp.registerFunction(
    "store/session-set!",
    (key, value) => guarded(`store/session-set!:${key}`, null, () => {
      sessionStorage.setItem(key, JSON.stringify(value));
      return null;
    })
  );
  interp.registerFunction(
    "store/session-remove!",
    (key) => guarded(`store/session-remove!:${key}`, null, () => {
      sessionStorage.removeItem(key);
      return null;
    })
  );
  interp.registerFunction(
    "store/session-clear!",
    () => guarded("store/session-clear!", null, () => {
      sessionStorage.clear();
      return null;
    })
  );
}

// src/reactive.ts
import { signal, computed, effect, batch, untracked } from "@preact/signals-core";
function registerReactiveBindings(interp, ctx) {
  const getActiveComponent3 = () => {
    const componentId = getCurrentOwnerId(ctx);
    return componentId != null ? ctx.mountedComponentsById.get(componentId) ?? null : null;
  };
  const inRenderBodyOf = (component) => ctx.renderContextStack[ctx.renderContextStack.length - 1] === component.instanceId;
  const ownedByRender = (component, dispose) => {
    component.renderReactive.computeds.push(dispose);
  };
  const reportRenderAllocation = (component) => {
    if (component.renderReactive.reported.has("watch")) return;
    component.renderReactive.reported.add("watch");
    ctx.onerror(
      new Error(
        "(watch \u2026) called from a render body is memoized per render \u2014 one live registration is kept and each render swaps its callback, rather than adding another subscription. Move it into an (on-mount \u2026) or an (effect \u2026) if it should be created once."
      ),
      `watch:${component.componentFn}`
    );
  };
  const disposeWatch = (watchId) => {
    const registration = ctx.watchDisposers.get(watchId);
    if (!registration) return;
    registration.dispose();
    ctx.watchDisposers.delete(watchId);
    releaseCallback(registration.callback);
    for (const component of ctx.mountedComponents.values()) {
      component.ownedWatchIds.delete(watchId);
    }
  };
  const createWatch = (signalId, callbackValue, owner) => {
    const s = ctx.signals.get(signalId);
    if (!s) throw new Error(`Unknown state: ${signalId}`);
    let invoke = toInvokableCallback(callbackValue, interp, "watch callback");
    let prev = untracked(() => s.value);
    const watchId = ctx.nextWatchId++;
    const dispose = effect(() => {
      const current = s.value;
      if (prev !== current) {
        const oldVal = prev;
        prev = current;
        try {
          withOwnerContext(ctx, owner?.instanceId ?? null, () => invoke(oldVal, current));
        } catch (e) {
          ctx.onerror(e instanceof Error ? e : new Error(String(e)), "watch");
        }
      }
    });
    ctx.watchDisposers.set(watchId, { dispose, callback: invoke });
    if (owner) owner.ownedWatchIds.add(watchId);
    const adopt = (nextValue) => {
      const next = toInvokableCallback(nextValue, interp, "watch callback");
      const registration = ctx.watchDisposers.get(watchId);
      if (!registration) {
        releaseCallback(next);
        return;
      }
      releaseCallback(registration.callback);
      registration.callback = next;
      invoke = next;
    };
    return { watchId, adopt };
  };
  const claimRenderScopedWatch = (component, signalId, callbackValue) => {
    const owned = component.renderReactive;
    const base = `${currentScopePath(ctx)}\0${signalId}`;
    const occurrence = owned.occurrences.get(base) ?? 0;
    owned.occurrences.set(base, occurrence + 1);
    const key = `${base}\0${occurrence}`;
    owned.claimed.add(key);
    const existing = owned.watches.get(key);
    if (existing) {
      existing.adopt(callbackValue);
      return existing.watchId;
    }
    const { watchId, adopt } = createWatch(signalId, callbackValue, component);
    owned.watches.set(key, { watchId, adopt, dispose: () => disposeWatch(watchId) });
    reportRenderAllocation(component);
    return watchId;
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
    if (owner) {
      owner.ownedSignalIds.add(id);
      if (inRenderBodyOf(owner)) {
        ownedByRender(owner, () => {
          owner.ownedSignalIds.delete(id);
          disposeSignal(ctx, id);
        });
      }
    }
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
    const owner = getActiveComponent3();
    if (owner && inRenderBodyOf(owner)) {
      return claimRenderScopedWatch(owner, signalId, callbackValue);
    }
    return createWatch(signalId, callbackValue, owner).watchId;
  });
  interp.registerFunction("__state/unwatch", (watchId) => {
    disposeWatch(watchId);
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
import { signal as signal2, effect as effect2, untracked as untracked2 } from "@preact/signals-core";
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
function normalizeProps(value) {
  if (Array.isArray(value)) {
    return value.map((item) => normalizeProps(item));
  }
  if (value && typeof value === "object") {
    const out = {};
    for (const [key, nested] of Object.entries(value)) {
      out[key.startsWith(":") ? key.slice(1) : key] = normalizeProps(nested);
    }
    return out;
  }
  return value;
}
function componentErrorContext(component) {
  const base = `component:${component.componentFn}`;
  const keys = component.props ? Object.keys(component.props) : [];
  return keys.length > 0 ? `${base}{${keys.join(",")}}` : base;
}
var RENDER_FAILED = /* @__PURE__ */ Symbol("sema-web:render-failed");
function withRenderCycleHint(error) {
  if (!/Cycle detected/i.test(error.message)) return error;
  error.message = `${error.message} \u2014 this render wrote reactive state that it also reads. Move the (put! \u2026) / (update! \u2026) into an (effect \u2026), an (on-mount \u2026), or an event handler.`;
  return error;
}
function callComponent(interp, component, ctx) {
  beginRenderScopedReactive(component, ctx);
  component.pendingLifecycle.clear();
  component.visitedScopes.clear();
  component.visitedScopes.add("");
  ctx.childScopeStack.length = 0;
  ctx.childScopeCounters.clear();
  try {
    return withComponentContext(
      ctx,
      component.instanceId,
      () => (
        // Called with no arguments unless props were supplied. Passing an
        // argument to a zero-parameter Sema function is an arity error, and
        // every component written before props existed takes no parameters.
        component.props === null ? interp.invokeGlobal(component.componentFn) : interp.invokeGlobal(component.componentFn, component.props)
      )
    );
  } catch (e) {
    releasePendingLifecycle(component);
    component.visitedScopes.clear();
    ctx.childScopeStack.length = 0;
    ctx.onerror(
      withRenderCycleHint(e instanceof Error ? e : new Error(String(e))),
      componentErrorContext(component)
    );
    return RENDER_FAILED;
  } finally {
    endRenderScopedReactive(component, ctx);
  }
}
function releasePendingLifecycle(component) {
  for (const bucket of component.pendingLifecycle.values()) {
    for (const pending of bucket) releaseCallback(pending.fn);
  }
  component.pendingLifecycle.clear();
}
function pendingFor(component, scope) {
  let bucket = component.pendingLifecycle.get(scope);
  if (!bucket) {
    bucket = [];
    component.pendingLifecycle.set(scope, bucket);
  }
  return bucket;
}
function depsEqual(a, b) {
  if (a == null || b == null) return false;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (!depValueEqual(a[i], b[i])) return false;
  }
  return true;
}
function depValueEqual(a, b) {
  if (Object.is(a, b)) return true;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return a.every((item, i) => depValueEqual(item, b[i]));
  }
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") {
    return false;
  }
  const aKeys = Object.keys(a);
  const bKeys = Object.keys(b);
  if (aKeys.length !== bKeys.length) return false;
  for (const key of aKeys) {
    if (!Object.prototype.hasOwnProperty.call(b, key)) return false;
    if (!depValueEqual(a[key], b[key])) {
      return false;
    }
  }
  return true;
}
function coerceCleanup(value, interp) {
  if (typeof value === "function") {
    retainCallback(value);
    return value;
  }
  if (typeof value === "string" && SEMA_IDENT_RE.test(value)) {
    return toInvokableCallback(value, interp, "effect cleanup");
  }
  return null;
}
function runLifecycleSlot(component, interp, ctx, index, pending) {
  if (pending.kind === "unmount") {
    return { ...pending, cleanup: pending.fn };
  }
  let cleanup = null;
  try {
    const result = withOwnerContext(ctx, component.instanceId, () => pending.fn());
    cleanup = coerceCleanup(result, interp);
  } catch (e) {
    ctx.onerror(
      e instanceof Error ? e : new Error(String(e)),
      `effect:${component.componentFn}#${index}`
    );
  }
  return { ...pending, cleanup };
}
function retireLifecycleSlot(component, ctx, index, slot, reason) {
  if (slot.retired) return;
  slot.retired = true;
  const cleanup = slot.cleanup;
  slot.cleanup = null;
  if (cleanup && (reason === "dispose" || slot.kind !== "unmount")) {
    try {
      withOwnerContext(ctx, component.instanceId, () => cleanup());
    } catch (e) {
      const kind = slot.kind === "unmount" ? "on-unmount" : "effect-cleanup";
      ctx.onerror(
        e instanceof Error ? e : new Error(String(e)),
        `${kind}:${component.componentFn}#${index}`
      );
    }
  }
  if (cleanup) releaseCallback(cleanup);
  if (slot.fn !== cleanup) releaseCallback(slot.fn);
}
function flushLifecycle(component, interp, ctx) {
  const owning = /* @__PURE__ */ new Set([
    ...component.ownedLifecycleSlots.keys(),
    ...component.ownedScopeResources.keys(),
    ...localScopes(component)
  ]);
  for (const scope of owning) {
    if (component.visitedScopes.has(scope)) continue;
    disposeScope(component, ctx, scope);
  }
  for (const scope of component.visitedScopes) {
    if (component.destroyed) break;
    flushScope(component, interp, ctx, scope);
  }
  component.visitedScopes.clear();
  releasePendingLifecycle(component);
}
function reportSlotShapeChange(component, ctx, index, detail) {
  ctx.onerror(
    new Error(
      `lifecycle slot ${index} ${detail}. Register (effect \u2026) and (on-unmount \u2026) unconditionally at the top level of the component body \u2014 slots are matched by call order, so a render that registers a different sequence re-keys the rest.`
    ),
    `lifecycle:${component.componentFn}#${index}`
  );
}
function flushScope(component, interp, ctx, scope) {
  const live = component.ownedLifecycleSlots.get(scope) ?? [];
  const pending = component.pendingLifecycle.get(scope) ?? [];
  if (live.length === 0 && pending.length === 0) return;
  component.pendingLifecycle.delete(scope);
  component.ownedLifecycleSlots.set(scope, live);
  const abandon = (fromPending) => {
    while (live.length > 0) {
      const index = live.length - 1;
      retireLifecycleSlot(component, ctx, index, live.pop(), "dispose");
    }
    component.ownedLifecycleSlots.delete(scope);
    for (let rest = fromPending; rest < pending.length; rest++) {
      releaseCallback(pending[rest].fn);
    }
  };
  for (let index = live.length - 1; index >= pending.length; index--) {
    const slot = live[index];
    if (slot.kind === "unmount") {
      reportSlotShapeChange(
        component,
        ctx,
        index,
        "held an (on-unmount \u2026) hook that this render no longer registers, so the hook will never run"
      );
    }
    retireLifecycleSlot(component, ctx, index, slot, "rekey");
    if (component.destroyed) {
      abandon(0);
      return;
    }
  }
  if (live.length > pending.length) live.length = pending.length;
  for (let index = 0; index < pending.length; index++) {
    const next = pending[index];
    const current = live[index];
    if (current && current.kind === next.kind && depsEqual(current.deps, next.deps)) {
      releaseCallback(next.fn);
      continue;
    }
    if (current) {
      if (current.kind !== next.kind) {
        reportSlotShapeChange(
          component,
          ctx,
          index,
          `changed from ${current.kind === "unmount" ? "(on-unmount \u2026)" : "(effect \u2026)"} to ${next.kind === "unmount" ? "(on-unmount \u2026)" : "(effect \u2026)"} between renders`
        );
      }
      retireLifecycleSlot(component, ctx, index, current, "rekey");
      if (component.destroyed) {
        abandon(index);
        return;
      }
    }
    const slot = runLifecycleSlot(component, interp, ctx, index, next);
    if (component.destroyed) {
      retireLifecycleSlot(component, ctx, index, slot, "dispose");
      abandon(index + 1);
      return;
    }
    live[index] = slot;
  }
  if (live.length === 0) component.ownedLifecycleSlots.delete(scope);
}
function disposeScope(component, ctx, scope) {
  const resources = component.ownedScopeResources.get(scope);
  if (resources) {
    for (const id of resources) {
      try {
        disposeSignal(ctx, id);
      } catch (e) {
        ctx.onerror(
          e instanceof Error ? e : new Error(String(e)),
          `component-resource-cleanup:${component.componentFn}`
        );
      }
      component.ownedStreamIds.delete(id);
    }
    component.ownedScopeResources.delete(scope);
  }
  const slots = component.ownedLifecycleSlots.get(scope);
  if (slots) {
    while (slots.length > 0) {
      const index = slots.length - 1;
      const slot = slots.pop();
      try {
        retireLifecycleSlot(component, ctx, index, slot, "dispose");
      } catch (e) {
        ctx.onerror(
          e instanceof Error ? e : new Error(String(e)),
          `component-effect-cleanup:${component.componentFn}`
        );
      }
    }
    component.ownedLifecycleSlots.delete(scope);
  }
  disposeLocalScope(component, ctx, scope);
}
function disposeLifecycleSlots(component, ctx) {
  while (component.ownedLifecycleSlots.size > 0) {
    const scope = Array.from(component.ownedLifecycleSlots.keys()).pop();
    const slots = component.ownedLifecycleSlots.get(scope);
    if (slots.length === 0) {
      component.ownedLifecycleSlots.delete(scope);
      continue;
    }
    const index = slots.length - 1;
    const slot = slots.pop();
    try {
      retireLifecycleSlot(component, ctx, index, slot, "dispose");
    } catch (e) {
      ctx.onerror(
        e instanceof Error ? e : new Error(String(e)),
        `component-effect-cleanup:${component.componentFn}`
      );
    }
  }
  releasePendingLifecycle(component);
}
function sipMorphOptions(ctx, activeElement) {
  return {
    childrenOnly: true,
    // Honour SIP `:key`. Without this morphdom matches children by position,
    // so reordering a list rewrites every row in place: focus jumps, a
    // half-typed input lands on the wrong row, and any DOM state the app did
    // not put there (scroll, <details> open, a CSS animation mid-flight) goes
    // with it. Falls back to `id`, which is morphdom's own default.
    getNodeKey: sipNodeKey,
    onBeforeElUpdated(fromEl, toEl) {
      if (fromEl === activeElement && (fromEl.tagName === "INPUT" || fromEl.tagName === "TEXTAREA" || fromEl.tagName === "SELECT")) {
        syncAttributesPreservingValue(fromEl, toEl);
        if (fromEl.tagName === "SELECT") {
          morphFocusedSelectOptions(fromEl, toEl, ctx);
        }
        return false;
      }
      return true;
    },
    onNodeDiscarded(node) {
      releaseHandlesForSubtree(node, ctx);
    }
  };
}
function syncAttributesPreservingValue(fromEl, toEl) {
  for (const attr of Array.from(toEl.attributes)) {
    if (attr.name === "value") continue;
    if (attr.namespaceURI) {
      const local = attr.localName || attr.name;
      if (fromEl.getAttributeNS(attr.namespaceURI, local) !== attr.value) {
        fromEl.setAttributeNS(attr.namespaceURI, attr.name, attr.value);
      }
    } else if (fromEl.getAttribute(attr.name) !== attr.value) {
      fromEl.setAttribute(attr.name, attr.value);
    }
  }
  for (const attr of Array.from(fromEl.attributes)) {
    if (attr.name === "value") continue;
    if (attr.namespaceURI) {
      const local = attr.localName || attr.name;
      if (!toEl.hasAttributeNS(attr.namespaceURI, local)) {
        fromEl.removeAttributeNS(attr.namespaceURI, local);
      }
    } else if (!toEl.hasAttribute(attr.name)) {
      fromEl.removeAttribute(attr.name);
    }
  }
}
function morphFocusedSelectOptions(fromEl, toEl, ctx) {
  const selected = /* @__PURE__ */ new Set();
  for (const option of Array.from(fromEl.options)) {
    if (option.selected) selected.add(option.value);
  }
  morphdom(fromEl, toEl, {
    childrenOnly: true,
    getNodeKey: sipNodeKey,
    onNodeDiscarded(node) {
      releaseHandlesForSubtree(node, ctx);
    }
  });
  const options = Array.from(fromEl.options);
  if (!options.some((option) => selected.has(option.value))) return;
  for (const option of options) {
    option.selected = selected.has(option.value);
  }
}
function renderComponent(component, interp, ctx) {
  if (component.rendering) {
    ctx.onerror(
      new Error(
        `component/force-render!: ${component.componentFn} is already rendering, so the request was ignored. A render or effect body does not need to force a render \u2014 the component re-renders when the state it reads changes.`
      ),
      `force-render:${component.componentFn}`
    );
    return;
  }
  if (component.dispose) component.dispose();
  component.dispose = effect2(() => {
    const startedAt = ctx.diagnostics.enabled ? now() : 0;
    component.rendering = true;
    try {
      const sipData = callComponent(interp, component, ctx);
      if (sipData === RENDER_FAILED) return;
      if (component.destroyed) {
        releasePendingLifecycle(component);
        return;
      }
      if (sipData != null) {
        const clone = component.target.cloneNode(false);
        const sipNode = renderSip(sipData, interp, ctx);
        clone.appendChild(sipNode);
        morphdom(component.target, clone, sipMorphOptions(ctx, document.activeElement));
        ctx.diagnostics.record(() => ({
          kind: "render",
          at: Date.now(),
          context: `component:${component.componentFn}`,
          durationMs: now() - startedAt
        }));
      }
      untracked2(() => flushLifecycle(component, interp, ctx));
    } finally {
      component.rendering = false;
    }
  });
}
function getActiveComponent(ctx) {
  const componentId = ctx.renderContextStack[ctx.renderContextStack.length - 1];
  return componentId != null ? ctx.mountedComponentsById.get(componentId) ?? null : null;
}
function requireRenderingComponent(ctx, form) {
  if (ctx.renderContextStack.length === 0) {
    throw new Error(`(${form}) called outside of a component render context`);
  }
  const component = getActiveComponent(ctx);
  if (!component) {
    throw new Error(`(${form}) no active mounted component`);
  }
  return component;
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
    unregisterStream(ctx, signalId);
  }
}
function cleanupListener(ctx, key) {
  const registration = ctx.listeners.get(key);
  if (!registration) return;
  registration.target.removeEventListener(registration.event, registration.listener);
  releaseCallback(registration.callback);
  ctx.listeners.delete(key);
}
function drainOwned(component, ctx, ids, cleanup, label) {
  for (const id of ids) {
    try {
      cleanup(id);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `${label}:${component.componentFn}`);
    }
  }
}
function destroyMountedComponent(selector, component, ctx) {
  component.destroyed = true;
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
  disposeLifecycleSlots(component, ctx);
  if (component.eventCleanup) {
    try {
      component.eventCleanup();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-events:${component.componentFn}`);
    }
    component.eventCleanup = null;
  }
  drainOwned(component, ctx, component.ownedListenerKeys, (k) => cleanupListener(ctx, k), "component-listener-cleanup");
  component.ownedListenerKeys.clear();
  drainOwned(component, ctx, component.ownedSignalIds, (id) => disposeSignal(ctx, id), "component-signal-cleanup");
  component.ownedSignalIds.clear();
  drainOwned(component, ctx, component.ownedWatchIds, (id) => cleanupWatch(ctx, id), "component-watch-cleanup");
  component.ownedWatchIds.clear();
  drainOwned(component, ctx, component.ownedIntervalIds, (id) => cleanupInterval(ctx, id), "component-interval-cleanup");
  component.ownedIntervalIds.clear();
  drainOwned(component, ctx, component.ownedStreamIds, (id) => {
    cleanupStream(ctx, id);
    disposeSignal(ctx, id);
  }, "component-stream-cleanup");
  component.ownedStreamIds.clear();
  drainOwned(component, ctx, component.localState.values(), (id) => disposeSignal(ctx, id), "component-local-state-cleanup");
  component.localState.clear();
  for (const child of Array.from(component.target.childNodes)) {
    releaseHandlesForSubtree(child, ctx);
  }
  component.target.innerHTML = "";
  ctx.mountedComponents.delete(selector);
  ctx.mountedComponentsById.delete(component.instanceId);
}
var PASSIVE_BY_DEFAULT = /* @__PURE__ */ new Set(["wheel", "touchstart", "touchmove"]);
var EventDelegator = class {
  constructor() {
    /**
     * Elements whose `.once` handler for an event has already fired.
     *
     * Not a DOM attribute: morphdom re-syncs every attribute from the freshly
     * rendered clone on each patch, so an attribute mark would be restored on the
     * next render and the handler would fire again. The semantics are therefore
     * "once per element instance" — a node morphdom replaces re-arms — which is
     * also why the tests for it must use the real morphdom.
     */
    this.firedOnce = /* @__PURE__ */ new WeakMap();
  }
  setup(component, target, interp, ctx) {
    const listeners = [];
    for (const event of DELEGATED_EVENTS) {
      const passive = PASSIVE_BY_DEFAULT.has(event) ? { passive: false } : void 0;
      const bubbleListener = (ev) => {
        this.walk("bubble", event, component, target, interp, ctx, ev);
      };
      target.addEventListener(event, bubbleListener, passive);
      listeners.push({ event, listener: bubbleListener, capture: false });
      const captureListener = (ev) => {
        if (!ctx.captureEvents.has(event)) return;
        this.walk("capture", event, component, target, interp, ctx, ev);
      };
      target.addEventListener(event, captureListener, passive ? { ...passive, capture: true } : true);
      listeners.push({ event, listener: captureListener, capture: true });
    }
    const mouseOverListener = (ev) => {
      this.walkSynthetic("mouseenter", component, target, interp, ctx, ev);
    };
    target.addEventListener("mouseover", mouseOverListener);
    listeners.push({ event: "mouseover", listener: mouseOverListener, capture: false });
    const mouseOutListener = (ev) => {
      this.walkSynthetic("mouseleave", component, target, interp, ctx, ev);
    };
    target.addEventListener("mouseout", mouseOutListener);
    listeners.push({ event: "mouseout", listener: mouseOutListener, capture: false });
    return () => {
      for (const { event, listener, capture } of listeners) {
        target.removeEventListener(event, listener, capture);
      }
    };
  }
  /**
   * Walk from the event target to the mount root, running matching handlers.
   *
   * The capture pass walks the same chain outermost-first, which is what
   * "capture" means: an ancestor's `.capture` handler sees the event before a
   * descendant's, and an ancestor's `.capture.stop` keeps the descendant from
   * running at all.
   */
  walk(phase, event, component, target, interp, ctx, ev) {
    const attr = `data-sema-on-${event}`;
    const start = ev.target;
    if (!(start instanceof Element) || !target.contains(start)) {
      return;
    }
    const chain = [];
    for (let el = start; el; el = el.parentElement) {
      chain.push(el);
      if (el === target) break;
    }
    if (phase === "capture") chain.reverse();
    for (const el of chain) {
      if (!el.hasAttribute(attr)) continue;
      const fn = el.getAttribute(attr);
      if (!SEMA_IDENT_RE.test(fn)) continue;
      this.tryRun(interp, ctx, component, el, ev, event, fn, phase);
      if (ev.cancelBubble || ev.__sema_stop) break;
    }
  }
  /**
   * Run the `mouseenter`/`mouseleave` handlers of every element the pointer
   * actually entered or left.
   *
   * Real `mouseenter` is dispatched once per element being entered — the whole
   * ancestor chain, outermost first — and `mouseleave` innermost first. A
   * single `closest()` lookup finds only the *nearest* declaring ancestor, so a
   * hovered child ate its parent's `mouseenter` while the parent's
   * `mouseleave` still fired (its own `closest` lookup reached it on the way
   * out). An app tracking hover with a boolean or a counter then got a close it
   * never got an open for.
   *
   * `relatedTarget` is tested per element rather than once: an ancestor that
   * already contained the pointer was never entered, and is not left when the
   * pointer moves to another element inside it.
   */
  walkSynthetic(event, component, target, interp, ctx, ev) {
    const attr = `data-sema-on-${event}`;
    const start = ev.target;
    if (!(start instanceof Element) || !target.contains(start)) return;
    const chain = [];
    for (let el = start; el; el = el.parentElement) {
      if (el.hasAttribute(attr) && !el.contains(ev.relatedTarget)) chain.push(el);
      if (el === target) break;
    }
    if (event === "mouseenter") chain.reverse();
    for (const el of chain) {
      this.tryRun(interp, ctx, component, el, ev, event, el.getAttribute(attr), "synthetic");
      if (ev.cancelBubble || ev.__sema_stop) break;
    }
  }
  /**
   * Apply a handler's modifiers around its invocation.
   *
   * The order is fixed and load-bearing:
   *
   * 1. phase gate — `.capture` handlers belong to the capture pass only;
   * 2. `.self` — a handler filtered out here must behave as if it were absent,
   *    which means it must not `preventDefault` and must not burn its `.once`;
   * 3. `.prevent` — before the handler runs, so the handler observes
   *    `defaultPrevented`, and *before* the `.once` gate: `.prevent` is a
   *    property of the declaration, not of the invocation. A spent `.once` that
   *    stopped preventing let the second submit of a `.prevent.once` form
   *    navigate the page away — the exact catastrophe the modifier exists to
   *    prevent, and irreversible in a way "the handler didn't run" is not;
   * 4. `.once` — marked *before* the invoke, so a handler that re-dispatches
   *    the same event cannot fire itself a second time;
   * 5. the handler;
   * 6. `.stop` — after the handler, so it composes with whatever the handler
   *    did, and so a handler that throws still stops propagation. Unlike
   *    `.prevent` it stays tied to the handler running: it only decides which
   *    *other* handlers see the event, so a spent `.once` releasing it costs
   *    nothing irreversible.
   */
  tryRun(interp, ctx, component, el, ev, event, fn, phase) {
    const mods = readEventModifiers(el, event);
    if (phase === "capture" && !mods.capture) return;
    if (phase === "bubble" && mods.capture) return;
    if (mods.self && ev.target !== el) return;
    if (mods.prevent) ev.preventDefault();
    if (mods.once) {
      let fired = this.firedOnce.get(el);
      if (!fired) {
        fired = /* @__PURE__ */ new Set();
        this.firedOnce.set(el, fired);
      }
      if (fired.has(event)) return;
      fired.add(event);
    }
    withOwnerContext(ctx, component.instanceId, () => {
      this.dispatchEvent(interp, ctx, fn, ev, el);
    });
    if (mods.stop) {
      ev.stopPropagation();
      ev.__sema_stop = true;
    }
  }
  dispatchEvent(interp, ctx, callbackName, ev, declaringElement) {
    const evHandle = storeHandle(ev, ctx);
    const previousDeclaring = ev.__sema_current_target;
    ev.__sema_current_target = declaringElement;
    try {
      interp.invokeGlobal(callbackName, evHandle);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `event:${ev.type}:${callbackName}`);
    } finally {
      ev.__sema_current_target = previousDeclaring;
      if (evHandle != null) releaseHandle(evHandle, ctx);
    }
  }
};
var COMPONENT_SEMA_PRELUDE = `
    ;; (mount! "#app" my-view) or (mount! "#app" my-view {:items xs})
    (defmacro mount! (selector component-name . rest)
      \`(component/mount! ,selector
         ,(if (symbol? component-name)
            (symbol->string component-name)
            component-name)
         ,(if (null? rest) nil (car rest))))

    (define (local name initial) (__component/local name initial))

    (define (on-mount fn) (__component/on-mount fn))

    ;; Component-scoped lifecycle.
    ;;
    ;;   (effect deps fn)  deps is a list, or nil for "after every render".
    ;;                     fn runs after the DOM patch and may return a cleanup
    ;;                     (a function value or a global's name) that runs
    ;;                     before the next re-run and once at teardown.
    ;;   (on-unmount fn)   fn runs once when the component is destroyed, and
    ;;                     never merely because a later render re-keyed its
    ;;                     slot.
    ;;
    ;; Slots are matched by CALL ORDER across renders, so register these
    ;; unconditionally at the top level of a component body \u2014 a render that
    ;; registers a different number of slots re-keys every slot after it, which
    ;; is reported as lifecycle:<component>#<slot>. An on-unmount hook a render
    ;; stops registering is dropped without running.
    ;; Argument validation lives on the host side, where it surfaces as an
    ;; ordinary error rather than a Sema type check on every render.
    (define (effect deps fn) (__component/effect deps fn))

    (define (on-unmount fn) (__component/on-unmount fn))

    ;; A component declared WITH a parameter is compiled to a variadic function
    ;; that defaults its props to {}. Two reasons, both load-bearing:
    ;;
    ;;   1. Sema raises an arity error when a one-parameter function is called
    ;;      with no arguments, so \`(mount! "#app" view)\` on a \`(view props)\`
    ;;      component would fail. Defaulting keeps props optional at the call
    ;;      site, which is what makes them adoptable incrementally.
    ;;   2. A zero-parameter component stays a plain zero-arg function, so every
    ;;      component written before props existed is untouched.
    (defmacro defcomponent (name params . body)
      (if (null? params)
        \`(define ,name (fn () ,@body))
        \`(define ,name
           (fn (. __component-args)
             (let ((,(car params)
                     (if (null? __component-args) {} (car __component-args))))
               ,@body)))))

    ;; Invoke a child component, isolating its failures.
    ;;
    ;; A throwing child renders as nothing and is reported under its OWN name,
    ;; rather than taking the parent's whole subtree down and being blamed on
    ;; the mounted component.
    ;;
    ;; This guard was absent for one day (2026-07-27) because a Sema try/catch
    ;; inside a mounted render aborted the WASM instance. That was a VM defect,
    ;; not a language constraint - a Vec::pop() passed to debug_assert_eq!, so
    ;; release builds dropped it (docs/limitations.md section 38, now resolved).
    ;; Both the try and the map? guard below are safe again.
    (define (__component-render-guarded name f props)
      (__component/enter-child name (:key props))
      (let ((__component-rendered
              (try
                (f props)
                (catch e
                  (__component/report-error name (:message e))
                  nil))))
        (__component/exit-child)
        __component-rendered))

    ;; (component/render child-view {:prop 1} [:p "child content"])
    ;;
    ;; A macro, not a function, so the child's symbol can be captured as a
    ;; string for error attribution while still being evaluated to the function
    ;; itself, and so an omitted props argument can be filled in as a literal {}
    ;; at expansion time. Children are collected into :children on the props map.
    ;;
    ;; The child renders inside its OWN scope (see __component/enter-child), so
    ;; its effect / on-unmount / local / resource registrations belong to the
    ;; child instance rather than to the component that happens to be mounting
    ;; it. Give repeated children a :key - the same key that governs DOM
    ;; identity - and their state follows them through reorders and removals.
    (defmacro component/render (component . rest)
      (let ((props (if (null? rest) {} (car rest)))
            (children (if (null? rest) (list) (cdr rest))))
        \`(__component-render-guarded
           ,(if (symbol? component) (symbol->string component) "component")
           ,component
           (merge (if (map? ,props) ,props {})
                  {:children (list ,@children)}))))
  `;
function registerComponentBindings(interp, ctx) {
  interp.registerFunction(
    "component/mount!",
    (selector, componentFn, props) => {
      if (!SEMA_IDENT_RE.test(componentFn)) {
        throw new Error(`Invalid component function name: ${componentFn}`);
      }
      if (props != null && (typeof props !== "object" || Array.isArray(props))) {
        throw new Error(
          `mount! props must be a map, got ${Array.isArray(props) ? "list" : typeof props}`
        );
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
        props: props == null ? null : normalizeProps(props),
        dispose: null,
        rendering: false,
        destroyed: false,
        eventCleanup: null,
        localState: /* @__PURE__ */ new Map(),
        mountCleanup: null,
        pendingMount: null,
        ownedSignalIds: /* @__PURE__ */ new Set(),
        ownedWatchIds: /* @__PURE__ */ new Set(),
        ownedIntervalIds: /* @__PURE__ */ new Set(),
        ownedStreamIds: /* @__PURE__ */ new Set(),
        ownedListenerKeys: /* @__PURE__ */ new Set(),
        pendingLifecycle: /* @__PURE__ */ new Map(),
        ownedLifecycleSlots: /* @__PURE__ */ new Map(),
        visitedScopes: /* @__PURE__ */ new Set(),
        ownedScopeResources: /* @__PURE__ */ new Map(),
        renderReactive: createRenderScopedReactive()
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
            retainCallback(finalCleanupFn);
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
  interp.registerFunction("__component/enter-child", (name, key) => {
    const parent = currentScopePath(ctx);
    let segment;
    if (key != null && key !== "") {
      segment = `${name}#${String(key)}`;
    } else {
      const counterKey = `${parent}\0${name}`;
      const next = (ctx.childScopeCounters.get(counterKey) ?? 0) + 1;
      ctx.childScopeCounters.set(counterKey, next);
      segment = `${name}#${next}`;
    }
    ctx.childScopeStack.push(segment);
    const component = getActiveComponent(ctx);
    if (component) component.visitedScopes.add(currentScopePath(ctx));
    return null;
  });
  interp.registerFunction("__component/exit-child", () => {
    ctx.childScopeStack.pop();
    return null;
  });
  interp.registerFunction("__component/local", (name, initialValue) => {
    const component = requireRenderingComponent(ctx, "local");
    const scopedName = `${currentScopePath(ctx)}\0${name}`;
    const existingId = component.localState.get(scopedName);
    if (existingId != null) {
      return existingId;
    }
    const id = ctx.nextSignalId++;
    const s = signal2(initialValue);
    ctx.signals.set(id, s);
    component.localState.set(scopedName, id);
    return id;
  });
  interp.registerFunction("__component/on-mount", (callbackValue) => {
    const component = requireRenderingComponent(ctx, "on-mount");
    releaseCallback(component.pendingMount);
    component.pendingMount = callbackValue;
    return null;
  });
  interp.registerFunction("__component/effect", (deps, callbackValue) => {
    const component = requireRenderingComponent(ctx, "effect");
    if (deps != null && !Array.isArray(deps)) {
      throw new Error(`(effect) deps must be a list or nil, got ${typeof deps}`);
    }
    const fn = toInvokableCallback(callbackValue, interp, "effect callback");
    pendingFor(component, currentScopePath(ctx)).push({
      kind: "effect",
      deps: deps == null ? null : deps,
      fn
    });
    return null;
  });
  interp.registerFunction("__component/on-unmount", (callbackValue) => {
    const component = requireRenderingComponent(ctx, "on-unmount");
    const fn = toInvokableCallback(callbackValue, interp, "on-unmount callback");
    pendingFor(component, currentScopePath(ctx)).push({ kind: "unmount", deps: [], fn });
    return null;
  });
  interp.registerFunction("js/set-interval", (callbackValue, ms) => {
    const callback = toInvokableCallback(callbackValue, interp, "interval callback");
    const ownerId = getCurrentOwnerId(ctx);
    const id = window.setInterval(() => {
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
  interp.registerFunction("__component/report-error", (name, message) => {
    ctx.onerror(new Error(String(message)), `component:${String(name)}`);
    return null;
  });
  const semaResult = interp.evalStr(COMPONENT_SEMA_PRELUDE);
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
function buildProxyHeaders(opts) {
  const headers = { "Content-Type": "application/json" };
  for (const [key, value] of Object.entries(opts.headers ?? {})) {
    headers[key] = value;
  }
  if (opts.token) {
    const overridden = Object.keys(opts.headers ?? {}).find(
      (k) => k.toLowerCase() === "authorization"
    );
    if (overridden) {
      console.warn(
        `[sema-web] llmProxy: both \`token\` and a custom \`${overridden}\` header were provided. The token wins. Drop one to silence this warning.`
      );
      delete headers[overridden];
    }
    headers["Authorization"] = `Bearer ${opts.token}`;
  }
  return headers;
}
function buildProxyHeadersMap(opts) {
  const headers = buildProxyHeaders(opts);
  return `{${Object.entries(headers).map(([k, v]) => `"${escapeSemaString(k)}" "${escapeSemaString(v)}"`).join(" ")}}`;
}
function createStreamAccumulator(emit) {
  let text = "";
  let error = null;
  let terminal = false;
  const state = () => ({ text, done: terminal, error });
  return {
    get state() {
      return state();
    },
    /** Handle one SSE `data:` payload. */
    event(data) {
      if (terminal || !data) return;
      if (data.trim() === "[DONE]") {
        terminal = true;
        emit(state());
        return;
      }
      let parsed;
      try {
        parsed = JSON.parse(data);
      } catch {
        terminal = true;
        error = "Invalid stream payload";
        emit(state());
        return;
      }
      if (parsed?.type === "token" && typeof parsed.text === "string") {
        text += parsed.text;
        emit(state());
        return;
      }
      if (parsed?.type === "done") {
        terminal = true;
        emit(state());
        return;
      }
      if (parsed?.type === "error") {
        terminal = true;
        error = typeof parsed.error === "string" ? parsed.error : "Stream error";
        emit(state());
        return;
      }
    },
    /** Handle a transport-level failure. */
    fail(message) {
      if (terminal) return;
      terminal = true;
      error = message;
      emit(state());
    },
    /**
     * Handle the stream closing.
     *
     * A close with no prior terminal frame means the connection ended without
     * a `done` — mark it finished so consumers stop waiting, but do not invent
     * an error: a proxy that closes cleanly after its last token is behaving
     * acceptably, just tersely.
     */
    close() {
      if (terminal) return;
      terminal = true;
      emit(state());
    }
  };
}
function registerLlmBindings(interp, opts, ctx) {
  const proxyUrl = opts.url.replace(/\/+$/, "");
  const proxyHeaders = buildProxyHeaders(opts);
  const headersMap = buildProxyHeadersMap(opts);
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
    const accumulator = createStreamAccumulator((state) => {
      s.value = state;
    });
    const managedStream = openSseStream({
      url: `${proxyUrl}/stream`,
      method: "POST",
      headers: proxyHeaders,
      body,
      onEvent: (event) => accumulator.event(event.data),
      onError: (error) => accumulator.fail(error.message),
      onClose: () => {
        unregisterStream(ctx, id);
        accumulator.close();
      }
    });
    registerStream(ctx, id, {
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
      unregisterStream(ctx, signalId);
      for (const component of ctx.mountedComponents.values()) {
        component.ownedStreamIds.delete(signalId);
      }
    }
    const current = ctx.signals.get(signalId);
    if (current) {
      const previous = current.value ?? {};
      current.value = {
        text: typeof previous.text === "string" ? previous.text : "",
        done: true,
        error: previous.error ?? null
      };
    }
    return null;
  });
  const semaCode = buildLlmProxySema(proxyUrl, headersMap);
  const result = interp.evalStr(semaCode);
  if (result.error) {
    throw new Error(`[sema-web] Failed to register LLM bindings: ${result.error}`);
  }
}
function escapeSemaString(s) {
  return s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n").replace(/\r/g, "\\r").replace(/\t/g, "\\t");
}
function buildLlmProxySema(proxyUrl, headersMap) {
  return `
;; --- LLM proxy internals ---

(define __llm-proxy-url "${escapeSemaString(proxyUrl)}")
(define __llm-proxy-headers ${headersMap})

;; Normalize a raw http/* response into decoded JSON, or raise.
;;
;; Both transports must fail the same way. Previously this returned the decoded
;; body regardless of status, so a proxy 500 with a JSON error body came back
;; as an ordinary value and (llm/chat ...) handed the caller an error map
;; where it expected a string \u2014 the failure surfaced later, somewhere else, as
;; a type confusion. Streaming already reported errors properly through the
;; signal's :error field; this brings the request path in line.
(define (__llm-proxy-normalize endpoint resp)
  (if (not (map? resp))
    resp
    (let ((status (or (:status resp) 200))
          (body (:body resp)))
      (cond
        ((>= status 400)
          (error (format "llm proxy ~a failed: HTTP ~a~a" endpoint status
                   (if (and (string? body) (> (string-length body) 0))
                     (format " - ~a" body)
                     ""))))
        ((not (string? body)) resp)
        ((= (string-length body) 0) {})
        (else
          (try
            (json/decode body)
            (catch e
              (error (format "llm proxy ~a returned a body that is not valid JSON"
                       endpoint)))))))))

;; Helper: POST to the proxy and decode the JSON response body.
(define (__llm-proxy-post endpoint body-map)
  (let ((url (string-append __llm-proxy-url "/" endpoint)))
    (__llm-proxy-normalize endpoint
      (http/post url
        {:headers __llm-proxy-headers
         :body (json/encode body-map)}))))

;; Helper: GET from the proxy.
(define (__llm-proxy-get endpoint)
  (let ((url (string-append __llm-proxy-url "/" endpoint)))
    (__llm-proxy-normalize endpoint
      (http/get url {:headers __llm-proxy-headers}))))

;; --- Message normalization ---
;;
;; (message :user "hi") is a SPECIAL FORM in the Sema core, and a special form
;; always wins in operator position - see limitations.md #36. A
;; (define (message ...)) here therefore never takes effect; this module used to
;; ship one anyway, so every caller silently got the core form instead. That
;; form produces a :message value, and json/encode refuses to encode it
;; ("cannot encode message as JSON"), which broke the documented usage
;; (llm/chat (list (message :user "Hi"))) outright.
;;
;; So: don't fight the special form, convert its output. Every request path
;; funnels messages through here before encoding.
(define (__llm-message->map m)
  (if (= (type-of m) :message)
    {:role (message/role m) :content (message/content m)}
    m))

(define (__llm-normalize-messages messages)
  (if (list? messages)
    (map __llm-message->map messages)
    (__llm-message->map messages)))

;; --- Public API ---

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
    (let ((body (merge {:messages (__llm-normalize-messages messages)}
                  (if (map? opts) opts {}))))
      (let ((result (__llm-proxy-post "chat" body)))
        (if (map? result)
          (or (:content result) (:text result) result)
          result)))))

;; (llm/send prompt) or (llm/send prompt opts)
;; Send a prompt (list of messages or prompt object).
(define (llm/send prompt . rest)
  (let ((opts (if (null? rest) {} (car rest))))
    (let ((messages (if (list? prompt) prompt (list prompt))))
      (let ((body (merge {:messages (__llm-normalize-messages messages)}
                    (if (map? opts) opts {}))))
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
      (json/encode (__llm-normalize-messages messages))
      (json/encode (if (map? opts) opts {})))))

(define (llm/close-stream signal-id)
  (__llm/close-stream signal-id))
`;
}

// src/router.ts
import { signal as signal4 } from "@preact/signals-core";
var ROUTER_LINK_ATTR = "data-sema-router-link";
var EXTERNAL_PATH_RE = /^(?:[a-zA-Z][a-zA-Z0-9+.-]*:|[/\\]{2}|\\)/;
function escapeRegexLiteral(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
function literalRoutePattern(literal) {
  let out = "";
  for (const ch of literal) {
    const raw = escapeRegexLiteral(ch);
    if (ch === "/") {
      out += raw;
      continue;
    }
    let encoded;
    try {
      encoded = encodeURIComponent(ch);
    } catch {
      out += raw;
      continue;
    }
    if (encoded === ch) {
      out += raw;
      continue;
    }
    const anyCase = encoded.replace(/[A-F]/g, (hex) => `[${hex}${hex.toLowerCase()}]`);
    out += `(?:${raw}|${anyCase})`;
  }
  return out;
}
function samePath(a, b) {
  if (a === void 0) return false;
  if (a === b) return true;
  return decodeUriComponentSafe(a) === decodeUriComponentSafe(b);
}
function stripKeywordColon(value) {
  return value.startsWith(":") ? value.slice(1) : value;
}
function decodeUriComponentSafe(value) {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}
function decodeQueryPart(part) {
  return decodeUriComponentSafe(part.replace(/\+/g, " "));
}
function normalizeRoutePath(path) {
  if (!path) return "/";
  return path.startsWith("/") ? path : `/${path}`;
}
function splitPathQuery(raw) {
  const mark = raw.indexOf("?");
  if (mark === -1) return { path: normalizeRoutePath(raw), query: "" };
  return { path: normalizeRoutePath(raw.slice(0, mark)), query: raw.slice(mark + 1) };
}
function parseQuery(queryString) {
  const out = /* @__PURE__ */ Object.create(null);
  if (!queryString) return out;
  for (const pair of queryString.split("&")) {
    if (!pair) continue;
    const mark = pair.indexOf("=");
    const key = decodeQueryPart(mark === -1 ? pair : pair.slice(0, mark));
    if (!key) continue;
    const value = decodeQueryPart(mark === -1 ? "" : pair.slice(mark + 1));
    const existing = out[key];
    if (existing === void 0) {
      out[key] = value;
    } else if (Array.isArray(existing)) {
      existing.push(value);
    } else {
      out[key] = [existing, value];
    }
  }
  return out;
}
function routeHref(mode, path) {
  const normalized = normalizeRoutePath(path);
  return mode === "history" ? normalized : `#${normalized}`;
}
function compileRoute(pattern) {
  const paramNames = [];
  let regexStr = "^";
  let lastIndex = 0;
  pattern.replace(/:([a-zA-Z_][a-zA-Z0-9_]*)/g, (match, name, offset) => {
    regexStr += literalRoutePattern(pattern.slice(lastIndex, offset));
    paramNames.push(name);
    regexStr += "([^/]+)";
    lastIndex = offset + match.length;
    return match;
  });
  regexStr += literalRoutePattern(pattern.slice(lastIndex));
  regexStr += "$";
  return { regex: new RegExp(regexStr), paramNames };
}
function readOption(map, name) {
  if (Object.prototype.hasOwnProperty.call(map, name)) return map[name];
  const keyword = `:${name}`;
  if (Object.prototype.hasOwnProperty.call(map, keyword)) return map[keyword];
  return void 0;
}
function isPlainObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function readHandlerName(value, what) {
  if (value === null || value === void 0) return null;
  if (typeof value !== "string") {
    throw new Error(`router/init!: ${what} must be a handler name string, got ${typeof value}`);
  }
  const name = stripKeywordColon(value);
  if (!name) throw new Error(`router/init!: ${what} must not be empty`);
  return name;
}
function parseInitArg(arg) {
  if (!isPlainObject(arg)) {
    throw new Error(
      "router/init!: expected a map of pattern -> handler, or an options map with :routes"
    );
  }
  const routes = readOption(arg, "routes");
  if (routes === void 0 || typeof routes === "string") {
    return { mode: "hash", routes: arg, notFound: null, scrollToTop: false, focus: null };
  }
  if (!isPlainObject(routes)) {
    throw new Error("router/init!: :routes must be a map of pattern -> handler");
  }
  const rawMode = readOption(arg, "mode");
  let mode = "hash";
  if (rawMode !== null && rawMode !== void 0) {
    if (typeof rawMode !== "string") {
      throw new Error(`router/init!: :mode must be :hash or :history, got ${typeof rawMode}`);
    }
    const name = stripKeywordColon(rawMode);
    if (name !== "hash" && name !== "history") {
      throw new Error(`router/init!: unknown :mode ${JSON.stringify(name)} (:hash or :history)`);
    }
    mode = name;
  }
  const rawFocus = readOption(arg, "focus");
  let focus = null;
  if (rawFocus === true) {
    focus = true;
  } else if (typeof rawFocus === "string") {
    const selector = stripKeywordColon(rawFocus);
    focus = selector || true;
  } else if (rawFocus !== null && rawFocus !== void 0 && rawFocus !== false) {
    throw new Error("router/init!: :focus must be true or a CSS selector string");
  }
  return {
    mode,
    routes,
    notFound: readHandlerName(readOption(arg, "not-found"), ":not-found"),
    scrollToTop: readOption(arg, "scroll-to-top") === true,
    focus
  };
}
function registerRouterBindings(interp, ctx) {
  const routes = [];
  let mode = "hash";
  let notFound = null;
  let scrollToTop = false;
  let focusTarget = null;
  let teardownListeners = null;
  let lastRaw = null;
  const routeSignalId = ctx.nextSignalId++;
  const routeSignal = signal4(null);
  ctx.signals.set(routeSignalId, routeSignal);
  function readLocation() {
    if (mode === "history") {
      return `${window.location.pathname}${window.location.search}`;
    }
    return window.location.hash.slice(1);
  }
  function matchRoute(path, query) {
    for (const route of routes) {
      const match = path.match(route.regex);
      if (match) {
        const params = {};
        route.paramNames.forEach((name, i) => {
          params[name] = decodeUriComponentSafe(match[i + 1]);
        });
        return { path, params, query, handler: route.handler };
      }
    }
    return null;
  }
  function applyNavigationEffects() {
    if (scrollToTop) {
      try {
        window.scrollTo(0, 0);
      } catch (e) {
        ctx.onerror(e instanceof Error ? e : new Error(String(e)), "router:scroll");
      }
    }
    if (!focusTarget) return;
    try {
      if (focusTarget === true) {
        const active = document.activeElement;
        if (active && typeof active.blur === "function" && active !== document.body) {
          active.blur();
        }
        return;
      }
      const el = document.querySelector(focusTarget);
      if (!el) {
        ctx.onerror(
          new Error(`router: focus target ${JSON.stringify(focusTarget)} not found after navigation`),
          "router:focus"
        );
        return;
      }
      if (!el.hasAttribute("tabindex")) el.setAttribute("tabindex", "-1");
      el.focus();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), "router:focus");
    }
  }
  function updateRoute(initial) {
    const { path, query: queryString } = splitPathQuery(readLocation());
    const raw = queryString ? `${path}?${queryString}` : path;
    if (raw === lastRaw) return;
    lastRaw = raw;
    const query = parseQuery(queryString);
    const matched = matchRoute(path, query);
    const match = matched ?? (notFound ? { path, params: {}, query, handler: notFound } : null);
    routeSignal.value = match;
    ctx.diagnostics.record(() => ({
      kind: "route",
      at: Date.now(),
      context: "route",
      // An unmatched path is the single most common routing complaint, so say
      // so explicitly rather than recording a bare path and leaving the reader
      // to infer it from a missing handler. A configured fallback still says
      // "(no match)" first — that the URL matched nothing is the fact you are
      // looking for; which view covered for it is the follow-up.
      detail: matched ? `${raw} -> ${matched.handler}` : notFound ? `${raw} -> (no match) -> ${notFound}` : `${raw} -> (no match)`
    }));
    if (!initial) applyNavigationEffects();
  }
  const onRouteEvent = () => updateRoute(false);
  function navigate(path, replace, what) {
    const target = linkPath(path);
    if (target === null) {
      ctx.onerror(
        new Error(`${what}: expected an internal path string, got ${describeLinkPath(path)}`),
        "router:navigate"
      );
      return;
    }
    const href = routeHref(mode, target);
    try {
      if (replace) window.history.replaceState(null, "", href);
      else if (mode === "history") window.history.pushState(null, "", href);
      else window.location.hash = href;
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), "router:navigate");
      return;
    }
    updateRoute(false);
  }
  function handleLinkClick(ev) {
    if (ev.defaultPrevented) return;
    if ((ev.button ?? 0) !== 0) return;
    if (ev.metaKey || ev.ctrlKey || ev.shiftKey || ev.altKey) return;
    const target = ev.target;
    if (!target || typeof target.closest !== "function") return;
    const el = target.closest(`[${ROUTER_LINK_ATTR}]`);
    if (!el) return;
    if (el.hasAttribute("download")) return;
    const linkTarget = el.getAttribute("target");
    if (linkTarget && linkTarget !== "_self") return;
    const path = el.getAttribute(ROUTER_LINK_ATTR);
    if (path === null) return;
    ev.preventDefault();
    navigate(path, false, "router/link");
  }
  function buildLink(rawPath, label, rawAttrs) {
    const attrs = linkAttrs(rawAttrs);
    const path = linkPath(rawPath);
    if (path === null) {
      ctx.onerror(
        new Error(
          `router/link: expected an internal path string, got ${describeLinkPath(rawPath)}`
        ),
        "router:link"
      );
      return ["span", attrs, ...linkChildren(label, "")];
    }
    attrs.href = routeHref(mode, path);
    attrs[ROUTER_LINK_ATTR] = path;
    if (!("aria-current" in attrs) && samePath(routeSignal.value?.path, path)) {
      attrs["aria-current"] = "page";
    }
    return ["a", attrs, ...linkChildren(label, path)];
  }
  function linkAttrs(rawAttrs) {
    const attrs = {};
    if (isPlainObject(rawAttrs)) {
      for (const rawKey of Object.keys(rawAttrs)) {
        const key = stripKeywordColon(rawKey);
        if (key === "href" || key === ROUTER_LINK_ATTR) {
          ctx.onerror(
            new Error(`router/link: ${key} is set by the router and cannot be overridden`),
            "router:link"
          );
          continue;
        }
        attrs[key] = rawAttrs[rawKey];
      }
    } else if (rawAttrs !== null && rawAttrs !== void 0) {
      ctx.onerror(
        new Error(`router/link: attrs must be a map, got ${typeof rawAttrs}`),
        "router:link"
      );
    }
    return attrs;
  }
  function linkPath(rawPath) {
    if (typeof rawPath === "number" && Number.isFinite(rawPath)) {
      return normalizeRoutePath(String(rawPath));
    }
    if (typeof rawPath !== "string") return null;
    if (EXTERNAL_PATH_RE.test(rawPath)) return null;
    return normalizeRoutePath(rawPath);
  }
  function describeLinkPath(rawPath) {
    if (typeof rawPath === "string") return `${JSON.stringify(rawPath)} (not an internal path)`;
    if (rawPath === null || rawPath === void 0) return "nil";
    return Array.isArray(rawPath) ? "a list" : typeof rawPath;
  }
  function linkChildren(label, fallback) {
    if (label === null || label === void 0 || label === "") return fallback ? [fallback] : [];
    if (typeof label === "string" || typeof label === "number" || typeof label === "boolean") {
      return [label];
    }
    if (Array.isArray(label)) return [label];
    ctx.onerror(
      new Error(`router/link: label must be text or SIP data, got ${typeof label}`),
      "router:link"
    );
    return fallback ? [fallback] : [];
  }
  interp.registerFunction("router/init!", (arg) => {
    const options = parseInitArg(arg);
    const compiled = [];
    for (const rawPattern of Object.keys(options.routes)) {
      const handler = readHandlerName(options.routes[rawPattern], `handler for ${rawPattern}`);
      if (handler === null) {
        throw new Error(`router/init!: route ${JSON.stringify(rawPattern)} has no handler`);
      }
      const pattern = normalizeRoutePath(
        splitPathQuery(stripKeywordColon(rawPattern)).path
      );
      const { regex, paramNames } = compileRoute(pattern);
      compiled.push({ pattern, regex, paramNames, handler });
    }
    routes.length = 0;
    routes.push(...compiled);
    mode = options.mode;
    notFound = options.notFound;
    scrollToTop = options.scrollToTop;
    focusTarget = options.focus;
    if (teardownListeners) {
      teardownListeners();
      ctx.cleanupHooks.delete(teardownListeners);
    }
    const routeEvent = mode === "history" ? "popstate" : "hashchange";
    window.addEventListener(routeEvent, onRouteEvent);
    document.addEventListener("click", handleLinkClick);
    teardownListeners = () => {
      window.removeEventListener(routeEvent, onRouteEvent);
      document.removeEventListener("click", handleLinkClick);
    };
    ctx.cleanupHooks.add(teardownListeners);
    lastRaw = null;
    updateRoute(true);
    return null;
  });
  interp.registerFunction("router/push!", (path) => {
    navigate(path, false, "router/push!");
    return null;
  });
  interp.registerFunction("router/replace!", (path) => {
    navigate(path, true, "router/replace!");
    return null;
  });
  interp.registerFunction("router/back!", () => {
    window.history.back();
    return null;
  });
  interp.registerFunction("router/current", () => routeSignalId);
  interp.registerFunction("router/href", (path) => {
    const target = linkPath(path);
    if (target === null) {
      ctx.onerror(
        new Error(`router/href: expected an internal path string, got ${describeLinkPath(path)}`),
        "router:href"
      );
      return null;
    }
    return routeHref(mode, target);
  });
  interp.registerFunction(
    "router/link",
    (path, label, attrs) => buildLink(path, label, attrs)
  );
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
  unregisterStream(ctx, signalId);
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
        unregisterStream(ctx, id);
        s.value = {
          ...s.value,
          done: true,
          state: "closed"
        };
      }
    });
    registerStream(ctx, id, {
      kind: "event-source",
      close: managedStream.close
    });
    managedStream.done.catch((e) => {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `event-source:${id}`);
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

// src/resource.ts
import { signal as signal6 } from "@preact/signals-core";
var ALLOWED_AS = /* @__PURE__ */ new Set(["auto", "json", "text"]);
function stripColon(value) {
  return value.startsWith(":") ? value.slice(1) : value;
}
function callbackHandle(value) {
  if (value == null || typeof value !== "object" && typeof value !== "function") return null;
  const handle = value.__semaCallbackHandle;
  return typeof handle === "number" ? handle : null;
}
function snippet(text) {
  const trimmed = text.trim();
  return trimmed.length > 200 ? `${trimmed.slice(0, 200)}...` : trimmed;
}
function reportContained(ctx, error, context) {
  const err = error instanceof Error ? error : new Error(String(error));
  try {
    ctx.onerror(err, context);
  } catch (e) {
    console.error(`[sema-web] error handler threw while reporting ${context}:`, e, err);
  }
}
function requestFingerprint(request) {
  const headers = request.headers ? Object.entries(request.headers).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0) : null;
  return JSON.stringify([
    request.url,
    request.method,
    headers,
    request.body ?? null,
    request.credentials,
    request.as
  ]);
}
function failureFingerprint(error) {
  return `!${error instanceof Error ? error.message : String(error)}`;
}
function normalizeResourceRequest(spec, label) {
  if (typeof spec === "string") {
    if (spec.length === 0) {
      throw new Error(`resource:${label} spec function returned an empty URL`);
    }
    return { url: spec, method: "GET", credentials: "same-origin", as: "auto" };
  }
  if (spec == null || typeof spec !== "object" || Array.isArray(spec)) {
    throw new Error(
      `resource:${label} spec function must return a URL string or an options map with :url`
    );
  }
  const opts = normalizeProps(spec);
  const url = opts.url;
  if (typeof url !== "string" || url.length === 0) {
    throw new Error(
      `resource:${label} spec function must return a URL string or an options map with :url`
    );
  }
  const rawBody = opts.body;
  let body;
  let bodyWasEncoded = false;
  if (rawBody != null) {
    if (typeof rawBody === "string") {
      body = rawBody;
    } else {
      body = JSON.stringify(rawBody);
      bodyWasEncoded = true;
    }
  }
  const headers = normalizeHeaders(opts.headers, bodyWasEncoded);
  const rawMethod = opts.method;
  const method = typeof rawMethod === "string" && rawMethod.length > 0 ? stripColon(rawMethod).toUpperCase() : body != null ? "POST" : "GET";
  const rawAs = opts.as;
  const as = typeof rawAs === "string" ? stripColon(rawAs) : "auto";
  if (!ALLOWED_AS.has(as)) {
    throw new Error(
      `resource:${label} spec :as must be "auto", "json", or "text", got "${as}"`
    );
  }
  const withCredentials = opts.withCredentials ?? opts["with-credentials"];
  return {
    url,
    method,
    headers,
    body,
    credentials: withCredentials ? "include" : "same-origin",
    as
  };
}
function normalizeHeaders(raw, bodyWasEncoded) {
  const out = {};
  if (raw && typeof raw === "object" && !Array.isArray(raw)) {
    for (const [key, value] of Object.entries(raw)) {
      if (value == null) continue;
      out[key] = String(value);
    }
  }
  if (bodyWasEncoded && !Object.keys(out).some((k) => k.toLowerCase() === "content-type")) {
    out["content-type"] = "application/json";
  }
  return Object.keys(out).length > 0 ? out : void 0;
}
async function decodeResourceBody(response, as) {
  const text = await response.text();
  if (as === "text") return text;
  if (text.length === 0) return null;
  if (as === "json") return parseJson(text);
  const contentType = response.headers.get("content-type") ?? "";
  return contentType.toLowerCase().includes("json") ? parseJson(text) : text;
}
function parseJson(text) {
  try {
    return JSON.parse(text);
  } catch {
    throw new Error("resource: response was not valid JSON");
  }
}
function registerResourceBindings(interp, ctx) {
  function createResource(name, specValue) {
    const ownerId = getCurrentOwnerId(ctx);
    const key = name == null ? null : `${ownerId ?? "global"}:${currentScopePath(ctx)}:${name}`;
    if (key != null) {
      const existingId = ctx.resourcesByKey.get(key);
      const existing = existingId != null ? ctx.resources.get(existingId) : void 0;
      if (existingId != null && existing) {
        existing.replaceSpec(specValue);
        return existingId;
      }
      if (existingId != null) ctx.resourcesByKey.delete(key);
    } else if (ownerId != null) {
      throw new Error(
        `(resource) inside a component needs a name: (resource "user" (fn () ...)) \u2014 an unnamed resource is recreated on every call, and a component's render, effects, handlers and timers all run again. A named resource is memoized per component instance and refetches when its request changes.`
      );
    }
    let spec = toInvokableCallback(specValue, interp, "resource spec");
    let currentSpecValue = specValue;
    const id = ctx.nextSignalId++;
    const label = name ?? String(id);
    const state = signal6({ loading: true, value: null, error: null, status: null });
    ctx.signals.set(id, state);
    let seq = 0;
    let controller = null;
    let disposed = false;
    let specCheckQueued = false;
    let lastFingerprint = null;
    const currentValue = () => state.value.value;
    function markLoading() {
      state.value = {
        loading: true,
        value: currentValue(),
        error: null,
        status: state.value.status
      };
    }
    function fail(mySeq, err, status) {
      if (mySeq !== seq) return;
      const error = err instanceof Error ? err : new Error(String(err));
      state.value = { loading: false, value: currentValue(), error: error.message, status };
      reportContained(ctx, error, `resource:${label}`);
    }
    async function runAttempt(mySeq, myController, prepared) {
      if (mySeq !== seq) return;
      let request;
      if (prepared != null) {
        request = prepared;
      } else {
        try {
          request = normalizeResourceRequest(spec(), label);
        } catch (e) {
          lastFingerprint = failureFingerprint(e);
          fail(mySeq, e, null);
          return;
        }
        lastFingerprint = requestFingerprint(request);
      }
      let status = null;
      try {
        const response = await fetch(request.url, {
          method: request.method,
          headers: request.headers,
          body: request.body,
          credentials: request.credentials,
          signal: myController.signal
        });
        if (mySeq !== seq) return;
        status = response.status;
        if (!response.ok) {
          const detail = await response.text().catch(() => "");
          const suffix = detail ? ` - ${snippet(detail)}` : "";
          fail(mySeq, new Error(`HTTP ${response.status}${suffix}`), response.status);
          return;
        }
        const value = await decodeResourceBody(response, request.as);
        if (mySeq !== seq) return;
        state.value = { loading: false, value, error: null, status: response.status };
      } catch (e) {
        if (myController.signal.aborted) return;
        fail(mySeq, e, status);
      }
    }
    function scheduleAttempt(prepared = null) {
      if (disposed) return;
      const mySeq = ++seq;
      controller?.abort();
      const myController = new AbortController();
      controller = myController;
      queueMicrotask(() => {
        void runAttempt(mySeq, myController, prepared).catch((e) => {
          reportContained(ctx, e, `resource:${label}`);
        });
      });
    }
    function scheduleSpecCheck() {
      if (disposed || specCheckQueued) return;
      specCheckQueued = true;
      queueMicrotask(() => {
        try {
          runSpecCheck();
        } catch (e) {
          reportContained(ctx, e, `resource:${label}`);
        }
      });
    }
    function runSpecCheck() {
      specCheckQueued = false;
      if (disposed) return;
      let request;
      try {
        request = normalizeResourceRequest(spec(), label);
      } catch (e) {
        const fingerprint2 = failureFingerprint(e);
        if (fingerprint2 === lastFingerprint) return;
        lastFingerprint = fingerprint2;
        const mySeq = ++seq;
        controller?.abort();
        controller = null;
        fail(mySeq, e, null);
        return;
      }
      const fingerprint = requestFingerprint(request);
      if (fingerprint === lastFingerprint) return;
      const seeding = lastFingerprint === null;
      lastFingerprint = fingerprint;
      if (seeding) return;
      ctx.diagnostics.record(() => ({
        kind: "stream",
        at: Date.now(),
        context: `resource:${id}`,
        detail: "refetch"
      }));
      markLoading();
      scheduleAttempt(request);
    }
    function refresh() {
      if (disposed) return;
      markLoading();
      scheduleAttempt();
    }
    function cancel() {
      if (disposed) return;
      seq += 1;
      controller?.abort();
      controller = null;
      state.value = {
        loading: false,
        value: currentValue(),
        error: null,
        status: state.value.status
      };
    }
    function replaceSpec(nextValue) {
      if (disposed) return;
      adoptSpec(nextValue);
      scheduleSpecCheck();
    }
    function adoptSpec(nextValue) {
      if (nextValue === currentSpecValue) return;
      const nextHandle = callbackHandle(nextValue);
      if (nextHandle != null && nextHandle === callbackHandle(currentSpecValue)) return;
      const next = toInvokableCallback(nextValue, interp, "resource spec");
      const superseded = currentSpecValue;
      spec = next;
      currentSpecValue = nextValue;
      releaseCallback(superseded);
    }
    function dispose() {
      if (disposed) return;
      disposed = true;
      seq += 1;
      controller?.abort();
      controller = null;
      releaseCallback(currentSpecValue);
      unregisterStream(ctx, id);
      ctx.resources.delete(id);
      if (key != null) ctx.resourcesByKey.delete(key);
    }
    registerStream(ctx, id, { kind: "resource", close: cancel });
    ctx.resources.set(id, { key, refresh, cancel, replaceSpec });
    if (key != null) ctx.resourcesByKey.set(key, id);
    registerSignalFinalizer(ctx, id, dispose);
    const owner = ownerId != null ? ctx.mountedComponentsById.get(ownerId) : void 0;
    if (owner) {
      owner.ownedStreamIds.add(id);
      const scope = currentScopePath(ctx);
      let scoped = owner.ownedScopeResources.get(scope);
      if (!scoped) {
        scoped = /* @__PURE__ */ new Set();
        owner.ownedScopeResources.set(scope, scoped);
      }
      scoped.add(id);
    }
    scheduleAttempt();
    return id;
  }
  interp.registerFunction(
    "resource",
    (first, second) => second === void 0 ? createResource(null, first) : createResource(String(first), second)
  );
  interp.registerFunction("resource/refresh!", (handle) => {
    resolveResource(ctx, handle, "resource/refresh!")?.refresh();
    return null;
  });
  interp.registerFunction("resource/cancel!", (handle) => {
    resolveResource(ctx, handle, "resource/cancel!")?.cancel();
    return null;
  });
}
function resolveResource(ctx, handle, fnName) {
  if (typeof handle === "number") {
    return ctx.resources.get(handle) ?? null;
  }
  if (typeof handle === "string") {
    const id = ctx.resourcesByKey.get(
      `${getCurrentOwnerId(ctx) ?? "global"}:${currentScopePath(ctx)}:${handle}`
    );
    const found = id != null ? ctx.resources.get(id) ?? null : null;
    if (!found) {
      throw new Error(`(${fnName} "${handle}") \u2014 no resource named "${handle}" is registered here`);
    }
    return found;
  }
  throw new Error(`(${fnName}) expects a resource handle or name, got ${typeof handle}`);
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
function forgetSocket(ctx, handle, reg) {
  ctx.sockets.delete(handle);
  if (reg.streamId !== void 0) unregisterStream(ctx, reg.streamId);
  for (const cb of reg.callbacks) releaseCallback(cb);
  reg.callbacks.length = 0;
}
function closeSocket(ctx, handle, reg, code, reason) {
  const { socket } = reg;
  socket.onopen = null;
  socket.onmessage = null;
  socket.onerror = null;
  if (!socket.onclose) {
    socket.onclose = () => forgetSocket(ctx, handle, reg);
  }
  try {
    if (typeof code === "number") socket.close(code, reason);
    else socket.close();
  } catch (e) {
    ctx.onerror(e instanceof Error ? e : new Error(String(e)), "ws/close");
  }
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
    const reg = { socket, callbacks: [] };
    ctx.sockets.set(handle, reg);
    const ownerId = getCurrentOwnerId(ctx);
    const owner = ownerId != null ? ctx.mountedComponentsById.get(ownerId) : void 0;
    if (owner) {
      const streamId = ctx.nextSignalId++;
      reg.streamId = streamId;
      registerStream(ctx, streamId, {
        kind: "websocket",
        close: () => closeSocket(ctx, handle, reg)
      });
      owner.ownedStreamIds.add(streamId);
    }
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
    closeSocket(ctx, handle, reg, code, reason);
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
      const guarded = (label, run) => {
        try {
          run();
        } catch (e) {
          ctx.onerror(e instanceof Error ? e : new Error(String(e)), `ws/listen ${label}`);
        }
      };
      if (onOpen) {
        if (socket.readyState === WebSocket.OPEN) {
          guarded("on-open", () => onOpen(handle));
        } else {
          socket.onopen = () => guarded("on-open", () => onOpen(handle));
        }
      }
      if (onMessage) {
        socket.onmessage = (ev) => {
          const data = ev.data instanceof ArrayBuffer ? new Uint8Array(ev.data) : ev.data;
          guarded("on-message", () => onMessage(handle, data));
        };
      }
      socket.onclose = (ev) => {
        try {
          if (onClose) {
            guarded("on-close", () => onClose(handle, { ":code": ev.code, ":reason": ev.reason }));
          }
        } finally {
          forgetSocket(ctx, handle, reg);
        }
      };
      if (onError) {
        socket.onerror = () => guarded("on-error", () => onError(handle, "websocket error"));
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
function scriptLabel(src, inlineIndex) {
  return src ? `script:${src}` : `inline-script:${inlineIndex}`;
}
async function loadScripts(interp, opts, reporter) {
  const mimeType = opts?.type ?? "text/sema";
  const scripts = document.querySelectorAll(`script[type="${mimeType}"]`);
  const results = [];
  let inlineIndex = 0;
  const note = (context, detail) => {
    reporter?.diagnostics.record(() => ({
      kind: "script",
      at: Date.now(),
      context,
      detail
    }));
  };
  for (const script of scripts) {
    const src = script.getAttribute("src");
    const label = scriptLabel(src, src ? -1 : inlineIndex++);
    let code;
    if (src) {
      try {
        const resp = await fetch(src);
        if (!resp.ok) {
          const err = `Failed to fetch ${src}: ${resp.status} ${resp.statusText}`;
          console.error(`[sema-web] ${err}`);
          note(label, err);
          results.push({ value: null, output: [], error: err });
          continue;
        }
        const artifactKind = classifyExternalScript(src);
        if (artifactKind === "archive") {
          if (!interp.loadArchive || !interp.runEntry && !interp.runEntryAsync) {
            const err = `Runtime does not support compiled web archives: ${src}`;
            console.error(`[sema-web] ${err}`);
            note(label, err);
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
            note(label, err);
            results.push({ value: null, output: [], error: err });
            continue;
          }
          if (!archiveInfo.ok) {
            const err = archiveInfo.error || `Failed to load archive ${src}`;
            console.error(`[sema-web] ${err}`);
            note(label, err);
            results.push({ value: null, output: [], error: err });
            continue;
          }
          if (!archiveInfo.entryPoint) {
            const err = `Archive ${src} did not provide an entry point`;
            console.error(`[sema-web] ${err}`);
            note(label, err);
            results.push({ value: null, output: [], error: err });
            continue;
          }
          const result = interp.runEntryAsync ? await interp.runEntryAsync(archiveInfo.entryPoint) : interp.runEntry(archiveInfo.entryPoint);
          for (const line of result.output) {
            console.log(`[sema] ${line}`);
          }
          if (result.error) {
            console.error(`[sema-web] Error in ${label}: ${result.error}`);
            note(label, result.error);
          } else {
            note(label, `loaded archive ${archiveInfo.entryPoint}`);
          }
          results.push(result);
          continue;
        }
        code = await resp.text();
      } catch (e) {
        const err = `Failed to fetch ${src}: ${e instanceof Error ? e.message : String(e)}`;
        console.error(`[sema-web] ${err}`);
        note(label, err);
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
        console.error(`[sema-web] Error in ${label}: ${result.error}`);
        note(label, result.error);
      } else {
        note(label, "evaluated");
      }
      results.push(result);
    } catch (e) {
      const err = `Evaluation error in ${label}: ${e instanceof Error ? e.message : String(e)}`;
      console.error(`[sema-web] ${err}`);
      note(label, err);
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
    this._disposeOverlay = null;
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
    const dev = resolveDevOptions(opts?.dev);
    ctx.diagnostics.enabled = dev.enabled;
    ctx.diagnostics.limit = dev.limit;
    ctx.diagnostics.slowRenderMs = dev.slowRenderMs;
    if (opts?.onerror) {
      ctx.onerror = opts.onerror;
    }
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
    if (opts?.resources !== false) {
      registerResourceBindings(interp, ctx);
    }
    if (opts?.websocket !== false) {
      registerWsBindings(interp, ctx);
    }
    if (opts?.llmProxy) {
      const proxyOpts = typeof opts.llmProxy === "string" ? { url: opts.llmProxy } : opts.llmProxy;
      registerLlmBindings(interp, proxyOpts, ctx);
    }
    if (dev.overlay) {
      try {
        const { mountDevOverlay } = await import("./devtools-UQLFFQYD.js");
        web._disposeOverlay = mountDevOverlay(ctx);
      } catch (e) {
        ctx.onerror(e instanceof Error ? e : new Error(String(e)), "dev-overlay");
      }
    }
    if (opts?.autoLoad !== false) {
      await loadScripts(interp, opts?.loader, ctx);
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
   * The dev-mode diagnostics ring.
   *
   * Always present; records nothing unless `dev` was enabled at create time.
   * Reading it without dev mode returns an empty timeline rather than
   * throwing, so instrumentation code does not need to branch.
   *
   * @example
   * ```js
   * web.diagnostics.byKind("error");   // what failed
   * web.diagnostics.slowRenders();     // which components are over budget
   * web.diagnostics.dropped;           // entries evicted by the size bound
   * ```
   */
  get diagnostics() {
    return this._ctx.diagnostics;
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
    if (this._disposeOverlay) {
      try {
        this._disposeOverlay();
      } catch (e) {
        this._ctx.onerror(e instanceof Error ? e : new Error(String(e)), "dev-overlay-cleanup");
      }
      this._disposeOverlay = null;
    }
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
  DEFAULT_DEV_LIMIT,
  DEFAULT_SLOW_RENDER_MS,
  Diagnostics,
  ROUTER_LINK_ATTR,
  SemaWeb,
  SemaWebContext,
  decodeResourceBody,
  loadScripts,
  normalizeResourceRequest,
  normalizeRoutePath,
  parseQuery,
  registerComponentBindings,
  registerCssBindings,
  registerDomBindings,
  registerHttpBindings,
  registerLlmBindings,
  registerReactiveBindings,
  registerResourceBindings,
  registerRouterBindings,
  registerSipBindings,
  registerStoreBindings,
  renderSip,
  routeHref,
  splitPathQuery
};
//# sourceMappingURL=index.js.map