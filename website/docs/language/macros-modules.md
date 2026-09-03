---
outline: [2, 3]
---

# Macros & Modules

Sema has two macro systems and a file-based module system. Macros transform
forms before evaluation. Modules control which top-level bindings an imported
file exposes.

## Macro Expansion

When the evaluator sees a macro call, it passes the argument forms to the macro
without evaluating them. The macro returns a new form, which Sema expands again
and then evaluates. This recursive expansion means that one macro may produce a
call to another macro.

Macro definitions that must affect later forms in the same source unit belong
at the top level, or inside a top-level `begin`. A definition inside a function
or `let` is not available while sibling forms are compiled.

## Procedural Macros

### `defmacro`

`defmacro` defines a transformer with ordinary Sema code. Its parameters bind
to unevaluated forms. The body may inspect those forms, compute a result, and
return the form to evaluate.

```sema
(defmacro unless2 (test . body)
  `(if ,test nil (begin ,@body)))

(unless2 #f (+ 20 22)) ; => 42
```

The parameter list supports fixed and rest parameters. A procedural macro does
not capture the lexical environment where it was defined. Names used while
building or evaluating its expansion resolve in the environment of the call.

Quasiquote is the usual way to construct an expansion:

- `` `form `` quotes a template.
- `,expr` inserts one computed form.
- `,@expr` inserts every item from a computed list.

### `macroexpand`

`macroexpand` expands one outer macro call without evaluating the result. Quote
the input so that the call itself is passed as data.

```sema
(defmacro twice (x) (list '+ x x))
(macroexpand '(twice 4)) ; => (+ 4 4)
(twice 4)                ; => 8
```

If the outer form is not a macro call, `macroexpand` returns it unchanged. It
does not recursively expand macro calls inside the returned form.

### `gensym`

`gensym` creates a symbol that cannot collide with a source-level name.

```sema
(symbol? (gensym "tmp")) ; => #t
```

The printed suffix is intentionally unspecified. Use `gensym` when generated
names must be created by arbitrary macro logic. In quasiquote templates,
auto-gensym is shorter and less error-prone.

### Auto-gensym (`name#`)

Inside a quasiquote template, a symbol ending in `#` becomes a fresh generated
symbol. Repeated uses of the same spelling in one quasiquote use the same
generated symbol. A later evaluation of that quasiquote creates a new symbol.

```sema
(defmacro good-inc (x)
  `(let ((tmp# 1)) (+ tmp# ,x)))

(let ((tmp 100))
  (good-inc tmp)) ; => 101
```

Without a generated binding, a macro can capture a name from its caller:

```sema
(defmacro bad-inc (x)
  `(let ((tmp 1)) (+ tmp ,x)))

(let ((tmp 100))
  (bad-inc tmp)) ; => 2
```

Outside quasiquote, a name such as `tmp#` is an ordinary symbol. Use
auto-gensym for every temporary binding introduced by a procedural macro.

## Pattern Macros

### `define-syntax` and `syntax-rules`

`syntax-rules` defines ordered rewrite rules. Each rule has a pattern and a
template. The first matching pattern supplies the expansion. `_` is a wildcard,
and `...` matches or emits a sequence of forms.

```sema
(define-syntax my-or
  (syntax-rules ()
    ((_) #f)
    ((_ e) e)
    ((_ e1 e2 ...)
     (let ((t e1)) (if t t (my-or e2 ...))))))

(my-or #f #f 7) ; => 7
```

The first argument to `syntax-rules` lists literal identifiers. A literal must
appear exactly in that position instead of binding a pattern variable.

```sema
(define-syntax go
  (syntax-rules (to)
    ((_ to x) (list :to x))
    ((_ x)    (list :plain x))))

(go to 1) ; => (:to 1)
(go 2)    ; => (:plain 2)
```

Nested ellipses support nested input shapes when the template uses each pattern
variable at the same ellipsis depth.

```sema
(define-syntax rows
  (syntax-rules ()
    ((_ ((x ...) ...))
     (list (list x ...) ...))))

(rows ((1 2) (3 4))) ; => ((1 2) (3 4))
```

### Hygiene

Sema applies binder-directed hygiene to `syntax-rules` templates. Names that a
template introduces as binders in `let`, `let*`, `letrec`, `lambda`, `define`,
`do`, or named `let` are renamed for each expansion.

```sema
(define-syntax swap!
  (syntax-rules ()
    ((_ a b)
     (let ((tmp a)) (set! a b) (set! b tmp)))))

(define tmp 1)
(define x 2)
(swap! tmp x)
(list tmp x) ; => (2 1)
```

This is not full R7RS referential transparency. Free identifiers in a template
are kept as written and resolve at the use site. A caller can therefore shadow
a function or special form referenced by the template. Sema rejects a template
that uses a pattern variable at a deeper ellipsis level than its pattern.
`syntax-case` is not supported.

### Choosing a Macro System

Use `syntax-rules` for structural rewrites. It provides automatic hygiene for
introduced binders and makes the accepted syntax explicit. Use `defmacro` when
the transformer must run general Sema code or construct forms dynamically. Use
auto-gensym for bindings introduced by `defmacro` expansions.

## Built-in Macros

The prelude loads macros before user code runs. The main groups are:

| Purpose | Macros |
| --- | --- |
| Data flow | `->`, `->>`, `as->`, `some->` |
| Conditional binding and loops | `when-let`, `if-let`, `dotimes`, `for-range` |
| Resource and error handling | `with-open`, `with-stream`, `guard` |
| Concurrency | `parallel`, `pipeline`, `parallel-settled`, `pipeline-settled`, `settled-partition`, `async/map`, `async/pool-map`, `async/spawn-all`, `with-retry` |
| Observability | `with-span`, `with-session` |
| Policies and workflows | `defpolicy`, `policy/without`, `defworkflow`, `phase`, `step`, `approval`, `checkpoint` |

See [Special Forms](./special-forms.md), [Streams](../stdlib/streams.md),
[Concurrency](../stdlib/concurrency.md), [Observability](../llm/observability.md),
and [Workflows](../llm/workflows.md) for their evaluation rules and examples.

## Code as Data

### `eval`

`eval` evaluates a value as code in the current environment.

```sema
(eval '(+ 1 2)) ; => 3
```

### `read` and `io/read-many`

`read` parses one form from a string. `io/read-many` parses all forms in a
string and returns them as a list. Neither function evaluates the parsed forms.

```sema
(read "(+ 1 2)") ; => (+ 1 2)
(io/read-many "(+ 1 2) (* 3 4)") ; => ((+ 1 2) (* 3 4))
```

## Modules

An imported file is evaluated in a module environment that can read global and
prelude bindings but not the caller's local variables. After evaluation,
`import` copies the file's exports into the caller. An explicit `module` form
can keep other definitions private, while exported functions retain access to
those private helpers.

### Declaring Exports with `module`

`module` takes a symbolic name, an `export` clause, and body expressions. The
name describes the module; imports bind the exported names directly and do not
add a namespace prefix.

```sema
;; math-utils.sema
(module math-utils
  (export square cube)
  (define (private-mul x y) (* x y))
  (define (square x) (private-mul x x))
  (define (cube x) (private-mul x (private-mul x x))))
```

The export list may be empty. `(export)` makes every definition in the module
private. Use one module declaration per file; a later declaration replaces the
file's active export list rather than creating a second namespace.

A file without a `module` declaration exports all of its top-level bindings.
This is convenient for small scripts, but an explicit export list gives a
stable public interface.

### Importing a File or Package

The first argument to `import` is evaluated and must produce a string. Relative
paths are resolved from the importing source file. Absolute paths and package
identifiers are also supported.

```sema
;; main.sema
(import "math-utils.sema")
(square 5) ; => 25
(cube 3)   ; => 27
```

List bare names after the path to import only those exports:

```sema
(import "math-utils.sema" square)
```

A selective import fails if the file does not export a requested name. Without
a name list, all exports are added to the current environment.

Sema caches an imported module by its resolved path. Importing it again in the
same interpreter does not evaluate the file again. Cyclic imports return an
error. File-system imports also obey the active sandbox's read permissions.

Use [`load`](./special-forms.md#load) when the goal is different: `load`
evaluates a file directly in the current environment and does not apply module
export boundaries.
