---
name: "sys/interactive?"
module: "system"
section: "Session Information"
---

Test if stdin is a TTY (i.e., running interactively).

```sema
(sys/interactive?)   ; => #t in REPL, #f in scripts
```
