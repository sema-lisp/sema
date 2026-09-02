---
name: "string/pascal-case"
module: "strings"
section: "Case Conversion"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/snake-case", "string/kebab-case", "string/camel-case", "string/headline"]
---

Convert any identifier to PascalCase (each word capitalized, no separators). Like `string/camel-case` but capitalizes the first word too; shares the `string/words` tokenizer with the other case converters.

```sema
(string/pascal-case "hello_world")   ; => "HelloWorld"
(string/pascal-case "hello world")   ; => "HelloWorld"
(string/pascal-case "http-server")   ; => "HttpServer"
```
