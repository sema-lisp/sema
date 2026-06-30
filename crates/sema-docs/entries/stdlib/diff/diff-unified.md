---
name: "diff/unified"
module: "diff"
section: "Diff & Patch"
syntax: "(diff/unified old new [context])"
returns: "string"
---

Produce a unified-diff string describing how to turn the `old` string into the `new` string, comparing line by line. The optional third argument sets the number of unchanged context lines kept around each change (default 3). The result uses `old`/`new` file labels and the standard `@@ -l,s +l,s @@` hunk headers, and is the canonical format consumed by `diff/apply`, `diff/stat`, `diff/hunks`, and `diff/parse`.

```sema
(diff/unified "a\nb\nc\n" "a\nc\nd\n")
```
