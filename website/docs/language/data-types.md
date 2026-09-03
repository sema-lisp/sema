---
outline: [2, 3]
---

# Data Types

Sema values include exact and inexact numbers, text, immutable collections,
explicit mutable containers, executable values, asynchronous values, and LLM
objects. `(type value)` returns a keyword that identifies the runtime type.

## Literal Types

| Type | Literal syntax | Examples | `type` result |
| --- | --- | --- | --- |
| Nil | `nil` | `nil` | `:nil` |
| Boolean | `#t`, `#f` | `#t` | `:bool` |
| Integer | decimal or Scheme radix prefix | `42`, `#x2a`, `#b101010` | `:int` |
| Rational | integer `/` integer | `1/3`, `-7/2` | `:rational` |
| Float | decimal point or exponent | `3.14`, `1e-9` | `:float` |
| Complex | real and imaginary parts | `3+4i`, `-2i` | `:complex` |
| Character | `#\` prefix | `#\a`, `#\space` | `:char` |
| String | double quotes | `"hello"`, `"line\nbreak"` | `:string` |
| Symbol | identifier, quoted when used as data | `'foo`, `'my-var` | `:symbol` |
| Keyword | `:` prefix | `:name`, `:ok` | `:keyword` |
| List | quote or `list` | `'(1 2 3)`, `(list 1 2 3)` | `:list` |
| Vector | brackets | `[1 2 3]` | `:vector` |
| Map | braces | `{:name "Ada" :age 36}` | `:map` |
| Bytevector | `#u8(...)` | `#u8(1 2 3)` | `:bytevector` |

Parenthesized input is normally code, not a list literal. `(1 2 3)` attempts to
call `1`. Quote a list or construct it with `list` when the list is data. Bare
symbols also evaluate as variable references, so quote a symbol when its name
is the value you want.

## Numbers

Sema's numeric tower is integer ⊂ rational ⊂ real ⊂ complex. Integers have
arbitrary precision. Integers and rationals are exact; floats use IEEE 754
double precision and are inexact. Arithmetic keeps an exact result when all
inputs and the result can be represented exactly.

```sema
(+ 9223372036854775807 1) ; => 9223372036854775808
(/ 1 3)                   ; => 1/3
(+ 1/2 1/3)               ; => 5/6
(+ 1/2 0.5)               ; => 1.0
(sqrt -1)                 ; => 0+1i
```

See [Math](../stdlib/math.md#the-numeric-tower) for coercion, comparison, complex
arithmetic, and the complete numeric API.

### Integer

Integers may be decimal or use the Scheme radix prefixes `#b`, `#o`, and `#x`.
There is no `0x` prefix and no `_` digit separator.

```sema
42
-7
#b101010
#o52
#x2a
```

### Rational

A rational contains an integer numerator and a nonzero integer denominator.
Rationals are normalized and remain exact.

```sema
1/3
-7/2
(+ 1/6 1/3) ; => 1/2
```

Exact division produces a rational when the result is not an integer.

### Float

Floats use a decimal point, an exponent suffix, or both.

```sema
3.14
-0.5
6.022e23
1e-9
-2.5E6
```

A magnitude outside the finite `f64` range follows IEEE 754: `1e400` becomes
`inf`, and `1e-400` becomes `0.0`. Printed finite floats round-trip through the
reader.

### Complex

Complex literals use `i` for the imaginary part. Rectangular and pure-imaginary
forms are supported.

```sema
3+4i
3-4i
2i
-2i
```

An exact complex number with a zero imaginary part simplifies to its real
component.

### Number Literal Rules

- An explicit sign is allowed: `+42`, `-7`, `+1.5`.
- A number must end at whitespace or a bracket. Inputs such as `1abc`, `1.5e`,
  `0x1F`, and `1_000` are reader errors.
- A leading or trailing dot is not a decimal literal. Write `0.5` and `1.0`, not
  `.5` and `1.`.

## Text and Names

### String and F-String

Strings are Unicode text. An f-string evaluates each `${...}` expression and
concatenates its printed string value with the surrounding text.

```sema
(define name "Alice")
f"Hello ${name}"       ; => "Hello Alice"
f"2 + 2 = ${(+ 2 2)}" ; => "2 + 2 = 4"
```

Use `\$` for a literal dollar sign in an f-string: `f"costs \$5"`.

### Symbol

Symbols name variables, functions, and syntactic forms. A bare symbol is looked
up during evaluation. Quote it to use the symbol itself as data.

```sema
(define answer 42)
answer         ; => 42
'answer        ; => answer
(type 'answer) ; => :symbol
```

### Keyword

Keywords are self-evaluating values commonly used as map keys and tags. A
keyword can also look itself up in a map.

```sema
:name
(:name {:name "Ada" :age 36}) ; => "Ada"
(:missing {:name "Ada"})      ; => nil
```

### Character

Character literals use the `#\` prefix. Named characters include `#\space`,
`#\newline`, and `#\tab`.

```sema
#\a
#\space
#\λ
(integer->char #x41) ; => #\A
```

There is no hexadecimal character-literal form. `#\x41` is an error; convert a
code point with `integer->char` instead.

## Nil, Empty Collections, and Truth

`nil` is a distinct value. It is not the empty list. The empty list is `'()`.
Both satisfy `empty?`, but their type and equality differ.

```sema
(type nil)       ; => :nil
(type '())       ; => :list
(equal? nil '()) ; => #f
(nil? nil)       ; => #t
(null? '())      ; => #t
```

Only `#f` and `nil` are false in a condition. Empty strings and empty
collections are true.

```sema
(if '() :true :false) ; => :true
(if [] :true :false)  ; => :true
```

## Collections

Lists, vectors, maps, and hash maps are immutable values. Operations such as
`cons`, `append`, and `assoc` return updated values instead of changing the
input. Use the types in [Mutable State](../stdlib/mutable.md) when shared mutable
storage is required.

### List

Lists are sequential values with `car`/`first` and `cdr`/`rest` access.

```sema
'(1 2 3)
(list 1 2 3)
(first '(1 2 3)) ; => 1
(rest '(1 2 3))  ; => (2 3)
```

Lists are proper lists only; Sema has no pair type. A dot has special meaning in
parameter lists, such as `(lambda (first . rest) ...)`. In quoted list data it
is an ordinary symbol: `'(1 . 2)` has three elements.

### Vector

Vectors provide constant-time indexed access.

```sema
[1 2 3]
(vector/ref [10 20 30] 1) ; => 20
```

### Map and HashMap

A map literal creates a sorted map with deterministic iteration order. A hash
map uses hashing for average constant-time lookup; its iteration order is not
part of the API. Both support keyword lookup and the usual map operations.

```sema
{:name "Ada" :age 36}
(get {:a 1 :b 2} :b) ; => 2
(hashmap/new :a 1 :b 2)
```

Maps support [destructuring](./special-forms.md#map-destructuring) in bindings
and [`match`](./special-forms.md#match) patterns.

### Bytevector

A bytevector stores integers from 0 through 255. Literal and constructor values
have the same type.

```sema
#u8(1 2 3)
#u8()
(bytevector 1 2 3)
(bytevector/new 4)
```

## Constructed and Runtime Types

| Type | Common constructor | `type` result | Documentation |
| --- | --- | --- | --- |
| Function | `(fn (x) x)` | `:lambda` | [Functions](./special-forms.md#functions) |
| Procedural or pattern macro | `defmacro`, `define-syntax` | `:macro` | [Macros](./macros-modules.md) |
| Record | `define-record-type` | record's type tag | [Records](../stdlib/records.md) |
| Lazy promise | `(delay expr)` | `:promise` | [`delay`](./special-forms.md#delay) |
| Async promise | `(async/resolved value)` | `:async-promise` | [Concurrency](../stdlib/concurrency.md) |
| Channel | `(channel/new)` | `:channel` | [Concurrency](../stdlib/concurrency.md) |
| Stream | stream constructors | `:stream` | [Streams](../stdlib/streams.md) |
| Typed array | `f64-array`, `i64-array` | `:f64-array`, `:i64-array` | [Typed Arrays](../stdlib/typed-arrays.md) |
| Mutable container | `mutable-array/new`, `mutable-cell/new` | `:mutable-array`, `:mutable-cell` | [Mutable State](../stdlib/mutable.md) |
| Prompt, message, conversation | LLM constructors | `:prompt`, `:message`, `:conversation` | [Prompts](../llm/prompts.md) |
| Tool, agent | `deftool`, `defagent` | `:tool`, `:agent` | [Tools & Agents](../llm/tools-agents.md) |

Native functions such as `+` report `:native-fn`; functions created with `fn`
report `:lambda`. A record is the exception to the general `type` rule: it
returns the record's declared tag, such as `:point`, rather than `:record`.

### Lazy and Async Promises

The two promise types serve different purposes. `delay` creates a synchronous
lazy computation. `force` evaluates it at most once and memoizes the result.
`async` and `async/resolved` create asynchronous task results consumed with
`await`.

```sema
(define p (delay (+ 1 2)))
(type p)     ; => :promise
(promise? p) ; => #t
(force p)    ; => 3
```

`promise?` tests lazy promises. Use `async/promise?` for asynchronous promises.

### Record

Records define a type-specific constructor, predicate, and accessors.

```sema
(define-record-type point
  (make-point x y)
  point?
  (x point-x)
  (y point-y))

(define p (make-point 3 4))
(type p)    ; => :point
(point-x p) ; => 3
```

## String Escape Sequences

| Escape | Description | Example |
| --- | --- | --- |
| `\n` | Newline | `"line\nbreak"` |
| `\t` | Tab | `"col1\tcol2"` |
| `\r` | Carriage return | `"text\r"` |
| `\\` | Backslash | `"path\\file"` |
| `\"` | Double quote | `"say \"hi\""` |
| `\0` | Null character | `"\0"` |
| `\x<hex>;` | Unicode scalar, one or more hex digits | `"\x3BB;"` |
| `\uNNNN` | Unicode code point, four hex digits | `"\u03BB"` |
| `\UNNNNNNNN` | Unicode code point, eight hex digits | `"\U0001F600"` |
| `\$` | Literal dollar sign in an f-string | `f"costs \$5"` |

## Type Tests and Equality

Common predicates include:

```sema
(nil? nil)            ; => #t
(null? '())           ; => #t
(list? '(1))          ; => #t
(vector? [1])         ; => #t
(map? {:a 1})         ; => #t
(number? 1/2)         ; => #t
(integer? 42)         ; => #t
(float? 3.14)         ; => #t
(string? "hi")       ; => #t
(symbol? 'x)          ; => #t
(keyword? :x)         ; => #t
(char? #\a)           ; => #t
(bytevector? #u8())   ; => #t
(fn? (fn (x) x))      ; => #t
```

`eq?` and `equal?` are aliases for exact structural equality. They do not
coerce numeric types. `=` compares numbers with numeric coercion and also
accepts structurally equal nonnumeric values.

```sema
(equal? 1 1.0)   ; => #f
(= 1 1.0)        ; => #t
(equal? [1] [1]) ; => #t
```

`boolean?` is an alias for `bool?`, and `procedure?` is an alias for `fn?`.

## Type Conversions

```sema
(str 42)                      ; => "42"
(string/to-number "42")      ; => 42
(number/to-string 42)         ; => "42"
(string/to-symbol "foo")     ; => foo
(symbol/to-string 'foo)       ; => "foo"
(string/to-keyword "name")   ; => :name
(keyword/to-string :name)     ; => "name"
(char/to-integer #\A)         ; => 65
(integer/to-char 65)          ; => #\A
(string/to-list "abc")       ; => (#\a #\b #\c)
(list->string '(#\h #\i))    ; => "hi"
(vector->list [1 2 3])        ; => (1 2 3)
(list->vector '(1 2 3))       ; => [1 2 3]
(bytevector/to-list #u8(65))  ; => (65)
(list/to-bytevector '(1 2 3)) ; => #u8(1 2 3)
(utf8/to-string #u8(104 105)) ; => "hi"
(string/to-utf8 "hi")        ; => #u8(104 105)
```
