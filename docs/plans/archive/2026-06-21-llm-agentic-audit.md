# Sema LLM / Agentic Features — Master Audit

**Date:** 2026-06-21
**Scope:** The `sema-llm` crate, the language-level LLM/agentic surface (special forms + builtins), public claims (homepage/README/docs), and test/verification coverage.
**Method:** Code inspection + offline unit-test review + **live end-to-end API calls** against three real providers (OpenAI, Anthropic, Gemini) using the release binary. Ollama was not running and could not be live-verified.

---

## 1. Executive Verdict

**The LLM/agentic features are real, substantial, and live-verified — not vaporware.** `sema-llm` is a genuinely working LLM engine: a sync `LlmProvider` trait hides async via per-provider tokio runtimes, and chat / streaming / batch / embeddings are really implemented for Anthropic, OpenAI (+ OpenAI-compatible), Gemini, and Ollama. Across the three providers with live keys the audit confirmed working completion, multi-message chat, streaming, structured extraction with validate+reask, embeddings, vector search, concurrent batch/pmap, **exact** cost tracking (hand-verified to f64 precision), and budget enforcement — plus a genuine multi-round agent tool loop that re-enters Sema and executes Sema-defined tools. The umbrella homepage claim — *"the scaffolding is the runtime"* — substantially holds.

**Biggest risk — one correctness bug undercuts the headline agent promise.** The tool loop feeds results back as plain `[Tool result for X]: ...` user-text and never echoes the assistant's `tool_calls` with a `tool_call_id`. On **OpenAI gpt-4o-mini the same agent re-called its tool 5×, hit max-turns, and returned an EMPTY response.** The agent loop works on Anthropic but is **broken for OpenAI-family tool agents**, despite docs framing it as universal (`builtins.rs:4501-4504`, `types.rs:69-73`, `openai.rs:77`).

**Other top risks:**
- **Zero deterministic CI coverage** of any real LLM/agent path — there is no mock/fake provider. Every live path is `#[ignore]`'d or excluded from CI by omission. This is exactly what let the OpenAI breakage ship undetected.
- **Thin resilience** — retry fires only on HTTP 429 (max 3, no backoff/jitter), with no 5xx/network/timeout retry, despite docs claiming *"exponential backoff"* (`builtins.rs:4241-4259`).
- **Streaming silently bypasses** cache, budget, fallback, and rate-limit (`builtins.rs:1521-1553`).
- **Two factual/accuracy errors in docs**: homepage says *"a bytecode VM and a tree-walker"* (the tree-walker was retired) at `website/sema-homepage-v2.html:545`; README says pricing is *"dynamic ... from llm-prices.com"* when it is an embedded static models.dev snapshot (`pricing.rs:23-185`).

**Bottom line:** Fix the tool-result protocol and add a mock provider with CI tests, and the thesis genuinely holds. Everything else is hardening and honesty edits.

---

## 2. Inventory — What Exists

### 2.1 Provider / Core Engine (`sema-llm`)

| Feature | Kind | Location | Status |
|---|---|---|---|
| `LlmProvider` trait (sync, async-hidden) | type | `provider.rs:4-32` | real |
| `ProviderRegistry` (named + default + embedding slot) | type | `provider.rs:35-91` | real |
| Sync-over-async `BlockingRuntime` | infra | `http.rs:17-48`; per-provider `block_on` | real |
| `create_client` / `DEFAULT_TIMEOUT` (120s, hardcoded) | infra | `http.rs:6,52-60` | real (timeout arg dead) |
| AnthropicProvider — chat | provider | `anthropic.rs:74-145` | real |
| AnthropicProvider — streaming | provider | `anthropic.rs:147-241` | partial (drops tool_use deltas) |
| Anthropic 429 handling (hardcoded 5000ms) | infra | `anthropic.rs:89-93,168-172` | partial |
| OpenAiProvider — chat + JSON mode + tools | provider | `openai.rs:114-191` | real |
| OpenAiProvider — streaming | provider | `openai.rs:193-284` | partial (drops tool_call deltas) |
| OpenAiProvider — embeddings | provider | `openai.rs:286-352` | real |
| OpenAI-compatible reuse (groq/xai/mistral/moonshot/unknown) | provider | `openai.rs:34-51`; `builtins.rs:867-1009` | real |
| GeminiProvider — chat + tools + images | provider | `gemini.rs:197-347` | real |
| GeminiProvider — streaming (accumulates tool_calls) | provider | `gemini.rs:86-195` | real |
| Gemini URL/SSRF + path-injection guard | infra | `gemini.rs:8-18,406-437` | real |
| OllamaProvider — chat/tools/images/streaming | provider | `ollama.rs:47-327` | real (unverified live) |
| SSE / NDJSON stream parsers | infra | `sse.rs:7-48`; `ndjson.rs:7-45` | real |
| OpenAI-compat / Cohere embedding providers | provider | `embeddings.rs:6-220` | real |
| Pricing lookup (embedded models.dev snapshot) | infra | `pricing.rs:23-185` | real |
| Cost calculation (per-serving-provider) | infra | `pricing.rs:152-165`; `builtins.rs:205-279` | real |
| Vector store (in-memory + JSON + cosine) | infra | `vector_store.rs:1-254` | real |
| `do_complete` dispatch (cache → fallback → rate-limit retry) | infra | `builtins.rs:4101-4261` | real |
| Retry / backoff (429-only, fixed) | infra | `builtins.rs:4210-4259` | partial |
| Lisp-defined providers | infra | `builtins.rs:447-656` | real |
| SSRF guard for provider base-url (sandboxed) | infra | `builtins.rs:696-811` | real |
| Token counting (chars/4 estimate) | infra | `builtins.rs:3406-3441` | partial |

### 2.2 Agentic Surface (special forms + agent loop)

| Feature | Kind | Location | Status |
|---|---|---|---|
| `prompt` special form | special-form | `lower.rs:161,1643`; runtime `eval.rs:778` | real |
| `message` special form | special-form | `lower.rs:162,1669`; runtime `eval.rs:840` | real |
| `deftool` special form | special-form | `lower.rs:163,1684`; `special_forms.rs:65` | real |
| `defagent` special form | special-form | `lower.rs:164,1700`; `special_forms.rs:88` | real |
| `agent/run` (primary agent entry) | builtin | `builtins.rs:2096` | real (broken on OpenAI) |
| `llm/chat` with `:tools` (tool loop) | builtin | `builtins.rs:1386` | real (broken on OpenAI) |
| `run_tool_loop` (agent loop core) | infra | `builtins.rs:4425` | partial |
| `execute_tool_call` (dispatch back into Sema) | infra | `builtins.rs:4516` | real |
| `call_value_fn` / `full_eval` / `EVAL_FN` re-entry | infra | `builtins.rs:4591,142`; wired `eval.rs:69` | real |
| `__vm-prompt/-message/-deftool/-defagent` natives | builtin | `eval.rs:778,840,881,905` | real |
| agent/tool accessors + predicates | builtin | `builtins.rs:2713-2799` | real |
| LLM/agent prelude macros | macro | `prelude.rs` | **absent** (none exist) |

### 2.3 Structured / Auxiliary

| Feature | Kind | Location | Status |
|---|---|---|---|
| `llm/complete` / `send` | builtin | `builtins.rs:1344,1458` | real |
| `llm/stream` | builtin | `builtins.rs:1470` | real (bypasses dispatch) |
| `llm/extract` (validate+reask loop) | builtin | `builtins.rs:1565` | real |
| `llm/extract-from-image` (vision) | builtin | `builtins.rs:1666` | real (vision path unverified) |
| `llm/classify` / `summarize` / `compare` | builtin | `builtins.rs:1753,3701,3743` | real |
| `llm/pmap` / `llm/batch` (parallel) | builtin | `builtins.rs:2164,2238` | real |
| `llm/embed` / `similarity` / `embedding/*` | builtin | `builtins.rs:2403-2631` | real |
| `vector-store/*` / `vector/*` | builtin | `builtins.rs:3445-3680` | real |
| cost/usage tracking (`llm/last-usage`, session) | builtin | `builtins.rs:2042-2092` | real |
| budgets (`llm/with-budget`) | builtin | `builtins.rs:282-335,3241` | real (post-call) |
| cache / rate-limit / fallback (`llm/with-*`) | builtin | `builtins.rs:3273-3383,3680` | real |
| auto-configuration from env | builtin | `builtins.rs:813-1341` | real |
| conversation / prompt / message families | builtin | `builtins.rs:1809-3100` | real |

### 2.4 Language Surface (counts)

~90+ user-callable LLM/agent functions registered in `builtins.rs`. Four LLM special forms (`prompt`, `message`, `deftool`, `defagent`) lowered in `sema-vm/src/lower.rs`. **No** LLM/agent macros in the prelude (`prelude.rs` defines only generic threading/binding/loop macros). `defmulti`/`defmethod` are general multimethods, unrelated to agents.

---

## 3. Verification Status — What ACTUALLY Works

### 3.1 Live-verified (real API calls, OpenAI / Anthropic / Gemini)

| Feature | Verdict | Evidence |
|---|---|---|
| Provider construction / auto-configure / selection | live-verified | `(llm/auto-configure)` → 11 providers; `(llm/current-provider)` → `{:model "claude-sonnet-4-6" :name :anthropic}` |
| `llm/complete` | live-verified | OpenAI/Anthropic/Gemini → `pong`. Note: bare `(complete ...)` is **unbound** — only `llm/complete` exists |
| `llm/chat` multi-message | live-verified | `[(message :system ...) (message :user "Capital of France?")]` → `Paris` (OpenAI) |
| `llm/stream` incremental SSE | live-verified | OpenAI/Anthropic/Gemini delivered separate chunks (real incremental SSE) |
| Error surfacing | live-verified | bad key → `401 invalid_api_key`; bad model → `404 ... does not exist`, propagated via `SemaError::Llm` |
| Cost/usage tracking | live-verified | `(llm/last-usage)` → exact tokens + pricing-derived cost; hand-checked `(43*0.15 + 1*0.60)/1e6 = 7.05e-6` ✓ |
| `deftool` / `defagent` registration | live-verified | `tool?`/`agent?` → `#t`; accessors return correct metadata |
| **Agent loop + tool dispatch (Anthropic)** | live-verified | claude-haiku-4-5: tool printed `>>> TOOL EXECUTED IN SEMA for name=Alice`, value 4273 fed back, model produced "The magic number for Alice is 4273" |
| `llm/chat` tool loop (Anthropic) | live-verified | `>>> SEMA add 17 + 25` → "result ... is 42" |
| on-tool-call event callbacks | live-verified | Sema lambda printed `:event start/end`, `:tool magic-number` per call |
| Multi-turn conversation state | live-verified | turn 2 recalled "teal" from turn 1 |
| Conversation immutability / fork | live-verified | c1 stayed 2 msgs while c2 grew to 4; `conversation/fork` returned independent 4-msg copy |
| `llm/classify` | live-verified | OpenAI → `:positive` (keyword). **NOTE:** real signature is `(categories text opts)`, not homepage's `{:labels ...}` |
| `llm/extract` (schema + validate/reask) | live-verified | `{:name :string :age :number}` on "John Smith is 42..." → `{:age 42 (:int) :name "John Smith" (:string)}` |
| `llm/batch` (concurrency) | live-verified | 4 prompts: 5597ms sequential vs 1820ms batched (~3×) via `join_all` |
| `llm/pmap` | live-verified | inherits batch concurrency |
| `llm/embed` + `llm/similarity` | live-verified | sim(cat,feline)=0.81 vs sim(cat,physics)=0.25 (semantically correct) |
| vector store (create/add/search) | live-verified | top hit doc1 score 0.808, ranked over physics |
| `llm/with-budget` enforcement | live-verified | `{:max-cost-usd 0.0000001}` → caught "budget exceeded", 0 follow-on calls |

### 3.2 Broken (live-verified failure)

| Feature | Verdict | Evidence |
|---|---|---|
| **Agent loop on OpenAI-family** | **broken** | gpt-4o-mini: identical agent repeated the SAME tool call 5× (5 start/end events), hit max-turns=5, returned **EMPTY** response. Root cause: tool results pushed as `ChatMessage::new("user", "[Tool result for X]: ...")` (`builtins.rs:4501-4504`); `ChatMessage` has only role+content, no `tool_call_id` (`types.rs:70-73`); `openai.rs:77` sets `tool_calls:None` on every history msg and drops the assistant tool_call. OpenAI never sees a correlated `role:tool` result → re-calls forever. Same code works on Anthropic |

### 3.3 Offline-verified (code + unit tests, no live call)

| Feature | Verdict | Evidence |
|---|---|---|
| Retry / timeout behavior | offline-verified | `builtins.rs:4241-4259`: only on `RateLimited`, max 3, no backoff/jitter, no 5xx/network retry; timeout hardcoded via `create_client(None)`. Live-observed Gemini 429 → "rate limited after 3 retries" confirms loop runs |
| Tool result always stringified | offline-verified | `execute_tool_call` returns String; maps/seqs JSON-encoded (`builtins.rs:4539-4549`) |
| `call_value_fn` lambda re-evals body via `full_eval` | offline-verified | `builtins.rs:4595-4647` — matches documented stdlib-HOF fallback limitation |
| SSRF guard, rate-limiter clock safety, pricing, vector math | offline-verified | Strong inline unit tests (`builtins.rs:4697-4815`; `pricing.rs:241-280`; `vector_store.rs:265-357`) |
| Gemini build_url key-omission / path-injection | offline-verified | `gemini.rs:406-440` |
| Arg marshalling, extraction validation, fallback parsing | offline-verified | `builtins.rs:4837-5107` |

### 3.4 Structural-only (code exists, no real exercise)

| Feature | Verdict | Evidence |
|---|---|---|
| `llm/extract-from-image` vision happy-path | structural-only | Code reads file/bytevector, detects media type, base64 (`builtins.rs:1666`); only `#[ignore]`'d key-gated tests; not exercised live. Offline error-path tests (arity, bad path) DO pass |
| MCP `deftool` exposure to a real LLM | structural-only | MCP e2e tests real (`mcp_e2e_test.rs`) but no test drives an LLM through MCP to invoke a deftool tool |

### 3.5 Unverified

| Feature | Verdict | Evidence |
|---|---|---|
| Ollama (complete/chat/stream/tools) | unverified | Daemon not running; code path exists (`ollama.rs:47-339`), no live call possible, no mock |
| Gemini tool loop | unverified | Key present but agent loop not exercised on Gemini |

---

## 4. Claims vs Reality

| Claim (location) | Reality | Verdict |
|---|---|---|
| "Stop rewriting the agent loop ... Sema makes that scaffolding the runtime" (`sema-homepage-v2.html:6-7,260-266`; `README.md:16`) | Completion/chat/streaming/conversations/agent loop/cost/budget/cache/fallback/rate-limit all real and live-verified. Caveats: agent loop broken on OpenAI; resilience thinner than implied | **partially-holds** |
| "Agents ... handle the back-and-forth of tool calls automatically" / "Full coding agent" (`tools-agents.md:62-128`; `README.md:44-55`) | Proven live on Anthropic. On OpenAI the agent re-calls forever (empty result). Not universal as framed | **partially-holds** |
| "(llm/with-budget {:max-cost-usd 1.00} f) — hard spend cap" (`sema-homepage-v2.html:464,494`) | Hard cap on **continuation**: the tripping request is sent, then `track_usage` aborts. Not pre-emptive; a batch can overshoot; streaming bypasses it entirely | **partially-holds** |
| "Cost-aware — ... dynamic pricing from llm-prices.com" (`README.md:340`) | Cost math exact, but pricing is an **embedded static** models.dev snapshot (`include_str!`, updated per release). Not fetched live; source is models.dev not llm-prices.com | **oversold** |
| "retries with backoff" / "generic retry with exponential backoff" (`sema-homepage-v2.html:379`; `docs/llm/index.md:57-59`) | Retry only on HTTP 429, max 3, fixed wait, **no exponential backoff, no jitter, no 5xx/network retry** | **oversold** |
| "(llm/extract ...) — typed maps back, not strings" (`sema-homepage-v2.html:497`; `README.md:64-78`) | Live-verified typed map with validate+reask retry | **holds** |
| "(llm/with-cache ...) — response cache" (`sema-homepage-v2.html:464,495`) | Real memory+disk SHA256-keyed cache, TTL, well unit-tested. Caveat: streaming bypasses cache | **holds** |
| "(llm/with-fallback [...]) — provider failover" (`sema-homepage-v2.html:465,496`) | Real chain, tries each on error, multiple entry forms. Caveat: fallback path skips rate-limit; streaming bypasses | **holds** |
| "Eleven providers ... from environment variables" (`sema-homepage-v2.html:502`) | Live-verified: 11 providers registered from env | **holds** |
| "(llm/pmap ...) — parallel batch" (`sema-homepage-v2.html:499`) | Live-verified ~3× concurrency via `join_all` | **holds** |
| "A conversation is an immutable value you can fork, diff, and inspect" (`sema-homepage-v2.html:477-480`) | fork/inspect/immutability/multi-turn live-verified; "diff" not exercised | **holds** |
| Embeddings + vector store + semantic search (`docs/llm/index.md:41-51`) | Live-verified semantic ordering + ranked search; dim-mismatch errors | **holds** |
| "No JIT. A bytecode VM and a tree-walker." (`sema-homepage-v2.html:545`) | **FACTUAL ERROR** — tree-walker was retired; VM is sole evaluator. `README.md:352` is correct. Damaging because it's in the "honest" section | **oversold** |
| Streaming across all providers (`README.md:80-86`) | Text streaming live-verified (OpenAI/Anthropic/Gemini). But Anthropic/OpenAI streaming **drop tool_use/tool_call deltas**; only Gemini/Ollama accumulate | **partially-holds** |
| `(llm/classify {:labels [...]} ...)` (`sema-homepage-v2.html:430`) | Works, returns keyword, but real signature is `(categories text opts)` — map form doesn't match API | **partially-holds** |
| Vision extraction `llm/extract-from-image` (`README.md:88-96`) | Code exists, offline error-paths pass, vision happy-path only `#[ignore]`'d; not confirmed live | **unverifiable** |
| MCP exposes your `deftool` tools to Claude/Cursor (`README.md:304`; `mcp.md:45`) | MCP server e2e tests real, but no test drives an actual LLM to invoke a deftool tool | **partially-holds** |

---

## 5. Gap Analysis (prioritized)

| # | Gap | Severity | Effort | Evidence |
|---|---|---|---|---|
| 1 | **Agent tool loop broken on OpenAI-family** — tool results sent as plain user text, no `tool_call_id`, no assistant `tool_calls` echo | **critical** | medium | `builtins.rs:4501-4504`, `types.rs:70-73`, `openai.rs:77`; live: 5× repeat, empty response |
| 2 | **No deterministic CI coverage of any real LLM/agent path — no mock provider exists** | **critical** | medium | `provider.rs:4` (trivially mockable, no impl); live tests all `#[ignore]`'d; `run-examples.sh` skips llm dirs |
| 3 | **Tool error aborts the entire agent run** (propagated via `?`); no per-call input validation | high | medium | `execute_tool_call` `?` at `builtins.rs:4537`; no schema validation before handler |
| 4 | **Streaming bypasses cache/budget/fallback/rate-limit**; Anthropic/OpenAI streaming drop tool deltas | high | medium | `builtins.rs:1521-1553`; `anthropic.rs:147-241`, `openai.rs:193-284` |
| 5 | **Thin resilience** — retry only on 429, no 5xx/network/timeout, no backoff/jitter; per-call timeout dead | high | medium | `builtins.rs:4241-4259`; `create_client(None)` at `http.rs:52-60` |
| 6 | **Homepage factual error** — "bytecode VM and a tree-walker" | medium | low | `sema-homepage-v2.html:545` (verified verbatim) |
| 7 | **"Dynamic pricing from llm-prices.com" overstates** a bundled static snapshot | medium | low | `README.md:340` vs `pricing.rs:1-8,23-185` |
| 8 | **No structured-output schema/repair beyond `llm/extract`**; agent/tool returns are strings only | medium | medium | `json_mode` bare bool at `types.rs:108`; `execute_tool_call` stringifies (`builtins.rs:4539-4549`) |
| 9 | **No observability/tracing** of LLM or tool calls (cost/latency/tokens invisible per-step) | medium | medium | only `on_tool_call` truncated preview + aggregate usage; `duration_ms` measured but not exported |
| 10 | **`llm/classify` signature mismatch** with homepage/docs | low | low | `builtins.rs:1757-1763` vs `sema-homepage-v2.html:430` |
| 11 | **Gemini thinking-model empty-output footgun** under small `max-tokens` | low | low | `gemini.rs:34`; live: empty at 10, `pong` at 200 |
| 12 | **Budget enforcement is post-call**; a single batch can overshoot before the cap fires | low | low | `track_usage` at `builtins.rs:243-260`; batch fires concurrently then accounts |

### Recommended sequence

1. **Fix the tool-result protocol** (gap 1): add `tool_call_id` + echo assistant `tool_calls`, send provider-native `tool_result` (Anthropic) / `role:tool` (OpenAI). `ToolCall.id` is already parsed — fix is localized to `run_tool_loop` + per-provider serializers.
2. **Build a `FakeProvider` mock** (gap 2) and convert the key `#[ignore]`'d tests to deterministic CI covering both Anthropic-shaped and OpenAI-shaped protocols, extract reask, fallback, cache, budget. Land alongside fix 1 to lock it in.
3. **Make tool errors recoverable** (gap 3) — feed failures back into the loop, reuse the existing validate/reask machinery at the tool boundary.
4. **Fix the two accuracy errors** (gaps 6, 7) and the classify signature (gap 10) — trivial, credibility-damaging.
5. **Harden resilience** (gap 5) — 5xx/network/timeout retry with backoff+jitter; read Anthropic's real `retry-after`.
6. **Route streaming through the dispatch layer** (gap 4) or document the bypass; accumulate streamed tool deltas.
7. **Add a structured-output primitive** (gap 8) generalizing the extract machinery.
8. **Add opt-in OpenTelemetry GenAI tracing** (gap 9) at the single `sema-llm` chokepoint.

---

## 6. Test / Verification Debt

**What is genuinely well-covered offline (deterministic, no keys/network):**
- Tool-call arg ordering & dispatch (`builtins.rs:4837-4986`)
- Extraction validation + reask formatting (`builtins.rs:4988-5053`)
- Fallback-entry parsing (`builtins.rs:5055-5107`)
- SSRF guard incl. inet_aton encodings (`builtins.rs:4718-4815`)
- Rate-limiter backward-clock safety (`builtins.rs:4697-4716`)
- Pricing/cost lookup (`pricing.rs:241-280`)
- Vector store: cosine, CRUD, JSON roundtrip, dim-mismatch (`vector_store.rs:265-357`, 24 tests)
- Gemini build_url (`gemini.rs:406-440`)
- Request/response (de)serialization (`types.rs:183-312`)
- Prompt/message/conversation/tool/agent **constructors** via eval (`eval_stdlib_test.rs:98-167`)

**What has NO real automated coverage:**

| Area | State | Location |
|---|---|---|
| **Mock/fake LLM provider** | **does not exist** — the single biggest gap | `provider.rs:4` (trait present, no test impl); no wiremock/mockito/httptest anywhere |
| `llm/complete` real round-trip | `#[ignore]`'d, key-gated | `llm_test.rs:13-22` |
| `llm/extract` live round-trip (basic/validate/optional/message) | 4× `#[ignore]`'d | `llm_test.rs:26-103` |
| Vision happy-path (`extract-from-image` + chat) | 2× `#[ignore]`'d | `llm_test.rs:107-136` |
| Agent tool-loop end-to-end | no test without keys; no mock | — |
| `examples/llm/*`, `ai-tools/*`, `providers/*`, `pi-sema/*` | excluded from CI by omission | `run-examples.sh:51-64` |
| Offline-capable LLM examples (prompt-builder, conversation-patterns, test-vector-store, test-text-tools) | make ZERO real calls but never run by CI — **missed free coverage** | `examples/llm/` |
| Website doc LLM/agent code samples | **none run**; doc runner is `#[ignore]`'d and `SKIP_MARKERS` drop `llm/`, `prompt`, `agent`, `conversation`, `embedding`, `tool/`, `message` | `doc_examples_test.rs:14-40` |
| Notebook LLM cells | none exist; Playwright E2E covers no LLM cell | `examples/notebook/demo.sema-nb` |

**Debt headline:** every code path that actually performs an LLM completion, chat, extraction, embedding, agent tool-loop, streaming, or batch is verified ONLY by `#[ignore]`'d key-gated tests or CI-excluded example scripts. The deterministic parts are well-tested; the end-to-end agentic behavior has **zero** deterministic automated verification. Adding a `FakeProvider` (gap 2) is the lever that closes most of this debt at once.
