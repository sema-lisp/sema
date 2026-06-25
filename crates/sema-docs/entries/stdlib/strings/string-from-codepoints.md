---
name: "string/from-codepoints"
module: "strings"
section: "Unicode & Encoding"
params: [{ name: codepoints, type: list, doc: "list or vector of integer codepoints" }]
returns: "string"
---

Construct a string from a list of Unicode codepoint integers. This is the inverse of `string/codepoints` and enables building emoji programmatically by combining codepoints.

```sema
(string/from-codepoints (list 65 66 67))   ; => "ABC"
(string/from-codepoints (list 233))        ; => "é"
```

Build emoji by combining people with ZWJ (8205):

```sema
;; Build a family: 👨 + ZWJ + 👩 + ZWJ + 👧
(string/from-codepoints (list 128104 8205 128105 8205 128103))
;; => 👨‍👩‍👧

;; Build a profession: 👩 + ZWJ + 💻
(string/from-codepoints (list 128105 8205 128187))
;; => 👩‍💻

;; Add skin tone: 👋 + modifier
(string/from-codepoints (list 128075 127997))
;; => 👋🏽

;; Build flags from Regional Indicators (A=127462):
(string/from-codepoints (list 127475 127476))
;; => 🇳🇴 (NO = Norway)
```

Roundtrip any string through codepoints:

```sema
(string/from-codepoints (string/codepoints "Hello 世界"))
;; => "Hello 世界"
```
