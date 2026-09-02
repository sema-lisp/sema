---
name: "conversation?"
module: "predicates"
section: "LLM Type Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["conversation/new", "agent?", "message?", "type-of"]
---

Test if a value is a conversation.

```sema
(conversation? (conversation/new {}))   ; => #t
```
