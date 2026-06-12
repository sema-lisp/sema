---
name: "llm/current-provider"
module: "llm"
params: []
returns: "map or nil"
---

Return information about the active default provider as a map with `:name` (keyword) and `:model` (the provider's default model string). Returns nil if no provider is configured.

```sema
(llm/current-provider)   ; => {:name :anthropic :model "claude-sonnet-4-6"}
```
