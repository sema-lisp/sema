---
name: "env"
module: "system"
section: "Environment Variables"
---

Get the value of an environment variable. Returns `nil` if not set.

```sema
(env "HOME")       ; => "/Users/ada"
(env "PATH")       ; => "/usr/bin:/bin:..."
(env "MISSING")    ; => nil
```
