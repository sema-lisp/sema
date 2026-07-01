---
name: "diff/hunks"
module: "diff"
section: "Diff & Patch"
syntax: "(diff/hunks patch)"
returns: "list"
---

Parse a unified-diff string into a list of hunk maps. Each map has `{:header "@@ ... @@" :old-start <int> :old-count <int> :new-start <int> :new-count <int> :lines [...]}`, where `:lines` holds the hunk body lines including their leading marker (`" "`, `"+"`, or `"-"`). Counts default to 1 when omitted from the header.

```sema
(diff/hunks (diff/unified "a\nb\nc\n" "a\nB\nc\n"))
```
