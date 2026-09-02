---
name: "sys/set-env"
module: "system"
section: "Environment Variables"
params: [{ name: name, type: string }, { name: value, type: string }]
returns: "nil"
see_also: ["env", "sys/env-all"]
---

Set an environment variable for the current process.

```sema
(sys/set-env "KEY" "value")
(env "KEY")   ; => "value"
```
