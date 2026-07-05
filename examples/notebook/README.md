# Sema Notebooks

Runnable, annotated notebooks (`.sema-nb`). Open them in the notebook UI
(`sema notebook serve <file>`) or execute headlessly:

```bash
sema notebook run examples/notebook/async-basics.sema-nb
```

Headless runs re-execute every cell and bake the outputs back into the file.

## The async series

Five notebooks that teach Sema's cooperative async runtime through real
use-cases, in reading order. Every claim is demonstrated by a printed
measurement, and each notebook ends with its honest limits.

| Notebook | What you take home | Requires |
|---|---|---|
| `async-basics.sema-nb` | The whole concurrency model: tasks, channels, combinators, cancellation, bounded fan-out — with wall-clock proof (incl. *what does not yield*) | nothing |
| `concurrent-data-pipeline.sema-nb` | The fetch→parse→aggregate→persist template: per-item error resilience, measured pool-map speedup, the async error-catchability asymmetry | network |
| `llm-batch-enrichment.sema-nb` | Classify a dataset concurrently with the three guardrails: bounded pool, response cache (re-runs are free), fail-closed budget — plus a cost report | none¹ |
| `research-agents.sema-nb` | Agents with tools → streaming → two concurrent conversations → mid-conversation cancellation → sessions | none¹ |
| `realtime-monitor.sema-nb` | The monitoring template: sensors + subprocess + file-tail sources fanning into a channel bus, windowed stats, alerts, clean shutdown | nothing² |

¹ By default the LLM notebooks **replay recorded responses from cassette
tapes** (`examples/notebook/tapes/`) — keyless, free, deterministic. Set
`live-run?` to `#t` in the first code cell for a live run against real
providers (`ANTHROPIC_API_KEY` etc.; well under a cent per run). The
cancellation demo in `research-agents` only runs live — an aborted round
records nothing to a tape.

² Includes one optional WebSocket cell against a public echo server that
skips itself gracefully when offline.

## Other notebooks

- `demo.sema-nb` — the general Sema language tour.
