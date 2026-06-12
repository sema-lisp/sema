---
name: "defn"
module: "special-forms"
syntax: "(defn name (params ...) body ...)"
---

Define a named function. `defn` is an alias for `defun` and behaves identically. It expands into `(define (name params ...) body ...)`, taking a name symbol, a parameter list, and one or more body expressions.

Parameter lists support rest arguments via dot notation `(x . rest)` and destructuring patterns in parameter positions. The last body expression is evaluated in tail position. `defn` always returns `nil`.

Use whichever spelling matches your preference or codebase conventions. The evaluator recognizes both `defun` and `defn` as the same special form.

```sema
(defn add (a b)
  (+ a b))
(add 3 4)  ; => 7
```

```sema
(defn sum-pair [[a b]]
  (+ a b))
(sum-pair '(3 4))  ; => 7
```

```sema
(defn greet {:keys [name title]}
  (format "Hello ~a ~a" title name))
(greet {:name "Smith" :title "Dr."})  ; => "Hello Dr. Smith"
```
