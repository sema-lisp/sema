# Verify LLM / Agentic Features

Verify a change to Sema's LLM/agent layer (`sema-llm`) using the two-tier flow
that's caught real bugs (see CHANGELOG 1.21.x). `$ARGUMENTS` names the feature(s)
to focus on (e.g. "tool loop", "fallback", "caching", "reasoning-effort"); if
empty, cover the agent loop, fallback, cache, and budget.

## Tier 1 — Deterministic, keyless (required, runs in CI)

Use `sema_llm::fake::FakeProvider` (scripted replies / tool calls / errors /
streamed chunks) installed via `register_test_provider`; assert on
`FakeRecorder` requests. Tests live in `crates/sema/tests/llm_fake_test.rs`.
Hooks: `set_retry_base_ms(0)` (no sleeps), `set_network_max_retries`.

```bash
cargo test -p sema-lang --test llm_fake_test
```

Add a FakeProvider test for any change to the agent loop, retry, cache, budget,
or a provider serializer — this is the regression oracle. Prefer asserting on
**behavior the model can't fake**: round-2 tool-result correlation, retry attempt
counts, cache hit ⇒ `recorder.call_count()` unchanged and zero added usage.

## Tier 2 — Live integration (when feasible; keys are in the env)

Keys present: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`,
`MISTRAL_API_KEY`. **Cheap models for testing** (don't hammer `gpt-5.5`):

| Provider  | Model | configure |
|-----------|-------|-----------|
| OpenAI    | `gpt-5.4-mini` (dots, not the dashed snapshot form) | `(llm/configure :openai {:api-key (env "OPENAI_API_KEY") :default-model "gpt-5.4-mini"})` |
| Anthropic | `claude-haiku-4-5-20251001` | `(llm/configure :anthropic {:api-key (env "ANTHROPIC_API_KEY") :default-model "...")` |
| Gemini    | `gemini-2.5-flash` | `(llm/configure :gemini {:api-key (env "GEMINI_API_KEY") :default-model "gemini-2.5-flash"})` |
| Mistral   | `mistral-small-latest` | `(llm/configure :mistral {:api-key (env "MISTRAL_API_KEY") :default-model "mistral-small-latest"})` |

Write a `/tmp/*.sema` script, build (`cargo build`), run `./target/debug/sema /tmp/x.sema`.
Verify across **all three major families** for serializer-level changes (tool
loop, reasoning-effort) — quirks differ per provider.

Useful live probes:
- **Agent loop**: `deftool` + `defagent` + `agent/run`; a multi-turn task with
  in-memory `set!` state and a `:on-tool-call` trace. Confirm it converges (not
  empty, not max-turns). For stress, use safe in-memory tools only (no
  filesystem/shell/network/repo).
- **Fallback**: `llm/configure :ollama` (Ollama is typically down → hard fail),
  then `(llm/with-fallback [:ollama :mistral] (fn () (llm/complete ...)))` — prove
  `[:ollama]` alone fails and `[:ollama :mistral]` falls through.
- **Caching**: `llm/cache-clear`, two identical calls in `llm/with-cache`; assert
  identical result, `llm/cache-stats` `{:hits 1 :misses 1}`, and
  `(:cost-usd (llm/session-usage))` **unchanged** on the 2nd (hit ⇒ zero usage).
- **Budget**: tiny `{:max-cost-usd ...}` / `{:max-tokens ...}` trips (raises);
  generous completes. Wrap in `(try ... (catch e ...))`.

## Honesty check

If a feature only partially works, fix it or document the limitation — never
leave a doc/homepage claim the code doesn't back (see `docs/plans/archive/2026-06-21-llm-agentic-audit.md`).
Cache hits must report zero usage; canonical `ChatRequest` fields map per-provider
(no provider branching in Sema code or builtins).
