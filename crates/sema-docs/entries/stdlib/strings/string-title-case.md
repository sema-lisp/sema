---
name: "string/title-case"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/snake-case", "string/kebab-case", "string/camel-case", "string/pascal-case"]
---

Capitalize the first character of each whitespace-separated word, leaving the rest untouched. It splits on spaces only — underscores, hyphens, and camelCase humps are *not* word boundaries.

For input that uses `_`/`-` separators or camelCase (e.g. identifiers), use `string/headline` instead, which normalizes those into spaced Title Case.

```sema
(string/title-case "hello world")   ; => "Hello World"
(string/title-case "hello_world")   ; => "Hello_world"   ; underscore not a boundary
(string/title-case "helloWorld")    ; => "Helloworld"    ; lowercases the rest
(string/headline   "helloWorld")    ; => "Hello World"   ; use headline for these
```
