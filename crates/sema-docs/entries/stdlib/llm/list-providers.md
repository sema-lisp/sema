---
name: "llm/list-providers"
module: "llm"
params: []
returns: "list"
see_also: ["llm/providers", "llm/default-provider", "llm/current-provider"]
syntax: "(llm/list-providers)"
---

Return a list of keywords naming all currently configured providers.

```sema
(llm/list-providers)   ; => (:anthropic :ollama)
```
