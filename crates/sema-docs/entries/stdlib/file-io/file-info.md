---
name: "file/info"
module: "file-io"
section: "Directory Operations"
params: [{ name: path, type: string }]
returns: "map"
see_also: ["file/exists?", "file/is-file?", "file/is-directory?"]
---

Get file metadata. Returns a map with `:size` (bytes), `:modified` (Unix epoch
**milliseconds**), `:is-file`, and `:is-dir`.

```sema
(file/info "data.txt")
; => {:is-dir #f :is-file #t :modified 1782248141021 :size 1234}
```
