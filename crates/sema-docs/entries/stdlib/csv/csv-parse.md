---
name: "csv/parse"
module: "csv"
section: "CSV"
params: [{ name: csv-string, type: string }]
returns: "list"
see_also: ["csv/parse-maps", "csv/encode", "string/lines"]
---

Parse a CSV string into a list of lists (rows of fields). No header processing — every row (including the first) is returned as-is, with every field a string. Use `csv/parse-maps` when the first row is a header and you want maps keyed by column name.

**Signature:** `(csv/parse csv-string) → list`

```sema
(csv/parse "a,b\n1,2\n3,4")
; => (("a" "b") ("1" "2") ("3" "4"))
```

Quoted fields with commas and newlines are handled correctly:

```sema
(csv/parse "name,bio\n\"Ada\",\"Mathematician, writer\"\n")
; => (("name" "bio") ("Ada" "Mathematician, writer"))
```
