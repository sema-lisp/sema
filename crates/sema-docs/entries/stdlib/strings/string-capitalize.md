---
name: "string/capitalize"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/snake-case", "string/kebab-case", "string/camel-case", "string/pascal-case"]
---

Uppercase the first character and lowercase the rest.

```sema
(string/capitalize "hello")   ; => "Hello"
(string/capitalize "hELLO")   ; => "Hello"
```
