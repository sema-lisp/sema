---
name: "def"
module: "special-forms"
syntax: "(def name value)"
---

Bind a value to a name in the current environment. `def` is a silent alias for `define`, provided for compatibility with other Lisp dialects (Clojure, Scheme, etc.). It behaves identically to `define`: it creates a top-level binding, and if the name already exists, the previous value is overwritten.

Like `define`, `def` supports destructuring patterns in the binding position. You can destructure vectors with `[a b c]` or maps with `{:keys [name age]}` to extract values directly.

```sema
(def x 42)
x  ; => 42
```

Defining a function with `def` using `fn`:

```sema
(def square (fn (x) (* x x)))
(square 7)  ; => 49
```

Destructuring with `def`:

```sema
(def [a b c] '(1 2 3))
(+ a b c)  ; => 6

(def {:keys [host port]} {:host "localhost" :port 8080})
host  ; => "localhost"
port  ; => 8080
```

**Note:** `def` does not create local bindings — for that, use `let`, `let*`, or `letrec`.
