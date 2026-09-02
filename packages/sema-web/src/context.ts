/**
 * Instance-scoped state container for Sema Web.
 *
 * All module-level singletons (handles, signals, mounted components, etc.)
 * are collected here so that multiple SemaWeb instances can coexist
 * without interference.
 *
 * @module
 */

import type { Signal } from "@preact/signals-core";
import { releaseCallback, type SemaCallback } from "./callbacks.js";
import { Diagnostics } from "./diagnostics.js";

/** A mounted component managed by the component system. */
export interface MountedComponent {
  instanceId: number;
  target: Element;
  componentFn: string;
  /**
   * Initial props, colon-stripped and ready to hand back to Sema, or `null`
   * when the component was mounted without any.
   *
   * `null` is meaningfully different from `{}`: a component declared with no
   * parameters throws if called with an argument, and every component written
   * before props existed is exactly that. `null` means "call with no args".
   */
  props: Record<string, unknown> | null;
  dispose: (() => void) | null;
  /**
   * A render of this component — body, DOM patch, and lifecycle flush — is in
   * flight.
   *
   * Guards re-entrancy: `component/force-render!` called from a render or an
   * effect body used to dispose the effect it was running inside, start a
   * nested one, and re-enter the same not-yet-stored lifecycle slot, which
   * recursed until the JS stack overflowed and left an orphaned render effect
   * that kept firing after unmount.
   */
  rendering: boolean;
  /**
   * Teardown has begun. Set before anything is drained, so work still on the
   * stack — an effect body that unmounted its own component — can tell that the
   * registries it is about to write to have already been emptied.
   */
  destroyed: boolean;
  eventCleanup: (() => void) | null;
  localState: Map<string, number>;
  mountCleanup: (() => void) | null;
  pendingMount: unknown;
  ownedSignalIds: Set<number>;
  ownedWatchIds: Set<number>;
  ownedIntervalIds: Set<number>;
  ownedStreamIds: Set<number>;
  ownedListenerKeys: Set<string>;
  /**
   * Lifecycle registrations made by the render currently in flight, drained by
   * the post-render flush. Cleared at the start of every render, so a render
   * that throws cannot leave half a registration behind.
   *
   * Keyed by render-scope path (see {@link SemaWebContext.childScopeStack}), so
   * a child invoked through `component/render` accumulates into its own bucket
   * rather than the mounting component's.
   */
  pendingLifecycle: Map<string, PendingLifecycle[]>;
  /**
   * Live lifecycle slots per render-scope path, indexed within a scope by
   * registration order. Disposed — cleanups run — when the scope disappears
   * from a render, or when the component is destroyed.
   */
  ownedLifecycleSlots: Map<string, LifecycleSlot[]>;
  /**
   * Scope paths entered during the render in flight. A scope that was live but
   * is absent here has been removed from the tree, and its slots are disposed
   * by the post-render flush.
   */
  visitedScopes: Set<string>;
  /**
   * Resource signal ids per render scope.
   *
   * Resources also land in `ownedStreamIds`, which is only drained at component
   * teardown. A composed child that leaves the tree mid-life needs its own
   * resources released then, not whenever its parent eventually unmounts.
   */
  ownedScopeResources: Map<string, Set<number>>;
  /** What the render pass in flight owns on the reactive axis. */
  renderReactive: RenderScopedReactive;
}

/** A `watch` registration owned by a component's render body. */
export interface RenderScopedWatch {
  /** Key into {@link SemaWebContext.watchDisposers}. */
  watchId: number;
  /** Take on a later render's callback, handing the superseded one back. */
  adopt: (callbackValue: unknown) => void;
  /** Stop the subscription and release its callback handle. */
  dispose: () => void;
}

/**
 * `watch` and `computed` registrations made by a component's render body.
 *
 * `(local name …)` and `(resource name …)` memoize on their name; `(watch …)`
 * and `(computed …)` have none, so a render body that called them allocated a
 * *live* registration on every pass — 31 renders left 31 subscriptions, all
 * firing on every change and each holding a WASM callback handle until unmount.
 *
 * The two are recycled differently because they fail differently. A watch is a
 * subscription that outlives the render, so it is memoized: the subscription
 * survives and each pass swaps its callback. A computed is a derivation the
 * render reads back immediately, and a memoized one would keep serving the
 * value its *first* closure produced until a tracked dependency happened to
 * change — stale data rather than a leak — so it is disposed and rebuilt.
 */
export interface RenderScopedReactive {
  /** Disposers for computeds this pass allocated. */
  computeds: Array<() => void>;
  /** Live watches, keyed by render scope, watched signal, and occurrence. */
  watches: Map<string, RenderScopedWatch>;
  /** Watch keys the pass in flight has claimed. */
  claimed: Set<string>;
  /** Per-pass occurrence counter for repeated watches on one signal. */
  occurrences: Map<string, number>;
  /** Guidance already emitted, so it is said once per component. */
  reported: Set<string>;
}

/** An empty reactive-ownership record for a freshly mounted component. */
export function createRenderScopedReactive(): RenderScopedReactive {
  return {
    computeds: [],
    watches: new Map(),
    claimed: new Set(),
    occurrences: new Map(),
    reported: new Set(),
  };
}

/**
 * What a lifecycle registration is.
 *
 * `"effect"` runs its body after every render whose deps changed;
 * `"unmount"` never runs a body — the registered function *is* the cleanup.
 * One kind tag rather than two registries keeps flush, prune, and disposal to
 * a single code path.
 */
export type LifecycleKind = "effect" | "unmount";

/** A lifecycle registration captured during render, not yet flushed. */
export interface PendingLifecycle {
  kind: LifecycleKind;
  /** `null` means "no dependency list" — re-run after every render. */
  deps: unknown[] | null;
  /** Effect body, or (for `unmount`) the cleanup itself. */
  fn: SemaCallback;
}

/** A live lifecycle slot owned by a mounted component. */
export interface LifecycleSlot extends PendingLifecycle {
  /** Runs before the next re-run and once at teardown; `null` when there is none. */
  cleanup: SemaCallback | null;
  /**
   * Already retired — its cleanup ran and its handles went back.
   *
   * A cleanup is user code and may unmount the component whose slots are being
   * reconciled, which re-enters teardown's drain over the same array while a
   * flush is still holding one of its entries. Without this flag that slot's
   * callback handle would be handed back twice.
   */
  retired?: boolean;
}

export interface ListenerRegistration {
  target: EventTarget;
  event: string;
  listener: EventListener;
  callback?: SemaCallback;
}

export interface WatchRegistration {
  dispose: () => void;
  callback?: SemaCallback;
}

export interface IntervalRegistration {
  callback?: SemaCallback;
}

export interface StreamRegistration {
  kind: "event-source" | "llm-stream" | "resource" | "websocket";
  close: () => void;
}

/**
 * Control surface for an async resource, keyed by its signal id.
 *
 * Everything a resource actually owns — the abort controller, the sequence
 * number, the spec callback — stays private to `resource.ts`. This record
 * exists so `resource/refresh!` and `resource/cancel!` can find a resource by
 * handle or by name, and so teardown can assert the registry drained.
 */
export interface ResourceRegistration {
  /**
   * `<ownerId|global>:<scope path>:<name>` for a named resource, `null` for an
   * unnamed one. The scope path is what gives each composed child instance its
   * own resource instead of one shared with its siblings.
   */
  key: string | null;
  /** Start a fresh attempt, keeping the current value while it runs. */
  refresh: () => void;
  /** Abort the in-flight attempt. Not a failure: nothing is reported. */
  cancel: () => void;
  /**
   * Adopt the spec function from the latest render, releasing the superseded
   * one, and re-check on a clean stack whether the request it describes moved.
   */
  replaceSpec: (specValue: unknown) => void;
}

export interface SocketRegistration {
  socket: WebSocket;
  /** Sema callbacks wired via `ws/listen`, released on close/dispose. */
  callbacks: SemaCallback[];
  /**
   * Stream id the socket is registered under when a component opened it, so
   * unmounting that component closes the socket like any other owned stream.
   */
  streamId?: number;
}

/** Error handler callback type. */
export type ErrorHandler = (error: Error, context: string) => void;

/**
 * Per-instance state container for SemaWeb.
 *
 * Each `SemaWeb.create()` call produces its own `SemaWebContext`,
 * ensuring complete isolation between instances (handles, signals,
 * mounted components, event listeners, etc.).
 */
export class SemaWebContext {
  /** DOM element/text/event handles */
  handles = new Map<number, Element | Text | Event>();
  handleIds = new WeakMap<Element | Text | Event, number>();
  nextHandle = 1;

  /** Reactive signals */
  signals = new Map<number, Signal<any>>();
  nextSignalId = 1;

  /** Mounted components */
  mountedComponents = new Map<string, MountedComponent>();
  mountedComponentsById = new Map<number, MountedComponent>();
  nextComponentId = 1;

  /** Next capture ID for callComponent */
  nextCaptureId = 1;

  /** Component render context stack (per-instance for multi-instance isolation) */
  renderContextStack: number[] = [];

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
  captureEvents = new Set<string>();

  /** Current execution owner stack for callbacks invoked outside render. */
  ownerStack: number[] = [];

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
  childScopeStack: string[] = [];

  /**
   * Per-parent-scope counters for un-keyed children, reset each render.
   *
   * A child with no `:key` gets its ordinal among same-named siblings. Stable
   * as long as the sibling set is, which is exactly the condition under which
   * an un-keyed list is stable anyway.
   */
  childScopeCounters: Map<string, number> = new Map();

  /** DOM event listeners registry */
  listeners = new Map<string, ListenerRegistration>();

  /** Reactive watch cleanup callbacks */
  watchDisposers = new Map<number, WatchRegistration>();
  nextWatchId = 1;

  /** Browser interval handles */
  intervals = new Map<number, IntervalRegistration>();

  /** Managed streaming resources keyed by signal id */
  streams = new Map<number, StreamRegistration>();

  /**
   * Async resources keyed by signal id.
   *
   * Drained by each resource's signal finalizer, not by a defensive `clear()`
   * in {@link disposeContextResources}: a leftover entry after dispose means a
   * finalizer did not run, which also means a Sema callback handle leaked, and
   * clearing the map here would hide exactly that.
   */
  resources = new Map<number, ResourceRegistration>();

  /** Named resources, `<ownerId|global>:<scope path>:<name>` to signal id. */
  resourcesByKey = new Map<string, number>();

  /** Open WebSocket connections keyed by numeric handle */
  sockets = new Map<number, SocketRegistration>();
  nextSocketId = 1;

  /** Per-signal cleanup hooks (used for callback-backed computed signals, etc.) */
  signalFinalizers = new Map<number, () => void>();

  /** Runtime-level cleanup hooks */
  cleanupHooks = new Set<() => void>();

  /** Instance-owned scoped CSS stylesheet */
  styleEl: HTMLStyleElement | null = null;
  cssNamespace = Math.random().toString(36).slice(2, 10);
  nextCssClassId = 1;

  /**
   * Bounded dev-mode event recorder. Disabled unless `SemaWeb.create({dev})`
   * turned it on, in which case `record()` is a no-op that allocates nothing.
   */
  diagnostics = new Diagnostics();

  private _onerror: ErrorHandler = (error, context) => {
    console.error(`[sema-web] Error in ${context}:`, error);
  };

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
  get onerror(): ErrorHandler {
    return (error, context) => {
      this.diagnostics.record(() => ({
        kind: "error",
        at: Date.now(),
        context,
        detail: error?.message ?? String(error),
      }));
      this._onerror(error, context);
    };
  }

  set onerror(handler: ErrorHandler) {
    this._onerror = handler;
  }
}

/**
 * Register a managed stream and note it in the dev timeline.
 *
 * Streams are opened and closed from four different modules; routing every
 * mutation through this pair is what keeps the recorded lifecycle honest. A
 * direct `ctx.streams.set(...)` would silently skip the timeline, and a
 * half-recorded lifecycle ("opened, never closed") reads as a leak that isn't
 * one — worse than no record at all.
 */
export function registerStream(
  ctx: SemaWebContext,
  id: number,
  registration: StreamRegistration,
): void {
  ctx.streams.set(id, registration);
  ctx.diagnostics.record(() => ({
    kind: "stream",
    at: Date.now(),
    context: `${registration.kind}:${id}`,
    detail: "open",
  }));
}

/** Drop a managed stream, noting it only if it was actually registered. */
export function unregisterStream(ctx: SemaWebContext, id: number): void {
  const registration = ctx.streams.get(id);
  if (!registration) return;
  ctx.streams.delete(id);
  ctx.diagnostics.record(() => ({
    kind: "stream",
    at: Date.now(),
    context: `${registration.kind}:${id}`,
    detail: "close",
  }));
}

/**
 * Path identifying the render scope currently registering lifecycle and state.
 *
 * `""` is the mounting component itself; a composed child appends its
 * `name#key` segment. See `SemaWebContext.childScopeStack` for why this second
 * axis exists.
 */
export function currentScopePath(ctx: SemaWebContext): string {
  return ctx.childScopeStack.join("/");
}

/**
 * Live lifecycle slots for one render scope.
 *
 * Introspection helper for tests and dev tooling. Slots are stored per scope
 * so a composed child owns its own; reading the raw Map means encoding that
 * layout at every call site, which is how a test ends up asserting the shape
 * of the storage rather than the behaviour.
 */
export function lifecycleSlots(component: MountedComponent, scope = ""): LifecycleSlot[] {
  return component.ownedLifecycleSlots.get(scope) ?? [];
}

function safely(component: MountedComponent, ctx: SemaWebContext, fn: () => void): void {
  try {
    fn();
  } catch (e) {
    ctx.onerror(
      e instanceof Error ? e : new Error(String(e)),
      `render-reactive-cleanup:${component.componentFn}`,
    );
  }
}

/**
 * Open a render pass on the reactive axis: drop the computeds the previous
 * pass built and reset what this one may claim.
 *
 * Computeds go before the body rather than after it, because by now the
 * previous render's DOM is patched and its values have been read, whereas
 * sweeping mid-render could release a callback the current pass is still
 * reading through.
 */
export function beginRenderScopedReactive(
  component: MountedComponent,
  ctx: SemaWebContext,
): void {
  const owned = component.renderReactive;
  // Splice rather than iterate-and-clear: a disposer is user-facing cleanup and
  // must not be handed out twice if one of them re-enters the render.
  for (const dispose of owned.computeds.splice(0)) safely(component, ctx, dispose);
  owned.claimed.clear();
  owned.occurrences.clear();
}

/**
 * Close a render pass: dispose every watch the body stopped registering.
 *
 * Runs whether the body returned or threw. A render that throws part-way
 * registered less than it meant to, and losing a subscription it will rebuild
 * on the next pass is cheaper than keeping one nothing will ever claim again.
 */
export function endRenderScopedReactive(
  component: MountedComponent,
  ctx: SemaWebContext,
): void {
  const owned = component.renderReactive;
  for (const [key, watch] of owned.watches) {
    if (owned.claimed.has(key)) continue;
    owned.watches.delete(key);
    safely(component, ctx, watch.dispose);
  }
}

/** The signal id backing `(local name)` within one render scope. */
export function localCell(
  component: MountedComponent,
  name: string,
  scope = "",
): number | undefined {
  return component.localState.get(`${scope}\u0000${name}`);
}

/**
 * Render scopes holding at least one `(local …)` cell.
 *
 * A composed child that owns no lifecycle slot and no resource is still a scope
 * that can leave the tree, and its cells have to go with it: `localState` is
 * keyed by the *mounting* component, so nothing else would ever prune them and
 * a churning list (search results, infinite scroll, a paginated table) grew the
 * map without bound.
 */
export function localScopes(component: MountedComponent): Set<string> {
  const scopes = new Set<string>();
  for (const key of component.localState.keys()) {
    scopes.add(key.slice(0, key.indexOf("\u0000")));
  }
  return scopes;
}

/** Dispose the `(local …)` cells one render scope owns and forget them. */
export function disposeLocalScope(
  component: MountedComponent,
  ctx: SemaWebContext,
  scope: string,
): void {
  const prefix = `${scope}\u0000`;
  for (const [key, signalId] of component.localState) {
    if (!key.startsWith(prefix)) continue;
    component.localState.delete(key);
    try {
      disposeSignal(ctx, signalId);
    } catch (e) {
      ctx.onerror(
        e instanceof Error ? e : new Error(String(e)),
        `component-local-state-cleanup:${component.componentFn}`,
      );
    }
  }
}

export function getCurrentOwnerId(ctx: SemaWebContext): number | null {
  const ownerId = ctx.ownerStack[ctx.ownerStack.length - 1];
  if (ownerId != null) return ownerId;
  const renderId = ctx.renderContextStack[ctx.renderContextStack.length - 1];
  return renderId != null ? renderId : null;
}

export function withOwnerContext<T>(
  ctx: SemaWebContext,
  ownerId: number | null,
  fn: () => T,
): T {
  if (ownerId == null) return fn();
  ctx.ownerStack.push(ownerId);
  try {
    return fn();
  } finally {
    ctx.ownerStack.pop();
  }
}

export function registerSignalFinalizer(
  ctx: SemaWebContext,
  signalId: number,
  finalizer: () => void,
): void {
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

export function disposeSignal(ctx: SemaWebContext, signalId: number): void {
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

export function disposeContextResources(ctx: SemaWebContext): void {
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
        `${stream.kind}-cleanup`,
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

  // Subscribers (the dev overlay) hold DOM references; dropping them here is
  // what makes `dispose()` leak-free with the overlay enabled. Retained
  // entries are left alone — reading the timeline after teardown is exactly
  // when you most want it.
  ctx.diagnostics.clearListeners();
}
