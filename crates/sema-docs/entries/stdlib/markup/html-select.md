---
name: "html/select"
module: "markup"
section: "Markdown & HTML"
---

Run a CSS selector against an HTML string and return a list of the matched elements as their outer HTML strings. Supports the full CSS selector syntax (tags, classes, ids, attributes, combinators). An invalid selector raises an error.

```sema
(html/select "<p class=x>a</p><p>b</p>" "p.x")
;; => ("<p class=\"x\">a</p>")
```

```sema
;; Pull every link element out of a page
(html/select "<a href=/1>one</a><a href=/2>two</a>" "a")
```
