---
name: "llm/embed"
module: "llm"
params: [{ name: input }, { name: opts, type: map }]
returns: "bytevector or list"
---

Compute embeddings using the configured embedding provider. Given a single string, returns one embedding as a bytevector (f64 little-endian floats); given a list of strings, returns a list of bytevectors. The opts map accepts `:model`. Use the `embedding/*` helpers to convert to/from float lists.

```sema
(llm/embed "the quick brown fox")          ; => #bytevector(...)
(llm/embed ["alpha" "beta"] {:model "text-embedding-3-small"})
```
