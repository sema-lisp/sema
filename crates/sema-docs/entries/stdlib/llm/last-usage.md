---
name: "llm/last-usage"
module: "llm"
params: []
returns: "map or nil"
---

Return token usage for the most recent LLM call as a map with `:prompt-tokens`, `:completion-tokens`, `:total-tokens`, `:model`, and (when pricing is known) `:cost-usd`. Returns nil if no call has been made yet.

```sema
(llm/complete "hi")
(llm/last-usage)   ; => {:prompt-tokens 8 :completion-tokens 12 :total-tokens 20 :model "..."}
```
