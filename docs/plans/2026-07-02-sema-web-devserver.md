# Design: `sema web` — the dev server

**Date:** 2026-07-02
**Status:** M0/M1 shipped + M4 docs (2026-07-02). `sema web` serves an app in the
browser, hot-reloads on edit, and proxies llm/* with server keys — verified
end-to-end in a real browser (render, hot reload, and a live chat round-trip).
M2 (concurrency / progressive streaming) and M3 (state-preserving HMR) deferred.
**Related:** GitHub issue #18 (§11 Tooling), `docs/plans/2026-07-02-sema-web-framework-gaps.md`, `packages/llm-proxy/`

## 1. Problem

Running a sema-web app today takes ~4 moving parts: build the `sema-web` TS
bindings, `sema build --target web app.sema` to produce the VFS archive, serve
the static files (`npx serve`/vite), and run a hand-rolled Node script to proxy
`llm/*` calls. Issue #18 §11 promised `sema web app.sema` collapsing all of that
into one zero-config command. This is the last un-built piece of the sema-web
"Phase 4 tooling"; the build half (`sema build --target web`) already exists.

## 2. Goal & non-goals

**Goal:** one command — `sema web app.sema` — that serves a Sema web app in the
browser with no npm / bundler / hand-rolled proxy, watches files and reloads the
browser on change, and proxies `llm/*` server-side with native keys.

**Non-goals (this design):**
- **Deploy/packaging** to Vercel/Netlify/Cloudflare. Separate research-spec.
  This design's only obligation to it: leave a **clean seam** (the dev LLM proxy
  speaks the same protocol as the production `packages/llm-proxy`).
- **AOT `--target wasm`** compiler and **`--target js`** transpiler. Out of scope.
- **State-preserving HMR** in M1 (full reload first; HMR is a fast-follow — §7).

## 3. Key architectural fact

**The dev server never compiles or evaluates the app's Sema.** The browser WASM
runtime (`sema-wasm`) evals `.sema` source directly. So the server's entire job
is dumb plumbing: serve source + assets, watch files, signal reload, proxy LLM.
Compile/runtime errors surface *in the browser* at WASM-eval time, so the error
overlay is a browser-side concern in the sema-web loader, not the server's.

## 4. Substrate decision

**Sema-native dev server, thin Rust launcher.** The server logic is written in
Sema (`dev_server.sema`, embedded in the binary via `include_str!`), using
`http/serve` + `http/router` + `http/stream` + `fs/watch` + `llm/*`. Rust adds
only a `Web` subcommand that embeds the browser-runtime assets, assembles a VFS,
wires args (port/host/entry/root), and runs the embedded script through the
normal interpreter.

Rationale: dogfoods the language; the LLM proxy is literally native `llm/*` (same
providers/caching/retry/budget as CLI, **zero protocol drift**); minimal new
Rust; users can eject/customize the server (it's just a script). Mirrors the
existing `Notebook`/`Mcp`/`Lsp` subcommand pattern.

### 4.1 The concurrency constraint (from the spike — see §10)

`http/serve` is **sequential**: it runs every handler (including SSE/WS) inline
on one evaluator thread, so any long-lived handler blocks all other requests
(measured: a plain request blocked 1.71s behind a live 3s stream). Consequences:

- **Hot reload uses short-poll, not a held-open connection.** The browser polls
  `GET /__dev/poll` every ~300ms; each poll returns *instantly*, so the
  sequential server never blocks on it. A held-open WS/SSE reload channel would
  freeze the server and is therefore rejected.
- **A live `llm/stream` monopolizes the server** for the stream's duration.
  Usually invisible on a single-user dev loop, but it degrades: editing while a
  stream runs (reload waits), and two simultaneous streams (chat-widget/board
  demos). This is the **known M1 limitation**, documented, fixed in M2.

## 5. Architecture

```
  sema web app.sema --port 3000
        │
        ▼  Rust `Web` subcommand  (crates/sema/src/web/mod.rs, ~80 lines)
     • embeds browser runtime (sema_wasm_bg.wasm + sema-web JS) via rust-embed
     • embeds dev_server.sema
     • assembles VFS: {app dir source} + {embedded runtime} + {args}
     • runs dev_server.sema through the interpreter
        │
        ▼  dev_server.sema  (~150 lines — the actual server)
     (http/serve
       (http/router
         [[:get    "/"           serve-html-shell]     ; app index.html or synthesized
          [:static "/runtime"    <embedded-runtime>]   ; wasm + sema-web JS
          [:static "/app"        <app-dir>]            ; raw .sema + user assets
          [:get    "/__dev/poll" poll-reload]          ; short-poll: {:reload? bool :paths [...]}
          [:post   "/complete"   llm-proxy]            ; ┐ production llm-proxy protocol
          [:post   "/chat"       llm-proxy]            ; │ (/complete /chat /extract
          [:post   "/stream"     llm-proxy-sse]        ; │  /classify /embed /models /stream)
          [:post   "/extract"    llm-proxy]            ; │
          [:post   "/classify"   llm-proxy]            ; │
          [:post   "/embed"      llm-proxy]            ; │
          [:get    "/models"     llm-models]])         ; ┘
       {:port port :host "127.0.0.1"})
```

### 5.1 Components (each independently understandable/testable)

- **`Web` subcommand (Rust)** — arg parsing, asset embedding, VFS assembly,
  script launch. Depends on: interpreter, rust-embed. No web logic.
- **`dev_server.sema`** — routing + the four jobs below. Depends on: stdlib
  `http/*`, `fs/watch`, `llm/*`. Pure Sema, ejectable.
- **Reload poller** — `fs/watch` handle + a debounced "last-changed" timestamp;
  `poll-reload` compares the client's last-seen stamp and answers instantly.
- **LLM proxy handlers** — thin wrappers over native `llm/*`; one per protocol
  endpoint. `/stream` uses `http/stream` → SSE.
- **HTML shell** — serve app's `index.html` if present next to the entry, else
  synthesize a minimal shell that loads `/runtime` + mounts the entry `.sema`.
- **Browser dev-client** (`packages/sema-web/src/dev-client.ts`) — short-poll
  loop, triggers reload, renders the error overlay. Depends on: loader.

## 6. The LLM proxy (deploy seam)

Hard constraint: **dev speaks the production protocol.** `dev_server.sema`
implements the exact endpoints `packages/llm-proxy` defines (`/complete`,
`/chat`, `/extract`, `/classify`, `/embed`, `/models`, `/stream`). The browser
client (`sema-web/src/llm.ts` → `llm/proxy-url`) points at the dev server with
**zero code change** between dev and prod — only the URL differs. Handlers are
trivially correct because they call native `llm/*`:

```scheme
(define (llm-proxy req)
  (http/ok (llm/complete (:prompt (:json req)) (:opts (:json req)))))
```

Keys come from env, same as CLI `sema`. This makes deploy "swap the transport,
keep the protocol."

## 7. Hot reload

- Browser dev-client short-polls `GET /__dev/poll?since=<stamp>`; server returns
  `{:reload? true :paths [...]}` when `fs/watch` fired since `<stamp>`.
- `fs/watch` emits multiple events per logical change → **debounce** (coalesce
  within ~100ms) and exclude `node_modules`/`.git`/dotfiles.
- **M1 behavior: full re-eval.** Browser re-fetches changed `.sema` and re-runs
  the WASM app. `store/*` (localStorage) survives; in-memory reactive `state`
  resets.
- **HMR (fast-follow, §2):** the poll response already carries changed paths, so
  a state-preserving HMR (keep reactive signals, swap only changed component
  defs) is an *additive* change to the browser loader — no server change. Spike
  it early; ship if tractable, otherwise stay on full reload (acceptable).

## 8. Zero-config surface

- `sema web app.sema` → binds `127.0.0.1:<port>`, opens browser.
- **HTML shell:** serve `index.html` next to the entry if present; else
  synthesize one. Zero-config path = synthesize.
- **Flags:** `--port`, `--host` (loud warning if non-localhost — exposes the LLM
  proxy), `--no-open`, `--no-llm` (disable proxy), `--llm-proxy <url>` (defer to
  an external proxy).
- **Port fallback (built 2026-07-02):** the dev server binds with
  `:port-fallback true` + `:on-listen` (already shipped in `http/serve`, backed
  by `sema_core::net::bind_with_fallback`), so a busy port auto-advances and the
  launcher opens the browser at the *actual* bound URL. `http/serve` keeps this
  off by default (opt-in); first-party servers (notebook, this dev server) opt
  in. See `crates/sema-docs/entries/stdlib/web-server/http-serve.md`.
- **Offline & flawless:** runtime assets embedded in the binary (no CDN/npm).
  Biggest integration cost — §9.

## 9. Risks / integration costs

1. **Asset embedding** — the binary must carry a known-good `sema_wasm_bg.wasm` +
   compiled `sema-web` JS bundle. Couples the Rust release build to the JS/WASM
   build (a Makefile step builds+copies them into `crates/sema/src/web/runtime/`
   before `cargo build`). Notebook already embeds its UI, so there's precedent;
   still the most likely place to feel "hacky" if rushed. **Spike before M1.**
2. **`http/serve` sequential** (§4.1) — the known M1 streaming limitation. M2.
3. **Watch storms** — exclude `node_modules`/`.git`; debounce.
4. **`http/serve` static maturity** — MIME/range/headers may need hardening;
   fixes land in the shared stdlib server (benefits everyone).

## 10. Spike results (2026-07-02)

Full detail in `spikes/sema-web-devserver/README.md`. Summary:
- ✅ `http/serve` static/routing/handlers, `http/stream` SSE, `fs/watch` all work.
- ⚠️ `http/serve` is **sequential** — a plain request blocked **1.71s** behind a
  live SSE stream. Drives the short-poll reload design and the M1 streaming
  limitation.
- `fs/watch` fires multiple events per change → debounce required.

## 11. Milestones

**Shipped 2026-07-02:** M0 (asset embedding via `make web-runtime` + build.rs
`web_runtime` cfg), M1 (the `Web` subcommand + `dev_server.sema`: serve embedded
runtime + synthesized import-map shell + app source; short-poll hot reload;
native LLM proxy speaking the production protocol; browser error overlay;
auto-open), M4 (`website/docs/web/dev-server.md`). Gates: `make test-web-e2e`
(Playwright render + hot reload), `crates/sema/tests/web_dev_server_test.rs`
(serving contract + live proxy). **M2 Tier 1 shipped
(2026-07-02):** the SSE channel is now unbounded + non-blocking, fixing the
llm/stream panic and delivering **progressive** token streaming (`/stream` uses
real `llm/stream`). **Still deferred:** M2 concurrency (head-of-line blocking — a
live stream holds the single evaluator thread; the worker-pool / LLM-offload
options both need soundness-sensitive `Send` surface in sema-core, so parked),
M3 (state-preserving HMR). See the concurrency design workflow judgment.


- **M0 — asset-embedding spike:** prove the Rust launcher can embed + serve the
  wasm + sema-web bundle and boot an existing example with no npm. De-risks §9.1.
- **M1 — dev server (sequential):** `Web` subcommand + `dev_server.sema` (serve +
  short-poll reload + full-reload browser client + native LLM proxy speaking the
  production protocol + error overlay). Ships a real, working `sema web`. Known
  limitation: live streams monopolize the server (documented).
- **M2 — concurrency fix (open sub-decision):** remove the streaming limitation.
  Two candidates, decided with M1 usage data (client is transport-agnostic, so
  no rework either way):
  - **Harden `http/serve`** with opt-in concurrent handlers (worker
    interpreters; handlers documented stateless) — keeps pure-Sema, general win.
  - **Hybrid Rust/axum serving layer** (native concurrency, mirrors notebook) —
    robust, adds Rust, drops the Sema-server dogfooding.
- **M3 — HMR fast-follow:** attempt state-preserving hot reload (browser-side,
  additive). Ship if tractable.
- **M4 — docs:** `website/docs/web/dev-server.md`.

## 12. Placement

Production homes (graduation targets):

| Thing | Home |
|---|---|
| Rust `Web` subcommand | `crates/sema/src/web/mod.rs` (new submodule dir) |
| Dev server script | `crates/sema/src/web/dev_server.sema` (`include_str!`) |
| Embedded browser runtime | `crates/sema/src/web/runtime/` (built + embedded) |
| Browser dev-client + overlay | `packages/sema-web/src/dev-client.ts` + loader hook |
| User docs | `website/docs/web/dev-server.md` |

**Spike home:** `spikes/sema-web-devserver/` — deleted on graduation (leaves no
top-level `spikes/` dir behind).

## 13. Testing

- **Rust:** `Web` subcommand arg parsing + VFS assembly + asset embedding
  (integration test booting an example headlessly, à la notebook tests).
- **Sema:** unit-test the reload debounce and the proxy handlers against a
  FakeProvider (per AGENTS.md LLM conventions).
- **E2E (Playwright):** `sema web` boots an example, hot reload fires on file
  change, an `llm/*` call round-trips through the dev proxy. Extends the existing
  `packages/sema-web/e2e/` harness.
