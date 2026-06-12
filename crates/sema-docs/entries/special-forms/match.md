---
name: "match"
module: "special-forms"
syntax: "(match expr [pattern body ...] [pattern when guard body ...] ...)"
---

Pattern-match a value against a series of clauses. Each clause consists of a pattern, an optional `when` guard, and one or more body expressions. The first clause whose pattern matches the value and whose guard (if present) evaluates to truthy is executed, and its last body's value is returned. If no clause matches, `match` returns `nil`.

Patterns can be literals (numbers, strings, keywords, symbols), vectors (matching lists or vectors by structure), maps (matching maps by keys), or binding patterns (symbols that capture the matched value). The wildcard `_` matches any value without binding it. Nested patterns are fully supported, allowing deep structural matching in a single clause. Guards add arbitrary boolean conditions after a pattern match using `when`.

```sema
(match status
  (:ok "success")
  (:error "failure")
  (_ "unknown"))
```

```sema
(match '(1 2 3)
  ([a b c] (+ a b c)))
;; => 6
```

```sema
(match response
  ({:type :ok :data d}   (process d))
  ({:type :error :msg m} (log-error m))
  (_                     (println "unknown")))
```

```sema
(match n
  (x when (> x 100) "big")
  (x when (> x 0)   "small")
  (_                 "non-positive"))
```

**Note:** `match` is implemented in both the tree-walker and the VM backend. In the VM, it lowers to nested `if`/`let*` chains with a runtime `__vm-try-match` helper.