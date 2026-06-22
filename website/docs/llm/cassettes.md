---
outline: [2, 3]
---

# Cassettes (Record & Replay)

A **cassette** records the responses of real LLM calls to a file once, then replays
them deterministically forever — no API key, no network, identical output every run.
It's the [VCR](https://github.com/vcr/vcr) / `polly.js` pattern applied at Sema's LLM
provider seam.

Two problems it solves:

- **Tests without keys.** Record a real run once; commit the tape; every
  `llm/complete` / `agent/run` test then runs offline and deterministically in CI.
- **Reproducible demos & docs.** A playground or notebook example can ship a tape so
  it renders the same output every time, with no key and no live model drift.

Because the recorded response carries its real token counts, cost and budget logic
keep working on replay — so even cost-tracking tests become deterministic.

## Quick start

```sema
;; First run: RECORD against the real provider, writing a tape file.
;; Every run after: that same call REPLAYS from the tape — offline, deterministic.
(llm/with-cassette "tapes/greeting.jsonl" {:mode :auto}
  (lambda ()
    (llm/complete "Say hello in one word." {:model "gpt-5-mini"})))
;; => "Hello"   (live the first time, from the tape thereafter)
```

`:auto` is the authoring default: it replays a call if the tape already has it, and
records it if not. Run once with a key to capture the tape, commit the file, and the
call is deterministic from then on.

## Modes

`:mode` controls what happens on each call:

| Mode | On a recorded call | On a new call | Use for |
| --- | --- | --- | --- |
| `:auto` *(default)* | replay from tape | call provider + record | authoring tapes locally |
| `:replay` | replay from tape | **hard error** (a "miss") | CI / offline — pin exact behavior |
| `:record` | call provider + record | call provider + record | (re-)capturing a fresh tape |

A **miss in `:replay`** is deliberately a hard error naming the request — that's what
makes a prompt change visible ("this call was never recorded") instead of silently
hitting the network.

## The Sema API

### `llm/with-cassette`

Scoped record/replay for the duration of a thunk. Restores the previous state on exit
(and flushes the tape to disk):

```sema
(llm/with-cassette "tapes/weather-agent.jsonl" {:mode :auto}
  (lambda ()
    (define bot (agent {:model "gpt-5-mini" :tools [get-weather]}))
    (agent/run bot "What's the weather in Oslo?")))
```

The opts map is optional and currently takes `:mode` (`:auto` / `:record` / `:replay`,
default `:auto`). The path is created (including parent directories) when the tape is
saved.

### Imperative control

For setup/teardown that isn't a single scope (e.g. a test harness):

```sema
(llm/cassette-load "tapes/suite.jsonl" {:mode :replay})  ; install globally
;; ... run many calls ...
(llm/cassette-save)    ; flush the tape to disk (returns #t if a cassette is active)
(llm/cassette-eject)   ; flush and remove the active cassette
```

### Forcing replay in CI

Two environment variables install a cassette for the whole process, so a test suite
can be forced offline without touching its source:

```bash
SEMA_LLM_CASSETTE=tapes/suite.jsonl \
SEMA_LLM_CASSETTE_MODE=replay \
  sema test/agents.sema
```

`SEMA_LLM_CASSETTE_MODE` accepts `replay` · `record` · `auto` (default `auto`). The
env-var cassette is ignored under `--sandbox` (it reads and writes a file path).

## What's covered

Cassettes intercept every call that flows through the standard completion path:

| Call | Recorded / replayed? |
| --- | --- |
| `llm/complete`, `llm/chat` | ✅ yes |
| `llm/extract` and other structured calls | ✅ yes |
| `agent/run` and tool loops | ✅ yes — **each turn** is keyed and replayed independently, so a full multi-turn run (model → tool call → tool result → final answer) replays deterministically |
| `llm/stream` (streaming) | ❌ not yet — falls through to the real provider |
| `llm/embed` (embeddings) | ❌ not yet — falls through to the real provider |

Streaming and embeddings are the next milestone (see [Limitations](#limitations--roadmap)).

## How it folds with the rest of Sema

A cassette is layered *below* tracing, the response cache, and cost accounting, and
*above* the real provider — so it composes rather than conflicts:

- **Cost & budgets.** A replayed response carries its recorded token counts, so
  `llm/last-usage`, `llm/session-usage`, and budget enforcement all run on real
  numbers. This is different from a [cache](./caching) hit, which reports **zero**
  usage (no call was made); a replay stands in for a real call, so it reports the real
  spend.
- **Tracing.** A replayed call still emits its
  [OpenTelemetry](./observability) `chat` span, populated from the recorded model and
  tokens — so replayed runs produce traces just like live ones.
- **Response cache.** `llm/with-cassette` turns the in-memory response cache **off**
  for its scope, so a cache hit can't short-circuit before the tape is consulted. The
  two are independent layers; you generally want one or the other.
- **Retries & fallback.** While *recording*, the normal
  [retry / fallback](./resilience) logic wraps the real call, so the tape captures the
  final successful response. On *replay*, there's nothing to retry.

## The tape format

A tape is **NDJSON** — one JSON object per line, so it's diffable, appendable, and
reviewable in a PR. Each line is one recorded interaction:

```jsonl
{"v":1,"kind":"complete","key":"a1b2c3…","content":"Hello","role":"assistant","model":"gpt-5-mini","tool_calls":[],"stop_reason":"stop","prompt_tokens":12,"completion_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}
```

The tape stores **only the response**, keyed by a hash of the request. The prompt
text, your API key, and any header are **never written to disk** — redaction is
guaranteed by construction, because the request body simply isn't persisted. The `v`
field is a format-version hook for future migrations.

### What counts as "the same call" (the key)

The match key is a hash over the request's meaningful fields (model, system prompt,
temperature, and the role+content of each message) — the same canonicalization the
response cache uses. Two calls with identical inputs share a key and replay the same
recorded response; change the prompt and the key changes (so `:replay` reports a miss,
surfacing the drift).

## Limitations & roadmap

- **Completions and agents only (today).** Streaming (`llm/stream`) and embeddings
  (`llm/embed`) are not yet recorded — they call the real provider even under a
  cassette. Recording the streamed chunk sequence and embedding vectors is the next
  milestone.
- **Re-record on shape changes.** If you change a prompt, model, or temperature, the
  key changes; re-record the tape (`:record` or delete the file and run `:auto`).
- **One entry per key.** The first recorded response for a key is the one replayed.

See the design notes in `docs/plans/2026-06-21-llm-cassettes.md` for the full roadmap
(streaming/embeddings, ordinal matching, and an MCP tie-in).
