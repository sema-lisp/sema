---
name: "llm/auto-configure"
module: "llm"
params: []
returns: "keyword or nil"
see_also: ["llm/configure", "llm/default-provider", "llm/list-providers"]
syntax: "(llm/auto-configure)"
---

Auto-configure providers from environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GROQ_API_KEY`, `XAI_API_KEY`, `MISTRAL_API_KEY`, `MOONSHOT_API_KEY`, `GOOGLE_API_KEY`, plus embedding keys like `JINA_API_KEY`, `VOYAGE_API_KEY`, `COHERE_API_KEY`, `NOMIC_API_KEY`). Ollama is always registered as a local fallback. Honors `SEMA_CHAT_PROVIDER`/`SEMA_CHAT_MODEL`, `SEMA_EMBEDDING_PROVIDER`/`SEMA_EMBEDDING_MODEL`, and `SEMA_RERANK_PROVIDER`/`SEMA_RERANK_MODEL` overrides. All available retrieval providers are registered, so per-call `:provider` overrides can select them. Returns the default provider keyword, or nil if none configured.

Retrieval selection uses Jina, Voyage, Cohere, Nomic, OpenAI, Mistral, Fireworks, Together, Gemini, OpenRouter, DeepInfra, Azure, Cloudflare, Alibaba, TEI, then Ollama. Azure and Alibaba require complete endpoint/model configuration; Cloudflare requires an account ID or complete base URL. Incomplete unselected accounts are skipped. An explicitly selected incomplete provider fails at startup. TEI is discovered when its model is configured, without requiring an API key. See `llm/configure-embeddings` for environment variables.

```sema
(llm/auto-configure)   ; => :anthropic  (if ANTHROPIC_API_KEY is set)
```
