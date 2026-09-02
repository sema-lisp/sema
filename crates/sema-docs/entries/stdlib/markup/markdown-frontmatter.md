---
name: "markdown/frontmatter"
module: "markup"
section: "Markdown & HTML"
params: [{ name: markdown, type: string }]
returns: "map"
see_also: ["markdown/headings", "markdown/to-html", "toml/decode"]
---

Split a leading `---` fenced frontmatter block from a Markdown document. Returns a map with `:frontmatter` (the raw block text, or `nil` when no frontmatter is present) and `:body` (the remaining document). The frontmatter block is returned verbatim; it is not parsed as YAML or TOML.

```sema
(markdown/frontmatter "---\ntitle: Hello\n---\nBody text")
;; => {:frontmatter "title: Hello\n" :body "Body text"}
```

```sema
;; No frontmatter -> :frontmatter is nil, :body is the original
(markdown/frontmatter "Just a plain document")
;; => {:frontmatter nil :body "Just a plain document"}
```
