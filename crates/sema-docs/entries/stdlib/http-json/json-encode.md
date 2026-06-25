---
name: "json/encode"
module: "http-json"
section: "JSON"
params: [{ name: value, type: any, doc: "any JSON-encodable Sema value" }]
returns: "string"
---

```
(json/encode value) → string
```

Encode a Sema value as a compact JSON string. Uses **strict** conversion — errors on values that cannot be represented in JSON (functions, records, NaN, Infinity).

- **value** — any JSON-encodable Sema value

```sema
(json/encode 42)                    ; => "42"
(json/encode "hello")               ; => "\"hello\""
(json/encode #t)                    ; => "true"
(json/encode nil)                   ; => "null"
(json/encode '(1 2 3))             ; => "[1,2,3]"
(json/encode [1 2 3])              ; => "[1,2,3]"
(json/encode {:name "Ada" :age 36}) ; => "{\"age\":36,\"name\":\"Ada\"}"
```

Encoding errors:

```sema
;; NaN and Infinity cannot be represented in JSON
(json/encode (/ 0.0 0.0))   ; Error: cannot encode NaN/Infinity as JSON

;; Functions cannot be encoded
(json/encode println)        ; Error: cannot encode native-fn as JSON
```
