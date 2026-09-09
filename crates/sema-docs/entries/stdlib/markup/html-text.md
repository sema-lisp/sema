---
name: "html/text"
module: "markup"
section: "Markdown & HTML"
params: [{ name: html, type: string }]
returns: "string"
see_also: ["html/select-text", "text/strip-html", "html/parse"]
---

Extract text content from an HTML string, stripping tags and excluding `script`,
`style`, and `template` subtrees. Remaining text is concatenated and
whitespace-collapsed into a single trimmed string. This does not run CSS or
JavaScript, so CSS-based visibility is not evaluated.

```sema
(html/text "<div><p>hello</p> <p>world</p></div>")
;; => "hello world"
```

```sema
;; Strip markup to get plain text for indexing or display
(html/text "<h1>Title</h1><p>Some <b>bold</b> body.</p>")
;; => "Title Some bold body."
```
