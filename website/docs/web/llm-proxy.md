# LLM Proxy

The `@sema-lang/llm-proxy` package is a server-side proxy that sits between Sema Web in the browser and LLM providers. It holds your API keys, translates requests into provider-native formats, and streams responses back.

## Why a Proxy?

LLM API keys are secrets. Shipping them in browser JavaScript means anyone can extract them from DevTools and use them at your expense. The proxy keeps keys server-side while exposing a simple, uniform API to the browser.

The proxy also normalizes the differences between providers -- your Sema code does not change when switching from OpenAI to Anthropic to Gemini.

## Installation

```bash
npm install @sema-lang/llm-proxy
```

## Supported Providers

| Provider | Identifier | Default Model | Embeddings |
|----------|-----------|---------------|------------|
| OpenAI | `"openai"` | `gpt-4o-mini` | `text-embedding-3-small` |
| Anthropic | `"anthropic"` | `claude-sonnet-4-20250514` | Not supported |
| Google Gemini | `"gemini"` | `gemini-2.0-flash` | `text-embedding-004` |
| Ollama | `"ollama"` | `llama3.2` | `nomic-embed-text` |
| Groq | `"groq"` | `llama-3.3-70b-versatile` | Not supported |
| Mistral | `"mistral"` | `mistral-small-latest` | `mistral-embed` |
| xAI | `"xai"` | `grok-3-mini` | Not supported |

Groq, Mistral, and xAI use the OpenAI-compatible API format internally.

## Platform Adapters

Each adapter wraps the core handler for a specific deployment platform. They convert platform-specific request/response objects to the internal `ProxyRequest`/`ProxyResponse` format.

### Vercel (App Router)

```ts
// api/llm/[...path].ts
import { createVercelHandler } from "@sema-lang/llm-proxy/vercel";

export const { GET, POST, OPTIONS } = createVercelHandler({
  provider: "openai",
  apiKey: process.env.OPENAI_API_KEY!,
  defaultModel: "gpt-4o",
  cors: "*",
});
```

A plain `export default createVercelHandler({...})` won't work — Vercel's
`api/` directory expects named exports per HTTP method (what the
destructuring above produces) or a `fetch`-shaped default export, not an
object of `{GET, POST, OPTIONS}` methods as the default export itself. Don't
add `export const runtime = "edge"` either — Vercel now deprecates Edge
Functions for new projects; the default Node.js runtime streams natively.

### Netlify (Edge Functions)

```ts
// netlify/edge-functions/llm.ts
import { createNetlifyHandler } from "@sema-lang/llm-proxy/netlify";

export default createNetlifyHandler({
  provider: "anthropic",
  apiKey: Netlify.env.get("ANTHROPIC_API_KEY")!,
  defaultModel: "claude-sonnet-4-20250514",
});

export const config = { path: "/api/llm/*" };
```

Use the `Netlify.env` global to read environment variables in Edge
Functions, not `Deno.env` directly — `Netlify.env` is scoped to variables
you've declared for the Functions scope and is what Netlify's current docs
document for this runtime.

### Cloudflare Workers

```ts
// src/worker.ts
import { createCloudflareHandler } from "@sema-lang/llm-proxy/cloudflare";

export default {
  fetch(request: Request, env: { OPENAI_API_KEY: string }) {
    // Constructed per-request so it can read the env binding, which isn't
    // available at module-eval time in Workers.
    return createCloudflareHandler({
      provider: "openai",
      apiKey: env.OPENAI_API_KEY,
    }).fetch(request);
  },
};
```

### Node.js (Express / standalone)

```ts
import express from "express";
import { createNodeHandler } from "@sema-lang/llm-proxy/node";

const app = express();
const llmHandler = createNodeHandler({
  provider: "openai",
  apiKey: process.env.OPENAI_API_KEY!,
});

app.use("/api/llm", llmHandler);
app.listen(3001);
```

## Configuration Reference

The `ProxyConfig` object is shared across all adapters:

```ts
interface ProxyConfig {
  /** Provider name or full config object. */
  provider: "openai" | "anthropic" | "gemini" | "ollama"
           | "groq" | "mistral" | "xai" | ProviderConfig;

  /** API key (shorthand when provider is a string). */
  apiKey?: string;

  /** Override the provider's base URL. */
  baseUrl?: string;

  /** Default model when request doesn't specify one. */
  defaultModel?: string;

  /** Authentication for incoming browser requests. */
  auth?: AuthConfig;

  /** CORS origin. Default: "*". */
  cors?: string;

  /** Max request body size in bytes. Default: 1MB. */
  maxBodySize?: number;

  /** Rate limiting. */
  rateLimit?: RateLimitConfig;

  /**
   * Whether to trust proxy forwarding headers (`cf-connecting-ip`,
   * `x-forwarded-for`, `x-real-ip`) for client identity, used to key rate
   * limiting per-IP. See "Rate Limiting" below — the default differs by
   * adapter.
   */
  trustProxyHeaders?: boolean | string[];
}
```

### Provider Config (advanced)

When you need full control, pass a `ProviderConfig` object instead of a string:

```ts
createVercelHandler({
  provider: {
    provider: "openai",
    apiKey: process.env.OPENAI_API_KEY!,
    baseUrl: "https://my-azure-openai.openai.azure.com/v1",
    defaultModel: "gpt-4o",
  },
});
```

### Authentication

Protect your proxy from unauthorized access. Two modes are available:

**Shared token** -- simple, good for prototypes:

```ts
createVercelHandler({
  provider: "openai",
  apiKey: process.env.OPENAI_API_KEY!,
  auth: {
    token: process.env.PROXY_SECRET!,
  },
});
```

The browser must send `Authorization: Bearer {token}` on every request. Configure this in `SemaWeb.create()`:

```js
SemaWeb.create({
  llmProxy: {
    url: "/api/llm",
    token: "the-shared-secret",
  },
});
```

**Custom verification** -- for JWT validation, session checks, etc.:

```ts
createVercelHandler({
  provider: "openai",
  apiKey: process.env.OPENAI_API_KEY!,
  auth: {
    verify: async (authHeader) => {
      // Validate JWT, check session, etc.
      const token = authHeader?.replace("Bearer ", "");
      return await validateSession(token);
    },
  },
});
```

### Rate Limiting

The proxy includes an in-memory sliding-window rate limiter. Each request is keyed by the first of these that's available: the client's IP address (if `trustProxyHeaders` resolves one), then the raw `Authorization` header value, then `"anonymous"`.

```ts
createVercelHandler({
  provider: "openai",
  apiKey: process.env.OPENAI_API_KEY!,
  rateLimit: {
    windowMs: 60_000,   // 1 minute window (default)
    maxRequests: 20,     // 20 requests per window (default: 60)
  },
});
```

`trustProxyHeaders` controls whether `cf-connecting-ip` / `x-forwarded-for` / `x-real-ip` are trusted for per-IP keying, and its **default differs by adapter**:

- **Vercel, Cloudflare, Netlify** — defaults to `true`. These platforms' own edge network sets these headers, so unauthenticated traffic is rate-limited per-IP out of the box.
- **Node** (`createNodeHandler`) — defaults to `false`. A self-hosted server can't assume there's a trusted reverse proxy in front rewriting these headers (a client could otherwise forge `X-Forwarded-For` to dodge the limit), so it's conservative by default.

```ts
createNodeHandler({
  provider: "openai",
  apiKey: process.env.OPENAI_API_KEY!,
  trustProxyHeaders: true, // only if you terminate TLS/proxying yourself (nginx, etc.)
});
```

::: warning
Two things to watch for:

1. **In-memory and per-instance.** In serverless environments where each invocation may run in a separate container, this provides a best-effort limit rather than a hard guarantee. For strict rate limiting, use an external store (Redis, etc.) via the `auth.verify` callback.
2. **The Node adapter's shared "anonymous" bucket.** If you deploy with `createNodeHandler`, don't configure `auth`, and don't set `trustProxyHeaders`, every unauthenticated client falls into the *same* `"anonymous"` bucket — so `maxRequests` becomes a global cap shared by all your users, not a per-user limit. One busy client can lock everyone else out. Either enable `auth` (which keys by the caller's token) or set `trustProxyHeaders: true` (only if you're actually behind a proxy that sets these headers honestly) to get per-client limiting.
:::

## Endpoints

The proxy exposes these endpoints (relative to the base URL):

| Method | Path | Description |
|--------|------|-------------|
| POST | `/chat` | Chat completion with messages |
| POST | `/complete` | Simple text completion |
| POST | `/stream` | Streaming chat (SSE response) |
| POST | `/extract` | Structured data extraction |
| POST | `/classify` | Text classification |
| POST | `/embed` | Text embeddings |
| GET | `/models` | List available models |

## Error Codes

All errors return a structured JSON body:

```json
{
  "error": "Human-readable message",
  "code": "ERROR_CODE",
  "details": "Optional extra context"
}
```

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `AUTH_FAILED` | 401 | Missing or invalid authorization |
| `RATE_LIMITED` | 429 | Too many requests in the current window |
| `BODY_TOO_LARGE` | 413 | Request body exceeds `maxBodySize` |
| `INVALID_REQUEST` | 404 | Unknown endpoint path |
| `PROVIDER_ERROR` | 502 | The upstream LLM provider returned an error |

## SSE Streaming Protocol

The `/stream` endpoint returns a normalized `text/event-stream` response. Each event is a `data:` line containing a small JSON object.

```
data: {"type":"token","text":"Hello"}

data: {"type":"token","text":" world"}

data: {"type":"done"}
```

If a stream fails after it has started, the proxy emits `data: {"type":"error","error":"..."}` before closing the stream.

The `llm/chat-stream` function in Sema Web handles this protocol automatically. You only need to parse it yourself when building a custom client.
