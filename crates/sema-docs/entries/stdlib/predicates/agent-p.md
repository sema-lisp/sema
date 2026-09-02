---
name: "agent?"
module: "predicates"
section: "LLM Type Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["agent", "conversation?", "tool?", "type-of"]
---

Test if a value is an agent.

```sema
(defagent my-agent {:system "test"})
(agent? my-agent)   ; => #t
(agent? 42)         ; => #f
```
