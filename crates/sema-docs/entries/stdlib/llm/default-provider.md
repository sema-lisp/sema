---
name: "llm/default-provider"
module: "llm"
params: []
returns: "keyword or nil"
---

Return the name of the current default provider as a keyword, or nil if none is configured.

```sema
(llm/default-provider)   ; => :anthropic
```
