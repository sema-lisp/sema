# Spike: `sema web` dev server

**Status:** exploratory. Delete this directory once the dev server graduates to
`crates/sema/src/web/` (leave no top-level `spikes/` dir behind).

**Goal:** de-risk the design for a `sema web` dev server before committing to a
plan. The proposed design is a Sema-native server (using `http/serve` +
`http/websocket`/`http/stream` + `fs/watch` + `llm/*`) launched by a thin Rust
`Web` subcommand. This spike tests whether the existing stdlib primitives
actually *compose* into a flawless dev server.

## What the dev server must do

1. Serve `.sema` source + HTML + embedded WASM/JS runtime (static).
2. Watch files and push a reload when app source changes.
3. Proxy `llm/*` calls server-side (native keys), incl. streaming.
4. Surface errors as a browser overlay (browser-side, not tested here).

The server never evaluates the app's Sema — the browser WASM runtime does. The
server is dumb plumbing.

## Probes & findings

### 1. `fs/watch` detects changes — ✅ WORKS
`fs-watch-probe.sema`: watch a dir, write a file, drain `fs/watch-events`.
- Detects `:create` and `:modify` with absolute `:paths`.
- **Caveat:** emits *multiple* events per logical change (macOS: create + 2×
  modify). The reload loop must **debounce** (coalesce events within ~100ms).

### 2. `http/serve` concurrency — ⚠️ SEQUENTIAL (the decisive finding)
`concurrency-probe.sema` + `run-concurrency-probe.sh`: fire a 3s SSE stream,
then time a concurrent plain `/hello` request fired 0.3s into the stream.

- **Result: `/hello` took 1.71s** — it blocked until the SSE stream finished.
- Root cause (`crates/sema-stdlib/src/server.rs:1331`): `http/serve` funnels all
  requests through one mpsc channel to a **single evaluator thread** that runs
  each handler — including SSE/WS handlers — **inline**:
  ```rust
  while let Some(req) = rx.blocking_recv() {
      call_callback(ctx, &handler, ...)       // one at a time
      if is_stream_response { handle_sse_response(...) }  // blocks the loop
  }
  ```
  axum accepts connections concurrently, but Sema handler execution is
  serialized. **Any long-lived handler (held-open WS, or a streaming
  `llm/stream`) freezes the entire server for its duration.**

### Implications for the design

- **Hot reload cannot use a held-open WS/SSE push** — it would block everything.
  → Use **short-poll** instead: browser polls `GET /__dev/poll` every ~300ms;
  each poll returns *instantly* ("reload" or "nothing"), so the sequential
  server handles it fine. Reload latency ≈ poll interval; fine for dev.
- **LLM streaming monopolizes the server** for the stream's duration. On a
  single-user dev loop this is usually invisible (nothing else is in flight
  during a "click → stream" interaction), but it breaks:
  - editing a file *while* a stream runs (reload waits for the stream), and
  - two simultaneous streams (a real pattern in the chat/board demos).

This is the crux decision — see `../../docs/plans/` design doc. Three ways
forward: (1) ship sequential + short-poll + document the streaming limitation;
(2) add opt-in concurrency to `http/serve` (worker interpreters); (3) hybrid —
Rust/axum serving layer, sema-llm proxy.

## How to reproduce

```bash
cargo build --release
./spikes/sema-web-devserver/run-concurrency-probe.sh   # concurrency finding
target/release/sema spikes/sema-web-devserver/fs-watch-probe.sema   # (mkdir .watch-tmp first)
```
