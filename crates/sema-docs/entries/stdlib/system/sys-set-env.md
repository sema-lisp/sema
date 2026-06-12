---
name: "sys/set-env"
module: "system"
section: "Environment Variables"
---

Set an environment variable for the current process.

```sema
(sys/set-env "KEY" "value")
(env "KEY")   ; => "value"
```
