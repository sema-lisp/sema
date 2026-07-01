---
name: "html/parse"
module: "markup"
section: "Markdown & HTML"
---

Parse and normalize an HTML document, returning the normalized HTML as a string. The returned string is the handle accepted by `html/select`, `html/text`, and `html/select-text`, which re-parse it internally. Parsing is lenient (html5ever), so malformed markup is repaired rather than rejected.

```sema
(html/parse "<p>hello<p>world")
```

```sema
;; The result feeds the other html functions
(html/text (html/parse "<h1>Title</h1>"))
;; => "Title"
```
