---
outline: [2, 3]
---

# Macros & Modules

## Macros

Sema supports two macro systems: procedural `defmacro`-style macros with quasiquoting, unquoting, and splicing, and R7RS pattern-based [`define-syntax` / `syntax-rules`](#define-syntax-syntax-rules) macros with automatic hygiene.

### `defmacro`

Define a macro that transforms code at expansion time.

```sema
(defmacro unless2 (test . body)
  `(if ,test nil (begin ,@body)))

(unless2 #f (println "runs!"))
```

### `macroexpand`

Inspect the expansion of a macro call without evaluating it.

```sema
(macroexpand '(unless2 #f (println "x")))
```

### `gensym`

Generate a unique symbol manually. For most macro use cases, prefer [auto-gensym (`foo#`)](#auto-gensym-foo) instead.

```sema
(gensym "tmp")   ; => tmp__42 (unique each call)
```

### Auto-gensym (`foo#`)

Inside a quasiquote template, any symbol ending with `#` is automatically replaced with a unique generated symbol. All occurrences of the same `foo#` within a single quasiquote resolve to the same gensym, ensuring consistency.

This prevents **variable capture** — a common bug where macro-introduced bindings accidentally shadow user variables.

```sema
;; Without auto-gensym — BUG if user has a variable named "tmp"
(defmacro bad-inc (x)
  `(let ((tmp ,x)) (+ tmp 1)))

(let ((tmp 100))
  (bad-inc tmp))   ; => 2, not 101! "tmp" is captured

;; With auto-gensym — always correct
(defmacro good-inc (x)
  `(let ((tmp# ,x)) (+ tmp# 1)))

(let ((tmp 100))
  (good-inc tmp))  ; => 101 ✓
```

**Rules:**
- Same `foo#` in one quasiquote → same generated symbol
- Each quasiquote evaluation → fresh symbols (no cross-expansion collisions)
- Outside quasiquote, `foo#` is a regular symbol (no magic)

**Best practice:** Always use auto-gensym for bindings introduced by macros:

```sema
(defmacro swap! (a b)
  `(let ((tmp# ,a))
     (set! ,a ,b)
     (set! ,b tmp#)))
```

### `define-syntax` / `syntax-rules`

R7RS pattern-based macros. Instead of computing an expansion with quasiquote, you declare rewrite rules — each a `(pattern template)` pair. Rules are tried in order and the first matching pattern wins; `...` (ellipsis) matches a sequence of forms.

```sema
(define-syntax my-or
  (syntax-rules ()
    ((_) #f)
    ((_ e) e)
    ((_ e1 e2 ...)
     (let ((t e1)) (if t t (my-or e2 ...))))))

(my-or #f #f 7)   ; => 7
```

The first argument to `syntax-rules` is a list of **literal identifiers** — symbols that must appear verbatim in the call rather than binding a pattern variable:

```sema
(define-syntax go
  (syntax-rules (to)
    ((_ to x) (list :to x))
    ((_ x)    (list :plain x))))

(go to 1)   ; => (:to 1)
(go 2)      ; => (:plain 2)
```

Nested ellipsis in patterns lets you destructure binding-list shapes:

```sema
(define-syntax my-let
  (syntax-rules ()
    ((_ ((name val) ...) body)
     ((lambda (name ...) body) val ...))))

(my-let ((a 1) (b 2)) (+ a b))   ; => 3
```

#### Hygiene

`syntax-rules` templates are hygienic without manual gensyms. Hygiene is **binder-directed**: identifiers the template introduces *as binders* (the variables of a template's `let`, `let*`, `letrec`, `lambda`, `define`, `do`, or named `let`) are automatically alpha-renamed to a fresh symbol on every expansion — so they can never capture a user variable of the same name:

```sema
(define-syntax swap!
  (syntax-rules ()
    ((_ a b)
     (let ((tmp a)) (set! a b) (set! b tmp)))))

(define tmp 1)
(define x 2)
(swap! tmp x)
(list tmp x)   ; => (2 1) — the template's tmp did not capture the user's tmp
```

Every *other* template identifier — free references to user-defined globals, builtins, and the macro's own name for recursion — is kept verbatim and resolves at the use site at runtime:

```sema
(define (double n) (* 2 n))
(define-syntax d (syntax-rules () ((_ x) (double x))))
(d 21)   ; => 42
```

`macroexpand` works on `syntax-rules` macros too and shows the renaming:

```sema
(macroexpand '(swap! x y))
;; => (let ((tmp__0 x)) (set! x y) (set! y tmp__0))
```

**Caveats** (the hygiene is an approximation, not full R7RS referential transparency):

- Only template-introduced *binders* are renamed. The other direction isn't covered: if the use site shadows a global or special form that the template references freely, the template sees the shadowing binding.
- Templates support a single level of ellipsis; a template with ellipsis depth > 1 (`x ... ...`) is rejected with a clear error rather than mis-expanded.
- Like `defmacro`, `define-syntax` must appear at the top level (or inside a top-level `begin`) to be visible to sibling forms — one nested inside a lambda or `let` body is not.
- `syntax-case` is not supported.

#### `defmacro` or `syntax-rules`?

- **`syntax-rules`** — reach for it when the macro is a structural rewrite: it's declarative, hygiene is automatic, and the definition is portable R7RS.
- **`defmacro`** — reach for it when the expansion needs real computation (inspecting arguments, generating different shapes, calling functions at expansion time): the whole language is available. Use [auto-gensym (`foo#`)](#auto-gensym-foo) for any bindings the expansion introduces.

### Built-in Macros

Sema includes several macros that are auto-loaded at startup. These don't need to be defined or imported:

- `->`, `->>`, `as->`, `some->` — [Threading macros](./special-forms.html#threading-macros)
- `when-let`, `if-let` — [Conditional binding](./special-forms.html#when-let)

See [Special Forms](./special-forms.html) for full documentation.

## Metaprogramming

### `eval`

Evaluate data as code.

```sema
(eval '(+ 1 2))   ; => 3
```

### `read`

Parse a string into a Sema value.

```sema
(read "(+ 1 2)")   ; => (+ 1 2) as a list value
```

### `io/read-many`

Parse a string containing multiple forms.

```sema
(io/read-many "(+ 1 2) (* 3 4)")   ; => ((+ 1 2) (* 3 4))
```

### `type`

Return the type of a value as a keyword.

```sema
(type 42)              ; => :int
(type 3.14)            ; => :float
(type "hi")            ; => :string
(type :foo)            ; => :keyword
(type 'foo)            ; => :symbol
(type '(1 2 3))        ; => :list
(type [1 2 3])         ; => :vector
(type {:a 1})          ; => :map
```

For records, `type` returns the record type tag as a keyword (e.g. `:point`).

### Type Conversion Functions

```sema
(string/to-symbol "foo")       ; => foo
(keyword/to-string :bar)       ; => "bar"
(string/to-keyword "name")     ; => :name
(symbol/to-string 'foo)        ; => "foo"
```

## Modules

### `module`

Define a module with explicit exports.

```sema
;; math-utils.sema
(module math-utils
  (export square cube)
  (define (square x) (* x x))
  (define (cube x) (* x x x))
  (define (internal-helper x) x))      ; not exported
```

### `import`

Import a module from a file. Only exported bindings become available.

```sema
;; main.sema
(import "math-utils.sema")
(square 5)   ; => 25
(cube 3)     ; => 27
```
