---
name: "markdown/to-html"
module: "markup"
section: "Markdown & HTML"
---

Render a CommonMark Markdown string to an HTML string. Uses `pulldown-cmark` for standards-compliant parsing, covering headings, emphasis, lists, links, code spans, and fenced code blocks.

```sema
(markdown/to-html "# Title\n\nHello **world**.")
```

```sema
;; Lists and inline code render too
(markdown/to-html "- one\n- two\n\nRun `make build`.")
```
