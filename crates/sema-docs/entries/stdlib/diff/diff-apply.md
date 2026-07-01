---
name: "diff/apply"
module: "diff"
section: "Diff & Patch"
syntax: "(diff/apply content patch)"
returns: "string"
---

Apply a unified-diff `patch` to the `content` string and return the patched result. Each hunk is located by its recorded line number and surrounding context; if the context or deleted lines do not match the content, an error is raised. Round-trips with `diff/unified`: applying `(diff/unified old new)` to `old` reconstructs `new`.

```sema
(diff/apply "a\nb\nc\n" (diff/unified "a\nb\nc\n" "a\nB\nc\n"))
```
