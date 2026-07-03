import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { createHandler } from "../dist/handler.js";

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
});

test("handler rejects invalid request bodies before provider fetch", async () => {
  let fetchCalls = 0;
  globalThis.fetch = async () => {
    fetchCalls += 1;
    throw new Error("fetch should not be called");
  };

  const handler = createHandler({ provider: "openai", apiKey: "test-key" });
  const response = await handler({
    method: "POST",
    endpoint: "chat",
    body: { nope: true },
    authHeader: null,
    clientId: "client-a",
  });

  assert.equal(response.status, 400);
  assert.equal(fetchCalls, 0);

  const payload = JSON.parse(response.body);
  assert.equal(payload.code, "INVALID_REQUEST");
  assert.match(payload.error, /Invalid chat request body/);
});

test("handler enforces body limits using UTF-8 byte length", async () => {
  const handler = createHandler({
    provider: "openai",
    apiKey: "test-key",
    maxBodySize: 20,
  });

  const response = await handler({
    method: "POST",
    endpoint: "complete",
    body: { prompt: "😀😀😀😀😀" },
    authHeader: null,
    clientId: "client-a",
  });

  assert.equal(response.status, 413);
  const payload = JSON.parse(response.body);
  assert.equal(payload.code, "BODY_TOO_LARGE");
  assert.match(payload.error, /Request body too large/);
});

test("rate limiting uses client identity when auth is absent", async () => {
  const handler = createHandler({
    provider: "openai",
    apiKey: "test-key",
    rateLimit: { windowMs: 60_000, maxRequests: 1 },
  });

  const first = await handler({
    method: "GET",
    endpoint: "unknown",
    body: null,
    authHeader: null,
    clientId: "client-a",
  });
  const second = await handler({
    method: "GET",
    endpoint: "unknown",
    body: null,
    authHeader: null,
    clientId: "client-b",
  });
  const third = await handler({
    method: "GET",
    endpoint: "unknown",
    body: null,
    authHeader: null,
    clientId: "client-a",
  });

  assert.equal(first.status, 404);
  assert.equal(second.status, 404);
  assert.equal(third.status, 429);

  const payload = JSON.parse(third.body);
  assert.equal(payload.code, "RATE_LIMITED");
});

test("stream endpoint normalizes provider SSE payloads", async () => {
  globalThis.fetch = async () => new Response(
    new ReadableStream({
      start(controller) {
        controller.enqueue(new TextEncoder().encode('data: {"choices":[{"delta":{"content":"Hello"}}]}\n\n'));
        controller.enqueue(new TextEncoder().encode('data: {"choices":[{"delta":{"content":" world"}}]}\n\n'));
        controller.enqueue(new TextEncoder().encode("data: [DONE]\n\n"));
        controller.close();
      },
    }),
    {
      status: 200,
      headers: { "Content-Type": "text/event-stream" },
    },
  );

  const handler = createHandler({ provider: "openai", apiKey: "test-key" });
  const response = await handler({
    method: "POST",
    endpoint: "stream",
    body: {
      messages: [{ role: "user", content: "hello" }],
    },
    authHeader: null,
    clientId: "client-a",
  });

  assert.equal(response.status, 200);
  assert.equal(response.headers["Content-Type"], "text/event-stream");
  assert.ok(response.stream);
  const text = await new Response(response.stream).text();
  assert.equal(
    text,
    'data: {"type":"token","text":"Hello"}\n\n'
      + 'data: {"type":"token","text":" world"}\n\n'
      + 'data: {"type":"done"}\n\n',
  );
});

test("handler returns TIMEOUT when the upstream provider stalls", async () => {
  globalThis.fetch = (_url, init = {}) =>
    new Promise((_resolve, reject) => {
      init.signal?.addEventListener("abort", () => {
        reject(new Error("aborted"));
      });
    });

  const handler = createHandler({
    provider: "openai",
    apiKey: "test-key",
    upstreamTimeoutMs: 5,
  });

  const response = await handler({
    method: "POST",
    endpoint: "chat",
    body: {
      messages: [{ role: "user", content: "hello" }],
    },
    authHeader: null,
    clientId: "client-a",
  });

  assert.equal(response.status, 504);
  assert.deepEqual(JSON.parse(response.body), {
    error: "Upstream provider timed out after 5ms",
    code: "TIMEOUT",
  });
});

test("stream endpoint emits an error event when the upstream stream goes idle", async () => {
  globalThis.fetch = async () => new Response(
    new ReadableStream({
      start() {
        // Intentionally never enqueue or close.
      },
    }),
    {
      status: 200,
      headers: { "Content-Type": "text/event-stream" },
    },
  );

  const handler = createHandler({
    provider: "openai",
    apiKey: "test-key",
    upstreamTimeoutMs: 5,
  });

  const response = await handler({
    method: "POST",
    endpoint: "stream",
    body: {
      messages: [{ role: "user", content: "hello" }],
    },
    authHeader: null,
    clientId: "client-a",
  });

  assert.equal(response.status, 200);
  assert.ok(response.stream);
  assert.equal(
    await new Response(response.stream).text(),
    'data: {"type":"error","error":"Upstream provider timed out after 5ms"}\n\n',
  );
});
