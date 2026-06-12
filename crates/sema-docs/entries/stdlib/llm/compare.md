---
name: "llm/compare"
module: "llm"
params: [{ name: text-a, type: string }, { name: text-b, type: string }, { name: opts, type: map }]
returns: "map"
---

Compare two texts using the default provider and return a parsed JSON map with `:similarity` (0.0–1.0), `:differences` (a list of key differences), and `:summary` (a brief comparison). The opts map accepts `:model`.

```sema
(llm/compare "The cat sat on the mat" "A cat is sitting on a rug")
```
