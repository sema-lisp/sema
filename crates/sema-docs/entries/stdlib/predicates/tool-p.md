---
name: "tool?"
module: "predicates"
section: "LLM Type Predicates"
---

Test if a value is a tool definition.

```sema
(deftool my-tool "A test tool" {:x {:type :string}} (lambda (x) x))
(tool? my-tool)   ; => #t
(tool? 42)        ; => #f
```
