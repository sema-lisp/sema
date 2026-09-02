---
name: "llm/set-budget"
module: "llm"
params: [{ name: max-cost-usd, type: number }]
returns: "nil"
see_also: ["llm/budget-remaining", "llm/clear-budget", "llm/with-budget"]
---

Set a global spending budget (in USD) for LLM calls and reset the amount spent to zero. Subsequent calls accumulate cost; the call that would exceed the limit raises an error. Costs are estimated from the pricing table, so enforcement is best-effort when a model's pricing is unknown.

This is process-global and persists until you call `llm/clear-budget` — handy as
a safety rail at the top of a script. For a cap that applies to just one section
of code (and nests/cleans up automatically) use `llm/with-budget` instead. Check
how much headroom is left with `llm/budget-remaining`.

```sema
(llm/set-budget 0.50)
(llm/budget-remaining)   ; => {:limit 0.5 :remaining 0.5 :spent 0.0}
;; ... LLM calls accumulate cost; one that would push past 0.50 throws.
(llm/clear-budget)       ; remove the cap
```
