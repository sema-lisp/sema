---
name: "llm/clear-budget"
module: "llm"
params: []
returns: "nil"
see_also: ["llm/set-budget", "llm/budget-remaining", "llm/with-budget"]
syntax: "(llm/clear-budget)"
---

Clear any active budget limit, removing both the cost and token budget caps.

```sema
(llm/clear-budget)
```
