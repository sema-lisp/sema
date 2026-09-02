---
name: "string/to-list"
module: "strings"
section: "Type Conversions"
aliases: ["string->list"]
params: [{ name: s, type: string }]
returns: "list"
see_also: ["list->string", "string/chars", "string/to-char"]
---

Split a string into a list of its characters (Unicode scalar values, not
bytes). Multi-byte characters such as `é` are single elements. The inverse is
`list->string`, which joins a list of chars back into a string.

Use it when an algorithm wants to walk or transform characters one at a time
with the list functions (`map`, `filter`, `reverse`). `string/chars` is the
same operation under its slash-namespaced name. For a list of one-character
*strings* rather than chars, use `(string/split s "")`.

```sema
(string/to-list "abc")                        ; => (#\a #\b #\c)
(string/to-list "héllo")                      ; => (#\h #\é #\l #\l #\o)
(list->string (string/to-list "ab"))          ; => "ab"
(map char-upcase (string/to-list "ab"))       ; => (#\A #\B)
(list->string (reverse (string/to-list "abc")))   ; => "cba"
```
