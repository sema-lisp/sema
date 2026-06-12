---
name: "term/spinner-start"
module: "terminal"
section: "Spinners"
---

Start a spinner with a message. Returns an integer spinner ID used to update or stop it.

```sema
(define id (term/spinner-start "Loading data..."))
```
