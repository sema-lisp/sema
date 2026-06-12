---
name: "llm/auto-configure"
module: "llm"
params: []
returns: "keyword or nil"
---

Auto-configure providers from environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GROQ_API_KEY`, `XAI_API_KEY`, `MISTRAL_API_KEY`, `MOONSHOT_API_KEY`, `GOOGLE_API_KEY`, plus embedding keys like `JINA_API_KEY`, `VOYAGE_API_KEY`, `COHERE_API_KEY`). Ollama is always registered as a local fallback. Honors `SEMA_CHAT_PROVIDER`/`SEMA_CHAT_MODEL` and `SEMA_EMBEDDING_PROVIDER`/`SEMA_EMBEDDING_MODEL` overrides. Returns the default provider keyword, or nil if none configured.

```sema
(llm/auto-configure)   ; => :anthropic  (if ANTHROPIC_API_KEY is set)
```
