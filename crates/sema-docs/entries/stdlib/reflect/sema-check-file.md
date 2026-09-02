---
name: "sema/check-file"
module: "reflect"
section: "Reflection"
params: [{ name: path, type: string }]
returns: "map"
see_also: ["sema/check-string", "read/all", "file/read"]
---

Like `sema/check-string` but reads a file first. The argument `path` is a file path; the file is read as UTF-8 source and checked the same way `sema/check-string` checks a string. The result is the same map shape, `{:ok bool :diagnostics list}`; when the file cannot be read, `:ok` is `#f` and the single diagnostic has `:code "io"` with the operating-system message. Reading the file needs the `fs-read` capability in a sandbox.

```sema
(sema/check-file "src/main.sema")        ; {:diagnostics () :ok #t} when the file compiles
(:ok (sema/check-file "no-such-file.sema"))   ; #f, with one :code "io" diagnostic
```
