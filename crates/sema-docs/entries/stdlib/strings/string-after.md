---
name: "string/after"
module: "strings"
section: "Slicing & Extraction"
---

Everything after the first occurrence of a needle. Returns the original string if needle not found.

```sema
(string/after "hello@world.com" "@")  ; => "world.com"
(string/after "no-match" "@")         ; => "no-match"
```
