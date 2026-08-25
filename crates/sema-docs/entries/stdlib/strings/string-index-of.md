---
name: "string/index-of"
module: "strings"
section: "Core String Operations"
---

Return the character index of the first occurrence of a substring, or `nil` if not found. The index counts characters, so check for `nil` rather than treating the result as a boolean (index `0` is a valid match and is falsey-looking but not `nil`). Use `string/last-index-of` to search from the end.

An optional third argument sets the character index to start searching from — the returned index is still relative to the start of the whole string, not to the offset. Use it to walk past earlier matches.

```sema
(string/index-of "hello" "ll")            ; => 2
(string/index-of "banana" "a")            ; => 1   ; first match only
(string/index-of "hello" "h")             ; => 0   ; match at start is index 0, not nil
(string/index-of "hello" "xyz")           ; => nil
(string/index-of "abcabc" "bc" 2)         ; => 4   ; skip the match at index 1
(string/index-of "abcabc" "bc" 5)         ; => nil ; no match left after the offset
```
