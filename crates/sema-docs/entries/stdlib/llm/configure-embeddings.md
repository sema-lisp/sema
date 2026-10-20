---
name: "llm/configure-embeddings"
module: "llm"
params: [{ name: provider, type: keyword }, { name: opts, type: map }]
returns: "nil"
see_also: ["llm/embed", "llm/similarity", "llm/configure"]
---

Configure and register the embedding provider used by `llm/embed`. Presets are `:openai`, `:jina`, `:voyage`, `:cohere`, `:ollama`, `:mistral`, `:gemini`, `:together`, and `:fireworks`. Other names require an OpenAI-compatible `:base-url`. This does not replace chat configuration.

The map accepts `:api-key`, `:base-url`, `:default-model` (preferred over the alias `:model`), and defaults for `:dimensions`, `:input-type`, `:task`, `:truncate`, `:keep-alive`, and `:timeout` (milliseconds), where supported by the provider. Calls can override these options. Rerank defaults use separate `:rerank-model` and `:top-k` options. Ollama uses `/api/embed`, defaults to `embeddinggemma`, accepts `:host`, and reads `OLLAMA_HOST`. Install its model first with `ollama pull embeddinggemma`. Returns nil.

```sema
(llm/configure-embeddings :openai {:api-key "sk-..." :model "text-embedding-3-small"})
```
