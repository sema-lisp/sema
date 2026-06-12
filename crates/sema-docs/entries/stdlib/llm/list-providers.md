---
name: "llm/list-providers"
module: "llm"
params: []
returns: "list"
---

Return a list of keywords naming all currently configured providers.

```sema
(llm/list-providers)   ; => (:anthropic :ollama)
```
