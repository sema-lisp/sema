---
outline: [2, 3]
---

# Predicates & Type Checking

Predicates return `#t` or `#f` and conventionally end with `?`.

## Emptiness Predicates

These three predicates overlap but are not interchangeable. `null?` returns `#t` for both `'()` and `nil` — it tests for "absence of a value or empty list". `nil?` is true only for the `nil` value itself (not for `'()`). `empty?` is the broadest: it accepts `nil`, strings, lists, vectors, maps, and other collections, returning `#t` when the value has no elements. Reach for `empty?` when you have a collection of any shape; reach for `nil?` when you specifically need to distinguish `nil` from `'()`.

### `null?`

Test if a value is the empty list or `nil`.

```sema
(null? '())    ;; => #t
(null? nil)    ;; => #t
(null? '(1))   ;; => #f
```

### `nil?`

Test if a value is `nil` specifically (not the empty list).

```sema
(nil? nil)     ;; => #t
(nil? '())     ;; => #f
(nil? 0)       ;; => #f
```

### `empty?`

Test if a collection, string, or `nil` is empty. Accepts strings, lists, vectors, maps, and `nil`.

```sema
(empty? "")        ;; => #t
(empty? '())       ;; => #t
(empty? nil)       ;; => #t
(empty? "hello")   ;; => #f
(empty? [1 2 3])   ;; => #f
```

## Collection Predicates

### `list?`

Test if a value is a list.

```sema
(list? '(1))    ; => #t
(list? 42)      ; => #f
```

### `pair?`

Test if a value is a non-empty list (Scheme compatibility).

```sema
(pair? '(1 2))   ; => #t
(pair? '())      ; => #f
```

### `vector?`

Test if a value is a vector.

```sema
(vector? [1])   ; => #t
(vector? '(1))  ; => #f
```

### `map?`

Test if a value is a map.

```sema
(map? {:a 1})   ; => #t
(map? '())      ; => #f
```

## Numeric Predicates

Sema implements the full R7RS [numeric tower](/docs/stdlib/math#the-numeric-tower), so two families of predicates apply to numbers: **type/level** tests (`number?`, `integer?`, `rational?`, `real?`, `complex?`, `float?`) and **exactness** tests (`exact?`, `inexact?`, `exact-integer?`). The type predicates nest — every integer is rational, every rational is real, every real is complex — so `complex?` is the widest and true for *all* numbers.

### `number?`

Test if a value is a number — anything in the tower (integer, bignum, rational, float, or complex). Equivalent to [`complex?`](#complex).

```sema
(number? 42)     ; => #t
(number? 3.14)   ; => #t
(number? 1/3)    ; => #t
(number? "42")   ; => #f
```

### `integer?`

Test if a value is an integer, per R7RS: true for any exact integer (including bignums) **and** for an integer-valued float like `3.0`. A float with a fractional part is not an integer. To exclude integer-valued floats, use [`exact-integer?`](#exact-integer); to test representation, use [`float?`](#float).

```sema
(integer? 42)     ; => #t
(integer? 3.14)   ; => #f
(integer? 3.0)    ; => #t   ; integer-valued float
```

### `rational?`

Test if a number is rational — exact and expressible as a ratio of two integers. Every exact integer and exact rational qualifies; floats and non-real complex numbers do not. (This tracks *exactness*, so it is stricter than strict R7RS where a finite float is also rational.)

```sema
(rational? 42)     ; => #t
(rational? 1/3)    ; => #t
(rational? 3.14)   ; => #f
(rational? 3+4i)   ; => #f
```

### `real?`

Test if a number is real — has no non-zero imaginary part. Every integer, rational, and float is real; `real?` is false *only* for a complex with a genuine imaginary component. A complex whose imaginary part is exact zero collapses to a real, so `3+0i` is real.

```sema
(real? 42)     ; => #t
(real? 3.14)   ; => #t
(real? 3+4i)   ; => #f
(real? 3+0i)   ; => #t
```

### `complex?`

Test if a value is a number. In R7RS the number types nest, so `complex?` is true for *every* number in the tower and false only for non-numbers. It is the widest numeric predicate.

```sema
(complex? 42)     ; => #t
(complex? 3.14)   ; => #t
(complex? 3+4i)   ; => #t
(complex? "hi")   ; => #f
```

### `float?`

Test if a value is a floating-point number.

```sema
(float? 3.14)   ; => #t
(float? 42)     ; => #f
```

### `exact?`

Test if a number is exact — represented without floating point. Exact numbers are integers, exact rationals, and complex numbers whose parts are both exact. The complement of [`inexact?`](#inexact) on numbers.

```sema
(exact? 42)      ; => #t
(exact? 1/3)     ; => #t
(exact? 3.14)    ; => #f
(exact? 3+4i)    ; => #t
(exact? 3.0+4i)  ; => #f
```

### `inexact?`

Test if a number is inexact — carries a floating-point component. True for any float and for any complex with at least one inexact part. The complement of [`exact?`](#exact) on numbers.

```sema
(inexact? 42)      ; => #f
(inexact? 3.14)    ; => #t
(inexact? 1/3)     ; => #f
(inexact? 3.0+4i)  ; => #t
```

### `exact-integer?`

Test if a value is an exact integer — true exactly when both `exact?` and `integer?` hold. Stricter than a bare `integer?`: `2.0` is an integer value but inexact, so it fails.

```sema
(exact-integer? 42)    ; => #t
(exact-integer? 1/2)   ; => #f
(exact-integer? 2.0)   ; => #f
(exact-integer? 3+0i)  ; => #t
```

### `zero?`

Test if a number is zero.

```sema
(zero? 0)   ; => #t
(zero? 1)   ; => #f
```

### `even?`

Test if an integer is even.

```sema
(even? 4)   ; => #t
(even? 3)   ; => #f
```

### `odd?`

Test if an integer is odd.

```sema
(odd? 3)   ; => #t
(odd? 4)   ; => #f
```

### `positive?`

Test if a number is positive.

```sema
(positive? 1)    ; => #t
(positive? -1)   ; => #f
```

### `negative?`

Test if a number is negative.

```sema
(negative? -1)   ; => #t
(negative? 1)    ; => #f
```

## Type Predicates

### `string?`

Test if a value is a string.

```sema
(string? "hi")   ; => #t
(string? 42)     ; => #f
```

### `symbol?`

Test if a value is a symbol.

```sema
(symbol? 'x)     ; => #t
(symbol? "x")    ; => #f
```

### `keyword?`

Test if a value is a keyword.

```sema
(keyword? :k)    ; => #t
(keyword? "k")   ; => #f
```

### `char?`

Test if a value is a character.

```sema
(char? #\a)      ; => #t
(char? "a")      ; => #f
```

### `bool?`

Test if a value is a boolean. `boolean?` is an alias.

```sema
(bool? #t)   ; => #t
(bool? 0)    ; => #f
```

### `fn?`

Test if a value is a function. `procedure?` is an alias.

```sema
(fn? car)        ; => #t
(fn? 42)         ; => #f
```

### `record?`

Test if a value is a record instance.

```sema
(record? my-record)   ; => #t
(record? 42)          ; => #f
```

### `bytevector?`

Test if a value is a bytevector.

```sema
(bytevector? #u8())   ; => #t
(bytevector? '())     ; => #f
```

## Promise Predicates

### `promise?`

Test if a value is a promise (created with `delay`).

```sema
(promise? (delay 1))   ; => #t
(promise? 42)          ; => #f
```

### `promise-forced?`

Test if a promise has been forced (evaluated).

```sema
(define p (delay (+ 1 2)))
(promise-forced? p)   ; => #f
(force p)
(promise-forced? p)   ; => #t
```

## Equality

### `eq?`

Test structural equality. `equal?` is an alias.

```sema
(eq? 'a 'a)           ; => #t
(eq? '(1 2) '(1 2))   ; => #t
(eq? 1 2)             ; => #f
```

### `=`

Equality. For numbers this is numeric equality (so `(= 1 1.0)` is `#t`); for non-numbers it falls back to structural equality. Unlike `<` / `>`, comparing non-numbers does not error.

```sema
(= 1 1)           ; => #t
(= 1 1.0)         ; => #t
(= 1 2)           ; => #f
(= "abc" "abc")   ; => #t   (structural, not an error)
```

## LLM Type Predicates

### `prompt?`

Test if a value is an LLM prompt.

```sema
(prompt? (prompt (user "hi")))   ; => #t
```

### `message?`

Test if a value is an LLM message.

```sema
(message? (message :user "hi"))   ; => #t
```

### `conversation?`

Test if a value is a conversation.

```sema
(conversation? (conversation/new {}))   ; => #t
```

### `tool?`

Test if a value is a tool definition.

```sema
(deftool my-tool "A test tool" {:x {:type :string}} (lambda (x) x))
(tool? my-tool)   ; => #t
(tool? 42)        ; => #f
```

### `agent?`

Test if a value is an agent.

```sema
(defagent my-agent {:system "test"})
(agent? my-agent)   ; => #t
(agent? 42)         ; => #f
```
