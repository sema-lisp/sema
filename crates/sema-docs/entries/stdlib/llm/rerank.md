---
name: "llm/rerank"
module: "llm"
params: [{ name: query, type: string }, { name: documents, type: list }, { name: opts, type: map }]
returns: "list"
---

Reorder `documents` (a list of strings) by their cross-encoder relevance to `query`, using a hosted reranking provider (Cohere, Jina, or Voyage — the same **API key** you use for embeddings, e.g. `COHERE_API_KEY` / `JINA_API_KEY` / `VOYAGE_API_KEY`). Unlike cosine similarity over embeddings, a reranker reads the query and each document *together*, so it is far more precise — the standard RAG move is to retrieve many candidates by vector search, then rerank to the best few.

Returns a list of maps `{:index :score :document}`, highest relevance first. `:index` is the position in the original `documents` list, `:score` the relevance (higher is better; scores are query-dependent, use them for ordering, not as calibrated probabilities).

The opts map accepts `:top-k` (keep only the K best), `:model` (override the provider's default reranker), and `:provider` (`:cohere` / `:jina` / `:voyage` — defaults to the configured rerank provider).

```sema
(llm/rerank "how do I read a file?"
            (list "vectors are cool" "use file/read to read a file" "unrelated trivia")
            {:top-k 2})
;; => ({:index 1 :score 0.91 :document "use file/read to read a file"} ...)
```

See the [RAG guide](https://sema-lang.com/docs/llm/rag) for an end-to-end retrieve → rerank → answer pipeline.
