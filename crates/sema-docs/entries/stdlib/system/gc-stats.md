---
name: "gc/stats"
module: "system"
section: "Memory"
returns: "map"
---

Return the cycle collector's stats without collecting: the last completed collection's `:candidates`, `:traced`, `:collected`, and `:pruned` counts (all zero before the first collection), plus `:registry-size` — the current number of registered cycle candidates awaiting the next collection. Useful for diagnosing memory growth in long-running sessions.

```sema
(gc/stats)   ; => {:candidates 3 :collected 4 :pruned 1 :registry-size 2 :traced 9}
```
