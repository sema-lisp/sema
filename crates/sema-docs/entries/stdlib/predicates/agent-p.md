---
name: "agent?"
module: "predicates"
section: "LLM Type Predicates"
---

Test if a value is an agent.

```sema
(defagent my-agent {:system "test"})
(agent? my-agent)   ; => #t
(agent? 42)         ; => #f
```
