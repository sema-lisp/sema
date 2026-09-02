---
name: "llm/cache-stats"
module: "llm"
params: []
returns: "map"
see_also: ["llm/with-cache", "llm/cache-clear", "llm/cache-key"]
syntax: "(llm/cache-stats)"
---

Return cache statistics as a map with `:hits`, `:misses`, and `:size` (the number of entries currently in the in-memory cache).

```sema
(llm/cache-stats)   ; => {:hits 3 :misses 1 :size 4}
```
