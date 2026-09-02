---
name: "llm/similarity"
module: "llm"
params: [{ name: a }, { name: b }]
returns: "float"
see_also: ["llm/embed", "llm/rerank"]
---

Compute the cosine similarity between two embeddings — the standard "how related is this text?" metric for semantic search. Both arguments must be the same type: either two embedding bytevectors (from `llm/embed`) or two lists of numbers of equal length. Returns a float; 0.0 when either vector has zero magnitude.

Cosine similarity measures the angle between vectors, ignoring magnitude, so it
ranges 1.0 (identical direction) → 0.0 (orthogonal) → -1.0 (opposite). For
normalized embeddings the practical range is roughly 0–1, and "related" text
typically scores 0.7+. It does **not** read the actual text — it only compares
the two vectors — so for precise ranking of retrieved candidates reach for
`llm/rerank`, which a cross-encoder evaluates query and document together.

```sema
;; Real use: rank candidate answers against a query embedding.
(let [q (llm/embed "how do I read a file?")]
  (llm/similarity q (llm/embed "use file/read to open a file")))   ; => ~0.86

;; Works on plain number lists too — handy for tests without a provider.
(llm/similarity (list 1.0 2.0 3.0) (list 2.0 4.0 6.0))   ; => 1.0  (same direction)
(llm/similarity (list 1.0 0.0) (list 0.0 1.0))           ; => 0.0  (orthogonal)
```
