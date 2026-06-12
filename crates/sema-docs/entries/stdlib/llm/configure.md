---
name: "llm/configure"
module: "llm"
params: [{ name: provider, type: keyword }, { name: opts, type: map }]
returns: "nil"
---

Configure and register an LLM provider. Chat providers (`:anthropic`, `:openai`, `:gemini`, `:groq`, `:xai`, `:mistral`, `:moonshot`, `:ollama`, or any unknown name treated as OpenAI-compatible) become the default chat provider. Embedding providers (`:jina`, `:voyage`, `:cohere`) are registered as the embedding provider instead — equivalent to `llm/configure-embeddings`. The map carries `:api-key`, optional `:default-model`, and `:base-url`/`:host` depending on the provider. Returns nil.

```sema
(llm/configure :anthropic {:api-key "sk-ant-..." :default-model "claude-sonnet-4-6"})
```
