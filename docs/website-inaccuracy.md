# Website feature page inaccuracies

Tracked factual mismatches between the marketing feature pages (`website/feature/*.md` + their Vue components) and the actual Sema implementation. Most issues have been applied; the tables below include a `Status` column.

---

## Observability page

| Claim | Actual behavior | Evidence | Minimal fix | Status |
|---|---|---|---|---|
| Mockup shows `sema.gen_ai.cache.hit` = `false` | Attribute is only emitted on cache hits; omitted on miss. | `crates/sema-otel/src/imp.rs:1113-1117` | Change mock value to `true` (a hit), or remove the attribute from the mock. | ✅ Fixed |
| "Telemetry is sent in the background… A slow or dead backend can’t delay or crash your script." | True for OTLP (`BatchSpanProcessor`), but the `SEMA_OTEL_FILE` backend writes spans synchronously. | `crates/sema-otel/src/imp.rs:496-499`, `:520-544` | Qualify the bullet: "OTLP backend sends telemetry in the background…" and give the file backend its own "writes locally, no network" bullet. | ✅ Fixed |
| Model name `claude-sonnet-4` in trace mockups | Canonical Anthropic default is `claude-sonnet-4-6`. | `crates/sema-llm/src/anthropic.rs:18`, `website/docs/llm/providers.md:256` | Update mock model strings to `claude-sonnet-4-6`. | ✅ Fixed |

---

## Agents page

| Claim | Actual behavior | Evidence | Minimal fix | Status |
|---|---|---|---|---|
| "429s and 5xx retried automatically, up to **3 attempts**" | Default is 3 retries, so up to **4 attempts** total. | `crates/sema-llm/src/builtins.rs:5700-5701`, `:5784-5806` | Change "3 attempts" to "3 retries". | ✅ Fixed |
| `:max-turns` presented as part of the agent config | `:max-turns` is optional and defaults to `10`. | `crates/sema-eval/src/special_forms.rs:111-114` | Add "(optional, defaults to 10)" in the agent config description. | ✅ Fixed |
| "The tool loop, retries with backoff, **rate limiting**, and cost tracking live in the runtime" | Rate limiting is only active inside an explicit `(llm/with-rate-limit rps f)` wrapper. | `crates/sema-llm/src/builtins.rs:4345-4362` | Change "rate limiting" to "optional rate limiting" or list it with the scoped forms below. | ✅ Fixed (rate limiting removed from that sentence in the comparison pane) |
| Model name `claude-sonnet-4` in examples | Canonical Anthropic default is `claude-sonnet-4-6`. | `crates/sema-llm/src/anthropic.rs:18`, `website/docs/llm/providers.md:256` | Update example model strings to `claude-sonnet-4-6`. | ✅ Fixed |

---

## Cassettes page

| Claim | Actual behavior | Evidence | Minimal fix | Status |
|---|---|---|---|---|
| Terminal examples use the command **`semal`** | The installed binary is **`sema`**. | `crates/sema/Cargo.toml:2,14` | Replace `semal` with `sema`. | ✅ Fixed |
| "Run **`sema test`** with zero secrets." | There is no `test` subcommand; scripts run positionally. | `crates/sema/src/main.rs:161-328` | Change to a positional file path like `sema test/agents.sema`. | ✅ Fixed (now shown as `sema test/agents.sema`, which is a valid positional file argument) |
| CTA: "Wrap your LLM calls in **`with-cassette`**" | The actual builtin is **`llm/with-cassette`**. | `crates/sema-llm/src/builtins.rs:3940` | Change to `llm/with-cassette`. | ✅ Fixed |
| "Versioned. A `"v":1` field lets old tapes be migrated if the shape ever changes." | `v:1` is written, but no migration logic exists. | `crates/sema-llm/src/cassette.rs:52` | Softened wording: "Versioned with a `v` field for future format changes." | ✅ Fixed |

---

## Notebook page

| Claim | Actual behavior | Evidence | Minimal fix | Status |
|---|---|---|---|---|
| "LLM outputs with **cost** and timing" and mock output `$0.0021` | Timing is shown; `cost_usd` is always `None`. | `crates/sema-notebook/src/engine.rs:160,224,237` | Drop "cost" from the lede and remove `$0.0021` from mock outputs, leaving timing only. | ✅ Fixed |
| REST `POST /api/cells/{id}/eval` returns `{ "id", "outputs": [...] }` | Returns `{ "id", "output", "stdout", "can_undo" }` with a singular `output`. | `crates/sema-notebook/src/render.rs:163-172`, `server.rs:239-249` | Update mock JSON to the real shape. | ✅ Fixed |
| Linked docs `/docs/notebook#rest-api`: `POST /api/undo` — "Undo the last cell edit/delete" | `/api/undo` rolls back the last **evaluation** snapshot, not edits/deletes. | `crates/sema-notebook/src/engine.rs:305-337`, `:119-137` | Fix the linked docs entry; the feature page text is already correct. | ✅ Fixed |
| "Evaluate **all cells**" / mock "evaluating 6 cells…" | Only code cells are evaluated; markdown cells are skipped. | `crates/sema-notebook/src/engine.rs:245-264` | Change to "evaluate all code cells" and adjust the mock count to match code cells only. | ✅ Fixed |

---

## What is Sema page

| Claim | Actual behavior | Evidence | Minimal fix | Status |
|---|---|---|---|---|
| "**17 crates**" (hero and architecture) | Workspace has **16** crates. | `Cargo.toml:3-19` | Change "17 crates" to "16 crates" everywhere on the page. | ✅ Fixed |
| "v1.27" | Workspace version is **1.27.1**. | `Cargo.toml:24` | Change to "v1.27.1". | ✅ Fixed |
| "~116k lines" | `tokei` reports ~125k lines of Rust. | `tokei crates --types Rust` | Update to "~125k lines". | ✅ Fixed |
| "Eleven providers… Anthropic, OpenAI, Gemini, Groq, xAI, Mistral, Moonshot, Ollama" | Lists 8 chat providers; omits 3 embedding-only providers (`jina`, `voyage`, `cohere`). | `crates/sema-llm/src/builtins.rs:1012-1215`, `:1263-1552` | Either list all 11 or change the claim to "Eight chat providers plus embedding providers". | ✅ Fixed |
| Caught errors are maps with `:type`, `:message`, **`:stack-trace`** | Error maps contain `:type`, `:message`; user throws add `:value`. No `:stack-trace`. | `crates/sema-vm/src/vm.rs:3682-3722` | Remove `:stack-trace` from the list. | ✅ Fixed |
| NaN-boxed `Value` visual: "tag **13 bits** / payload **48 bits**" | Layout is **6-bit tag / 45-bit payload**. | `crates/sema-core/src/value.rs:493-515` | Update visual labels to "6-bit tag / 45-bit payload". | ✅ Fixed |
| Data types list includes **"F-String"** as a scalar runtime type | F-strings are reader syntax that compile to ordinary strings. | `crates/sema-reader/src/reader.rs:489-576`, `crates/sema-core/src/value.rs` | Remove "F-String" from the runtime type grid; mention it under the Clojure-surface list instead. | ✅ Fixed |

---

## Related doc-site fixes applied

These were not on the feature pages themselves but repeated the same inaccurate claims:

| File | Change |
|---|---|
| `website/docs/llm/resilience.md` | "3 attempts" → "3 retries" |
| `website/docs/internals/architecture.md` | "~116k lines of Rust across 15 crates" → "~125k lines of Rust across 16 crates" |
| `website/docs/for-agents.md` | `:stack-trace` → `:value` for user-thrown values |
| `website/docs/language/special-forms.md` | `:type`, `:message`, `:stack-trace` → `:type`, `:message` (plus `:value` for user throws) |
| `website/docs/internals/glossary.md` | `:stack-trace` → `:value`; added note that VM stack-trace parity is deferred |
| `website/.vitepress/theme/CustomHome.vue` | `:stack-trace` → `:value` for user-thrown values |

---

## Design-preserving correction principles

1. **Do not redesign sections.** Each fix above is a word swap, a number update, or the removal of an unsupported detail. No new sections or layout changes are required.
2. **Keep mock data plausible.** Where a mock value is wrong, replace it with a value the implementation would actually produce (e.g. `sema.gen_ai.cache.hit` → `true`, or remove it).
3. **Prefer dropping unsupported details over adding caveats.** Marketing copy reads better when an unsupported claim is removed entirely rather than qualified with a footnote.
4. **Link to canonical docs.** For notebook API shape and undo behavior, point readers to the canonical docs page and keep the feature page prose at the higher level.
5. **Sync numbers across the site.** Version, crate count, line count, and provider count should be single-sourced from `Cargo.toml` and `crates/sema-llm/src/builtins.rs` to avoid the same drift recurring.
