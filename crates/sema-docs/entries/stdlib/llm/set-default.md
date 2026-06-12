---
name: "llm/set-default"
module: "llm"
params: [{ name: provider }]
returns: "keyword"
---

Set which already-configured provider is the default for chat calls. The argument is a provider keyword or string. Errors if the named provider is not configured. Returns the provider name as a keyword.

```sema
(llm/set-default :openai)
```
