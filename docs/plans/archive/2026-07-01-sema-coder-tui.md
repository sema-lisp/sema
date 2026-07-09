# Sema Coder — full-screen TUI (Mire-inspired)

Turns the `examples/sema-coder` app from a blocking `io/read-line` loop into a
full-screen terminal UI that actually exercises the host primitives this branch
added (`term/*`, `io/tty-raw!`, `io/read-key*`, `event/select`, `sys/on-signal`).
Inspiration: the **Mire** agent demo (`/Users/helge/code/f-terminal`) — Elmish
MVU with cell-diffed rendering, a fuzzy command palette, `/slash` completion, a
chat transcript with follow-tail, and a spinner-driven streaming reply.

## Decisions (locked)

- **Bespoke hand-rendered screens**, not a general TUI framework. Region
  renderers over `term/*`; no reusable Surface/widget layer.
- **Token streaming added to `agent/run` in Rust up front**, so the assistant
  reply types in live. Verified with a FakeProvider test before any TUI work.

## Part 1 — Rust: streaming in `agent/run`

Add an **opt-in** `:on-text` callback to `agent/run` (a Sema fn called with each
text delta string). Wiring:

- `run_tool_loop` gains an `on_text: Option<&Value>` param.
- Inside the round loop, the completion becomes:
  `if let Some(cb) = on_text { do_complete_streaming(ctx, request, cb)? } else { do_complete(request)? }`.
- New helper `do_complete_streaming(ctx, request, on_text)` mirrors `do_complete`'s
  span/scope setup but drives `stream_with_dispatch`, dispatching each chunk to the
  Sema callback via `sema_core::call_callback(ctx, on_text, &[Value::string(chunk)])`.
- `agent/run` reads `:on-text` from opts and threads it through.

Invariants:
- No `:on-text` → byte-identical to today (compat is a no-op).
- Usage accounting unchanged: the loop still calls `track_usage(&response.usage)`
  on the returned `ChatResponse`; `do_complete_streaming` does **not** double-count.
- Tool-result correlation unchanged: streaming only replaces *how the assistant
  text arrives*; the assistant/tool-result message plumbing is untouched.
- Streaming bypasses the completion cache (matches `llm/stream`); documented, not
  a regression.

Verification (mandatory, AGENTS.md):
1. FakeProvider test in `llm_fake_test.rs`: script a `.stream([...])` reply, run
   `agent/run` with `:on-text` accumulating deltas, assert (a) deltas arrive in
   order, (b) final `:response` equals their concatenation. Add a second case with
   a tool round (round 1 tool call, round 2 streamed text) to prove correlation
   survives streaming.
2. Live smoke with `claude-haiku-4-5-20251001`.

## Part 2 — Sema TUI (`examples/sema-coder/`)

Single-threaded loop, two phases:
- **Idle:** raw mode; `io/read-key-timeout` drives prompt editing / palette nav /
  scroll; re-render only changed regions per key.
- **In-turn:** `run-turn` blocks; `:on-text` + `:on-tool-call` callbacks are the
  render hooks (append to transcript, repaint transcript + thinking bar). `Ctrl-C`
  sets a SIGINT flag (`sys/on-signal`) the `:on-text` callback checks and raises to
  unwind the turn (best-effort cancel, wrapped in `try`).

Layout regions (alt-screen; region line-diff, no full clears → no flicker):
header (model · cwd · token counts) · transcript (scroll region, wraps, follows
tail unless scrolled) · prompt line · status/thinking bar. Overlays composited on
top: `/slash` completion popup above the prompt; `Ctrl-K` full command palette as a
centered modal. `SIGWINCH` (`sys/check-signals`) → full re-layout.

Command palette:
- `/` at column 0 → inline fuzzy completion popup (↑↓ select, ⏎/Tab accept, Esc
  close), sourced from the `register-command!` registry + config `commands`.
- `Ctrl-K` → same list as a centered modal overlay. One fuzzy-match + one
  list-render helper shared by both.

### Files
```
main.sema     entry: TTY? → TUI loop; non-TTY or -p → existing plain one-shot path
loop.sema     raw-mode loop, phase machine, key routing, SIGWINCH/SIGINT
state.sema    model map + pure update helpers (prompt, palette, scroll, transcript)
render.sema   region renderers + per-region line-diff helper
palette.sema  fuzzy match + list overlay + /slash completion popup
theme.sema    (exists) brand palette
tools.sema    (exists) 7 tools — unchanged
agent.sema    (exists) run-turn extended to pass :on-text
commands.sema (exists) registry — unchanged
```

### Non-TTY fallback
When stdout isn't a TTY (`sys/tty`), or `-p` is given, keep today's plain
one-shot / readline path so pipes and CI still work.

## Scope

**v1:** streaming reply, thinking bar, transcript + scroll/follow-tail, `/`
completion popup, `Ctrl-K` palette, resize, interrupt, non-TTY fallback.

**Deferred (phase 2, listed not built):** approval modal for `bash`, `@file`
mentions, mouse selection, markdown rendering in the transcript, OSC-8 links.
