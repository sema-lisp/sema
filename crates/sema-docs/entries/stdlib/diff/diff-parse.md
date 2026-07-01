---
name: "diff/parse"
module: "diff"
section: "Diff & Patch"
syntax: "(diff/parse patch)"
returns: "map"
---

Parse a (possibly multi-file) unified-diff string into a structured map `{:files [ {:old-path :new-path :hunks [...]} ... ]}`. File paths come from the `--- a/path` and `+++ b/path` headers (nil if absent), and each file's `:hunks` use the same shape as `diff/hunks`. A bare hunk-only patch yields a single file with nil paths.

```sema
(diff/parse "--- a/x.txt\n+++ b/x.txt\n@@ -1 +1 @@\n-old\n+new\n")
```
