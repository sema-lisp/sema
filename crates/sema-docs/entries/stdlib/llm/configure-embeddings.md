---
name: "llm/configure-embeddings"
module: "llm"
params: [{ name: provider, type: keyword }, { name: opts, type: map }]
returns: "nil"
---

Configure and register the embedding provider used by `llm/embed`. The provider keyword may be `:jina`, `:voyage`, `:cohere`, or any other (treated as OpenAI-compatible). The map carries `:api-key`, optional `:default-model`/`:model`, and `:base-url` for OpenAI-compatible endpoints. Returns nil.

```sema
(llm/configure-embeddings :openai {:api-key "sk-..." :model "text-embedding-3-small"})
```
