---
name: "type"
module: "records"
section: "Introspection"
params: [{ name: v, type: any }]
returns: "keyword"
see_also: ["type-of", "record?", "define-record-type"]
---

Return the type of a value as a keyword. For records, returns the record's type name:

```sema
(type (make-point 3 4))   ; => :point
(type [1 2 3])            ; => :vector
(type {:a 1})             ; => :map
```
