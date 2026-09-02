---
name: "term/spinner-start"
module: "terminal"
section: "Spinners"
params: [{ name: message, type: string }]
returns: "int"
see_also: ["term/spinner-update", "term/spinner-stop"]
---

Start a spinner with a message. Returns an integer spinner ID used to update or stop it.

```sema
(define id (term/spinner-start "Loading data..."))
```
