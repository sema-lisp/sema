---
name: "string/headline"
module: "strings"
section: "Case Conversion"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/snake-case", "string/kebab-case", "string/camel-case", "string/pascal-case"]
---

Convert any identifier or sentence to a spaced Title Case headline. It treats whitespace, underscores, hyphens, and camelCase humps all as word boundaries, then capitalizes each word.

Use this (not `string/title-case`) when the input may be an identifier — `string/title-case` only splits on spaces.

```sema
(string/headline "hello_world")          ; => "Hello World"
(string/headline "helloWorld")           ; => "Hello World"
(string/headline "the_quick brown-fox")  ; => "The Quick Brown Fox"
```
