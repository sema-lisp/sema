---
name: "otel/retrieval-span"
module: "otel"
section: "Observability"
---

Run a thunk inside a typed **RETRIEVER** span for a retrieval / vector-search step you build yourself. When `SEMA_OTEL_COMPAT` is set it emits the backend-native RETRIEVER span-kind, so a custom RAG step renders like the built-in `vector-store/search` span in Phoenix/Langfuse. Optional third arg is an attributes map. A no-op when telemetry is disabled.

**Signature:** `(otel/retrieval-span name thunk) → any` · `(otel/retrieval-span name thunk attrs) → any`

```sema
(otel/retrieval-span "vector-search"
  (fn () (search index query))
  {:top-k 5})
```

See the [Observability guide](https://sema-lang.com/docs/llm/observability).
