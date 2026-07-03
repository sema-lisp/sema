import { afterEach, describe, expect, it, vi } from "vitest";
import { openSseStream, type SseEvent } from "../src/sse.js";

function sseResponse(chunks: string[], init?: ResponseInit): Response {
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

describe("openSseStream", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("parses comments, CRLF chunks, multiline data, ids, event names, and retry fields", async () => {
    const events: SseEvent[] = [];
    const onOpen = vi.fn();
    const onClose = vi.fn();
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(
      sseResponse([
        ": keepalive\r\n",
        "event: token\r\nid: 7\r\nretry: 2500\r\n",
        "data: hello\r\n",
        "data: world\r\n\r\n",
        "unknown: ignored\r\n",
        "data: final-without-blank",
      ]),
    ));

    const stream = openSseStream({
      url: "/events",
      onOpen,
      onClose,
      onEvent: (event) => events.push(event),
    });
    await stream.done;

    expect(onOpen).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
    expect(events).toEqual([
      { data: "hello\nworld", event: "token", id: "7", retry: 2500 },
      { data: "final-without-blank", event: null, id: null, retry: null },
    ]);
  });

  it("uses GET by default, POST when a body is present, and forwards request options", async () => {
    const fetchMock = vi.fn().mockResolvedValue(sseResponse([]));
    vi.stubGlobal("fetch", fetchMock);

    await openSseStream({ url: "/get", onEvent: () => {} }).done;
    await openSseStream({
      url: "/post",
      headers: { authorization: "Bearer token" },
      body: "payload",
      credentials: "include",
      onEvent: () => {},
    }).done;

    expect(fetchMock).toHaveBeenNthCalledWith(1, "/get", expect.objectContaining({
      method: "GET",
      body: undefined,
      credentials: undefined,
    }));
    expect(fetchMock).toHaveBeenNthCalledWith(2, "/post", expect.objectContaining({
      method: "POST",
      body: "payload",
      credentials: "include",
      headers: { authorization: "Bearer token" },
    }));
  });

  it("reports HTTP errors and still calls onClose", async () => {
    const onError = vi.fn();
    const onClose = vi.fn();
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    await openSseStream({
      url: "/down",
      onEvent: () => {},
      onError,
      onClose,
    }).done;

    expect(onError).toHaveBeenCalledWith(expect.objectContaining({ message: "HTTP 503" }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("treats invalid retry values as null instead of carrying stale retry state", async () => {
    const events: SseEvent[] = [];
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(
      sseResponse([
        "retry: abc\n",
        "data: payload\n\n",
      ]),
    ));

    await openSseStream({
      url: "/events",
      onEvent: (event) => events.push(event),
    }).done;

    expect(events).toEqual([{ data: "payload", event: null, id: null, retry: null }]);
  });

  it("emits final event fields even when the stream ends without a blank line", async () => {
    const events: SseEvent[] = [];
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(
      sseResponse([
        "event: done\n",
        "id: final-id\n",
        "retry: 99\n",
        "data: payload",
      ]),
    ));

    await openSseStream({
      url: "/events",
      onEvent: (event) => events.push(event),
    }).done;

    expect(events).toEqual([
      { data: "payload", event: "done", id: "final-id", retry: 99 },
    ]);
  });

  it.each([
    ["event: done", { data: "", event: "done", id: null, retry: null }],
    ["id: final-id", { data: "", event: null, id: "final-id", retry: null }],
    ["retry: 99", { data: "", event: null, id: null, retry: 99 }],
  ])("emits a final metadata-only line without a trailing newline: %s", async (line, expected) => {
    const events: SseEvent[] = [];
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(sseResponse([line])));

    await openSseStream({
      url: "/events",
      onEvent: (event) => events.push(event),
    }).done;

    expect(events).toEqual([expected]);
  });

  it("external aborts suppress onError and unregister the abort forwarder", async () => {
    const controller = new AbortController();
    const onError = vi.fn();
    const onClose = vi.fn();
    const removeSpy = vi.spyOn(controller.signal, "removeEventListener");

    vi.stubGlobal("fetch", vi.fn().mockImplementation((_url, init?: RequestInit) =>
      new Promise((_resolve, reject) => {
        init?.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
      }),
    ));

    const stream = openSseStream({
      url: "/events",
      signal: controller.signal,
      onEvent: () => {},
      onError,
      onClose,
    });
    controller.abort("stop");
    await stream.done;

    expect(onError).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalledOnce();
    expect(removeSpy).toHaveBeenCalledWith("abort", expect.any(Function));
  });

  it("already-aborted external signals never report fetch aborts as stream errors", async () => {
    const controller = new AbortController();
    controller.abort("already stopped");
    const onError = vi.fn();
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new DOMException("aborted", "AbortError")));

    await openSseStream({
      url: "/events",
      signal: controller.signal,
      onEvent: () => {},
      onError,
    }).done;

    expect(onError).not.toHaveBeenCalled();
  });
});
