---
outline: [2, 3]
---

# Embeddings & Similarity

Generate vector embeddings from text and compute similarity between them. On startup, `(llm/auto-configure)` registers available providers and selects the first
in this order: Jina, Voyage, Cohere, Nomic, OpenAI, Mistral, Fireworks, Together, Gemini,
OpenRouter, DeepInfra, Azure, Cloudflare, Alibaba, TEI, then local Ollama.
Incomplete account configurations are skipped unless explicitly selected.
Set `SEMA_EMBEDDING_PROVIDER` / `SEMA_EMBEDDING_MODEL` or
`--embedding-provider` / `--embedding-model` to override that selection.
Reranking has independent `SEMA_RERANK_PROVIDER` / `SEMA_RERANK_MODEL` and
`--rerank-provider` / `--rerank-model` overrides.

## Configuration

### `llm/configure-embeddings`

Configure a dedicated embedding provider separately from the chat provider — so you can use
one provider for chat and another for embeddings. Pass `:default-model` to pick the model
(otherwise each provider uses its default: `jina-embeddings-v3`, `voyage-3`, or
`text-embedding-3-small`):

```sema
(llm/configure-embeddings :voyage
  {:api-key (env "VOYAGE_API_KEY") :default-model "voyage-3-large"})

(llm/configure-embeddings :ollama
  {:model "embeddinggemma" :host "http://localhost:11434"})

(llm/configure-embeddings :together
  {:api-key (env "TOGETHER_API_KEY")})

(llm/configure-embeddings :fireworks
  {:api-key (env "FIREWORKS_API_KEY")})

;; OpenAI-compatible embedding provider, with a model and optional base URL
(llm/configure-embeddings :openai
  {:api-key (env "OPENAI_API_KEY") :default-model "text-embedding-3-large"})
```

Configuration does not replace the chat provider, even when both use the same name.

Model precedence is per-call `:model`, configured `:default-model`, configured
`:model`, `<PROVIDER>_EMBEDDING_MODEL`, then the provider default. Azure and TEI
require an explicit deployment/model name or their model environment variable.
`:base-url` overrides `<PROVIDER>_BASE_URL`;
Ollama also accepts `:host` and reads `OLLAMA_HOST`. Custom OpenAI-compatible
providers require an explicit `:base-url`. API keys default to the provider's
environment variable; Gemini also accepts `GOOGLE_API_KEY`.

These options can be set at configuration time and overridden by each `llm/embed` call:

| Option | Meaning |
|--------|---------|
| `:dimensions` | Positive output dimension, where supported by the model |
| `:input-type` | Cohere / Voyage input type, such as `"search_query"` / `"query"` |
| `:task` | Jina task, such as `"retrieval.query"`, or Nomic task, such as `"search_query"` |
| `:long-text-mode` | Nomic: `"truncate"` or `"mean"` |
| `:max-tokens-per-text` | Nomic: positive token limit per text |
| `:truncate` | Boolean; Ollama truncation, Voyage truncation, or Cohere END/NONE |
| `:keep-alive` | Ollama model lifetime, e.g. `"5m"` or `0` |
| `:timeout` | Positive request timeout in milliseconds |

Use options supported by the selected provider/model. Configure `:rerank-model`
and `:top-k` separately for reranking; calls can override them with `:model`,
`:top-k`, `:provider`, and `:timeout`.

Known protocols reject unsupported options before making an HTTP request.
Mistral and Voyage translate `:dimensions` to `output_dimension`; OpenAI-compatible
dimension options use `dimensions`. Cloudflare's BGE preset and the TEI preset
use the served model's full vector width and do not accept `:dimensions`.

Ollama uses its native [embedding API](https://docs.ollama.com/api/embed),
`POST /api/embed`. Install the default model first with `ollama pull embeddinggemma`.
Omitting `:truncate` and `:keep-alive` preserves the server defaults. Ollama has no
native reranking endpoint.

### Nomic hosted embeddings

The `:nomic` preset uses Nomic's native
[text embedding API](https://docs.nomic.ai/reference/api/embed-text-v-1-embedding-text-post),
`POST https://api-atlas.nomic.ai/v1/embedding/text`. It reads `NOMIC_API_KEY`,
defaults to `nomic-embed-text-v1.5`, and accepts `NOMIC_BASE_URL` or `:base-url`
as an API root ending in `/v1`. It supports embeddings only; no hosted Nomic
reranking endpoint is configured.

```sema
(llm/configure-embeddings :nomic
  {:api-key (env "NOMIC_API_KEY") :task "search_document"})
(define docs (llm/embed ["Cats are mammals." "Rocks are minerals."]))
(define query (llm/embed "Which animals are mammals?" {:task "search_query"}))
```

Use `:task` with `"search_document"`, `"search_query"`, `"classification"`, or
`"clustering"`. The default is `"search_document"`. `:dimensions` maps to
`dimensionality` (64–768 for v1.5); `:long-text-mode` maps to `long_text_mode`,
and `:max-tokens-per-text` maps to the token limit. The default long-text mode is
`"truncate"`, matching Nomic's Python SDK. `:truncate #t` is accepted as an alias for
`:long-text-mode "truncate"`; `#f` does not mean `"mean"` and is rejected.
Use `:task`, not `:input-type`. `:keep-alive` applies to Ollama, not Nomic.

The adapter sends at most ten texts per request, preserves input order, and sums
input token usage across requests. `:timeout` applies to each request. A failed
request returns an error, not a partial result. Embedding calls do not retry
automatically; a repeated call can charge again for earlier successful requests.

The adapter has deterministic tests against the documented schema and passed
live API verification with `nomic-embed-text-v1.5` on 2026-10-21.

### OpenRouter and DeepInfra

Create keys in [OpenRouter](https://openrouter.ai/settings/keys) or
[DeepInfra](https://deepinfra.com/dash/api_keys), then set `OPENROUTER_API_KEY`
or `DEEPINFRA_API_KEY`. The defaults are `openai/text-embedding-3-small` and
`Qwen/Qwen3-Embedding-0.6B`, respectively. These presets configure embeddings
independently of chat and do not configure reranking.

```sema
(llm/configure-embeddings :openrouter {})
(llm/embed "A short document.")

(llm/configure-embeddings :deepinfra {})
(llm/embed ["A short document." "Et kort dokument."])
```

Their API roots are `https://openrouter.ai/api/v1` and
`https://api.deepinfra.com/v1/openai`. Override them with `:base-url`,
`OPENROUTER_BASE_URL`, or `DEEPINFRA_BASE_URL`. Model-specific task instructions
remain part of the input text; a hosted Qwen model does not automatically use
Jina or Cohere task parameters.

### Azure OpenAI v1

Create an [Azure account](https://azure.microsoft.com/free/) and an
[embedding model deployment](https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/embeddings).
Set the API key, resource endpoint, and deployment name:

```bash
export AZURE_OPENAI_API_KEY="your-resource-key"
export AZURE_OPENAI_ENDPOINT="https://your-resource.openai.azure.com"
export AZURE_OPENAI_EMBEDDING_DEPLOYMENT="your-embedding-deployment"
```

```sema
(llm/configure-embeddings :azure {})
(llm/embed "A short document.")
```

The request's model is the **deployment name**. Sema appends `/openai/v1` to the
resource endpoint and uses bearer API-key authentication. `AZURE_BASE_URL`,
`AZURE_OPENAI_BASE_URL`, or `:base-url` can instead supply the complete API root.
`AZURE_API_KEY` is an API-key alias. This preset uses the current v1 API; it does
not construct older dated deployment routes or refresh Microsoft Entra tokens.

### Cloudflare Workers AI

[Create an account](https://dash.cloudflare.com/sign-up), then follow the
[Workers AI token setup](https://developers.cloudflare.com/workers-ai/get-started/rest-api/).
The token needs Workers AI Read and Edit permissions. Set `CLOUDFLARE_API_TOKEN`
and `CLOUDFLARE_ACCOUNT_ID`. `CLOUDFLARE_API_KEY` is accepted as a token alias.

```sema
(llm/configure-embeddings :cloudflare {})
(llm/embed "A short document.")
```

The default model is `@cf/baai/bge-m3`. The API root is
`https://api.cloudflare.com/client/v4/accounts/ACCOUNT_ID/ai/v1`.
`:account-id` overrides the account environment variable. `:base-url` or
`CLOUDFLARE_BASE_URL` supplies a complete root instead, without requiring an
account ID. This is the Workers AI compatible route, not the native `/ai/run` API.

### Alibaba Model Studio

Create a pay-as-you-go [Model Studio API key](https://www.alibabacloud.com/help/en/model-studio/get-api-key).
Use `ALIBABA_API_KEY` or `DASHSCOPE_API_KEY`. Set `ALIBABA_BASE_URL` or
`DASHSCOPE_BASE_URL` to the compatible API root for the key's region and workspace.
For example, Singapore uses
`https://WORKSPACE_ID.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1`.
Use the [embedding API reference](https://www.alibabacloud.com/help/en/model-studio/text-embedding-synchronous-api)
to select the matching regional root. Sema does not guess a region or workspace.

```sema
(llm/configure-embeddings :alibaba {:dimensions 256})
(llm/embed ["A short document." "Et kort dokument."])
```

The default is `text-embedding-v4`. Sema splits v3/v4 inputs into requests of at
most ten texts, and `qwen3.7-text-embedding` inputs into requests of at most twenty.
The per-call model selects the limit. Results preserve input order and sum token
usage across requests. A failed chunk returns an error; no partial result is
returned. Earlier successful chunks can still incur charges. This preset returns
dense float vectors; it does not expose the native DashScope sparse-vector API.

### Self-hosted TEI

The `:tei` preset connects to Hugging Face Text Embeddings Inference. The default
API root is `http://127.0.0.1:8080/v1`; override it with `TEI_BASE_URL` or `:base-url`.
Set `TEI_MODEL` or `:model` to the server's model ID or `--served-model-name` alias.
The request does not load or switch models on the server. Authentication is
optional; set `TEI_API_KEY` when the server requires it.

On Apple Silicon, use the [native Metal installation](https://huggingface.co/docs/text-embeddings-inference/local_metal):

```bash
brew install text-embeddings-inference
text-embeddings-router --model-id BAAI/bge-small-en-v1.5 \
  --hostname 127.0.0.1 --port 8080 --max-client-batch-size 32
```

In another terminal:

```bash
export TEI_MODEL="BAAI/bge-small-en-v1.5"
sema --embedding-provider tei -e '(map embedding/length (llm/embed ["hello" "hei"]))'
```

This model produces 384-dimensional vectors. For NVIDIA GPUs, the official
[container setup](https://huggingface.co/docs/text-embeddings-inference/quick_tour)
supports models such as `Qwen/Qwen3-Embedding-0.6B`. Select the container for your
hardware from the [supported hardware list](https://huggingface.co/docs/text-embeddings-inference/supported_models).
The `cpu-1.9` image is published for x86-64; use the native Metal setup on Apple Silicon.

```sema
(llm/configure-embeddings :tei
  {:model "BAAI/bge-small-en-v1.5" :base-url "http://127.0.0.1:8080/v1"})
(llm/embed ["hello" "hei"])
```

Sema uses batches of at most 32 texts. Keep the server's `--max-client-batch-size`
at least 32. This preset exposes the compatible dense-text endpoint, without
native sparse output, reranking, or per-request pooling controls. Model-specific
query/document prefixes and normalization must match the model and index setup.

Embedding cassette keys include the resolved provider, endpoint, model, and options.
Re-record older embedding tapes after upgrading.

## Generating Embeddings

### `llm/embed`

Generate an embedding for a string or a list of strings. Returns a **bytevector** containing densely-packed f64 values in little-endian format. This format avoids per-element unboxing overhead for similarity computations compared to a list of floats.

```sema
;; Single embedding (returns a bytevector)
(define v1 (llm/embed "hello world"))

;; Pick the model per call with an options map
(llm/embed "hello world" {:provider :openai :model "text-embedding-3-small"
                         :dimensions 256 :timeout 30000})

;; Batch embeddings
(llm/embed ["cat" "dog" "fish"])       ; => list of bytevectors
```

## Embedding Accessors

### `embedding/length`

Returns the number of dimensions (f64 elements) in an embedding bytevector.

```sema
(define v (llm/embed "hello"))
(embedding/length v)                   ; => 1024 (depends on provider)
```

### `embedding/ref`

Access a specific dimension by index.

```sema
(define v (llm/embed "hello"))
(embedding/ref v 0)                    ; => 0.0123 (first dimension)
```

### `embedding/->list`

Convert an embedding bytevector to a list of floats (useful for interop).

```sema
(define v (llm/embed "hello"))
(embedding/->list v)                   ; => (0.0123 -0.0456 ...)
```

### `embedding/list->embedding`

Convert a list of numbers to an embedding bytevector.

```sema
(define v (embedding/list->embedding '(0.1 0.2 0.3)))
(embedding/length v)                   ; => 3
```

## Computing Similarity

### `llm/similarity`

Compute cosine similarity between two embedding vectors. Returns a value between -1.0 and 1.0. Accepts both bytevectors (fast path) and lists of floats (backward compatible).

```sema
(define v1 (llm/embed "hello world"))
(define v2 (llm/embed "hi there"))
(llm/similarity v1 v2)                 ; => 0.87 (cosine similarity)

;; Also works with plain lists
(llm/similarity '(0.1 0.2 0.3) '(0.4 0.5 0.6))
```

## Reranking

### `llm/rerank`

Reorder a list of candidate documents by their relevance to a query using a hosted **cross-encoder** reranker (Cohere, Jina, Voyage, Together AI, or Fireworks AI — the same **API key** you already use for embeddings; see [Supported Embedding Providers](#supported-embedding-providers) below for setup). Where `llm/similarity` / `vector-store/search` embed the query and documents *independently* (a bi-encoder), a reranker reads the query and each document *together*, so it's far more precise. The standard pattern is to retrieve a generous shortlist by vector search, then rerank it to the best few.

```sema
(llm/rerank "how do I read a file?"
            (list "vectors are cool" "use file/read to read a file" "unrelated trivia")
            {:top-k 2})
;; => ({:index 1 :score 0.91 :document "use file/read to read a file"} ...)
```

Returns `{:index :score :document}` maps, highest relevance first; `:index` points back into the input list. Options: `:top-k`, `:model`, `:timeout`, and `:provider` (`:cohere` / `:jina` / `:voyage` / `:together` / `:fireworks`). See the **[RAG guide](/docs/llm/rag)** for the full retrieve → rerank → answer pipeline.

## Token Counting

### `llm/token-count`

Estimate the number of tokens in a string or list of strings. Uses a heuristic (chars/4) — no tokenizer dependency required.

```sema
(llm/token-count "hello world")           ; => 3
(llm/token-count '("hello" "world"))      ; => sum of individual counts
```

### `llm/token-estimate`

Returns a detailed estimate map with the token count and the estimation method used.

```sema
(llm/token-estimate "hello world")
; => {:method "chars/4" :tokens 3}
```

## Supported Embedding Providers

| Provider | Env Variable | Reranking | Default model |
|----------|-------------|:---------:|---------------|
| Jina | `JINA_API_KEY` | ✅ | `jina-embeddings-v3` |
| Voyage | `VOYAGE_API_KEY` | ✅ | `voyage-3` |
| Cohere | `COHERE_API_KEY` | ✅ | `embed-english-v3.0` |
| Nomic | `NOMIC_API_KEY` | — | `nomic-embed-text-v1.5` |
| Together AI | `TOGETHER_API_KEY` | ✅ | `BAAI/bge-base-en-v1.5` |
| Fireworks AI | `FIREWORKS_API_KEY` | ✅ | `fireworks/qwen3-embedding-8b` |
| OpenAI | `OPENAI_API_KEY` | — | `text-embedding-3-small` |
| Ollama | `OLLAMA_HOST` (optional) | — | `embeddinggemma` |
| Mistral | `MISTRAL_API_KEY` | — | `mistral-embed` |
| Gemini | `GEMINI_API_KEY` / `GOOGLE_API_KEY` | — | `gemini-embedding-001` |
| OpenRouter | `OPENROUTER_API_KEY` | — | `openai/text-embedding-3-small` |
| DeepInfra | `DEEPINFRA_API_KEY` | — | `Qwen/Qwen3-Embedding-0.6B` |
| Azure | `AZURE_OPENAI_API_KEY` / `AZURE_API_KEY` | — | Required deployment name |
| Cloudflare | `CLOUDFLARE_API_TOKEN` / `CLOUDFLARE_API_KEY` | — | `@cf/baai/bge-m3` |
| Alibaba | `ALIBABA_API_KEY` / `DASHSCOPE_API_KEY` | — | `text-embedding-v4` |
| TEI | `TEI_MODEL`, optional `TEI_API_KEY` | — | Required served model name |

See [Provider Management](./providers.md) for the full provider capability table.
