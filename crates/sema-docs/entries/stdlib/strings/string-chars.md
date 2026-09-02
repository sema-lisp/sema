---
name: "string/chars"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "list"
see_also: ["list->string", "string/to-list", "string/codepoints"]
---

Convert a string to a list of characters. Splits on Unicode *characters* (not bytes), so multi-byte glyphs stay whole. The inverse is `list->string`; for raw code points use `string/codepoints`.

```sema
(string/chars "abc")   ; => (#\a #\b #\c)
(string/chars "héy")   ; => (#\h #\é #\y)
```
