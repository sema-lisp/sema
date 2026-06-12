---
name: "type-of"
module: "predicates"
section: "Type Predicates"
params: [{ name: x, type: any }]
returns: keyword
---

Return the type of `x` as a keyword. Alias of `type`. For records, returns the record's type tag.

```sema
(type-of 42)       ; => :int
(type-of "hi")     ; => :string
(type-of '(1 2))   ; => :list
```
