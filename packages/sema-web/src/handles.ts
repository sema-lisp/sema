/**
 * Shared element handle system for Sema web bindings.
 *
 * Provides a numeric handle map so Sema can reference DOM elements,
 * text nodes, and events by numeric ID across the WASM boundary.
 *
 * Functions accept an optional SemaWebContext parameter. When provided,
 * state is stored in the context (enabling multi-instance isolation).
 * When omitted, a module-level fallback is used for backward compatibility
 * with modules not yet migrated to context-based usage (dom.ts, component.ts).
 *
 * @module
 */

import type { SemaWebContext } from "./context.js";

/**
 * Regex pattern for valid Sema identifier names.
 * Used to validate callback names and prevent code injection via evalStr.
 */
export const SEMA_IDENT_RE = /^[a-zA-Z_][a-zA-Z0-9_/\-?!*><=+.]*$/;

// --- Module-level fallback state (for backward compatibility) ---
// These will be removed once all modules are migrated to context-based usage.
const _handles = new Map<number, Element | Text | Event>();
const _handleIds = new WeakMap<Element | Text | Event, number>();
let _nextHandle = 1;

/** Get the handles map from ctx or the module-level fallback. */
function getHandles(ctx?: SemaWebContext): Map<number, Element | Text | Event> {
  return ctx ? ctx.handles : _handles;
}

/** Get the reverse object-to-handle map from ctx or the module-level fallback. */
function getHandleIds(ctx?: SemaWebContext): WeakMap<Element | Text | Event, number> {
  return ctx ? ctx.handleIds : _handleIds;
}

/** Get and increment the next handle ID. */
function allocHandle(ctx?: SemaWebContext): number {
  if (ctx) {
    return ctx.nextHandle++;
  }
  return _nextHandle++;
}

/**
 * Store a DOM object and return its numeric handle.
 * Returns null if the object is null/undefined.
 */
export function storeHandle(obj: Element | Text | Event | null, ctx?: SemaWebContext): number | null {
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

/**
 * Retrieve an Element by handle ID.
 * @throws Error if handle is invalid or not an Element
 */
export function getElement(id: number, ctx?: SemaWebContext): Element {
  const el = getHandles(ctx).get(id);
  if (!el || !(el instanceof Element)) {
    throw new Error(`Invalid element handle: ${id}`);
  }
  return el;
}

/**
 * Retrieve an Element or Text node by handle ID.
 * @throws Error if handle is invalid
 */
export function getNode(id: number, ctx?: SemaWebContext): Element | Text {
  const node = getHandles(ctx).get(id);
  if (!node || (!(node instanceof Element) && !(node instanceof Text))) {
    throw new Error(`Invalid node handle: ${id}`);
  }
  return node;
}

/**
 * Retrieve an Event by handle ID.
 * @throws Error if handle is invalid or not an Event
 */
export function getEvent(id: number, ctx?: SemaWebContext): Event {
  const ev = getHandles(ctx).get(id);
  if (!ev || !(ev instanceof Event)) {
    throw new Error(`Invalid event handle: ${id}`);
  }
  return ev;
}

/**
 * Release a handle, freeing the reference to the DOM object.
 * No-op if the handle does not exist.
 */
export function releaseHandle(id: number, ctx?: SemaWebContext): void {
  const handles = getHandles(ctx);
  const value = handles.get(id);
  if (value) {
    getHandleIds(ctx).delete(value);
  }
  handles.delete(id);
}

/**
 * Release all element/text handles that point to a node inside the given subtree.
 */
export function releaseHandlesForSubtree(root: Node, ctx?: SemaWebContext): void {
  const handles = getHandles(ctx);
  const handleIds = getHandleIds(ctx);
  for (const [id, value] of handles) {
    if ((value instanceof Element || value instanceof Text) && (value === root || root.contains(value))) {
      handleIds.delete(value);
      handles.delete(id);
    }
  }
}
