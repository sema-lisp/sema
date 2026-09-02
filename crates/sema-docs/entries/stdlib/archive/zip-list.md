---
name: "zip/list"
module: "archive"
section: "Compression & Archives"
params: [{ name: zip-path, type: string }]
returns: "list"
see_also: ["zip/create", "zip/extract"]
---

List the entry names contained in the ZIP archive at `zip-path`, returning a list of strings without extracting anything. Requires the `fs-read` capability.

```sema
(zip/list "bundle.zip")
```
