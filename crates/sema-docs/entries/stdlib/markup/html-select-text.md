---
name: "html/select-text"
module: "markup"
section: "Markdown & HTML"
params: [{ name: html, type: string }, { name: selector, type: string }]
returns: "list"
see_also: ["html/select", "html/text", "html/parse"]
---

Run a CSS selector against an HTML string and return a list of the matched elements' text content (one whitespace-collapsed string per match), with tags stripped. Like `html/select` but yields text rather than outer HTML. An invalid selector raises an error.

```sema
(html/select-text "<p class=x>alpha</p><p class=x>beta</p>" "p.x")
;; => ("alpha" "beta")
```

```sema
;; Scrape the text of every list item
(html/select-text "<ul><li>one</li><li>two</li></ul>" "li")
;; => ("one" "two")
```
