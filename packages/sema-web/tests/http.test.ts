import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerHttpBindings } from "../src/http.js";
import { SemaWebContext, disposeContextResources } from "../src/context.js";
import { createMockInterpreter } from "./helpers.js";

function makeSseResponse(chunks: string[], init?: ResponseInit): Response {
  return new Response(
    new ReadableStream({
      start(controller) {
        for (const chunk of chunks) {
          controller.enqueue(new TextEncoder().encode(chunk));
        }
        controller.close();
      },
    }),
    {
      status: 200,
      headers: { "Content-Type": "text/event-stream" },
      ...init,
    },
  );
}

async function flushAsyncWork(): Promise<void> {
  await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("registerHttpBindings", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
    registerHttpBindings(interp, ctx);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("streams SSE over fetch with custom headers and POST bodies", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      makeSseResponse([
        'event: token\n',
        'id: 42\n',
        'data: hello\n\n',
      ]),
    );
    vi.stubGlobal("fetch", fetchMock);

    const signalId = interp.getFunction("http/event-source")!({
      ":url": "/stream",
      ":method": "POST",
      ":headers": { ":authorization": "Bearer token" },
      ":body": "payload",
      ":with-credentials": true,
    });

    await flushAsyncWork();

    expect(fetchMock).toHaveBeenCalledWith("/stream", expect.objectContaining({
      method: "POST",
      body: "payload",
      credentials: "include",
      headers: { authorization: "Bearer token" },
    }));

    const state = ctx.signals.get(signalId)?.value;
    expect(state).toMatchObject({
      data: "hello",
      event: "token",
      id: "42",
      done: true,
      state: "closed",
      error: null,
      status: 200,
    });
  });

  it("accepts string URL plus colon-key options and strips nested header keys", async () => {
    const fetchMock = vi.fn().mockResolvedValue(makeSseResponse(["data: ok\n\n"]));
    vi.stubGlobal("fetch", fetchMock);

    const signalId = interp.getFunction("http/event-source")!("/stream", {
      ":method": "POST",
      ":headers": { ":x-token": "secret" },
      ":body": "payload",
      ":with-credentials": true,
    });
    await flushAsyncWork();

    expect(fetchMock).toHaveBeenCalledWith("/stream", expect.objectContaining({
      method: "POST",
      headers: { "x-token": "secret" },
      credentials: "include",
      body: "payload",
    }));
    expect(ctx.signals.get(signalId)?.value.data).toBe("ok");
  });

  it("throws a clear error for missing or empty event-source URLs", () => {
    expect(() => interp.getFunction("http/event-source")!({})).toThrow(/expected a URL/);
    expect(() => interp.getFunction("http/event-source")!({ ":url": "" })).toThrow(/expected a URL/);
  });

  it("sets error state and removes stream registration when fetch fails", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new Error("network down")));

    const signalId = interp.getFunction("http/event-source")!("/stream");
    expect(ctx.streams.has(signalId)).toBe(true);

    await flushAsyncWork();

    expect(ctx.streams.has(signalId)).toBe(false);
    expect(ctx.signals.get(signalId)?.value).toMatchObject({
      done: true,
      error: "network down",
      state: "closed",
    });
  });

  it("closes managed streams on dispose", async () => {
    let aborted = false;
    vi.stubGlobal("fetch", vi.fn().mockImplementation((_url, init?: RequestInit) => {
      init?.signal?.addEventListener("abort", () => {
        aborted = true;
      });
      return Promise.resolve(new Response(
        new ReadableStream({
          start() {
            // Leave open until aborted.
          },
        }),
        {
          status: 200,
          headers: { "Content-Type": "text/event-stream" },
        },
      ));
    }));

    const signalId = interp.getFunction("http/event-source")!("/stream");
    expect(ctx.streams.has(signalId)).toBe(true);

    disposeContextResources(ctx);
    await flushAsyncWork();

    expect(aborted).toBe(true);
    expect(ctx.streams.has(signalId)).toBe(false);
  });

  it("close aliases mark streams closed, abort fetch, and remove component ownership", async () => {
    let aborted = 0;
    vi.stubGlobal("fetch", vi.fn().mockImplementation((_url, init?: RequestInit) => {
      init?.signal?.addEventListener("abort", () => {
        aborted += 1;
      });
      return Promise.resolve(new Response(
        new ReadableStream({
          start() {
            // Leave open until aborted.
          },
        }),
        {
          status: 200,
          headers: { "Content-Type": "text/event-stream" },
        },
      ));
    }));

    const component = {
      instanceId: 1,
      target: document.createElement("div"),
      componentFn: "view",
      dispose: null,
      eventCleanup: null,
      localState: new Map(),
      mountCleanup: null,
      pendingMount: null,
      ownedSignalIds: new Set<number>(),
      ownedWatchIds: new Set<number>(),
      ownedIntervalIds: new Set<number>(),
      ownedStreamIds: new Set<number>(),
      ownedListenerKeys: new Set<string>(),
    };
    ctx.mountedComponents.set("#app", component as any);
    ctx.mountedComponentsById.set(component.instanceId, component as any);
    ctx.ownerStack.push(component.instanceId);
    const signalA = interp.getFunction("http/event-source")!("/a");
    const signalB = interp.getFunction("http/event-source")!("/b");
    ctx.ownerStack.pop();

    interp.getFunction("http/close-event-source")!(signalA);
    interp.getFunction("http/close-stream")!(signalB);
    interp.getFunction("http/close-stream")!(999);

    expect(aborted).toBe(2);
    expect(ctx.streams.has(signalA)).toBe(false);
    expect(ctx.streams.has(signalB)).toBe(false);
    expect(component.ownedStreamIds.size).toBe(0);
    expect(ctx.signals.get(signalA)?.value).toMatchObject({ done: true, state: "closed" });
    expect(ctx.signals.get(signalB)?.value).toMatchObject({ done: true, state: "closed" });
  });

  it("assigns stream ownership from the current execution context", () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(makeSseResponse([])));

    const component = {
      instanceId: 1,
      target: document.createElement("div"),
      componentFn: "view",
      dispose: null,
      eventCleanup: null,
      localState: new Map(),
      mountCleanup: null,
      pendingMount: null,
      ownedSignalIds: new Set<number>(),
      ownedWatchIds: new Set<number>(),
      ownedIntervalIds: new Set<number>(),
      ownedStreamIds: new Set<number>(),
      ownedListenerKeys: new Set<string>(),
    };

    ctx.mountedComponentsById.set(component.instanceId, component as any);
    ctx.ownerStack.push(component.instanceId);
    const signalId = interp.getFunction("http/event-source")!("/stream");
    ctx.ownerStack.pop();

    expect(component.ownedStreamIds.has(signalId)).toBe(true);
  });
});
