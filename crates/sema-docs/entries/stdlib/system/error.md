---
name: "error"
module: "system"
section: "Errors"
params: [{ name: message, type: any }]
---

Raise an error (a catchable exception) with the given message. Non-string arguments are stringified.

```sema
(try (error "boom") (catch e (get e :message)))   ; => "boom"
```
