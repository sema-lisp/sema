---
name: "llm/providers"
module: "llm"
params: []
returns: "list"
---

Return a list of keywords naming all currently configured providers. Equivalent to `llm/list-providers`.

```sema
(llm/providers)   ; => (:anthropic :ollama)
```
