---
outline: [2, 3]
---

# Macros & Modules

## Macros

Sema supports `defmacro`-style macros with quasiquoting, unquoting, and splicing.

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
