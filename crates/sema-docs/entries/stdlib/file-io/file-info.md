---
name: "file/info"
module: "file-io"
section: "Directory Operations"
---

Get file metadata. Returns a map with `:size`, `:modified`, and other keys.

```sema
(file/info "data.txt")   ; => {:size 1234 :modified 1707955200 ...}
```
