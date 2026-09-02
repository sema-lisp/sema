---
name: "sema/check-string"
module: "reflect"
section: "Reflection"
params: [{ name: s, type: string }]
returns: "map"
see_also: ["sema/check-file", "read/all", "eval"]
---

Check a Sema source string and return diagnostics as data. The argument `s` is parsed and then compiled without running it, so syntax errors and compile errors (unbound symbols, arity and type errors the compiler can see) are reported while runtime errors are not. The result is a map `{:ok bool :diagnostics list}`; each diagnostic is a map with `:level` (always `:error`), `:code` (`"syntax"`, `"unbound-symbol"`, `"arity"`, `"type"`, `"internal"`, or `"error"`), `:message`, and for syntax errors a `:span` map with `:line`, `:col`, `:end-line`, and `:end-col`. Built for agent repair loops: check, read the diagnostics, edit, check again.

```sema
(sema/check-string "(+ 1 2)")           ; => {:diagnostics () :ok #t}
(:ok (sema/check-string "(+ 1"))         ; => #f
(:code (first (:diagnostics (sema/check-string "(+ 1"))))   ; => "syntax"
```
