---
name: "string/before"
module: "strings"
section: "Slicing & Extraction"
---

Everything before the first occurrence of a needle.

```sema
(string/before "hello@world.com" "@")  ; => "hello"
(string/before "no-match" "@")         ; => "no-match"
```
