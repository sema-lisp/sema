---
name: "llm/default-provider"
module: "llm"
params: []
returns: "keyword or nil"
see_also: ["llm/set-default", "llm/current-provider", "llm/list-providers"]
syntax: "(llm/default-provider)"
---

Return the name of the current default provider as a keyword, or nil if none is configured.

```sema
(llm/default-provider)   ; => :anthropic
```
