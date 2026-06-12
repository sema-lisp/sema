---
name: "embedding/length"
module: "embedding"
params: [{ name: embedding, type: bytevector }]
returns: "int"
---

Return the number of dimensions in an embedding. Embeddings are bytevectors of little-endian f64 values, so the length is the byte length divided by 8.

```sema
(embedding/length (llm/embed "hello"))   ; => 1536
```
