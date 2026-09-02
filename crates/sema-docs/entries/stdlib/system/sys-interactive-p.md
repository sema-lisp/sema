---
name: "sys/interactive?"
module: "system"
section: "Session Information"
syntax: "(sys/interactive?)"
returns: "bool"
see_also: ["sys/tty", "sys/term-size"]
---

Test if stdin is a TTY (i.e., running interactively).

```sema
(sys/interactive?)   ; => #t in REPL, #f in scripts
```
