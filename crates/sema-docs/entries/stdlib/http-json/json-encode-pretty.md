---
name: "json/encode-pretty"
module: "http-json"
section: "JSON"
params: [{ name: value, type: any, doc: "any JSON-encodable Sema value" }]
returns: "string"
see_also: ["json/encode", "json/decode", "pprint"]
---

```
(json/encode-pretty value) → string
```

Encode a Sema value as a pretty-printed JSON string with 2-space indentation. Same strict conversion rules as `json/encode`.

- **value** — any JSON-encodable Sema value

```sema
(json/encode-pretty {:name "Ada" :scores [95 87 92]})
;; =>
;; {
;;   "name": "Ada",
;;   "scores": [
;;     95,
;;     87,
;;     92
;;   ]
;; }
```
