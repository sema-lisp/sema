---
name: "char?"
module: "predicates"
section: "Type Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["string?", "symbol?", "type-of"]
---

Test if a value is a character.

```sema
(char? #\a)      ; => #t
(char? "a")      ; => #f
```
