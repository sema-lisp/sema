---
name: "json/decode"
module: "http-json"
section: "JSON"
---

```
(json/decode json-string) → value
```

Decode a JSON string into a Sema value. JSON objects become maps with keyword keys, arrays become lists. See the [type mapping table](#decoding-json-sema) for full details.

- **json-string** — a string containing valid JSON

```sema
(json/decode "42")                          ; => 42
(json/decode "3.14")                        ; => 3.14
(json/decode "\"hello\"")                   ; => "hello"
(json/decode "true")                        ; => #t
(json/decode "null")                        ; => nil
(json/decode "[1, 2, 3]")                   ; => (1 2 3)
(json/decode "{\"name\": \"Ada\"}")         ; => {:name "Ada"}
```

Decoding errors:

```sema
;; Invalid JSON throws an error
(json/decode "not json")    ; Error: json/decode: expected value at line 1 column 1

;; Argument must be a string
(json/decode 42)            ; Error: type error: expected string, got int
```
