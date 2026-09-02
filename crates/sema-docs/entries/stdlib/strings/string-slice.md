---
name: "string/slice"
module: "strings"
section: "Scheme Compatibility Aliases"
aliases: ["substring"]
params: [{ name: s, type: string }, { name: start, type: int }, { name: end, type: int, doc: "optional, defaults to string length" }]
returns: "string"
see_also: ["string/take", "string/ref"]
---

Extract a substring by character index: `start` (inclusive) to `end` (exclusive). With one index, it slices to the end of the string. Indices count Unicode scalar characters, not bytes, so multi-byte characters are handled correctly. An out-of-range `end` raises rather than clamping — use `string/take` for length-based, clamping extraction.

```sema
(string/slice "hello" 1 3)   ; => "el"
(string/slice "hello" 0 5)   ; => "hello"
(string/slice "hello" 2)     ; => "llo"   ; one index = to end
(string/slice "héllo" 1 2)   ; => "é"     ; counts characters, not bytes
```
