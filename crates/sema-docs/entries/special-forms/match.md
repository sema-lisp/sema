---
name: "match"
module: "special-forms"
syntax: "(match expr [pattern body ...] [pattern when guard body ...] ...)"
---

Pattern-match a value against a series of clauses. Each clause consists of a pattern, an optional `when` guard, and one or more body expressions. The first clause whose pattern matches the value and whose guard (if present) evaluates to truthy is executed, and its last body's value is returned. If no clause matches, `match` **raises** `match: no clause matched` — it does not silently return `nil`. Add a catch-all `(_ ...)` clause, or use `match*` (the sibling form) when you want `nil` on no match instead of an error.

Patterns can be literals (numbers, strings, keywords, symbols), vectors (matching lists or vectors by structure), maps (matching maps by keys), or binding patterns (symbols that capture the matched value). The wildcard `_` matches any value without binding it. Nested patterns are fully supported, allowing deep structural matching in a single clause. Guards add arbitrary boolean conditions after a pattern match using `when`.

Explicit map entries such as `{:type :ok}` require the key to exist and its
value to match. The `{:keys [name]}` shorthand extracts keyword keys instead;
a missing key binds the corresponding symbol to `nil` and does not make the
clause fail.

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

No matching clause is an error, so make patterns exhaustive (or add `_`):

```sema
(match 5 (1 :one) (2 :two))   ; => error: match: no clause matched value: 5
(match 5 (1 :one) (_ :other)) ; => :other
(match* 5 (1 :one) (2 :two))  ; => nil  (match* tolerates no match)
```

**Note:** `match` lowers to nested `if`/`let*` chains with a runtime `__vm-try-match` helper.
