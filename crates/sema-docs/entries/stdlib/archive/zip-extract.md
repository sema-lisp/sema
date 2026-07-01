---
name: "zip/extract"
module: "archive"
section: "Compression & Archives"
---

Extract every entry of the ZIP archive at `zip-path` into `dest-dir`, creating directories as needed. Entries whose paths would escape the destination (zip-slip via `..` or absolute roots) are skipped for safety. Returns the number of entries extracted. Requires the `fs-write` capability.

```sema
(zip/extract "bundle.zip" "out-dir")
```
