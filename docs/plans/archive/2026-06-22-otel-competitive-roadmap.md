# OTel GenAI: Competitive Gap Analysis & Best-in-Class Roadmap

**Status:** ✅ **P0 + P1 shipped & tested; P2 deferred — archived 2026-06-23.**
The compat layer (`SEMA_OTEL_COMPAT=openinference,traceloop,langsmith,langfuse,braintrust`)
landed in `crates/sema-otel/src/compat.rs`, wired through `imp.rs`, with the end-to-end
regression suite `crates/sema/tests/otel_compat_test.rs` (+ `otel_compat_off_test.rs`,
`otel_tags_test.rs`). The two P2 spans that *were* auto-derivable (retriever / reranker)
also shipped; the remaining P2 items need new Sema language concepts and are deferred —
tracked in IDEAS.md and the Sema-native tracing scoping doc.
**Date:** 2026-06-22 (research) · 2026-06-23 (closed out)
**Sources:** two parallel research passes — (A) OpenInference (Arize/Phoenix) +
OpenLLMetry (Traceloop); (B) Langfuse, LangSmith, Braintrust, Pydantic Logfire — each
grounded against the live platform docs and Sema's emitter (`crates/sema-otel/src/imp.rs`).

---

## 0. Strategic position

Sema is the **only** of these emitters already on the *current* OTel GenAI semconv
(`gen_ai.*`, schema 1.37, structured-JSON messages, **in-SDK cost computation**, a real
`sema.gen_ai.cache.hit` boolean, deterministic `gen_ai.conversation.id`, VM `vm_span`
cells, retry sub-spans). That makes it **already best-in-class in any vanilla-OTel
backend and in Logfire/Braintrust today**.

The gap is **platform-native namespace keys**: Phoenix/Arize read `openinference.*`,
Traceloop reads `traceloop.*` + indexed `gen_ai.prompt.{i}.*`, Langfuse/LangSmith read
their own `langfuse.*`/`langsmith.*` keys. They largely **ignore** `gen_ai.operation.name`
and the structured `.messages` blobs, so a Sema trace currently renders as "unknown
span / blank I/O" in those UIs.

**The "exceed them" move:** keep `gen_ai.*` as the source of truth and add a thin
**compat layer** (behind `SEMA_OTEL_COMPAT=openinference,traceloop,langsmith`) that
also writes the alias keys — isolated like `provider_map.rs`. Then Sema is the only
emitter that renders natively in **vanilla OTel + Phoenix + Traceloop + Langfuse +
LangSmith + Braintrust + Logfire**, with zero manual instrumentation. All of this is
auto-derivable from data already in scope at the call sites.

---

## 1. Shipped 2026-06-22 (Tier-1, highest leverage)

- `gen_ai.usage.cost` (the key **Langfuse** maps — fixes the observed cost=0) +
  `gen_ai.usage.total_tokens`, alongside the existing `gen_ai.usage.cost_usd`.
- `langfuse.observation.input` / `output` (content-gated) so captured content renders
  on the Langfuse generation.
- `deployment.environment.name` from `SEMA_OTEL_ENVIRONMENT` / `DEPLOYMENT_ENVIRONMENT`
  (Langfuse + Logfire filter on it).
- Scope `schema_url` + descriptive attributes; Resource `service.version` + runtime.

Live-verified against self-hosted Langfuse: cost, total tokens, and input/output now
populate; sessions/users group; gRPC + HTTP both deliver.

---

## 2. Roadmap (prioritized, all auto-emit / no manual instrumentation)

### P0 — Cheap, high-signal, "push one more KeyValue from data in scope"

**SHIPPED 2026-06-22/23** — all five items implemented in `compat.rs` and wired in `imp.rs`:
span-kind tags at every constructor (`compat::span_kind`, LLM/Tool/Agent/Retriever/Reranker/Chain),
generic span I/O (`compat::io` on the LLM span — `input.value`/`output.value` + mime),
tool args/result (`compat::tool_io` ← `set_tool_io` in the tool loop), advertised tool
schemas (`compat::tools` ← `set_tools` from the request's `ToolView`s), and the
`langfuse.trace.input/output` rollup on the agent root (standalone chats are backfilled by
Langfuse from the root observation's `langfuse.observation.input/output`). All asserted
end-to-end in `crates/sema/tests/otel_compat_test.rs`.

| Item | Keys | Where | Notes |
|---|---|---|---|
| **Compat span-kind tagging** | `openinference.span.kind` (`LLM`/`TOOL`/`AGENT`/`EMBEDDING`), `traceloop.span.kind` (`task`/`tool`/`agent`), `langsmith.span.kind=llm` | every span constructor | Single biggest interop win — makes Phoenix/Traceloop/LangSmith classify + cost Sema spans. Gate behind `SEMA_OTEL_COMPAT`. |
| **Generic span I/O** | `input.value`/`output.value` (+ `*.mime_type`), `traceloop.entity.input/output` | `set_messages` | Phoenix/Traceloop key their I/O panes off these; pure relabel of existing JSON, content-gated. |
| **Tool args + result** | `gen_ai.tool.call.arguments`, `gen_ai.tool.call.result` (+ OpenInference `tool_call.function.arguments`) | `run_tool_loop` tool span | Already truncated for the callback; the single most useful agent-debugging datum. Content-gated. |
| **Advertised tool schemas** | `llm.tools.{i}.tool.json_schema` (OpenInference) / `llm.request.functions.{i}.*` | `chat` span in `do_complete` | `tool_schemas` already serialized for the wire request. "Which tools were available this turn." |
| **Trace-level I/O rollup** | `langfuse.trace.input` / `langfuse.trace.output` on the run's root span | agent span / standalone chat | Fills the Langfuse **trace** panel (distinct from observation I/O). Needs first-input/final-output on the root. |

### P1 — Genuine differentiators, modest effort

**SHIPPED 2026-06-22** (verified end-to-end against a live OTel Collector — HTTP/protobuf
+ gRPC — and Jaeger, with real Anthropic calls): TTFT, auto-tags + user `:tags`, metadata
passthrough, LangSmith session / Langfuse release, per-direction cost split, and embedding
model/texts. Deterministic regression test: `crates/sema/tests/otel_tags_test.rs`. **All
P1 items are now done** — the remaining backlog is P2 (each needs a new language feature).

| Item | Keys | Status |
|---|---|---|
| **Time-to-first-token (streaming)** | `sema.gen_ai.server.time_to_first_token` + `sema.gen_ai.is_streaming` (always-on), `langfuse.observation.completion_start_time` (RFC3339), Traceloop `gen_ai.is_streaming` (OpenLLMetry has no per-span TTFT attribute — streaming latency is a histogram metric there) | ✅ `llm/stream` stamps first-chunk time. **Almost no emitter does this.** |
| **Auto-tags** | `langfuse.trace.tags`, `langsmith.span.tags` (CSV), `braintrust.tags` | ✅ provider + model + operation + cache-hit, merged with user `:tags`. |
| **Metadata passthrough** | `langfuse.trace.metadata.*`, `langsmith.metadata.*`, `traceloop.association.properties.*`, `braintrust.metadata` | ✅ `:metadata` map on `llm/complete`/`llm/chat`/`llm/stream`/`agent/run`. |
| **LangSmith session / release** | `langsmith.trace.session_id`, `langfuse.release` (from `SEMA_OTEL_RELEASE`) | ✅ applied to every span when the backend is active. |
| **`llm.invocation_parameters`** | OpenInference consolidated JSON blob | ✅ (shipped earlier with the P0 layer). |
| **Cost split / aliases** | `llm.cost.total`/`prompt`/`completion` (OpenInference), cache-token aliases (`llm.token_count.prompt_details.cache_read`) | ✅ per-direction `llm.cost.prompt`/`.completion` now ship alongside `.total` + the cache-read alias. |
| **Embedding detail** | `embedding.model_name`, texts (gated), `openinference.span.kind=EMBEDDING` | ✅ named model + (content-gated, capped) input texts now ship alongside the span-kind. |

### P2 — Needs a new Sema concept; roadmap notes only

- **Prompt templates / registry** (`llm.prompt_template.*`, `traceloop.prompt.*`): only
  feasible if Sema adds first-class prompt templating. Sema's f-strings (`f"...${x}..."`)
  are *exactly* a template + bound vars — a natural future fit. Track with the
  `defprompt`/structured-output idea in IDEAS.md.
- **Retriever / reranker / vector-DB spans** (`retrieval.documents.*`): ✅ **SHIPPED
  2026-06-22.** `vector-store/search` emits an OpenInference `RETRIEVER` span; the new
  `llm/rerank` (Cohere/Jina/Voyage cross-encoder) emits a `RERANKER` span
  (`reranker.query`/`model_name`/`top_k`/`{input,output}_documents.*`). Completes the RAG
  story (`llm/embed` + `vector-store/*` + `llm/rerank` + `llm/complete`); see
  `examples/llm/rag-docs-search.sema` + `website/docs/llm/rag.md`.
- **Scores / evaluations** (`braintrust.scores`, Langfuse/LangSmith score APIs): not
  auto-derivable; a manual `(otel/score ...)` surface — see the Sema-native tracing
  scoping doc + the deferred evals item.
- **Content-capture granularity**: OpenInference has ~14 `HIDE_*` flags; Sema's single
  toggle matches OpenLLMetry and is fine. Add one embedding-vector suppression flag only
  if embedding-vector capture lands.

---

## 3. Trace structure: flat siblings is CORRECT

Earlier concern (tool spans flat under `invoke_agent`, siblings of `chat`, vs nested
under the requesting `chat`): the research confirms **both OpenInference and OpenLLMetry
also make the tool a child of the agent span, sibling to the LLM span** — the model's
tool-call *request* is not its own span. So Sema's flat structure **matches the
convention**; no change needed. (Langfuse rendered our long multi-turn run correctly as
AGENT → GENERATION×N + TOOL×M in temporal order.)

---

## 4. Recommended next slice

If we pursue this: ship the **P0 compat layer** behind `SEMA_OTEL_COMPAT` (span-kind
tags + `input/output.value` + tool args/result + advertised tool schemas) as one
focused milestone — it's all relabeling/forwarding of in-scope data and turns
currently-blank Sema traces into fully-populated ones across Phoenix, Traceloop, and
LangSmith simultaneously. Then P1 TTFT + auto-tags as the differentiators. Decide
whether the compat layer is opt-in (flag) or always-on (attribute bloat vs zero-config).
