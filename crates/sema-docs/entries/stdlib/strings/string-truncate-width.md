---
name: "string/truncate-width"
module: "strings"
section: "Core String Operations"
syntax: "(string/truncate-width s width [ellipsis])"
returns: "string"
---

Clamp a string to a target **display width**, in columns — the truncation counterpart to `string/width`. Splits on grapheme-cluster boundaries, so wide characters (CJK, most emoji) are never cut in half. A string already at or under the width is returned unchanged; like `string/pad-left`/`string/pad-right` only pad, this only shrinks.

With an optional `ellipsis` string, an over-width input is truncated to leave room for it, and the ellipsis is appended — the result never exceeds `width` columns. If `ellipsis` itself is wider than `width`, it's truncated in the content's place.

```sema
(string/truncate-width "hello world" 5)        ; => "hello"
(string/truncate-width "hi" 10)                ; => "hi"            ; already fits, unchanged
(string/truncate-width "日本語です" 6)          ; => "日本語"        ; grapheme-safe (wide chars = 2 cols)
(string/truncate-width "hello world" 6 "…")    ; => "hello…"
(string/truncate-width "hi" 6 "…")             ; => "hi"            ; fits, ellipsis unused
```
