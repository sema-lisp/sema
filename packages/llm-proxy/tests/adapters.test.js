import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { createVercelHandler } from "../dist/adapters/vercel.js";
import { createCloudflareHandler } from "../dist/adapters/cloudflare.js";
import { createNetlifyHandler } from "../dist/adapters/netlify.js";
import { createNodeHandler } from "../dist/adapters/node.js";

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
});

function mockStreamingFetch(chunks) {
  globalThis.fetch = async () => new Response(
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

function makeStreamRequest(url) {
  return new Request(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      messages: [{ role: "user", content: "hello" }],
    }),
  });
}

function createNodeResponseRecorder() {
  return {
    status: null,
    headers: null,
    body: "",
    writeHead(status, headers) {
      this.status = status;
      this.headers = headers;
    },
    write(chunk) {
      this.body += chunk instanceof Uint8Array ? new TextDecoder().decode(chunk) : chunk;
      return true;
    },
    end(body = "") {
      this.body += body;
    },
  };
}

function emitNodeBody(req, body) {
  const bytes = new TextEncoder().encode(body);
  req._data?.(bytes);
  req._end?.();
}

test("Vercel adapter preserves streaming response bodies", async () => {
  mockStreamingFetch(["data: first\n\n", "data: second\n\n"]);
  const handler = createVercelHandler({ provider: "openai", apiKey: "test-key" });

  const response = await handler.POST(makeStreamRequest("https://example.com/api/llm/stream"));

  assert.equal(response.status, 200);
  assert.equal(response.headers.get("content-type"), "text/event-stream");
  assert.equal(
    await response.text(),
    'data: {"type":"token","text":"first"}\n\n'
      + 'data: {"type":"token","text":"second"}\n\n'
      + 'data: {"type":"done"}\n\n',
  );
});

test("Cloudflare adapter preserves streaming response bodies", async () => {
  mockStreamingFetch(["data: cloud\n\n"]);
  const worker = createCloudflareHandler({ provider: "openai", apiKey: "test-key" });

  const response = await worker.fetch(makeStreamRequest("https://example.com/api/llm/stream"));

  assert.equal(response.status, 200);
  assert.equal(response.headers.get("content-type"), "text/event-stream");
  assert.equal(
    await response.text(),
    'data: {"type":"token","text":"cloud"}\n\n'
      + 'data: {"type":"done"}\n\n',
  );
});

test("Netlify adapter preserves streaming response bodies", async () => {
  mockStreamingFetch(["data: netlify\n\n"]);
  const handler = createNetlifyHandler({ provider: "openai", apiKey: "test-key" });

  const response = await handler(makeStreamRequest("https://example.com/api/llm/stream"));

  assert.equal(response.status, 200);
  assert.equal(response.headers.get("content-type"), "text/event-stream");
  assert.equal(
    await response.text(),
    'data: {"type":"token","text":"netlify"}\n\n'
      + 'data: {"type":"done"}\n\n',
  );
});

test("Vercel adapter returns a structured 400 for invalid JSON", async () => {
  const handler = createVercelHandler({ provider: "openai", apiKey: "test-key" });
  const response = await handler.POST(new Request("https://example.com/api/llm/chat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: "{",
  }));

  assert.equal(response.status, 400);
  assert.deepEqual(await response.json(), {
    error: "Invalid JSON body",
    code: "INVALID_REQUEST",
  });
});

test("Vercel adapter rejects oversized request bodies before JSON parsing", async () => {
  const handler = createVercelHandler({
    provider: "openai",
    apiKey: "test-key",
    maxBodySize: 10,
  });

  const response = await handler.POST(new Request("https://example.com/api/llm/chat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ messages: [{ role: "user", content: "hello world" }] }),
  }));

  assert.equal(response.status, 413);
  assert.equal((await response.json()).code, "BODY_TOO_LARGE");
});

test("preflight reflects requested custom headers", async () => {
  const handler = createVercelHandler({ provider: "openai", apiKey: "test-key" });
  const response = await handler.OPTIONS(new Request("https://example.com/api/llm/chat", {
    method: "OPTIONS",
    headers: {
      "Access-Control-Request-Headers": "x-session-id, x-trace-id",
    },
  }));

  assert.equal(response.status, 204);
  assert.equal(
    response.headers.get("access-control-allow-headers"),
    "Content-Type, Authorization, x-session-id, x-trace-id",
  );
});

// --- Cloudflare / Netlify adapter parity ---
//
// Cloudflare and Netlify both wrap the same `createHandler` core and the same
// `readRequestTextWithLimit` / `buildBodyTooLargeResponse` / `getMaxBodySize`
// helpers from body.ts as Vercel (see src/adapters/{vercel,cloudflare,netlify}.ts —
// the request-parsing bodies are effectively identical). These tests mirror the
// three Vercel-only cases above to confirm that shared logic actually behaves
// the same across adapters.

test("Cloudflare adapter returns a structured 400 for invalid JSON", async () => {
  const worker = createCloudflareHandler({ provider: "openai", apiKey: "test-key" });
  const response = await worker.fetch(new Request("https://example.com/api/llm/chat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: "{",
  }));

  assert.equal(response.status, 400);
  assert.deepEqual(await response.json(), {
    error: "Invalid JSON body",
    code: "INVALID_REQUEST",
  });
});

test("Cloudflare adapter rejects oversized request bodies before JSON parsing", async () => {
  const worker = createCloudflareHandler({
    provider: "openai",
    apiKey: "test-key",
    maxBodySize: 10,
  });

  const response = await worker.fetch(new Request("https://example.com/api/llm/chat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ messages: [{ role: "user", content: "hello world" }] }),
  }));

  assert.equal(response.status, 413);
  assert.equal((await response.json()).code, "BODY_TOO_LARGE");
});

test("Cloudflare adapter preflight reflects requested custom headers", async () => {
  const worker = createCloudflareHandler({ provider: "openai", apiKey: "test-key" });
  const response = await worker.fetch(new Request("https://example.com/api/llm/chat", {
    method: "OPTIONS",
    headers: {
      "Access-Control-Request-Headers": "x-session-id, x-trace-id",
    },
  }));

  assert.equal(response.status, 204);
  assert.equal(
    response.headers.get("access-control-allow-headers"),
    "Content-Type, Authorization, x-session-id, x-trace-id",
  );
});

test("Netlify adapter returns a structured 400 for invalid JSON", async () => {
  const handler = createNetlifyHandler({ provider: "openai", apiKey: "test-key" });
  const response = await handler(new Request("https://example.com/api/llm/chat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: "{",
  }));

  assert.equal(response.status, 400);
  assert.deepEqual(await response.json(), {
    error: "Invalid JSON body",
    code: "INVALID_REQUEST",
  });
});

test("Netlify adapter rejects oversized request bodies before JSON parsing", async () => {
  const handler = createNetlifyHandler({
    provider: "openai",
    apiKey: "test-key",
    maxBodySize: 10,
  });

  const response = await handler(new Request("https://example.com/api/llm/chat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ messages: [{ role: "user", content: "hello world" }] }),
  }));

  assert.equal(response.status, 413);
  assert.equal((await response.json()).code, "BODY_TOO_LARGE");
});

test("Netlify adapter preflight reflects requested custom headers", async () => {
  const handler = createNetlifyHandler({ provider: "openai", apiKey: "test-key" });
  const response = await handler(new Request("https://example.com/api/llm/chat", {
    method: "OPTIONS",
    headers: {
      "Access-Control-Request-Headers": "x-session-id, x-trace-id",
    },
  }));

  assert.equal(response.status, 204);
  assert.equal(
    response.headers.get("access-control-allow-headers"),
    "Content-Type, Authorization, x-session-id, x-trace-id",
  );
});

test("Node adapter does not trust forwarded IP headers by default", async () => {
  const handler = createNodeHandler({
    provider: "openai",
    apiKey: "test-key",
    rateLimit: { windowMs: 60_000, maxRequests: 1 },
  });

  const req1 = {
    method: "GET",
    url: "/api/llm/unknown",
    headers: { "x-forwarded-for": "1.1.1.1" },
    on() {},
  };
  const res1 = createNodeResponseRecorder();
  handler(req1, res1);

  const req2 = {
    method: "GET",
    url: "/api/llm/unknown",
    headers: { "x-forwarded-for": "2.2.2.2" },
    on() {},
  };
  const res2 = createNodeResponseRecorder();
  handler(req2, res2);

  await new Promise((resolve) => setTimeout(resolve, 0));

  assert.equal(res1.status, 404);
  assert.equal(res2.status, 429);
  assert.equal(JSON.parse(res2.body).code, "RATE_LIMITED");
});

test("Node adapter rejects oversized request bodies before parsing JSON", async () => {
  const handler = createNodeHandler({
    provider: "openai",
    apiKey: "test-key",
    maxBodySize: 10,
  });

  const listeners = {};
  const req = {
    method: "POST",
    url: "/api/llm/chat",
    headers: { "content-length": "50" },
    on(event, listener) {
      listeners[event] = listener;
      this[`_${event}`] = listener;
    },
  };
  const res = createNodeResponseRecorder();
  handler(req, res);

  emitNodeBody(req, JSON.stringify({ messages: [{ role: "user", content: "hello world" }] }));
  await new Promise((resolve) => setTimeout(resolve, 0));

  assert.equal(res.status, 413);
  assert.equal(JSON.parse(res.body).code, "BODY_TOO_LARGE");
});
