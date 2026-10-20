---
name: "llm/embed"
module: "llm"
params: [{ name: input }, { name: opts, type: map }]
returns: "bytevector or list"
see_also: ["llm/similarity", "llm/rerank", "llm/configure-embeddings"]
---

Compute embeddings — dense vectors that place semantically similar text near each other — using the configured embedding provider. A single string returns one embedding as a bytevector (f64 little-endian floats); a list of strings returns a list of bytevectors, one per input. The optional opts map accepts `:provider` (keyword or string), `:model`, `:dimensions`, `:input-type`, `:task`, `:truncate`, `:keep-alive`, and `:timeout` (positive milliseconds). Per-call values override `llm/configure-embeddings` defaults. Use options supported by the chosen provider/model.

Embeddings are the foundation of semantic search and RAG: embed your documents
once, then at query time embed the question and rank documents by `llm/similarity`
(cosine). Batch your inputs (pass a list) rather than calling `llm/embed` per
string — it is one provider request instead of N, far cheaper and faster.

Nomic additionally accepts `:long-text-mode` (`"truncate"` or `"mean"`) and
`:max-tokens-per-text` (positive integer). Use `:task "search_document"` for
documents and `:task "search_query"` for queries; `:dimensions` selects the
output dimensionality with `nomic-embed-text-v1.5`.

The result is an opaque bytevector, not a list of numbers, so use the
`embedding/*` helpers to inspect or convert: `embedding/->list` for the raw floats,
`embedding/length` for the dimension count, `embedding/list->embedding` to go back.
Embeddings from different models (or different providers) are **not** comparable —
only compare vectors produced by the same model.

```sema
;; Configure once, then embed and rank by similarity.
(llm/configure-embeddings :openai {:api-key (env "OPENAI_API_KEY")})
(let [docs ["cats are mammals" "the stock market fell" "kittens are baby cats"]
      vecs (llm/embed docs)                       ; one batched request
      q    (llm/embed "feline animals")]
  (map (fn (v) (llm/similarity q v)) vecs))
; => (0.82 0.11 0.79)   ; doc 0 and 2 are about cats

;; A single string returns a single bytevector.
(embedding/length (llm/embed "hello"))   ; => 1536  (model-dependent)
```
