---
name: "defmacro"
module: "special-forms"
syntax: "(defmacro name (params ...) body ...)"
---

Define a compile-time macro. Macros receive their arguments unevaluated as Sema data structures and must return a new form that the evaluator then expands and evaluates. `defmacro` takes a name symbol, a parameter list, and one or more body expressions.

Unlike `lambda`, macros do not capture a closure environment. Macro expansion uses the environment active at the call site. Parameter lists support rest arguments via dot notation. The body is typically written with quasiquote (`` ` ``) to construct the output form conveniently, using `,expr` to unquote evaluated subexpressions and `,@expr` for unquote-splicing.

`defmacro` always returns `nil`.

```sema
(defmacro unless (test body)
  `(if (not ,test) ,body))
(unless (> 3 5) "it is false")  ; => "it is false"
```

```sema
(defmacro when-let (binding then)
  `(let (,binding)
     (when ,(car binding) ,then)))
(when-let (x 42) (* x 2))  ; => 84
```

```sema
(defmacro my-or (a b)
  `(let ((temp ,a))
     (if temp temp ,b)))
(my-or #f 99)  ; => 99
```
