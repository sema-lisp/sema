---
name: "llm/set-budget"
module: "llm"
params: [{ name: max-cost-usd, type: number }]
returns: "nil"
---

Set a global spending budget (in USD) for LLM calls and reset the amount spent to zero. Subsequent calls accumulate cost; exceeding the limit raises an error. Costs are estimated from the pricing table, so enforcement is best-effort when pricing is unknown.

```sema
(llm/set-budget 0.50)
```
