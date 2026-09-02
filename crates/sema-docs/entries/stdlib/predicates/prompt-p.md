---
name: "prompt?"
module: "predicates"
section: "LLM Type Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["prompt", "message?", "conversation?", "type-of"]
---

Test if a value is an LLM prompt.

```sema
(prompt? (prompt (user "hi")))   ; => #t
```
