---
name: "llm/token-estimate"
module: "llm"
params: [{ name: input }]
returns: "map"
---

Estimate tokens for a string and return details as a map with `:tokens` (the chars/4 estimate), `:method` (`"chars/4"`), and `:chars` (the character count).

```sema
(llm/token-estimate "hello world")   ; => {:tokens 2 :method "chars/4" :chars 11}
```
