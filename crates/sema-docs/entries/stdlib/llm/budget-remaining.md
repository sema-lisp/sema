---
name: "llm/budget-remaining"
module: "llm"
params: []
returns: "map or nil"
---

Return the current budget status, or nil if no budget is set. When a cost budget is active the map includes `:limit`, `:spent`, and `:remaining` (USD); when a token budget is active it includes `:token-limit`, `:tokens-spent`, and `:tokens-remaining`.

```sema
(llm/set-budget 1.0)
(llm/budget-remaining)   ; => {:limit 1.0 :spent 0.0 :remaining 1.0}
```
