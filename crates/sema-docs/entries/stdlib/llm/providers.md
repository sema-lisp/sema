---
name: "llm/providers"
module: "llm"
params: []
returns: "list"
see_also: ["llm/list-providers", "llm/define-provider", "llm/default-provider"]
syntax: "(llm/providers)"
---

Return a list of keywords naming all currently configured providers. Equivalent to `llm/list-providers`.

```sema
(llm/providers)   ; => (:anthropic :ollama)
```
