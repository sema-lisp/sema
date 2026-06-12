---
name: "spy"
module: "system"
section: "Debugging"
params: [{ name: label, type: string }, { name: value, type: any }]
---

Print `[label] value` to standard error and return `value` unchanged. Handy for inspecting intermediate values inside a pipeline.

```sema
(spy "x" (+ 1 2))   ; prints "[x] 3" to stderr, returns 3
```
