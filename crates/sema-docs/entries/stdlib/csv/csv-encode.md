---
name: "csv/encode"
module: "csv"
section: "CSV"
params: [{ name: rows, type: list | vector }]
returns: "string"
see_also: ["csv/parse", "csv/parse-maps", "string/join"]
---

Encode a list of lists (or vectors) into a CSV string. Each inner list/vector becomes one row. Non-string values are stringified automatically.

**Signature:** `(csv/encode rows) → string`

```sema
(csv/encode '(("a" "b") ("1" "2")))
; => "a,b\n1,2\n"
```

Numeric and other values are converted to strings:

```sema
(csv/encode '(("name" "score") ("Ada" 100)))
; => "name,score\nAda,100\n"
```
