---
name: "llm/set-pricing"
module: "llm"
params: [{ name: model-pattern, type: string }, { name: input-per-million, type: number }, { name: output-per-million, type: number }]
returns: "nil"
---

Register custom pricing for cost calculation. The model pattern matches model names; the two numbers are the input and output costs per million tokens (USD). Used to compute `:cost-usd` in usage reports.

```sema
(llm/set-pricing "my-model" 0.5 1.5)
```
