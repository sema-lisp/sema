---
outline: [2, 3]
---

# Markdown & HTML

Render Markdown and query HTML.

```sema
(markdown/to-html "# Title\n\nHello **world**.")
(markdown/headings md)             ; list of {:level :text}
(markdown/frontmatter md)          ; {:frontmatter :body}
(html/parse html)                  ; parsed document
(html/select html "a.button")      ; list of matched elements' outer HTML
(html/select-text html "h1")       ; list of matched elements' text
(html/text html)                   ; all visible text, whitespace-collapsed
```
