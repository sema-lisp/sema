# LLM Cassettes — record/replay for deterministic LLM testing & demos

**Status:** M1 + Sema surface SHIPPED (2026-06-22). `complete`/`chat`/`extract` and
**agent loops** record/replay deterministically and keylessly; folds with otel + cache
+ cost accounting (see `crates/sema-llm/src/cassette.rs`, tests `llm_cassette_test` /
`otel_cassette_test`, docs `website/docs/llm/cassettes.md`). **Remaining:** M2
(streaming chunk-array + embeddings + batch), M4 (record tapes for the playground
`llm-tools` examples + wire `SEMA_LLM_CASSETTE_MODE=replay` into `make test`).

Implementation note vs. the sketch below: rather than a registry-swapped decorator
provider (which fights Rust's "can't move out of `Box<dyn Trait>`" on scope exit), the
shipped design is a thread-local interceptor in `do_complete` — below the otel span +
response cache, above the real provider. Same seam, cleaner ownership. The tape stores
only the response keyed by a request hash (no request body) so redaction is free.

Companion to the LLM bulletproofing plan; this is the foundational testability
primitive that unblocks CI verification of every LLM/agentic feature **without API keys**.

## Why this is the highest-leverage thing

The homepage makes confident claims about LLM/agentic features, but almost none
of them are verifiable in CI today — they need live API keys and produce
nondeterministic output, so they can't be asserted on. A cassette layer fixes
three things at once:

1. **Testability without keys.** Record real provider responses once; replay them
   deterministically forever. Every `complete`/`chat`/`stream`/`extract`/agent-loop
   test becomes a normal, offline, deterministic test that runs in CI.
2. **Reproducible demos & docs.** Playground/notebook LLM examples can ship a
   recorded tape so they render identical output every time, offline.
3. **Thesis alignment.** Sema already sells a *deterministic cooperative
   scheduler*. "Deterministic, replayable LLM/agent runs" is the same story
   extended to I/O — a genuine differentiator, not a me-too feature.

This is the VCR / `vcr`/`betamax`/`polly.js` pattern, applied at Sema's clean
provider seam.

## The seam (verified)

Every LLM call funnels through one place. In `crates/sema-llm/src/builtins.rs`:

```rust
fn with_provider<F, R>(f: F) -> Result<R, SemaError>
where F: FnOnce(&dyn LlmProvider) -> Result<R, SemaError> {
    PROVIDER_REGISTRY.with(|reg| {
        let reg = reg.borrow();
        let provider = reg.default_provider().ok_or_else(/* ... */)?;
        f(provider)            // <-- the single chokepoint
    })
}
```

`LlmProvider` (`crates/sema-llm/src/provider.rs`) is a **sync** trait:

```rust
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError>;
    fn default_model(&self) -> &str;
    fn stream_complete(&self, request: ChatRequest,
        on_chunk: &mut dyn FnMut(&str) -> Result<(), LlmError>) -> Result<ChatResponse, LlmError>;
    fn batch_complete(&self, requests: Vec<ChatRequest>) -> Vec<Result<ChatResponse, LlmError>>;
    fn embed(&self, request: EmbedRequest) -> Result<EmbedResponse, LlmError>;
}
```

Because **all** call paths (complete, stream, batch, embed) go through a
`&dyn LlmProvider`, a single **decorator provider** captures everything. No need
to touch any builtin or any of the four concrete providers.

## Design: `CassetteProvider` decorator

```rust
// crates/sema-llm/src/cassette.rs  (new)

pub enum CassetteMode {
    /// Always hit the real provider AND write each interaction to the tape.
    Record,
    /// Never hit the network; serve from the tape. Miss => error.
    Replay,
    /// Replay if present, else record (the convenient default for dev/test authoring).
    Auto,
}

pub struct CassetteProvider {
    inner: Box<dyn LlmProvider>,   // the real provider (None-able for pure replay)
    mode: CassetteMode,
    tape: RefCell<Tape>,           // loaded from disk; flushed on drop / explicit save
    path: PathBuf,
}

impl LlmProvider for CassetteProvider {
    fn name(&self) -> &str { self.inner.name() }
    fn default_model(&self) -> &str { self.inner.default_model() }

    fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let key = cassette_key(&request);
        match self.mode {
            CassetteMode::Replay => self.tape.borrow().lookup(&key)
                .ok_or_else(|| LlmError::Config(format!("cassette miss: {key}")))?
                .clone().into_response(),
            CassetteMode::Auto if self.tape.borrow().has(&key) =>
                self.tape.borrow().lookup(&key).unwrap().clone().into_response(),
            _ /* Record | Auto-miss */ => {
                let resp = self.inner.complete(request.clone())?;
                self.tape.borrow_mut().record(key, &request, &resp);
                Ok(resp)
            }
        }
    }
    // stream_complete: record the *sequence of chunks* + final response; on replay,
    //   feed recorded chunks to on_chunk in order (optionally with synthetic pacing).
    // batch_complete: key each request independently; preserves order.
    // embed: same pattern against EmbedRequest/EmbedResponse.
}
```

### Request key (the matching strategy)

The key decides what counts as "the same call". Default = a stable hash over the
**semantically meaningful** fields of `ChatRequest`, normalized:

- model, messages (role+content), system prompt, temperature, max-tokens, tools,
  tool-choice, response-format.
- **Excluded** from the key (recorded for debugging, not matched): timestamps,
  request IDs, api-key, any header.
- Normalization: trim, stable JSON key ordering, drop nulls.

Provide an escape hatch for "match by ordinal position instead of content"
(useful when a test fires N calls and you don't want to pin exact prompts):
`:match :sequence` records/serves in call order; `:match :request` (default)
matches by the hash above.

### Tape format (on disk)

One file per tape (a "cassette"), NDJSON so it's diffable, appendable, and
human-readable. Reuse the existing `ndjson` machinery in `sema-llm`.

```jsonl
{"v":1,"kind":"complete","key":"a1b2…","request":{…normalized…},"response":{"content":"…","model":"…","usage":{"prompt-tokens":12,"completion-tokens":34},"stop-reason":"end"}}
{"v":1,"kind":"stream","key":"c3d4…","request":{…},"chunks":["Hel","lo"],"response":{…}}
{"v":1,"kind":"embed","key":"e5f6…","request":{…},"response":{"vectors":[[…]]}}
```

API keys and auth headers are **redacted at record time** — never written to a tape.

## Sema surface

Two entry points — a scoped macro for tests/examples and an env var for CI:

```sema
;; Scoped: record on first run, replay forever after. Restores prior provider on exit.
(with-cassette "tests/tapes/weather-agent.jsonl" {:mode :auto}
  (defagent weather { … })
  (run-agent weather "What's the weather in Oslo?"))

;; Or imperative:
(llm/cassette-load "path.jsonl" :mode :replay)
…
(llm/cassette-save)   ; flush (also auto-flushed on scope/drop)
```

- `SEMA_LLM_CASSETTE=path` + `SEMA_LLM_CASSETTE_MODE=replay|record|auto` lets CI
  force replay globally without touching test source.
- Implementation: `with-cassette` wraps the currently-registered default provider
  in a `CassetteProvider`, swaps it into `PROVIDER_REGISTRY` for the dynamic
  extent, restores on exit (mirrors how a dynamic-binding form would work).

## Interactions / things to get right

- **Streaming determinism:** replay must reproduce the same chunk boundaries the
  recording saw (store the chunk array verbatim). Optional `:pacing :realtime`
  re-injects recorded inter-chunk delays for demos; default is instant.
- **`pmap`/`batch` ordering:** key per-request so concurrent execution still
  replays deterministically regardless of completion order. Pairs with the
  existing deterministic scheduler.
- **Cost tracking:** replayed responses carry recorded `usage`, so `track_usage`,
  budgets, and cost math all exercise on replay — cost-tracking tests become
  deterministic too.
- **Cassette miss in `:replay`** is a hard error with the offending key + a diff
  hint ("nearest recorded request differs in: temperature") — this is what makes
  prompt regressions visible.
- **Redaction** is mandatory and tested (no key/header ever lands on disk).
- **MCP tie-in:** the same record/replay idea generalizes to MCP tool calls (see
  the MCP-client spike) — design the tape `kind` field to be open for `"mcp-call"`.

## Milestones

- **M1 — minimal:** `CassetteProvider` for `complete` only; record + replay +
  auto; NDJSON tape; request-hash key; redaction. One Rust test that records
  against a fake provider and replays. *Acceptance:* a `complete` call replays
  byte-identically offline.
- **M2 — streaming + embed + batch:** cover the rest of the trait; chunk-array
  replay; per-request keys for batch. *Acceptance:* an agent-loop test (tool call
  → tool result → final answer) replays deterministically offline.
- **M3 — Sema surface:** `with-cassette` macro + `llm/cassette-*` builtins + env
  vars. *Acceptance:* a `.sema` example with a committed tape runs offline in CI.
- **M4 — CI + corpus:** record tapes for the playground `llm-tools` examples and
  the agentic test suite; wire `SEMA_LLM_CASSETTE_MODE=replay` into `make test`.
  *Acceptance:* the LLM/agentic test suite runs green in CI with no keys.

## Open questions

- Pure-replay with **no inner provider** (CI has no key at all) vs. always
  requiring a real provider to wrap — M1 should support a `NullProvider` inner so
  replay works with zero credentials.
- Tape **versioning/migration** when `ChatRequest`/`ChatResponse` shapes change
  (the `"v":1` field is the hook; need a re-record workflow).
- Should cassettes live beside tests (`tests/tapes/`) or in a top-level
  `cassettes/`? Leaning beside-tests for locality.
- One tape per test vs. one shared tape — leaning one-per-test for isolation and
  clean diffs.
