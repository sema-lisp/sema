/**
 * DOM bindings for Sema — registers `dom/*` namespace functions.
 *
 * These functions provide a thin mirror of the browser DOM API,
 * exposed as Sema functions via the interpreter's registerFunction API.
 *
 * @module
 */

import { storeHandle, getElement, getNode, getEvent, releaseHandle, SEMA_IDENT_RE } from "./handles.js";
import type { SemaWebContext } from "./context.js";
import { getCurrentOwnerId, withOwnerContext } from "./context.js";
import { renderSip } from "./sip.js";
import { toInvokableCallback, releaseCallback } from "./callbacks.js";

interface SemaInterpreterLike {
  registerFunction(name: string, fn: (...args: any[]) => any): void;
  invokeGlobal(name: string, ...args: any[]): any;
  evalStr(code: string): { value: string | null; output: string[]; error: string | null };
}

/**
 * Register all `dom/*` namespace functions on the given interpreter.
 *
 * Functions registered:
 * - `dom/query` — querySelector, returns element or nil
 * - `dom/query-all` — querySelectorAll, returns list of elements
 * - `dom/create-element` — createElement
 * - `dom/create-text` — createTextNode
 * - `dom/append-child!` — appendChild
 * - `dom/remove-child!` — removeChild
 * - `dom/remove!` — remove element from DOM
 * - `dom/set-attribute!` — setAttribute
 * - `dom/get-attribute` — getAttribute
 * - `dom/remove-attribute!` — removeAttribute
 * - `dom/add-class!` — add CSS class(es)
 * - `dom/remove-class!` — remove CSS class(es)
 * - `dom/toggle-class!` — toggle a CSS class
 * - `dom/has-class?` — check if element has a CSS class
 * - `dom/set-style!` — set a style property
 * - `dom/get-style` — get a style property
 * - `dom/set-text!` — set textContent
 * - `dom/get-text` — get textContent
 * - `dom/set-html!` — set innerHTML
 * - `dom/get-html` — get innerHTML
 * - `dom/on!` — addEventListener
 * - `dom/off!` — removeEventListener
 * - `dom/prevent-default!` — event.preventDefault()
 * - `dom/set-value!` — set input value
 * - `dom/get-value` — get input value
 * - `dom/get-id` — get element by id
 * - `dom/render` — render SIP data, return element handle
 * - `dom/render-into!` — render SIP data into a target element
 * - `dom/event-value` — read event.target.value from an event handle
 */
export function registerDomBindings(interp: SemaInterpreterLike, ctx: SemaWebContext): void {

  // --- Query ---

  interp.registerFunction("dom/query", (selector: string) => {
    const el = document.querySelector(selector);
    return storeHandle(el, ctx);
  });

  interp.registerFunction("dom/query-all", (selector: string) => {
    const els = document.querySelectorAll(selector);
    const ids: number[] = [];
    els.forEach((el) => {
      const id = storeHandle(el, ctx);
      if (id != null) ids.push(id);
    });
    return ids;
  });

  interp.registerFunction("dom/get-id", (id: string) => {
    const el = document.getElementById(id);
    return storeHandle(el, ctx);
  });

  // --- Create ---

  interp.registerFunction("dom/create-element", (tag: string) => {
    const el = document.createElement(tag);
    return storeHandle(el, ctx);
  });

  interp.registerFunction("dom/create-text", (content: string) => {
    const node = document.createTextNode(content);
    return storeHandle(node, ctx);
  });

  // --- Tree manipulation ---

  interp.registerFunction("dom/append-child!", (parentId: number, childId: number) => {
    const parent = getElement(parentId, ctx);
    const child = getNode(childId, ctx);
    parent.appendChild(child);
    return childId;
  });

  interp.registerFunction("dom/remove-child!", (parentId: number, childId: number) => {
    const parent = getElement(parentId, ctx);
    const child = getNode(childId, ctx);
    parent.removeChild(child);
    return childId;
  });

  interp.registerFunction("dom/remove!", (id: number) => {
    const el = getElement(id, ctx);
    el.remove();
    return null;
  });

  // --- Attributes ---

  interp.registerFunction("dom/set-attribute!", (id: number, attr: string, val: string) => {
    getElement(id, ctx).setAttribute(attr, val);
    return null;
  });

  interp.registerFunction("dom/get-attribute", (id: number, attr: string) => {
    return getElement(id, ctx).getAttribute(attr);
  });

  interp.registerFunction("dom/remove-attribute!", (id: number, attr: string) => {
    getElement(id, ctx).removeAttribute(attr);
    return null;
  });

  // --- CSS classes ---

  interp.registerFunction("dom/add-class!", (id: number, ...classes: string[]) => {
    getElement(id, ctx).classList.add(...classes);
    return null;
  });

  interp.registerFunction("dom/remove-class!", (id: number, ...classes: string[]) => {
    getElement(id, ctx).classList.remove(...classes);
    return null;
  });

  interp.registerFunction("dom/toggle-class!", (id: number, cls: string) => {
    return getElement(id, ctx).classList.toggle(cls);
  });

  interp.registerFunction("dom/has-class?", (id: number, cls: string) => {
    return getElement(id, ctx).classList.contains(cls);
  });

  // --- Styles ---

  interp.registerFunction("dom/set-style!", (id: number, prop: string, val: string) => {
    const el = getElement(id, ctx) as HTMLElement;
    el.style.setProperty(prop, val);
    return null;
  });

  interp.registerFunction("dom/get-style", (id: number, prop: string) => {
    const el = getElement(id, ctx) as HTMLElement;
    return el.style.getPropertyValue(prop);
  });

  // --- Content ---

  interp.registerFunction("dom/set-text!", (id: number, text: string) => {
    getElement(id, ctx).textContent = text;
    return null;
  });

  interp.registerFunction("dom/get-text", (id: number) => {
    return getElement(id, ctx).textContent;
  });

  interp.registerFunction("dom/set-html!", (id: number, html: string) => {
    getElement(id, ctx).innerHTML = html;
    return null;
  });

  interp.registerFunction("dom/get-html", (id: number) => {
    return getElement(id, ctx).innerHTML;
  });

  // --- Form values ---

  interp.registerFunction("dom/set-value!", (id: number, val: string) => {
    const el = getElement(id, ctx) as HTMLInputElement;
    el.value = val;
    return null;
  });

  interp.registerFunction("dom/get-value", (id: number) => {
    const el = getElement(id, ctx) as HTMLInputElement;
    return el.value;
  });

  // --- Events ---

  interp.registerFunction("dom/on!", (id: number, event: string, callbackValue: any) => {
    const el = getElement(id, ctx);
    const callback = toInvokableCallback(callbackValue, interp, `dom/on! callback for "${event}"`);
    const ownerId = getCurrentOwnerId(ctx);
    const callbackKey =
      typeof callback.__semaCallbackHandle === "number"
        ? `handle:${callback.__semaCallbackHandle}`
        : typeof callbackValue === "string"
          ? `name:${callbackValue}`
          : `fn:${String(callback)}`;
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

    const listener = (ev: Event) => {
      const evHandle = storeHandle(ev, ctx);
      try {
        withOwnerContext(ctx, ownerId, () => callback(evHandle));
    } catch (e) {
      ctx.onerror(e instanceof Error ? e : new Error(String(e)), `event:${event}`);
    } finally {
      // Auto-release event handle
      if (evHandle != null) releaseHandle(evHandle, ctx);
    }
  };

    ctx.listeners.set(key, { target: el, event, listener, callback });
    el.addEventListener(event, listener);
    const owner = ownerId != null ? ctx.mountedComponentsById.get(ownerId) ?? null : null;
    if (owner) owner.ownedListenerKeys.add(key);
    return null;
  });

  interp.registerFunction("dom/off!", (id: number, event: string, callbackValue: any) => {
    const callbackKey =
      typeof callbackValue === "function" && typeof callbackValue.__semaCallbackHandle === "number"
        ? `handle:${callbackValue.__semaCallbackHandle}`
        : typeof callbackValue === "string"
          ? `name:${callbackValue}`
          : `fn:${String(callbackValue)}`;
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

  interp.registerFunction("dom/prevent-default!", (evId: number) => {
    getEvent(evId, ctx).preventDefault();
    return null;
  });

  interp.registerFunction("dom/stop-propagation!", (evId: number) => {
    const ev = getEvent(evId, ctx);
    ev.stopPropagation();
    (ev as any).__sema_stop = true;
    return null;
  });

  // --- Event value ---

  interp.registerFunction("dom/event-value", (evId: number) => {
    const ev = getEvent(evId, ctx);
    const target = (ev as any).target;
    if (target && "value" in target) {
      return target.value;
    }
    return null;
  });

  // --- Event key (for keyboard events) ---

  interp.registerFunction("dom/event-key", (evId: number) => {
    const ev = getEvent(evId, ctx);
    if ("key" in ev) {
      return (ev as KeyboardEvent).key;
    }
    return null;
  });

  // --- Event target (returns element handle) ---

  interp.registerFunction("dom/event-target", (evId: number) => {
    const ev = getEvent(evId, ctx);
    const target = ev.target;
    if (target instanceof Element) {
      return storeHandle(target, ctx);
    }
    return null;
  });

  // --- Event target closest (find closest ancestor matching selector) ---

  interp.registerFunction("dom/event-target-closest", (evId: number, selector: string) => {
    const ev = getEvent(evId, ctx);
    const target = ev.target;
    if (target instanceof Element) {
      const found = target.closest(selector);
      if (found) return storeHandle(found, ctx);
    }
    return null;
  });

  // --- Element focus ---

  interp.registerFunction("dom/focus!", (id: number) => {
    const el = getElement(id, ctx);
    if ("focus" in el) (el as HTMLElement).focus();
    return null;
  });

  // --- SIP rendering ---

  interp.registerFunction("dom/render", (sipData: any) => {
    const node = renderSip(sipData, interp, ctx);
    if (node instanceof Element) {
      return storeHandle(node, ctx);
    }
    // Wrap non-element nodes in a span for handle compatibility
    const wrapper = document.createElement("span");
    wrapper.appendChild(node);
    return storeHandle(wrapper, ctx);
  });

  interp.registerFunction("dom/render-into!", (selector: string, sipData: any) => {
    const target = document.querySelector(selector);
    if (!target) throw new Error(`dom/render-into!: target not found: ${selector}`);
    target.innerHTML = "";
    const node = renderSip(sipData, interp, ctx);
    target.appendChild(node);
    return null;
  });
}
