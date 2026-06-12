---
name: "llm/similarity"
module: "llm"
params: [{ name: a }, { name: b }]
returns: "float"
---

Compute the cosine similarity between two embeddings. Both arguments must be the same type: either two embedding bytevectors (f64 little-endian, length a multiple of 8) or two lists of numbers of equal length. Returns a float; 0.0 when either vector has zero magnitude.

```sema
(llm/similarity (llm/embed "cat") (llm/embed "kitten"))   ; => ~0.89
```
