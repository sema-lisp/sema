---
name: "csv/parse-maps"
module: "csv"
section: "CSV"
params: [{ name: csv-string, type: string }]
returns: "list"
see_also: ["csv/parse", "csv/encode"]
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

Every value is a **string** — there is no type inference, so coerce numbers yourself (e.g. `(string->number (:age row))`). Use `csv/parse` instead when you want raw positional rows and no header handling.
