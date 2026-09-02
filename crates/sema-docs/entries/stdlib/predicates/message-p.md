---
name: "message?"
module: "predicates"
section: "LLM Type Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["message", "conversation?", "prompt?", "type-of"]
---

Test if a value is an LLM message.

```sema
(message? (message :user "hi"))   ; => #t
```
