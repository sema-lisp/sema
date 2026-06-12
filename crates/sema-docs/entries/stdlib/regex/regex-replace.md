---
name: "regex/replace"
module: "regex"
section: "Replacement"
---

Replace the **first** match of a pattern.

**Signature:** `(regex/replace pattern replacement text) → string`

```sema
(regex/replace #"\d+" "X" "a1b2c3")    ; => "aXb2c3"
```

Capture group references (`$1`, `$2`, …) work in the replacement string:

```sema
(regex/replace #"(\d+)-(\w+)" "$2:$1" "item-42-foo")
; => "item-foo:42"
```

Named capture groups also work:

```sema
(regex/replace #"(?P<num>\d+)-(?P<word>\w+)" "$word:$num" "item-42-foo")
; => "item-foo:42"
```
