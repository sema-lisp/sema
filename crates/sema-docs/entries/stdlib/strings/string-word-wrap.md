---
name: "string/word-wrap"
module: "strings"
section: "Core String Operations"
params: [{ name: text, type: string }, { name: width, type: int, doc: "max display columns per line" }]
returns: "list"
---

Word-wrap `text` to lines of at most `width` display columns, returning a list of line strings. Wraps on spaces (collapsing runs), hard-breaks any word longer than `width` on grapheme-cluster boundaries, and preserves explicit newlines as line breaks. Widths are measured with `string/width`, so wrapping is correct for non-ASCII text. (Distinct from `string/wrap`, which wraps a string in left/right delimiters.)

```sema
(string/word-wrap "the quick brown fox" 10)
; => ("the quick" "brown fox")

(string/word-wrap "日本語 の テスト" 8)
; => ("日本語" "の" "テスト")
```
