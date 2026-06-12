---
name: "llm/pricing-status"
module: "llm"
params: []
returns: "map"
---

Return the status of the pricing table used for cost estimates: a map with `:source` (a symbol indicating where pricing came from) and, when available, `:updated-at` (a date string).

```sema
(llm/pricing-status)   ; => {:source hardcoded}
```
