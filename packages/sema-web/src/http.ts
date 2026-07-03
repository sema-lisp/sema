/**
 * Browser-specific HTTP bindings for Sema Web.
 *
 * Adds a production-oriented SSE client built on top of `fetch()` streaming
 * rather than the browser `EventSource` constructor so headers, auth, POST
 * streams, and explicit cancellation are supported consistently.
 *
 * @module
 */

import { signal } from "@preact/signals-core";
import type { SemaWebContext } from "./context.js";
import { getCurrentOwnerId } from "./context.js";
import { openSseStream } from "./sse.js";

interface SemaInterpreterLike {
  registerFunction(name: string, fn: (...args: any[]) => any): void;
}

interface HttpStreamOptions {
  url: string;
  method?: string;
  headers?: Record<string, string>;
  body?: string;
  withCredentials?: boolean;
}

interface HttpStreamState {
  data: string | null;
  event: string | null;
  id: string | null;
  retry: number | null;
  done: boolean;
  error: string | null;
  status: number | null;
  state: "connecting" | "open" | "closed";
}

function getActiveComponent(ctx: SemaWebContext) {
  const componentId = getCurrentOwnerId(ctx);
  return componentId != null ? ctx.mountedComponentsById.get(componentId) ?? null : null;
}

function stripColonKeys(obj: any): any {
  if (Array.isArray(obj)) return obj.map(stripColonKeys);
  if (obj && typeof obj === "object") {
    const out: Record<string, any> = {};
    for (const [k, v] of Object.entries(obj)) {
      out[k.startsWith(":") ? k.slice(1) : k] = stripColonKeys(v);
    }
    return out;
  }
  return obj;
}

function normalizeStreamOptions(
  input: string | Record<string, any>,
  maybeOpts?: Record<string, any>,
): HttpStreamOptions {
  if (typeof input === "string") {
    const normalized = stripColonKeys(maybeOpts && typeof maybeOpts === "object" ? maybeOpts : {});
    return {
      url: input,
      method: normalized.method,
      headers: normalized.headers,
      body: normalized.body,
      withCredentials: normalized.withCredentials ?? normalized["with-credentials"],
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
    withCredentials: normalized.withCredentials ?? normalized["with-credentials"],
  };
}

function closeManagedStream(ctx: SemaWebContext, signalId: number): void {
  const stream = ctx.streams.get(signalId);
  if (!stream) return;
  stream.close();
  ctx.streams.delete(signalId);
  for (const component of ctx.mountedComponents.values()) {
    component.ownedStreamIds.delete(signalId);
  }
}

/**
 * Register `http/*` browser-specific namespace functions.
 *
 * Functions registered:
 * - `http/event-source` — open an SSE stream and return a signal ID
 * - `http/close-event-source` — close a stream created by `http/event-source`
 * - `http/close-stream` — alias for `http/close-event-source`
 */
export function registerHttpBindings(interp: SemaInterpreterLike, ctx: SemaWebContext): void {
  interp.registerFunction("http/event-source", (input: string | Record<string, any>, maybeOpts?: Record<string, any>) => {
    const opts = normalizeStreamOptions(input, maybeOpts);
    const id = ctx.nextSignalId++;
    const s = signal<HttpStreamState>({
      data: null,
      event: null,
      id: null,
      retry: null,
      done: false,
      error: null,
      status: null,
      state: "connecting",
    });
    ctx.signals.set(id, s as any);

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
          error: null,
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
          state: "open",
        };
      },
      onError: (error) => {
        s.value = {
          ...s.value,
          done: true,
          error: error.message,
          state: "closed",
        };
      },
      onClose: () => {
        ctx.streams.delete(id);
        s.value = {
          ...s.value,
          done: true,
          state: "closed",
        };
      },
    });

    ctx.streams.set(id, {
      kind: "event-source",
      close: managedStream.close,
    });

    const owner = getActiveComponent(ctx);
    if (owner) owner.ownedStreamIds.add(id);

    return id;
  });

  interp.registerFunction("http/close-event-source", (signalId: number) => {
    closeManagedStream(ctx, signalId);
    const current = ctx.signals.get(signalId) as any;
    if (current) {
      current.value = {
        ...(current.value ?? {}),
        done: true,
        state: "closed",
      };
    }
    return null;
  });

  interp.registerFunction("http/close-stream", (signalId: number) => {
    closeManagedStream(ctx, signalId);
    const current = ctx.signals.get(signalId) as any;
    if (current) {
      current.value = {
        ...(current.value ?? {}),
        done: true,
        state: "closed",
      };
    }
    return null;
  });
}
