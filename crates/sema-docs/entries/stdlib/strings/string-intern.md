---
name: "string/intern"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/to-symbol", "string/to-keyword", "symbol/to-string"]
---

Intern a string, returning a shared instance for equal contents. Repeated calls with equal strings return values backed by the same underlying storage, which can reduce memory for many duplicated strings.

```sema
(string/intern "hello")   ; => "hello"
```
