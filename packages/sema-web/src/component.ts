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
import { signal, effect } from "@preact/signals-core";
import {
  type SemaWebContext,
  type MountedComponent,
  disposeSignal,
  getCurrentOwnerId,
  withOwnerContext,
} from "./context.js";
import { renderSip } from "./sip.js";
import { SEMA_IDENT_RE, storeHandle, releaseHandle, releaseHandlesForSubtree } from "./handles.js";
import { toInvokableCallback, releaseCallback, type SemaCallback } from "./callbacks.js";

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

function callComponent(interp: SemaInterpreterLike, component: MountedComponent, ctx: SemaWebContext): any {
  try {
    return withComponentContext(ctx, component.instanceId, () => interp.invokeGlobal(component.componentFn));
  } catch (e) {
    ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component:${component.componentFn}`);
    return null;
  }
}

/**
 * Render a mounted component using effect() for automatic dependency tracking
 * and morphdom for efficient DOM patching.
 */
function renderComponent(
  component: MountedComponent,
  interp: SemaInterpreterLike,
  ctx: SemaWebContext,
): void {
  // Dispose previous effect if any
  if (component.dispose) component.dispose();

  component.dispose = effect(() => {
    const sipData = callComponent(interp, component, ctx);
    if (sipData == null) return;

    const clone = component.target.cloneNode(false) as Element;
    const sipNode = renderSip(sipData, interp, ctx);
    clone.appendChild(sipNode);

    const activeElement = document.activeElement;
    morphdom(component.target, clone, {
      childrenOnly: true,
      onBeforeElUpdated(fromEl, toEl) {
        // Preserve focus and cursor position in active input elements
        if (
          fromEl === activeElement &&
          (fromEl.tagName === "INPUT" || fromEl.tagName === "TEXTAREA" || fromEl.tagName === "SELECT")
        ) {
          for (const attr of Array.from(toEl.attributes)) {
            if (attr.name !== "value") fromEl.setAttribute(attr.name, attr.value);
          }
          return false;
        }
        return true;
      },
      onNodeDiscarded(node) {
        releaseHandlesForSubtree(node, ctx);
      },
    });
  });
}

/**
 * Find the currently rendering mounted component.
 */
function getActiveComponent(ctx: SemaWebContext): MountedComponent | null {
  const componentId = ctx.renderContextStack[ctx.renderContextStack.length - 1];
  return componentId != null ? ctx.mountedComponentsById.get(componentId) ?? null : null;
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
    ctx.streams.delete(signalId);
  }
}

function cleanupListener(ctx: SemaWebContext, key: string): void {
  const registration = ctx.listeners.get(key);
  if (!registration) return;
  registration.target.removeEventListener(registration.event, registration.listener);
  releaseCallback(registration.callback);
  ctx.listeners.delete(key);
}

function destroyMountedComponent(selector: string, component: MountedComponent, ctx: SemaWebContext): void {
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

/**
 * Delegated event handler that intercepts events on the mount target
 * and dispatches to Sema callback functions via `data-sema-on-*` attributes.
 */
class EventDelegator {
  setup(
    component: MountedComponent,
    target: Element,
    interp: SemaInterpreterLike,
    ctx: SemaWebContext,
  ): () => void {
    const bubbling = [
      "click", "dblclick", "contextmenu", "input", "change", "submit",
      "keydown", "keyup", "keypress", "pointerdown", "pointerup", "pointermove",
      "focusin", "focusout",
    ];
    const listeners: Array<{ event: string; listener: EventListener }> = [];

    for (const event of bubbling) {
      const attr = `data-sema-on-${event}`;
      const listener = (ev: Event) => {
        const start = ev.target;
        if (!(start instanceof Element) || !target.contains(start)) {
          return;
        }

        let el: Element | null = start;
        while (el) {
          if (el.hasAttribute(attr)) {
            const fn = el.getAttribute(attr)!;
            if (SEMA_IDENT_RE.test(fn)) {
              withOwnerContext(ctx, component.instanceId, () => {
                this.dispatchEvent(interp, ctx, fn, ev);
              });
              // Stop walking up if the handler called stopPropagation
              if (ev.cancelBubble || (ev as any).__sema_stop) break;
            }
          }
          if (el === target) break;
          el = el.parentElement;
        }
      };
      target.addEventListener(event, listener);
      listeners.push({ event, listener });
    }

    // mouseenter via mouseover + relatedTarget
    const mouseOverListener = (ev: Event) => {
      const mev = ev as MouseEvent;
      const el = (mev.target as Element).closest?.("[data-sema-on-mouseenter]");
      if (!el || el.contains(mev.relatedTarget as Node)) return;
      withOwnerContext(ctx, component.instanceId, () => {
        this.dispatchEvent(interp, ctx, el.getAttribute("data-sema-on-mouseenter")!, mev);
      });
    };
    target.addEventListener("mouseover", mouseOverListener);
    listeners.push({ event: "mouseover", listener: mouseOverListener });

    const mouseOutListener = (ev: Event) => {
      const mev = ev as MouseEvent;
      const el = (mev.target as Element).closest?.("[data-sema-on-mouseleave]");
      if (!el || el.contains(mev.relatedTarget as Node)) return;
      withOwnerContext(ctx, component.instanceId, () => {
        this.dispatchEvent(interp, ctx, el.getAttribute("data-sema-on-mouseleave")!, mev);
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

  private dispatchEvent(interp: SemaInterpreterLike, ctx: SemaWebContext, callbackName: string, ev: Event) {
    const evHandle = storeHandle(ev, ctx);
    try {
      interp.invokeGlobal(callbackName, evHandle);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `event:${ev.type}:${callbackName}`);
    } finally {
      if (evHandle != null) releaseHandle(evHandle, ctx);
    }
  }
}

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
    (selector: string, componentFn: string) => {
      // Validate component function name
      if (!SEMA_IDENT_RE.test(componentFn)) {
        throw new Error(`Invalid component function name: ${componentFn}`);
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
        dispose: null,
        eventCleanup: null,
        localState: new Map(),
        mountCleanup: null,
        pendingMount: null,
        ownedSignalIds: new Set(),
        ownedWatchIds: new Set(),
        ownedIntervalIds: new Set(),
        ownedStreamIds: new Set(),
        ownedListenerKeys: new Set(),
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
            // Store cleanup function to call on unmount
            const finalCleanupFn = cleanupFn as SemaCallback;
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

  // __component/local — component-scoped state (name-based, no call-order dependency)
  interp.registerFunction("__component/local", (name: string, initialValue: any) => {
    const stack = ctx.renderContextStack;
    if (stack.length === 0) {
      throw new Error("(local) called outside of a component render context");
    }
    const component = getActiveComponent(ctx);
    if (!component) {
      throw new Error("(local) no active mounted component");
    }

    // Key by name within this component's local state
    const existingId = component.localState.get(name);
    if (existingId != null) {
      return existingId;
    }

    // First call: create a new signal
    const id = ctx.nextSignalId++;
    const s = signal(initialValue);
    ctx.signals.set(id, s);
    component.localState.set(name, id);
    return id;
  });

  // __component/on-mount — register lifecycle callback, called once after first render
  interp.registerFunction("__component/on-mount", (callbackValue: any) => {
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

  // js/set-interval — browser setInterval wrapper
  interp.registerFunction("js/set-interval", (callbackValue: any, ms: number) => {
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

  // js/clear-interval — browser clearInterval wrapper
  interp.registerFunction("js/clear-interval", (id: number) => {
    cleanupInterval(ctx, id);
    for (const component of ctx.mountedComponents.values()) {
      component.ownedIntervalIds.delete(id);
    }
    return null;
  });

  // --- Sema-side convenience wrappers ---
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

export function disposeAllComponents(ctx: SemaWebContext): void {
  for (const [selector, component] of Array.from(ctx.mountedComponents.entries())) {
    try {
      destroyMountedComponent(selector, component, ctx);
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `component-dispose-all:${component.componentFn}`);
    }
  }
}
