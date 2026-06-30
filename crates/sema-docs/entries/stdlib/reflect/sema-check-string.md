---
name: "sema/check-string"
module: "reflect"
section: "Reflection"
---

Check a Sema source string and return diagnostics as data: `{:ok bool :diagnostics [{:level :code :message :span?} ...]}`. Catches syntax and compile errors — built for agent repair loops.
