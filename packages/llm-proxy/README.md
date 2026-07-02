# @sema-lang/llm-proxy

Server-side LLM proxy for Sema web apps — deploy on Vercel, Netlify, Cloudflare Workers, or any Node.js server.

> The backend counterpart to [`@sema-lang/sema-web`](https://www.npmjs.com/package/@sema-lang/sema-web)'s `llmProxy` option. Keeps API keys server-side while your Sema code calls `llm/chat`, `llm/complete`, etc. from the browser.

## Installation

```bash
npm install @sema-lang/llm-proxy
```

## Quick Start

### Vercel (App Router)

Create `app/api/llm/[...path]/route.ts`:

```ts
import { createVercelHandler } from "@sema-lang/llm-proxy/vercel";

export const { GET, POST, OPTIONS } = createVercelHandler({
  provider: "openai",
  apiKey: process.env.OPENAI_API_KEY!,
});
```

Don't add `export const runtime = "edge"` — Vercel now deprecates Edge
Functions for new projects. Leave the default Node.js runtime, which
supports streaming responses natively.

### Netlify Functions

Create `netlify/functions/llm.ts`:

```ts
import { createNetlifyHandler } from "@sema-lang/llm-proxy/netlify";

export default createNetlifyHandler({
  provider: "anthropic",
  apiKey: process.env.ANTHROPIC_API_KEY!,
});

export const config = {
  path: "/api/llm/*",
};
```

### Cloudflare Workers

Create `src/index.ts`:

```ts
import { createCloudflareHandler } from "@sema-lang/llm-proxy/cloudflare";

export default {
  fetch: (req: Request, env: { OPENAI_API_KEY: string }) =>
    createCloudflareHandler({
      provider: "openai",
      apiKey: env.OPENAI_API_KEY,
    }).fetch(req),
};
```

### Node.js (Express)

```ts
import express from "express";
import { createNodeHandler } from "@sema-lang/llm-proxy/node";

const app = express();

// Express 5 (the current default) requires named wildcards — a bare "*"
// throws "Missing parameter name" at startup. On Express 4 use "/api/llm/*".
app.all("/api/llm/*splat", createNodeHandler({
  provider: "openai",
  apiKey: process.env.OPENAI_API_KEY!,
}));

app.listen(3001, () => {
  console.log("LLM proxy on http://localhost:3001/api/llm");
});
```

### Plain Node.js HTTP Server

```ts
import { createServer } from "http";
import { createNodeHandler } from "@sema-lang/llm-proxy/node";

createServer(createNodeHandler({
  provider: "openai",
  apiKey: process.env.OPENAI_API_KEY!,
})).listen(3001);
```

## Frontend Setup

Once deployed, point `@sema-lang/sema-web` at your proxy:

```js
import { SemaWeb } from "@sema-lang/sema-web";

const web = await SemaWeb.create({
  llmProxy: "/api/llm",     // relative URL (same origin)
  // or: "https://my-proxy.workers.dev"  // absolute URL
});
```

Then in Sema:

```scheme
;; Chat completion
(llm/chat (list (message :user "What is Sema?")) {:model "gpt-4o"})

;; Simple completion
(llm/complete "Say hello in 5 words")

;; Extract structured data
(llm/extract {:name {:type "string"} :age {:type "number"}}
  "John is 30 years old")

;; Classify text
(llm/classify (list "positive" "negative" "neutral")
  "This product is amazing!")

;; Text embeddings
(llm/embed "Hello world")

;; List available models
(llm/list-models)
```

## Configuration

```ts
import { createVercelHandler } from "@sema-lang/llm-proxy/vercel";

export const { GET, POST, OPTIONS } = createVercelHandler({
  // --- Provider ---
  provider: "openai",            // "openai" | "anthropic" | "gemini" | "groq" | "mistral" | "xai" | "ollama"
  apiKey: process.env.API_KEY!,  // API key for the provider
  baseUrl: "https://...",        // Override API base URL (Azure, self-hosted, etc.)
  defaultModel: "gpt-4o",       // Default model if not specified in request

  // Or use a full provider config:
  // provider: {
  //   provider: "openai",
  //   apiKey: process.env.OPENAI_API_KEY!,
  //   baseUrl: "https://my-azure.openai.azure.com/",
  //   defaultModel: "gpt-4o",
  // },

  // --- Authentication ---
  auth: {
    token: "my-secret",          // Require Bearer token from browser
    // Or custom:
    // verify: async (authHeader) => validateJWT(authHeader),
  },

  // --- CORS ---
  cors: "*",                     // Allow all origins (default)
  // cors: "https://myapp.com", // Restrict to specific origin
});
```

## Supported Providers

| Provider | Chat | Embeddings | Models |
|----------|------|------------|--------|
| OpenAI | ✅ | ✅ | ✅ |
| Anthropic | ✅ | ❌ | ✅ |
| Google Gemini | ✅ | ✅ | ✅ |
| Groq | ✅ | ❌ | ✅ |
| Mistral | ✅ | ✅ | ✅ |
| xAI (Grok) | ✅ | ❌ | ✅ |
| Ollama | ✅ | ✅ | ✅ |

Any OpenAI-compatible API works with `provider: "openai"` + custom `baseUrl`.

## Proxy Protocol

The proxy implements these endpoints (matching the sema-web client):

| Endpoint | Method | Body | Response |
|----------|--------|------|----------|
| `/complete` | POST | `{prompt, model?, max-tokens?, ...}` | `{content, usage?}` |
| `/chat` | POST | `{messages, model?, max-tokens?, ...}` | `{content, usage?}` |
| `/extract` | POST | `{schema, text, model?, ...}` | extracted data object |
| `/classify` | POST | `{categories, text, model?, ...}` | `{category}` |
| `/embed` | POST | `{text, model?, ...}` | `{embedding: [...]}` |
| `/models` | GET | — | `{models: [...]}` |

On errors, returns appropriate HTTP status (401, 404, 502) with `{error: "..."}`.

## Architecture

```
Browser (sema-web)              @sema-lang/llm-proxy         LLM Provider
┌────────────────┐         ┌──────────────────┐         ┌──────────────┐
│ (llm/chat ...) │──POST──▶│ handler.ts       │──POST──▶│ OpenAI       │
│                │         │  ↕ auth check    │         │ Anthropic    │
│ (llm/embed ..) │──POST──▶│  ↕ format req    │──POST──▶│ Gemini       │
│                │         │  ↕ parse resp    │         │ Groq, etc.   │
└────────────────┘         └──────────────────┘         └──────────────┘
                            │                │
                    ┌───────┴──────┐  ┌──────┴───────┐
                    │ vercel.ts    │  │ cloudflare.ts│
                    │ netlify.ts   │  │ node.ts      │
                    └──────────────┘  └──────────────┘
                    Platform adapters: convert between
                    platform APIs and ProxyRequest/Response
```

## Security

- **API keys stay server-side** — never exposed to the browser
- **Optional auth** — protect the proxy with Bearer tokens or custom verification
- **CORS** — configurable origin restrictions
- **No eval** — the proxy only forwards structured JSON requests

## License

MIT
