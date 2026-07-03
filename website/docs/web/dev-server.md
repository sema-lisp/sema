# Dev Server

`sema web` serves a Sema web app in the browser with one command — no bundler,
no `npm install`, no hand-rolled backend for `llm/*`. It bundles the browser
runtime (the WASM VM + JS), serves your app, reloads the page when you edit a
file, and proxies `llm/*` calls to real providers using server-side keys.

```bash
sema web app.sema
```

```
  Sema Web dev server
  → http://127.0.0.1:3000
  serving app.sema from /path/to/your/app
```

That's it — the browser opens to your running app.

## What it does

- **Serves the runtime.** The WASM interpreter and the `@sema-lang/*` JS modules
  are embedded in the `sema` binary and served under `/__sema/`, wired up with a
  generated `<script type="importmap">`. No `node_modules` at runtime.
- **Serves your app.** Your entry `.sema` (and anything next to it) is served
  from `/app/`. The browser fetches the source and the WASM VM evaluates it — the
  server never compiles your code.
- **Hot reloads.** The server watches your app directory. When a file changes,
  the page reloads and fetches the new source.
- **Proxies `llm/*`.** Browser `llm/*` calls are answered by the dev server using
  the API keys in your environment (see [LLM proxy](#llm-proxy)).

## Options

```bash
sema web app.sema [OPTIONS]
```

| Option        | Default       | Description                                            |
| ------------- | ------------- | ------------------------------------------------------ |
| `--port <n>`  | `3000`        | Port to serve on. Advances to the next free port if taken. |
| `--host <h>`  | `127.0.0.1`   | Address to bind. A non-loopback host exposes the LLM proxy to the network. |
| `--no-open`   | —             | Don't open a browser automatically.                    |
| `--no-llm`    | —             | Disable the built-in LLM proxy.                        |

## Hot reload

Edit your `.sema` file and the browser repaints. Reload is a **full page
reload** — the app re-runs from a clean slate, so there's no stale state. Values
persisted with `store/*` (localStorage) survive; in-memory reactive `state`
resets.

The page short-polls the server for changes, so reload works without a
persistent connection. There is no separate watch process to run.

## LLM proxy

Because API keys can't live in the browser, the dev server answers `llm/*` for
you. It speaks the same protocol as [`@sema-lang/llm-proxy`](/docs/web/llm-proxy),
so your app code is identical in development and production — only the proxy URL
differs. In dev, it's wired to the dev server automatically.

```sema
;; In the browser — no keys, no config. The dev server calls the real provider.
(llm/complete "Summarize this in one line: ..." {:model "claude-haiku-4-5-20251001"})
```

Keys come from your environment, exactly as with the `sema` CLI
(`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, …). Disable the proxy with `--no-llm`.

Streaming (`llm/chat-stream`) delivers tokens **progressively** — they render as
they arrive from the provider.

::: warning One stream at a time
The built-in HTTP server is single-threaded, so a live stream holds it for its
duration: other requests (including hot-reload polls and a second stream) queue
until it finishes. Fine for a single-user dev loop; true concurrency is a
planned improvement.
:::

## Error overlay

Compile and runtime errors in your app appear as an overlay across the top of
the page (and in the console), so you don't have to hunt for them. Click the
overlay to dismiss it; the next successful reload clears it.

## Multi-file apps

Multi-file apps work with no extra steps. A single-file app runs from raw source
(the browser compiles it directly); an app that `(import ...)`s other `.sema`
modules is compiled to a `.vfs` archive on the fly — the same artifact
[`sema build --target web`](/docs/web/deployment) produces — and rebuilt on each
reload. The dev server picks the right mode automatically; imports just resolve.

## How it compares

`sema web` is the zero-config path for developing an app. When you're ready to
ship, [`sema build --target web`](/docs/web/deployment) produces a compiled
`.vfs` you serve as static files, and [`@sema-lang/llm-proxy`](/docs/web/llm-proxy)
provides the production proxy for Vercel/Netlify/Cloudflare/Node.
