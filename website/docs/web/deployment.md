# Deployment

This guide covers deploying Sema Web applications to production. The deployment approach depends on whether your app uses LLM features.

If you want the broader recommended app structure first, start with [Building a Sema Web App](./building-apps). This page is focused on the deployment step.

## Static Hosting (No LLM)

If your app does not use any `llm/*` functions, deployment is straightforward: serve static files.
Sema Web runs entirely in the browser via WebAssembly.

For development and quick embeds, you can still load `.sema` source directly. For production,
prefer a compiled `.vfs` archive built with the CLI.

### Development

```html
<script type="text/sema" src="/app.sema"></script>
<script type="module">
  import { SemaWeb } from "@sema-lang/sema-web";
  await SemaWeb.init();
</script>
```

### Production

Build the app once:

```bash
sema build --target web app.sema -o public/app.vfs
```

Then deploy the compiled archive:

```
my-app/
  public/
    index.html
    app.vfs
```

```html
<!DOCTYPE html>
<html>
<head>
  <title>My Sema App</title>
</head>
<body>
  <div id="app"></div>

  <script type="text/sema" src="/app.vfs"></script>
  <script type="module">
    import { SemaWeb } from "@sema-lang/sema-web";
    await SemaWeb.init();
  </script>
</body>
</html>
```

The HTML shape stays the same. Only the `src` changes from source (`.sema`) to a compiled
archive (`.vfs`).

## With LLM Proxy

When your app uses `llm/chat`, `llm/complete`, or other LLM functions, you also need a server-side proxy to hold API keys. The proxy is a single serverless function.

### Architecture

```
Static host              Serverless function         LLM Provider
┌───────────────┐        ┌─────────────────┐        ┌────────────┐
│ HTML + .sema  │──POST──│ /api/llm/*      │──API──>│ OpenAI     │
│ sema-web WASM │        │ (holds API key) │        │ Anthropic  │
└───────────────┘        └─────────────────┘        │ Gemini ... │
                                                     └────────────┘
```

### Vercel Deployment

This is the recommended approach. Vercel serves both static files and serverless functions from a single project.

**1. Project structure:**

```
my-sema-app/
  public/
    index.html
    app.vfs
  api/
    llm/
      [...path].ts        # Proxy catch-all route
  package.json
  vercel.json
```

**2. Install dependencies:**

```bash
npm install @sema-lang/sema-web @sema-lang/llm-proxy
```

**3. Create the proxy function:**

```ts
// api/llm/[...path].ts
import { createVercelHandler } from "@sema-lang/llm-proxy/vercel";

export const { GET, POST, OPTIONS } = createVercelHandler({
  provider: "openai",
  apiKey: process.env.OPENAI_API_KEY!,
  defaultModel: "gpt-4o",
  rateLimit: { maxRequests: 30 },
});
```

A plain `export default createVercelHandler({...})` won't work here — Vercel's
`api/` directory expects either named exports per HTTP method (what the
destructuring above produces) or a `fetch`-shaped default export, not an
object of `{GET, POST, OPTIONS}` methods as the default export itself.

**4. Build the app for production:**

```bash
sema build --target web app.sema -o public/app.vfs
```

**5. Create the HTML entry point:**

```html
<!-- public/index.html -->
<!DOCTYPE html>
<html>
<head>
  <title>My AI App</title>
  <script type="text/sema" src="/app.vfs"></script>
  <script type="module">
    import { SemaWeb } from "@sema-lang/sema-web";

    await SemaWeb.create({
      llmProxy: "/api/llm",
    });
  </script>
</head>
<body>
  <div id="app"></div>
</body>
</html>
```

**6. Set environment variables:**

```bash
vercel env add OPENAI_API_KEY
# Paste your API key when prompted
```

**7. Deploy:**

```bash
vercel deploy          # Preview deployment
vercel --prod          # Production deployment
```

### Netlify Deployment

**1. Project structure:**

```
my-sema-app/
  public/
    index.html
    app.vfs
  netlify/
    edge-functions/
      llm.ts
  netlify.toml
  package.json
```

**2. Create the edge function:**

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
declared for the Functions scope and is what Netlify's current docs document
for this runtime.

**3. Build the app for production:**

```bash
sema build --target web app.sema -o public/app.vfs
```

**4. Configure Netlify:**

```toml
# netlify.toml
[build]
  publish = "public"

[[edge_functions]]
  path = "/api/llm/*"
  function = "llm"
```

**5. Set environment variables** in the Netlify dashboard under Site settings > Environment variables.

**6. Deploy:**

```bash
netlify deploy --prod
```

## Environment Variables

API keys must be set as server-side environment variables. Never include them in client-side code or commit them to source control.

| Variable | Provider | Required for |
|----------|----------|-------------|
| `OPENAI_API_KEY` | OpenAI | `provider: "openai"` |
| `ANTHROPIC_API_KEY` | Anthropic | `provider: "anthropic"` |
| `GOOGLE_API_KEY` | Gemini | `provider: "gemini"` |
| `GROQ_API_KEY` | Groq | `provider: "groq"` |
| `MISTRAL_API_KEY` | Mistral | `provider: "mistral"` |
| `XAI_API_KEY` | xAI | `provider: "xai"` |

Ollama runs locally and does not require an API key.

## CORS Configuration

By default, the proxy allows all origins (`cors: "*"`). For production, restrict it to your domain:

```ts
createVercelHandler({
  provider: "openai",
  apiKey: process.env.OPENAI_API_KEY!,
  cors: "https://my-app.example.com",
});
```

If your static files and proxy are served from the same domain (as in the Vercel example above), CORS is not an issue -- same-origin requests do not need CORS headers.

## Production Checklist

- [ ] API keys stored in environment variables, not in code
- [ ] Proxy authentication enabled (shared token or custom `verify`)
- [ ] CORS restricted to your domain
- [ ] Rate limiting configured to prevent abuse
- [ ] `maxBodySize` set appropriately (default: 1MB)
- [ ] Static assets served with appropriate cache headers
