---
name: "llm/complete"
module: "llm"
params: [{ name: prompt, type: string }, { name: opts, type: map }]
returns: "string"
---

Send a single prompt to the default provider and return the completion text. The first argument is a prompt string (or a prompt value). The optional opts map accepts `:model`, `:max-tokens` (defaults to 4096), `:temperature`, and `:system`. Tracks token usage.

```sema
(llm/complete "Write a haiku about autumn" {:max-tokens 100 :temperature 0.7})
```
