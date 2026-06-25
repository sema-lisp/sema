---
name: "regex/find-all"
module: "regex"
section: "Matching"
params: [{ name: pattern, type: string }, { name: text, type: string }]
returns: "list"
---

Find all non-overlapping matches of a pattern.

```sema
(regex/find-all #"\d+" "a1b2c3")          ; => ("1" "2" "3")
(regex/find-all #"[A-Z]" "Hello World")   ; => ("H" "W")
```
