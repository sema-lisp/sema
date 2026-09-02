/**
 * Browser WebSocket client bindings for Sema Web.
 *
 * The native `ws/*` client (sema-stdlib) is built on tokio-tungstenite and is
 * excluded from wasm32. This module re-implements the client over the browser's
 * native `WebSocket`, so the same `ws/connect` / `ws/send` / `ws/close` /
 * `ws/connected?` / `ws/listen` code runs in the browser build.
 *
 * Receive model: the browser main thread cannot block for the next frame, so
 * the pull-based `ws/recv` / `ws/recv-timeout` are native-only. Browser code
 * receives via the evented `ws/listen` (dispatching `:on-open` / `:on-message`
 * / `:on-close` / `:on-error`), mirroring how browser SSE and `llm/chat-stream`
 * deliver data. `ws/listen` is registered here as a real function, which
 * overwrites the prelude's native-only `ws/listen` macro binding in the global
 * env — so the compiler treats it as a call in the browser and expands the
 * blocking recv-loop macro only on native.
 *
 * @module
 */

import {
  getCurrentOwnerId,
  registerStream,
  unregisterStream,
  type SemaWebContext,
  type SocketRegistration,
} from "./context.js";
import { toInvokableCallback, releaseCallback, type SemaCallback } from "./callbacks.js";

interface SemaInterpreterLike {
  registerFunction(name: string, fn: (...args: any[]) => any): void;
  invokeGlobal(name: string, ...args: any[]): any;
  evalStr(code: string): { value: string | null; output: string[]; error: string | null };
}

/** Strip leading `:` from Sema keyword-style map keys (recursively). */
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

/** Look up a map value by key, tolerating both `:key` and `key` spellings. */
function mapGet(obj: any, key: string): any {
  if (!obj || typeof obj !== "object") return undefined;
  return `:${key}` in obj ? obj[`:${key}`] : obj[key];
}

/** Drop a socket's registry entry and release the callbacks it holds. */
function forgetSocket(ctx: SemaWebContext, handle: number, reg: SocketRegistration): void {
  ctx.sockets.delete(handle);
  if (reg.streamId !== undefined) unregisterStream(ctx, reg.streamId);
  for (const cb of reg.callbacks) releaseCallback(cb);
  reg.callbacks.length = 0;
}

/**
 * Close a socket, keeping a wired `:on-close` so a client-initiated close
 * still surfaces to the listen handlers (as in the native client). Without one,
 * a cleanup-only handler releases the registration when the close event lands.
 */
function closeSocket(
  ctx: SemaWebContext,
  handle: number,
  reg: SocketRegistration,
  code?: number,
  reason?: string,
): void {
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

function getReg(ctx: SemaWebContext, handle: unknown): SocketRegistration {
  const reg = typeof handle === "number" ? ctx.sockets.get(handle) : undefined;
  if (!reg) throw new Error("ws: invalid or closed connection handle");
  return reg;
}

/** Convert a byte-like Sema value (Uint8Array or number array) to a payload. */
function toBinary(value: any): ArrayBufferView | null {
  if (value instanceof Uint8Array) return value;
  if (ArrayBuffer.isView(value)) return value as ArrayBufferView;
  if (Array.isArray(value) && value.every((n) => typeof n === "number")) {
    return Uint8Array.from(value);
  }
  return null;
}

/** Encode the `ws/send` argument into a WebSocket payload (text or binary). */
function encodeSend(msg: any): string | ArrayBufferView {
  // Explicit framing: {:text s} / {:binary bv} / {:json v}
  if (msg && typeof msg === "object" && !Array.isArray(msg) && !(msg instanceof Uint8Array)) {
    const text = mapGet(msg, "text");
    if (text !== undefined) return String(text);
    const bin = mapGet(msg, "binary");
    if (bin !== undefined) {
      const b = toBinary(bin);
      if (b) return b;
      throw new Error("ws/send: {:binary …} expects a bytevector");
    }
    const json = mapGet(msg, "json");
    if (json !== undefined) return JSON.stringify(stripColonKeys(json));
    // Plain map (no framing key) → JSON text, matching the native client.
    return JSON.stringify(stripColonKeys(msg));
  }
  // Value shorthands: string → text, bytevector → binary.
  if (typeof msg === "string") return msg;
  const bin = toBinary(msg);
  if (bin) return bin;
  // Numbers, booleans, etc. → JSON text.
  return JSON.stringify(stripColonKeys(msg));
}

/**
 * Register the browser `ws/*` WebSocket client.
 */
export function registerWsBindings(interp: SemaInterpreterLike, ctx: SemaWebContext): void {
  // ws/connect: open a connection, returning an opaque numeric handle.
  //
  // Options map is accepted for source-compatibility with the native client;
  // the browser WebSocket API only honors :subprotocols (headers, handshake
  // timeout, and retry tuning are native-only and silently ignored here).
  interp.registerFunction("ws/connect", (url: string, opts?: any) => {
    if (typeof url !== "string" || url.length === 0) {
      throw new Error("ws/connect expects a ws:// or wss:// URL string");
    }
    const subprotocols = opts ? mapGet(opts, "subprotocols") : undefined;
    const socket =
      subprotocols !== undefined
        ? new WebSocket(url, subprotocols as string | string[])
        : new WebSocket(url);
    socket.binaryType = "arraybuffer";
    const handle = ctx.nextSocketId++;
    const reg: SocketRegistration = { socket, callbacks: [] };
    ctx.sockets.set(handle, reg);
    // A socket opened during a component's render or lifecycle hook belongs to
    // that component: unmounting it closes the socket, like `http/event-source`.
    const ownerId = getCurrentOwnerId(ctx);
    const owner = ownerId != null ? ctx.mountedComponentsById.get(ownerId) : undefined;
    if (owner) {
      const streamId = ctx.nextSignalId++;
      reg.streamId = streamId;
      registerStream(ctx, streamId, {
        kind: "websocket",
        close: () => closeSocket(ctx, handle, reg),
      });
      owner.ownedStreamIds.add(streamId);
    }
    return handle;
  });

  // ws/send: text (string), binary (bytevector), JSON (map), or explicit
  // framing {:text …} / {:binary …} / {:json …}.
  interp.registerFunction("ws/send", (handle: unknown, msg: any) => {
    const { socket } = getReg(ctx, handle);
    if (socket.readyState !== WebSocket.OPEN) {
      throw new Error("ws/send: connection is not open");
    }
    socket.send(encodeSend(msg) as any);
    return null;
  });

  // ws/connected?: true only while the socket is in the OPEN state.
  interp.registerFunction("ws/connected?", (handle: unknown) => {
    const reg = typeof handle === "number" ? ctx.sockets.get(handle) : undefined;
    return !!reg && reg.socket.readyState === WebSocket.OPEN;
  });

  // ws/close: close the socket. Stops inbound delivery but preserves onclose so
  // a wired ws/listen :on-close still fires — matching the native client, where
  // a client-initiated close surfaces to the listen loop (recv → nil →
  // on-close). If nothing wired onclose, a cleanup-only handler releases the
  // handle when the close event lands.
  interp.registerFunction("ws/close", (handle: unknown, code?: number, reason?: string) => {
    const reg = typeof handle === "number" ? ctx.sockets.get(handle) : undefined;
    if (!reg) return null;
    closeSocket(ctx, handle as number, reg, code, reason);
    return null;
  });

  // ws/listen: evented receive. Wires the socket's lifecycle to Sema handlers,
  // matching the native macro's contract:
  //   :on-open    (fn (conn) …)
  //   :on-message (fn (conn msg) …)   msg = text string or binary bytevector
  //   :on-close   (fn (conn info) …)  info = {:code … :reason …}
  //   :on-error   (fn (conn err) …)
  //
  // Registered as `__ws/listen` taking the handlers *positionally*, with a Sema
  // wrapper (below) that destructures the map. This is load-bearing: the WASM
  // boundary only converts top-level lambda args into invokable callbacks — a
  // lambda nested inside a map arg is serialized through JSON and lost, so
  // callbacks MUST cross as separate arguments. Returns the connection handle
  // (the native client returns a promise to await; the browser is evented, so
  // there is nothing to await).
  interp.registerFunction(
    "__ws/listen",
    (handle: unknown, onOpenV: any, onMessageV: any, onCloseV: any, onErrorV: any) => {
      const reg = getReg(ctx, handle);
      const wire = (v: any, label: string): SemaCallback | null => {
        if (v === undefined || v === null) return null;
        const cb = toInvokableCallback(v, interp, `ws/listen ${label}`);
        reg.callbacks.push(cb);
        return cb;
      };
      const onOpen = wire(onOpenV, "on-open");
      const onMessage = wire(onMessageV, "on-message");
      const onClose = wire(onCloseV, "on-close");
      const onError = wire(onErrorV, "on-error");
      const { socket } = reg;
      // Every handler runs inside browser event dispatch, where a throw would
      // vanish into the event loop; route it to the app's error handler.
      const guarded = (label: string, run: () => void) => {
        try {
          run();
        } catch (e) {
          ctx.onerror(e instanceof Error ? e : new Error(String(e)), `ws/listen ${label}`);
        }
      };

      if (onOpen) {
        // If already open (connect resolved before listen), fire immediately.
        if (socket.readyState === WebSocket.OPEN) {
          guarded("on-open", () => onOpen(handle));
        } else {
          socket.onopen = () => guarded("on-open", () => onOpen(handle));
        }
      }
      if (onMessage) {
        socket.onmessage = (ev: MessageEvent) => {
          const data = ev.data instanceof ArrayBuffer ? new Uint8Array(ev.data) : ev.data;
          guarded("on-message", () => onMessage(handle, data));
        };
      }
      socket.onclose = (ev: CloseEvent) => {
        try {
          if (onClose) {
            guarded("on-close", () => onClose(handle, { ":code": ev.code, ":reason": ev.reason }));
          }
        } finally {
          // The socket is done; drop it from the registry even if on-close threw.
          forgetSocket(ctx, handle as number, reg);
        }
      };
      if (onError) {
        socket.onerror = () => guarded("on-error", () => onError(handle, "websocket error"));
      }
      return handle;
    },
  );

  // Sema wrapper for ws/listen: pulls the handlers out of the map (where they
  // are still real Sema lambdas) and hands them to __ws/listen as top-level
  // args so each crosses the WASM boundary as an invokable callback. Defining
  // it as a function overwrites the prelude's native-only ws/listen *macro*
  // binding (macros and functions share one env slot), so browser code calling
  // `(ws/listen conn {:on-message …})` reaches this instead of the recv-loop
  // macro.
  // Use the symbol form `(define ws/listen (fn …))` — NOT `(define (ws/listen …))`.
  // In the latter, `ws/listen` sits in call position and the prelude macro
  // expands it before `define` runs ("define: expected a symbol"). As a bare
  // symbol it is never treated as a macro head, so this cleanly rebinds the slot
  // to a function.
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
