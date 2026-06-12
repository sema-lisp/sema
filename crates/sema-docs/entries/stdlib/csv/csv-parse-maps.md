---
name: "csv/parse-maps"
module: "csv"
section: "CSV"
---

Parse a CSV string into a list of maps. The first row is used as headers, which become keyword keys in each map.

**Signature:** `(csv/parse-maps csv-string) → list`

```sema
(csv/parse-maps "name,age\nAda,36\nBob,25")
; => ({:age "36" :name "Ada"} {:age "25" :name "Bob"})
```

Access fields by keyword:

```sema
(define rows (csv/parse-maps "name,age\nAda,36\nBob,25"))
(:name (first rows))   ; => "Ada"
```
