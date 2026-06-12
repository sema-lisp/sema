---
name: "llm/session-usage"
module: "llm"
params: []
returns: "map"
---

Return cumulative token usage for the whole session as a map with `:prompt-tokens`, `:completion-tokens`, `:total-tokens`, and `:cost-usd`. The totals accumulate across all LLM calls until reset with `llm/reset-usage`.

```sema
(llm/session-usage)   ; => {:prompt-tokens 120 :completion-tokens 340 :total-tokens 460 :cost-usd 0.0021}
```
