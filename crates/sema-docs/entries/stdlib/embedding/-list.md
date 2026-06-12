---
name: "embedding/->list"
module: "embedding"
params: [{ name: embedding, type: bytevector }]
returns: "list"
---

Convert an embedding bytevector into a list of float values, one per dimension.

```sema
(embedding/->list (llm/embed "hello"))   ; => (0.0123 -0.045 ...)
```
