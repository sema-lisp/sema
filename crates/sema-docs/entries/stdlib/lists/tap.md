---
name: "tap"
module: "lists"
section: "Utility"
---

Apply a side-effect function to a value, then return the original value.

```sema
(tap 42 (fn (x) (println x)))   ; prints 42, returns 42
```
