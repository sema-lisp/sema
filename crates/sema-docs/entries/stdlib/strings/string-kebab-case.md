---
name: "string/kebab-case"
module: "strings"
section: "Case Conversion"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/snake-case", "string/camel-case", "string/pascal-case", "string/headline"]
---

Convert any identifier to kebab-case (lowercase words joined by hyphens). Like its siblings `string/snake-case`, `string/camel-case`, and `string/pascal-case`, it tokenizes with `string/words` — so whitespace, underscores, hyphens, and camelCase humps are all treated as boundaries.

```sema
(string/kebab-case "helloWorld")    ; => "hello-world"
(string/kebab-case "Hello World")   ; => "hello-world"
(string/kebab-case "HTTPServer")    ; => "http-server"
```
