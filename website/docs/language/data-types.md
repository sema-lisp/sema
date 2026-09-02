---
outline: [2, 3]
---

# Data Types

Sema has a rich set of built-in data types covering numbers, text, collections, and LLM primitives.

## Type Table

| Type         | Syntax               | Examples                                                           |
| ------------ | -------------------- | ------------------------------------------------------------------ |
| Integer      | digits               | `42`, `-7`, `0`                                                    |
| Float        | `.` or exponent      | `3.14`, `-0.5`, `0.001`, `6.022e23`, `1e-9`                        |
| String       | double-quoted        | `"hello"`, `"line\nbreak"`, `"\x1B;"`                              |
| F-String     | `f"...${expr}..."` | `f"Hello ${name}"`, `f"${(+ 1 2)}"`                               |
| Boolean      | `#t` / `#f`          | `#t`, `#f`                                                         |
| Nil          | `nil`                | `nil`                                                              |
| Symbol       | bare identifier      | `foo`, `my-var`, `+`                                               |
| Keyword      | colon-prefixed       | `:name`, `:type`, `:ok`                                            |
| Character    | `#\` prefix          | `#\a`, `#\space`, `#\newline`                                      |
| List         | parenthesized        | `(1 2 3)`, `(+ a b)`                                               |
| Vector       | bracketed            | `[1 2 3]`, `["a" "b"]`                                             |
| Map          | curly-braced         | `{:name "Ada" :age 36}`                                            |
| HashMap      | `(hashmap/new ...)`  | `(hashmap/new :a 1 :b 2)`                                          |
| Prompt       | `(prompt ...)`       | LLM prompt (see [Prompts](../llm/prompts.md))                      |
| Message      | `(message ...)`      | LLM message (see [Prompts](../llm/prompts.md))                     |
| Conversation | `(conversation/new)` | LLM conversation (see [Conversations](../llm/conversations.md))    |
| Tool         | `(deftool ...)`      | LLM tool definition (see [Tools & Agents](../llm/tools-agents.md)) |
| Agent        | `(defagent ...)`     | LLM agent (see [Tools & Agents](../llm/tools-agents.md))           |
| Promise      | `(delay expr)`       | Lazy evaluation                                                    |
| Record       | `define-record-type` | `(define-record-type point ...)`                                   |
| Bytevector   | `#u8(...)` literal   | `#u8(1 2 3)`, `#u8()`                                              |
| Async Promise | `(async expr)` or `(async/resolved val)` | An async task result (pending, resolved, or rejected) |
| Channel      | `(channel/new)` or `(channel/new capacity)` | Bounded FIFO channel for inter-task communication |

## Scalars

### Integer

Whole numbers. Standard arithmetic applies.

```sema
42
-7
0
```

### Float

Floating-point numbers, written with a decimal point and/or a scientific
(exponent) suffix `e`/`E`:

```sema
3.14
-0.5
0.001

;; Scientific notation — <mantissa>e<exponent>, with an optional sign on the
;; exponent. The mantissa may be a bare integer (no decimal point required).
6.022e23     ;; Avogadro's number  → 6.022 × 10²³
1.0e19       ;; 10000000000000000000.0
1e-9         ;; one nano  → 0.000000001
-2.5E6       ;; uppercase E works too → -2500000.0
(* 2 3e2)    ;; usable anywhere a number is → 600.0
```

A literal whose magnitude exceeds `f64` range follows IEEE-754 (`1e400` → `inf`,
`1e-400` → `0.0`). Floats print in exponent form when the magnitude is at least
`1e21` or below `1e-7` (`1e300` prints as `1e300`, not 300 digits); every printed
form reads back to the same value.

#### Number literal rules

- An explicit sign is allowed: `+42`, `-7`, `+1.5`.
- A number must end at whitespace or a bracket. `1abc`, `1.5e`, `0x1F`, and
  `1_000` are reader errors (`invalid number literal`), not a number followed by
  a symbol. Identifiers such as `e` or `exp` are unaffected.
- Hex, octal, and binary use the Scheme prefixes `#x1F`, `#o17`, `#b101`; there
  is no `0x` prefix and no `_` digit separator.
- A leading or trailing dot is not a number: `.5` reads as a symbol and `1.` is
  an error. Write `0.5` and `1.0`.
- Rationals are `1/2`; complex numbers are `3+4i`.

### String

Double-quoted text with escape sequences.

```sema
"hello"
"line\nbreak"
"\x1B;"
```

### F-String (Interpolated String)

String interpolation with embedded expressions. `f"..."` reads as a `(str ...)` call (i.e. `f"Hello ${name}"` is the same as `(str "Hello " name)`).

```sema
(define name "Alice")
f"Hello ${name}"                ; => "Hello Alice"
f"2 + 2 = ${(+ 2 2)}"           ; => "2 + 2 = 4"
f"${(:name user)} is ${(:age user)} years old"
```

Use `\$` to include a literal dollar sign: `f"costs \$5"`.

### Boolean

`#t` for true, `#f` for false.

```sema
#t
#f
```

### Nil

The empty/null value.

```sema
nil
```

### Symbol

Bare identifiers used as variable names and in quoted data.

```sema
foo
my-var
+
```

### Keyword

Colon-prefixed identifiers. Keywords are self-evaluating and can be used as accessor functions on maps.

```sema
:name
:type
:ok

;; Keywords as functions
(:name {:name "Ada" :age 36})  ; => "Ada"
```

### Character

Character literals with `#\` prefix. Named characters are supported.

```sema
#\a
#\space
#\newline
#\tab
```

There is no hex form for character literals (`#\x41` is an error). Use
`(integer->char #x41)` for a character by code point, or write the character
itself (`#\λ`).

## Collections

### List

Parenthesized sequences. Lists are the fundamental data structure in Sema. Access the first element with `car` (or `first`) and the rest with `cdr` (or `rest`).

::: details Why `car`/`cdr`?
These names come from the [IBM 704](http://bitsavers.informatik.uni-stuttgart.de/pdf/ibm/704/24-6661-2_704_Manual_1955.pdf) (1955), the machine Lisp was born on. The 704 stored each cons cell in a single 36-bit word: `car` ("Contents of the Address Register") extracted one 15-bit pointer field, `cdr` ("Contents of the Decrement Register") extracted the other. They were single hardware instructions. Sema also provides `first`/`rest` as aliases.
:::

```sema
(1 2 3)
(+ a b)
'(hello world)
```

Lists are proper lists only; there is no pair type. Dotted syntax is meaningful
in parameter lists (`(lambda (a . rest) ...)`) but in a quoted list the `.` is
read as an ordinary symbol: `'(1 . 2)` is a three-element list, so
`(length '(1 . 2))` is `3` and `(cdr '(1 . 2))` is `(. 2)`.

### Vector

Bracketed sequences with O(1) indexed access.

```sema
[1 2 3]
["a" "b"]
```

### Map

Curly-braced key-value pairs with deterministic (sorted) ordering. Maps support [destructuring](./special-forms.md#map-destructuring) in `let`, `define`, `lambda`, and [`match`](./special-forms.md#match) patterns.

```sema
{:name "Ada" :age 36}
{:a 1 :b 2 :c 3}
```

### HashMap

Hash-based maps for O(1) lookup performance with many keys.

```sema
(hashmap/new :a 1 :b 2 :c 3)
```

### Bytevector

Byte arrays with `#u8(...)` literal syntax.

```sema
#u8(1 2 3)
#u8()
(bytevector 1 2 3)
(bytevector/new 4)
```

## Special Types

### Promise

Lazy evaluation via `delay`/`force`. The expression is not evaluated until forced, and the result is memoized.

```sema
(define p (delay (+ 1 2)))
(force p)       ; => 3
(promise? p)    ; => #t
```

### Record

User-defined record types with constructors, predicates, and field accessors.

```sema
(define-record-type point
  (make-point x y)
  point?
  (x point-x)
  (y point-y))

(define p (make-point 3 4))
(point-x p)    ; => 3
```

## String Escape Sequences

| Escape       | Description                          | Example               |
| ------------ | ------------------------------------ | --------------------- |
| `\n`         | Newline                              | `"line\nbreak"`       |
| `\t`         | Tab                                  | `"col1\tcol2"`        |
| `\r`         | Carriage return                      | `"text\r"`            |
| `\\`         | Backslash                            | `"path\\file"`        |
| `\"`         | Double quote                         | `"say \"hi\""`        |
| `\0`         | Null character                       | `"\0"`                |
| `\x<hex>;`   | Unicode scalar (R7RS, 1+ hex digits) | `"\x1B;"`, `"\x3BB;"` |
| `\uNNNN`     | Unicode code point (4 hex digits)    | `"\u03BB"` (λ)        |
| `\UNNNNNNNN` | Unicode code point (8 hex digits)    | `"\U0001F600"` (😀)   |
| `\$`         | Literal dollar sign (in f-strings)   | `f"costs \$5"`        |

## Type Predicates

```sema
(null? '())        (nil? nil)         (empty? "")
(list? '(1))       (vector? [1])      (map? {:a 1})
(pair? '(1 2))     ; #t (non-empty list, Scheme compat)
(number? 42)       (integer? 42)      (float? 3.14)
(string? "hi")     (symbol? 'x)       (keyword? :k)
(char? #\a)        (record? r)        (bytevector? #u8())
(promise? (delay 1))  (promise-forced? p)
(bool? #t)         (fn? car)
(zero? 0)          (even? 4)          (odd? 3)
(positive? 1)      (negative? -1)
(eq? 'a 'a)        (= 1 1)

;; Scheme aliases: boolean? = bool?, procedure? = fn?
;; eq? and equal? are the same function in Sema — both do structural
;; equality without numeric coercion. Use = for numeric comparison
;; (e.g. (= 1 1.0) is #t, but (eq? 1 1.0) is #f).

;; LLM type predicates
(prompt? p)        (message? m)       (conversation? c)
(tool? t)          (agent? a)
```

## Type Conversions

```sema
(str 42)                    ; => "42" (any value to string)
(string/to-number "42")       ; => 42
(number/to-string 42)         ; => "42"
(string/to-symbol "foo")      ; => foo
(symbol/to-string 'foo)       ; => "foo"
(string/to-keyword "name")    ; => :name
(keyword/to-string :name)     ; => "name"
(char/to-integer #\A)         ; => 65
(integer/to-char 65)          ; => #\A
(char/to-string #\a)          ; => "a"
(string/to-char "a")          ; => #\a
(string/to-list "abc")        ; => (#\a #\b #\c)
(list->string '(#\h #\i))   ; => "hi"
(vector->list [1 2 3])      ; => (1 2 3)
(list->vector '(1 2 3))     ; => [1 2 3]
(bytevector/to-list #u8(65))   ; => (65)
(list/to-bytevector '(1 2 3))  ; => #u8(1 2 3)
(utf8/to-string #u8(104 105))  ; => "hi"
(string/to-utf8 "hi")          ; => #u8(104 105)
(type 42)                    ; => :int
```
