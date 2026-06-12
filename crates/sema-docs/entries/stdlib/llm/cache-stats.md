---
name: "llm/cache-stats"
module: "llm"
params: []
returns: "map"
---

Return cache statistics as a map with `:hits`, `:misses`, and `:size` (the number of entries currently in the in-memory cache).

```sema
(llm/cache-stats)   ; => {:hits 3 :misses 1 :size 4}
```
