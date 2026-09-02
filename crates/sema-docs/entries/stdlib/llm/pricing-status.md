---
name: "llm/pricing-status"
module: "llm"
params: []
returns: "map"
see_also: ["llm/set-pricing", "llm/last-usage", "llm/session-usage"]
syntax: "(llm/pricing-status)"
---

Return the status of the pricing table used for cost estimates: a map with `:source` (a symbol indicating where pricing came from) and, when available, `:updated-at` (a date string).

```sema
(llm/pricing-status)   ; => {:source embedded :updated-at "2026-06-18"}
```

`:source` is `embedded` for the pricing snapshot baked into the binary, or
another symbol if you have overridden prices with `llm/set-pricing`. Pricing
drives the `:cost-usd` figure in `llm/last-usage`/`llm/session-usage`; if a
model isn't in the table its cost reports as `0.0` rather than erroring.
