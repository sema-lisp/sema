---
name: "term/spinner-update"
module: "terminal"
section: "Spinners"
params: [{ name: id, type: int }, { name: message, type: string }]
returns: "nil"
see_also: ["term/spinner-start", "term/spinner-stop"]
---

Update the message displayed next to a running spinner.

```sema
(term/spinner-update id "Processing records...")
(term/spinner-update id "Almost done...")
```
