---
name: "llm/batch"
module: "llm"
params: [{ name: prompts }, { name: opts, type: map }]
returns: "list"
---

Send a list of prompt strings to the default provider in parallel (via the provider's batch completion) and return a list of completion strings in the same order. The opts map accepts `:model`, `:max-tokens`, `:temperature`, and `:system`.

```sema
(llm/batch ["Capital of France?" "Capital of Japan?"] {:max-tokens 20})
```
