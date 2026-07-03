import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { registerWsBindings } from "../src/ws.js";
import { SemaWebContext, disposeContextResources } from "../src/context.js";
import { createMockInterpreter } from "./helpers.js";

// Minimal WebSocket stand-in — jsdom has no WebSocket. Records sent payloads
// and exposes helpers to drive the browser event callbacks.
class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static instances: MockWebSocket[] = [];

  url: string;
  protocols?: string | string[];
  readyState = MockWebSocket.OPEN; // default open so send/connected? are testable
  binaryType = "blob";
  sent: any[] = [];
  onopen: ((ev?: any) => void) | null = null;
  onmessage: ((ev: any) => void) | null = null;
  onclose: ((ev: any) => void) | null = null;
  onerror: ((ev?: any) => void) | null = null;
  closed = false;
  closeArgs: [number?, string?] | null = null;

  constructor(url: string, protocols?: string | string[]) {
    this.url = url;
    this.protocols = protocols;
    MockWebSocket.instances.push(this);
  }
  send(data: any) {
    this.sent.push(data);
  }
  close(code?: number, reason?: string) {
    this.closed = true;
    this.closeArgs = [code, reason];
    this.readyState = MockWebSocket.CLOSED;
    // Real browsers fire a CloseEvent after close() completes.
    this.onclose?.({ code: code ?? 1000, reason: reason ?? "" });
  }
  // test drivers
  fireMessage(data: any) {
    this.onmessage?.({ data });
  }
  fireClose(code: number, reason: string) {
    this.onclose?.({ code, reason });
  }
}

describe("registerWsBindings", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket as any);
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
    registerWsBindings(interp, ctx);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const connect = (url = "wss://example.test/socket", opts?: any) =>
    interp.getFunction("ws/connect")!(url, opts) as number;
  const sock = () => MockWebSocket.instances[0];

  it("ws/connect returns a numeric handle and opens a socket", () => {
    const h = connect();
    expect(typeof h).toBe("number");
    expect(MockWebSocket.instances).toHaveLength(1);
    expect(sock().url).toBe("wss://example.test/socket");
    expect(sock().binaryType).toBe("arraybuffer");
    expect(ctx.sockets.has(h)).toBe(true);
  });

  it("ws/connect passes :subprotocols through", () => {
    connect("wss://x", { ":subprotocols": ["v1.proto"] });
    expect(sock().protocols).toEqual(["v1.proto"]);
  });

  it("ws/connect rejects a non-URL", () => {
    expect(() => connect("")).toThrow(/URL string/);
  });

  it("ws/send sends a string as a text frame", () => {
    const h = connect();
    interp.getFunction("ws/send")!(h, "hello");
    expect(sock().sent).toEqual(["hello"]);
  });

  it("ws/send sends a plain map as JSON text", () => {
    const h = connect();
    interp.getFunction("ws/send")!(h, { ":hello": 1 });
    expect(sock().sent).toEqual(['{"hello":1}']);
  });

  it("ws/send honors explicit {:text}, {:json}, {:binary} framing", () => {
    const h = connect();
    const send = interp.getFunction("ws/send")!;
    send(h, { ":text": "hi" });
    send(h, { ":json": { ":a": 2 } });
    send(h, { ":binary": Uint8Array.from([1, 2, 3]) });
    expect(sock().sent[0]).toBe("hi");
    expect(sock().sent[1]).toBe('{"a":2}');
    expect(sock().sent[2]).toEqual(Uint8Array.from([1, 2, 3]));
  });

  it("ws/send sends a bytevector shorthand as a binary frame", () => {
    const h = connect();
    interp.getFunction("ws/send")!(h, Uint8Array.from([9, 8]));
    expect(sock().sent[0]).toEqual(Uint8Array.from([9, 8]));
  });

  it("ws/send throws when the socket is not open", () => {
    const h = connect();
    sock().readyState = MockWebSocket.CONNECTING;
    expect(() => interp.getFunction("ws/send")!(h, "x")).toThrow(/not open/);
  });

  it("ws/connected? reflects the socket ready state", () => {
    const h = connect();
    expect(interp.getFunction("ws/connected?")!(h)).toBe(true);
    sock().readyState = MockWebSocket.CLOSED;
    expect(interp.getFunction("ws/connected?")!(h)).toBe(false);
    expect(interp.getFunction("ws/connected?")!(9999)).toBe(false);
  });

  it("ws/close closes the socket and releases the handle", () => {
    const h = connect();
    interp.getFunction("ws/close")!(h, 1000, "bye");
    expect(sock().closed).toBe(true);
    expect(sock().closeArgs).toEqual([1000, "bye"]);
    expect(ctx.sockets.has(h)).toBe(false);
  });

  // ws/listen crosses the WASM boundary as __ws/listen with POSITIONAL callback
  // args (on-open, on-message, on-close, on-error) — a Sema wrapper destructures
  // the handlers map. These test the native binding directly.
  const listen = (
    h: number,
    cbs: { open?: any; message?: any; close?: any; error?: any },
  ) =>
    interp.getFunction("__ws/listen")!(
      h,
      cbs.open ?? null,
      cbs.message ?? null,
      cbs.close ?? null,
      cbs.error ?? null,
    );

  it("__ws/listen dispatches incoming text frames to on-message with the handle", () => {
    const h = connect();
    const received: any[] = [];
    listen(h, { message: (conn: number, msg: any) => received.push([conn, msg]) });
    sock().fireMessage("frame-1");
    expect(received).toEqual([[h, "frame-1"]]);
  });

  it("__ws/listen delivers binary frames as a Uint8Array", () => {
    const h = connect();
    let got: any = null;
    listen(h, {
      message: (_c: number, msg: any) => {
        got = msg;
      },
    });
    sock().fireMessage(Uint8Array.from([1, 2, 3]).buffer);
    expect(got).toBeInstanceOf(Uint8Array);
    expect(Array.from(got)).toEqual([1, 2, 3]);
  });

  it("__ws/listen fires on-open immediately when already open", () => {
    const h = connect();
    const opened: number[] = [];
    listen(h, { open: (conn: number) => opened.push(conn) });
    expect(opened).toEqual([h]);
  });

  it("__ws/listen delivers on-close with {:code :reason} and drops the socket", () => {
    const h = connect();
    let info: any = null;
    listen(h, {
      close: (_c: number, i: any) => {
        info = i;
      },
    });
    sock().fireClose(1006, "gone");
    expect(info).toEqual({ ":code": 1006, ":reason": "gone" });
    expect(ctx.sockets.has(h)).toBe(false);
  });

  it("client ws/close still fires a wired on-close (native parity)", () => {
    const h = connect();
    let closedInfo: any = null;
    listen(h, { close: (_c: number, i: any) => { closedInfo = i; } });
    interp.getFunction("ws/close")!(h, 1000, "bye");
    expect(closedInfo).toEqual({ ":code": 1000, ":reason": "bye" });
    expect(ctx.sockets.has(h)).toBe(false);
  });

  it("__ws/listen ignores null (absent) handlers without wiring them", () => {
    const h = connect();
    listen(h, {}); // all null
    // No throw; a message with no on-message handler is simply dropped.
    expect(() => sock().fireMessage("x")).not.toThrow();
  });

  it("disposeContextResources closes open sockets", () => {
    const h = connect();
    disposeContextResources(ctx);
    expect(sock().closed).toBe(true);
    expect(ctx.sockets.size).toBe(0);
    void h;
  });
});
