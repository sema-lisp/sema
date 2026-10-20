---
name: "llm/configure-embeddings"
module: "llm"
params: [{ name: provider, type: keyword }, { name: opts, type: map }]
returns: "nil"
see_also: ["llm/embed", "llm/similarity", "llm/configure"]
---

Configure and register the embedding provider used by `llm/embed`. Presets are `:openai`, `:jina`, `:voyage`, `:cohere`, `:nomic`, `:ollama`, `:mistral`, `:gemini`, `:together`, and `:fireworks`. Other names require an OpenAI-compatible `:base-url`. This does not replace chat configuration.

The map accepts `:api-key`, `:base-url`, `:default-model` (preferred over the alias `:model`), and defaults for `:dimensions`, `:input-type`, `:task`, `:truncate`, `:keep-alive`, and `:timeout` (milliseconds), where supported by the provider. Calls can override these options. Rerank defaults use separate `:rerank-model` and `:top-k` options. Ollama uses `/api/embed`, defaults to `embeddinggemma`, accepts `:host`, and reads `OLLAMA_HOST`. Install its model first with `ollama pull embeddinggemma`. Returns nil.

Nomic uses `/v1/embedding/text`, reads `NOMIC_API_KEY` and `NOMIC_BASE_URL`, and defaults to `nomic-embed-text-v1.5`. Its `:task` is `"search_document"` (default), `"search_query"`, `"classification"`, or `"clustering"`. It also accepts `:long-text-mode` (`"truncate"` or `"mean"`) and positive `:max-tokens-per-text`. No hosted Nomic reranker is configured. Live API verification is pending; the release is held until it passes.

```sema
(llm/configure-embeddings :openai {:api-key "sk-..." :model "text-embedding-3-small"})
```
