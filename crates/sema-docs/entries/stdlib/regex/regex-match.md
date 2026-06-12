---
name: "regex/match"
module: "regex"
section: "Matching"
---

Match a pattern and return match details as a map, or `nil` if no match.

**Signature:** `(regex/match pattern text) → map | nil`

The returned map contains:

| Key | Value |
|-----|-------|
| `:match` | The full matched substring |
| `:groups` | List of capture groups (group 1, 2, …) |
| `:start` | Start byte offset in the input |
| `:end` | End byte offset in the input |

```sema
(regex/match #"(\d+)-(\w+)" "item-42-foo")
; => {:match "42-foo" :groups ("42" "foo") :start 5 :end 11}

(regex/match #"xyz" "abc")
; => nil
```

Optional capture groups that don't participate in the match become `nil`:

```sema
(regex/match #"(\d+)(?:-(\d+))?" "42")
; => {:match "42" :groups ("42" nil) :start 0 :end 2}
```

`:start` and `:end` are byte offsets (UTF-8). For ASCII text they match character indices, but for non-ASCII they may differ.
