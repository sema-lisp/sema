---
outline: [2, 3]
---

# Cassettes (Record & Replay)

A **cassette** saves the answers from real LLM calls to a file the first time you run,
then plays them back on every run after — no API key, no network, the same output every
time. It's like recording a conversation once and replaying the tape.

Two things this makes easy:

- **Tests that don't need a key.** Record a run once, commit the file, and your
  `llm/complete` and `agent/run` tests run offline and give the same result forever — so
  they pass reliably in CI with no secrets and no flakiness.
- **Demos and docs that always work.** A playground example or a notebook can ship its
  tape and render the exact same output every time, offline, with no model drift.

Because the saved answer keeps its real token counts, cost and budget tracking keep
working on replay too — so even cost tests become repeatable.

## Quick start

```sema
;; First run: calls the real model and saves the answer to the file.
;; Every run after: plays the saved answer back — offline, identical.
(llm/with-cassette "tapes/greeting.jsonl" {:mode :auto}
  (fn ()
    (llm/complete "Say hello in one word." {:model "gpt-5-mini"})))
;; => "Hello"
```

Run it once with an API key set to capture the tape, commit `tapes/greeting.jsonl`, and
from then on the call is offline and deterministic. That's the whole idea.

## The three modes

`:mode` decides what happens on each call:

| Mode | If the call is on the tape | If it's a new call |
| --- | --- | --- |
| `:auto` *(default)* | play it back | call the model and record it |
| `:replay` | play it back | **error** — the call wasn't recorded |
| `:record` | call the model and record it | call the model and record it |

`:auto` is the friendly default for writing tapes: it records what's missing and replays
what it already has. `:replay` is what you want in CI — it never touches the network, and
a call that isn't on the tape is a **hard error** that names the request. That error is a
feature: if you change a prompt, the matching recording disappears, and the failure tells
you exactly which call drifted instead of silently hitting a live model.

## What you can record

Cassettes cover the everyday LLM calls. Each is matched and replayed independently:

| Call | Works? | Notes |
| --- | --- | --- |
| `llm/complete`, `llm/chat` | ✅ | the answer, model, tokens, and finish reason |
| `llm/extract` and structured calls | ✅ | the structured result is rebuilt from the saved answer |
| `agent/run` and tool loops | ✅ | **each turn is saved separately**, so a full multi-turn run (model → tool call → result → final answer) replays exactly — your tool handlers still run on replay |
| `llm/stream` (streaming) | ✅ | the text chunks are saved and replayed in order — see [Streaming](#streaming-in-detail) |
| `llm/embed` (embeddings) | ✅ | the vectors are saved and replayed byte-for-byte |

A note on **agents**: because each model turn is recorded on its own, your tools execute
normally during replay — the cassette only stands in for the *model's* responses, not for
your tool code. That's usually what you want: deterministic model output, real tool logic.

## Using cassettes

### `llm/with-cassette` — record/replay for a block

The usual way: wrap the calls you want recorded in a function. The tape is saved when the
block finishes, and the caller's prior cassette is restored.

```sema
(llm/with-cassette "tapes/weather-agent.jsonl" {:mode :auto}
  (fn ()
    (define bot (agent {:model "gpt-5-mini" :tools [get-weather]}))
    (agent/run bot "What's the weather in Oslo?")))
```

The options map is optional and currently takes `:mode` (`:auto`, `:record`, or
`:replay`, default `:auto`). The file — and any missing folders — is created when the tape
is written.

A task spawned inside the block captures its cassette scope. It can finish after the
block returns or be awaited later; any recordings it produces are flushed when the last
task using that captured scope finishes.

### Turning it on by hand

If your setup and teardown aren't a single block — for example in a test harness or a
notebook — use the imperative trio:

```sema
(llm/cassette-load "tapes/suite.jsonl" {:mode :replay})  ; turn it on
;; ... run many calls ...
(llm/cassette-save)    ; write the tape to disk (returns #t if a cassette is active)
(llm/cassette-eject)   ; write the tape and turn it back off
```

`llm/cassette-load` affects subsequent calls in the current evaluation. Tasks spawned
after the load inherit the cassette; tasks already spawned keep the scope they captured.
Ejecting removes the cassette from the current scope but does not detach it from those
existing tasks.

### Forcing replay across a whole run (CI)

Two environment variables initialize the cassette for a Sema run, so a whole suite — or
a whole notebook — runs offline without changing any code:

```bash
SEMA_LLM_CASSETTE=tapes/suite.jsonl \
SEMA_LLM_CASSETTE_MODE=replay \
  sema test/agents.sema
```

`SEMA_LLM_CASSETTE_MODE` is `replay`, `record`, or `auto` (default `auto`). This is
ignored under `--sandbox`, since it reads and writes a file.

A common CI pattern: record tapes locally once with a key, commit them, and run the suite
with `SEMA_LLM_CASSETTE_MODE=replay` so any un-recorded call fails loudly.

## Streaming in detail

Streaming hands you the answer in pieces — *chunks* — as the model generates them, by
calling a function you pass for each piece (a typing effect, a progress bar, live output).

A cassette records **the exact sequence of chunks**, then on replay feeds those same
chunks to your callback in the same order. So a streaming UI behaves identically offline:

```sema
;; Record once, then replay forever — the chunks arrive the same way both times.
(llm/with-cassette "tapes/story.jsonl" {:mode :auto}
  (fn ()
    (llm/stream "Tell me a two-line story."
      (fn (chunk) (display chunk))   ; called once per recorded chunk, in order
      {:model "gpt-5-mini"})))
```

Things worth knowing about streamed replay:

- **Boundaries are preserved.** If the recording arrived as `"Hel" "lo"`, replay calls
  your function with `"Hel"` then `"lo"` — not one combined `"Hello"`. Code that depends on
  chunking sees the same shape.
- **Replay is instant.** The chunks are delivered as fast as your callback accepts them;
  the original network timing between chunks is *not* reproduced. Replay is for
  determinism, not for re-simulating latency.
- **The full answer is saved too.** Alongside the chunks, the complete text, model, and
  token counts are recorded — so cost tracking and `llm/last-usage` work on a replayed
  stream just like a normal call.

If you only print the chunks (no callback), `llm/stream` writes to stdout; recording and
replay work the same way.

## Embeddings in detail

`llm/embed` returns vectors (as bytevectors). A cassette saves those vectors and returns
them verbatim on replay — so similarity scores, vector-store contents, and any math built
on them are exactly reproducible offline:

```sema
(llm/with-cassette "tapes/embeddings.jsonl" {:mode :auto}
  (fn ()
    (define v (llm/embed "semantic search query" {:model "text-embedding-3-small"}))
    (vector/cosine-similarity v (llm/embed "another phrase"))))
```

Both `llm/embed` calls are recorded (keyed by their text), so the similarity number is
identical every run. Batch embeddings — passing a list of strings — are saved as a set of
vectors and replayed in order.

## Using cassettes in notebooks

Cassettes are a great fit for [notebooks](../notebook): record the LLM cells once with a
key, commit the tape next to the `.sema-nb` file, and the notebook re-runs the same way
forever — offline, for anyone, in CI.

There are two clean patterns.

### A setup cell that turns it on

Put one cell near the top of the notebook that loads a cassette; every LLM cell after it
records or replays automatically (cells in a notebook share one environment):

```sema
;; Cell 1 — setup
(llm/cassette-load "tapes/notebook.jsonl" {:mode :auto})
```

```sema
;; Cell 2 — a normal LLM cell; recorded on first run, replayed after
(llm/complete "Summarize the Sema language in one sentence." {:model "gpt-5-mini"})
```

```sema
;; Last cell — flush the tape so the recording is written
(llm/cassette-save)
```

Run the notebook once with a key to capture `tapes/notebook.jsonl`, commit it alongside
the notebook, and every later run (including a headless `sema notebook run`) replays it.

### Force replay for the whole notebook

To guarantee a notebook never calls a model — say when you publish it or run it in CI —
run it with the environment variable set, no edits required:

```bash
SEMA_LLM_CASSETTE=tapes/notebook.jsonl \
SEMA_LLM_CASSETTE_MODE=replay \
  sema notebook run my-notebook.sema-nb
```

Any cell that makes a call not on the tape fails with a clear "cassette miss", so a stale
notebook can't quietly reach for a live model.

> **Tip:** keep tapes next to what they belong to — `tapes/` beside a test, or beside the
> `.sema-nb` — and commit them. They're plain text and diff cleanly, so a reviewer can see
> exactly how the recorded model output changed when you re-record.

## How it works with the rest of Sema

A cassette slots in just above the real model and below everything else, so it composes
instead of conflicting:

- **Cost & budgets.** A replayed answer keeps its real token counts, so
  `llm/last-usage`, `llm/session-usage`, and budget limits all behave as if the call really
  happened. This is different from a [cache](./caching) hit, which reports **zero** usage
  (no call was made); a replay stands in for a real call, so it reports the real spend.
- **Tracing.** A replayed call still produces its [OpenTelemetry](./observability) trace,
  with the recorded model and token counts — so replayed runs show up in your traces just
  like live ones.
- **The response cache.** `llm/with-cassette` turns the in-memory response
  [cache](./caching) off for its block, so the cache can't answer before the tape does.
  You generally want one or the other, not both.
- **Retries & fallback.** While *recording*, the normal [retry and fallback](./resilience)
  logic wraps the real call, so the tape captures the final successful answer. On replay
  there's nothing to retry.

## What's in the file

A tape is plain text — **NDJSON**, one JSON object per line — so it's diffable,
appendable, and reviewable in a pull request. There's one line per saved call, and the
`kind` field says what it is:

```jsonl
{"v":1,"kind":"complete","key":"a1b2…","content":"Hello","model":"gpt-5-mini","prompt_tokens":12,"completion_tokens":1}
{"v":1,"kind":"stream","key":"c3d4…","content":"Hi there","model":"gpt-5-mini","chunks":["Hi"," there"],"completion_tokens":2}
{"v":1,"kind":"embed","key":"e5f6…","model":"text-embedding-3-small","embeddings":[[0.01,-0.02,0.03]]}
```

Only the **answer** is saved, looked up by a fingerprint (`key`) of the request. The
prompt text, your API key, and any headers are **never written to the file** — they
simply aren't part of what gets saved, so a tape is safe to commit. The `v` field is a
format version, there so old tapes can be migrated if the shape ever changes.

### What counts as "the same call"

Two calls match if their meaningful inputs are the same — the model, the system prompt,
the temperature, and the messages. Change any of those and it's a different call: in
`:replay` mode you get a clear "not recorded" error, which is exactly what flags a prompt
or model change. Things that don't affect the answer — request IDs, timing, your API key —
are not part of the fingerprint.

## Recipes

- **Record once, replay in CI.** Run the suite locally with a key and
  `:mode :auto` (or `:record`) to capture tapes, commit them, then run CI with
  `SEMA_LLM_CASSETTE_MODE=replay`. New or changed calls fail loudly.
- **Update a tape after a prompt change.** Delete the tape (or the affected line) and
  re-run in `:auto`, or run that block in `:record` once. Commit the new tape; the diff
  shows how the model's answer changed.
- **A reproducible demo.** Wrap the demo's LLM calls in `llm/with-cassette … {:mode
  :replay}` and ship the tape, so it runs for anyone with no key.

## Good to know

- **Re-record after changes.** Change a prompt, model, or temperature and the old tape no
  longer matches — re-record it (`:record`, or delete the file and run `:auto`).
- **One answer per call.** The first recorded answer for a given call is the one replayed.
- **Replay needs no provider.** In `:replay` mode nothing calls a model, so a cassette
  works with no API key configured at all.
- **Cassette miss?** A "cassette miss in :replay mode" error means this exact call wasn't
  recorded. Either the request changed (re-record it) or you're replaying a call you never
  captured — switch that block to `:auto` to record it, then commit the updated tape.
