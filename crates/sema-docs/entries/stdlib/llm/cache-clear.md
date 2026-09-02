---
name: "llm/cache-clear"
module: "llm"
params: []
returns: "int"
see_also: ["llm/with-cache", "llm/cache-stats", "llm/cache-key"]
syntax: "(llm/cache-clear)"
---

Clear the LLM response cache: empties the in-memory cache, deletes cached `.json` files from the on-disk cache directory, and resets hit/miss counters. Returns the number of in-memory entries that were removed.

```sema
(llm/cache-clear)   ; => 12
```
