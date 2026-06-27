---
title: Sema for LLM Agents
description: Everything that differs from other Lisps, in one page — for AI coding agents.
aside: false
---

# Sema for LLM Agents

If you already know a Lisp, this page is **everything that's different about Sema** — read it
and you can write correct Sema without ingesting the full reference. It's deliberately
terse. When you need detail, the full per-page docs are indexed at
[`/llms.txt`](/llms.txt) — fetch only the specific `/docs/**/*.md` page you need on demand
(e.g. `/docs/llm/tools-agents.md`). Do **not** load `/llms-full.txt` (the whole-docs
concatenation, ~200k tokens) into context.

## Install & run

```bash
curl -fsSL https://sema-lang.com/install.sh | sh   # or: brew install helgesverre/tap/sema-lang
                                                   # or: cargo install sema-lang
sema script.sema          # run a file
sema -e '(println "hi")'  # eval an expression
sema                      # start the REPL
```

## What Sema is

A **Scheme core** with a **Clojure-flavored surface** and **first-class LLM/agent
primitives**, compiled to a NaN-boxed bytecode VM. **Single-threaded** (reference-counted,
no shared-memory threads). Implemented in Rust; embeddable as a crate; runs in the browser
via WASM.

## Syntax you may not expect

```sema
:keyword                  ; Clojure-style keyword (self-evaluating; also a getter)
{:a 1 :b 2}               ; map literal (sorted; iteration order is deterministic)
[1 2 3]                   ; vector literal (distinct from a list)
(:name person)            ; keywords are functions: same as (get person :name)
#(* % %)                  ; short lambda; %, %1, %2 … are positional args
f"hi ${name}, ${(+ 1 2)}" ; f-string interpolation
#"\d+"                    ; regex literal (raw; no escape doubling)
```

## Naming conventions (the #1 thing to get right)

- **New functions are slash-namespaced:** `file/read`, `path/join`, `string/split`,
  `regex/match?`, `http/get`, `json/encode`. Do **not** guess `read-file` or `split-string`.
- **Predicates end in `?`:** `null?`, `list?`, `empty?`, `file/exists?`.
- **Conversions use `->`:** `string->symbol`, `keyword->string`, `list->vector`.
- **Legacy Scheme names are kept** for a few string ops: `string-append`, `string-length`,
  `string-ref`, `substring` (no `string/` prefix on these).

## Semantics that bite

- **Truthiness:** only `#f` and `nil` are falsy. `0`, `""`, and the empty list `()` are all
  **truthy**. (Unlike Common Lisp, where `()` is false.)
- **Lists are vector-backed**, not cons cells: `Rc<Vec<Value>>`. `nth`/`length` are O(1);
  `cons`/`append` are O(n) copies. `car`/`first` and `cdr`/`rest` exist but it's an array
  underneath — prefer `map`/`filter`/`fold` and `vector` for hot paths.
- **Mutable state is `define` + `set!`** — there is **no** Clojure `atom`/`swap!`/`reset!`.
  ```sema
  (define counter 0)
  (set! counter (+ counter 1))
  ```
- **Two map types:** `{:k v}` literals are sorted `BTreeMap`s (deterministic, usable as keys);
  `(hashmap/new)` is a faster unordered hash map. Access with `(get m :k)` or `(:k m)`.
- **Errors** are raised with `throw` and caught with `try`/`catch`; a caught error is a
  structured map with `:type`, `:message`, `:value` (for user-thrown values), and
  `:stack-trace` (a list of `{:name :file :line :col}` frame maps, innermost first).
- **Equality:** `=` is numeric (`(= 1 1.0)` → `#t`); `eq?`/`equal?` are structural.
- **Definitions & functions:** `define` for bindings; `lambda` (alias `fn`) for anonymous
  functions; `defun`/`defn` are sugar for `(define name (lambda …))`. `let`/`let*`/`letrec`
  for locals.
- **Tail calls are optimized** — deep recursion in tail position won't overflow.

## LLM providers (configure one first)

LLM calls need a provider, and **Sema auto-configures every provider it finds an API key
for** in the environment on startup — so the only setup is exporting a key:

| Provider | Env var | Default model |
| --- | --- | --- |
| Anthropic | `ANTHROPIC_API_KEY` | `claude-sonnet-4-6` |
| OpenAI | `OPENAI_API_KEY` | `gpt-5.5` |
| Google Gemini | `GOOGLE_API_KEY` | `gemini-3.5-flash` |
| Groq · xAI · Mistral · Moonshot | `GROQ_API_KEY` · `XAI_API_KEY` · `MISTRAL_API_KEY` · `MOONSHOT_API_KEY` | per provider |
| Ollama (local, no key) | `OLLAMA_HOST` (default `localhost:11434`) | `gemma4` |

The first configured provider becomes the default. Switch at runtime with
`(llm/set-default :openai)`, force one via `SEMA_CHAT_PROVIDER` / `SEMA_CHAT_MODEL`, or check
the active one with `(llm/current-provider)`. Embeddings use their own providers
(Jina / Voyage / Cohere — see `/docs/llm/embeddings.md`).

> **The #1 first stumble:** a pinned `:model` must belong to the **active** provider.
> `(llm/complete "hi" {:model "gpt-5.5"})` fails with a 404 if Anthropic is the default —
> switch first with `(llm/set-default :openai)`. The simplest call **omits `:model`** and
> uses the active provider's default model.

## What's unique to Sema (why it exists)

LLM/agent operations are language primitives, not a bolted-on SDK:

```sema
;; With an API key in the env this just works — no :model means "active provider's default":
(llm/complete "Summarize this in one sentence." {:max-tokens 100})

(deftool get-weather "Get weather" {:city {:type :string}}
  (lambda (city) (format "{\"temp\": 22}")))
(define bot (agent {:tools [get-weather]}))   ; omit :model to use the default
(agent/run bot "Weather in Oslo?")            ; multi-turn tool loop
```

- **Prompts/messages/conversations** are first-class immutable values (`prompt`, `message`,
  `conversation/*`), not string templates.
- **Structured output:** `llm/extract` (schema-validated) and `llm/classify`.
- **Embeddings + an in-memory vector store** for semantic search / RAG (`llm/embed`,
  `vector-store/*`).
- **Cassettes** record/replay LLM calls to a file for keyless, deterministic tests
  (`llm/with-cassette`).
- **Observability:** built-in OpenTelemetry tracing + metrics (GenAI conventions), off by
  default.
- **Cost & resilience:** budgets (`llm/with-budget`), response caching, fallback chains, and
  retry with backoff — all built in.
- **Concurrency** is a deterministic *cooperative* scheduler (single-threaded): `async`/`await`
  and channels, not OS threads. (Determinism is the same property cassettes give to LLM I/O.)

## Where to look next

- **Index of every page:** [`/llms.txt`](/llms.txt) — fetch a specific `/docs/**/*.md` when
  you need detail (e.g. `/docs/llm/tools-agents.md`, `/docs/stdlib/strings.md`).
- **Everything in one file:** [`/llms-full.txt`](/llms-full.txt) (large — not meant to be
  read whole).
- **Term definitions:** [Glossary](/docs/internals/glossary).
