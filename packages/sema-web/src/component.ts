/**
 * Component system for Sema — reactive mounting of SIP views.
 *
 * Provides `mount!` to bind a Sema component function to a DOM element.
 * The component automatically re-renders when reactive state it depends on changes.
 *
 * Uses morphdom for efficient DOM patching and delegated event handling
 * via `data-sema-on-*` attributes set by the SIP renderer.
 *
 * ## Usage
 *
 * ```sema
 * (def count (state 0))
 *
 * (define (counter-view)
 *   [:div {:class "counter"}
 *     [:p (deref count)]
 *     [:button {:on-click "increment"} "+"]])
 *
 * (define (increment ev) (update! count (lambda (n) (+ n 1))))
 *
 * (mount! "#app" "counter-view")
 * ```
 *
 * @module
 */

import morphdom from "morphdom";
import { signal, effect, untracked } from "@preact/signals-core";
import {
  type SemaWebContext,
  type MountedComponent,
  type LifecycleSlot,
  type PendingLifecycle,
  disposeSignal,
  getCurrentOwnerId,
  currentScopePath,
  beginRenderScopedReactive,
  createRenderScopedReactive,
  disposeLocalScope,
  endRenderScopedReactive,
  lifecycleSlots,
  localCell,
  localScopes,
  withOwnerContext,
  unregisterStream,
} from "./context.js";
import { renderSip, sipNodeKey, readEventModifiers, DELEGATED_EVENTS } from "./sip.js";
import { now as diagnosticsNow } from "./diagnostics.js";
import { SEMA_IDENT_RE, storeHandle, releaseHandle, releaseHandlesForSubtree } from "./handles.js";
import {
  toInvokableCallback,
  retainCallback,
  releaseCallback,
  type SemaCallback,
} from "./callbacks.js";

interface SemaInterpreterLike {
  registerFunction(name: string, fn: (...args: any[]) => any): void;
  invokeGlobal(name: string, ...args: any[]): any;
  evalStr(code: string): { value: string | null; output: string[]; error: string | null };
}

/**
 * Call a Sema component function and capture its structured return value.
 *
 * Uses a unique registered capture function per call to avoid race conditions
 * when multiple components render.
 *
 * Pushes the component function name onto the render context stack before eval
 * and pops it after, so that `local` and `on-mount` can discover which
 * component is currently rendering.
 */
function withComponentContext<T>(ctx: SemaWebContext, componentId: number, fn: () => T): T {
  return withOwnerContext(ctx, componentId, () => {
    ctx.renderContextStack.push(componentId);
    try {
      return fn();
    } finally {
      ctx.renderContextStack.pop();
    }
  });
}

/**
 * Strip the leading colon Sema keyword keys carry when they cross into JS.
 *
 * A map built in Sema arrives here as `{":items": [...]}`, and handing that
 * straight back produces a map whose keys are literally `":items"` — so
 * `(:items props)` inside the component looks up `items` and finds nothing.
 * Every prop silently reads as nil, with no error anywhere. Normalizing on the
 * way in makes the round trip transparent.
 *
 * Recursive, because nested prop maps hit the same problem one level down.
 * Arrays are walked; anything else is passed through untouched.
 */
export function normalizeProps<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.map((item) => normalizeProps(item)) as unknown as T;
  }
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, nested] of Object.entries(value as Record<string, unknown>)) {
      out[key.startsWith(":") ? key.slice(1) : key] = normalizeProps(nested);
    }
    return out as unknown as T;
  }
  return value;
}

/**
 * Error context for a component, naming its props when it has any.
 *
 * Prop *keys* only. Values routinely hold user data, auth tokens, and whole
 * API responses, and this string reaches `onerror` — i.e. whatever error
 * reporter the app installed. Keys are enough to identify which render failed.
 */
function componentErrorContext(component: MountedComponent): string {
  const base = `component:${component.componentFn}`;
  const keys = component.props ? Object.keys(component.props) : [];
  return keys.length > 0 ? `${base}{${keys.join(",")}}` : base;
}


/**
 * Returned by {@link callComponent} when the component function threw.
 *
 * Distinct from `null`, which a component legitimately returns when it renders
 * nothing. The difference matters to the lifecycle flush: a render that threw
 * registered nothing usable, so its live effects must be left alone, whereas a
 * render that returned nil still declared its lifecycle and must be flushed.
 */
const RENDER_FAILED = Symbol("sema-web:render-failed");

/**
 * Name the cause behind the reactive library's bare "Cycle detected".
 *
 * A render body that writes state it also reads re-triggers itself until
 * signals-core's iteration guard fires. What reaches `onerror` is "Cycle
 * detected" wrapped in a WASM stack trace — it names neither the component's
 * mistake nor the form that made it, and the fix is one line of user code.
 *
 * The original error is kept (message appended, stack intact) rather than
 * replaced: the wrapped detail is still the only evidence of where the write
 * happened.
 */
function withRenderCycleHint(error: Error): Error {
  if (!/Cycle detected/i.test(error.message)) return error;
  error.message =
    `${error.message} — this render wrote reactive state that it also reads. `
    + "Move the (put! …) / (update! …) into an (effect …), an (on-mount …), or an event handler.";
  return error;
}

function callComponent(interp: SemaInterpreterLike, component: MountedComponent, ctx: SemaWebContext): any {
  // Brackets the body: what the previous pass built through `watch`/`computed`
  // is superseded by what this one registers, and whatever this pass stops
  // registering is disposed on the way out.
  beginRenderScopedReactive(component, ctx);
  component.pendingLifecycle.clear();
  component.visitedScopes.clear();
  // The mounting component is always scope "" — a component with no composed
  // children behaves exactly as before this axis existed.
  component.visitedScopes.add("");
  ctx.childScopeStack.length = 0;
  ctx.childScopeCounters.clear();
  try {
    return withComponentContext(ctx, component.instanceId, () =>
      // Called with no arguments unless props were supplied. Passing an
      // argument to a zero-parameter Sema function is an arity error, and
      // every component written before props existed takes no parameters.
      component.props === null
        ? interp.invokeGlobal(component.componentFn)
        : interp.invokeGlobal(component.componentFn, component.props),
    );
  } catch (e) {
    // Whatever the failed render managed to register is unreachable now — the
    // flush is skipped for a failed render — so hand the callback handles back
    // instead of stranding them in the WASM callback table.
    releasePendingLifecycle(component);
    component.visitedScopes.clear();
    ctx.childScopeStack.length = 0;
    // A child's failure never reaches here: `component/render` catches it and
    // reports under the child's own name. This is the mounted component's own
    // body failing.
    ctx.onerror(
      withRenderCycleHint(e instanceof Error ? e : new Error(String(e))),
      componentErrorContext(component),
    );
    return RENDER_FAILED;
  } finally {
    endRenderScopedReactive(component, ctx);
  }
}


export { lifecycleSlots, localCell };

/**
 * Drop every registration this render made but never handed to a slot.
 *
 * Reached when a render throws, when teardown drains, and when a body destroyed
 * its own component mid-flush: in each case nothing will ever run these, and
 * the handles they hold would otherwise sit in the WASM callback table forever.
 */
function releasePendingLifecycle(component: MountedComponent): void {
  for (const bucket of component.pendingLifecycle.values()) {
    for (const pending of bucket) releaseCallback(pending.fn);
  }
  component.pendingLifecycle.clear();
}

/** The pending-registration bucket for a scope, created on first use. */
function pendingFor(component: MountedComponent, scope: string): PendingLifecycle[] {
  let bucket = component.pendingLifecycle.get(scope);
  if (!bucket) {
    bucket = [];
    component.pendingLifecycle.set(scope, bucket);
  }
  return bucket;
}

/**
 * Compare two effect dependency lists.
 *
 * Structural, not identity-based: a Sema list or map crosses into JS through
 * JSON, so the *same* Sema value arrives as a different JS object on every
 * render. `Object.is` would therefore report "changed" every time and turn any
 * deps list holding a map or a list into an effect that re-runs each render —
 * silently, with no error anywhere.
 *
 * A `null` list means "no deps were given", which is defined as always
 * re-running, so it never compares equal — not even to another `null`.
 */
export function depsEqual(a: unknown[] | null, b: unknown[] | null): boolean {
  if (a == null || b == null) return false;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (!depValueEqual(a[i], b[i])) return false;
  }
  return true;
}

function depValueEqual(a: unknown, b: unknown): boolean {
  // Object.is first: it settles every primitive, and treats NaN as equal to
  // itself, which is what a dep holding a NaN should mean.
  if (Object.is(a, b)) return true;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return a.every((item, i) => depValueEqual(item, b[i]));
  }
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") {
    // Functions land here: two distinct Sema callbacks are never equal, which
    // is right — a fresh closure genuinely is a new dependency.
    return false;
  }
  const aKeys = Object.keys(a as Record<string, unknown>);
  const bKeys = Object.keys(b as Record<string, unknown>);
  if (aKeys.length !== bKeys.length) return false;
  for (const key of aKeys) {
    if (!Object.prototype.hasOwnProperty.call(b, key)) return false;
    if (!depValueEqual((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key])) {
      return false;
    }
  }
  return true;
}

/**
 * Turn whatever an effect body returned into a cleanup callback.
 *
 * Mirrors `on-mount`: a function value is used directly, and a Sema global's
 * name is resolved to an invoker. Anything else (nil, a number, a map) means
 * "this effect has no cleanup" rather than an error — an effect body's return
 * value is usually incidental.
 */
function coerceCleanup(value: unknown, interp: SemaInterpreterLike): SemaCallback | null {
  if (typeof value === "function") {
    // The slot is about to hold this until its next run or teardown, and two
    // effects returning the *same* Sema function share one handle, so take a
    // reference rather than assuming sole ownership.
    retainCallback(value);
    return value as SemaCallback;
  }
  if (typeof value === "string" && SEMA_IDENT_RE.test(value)) {
    return toInvokableCallback(value, interp, "effect cleanup");
  }
  return null;
}

/**
 * Run a lifecycle registration and capture its cleanup.
 *
 * Runs under {@link withOwnerContext} rather than {@link withComponentContext}:
 * the owner stack is what makes an interval, watch, stream, or signal created
 * inside the body owned by this component, while leaving the *render* stack
 * empty so `(local …)` / `(effect …)` / `(on-mount …)` still throw when called
 * from inside an effect body instead of silently corrupting slot indices.
 */
function runLifecycleSlot(
  component: MountedComponent,
  interp: SemaInterpreterLike,
  ctx: SemaWebContext,
  index: number,
  pending: PendingLifecycle,
): LifecycleSlot {
  if (pending.kind === "unmount") {
    // Nothing runs now — the registered function is the teardown itself.
    return { ...pending, cleanup: pending.fn };
  }
  let cleanup: SemaCallback | null = null;
  try {
    const result = withOwnerContext(ctx, component.instanceId, () => pending.fn());
    cleanup = coerceCleanup(result, interp);
  } catch (e) {
    ctx.onerror(
      e instanceof Error ? e : new Error(String(e)),
      `effect:${component.componentFn}#${index}`,
    );
  }
  return { ...pending, cleanup };
}

/**
 * Why a slot is being retired.
 *
 * `"dispose"` is a teardown — the component, or the composed child that owned
 * the scope, is going away. `"rekey"` is a live component whose latest render
 * no longer registers this slot at this index.
 */
type RetireReason = "dispose" | "rekey";

/**
 * Run a slot's cleanup and hand back the callback handles it held.
 *
 * An `on-unmount` hook is a *teardown*, not a per-render cleanup: the prelude
 * documents it as running "once when the component is destroyed". So a re-key
 * retires it without invoking it. Invoking it there made a render that changed
 * its slot count fire the hook mid-life and then fire it a second time at
 * teardown — twice, silently, for a component still on screen.
 */
function retireLifecycleSlot(
  component: MountedComponent,
  ctx: SemaWebContext,
  index: number,
  slot: LifecycleSlot,
  reason: RetireReason,
): void {
  // Idempotent: a cleanup may unmount the component being reconciled, and
  // teardown's drain then walks the same slot list this flush is holding.
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
        `${kind}:${component.componentFn}#${index}`,
      );
    }
  }
  if (cleanup) releaseCallback(cleanup);
  // For an `unmount` slot the body *is* the cleanup, already released above.
  if (slot.fn !== cleanup) releaseCallback(slot.fn);
}

/**
 * Reconcile the lifecycle registrations of the render that just finished.
 *
 * Slots are matched by registration order, so a component must register its
 * lifecycle unconditionally — the same rule React's hooks carry, and the price
 * of `(effect deps fn)` having no name to key on the way `(local name …)` does.
 */
function flushLifecycle(
  component: MountedComponent,
  interp: SemaInterpreterLike,
  ctx: SemaWebContext,
): void {
  // A scope that was live but was not entered this render has left the tree —
  // a switched branch, a removed row. Dispose it before reconciling survivors,
  // so a departing child's cleanup runs before a replacement's body.
  // The union of scopes owning anything: a child may own only a resource, or
  // only a `local` cell, and it still has to be released when it leaves.
  const owning = new Set<string>([
    ...component.ownedLifecycleSlots.keys(),
    ...component.ownedScopeResources.keys(),
    ...localScopes(component),
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
  // A cleanup or an effect body may have destroyed this component while the
  // flush was walking it. Everything still pending is unreachable — teardown
  // has already drained the slot lists — so hand its handles back rather than
  // dropping the buckets on the floor.
  releasePendingLifecycle(component);
}

/**
 * Note a lifecycle registration that changed shape between renders.
 *
 * Slots are matched by index, so a render that registers a different sequence
 * silently re-points every slot after the change. The consequences are
 * invisible from Sema — an effect adopting another effect's cleanup, an
 * `on-unmount` hook that never runs — which is why this is reported rather
 * than merely tolerated.
 */
function reportSlotShapeChange(
  component: MountedComponent,
  ctx: SemaWebContext,
  index: number,
  detail: string,
): void {
  ctx.onerror(
    new Error(
      `lifecycle slot ${index} ${detail}. Register (effect …) and (on-unmount …) `
      + "unconditionally at the top level of the component body — slots are matched "
      + "by call order, so a render that registers a different sequence re-keys the rest.",
    ),
    `lifecycle:${component.componentFn}#${index}`,
  );
}

/** Reconcile one render scope's slots against what this render registered. */
function flushScope(
  component: MountedComponent,
  interp: SemaInterpreterLike,
  ctx: SemaWebContext,
  scope: string,
): void {
  const live = component.ownedLifecycleSlots.get(scope) ?? [];
  const pending = component.pendingLifecycle.get(scope) ?? [];
  if (live.length === 0 && pending.length === 0) return;
  // Ownership of the pending bucket moves to this flush. A lifecycle body is
  // user code and may destroy its own component, and teardown's drain must not
  // release a callback this loop is in the middle of handing to a slot — that
  // would hand one handle back twice.
  component.pendingLifecycle.delete(scope);
  component.ownedLifecycleSlots.set(scope, live);

  /**
   * Teardown ran while this flush was walking the scope: give back everything
   * the flush is still holding and leave nothing on the destroyed component.
   */
  const abandon = (fromPending: number): void => {
    while (live.length > 0) {
      const index = live.length - 1;
      retireLifecycleSlot(component, ctx, index, live.pop()!, "dispose");
    }
    component.ownedLifecycleSlots.delete(scope);
    for (let rest = fromPending; rest < pending.length; rest++) {
      releaseCallback(pending[rest].fn);
    }
  };

  // Slots this render stopped registering: clean them up and drop them.
  for (let index = live.length - 1; index >= pending.length; index--) {
    const slot = live[index];
    if (slot.kind === "unmount") {
      reportSlotShapeChange(
        component,
        ctx,
        index,
        "held an (on-unmount …) hook that this render no longer registers, so the hook will never run",
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
    const current: LifecycleSlot | undefined = live[index];
    if (current && current.kind === next.kind && depsEqual(current.deps, next.deps)) {
      // Unchanged: keep the running registration and hand back the reference
      // this render's re-registration took.
      releaseCallback(next.fn);
      continue;
    }
    if (current) {
      if (current.kind !== next.kind) {
        reportSlotShapeChange(
          component,
          ctx,
          index,
          `changed from ${current.kind === "unmount" ? "(on-unmount …)" : "(effect …)"} to `
          + `${next.kind === "unmount" ? "(on-unmount …)" : "(effect …)"} between renders`,
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
      // The body destroyed its own component. Teardown already drained the slot
      // list, so storing this would resurrect lifecycle state on a dead
      // component and strand the cleanup the body just returned — run it now
      // and stop reconciling.
      retireLifecycleSlot(component, ctx, index, slot, "dispose");
      abandon(index + 1);
      return;
    }
    live[index] = slot;
  }
  if (live.length === 0) component.ownedLifecycleSlots.delete(scope);
}

/** Run every cleanup a departing scope owns, innermost first, and drop it. */
function disposeScope(component: MountedComponent, ctx: SemaWebContext, scope: string): void {
  // Resources first: a lifecycle cleanup may read one, and disposing it out
  // from under a cleanup that is about to run would be the same class of bug
  // this whole scope axis exists to remove.
  const resources = component.ownedScopeResources.get(scope);
  if (resources) {
    for (const id of resources) {
      try {
        disposeSignal(ctx, id);
      } catch (e) {
        ctx.onerror(
          e instanceof Error ? e : new Error(String(e)),
          `component-resource-cleanup:${component.componentFn}`,
        );
      }
      component.ownedStreamIds.delete(id);
    }
    component.ownedScopeResources.delete(scope);
  }

  const slots = component.ownedLifecycleSlots.get(scope);
  if (slots) {
    // Drains rather than indexing a snapshot: a cleanup is user code and may
    // register or destroy further work.
    while (slots.length > 0) {
      const index = slots.length - 1;
      const slot = slots.pop()!;
      try {
        retireLifecycleSlot(component, ctx, index, slot, "dispose");
      } catch (e) {
        ctx.onerror(
          e instanceof Error ? e : new Error(String(e)),
          `component-effect-cleanup:${component.componentFn}`,
        );
      }
    }
    component.ownedLifecycleSlots.delete(scope);
  }

  // Local cells last: a cleanup that just ran may read the state it is tearing
  // down, and disposing a signal out from under it would be the same class of
  // bug the scope axis exists to remove.
  disposeLocalScope(component, ctx, scope);
}

/** Run every owned cleanup, innermost registration first, and empty the slots. */
function disposeLifecycleSlots(component: MountedComponent, ctx: SemaWebContext): void {
  // Drains rather than indexes a snapshot: a cleanup is user code and may
  // register or destroy further work, so the loop re-reads the array each time
  // and only stops once nothing is left.
  while (component.ownedLifecycleSlots.size > 0) {
    const scope = Array.from(component.ownedLifecycleSlots.keys()).pop()!;
    const slots = component.ownedLifecycleSlots.get(scope)!;
    if (slots.length === 0) {
      component.ownedLifecycleSlots.delete(scope);
      continue;
    }
    const index = slots.length - 1;
    const slot = slots.pop()!;
    try {
      retireLifecycleSlot(component, ctx, index, slot, "dispose");
    } catch (e) {
      ctx.onerror(
        e instanceof Error ? e : new Error(String(e)),
        `component-effect-cleanup:${component.componentFn}`,
      );
    }
  }
  releasePendingLifecycle(component);
}

/**
 * The morphdom configuration every SIP patch uses.
 *
 * Exported so tests patch the DOM exactly the way a render does. These options
 * are not incidental — `getNodeKey` is what makes `:key` mean anything, and
 * `onBeforeElUpdated` is what stops a re-render from clobbering what the user
 * is currently typing. A test that re-specifies a subset of them proves
 * something about the subset, not about the renderer.
 *
 * @param activeElement - Snapshot of `document.activeElement` from before the
 *   patch. Read up front because morphdom's own DOM surgery can move focus
 *   mid-patch, at which point the answer to "what was focused" has changed.
 */
export function sipMorphOptions(ctx: SemaWebContext, activeElement: Element | null) {
  return {
    childrenOnly: true as const,
    // Honour SIP `:key`. Without this morphdom matches children by position,
    // so reordering a list rewrites every row in place: focus jumps, a
    // half-typed input lands on the wrong row, and any DOM state the app did
    // not put there (scroll, <details> open, a CSS animation mid-flight) goes
    // with it. Falls back to `id`, which is morphdom's own default.
    getNodeKey: sipNodeKey,
    onBeforeElUpdated(fromEl: Element, toEl: Element) {
      // Preserve what the user is editing: the value, the caret, and the
      // selection. Returning `false` is what protects those — morphdom's own
      // INPUT/TEXTAREA/SELECT handlers would assign `value` and
      // `selectedIndex` from the freshly rendered clone — but it also skips
      // everything else morphdom would have done, so this branch owes the
      // element the rest of the patch by hand.
      if (
        fromEl === activeElement &&
        (fromEl.tagName === "INPUT" || fromEl.tagName === "TEXTAREA" || fromEl.tagName === "SELECT")
      ) {
        syncAttributesPreservingValue(fromEl, toEl);
        // A `<select>`'s options are ordinary markup, not live state: a
        // dependent dropdown whose options load asynchronously must still
        // update for the user who has it focused — precisely the user about to
        // pick from it. INPUT has no children, and a TEXTAREA's child text is
        // its *default* value, which morphdom's TEXTAREA handler would push
        // into the live one.
        if (fromEl.tagName === "SELECT") {
          morphFocusedSelectOptions(fromEl as HTMLSelectElement, toEl, ctx);
        }
        return false;
      }
      return true;
    },
    onNodeDiscarded(node: Node) {
      releaseHandlesForSubtree(node, ctx);
    },
  };
}

/**
 * Bring `fromEl`'s attributes in line with `toEl`'s, leaving `value` alone.
 *
 * morphdom's `morphAttrs` in miniature, and it has to be: an element the
 * renderer protects from morphdom still has to see the render's *removals*.
 * Copying only additions left a focused input carrying whatever the previous
 * render declared — a stale `class`, a `required` the app dropped, and worst,
 * a removed `data-sema-on-*` whose handler the delegator kept dispatching
 * forever because the attribute it matches on was still there.
 *
 * `value` is exempt in both directions: on an input it is the *default* value,
 * and the whole point of this path is that the live one belongs to the user.
 */
function syncAttributesPreservingValue(fromEl: Element, toEl: Element): void {
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
  // Snapshotted: removing while iterating a live NamedNodeMap skips entries.
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

/**
 * Patch a focused `<select>`'s options without disturbing what is selected.
 *
 * A nested `childrenOnly` morph, because morphdom skips the whole subtree of an
 * element whose `onBeforeElUpdated` returned false. Selection is restored *by
 * value* rather than by index: morphdom's SELECT handler drives `selectedIndex`
 * from the `selected` attribute, which a user's own pick never sets, so an
 * untouched patch would silently deselect what they chose. An option that the
 * patch removed cannot be restored, and nothing is forced if none survives.
 */
function morphFocusedSelectOptions(
  fromEl: HTMLSelectElement,
  toEl: Element,
  ctx: SemaWebContext,
): void {
  const selected = new Set<string>();
  for (const option of Array.from(fromEl.options)) {
    if (option.selected) selected.add(option.value);
  }

  morphdom(fromEl, toEl, {
    childrenOnly: true,
    getNodeKey: sipNodeKey,
    onNodeDiscarded(node: Node) {
      releaseHandlesForSubtree(node, ctx);
    },
  });

  const options = Array.from(fromEl.options);
  if (!options.some((option) => selected.has(option.value))) return;
  for (const option of options) {
    option.selected = selected.has(option.value);
  }
}

/**
 * Render a mounted component using effect() for automatic dependency tracking
 * and morphdom for efficient DOM patching.
 *
 * Refuses to start a render *inside* one. `component/force-render!` reached
 * from a render or an effect body would otherwise dispose the effect it is
 * running inside, start a nested one, and re-enter a lifecycle slot the flush
 * has not stored yet — unbounded recursion ending in a stack overflow, with the
 * nested effect's disposer overwritten by the outer call and so undisposable:
 * it kept re-rendering after unmount, unreachable by anything.
 */
function renderComponent(
  component: MountedComponent,
  interp: SemaInterpreterLike,
  ctx: SemaWebContext,
): void {
  if (component.rendering) {
    ctx.onerror(
      new Error(
        `component/force-render!: ${component.componentFn} is already rendering, so the `
        + "request was ignored. A render or effect body does not need to force a render — "
        + "the component re-renders when the state it reads changes.",
      ),
      `force-render:${component.componentFn}`,
    );
    return;
  }

  // Dispose previous effect if any
  if (component.dispose) component.dispose();

  component.dispose = effect(() => {
    // Measured across the whole render — the Sema call, SIP construction, and
    // the morphdom patch — because "which component is slow" is the question,
    // and splitting it across phases would answer a question nobody asked.
    const startedAt = ctx.diagnostics.enabled ? diagnosticsNow() : 0;
    component.rendering = true;
    try {
      const sipData = callComponent(interp, component, ctx);
      if (sipData === RENDER_FAILED) return;

      // A render body that destroyed its own component has already been torn
      // down; patching would put its markup back on a dead component.
      if (component.destroyed) {
        releasePendingLifecycle(component);
        return;
      }

      if (sipData != null) {
        const clone = component.target.cloneNode(false) as Element;
        const sipNode = renderSip(sipData, interp, ctx);
        clone.appendChild(sipNode);

        morphdom(component.target, clone, sipMorphOptions(ctx, document.activeElement));

        ctx.diagnostics.record(() => ({
          kind: "render",
          at: Date.now(),
          context: `component:${component.componentFn}`,
          durationMs: diagnosticsNow() - startedAt,
        }));
      }

      // After the patch, so an effect body observes the DOM this render
      // produced, and after the render entry, so effect time is not billed as
      // render time.
      //
      // `untracked` is load-bearing: an effect body that reads a signal must
      // not thereby make that signal a dependency of the *render*, or reading
      // state inside an effect silently becomes a re-render loop.
      untracked(() => flushLifecycle(component, interp, ctx));
    } finally {
      component.rendering = false;
    }
  });
}

/**
 * Find the currently rendering mounted component.
 */
function getActiveComponent(ctx: SemaWebContext): MountedComponent | null {
  const componentId = ctx.renderContextStack[ctx.renderContextStack.length - 1];
  return componentId != null ? ctx.mountedComponentsById.get(componentId) ?? null : null;
}

/**
 * Require a component render to be in progress, and return that component.
 *
 * `local`, `on-mount`, `effect`, and `on-unmount` all bind to the component
 * being rendered; called from anywhere else there is nothing for them to
 * attach to, and for the ordered lifecycle slots there would be no slot index
 * either.
 *
 * @param form - the user-facing Sema form name, for the error message.
 */
function requireRenderingComponent(ctx: SemaWebContext, form: string): MountedComponent {
  if (ctx.renderContextStack.length === 0) {
    throw new Error(`(${form}) called outside of a component render context`);
  }
  const component = getActiveComponent(ctx);
  if (!component) {
    throw new Error(`(${form}) no active mounted component`);
  }
  return component;
}

function cleanupWatch(ctx: SemaWebContext, watchId: number): void {
  const registration = ctx.watchDisposers.get(watchId);
  if (registration) {
    registration.dispose();
    ctx.watchDisposers.delete(watchId);
    releaseCallback(registration.callback);
  }
}

function cleanupInterval(ctx: SemaWebContext, intervalId: number): void {
  clearInterval(intervalId);
  const registration = ctx.intervals.get(intervalId);
  if (registration) {
    releaseCallback(registration.callback);
  }
  ctx.intervals.delete(intervalId);
}

function cleanupStream(ctx: SemaWebContext, signalId: number): void {
  const stream = ctx.streams.get(signalId);
  if (stream) {
    stream.close();
    unregisterStream(ctx, signalId);
  }
}

function cleanupListener(ctx: SemaWebContext, key: string): void {
  const registration = ctx.listeners.get(key);
  if (!registration) return;
  registration.target.removeEventListener(registration.event, registration.listener);
  releaseCallback(registration.callback);
  ctx.listeners.delete(key);
}

/**
 * Drain an owned-id registry during teardown, running `cleanup` for each id
 * inside a try/catch that reports (never throws) on a failing individual
 * cleanup, before the caller clears the registry. The stream case passes a
 * composing callback because it cleans up two registries per item.
 */
function drainOwned<T>(
  component: MountedComponent,
  ctx: SemaWebContext,
  ids: Iterable<T>,
  cleanup: (id: T) => void,
  label: string,
): void {
  for (const id of ids) {
    try {
      cleanup(id);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `${label}:${component.componentFn}`);
    }
  }
}

function destroyMountedComponent(selector: string, component: MountedComponent, ctx: SemaWebContext): void {
  // Before anything is drained. An effect body that unmounts its own component
  // is still on the stack below this call, and the flush it will return into
  // has to know that the registries it was about to write are gone.
  component.destroyed = true;

  if (component.pendingMount) {
    releaseCallback(component.pendingMount);
    component.pendingMount = null;
  }

  if (component.mountCleanup) {
    try {
      withComponentContext(ctx, component.instanceId, () => component.mountCleanup!());
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

  // After the render effect is stopped, deliberately. A cleanup is user code
  // and may write a signal; with the effect still live that would re-render the
  // component being destroyed and re-register the very slots being drained.
  disposeLifecycleSlots(component, ctx);

  if (component.eventCleanup) {
    try {
      component.eventCleanup();
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-events:${component.componentFn}`);
    }
    component.eventCleanup = null;
  }

  drainOwned(component, ctx, component.ownedListenerKeys, (k) => cleanupListener(ctx, k), 'component-listener-cleanup');
  component.ownedListenerKeys.clear();

  drainOwned(component, ctx, component.ownedSignalIds, (id) => disposeSignal(ctx, id), 'component-signal-cleanup');
  component.ownedSignalIds.clear();

  drainOwned(component, ctx, component.ownedWatchIds, (id) => cleanupWatch(ctx, id), 'component-watch-cleanup');
  component.ownedWatchIds.clear();

  drainOwned(component, ctx, component.ownedIntervalIds, (id) => cleanupInterval(ctx, id), 'component-interval-cleanup');
  component.ownedIntervalIds.clear();

  drainOwned(component, ctx, component.ownedStreamIds, (id) => {
    cleanupStream(ctx, id);
    disposeSignal(ctx, id);
  }, 'component-stream-cleanup');
  component.ownedStreamIds.clear();

  drainOwned(component, ctx, component.localState.values(), (id) => disposeSignal(ctx, id), 'component-local-state-cleanup');
  component.localState.clear();

  for (const child of Array.from(component.target.childNodes)) {
    releaseHandlesForSubtree(child, ctx);
  }
  component.target.innerHTML = "";
  ctx.mountedComponents.delete(selector);
  ctx.mountedComponentsById.delete(component.instanceId);
}

/**
 * Which pass of the delegator is running a handler.
 *
 * `"synthetic"` is the `mouseenter`/`mouseleave` emulation, which is derived
 * from `mouseover`/`mouseout` and therefore has no capture phase of its own.
 */
type DispatchPhase = "capture" | "bubble" | "synthetic";

/**
 * Delegated events a browser treats as passive unless asked otherwise.
 *
 * Only when the listener is on the document, body, or window — but `mount!`
 * takes any selector, and "`.prevent` works unless you mounted on `<body>`" is
 * not a rule anyone should have to learn.
 */
const PASSIVE_BY_DEFAULT = new Set(["wheel", "touchstart", "touchmove"]);

/**
 * Delegated event handler that intercepts events on the mount target
 * and dispatches to Sema callback functions via `data-sema-on-*` attributes.
 *
 * Two listeners are registered per delegated event: one bubbling, one
 * capturing. A handler declared `.capture` belongs to the capture pass and the
 * rest belong to the bubble pass, so every handler runs exactly once. Reversing
 * the order within a single bubble-phase pass would not do: a descendant's own
 * `addEventListener` calling `stopPropagation` would then stop the event before
 * the mount root's listener ever ran, and the `.capture` handler would silently
 * never fire.
 */
class EventDelegator {
  /**
   * Elements whose `.once` handler for an event has already fired.
   *
   * Not a DOM attribute: morphdom re-syncs every attribute from the freshly
   * rendered clone on each patch, so an attribute mark would be restored on the
   * next render and the handler would fire again. The semantics are therefore
   * "once per element instance" — a node morphdom replaces re-arms — which is
   * also why the tests for it must use the real morphdom.
   */
  private firedOnce = new WeakMap<Element, Set<string>>();

  setup(
    component: MountedComponent,
    target: Element,
    interp: SemaInterpreterLike,
    ctx: SemaWebContext,
  ): () => void {
    const listeners: Array<{ event: string; listener: EventListener; capture: boolean }> = [];

    for (const event of DELEGATED_EVENTS) {
      // A passive listener cannot `preventDefault`, and browsers make these
      // passive by default when the listener sits on the document, body, or
      // window — so an app mounted on <body> would find `.prevent` silently
      // failing on exactly the events people scroll-lock with.
      const passive = PASSIVE_BY_DEFAULT.has(event) ? { passive: false } : undefined;

      const bubbleListener = (ev: Event) => {
        this.walk("bubble", event, component, target, interp, ctx, ev);
      };
      target.addEventListener(event, bubbleListener, passive);
      listeners.push({ event, listener: bubbleListener, capture: false });

      const captureListener = (ev: Event) => {
        // Gated so that an app using no `.capture` anywhere pays a single Set
        // lookup per event rather than a second ancestor walk.
        if (!ctx.captureEvents.has(event)) return;
        this.walk("capture", event, component, target, interp, ctx, ev);
      };
      target.addEventListener(event, captureListener, passive ? { ...passive, capture: true } : true);
      listeners.push({ event, listener: captureListener, capture: true });
    }

    // mouseenter / mouseleave, synthesized from the bubbling mouseover /
    // mouseout pair plus `relatedTarget`.
    const mouseOverListener = (ev: Event) => {
      this.walkSynthetic("mouseenter", component, target, interp, ctx, ev as MouseEvent);
    };
    target.addEventListener("mouseover", mouseOverListener);
    listeners.push({ event: "mouseover", listener: mouseOverListener, capture: false });

    const mouseOutListener = (ev: Event) => {
      this.walkSynthetic("mouseleave", component, target, interp, ctx, ev as MouseEvent);
    };
    target.addEventListener("mouseout", mouseOutListener);
    listeners.push({ event: "mouseout", listener: mouseOutListener, capture: false });

    return () => {
      for (const { event, listener, capture } of listeners) {
        // The capture flag is part of a listener's identity — omitting it here
        // leaves every capture listener attached past unmount.
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
  private walk(
    phase: "capture" | "bubble",
    event: string,
    component: MountedComponent,
    target: Element,
    interp: SemaInterpreterLike,
    ctx: SemaWebContext,
    ev: Event,
  ): void {
    const attr = `data-sema-on-${event}`;
    const start = ev.target;
    if (!(start instanceof Element) || !target.contains(start)) {
      return;
    }

    const chain: Element[] = [];
    for (let el: Element | null = start; el; el = el.parentElement) {
      chain.push(el);
      if (el === target) break;
    }
    if (phase === "capture") chain.reverse();

    for (const el of chain) {
      if (!el.hasAttribute(attr)) continue;
      const fn = el.getAttribute(attr)!;
      if (!SEMA_IDENT_RE.test(fn)) continue;
      this.tryRun(interp, ctx, component, el, ev, event, fn, phase);
      // Stop walking if the handler (or its `.stop` modifier) stopped propagation
      if (ev.cancelBubble || (ev as any).__sema_stop) break;
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
  private walkSynthetic(
    event: "mouseenter" | "mouseleave",
    component: MountedComponent,
    target: Element,
    interp: SemaInterpreterLike,
    ctx: SemaWebContext,
    ev: MouseEvent,
  ): void {
    const attr = `data-sema-on-${event}`;
    const start = ev.target;
    if (!(start instanceof Element) || !target.contains(start)) return;

    const chain: Element[] = [];
    for (let el: Element | null = start; el; el = el.parentElement) {
      if (el.hasAttribute(attr) && !el.contains(ev.relatedTarget as Node)) chain.push(el);
      if (el === target) break;
    }
    // The chain is collected innermost-first, which is `mouseleave` order.
    if (event === "mouseenter") chain.reverse();

    for (const el of chain) {
      this.tryRun(interp, ctx, component, el, ev, event, el.getAttribute(attr)!, "synthetic");
      if (ev.cancelBubble || (ev as any).__sema_stop) break;
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
  private tryRun(
    interp: SemaInterpreterLike,
    ctx: SemaWebContext,
    component: MountedComponent,
    el: Element,
    ev: Event,
    event: string,
    fn: string,
    phase: DispatchPhase,
  ): void {
    const mods = readEventModifiers(el, event);

    // `.capture` is a documented no-op for the synthetic mouseenter/mouseleave
    // pair — they are derived from mouseover/mouseout and have no capture phase
    // — so those handlers still run rather than silently disappearing.
    if (phase === "capture" && !mods.capture) return;
    if (phase === "bubble" && mods.capture) return;

    if (mods.self && ev.target !== el) return;

    if (mods.prevent) ev.preventDefault();

    if (mods.once) {
      let fired = this.firedOnce.get(el);
      if (!fired) {
        fired = new Set();
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
      (ev as any).__sema_stop = true;
    }
  }

  private dispatchEvent(
    interp: SemaInterpreterLike,
    ctx: SemaWebContext,
    callbackName: string,
    ev: Event,
    declaringElement: Element | null,
  ) {
    const evHandle = storeHandle(ev, ctx);
    // `ev.currentTarget` is the mount root for every delegated handler, which
    // is never the element the app wrote the attribute on. Stash the real one
    // for `dom/event-current-target`, saving and restoring so nested dispatch
    // of the same event object does not leave the wrong answer behind.
    const previousDeclaring = (ev as any).__sema_current_target;
    (ev as any).__sema_current_target = declaringElement;
    try {
      interp.invokeGlobal(callbackName, evHandle);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `event:${ev.type}:${callbackName}`);
    } finally {
      (ev as any).__sema_current_target = previousDeclaring;
      if (evHandle != null) releaseHandle(evHandle, ctx);
    }
  }
}

/**
 * The Sema half of the component system, evaluated at registration time.
 *
 * A module-level constant rather than an inline template so tests can run the
 * exact source the runtime uses through a real interpreter. Macros embedded in
 * a TypeScript string are invisible to the type checker and to a mocked
 * `evalStr`; the only other signal that one is malformed is a thrown
 * `SemaWeb.create()` in a browser.
 */
export const COMPONENT_SEMA_PRELUDE = `
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
    ;; unconditionally at the top level of a component body — a render that
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

/**
 * Register `component/*` namespace functions and the `mount!` Sema wrapper.
 *
 * Functions registered:
 * - `component/mount!` — mount a component function to a CSS selector
 * - `component/unmount!` — unmount a component
 * - `component/force-render!` — force re-render of a mounted component
 * - `__component/current-id` — get current component ID from render context stack
 *
 * Sema wrapper:
 * - `(mount! selector fn-name)` — convenience alias for component/mount!
 */
export function registerComponentBindings(interp: SemaInterpreterLike, ctx: SemaWebContext): void {
  // component/mount! — mount a component to a DOM target
  interp.registerFunction(
    "component/mount!",
    (selector: string, componentFn: string, props?: unknown) => {
      // Validate component function name
      if (!SEMA_IDENT_RE.test(componentFn)) {
        throw new Error(`Invalid component function name: ${componentFn}`);
      }
      if (props != null && (typeof props !== "object" || Array.isArray(props))) {
        throw new Error(
          `mount! props must be a map, got ${Array.isArray(props) ? "list" : typeof props}`,
        );
      }

      const target = document.querySelector(selector);
      if (!target) throw new Error(`mount! target not found: ${selector}`);

      // Unmount existing component at this selector
      const existing = ctx.mountedComponents.get(selector);
      if (existing) {
        destroyMountedComponent(selector, existing, ctx);
      }

      const component: MountedComponent = {
        instanceId: ctx.nextComponentId++,
        target,
        componentFn,
        props: props == null ? null : normalizeProps(props as Record<string, unknown>),
        dispose: null,
        rendering: false,
        destroyed: false,
        eventCleanup: null,
        localState: new Map(),
        mountCleanup: null,
        pendingMount: null,
        ownedSignalIds: new Set(),
        ownedWatchIds: new Set(),
        ownedIntervalIds: new Set(),
        ownedStreamIds: new Set(),
        ownedListenerKeys: new Set(),
        pendingLifecycle: new Map(),
        ownedLifecycleSlots: new Map(),
        visitedScopes: new Set(),
        ownedScopeResources: new Map(),
        renderReactive: createRenderScopedReactive(),
      };

      // Set up delegated event handling on the mount target
      const delegator = new EventDelegator();
      component.eventCleanup = delegator.setup(component, target, interp, ctx);

      // Store component before rendering so local/on-mount can find it
      ctx.mountedComponents.set(selector, component);
      ctx.mountedComponentsById.set(component.instanceId, component);

      // Initial render via effect (will auto-track reactive dependencies)
      renderComponent(component, interp, ctx);

      // Handle on-mount callback after first render
      const pendingMount = component.pendingMount;
      if (pendingMount) {
        try {
          const mountCallback = toInvokableCallback(pendingMount, interp, "on-mount callback");
          const cleanupFn = withComponentContext(ctx, component.instanceId, () => mountCallback());
          if (typeof cleanupFn === "function") {
            // Store cleanup function to call on unmount. Retained because two
            // components returning the same named cleanup share one handle, and
            // the first teardown would otherwise invalidate the second's.
            const finalCleanupFn = cleanupFn as SemaCallback;
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
    },
  );

  // component/unmount! — unmount a component from a DOM target
  interp.registerFunction("component/unmount!", (selector: string) => {
    const component = ctx.mountedComponents.get(selector);
    if (component) {
      destroyMountedComponent(selector, component, ctx);
    }
    return null;
  });

  // component/force-render! — force re-render by disposing and re-creating effect
  interp.registerFunction("component/force-render!", (selector: string) => {
    const component = ctx.mountedComponents.get(selector);
    if (component) {
      renderComponent(component, interp, ctx);
    }
    return null;
  });

  // __component/current-id — get current component from render context stack
  interp.registerFunction("__component/current-id", () => {
    const stack = ctx.renderContextStack;
    return stack.length > 0 ? stack[stack.length - 1] : null;
  });

  // __component/enter-child / exit-child — bracket a composed child's render so
  // its lifecycle, local state and resources bind to the child instance rather
  // than to whichever component happens to be mounting it.
  interp.registerFunction("__component/enter-child", (name: string, key: unknown) => {
    const parent = currentScopePath(ctx);
    let segment: string;
    if (key != null && key !== "") {
      segment = `${name}#${String(key)}`;
    } else {
      // No :key — fall back to the ordinal among same-named siblings of this
      // parent scope. Stable exactly as long as the sibling set is, which is
      // the same condition under which an un-keyed list is stable at all.
      const counterKey = `${parent}\u0000${name}`;
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

  // __component/local — component-scoped state (name-based, no call-order dependency)
  interp.registerFunction("__component/local", (name: string, initialValue: any) => {
    const component = requireRenderingComponent(ctx, "local");

    // Keyed by scope AND name. Name alone made every row of a list share one
    // cell, because a composed child renders inside its parent's context.
    const scopedName = `${currentScopePath(ctx)}\u0000${name}`;
    const existingId = component.localState.get(scopedName);
    if (existingId != null) {
      return existingId;
    }

    // First call: create a new signal
    const id = ctx.nextSignalId++;
    const s = signal(initialValue);
    ctx.signals.set(id, s);
    component.localState.set(scopedName, id);
    return id;
  });

  // __component/on-mount — register lifecycle callback, called once after first render
  interp.registerFunction("__component/on-mount", (callbackValue: any) => {
    const component = requireRenderingComponent(ctx, "on-mount");

    releaseCallback(component.pendingMount);
    component.pendingMount = callbackValue;
    return null;
  });

  // __component/effect — run after render, re-run when deps change
  interp.registerFunction("__component/effect", (deps: any, callbackValue: any) => {
    const component = requireRenderingComponent(ctx, "effect");
    // Validated before the callback is converted so a bad call cannot strand a
    // callback handle in the WASM table.
    if (deps != null && !Array.isArray(deps)) {
      throw new Error(`(effect) deps must be a list or nil, got ${typeof deps}`);
    }
    const fn = toInvokableCallback(callbackValue, interp, "effect callback");
    pendingFor(component, currentScopePath(ctx)).push({
      kind: "effect",
      deps: deps == null ? null : (deps as unknown[]),
      fn,
    });
    return null;
  });

  // __component/on-unmount — run once when the component is torn down.
  //
  // Registered with empty deps, which always compare equal, so a re-render
  // re-registering the hook keeps the original rather than running it early.
  interp.registerFunction("__component/on-unmount", (callbackValue: any) => {
    const component = requireRenderingComponent(ctx, "on-unmount");
    const fn = toInvokableCallback(callbackValue, interp, "on-unmount callback");
    pendingFor(component, currentScopePath(ctx)).push({ kind: "unmount", deps: [], fn });
    return null;
  });

  // js/set-interval — browser setInterval wrapper
  interp.registerFunction("js/set-interval", (callbackValue: any, ms: number) => {
    const callback = toInvokableCallback(callbackValue, interp, "interval callback");
    const ownerId = getCurrentOwnerId(ctx);
    // `window.setInterval`, not the bare global: the numeric handle is what
    // crosses into Sema and what `ctx.intervals` is keyed by, and only the DOM
    // overload returns a number.
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

  // js/clear-interval — browser clearInterval wrapper
  interp.registerFunction("js/clear-interval", (id: number) => {
    cleanupInterval(ctx, id);
    for (const component of ctx.mountedComponents.values()) {
      component.ownedIntervalIds.delete(id);
    }
    return null;
  });

  // __component/report-error — surface a child-component failure by name.
  //
  // Takes two plain strings so nothing has to marshal a Sema error value into
  // JS and back. `component/render` catches in Sema and calls this.
  interp.registerFunction("__component/report-error", (name: string, message: string) => {
    ctx.onerror(new Error(String(message)), `component:${String(name)}`);
    return null;
  });

  // --- Sema-side convenience wrappers ---
  const semaResult = interp.evalStr(COMPONENT_SEMA_PRELUDE);

  if (semaResult.error) {
    throw new Error(`[sema-web] Failed to register component wrappers: ${semaResult.error}`);
  }
}

export function disposeAllComponents(ctx: SemaWebContext): void {
  for (const [selector, component] of Array.from(ctx.mountedComponents.entries())) {
    try {
      destroyMountedComponent(selector, component, ctx);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-dispose-all:${component.componentFn}`);
    }
  }
}
