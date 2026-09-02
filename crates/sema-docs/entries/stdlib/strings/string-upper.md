---
name: "string/upper"
module: "strings"
section: "Core String Operations"
aliases: ["string-upcase"]
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/snake-case", "string/kebab-case", "string/camel-case", "string/pascal-case"]
---

Convert a string to uppercase (Unicode-aware). Some characters expand when uppercased — the German sharp s `ß` becomes `SS` — so the result can be longer than the input.

```sema
(string/upper "hello")   ; => "HELLO"
(string/upper "ß")       ; => "SS"
```
