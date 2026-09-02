---
name: "string/camel-case"
module: "strings"
section: "Case Conversion"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/snake-case", "string/kebab-case", "string/pascal-case", "string/headline"]
---

Convert to camelCase, treating underscores, hyphens, and spaces as word boundaries. Siblings: `string/snake-case`, `string/kebab-case`, `string/capitalize`.

```sema
(string/camel-case "hello_world")    ; => "helloWorld"
(string/camel-case "Hello World")    ; => "helloWorld"
(string/camel-case "user-id-value")  ; => "userIdValue"
```
