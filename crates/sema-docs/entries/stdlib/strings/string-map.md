---
name: "string/map"
module: "strings"
section: "Core String Operations"
params: [{ name: f, type: function }, { name: s, type: string }]
returns: "string"
see_also: ["string/chars", "list->string", "char/upcase"]
---

Apply a function to each character of a string, returning a new string. The function receives and must return a character (`#\...`), not a string — pair it with the `char/*` builtins. It's the string-flavored cousin of `map`, which would give you a list of characters instead.

```sema
(string/map char/upcase "hello")   ; => "HELLO"
(string/map (fn (c) (if (char=? c #\space) #\- c)) "a b c")  ; => "a-b-c"
```
