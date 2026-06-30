---
name: "markdown/headings"
module: "markup"
section: "Markdown & HTML"
---

Extract the headings from a Markdown document in document order. Returns a list of maps, each with a `:level` integer (1 through 6) and a `:text` string holding the heading's plain text content.

```sema
(markdown/headings "# Intro\n\n## Setup\n\n## Usage")
;; => ({:level 1 :text "Intro"} {:level 2 :text "Setup"} {:level 2 :text "Usage"})
```

```sema
;; Build a table of contents from a document
(map (lambda (h) (:text h))
     (markdown/headings "# A\n\n## B\n\n### C"))
```
