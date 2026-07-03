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

/** A mounted component managed by the component system. */
export interface MountedComponent {
  instanceId: number;
  target: Element;
  componentFn: string;
  dispose: (() => void) | null;
  eventCleanup: (() => void) | null;
  localState: Map<string, number>;
  mountCleanup: (() => void) | null;
  pendingMount: unknown;
  ownedSignalIds: Set<number>;
  ownedWatchIds: Set<number>;
  ownedIntervalIds: Set<number>;
  ownedStreamIds: Set<number>;
  ownedListenerKeys: Set<string>;
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
  kind: "event-source" | "llm-stream";
  close: () => void;
}

export interface SocketRegistration {
  socket: WebSocket;
  /** Sema callbacks wired via `ws/listen`, released on close/dispose. */
  callbacks: SemaCallback[];
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

  /** Current execution owner stack for callbacks invoked outside render. */
  ownerStack: number[] = [];

  /** DOM event listeners registry */
  listeners = new Map<string, ListenerRegistration>();

  /** Reactive watch cleanup callbacks */
  watchDisposers = new Map<number, WatchRegistration>();
  nextWatchId = 1;

  /** Browser interval handles */
  intervals = new Map<number, IntervalRegistration>();

  /** Managed streaming resources keyed by signal id */
  streams = new Map<number, StreamRegistration>();

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

  /** Error handler */
  onerror: ErrorHandler = (error, context) => {
    console.error(`[sema-web] Error in ${context}:`, error);
  };
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
}
