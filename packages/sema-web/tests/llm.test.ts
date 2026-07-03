import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerLlmBindings } from "../src/llm.js";
import { SemaWebContext, disposeContextResources } from "../src/context.js";
import { createMockInterpreter } from "./helpers.js";

function makeSseResponse(chunks: string[]): Response {
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
    },
  );
}

async function flushAsyncWork(): Promise<void> {
  await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("registerLlmBindings", () => {
  let interp: ReturnType<typeof createMockInterpreter>;
  let ctx: SemaWebContext;

  beforeEach(() => {
    interp = createMockInterpreter();
    ctx = new SemaWebContext();
    registerLlmBindings(interp, { url: "https://proxy.example.com/llm", token: "abc" }, ctx);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("consumes the normalized proxy stream protocol", async () => {
    const fetchMock = vi.fn().mockResolvedValue(makeSseResponse([
      'data: {"type":"token","text":"Hello"}\n\n',
      'data: {"type":"token","text":" world"}\n\n',
      'data: {"type":"done"}\n\n',
    ]));
    vi.stubGlobal("fetch", fetchMock);

    const signalId = interp.getFunction("__llm/chat-stream-raw")!(
      JSON.stringify([{ role: "user", content: "hi" }]),
      JSON.stringify({ model: "gpt-4o" }),
    );

    await flushAsyncWork();

    expect(fetchMock).toHaveBeenCalledWith(
      "https://proxy.example.com/llm/stream",
      expect.objectContaining({
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: "Bearer abc",
        },
      }),
    );
    expect(ctx.signals.get(signalId)?.value).toEqual({
      text: "Hello world",
      done: true,
      error: null,
    });
  });

  it("aborts llm streams on close/dispose", async () => {
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

    const signalId = interp.getFunction("__llm/chat-stream-raw")!(
      JSON.stringify([{ role: "user", content: "hi" }]),
      JSON.stringify({}),
    );

    interp.getFunction("__llm/close-stream")!(signalId);
    await flushAsyncWork();
    expect(aborted).toBe(true);

    const signalId2 = interp.getFunction("__llm/chat-stream-raw")!(
      JSON.stringify([{ role: "user", content: "hi" }]),
      JSON.stringify({}),
    );
    disposeContextResources(ctx);
    await flushAsyncWork();
    expect(ctx.streams.has(signalId2)).toBe(false);
  });

  it("assigns llm stream ownership from the current execution context", () => {
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
    const signalId = interp.getFunction("__llm/chat-stream-raw")!(
      JSON.stringify([{ role: "user", content: "hi" }]),
      JSON.stringify({}),
    );
    ctx.ownerStack.pop();

    expect(component.ownedStreamIds.has(signalId)).toBe(true);
  });
});
