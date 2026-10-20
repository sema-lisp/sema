---
name: "llm/configure-embeddings"
module: "llm"
params: [{ name: provider, type: keyword }, { name: opts, type: map }]
returns: "nil"
see_also: ["llm/embed", "llm/similarity", "llm/configure"]
---

Configure and register the embedding provider used by `llm/embed`. Presets are `:openai`, `:jina`, `:voyage`, `:cohere`, `:nomic`, `:ollama`, `:mistral`, `:gemini`, `:together`, `:fireworks`, `:openrouter`, `:deepinfra`, `:azure`, `:cloudflare`, `:alibaba`, and `:tei`. Other names require an OpenAI-compatible `:base-url`. This does not replace chat configuration.

The map accepts `:api-key`, `:base-url`, `:default-model` (preferred over the alias `:model`), and defaults for `:dimensions`, `:input-type`, `:task`, `:truncate`, `:keep-alive`, and `:timeout` (milliseconds), where supported by the provider. Calls can override these options. Rerank defaults use separate `:rerank-model` and `:top-k` options. Ollama uses `/api/embed`, defaults to `embeddinggemma`, accepts `:host`, and reads `OLLAMA_HOST`. Install its model first with `ollama pull embeddinggemma`. Returns nil.

Nomic uses `/v1/embedding/text`, reads `NOMIC_API_KEY` and `NOMIC_BASE_URL`, and defaults to `nomic-embed-text-v1.5`. Its `:task` is `"search_document"` (default), `"search_query"`, `"classification"`, or `"clustering"`. It also accepts `:long-text-mode` (`"truncate"` or `"mean"`) and positive `:max-tokens-per-text`. No hosted Nomic reranker is configured.

Provider defaults and automatic selection share the same preset registry. `<PROVIDER>_EMBEDDING_MODEL` overrides the preset's default. Azure requires `:model` or `AZURE_OPENAI_EMBEDDING_DEPLOYMENT`, and a complete `AZURE_BASE_URL` or resource `AZURE_OPENAI_ENDPOINT`. Cloudflare reads `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` (or `:account-id`); a complete `:base-url` overrides account URL construction. Alibaba accepts `ALIBABA_API_KEY` / `DASHSCOPE_API_KEY` and requires `ALIBABA_BASE_URL` / `DASHSCOPE_BASE_URL` for the matching region/workspace. TEI uses `TEI_MODEL`, optional `TEI_API_KEY`, and `TEI_BASE_URL` (default `http://127.0.0.1:8080/v1`).

Mistral and Voyage map `:dimensions` to `output_dimension`. Known protocols reject unsupported request options. Cloudflare and TEI use the served model's full vector width. Alibaba v3/v4 batches at ten texts, qwen3.7 at twenty; TEI batches at 32. These new presets configure embeddings only.

```sema
(llm/configure-embeddings :openai {:api-key "sk-..." :model "text-embedding-3-small"})
```
