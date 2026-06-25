---
name: "string/codepoints"
module: "strings"
section: "Unicode & Encoding"
params: [{ name: s, type: string }]
returns: "list"
---

Return a list of Unicode codepoint integers for each character in a string. This reveals the internal structure of composed characters and emoji sequences.

```sema
(string/codepoints "ABC")      ; => (65 66 67)
(string/codepoints "é")        ; => (233)
(string/codepoints "😀")       ; => (128512)
```

Emoji that appear as a single glyph are often multiple codepoints joined by Zero Width Joiner (U+200D = 8205):

```sema
;; 👨‍👩‍👦 is actually 👨 + ZWJ + 👩 + ZWJ + 👦
(string/codepoints "👨‍👩‍👦")   ; => (128104 8205 128105 8205 128102)

;; 👋🏽 is 👋 + skin tone modifier
(string/codepoints "👋🏽")      ; => (128075 127997)
```
