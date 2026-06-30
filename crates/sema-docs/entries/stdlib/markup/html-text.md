---
name: "html/text"
module: "markup"
section: "Markdown & HTML"
---

Extract the visible text content from an HTML string, stripping all tags. Text from every node is concatenated and whitespace-collapsed into a single trimmed string.

```sema
(html/text "<div><p>hello</p> <p>world</p></div>")
;; => "hello world"
```

```sema
;; Strip markup to get plain text for indexing or display
(html/text "<h1>Title</h1><p>Some <b>bold</b> body.</p>")
;; => "Title Some bold body."
```
