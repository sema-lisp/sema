---
outline: [2, 3]
---

# Math & Arithmetic

## Domain & error policy

Sema's numeric error behavior follows one rule, split by type:

- **Integer division or modulo by zero raises an error.** `(/ 1 0)`, `(modulo 7 0)`, and `(mod 7 0)` all raise (`division by zero` / `modulo by zero`). Integers have no infinity or NaN to return, so the failure surfaces where it happens.
- **Floating-point follows IEEE 754** — overflow and undefined real-domain results return `inf`, `-inf`, or `NaN` instead of raising:

```sema
(/ 1.0 0)     ; => inf
(/ -1.0 0)    ; => -inf
(/ 0.0 0.0)   ; => NaN
(log 0)       ; => -inf
(log -1)      ; => NaN
(pow 0 0)     ; => 1
(pow 2 -1)    ; => 1/2
```

This matches the hardware and mainstream numeric languages, so `NaN` propagates and `inf` accumulates rather than forcing error handling around every operation. If you need to reject these, test with `math/nan?` / `math/infinite?` explicitly.

> **No integer overflow** — Sema has a full numeric tower, so exact integer arithmetic never wraps. A result beyond `i64` range promotes to an arbitrary-precision bignum: `(+ 9223372036854775807 1)` → `9223372036854775808`, `(* 1000000000000 1000000000000)` → `1000000000000000000000000`. Exact division yields rationals (`(/ 1 3)` → `1/3`) and `(sqrt -1)` → `0+1i`. (See ADR #64 and the numeric-tower ADR.)

## The Numeric Tower

Sema implements the full R7RS **numeric tower**, a nested hierarchy of number types:

```
integer  ⊂  rational  ⊂  real  ⊂  complex
```

Every integer is a rational, every rational is real, every real is complex — so [`complex?`](/docs/stdlib/predicates#complex) is true for *every* number, and [`integer?`](/docs/stdlib/predicates#integer) is the narrowest test. Two independent axes describe a value: its **level** in the tower (integer / rational / real / complex) and its **exactness** (exact vs. inexact).

- **Exact numbers** — integers (any size — they promote to bignums instead of overflowing), exact rationals like `1/3`, and complex numbers whose parts are both exact — carry no rounding error.
- **Inexact numbers** — floats and any complex with a floating-point part — follow IEEE 754.
- **Exactness contagion:** an operation is exact only when *all* its operands are exact; a single float makes the whole result inexact. This is why `(/ 1 3)` is the exact `1/3` but `(/ 1.0 3)` is `0.3333…`.

### Literal grammar

The reader parses each tower type directly, and every literal round-trips through print → read:

| Form | Example | Reads as |
| --- | --- | --- |
| Rational | `1/3`, `3/6` (→ `1/2`) | exact ratio, reduced to lowest terms |
| Complex (rectangular) | `3+4i`, `1.5+2.5i`, `3-4i` | real + imaginary part |
| Pure imaginary | `+i`, `-i`, `2i`, `-2i` | `0 ± ni` |
| Radix prefix | `#xFF` (255), `#o17` (15), `#b1010` (10), `#d42` | base 16 / 8 / 2 / 10 integer |
| Exactness prefix | `#e1.5` (→ `3/2`), `#i1/2` (→ `0.5`) | force exact / inexact |
| Combined | `#e#xFF`, `#x#eFF` | radix + exactness, either order |
| Bignum | `99999999999999999999999999` | out-of-range integer literals become bignums |

See the [reader internals](/docs/internals/reader#numeric-literals) for the full grammar.

## Basic Arithmetic

### `+`

Add numbers together. Accepts any number of arguments.

```sema
(+ 1 2 3)     ; => 6
(+ 10)        ; => 10
(+)           ; => 0
```

### `-`

Subtract numbers. With one argument, negates. With multiple, subtracts left to right.

```sema
(- 10 3)      ; => 7
(- 10 3 2)    ; => 5
(- 5)         ; => -5
```

### `*`

Multiply numbers together.

```sema
(* 4 5)       ; => 20
(* 2 3 4)     ; => 24
(*)           ; => 1
```

### `/`

Divide numbers. Exact-by-exact division is *exact*: it yields a reduced rational when the result is not a whole number (so `(/ 10 3)` is `10/3`, not `3.333…`), collapsing back to an integer when the denominator reduces to 1. A float operand makes the result a float (inexact contagion). For truncated integer division use [`quotient`](#quotient) (or its `math/quotient` alias).

```sema
(/ 10 2)      ;; => 5
(/ 10 3)      ;; => 10/3
(/ 10 4)      ;; => 5/2
(/ 10.0 3)    ;; => 3.3333333333333335
```

### `mod`

Modulo. On exact integers this is *floored* division — the result takes the sign of the **divisor**, per R7RS — so `(mod -7 2)` is `1`, not `-1`. (Float operands keep the IEEE truncated `%`, whose sign follows the dividend.) For the truncated integer counterpart whose sign follows the dividend, use [`remainder`](#remainder).

```sema
(mod 10 3)    ; => 1
(mod 7 2)     ; => 1
(mod -7 2)    ; => 1
```

## Comparison

### `<`

Less than. Supports chaining.

```sema
(< 1 2)       ; => #t
(< 1 2 3)     ; => #t
(< 3 2)       ; => #f
```

### `>`

Greater than.

```sema
(> 3 2)       ; => #t
(> 1 2)       ; => #f
```

### `<=`

Less than or equal.

```sema
(<= 1 2)      ; => #t
(<= 2 2)      ; => #t
```

### `>=`

Greater than or equal.

```sema
(>= 3 2)      ; => #t
(>= 2 2)      ; => #t
```

### `=`

Equality. For numbers this is numeric equality (so `(= 1 1.0)` is `#t`); for non-numbers it falls back to structural equality. Unlike `<` / `>`, comparing non-numbers does not error.

```sema
(= 1 1)           ; => #t
(= 1 1.0)         ; => #t
(= 1 2)           ; => #f
(= "abc" "abc")   ; => #t   (structural, not an error)
```

## Numeric Utilities

### `abs`

Absolute value.

```sema
(abs -5)      ; => 5
(abs 3)       ; => 3
(abs -3.14)   ; => 3.14
```

### `min`

Return the smallest of 1 or more numbers (the no-arg case errors).

```sema
(min 1 2 3)   ;; => 1
(min 5)       ;; => 5
(min)         ;; error: Arity error: min expects 1+ args, got 0
```

### `max`

Return the largest of 1 or more numbers (the no-arg case errors).

```sema
(max 1 2 3)   ;; => 3
(max 5)       ;; => 5
(max)         ;; error: Arity error: max expects 1+ args, got 0
```

### `pow`

Raise a number to a power.

```sema
(pow 2 10)    ; => 1024
(pow 3 3)     ; => 27
```

### `sqrt`

Square root. The square root of an exact perfect square is returned *exactly* (`(sqrt 16)` is `4`, not `4.0`); otherwise the result is an inexact float. The square root of a negative number is complex.

```sema
(sqrt 16)     ; => 4
(sqrt 2)      ; => 1.4142135623730951
(sqrt -1)     ; => 0+1i
```

### `log`

Natural logarithm.

```sema
(log 1)       ; => 0.0
(log 100)     ; => 4.605...
```

### `floor`

Round down toward negative infinity. **Exactness-preserving**: a float argument rounds to a float (`3.7` → `3.0`), while an exact rational rounds to an exact integer (`7/2` → `3`).

```sema
(floor 3.7)   ; => 3.0
(floor -2.3)  ; => -3.0
(floor 7/2)   ; => 3
```

### `ceil`

Round up toward positive infinity. Exactness-preserving, like `floor`.

```sema
(ceil 3.2)    ; => 4.0
(ceil -2.7)   ; => -2.0
(ceil 7/2)    ; => 4
```

### `round`

Round to the nearest integer, ties to even (banker's rounding). Exactness-preserving: a float rounds to a float, an exact rational to an exact integer.

```sema
(round 3.5)   ; => 4.0
(round 3.4)   ; => 3.0
(round 7/2)   ; => 4
```

### `math/round-to`

Round to `places` decimal places, returning a float (where `round` only rounds to a whole integer).

```sema
(math/round-to 3.14159 2)   ; => 3.14
(math/round-to 0.46666 3)   ; => 0.467
```

### `math/format-fixed`

Format a number as a fixed-decimal **string**, padding trailing zeros to `places` digits — for money/metrics display where `math/round-to` (a float, which drops trailing zeros) isn't enough.

```sema
(math/format-fixed 1.2 3)     ; => "1.200"
(math/format-fixed 3.14159 2) ; => "3.14"
```

## Trigonometry

### `sin`

Sine (argument in radians).

```sema
(sin 0)       ; => 0.0
(sin pi)      ; => ~0.0
```

### `cos`

Cosine (argument in radians).

```sema
(cos 0)       ; => 1.0
(cos pi)      ; => -1.0
```

### `math/tan`

Tangent (argument in radians).

```sema
(math/tan 0)       ; => 0.0
(math/tan (/ pi 4)); => ~1.0
```

### `math/asin`

Inverse sine. Returns radians.

```sema
(math/asin 1)      ; => ~1.5707 (π/2)
(math/asin 0)      ; => 0.0
```

### `math/acos`

Inverse cosine. Returns radians.

```sema
(math/acos 0)      ; => ~1.5707 (π/2)
(math/acos 1)      ; => 0.0
```

### `math/atan`

Inverse tangent. Returns radians.

```sema
(math/atan 1)      ; => ~0.7854 (π/4)
(math/atan 0)      ; => 0.0
```

### `math/atan2`

Two-argument inverse tangent. Returns the angle in radians between the positive x-axis and the point (x, y).

```sema
(math/atan2 1 1)   ; => ~0.7854 (π/4)
(math/atan2 0 -1)  ; => ~3.1416 (π)
```

## Hyperbolic Functions

### `math/sinh`

Hyperbolic sine.

```sema
(math/sinh 0)      ; => 0.0
(math/sinh 1)      ; => 1.1752...
```

### `math/cosh`

Hyperbolic cosine.

```sema
(math/cosh 0)      ; => 1.0
(math/cosh 1)      ; => 1.5430...
```

### `math/tanh`

Hyperbolic tangent.

```sema
(math/tanh 0)      ; => 0.0
(math/tanh 1)      ; => 0.7615...
```

## Exponential & Logarithmic

### `math/exp`

Euler's number raised to a power (e^x).

```sema
(math/exp 1)       ; => 2.71828...
(math/exp 0)       ; => 1.0
```

### `math/log10`

Base-10 logarithm.

```sema
(math/log10 100)   ; => 2.0
(math/log10 1000)  ; => 3.0
```

### `math/log2`

Base-2 logarithm.

```sema
(math/log2 8)      ; => 3.0
(math/log2 1024)   ; => 10.0
```

## Integer Division

These operate on exact integers, are **bignum-aware** (they promote past `i64` automatically), and raise on a zero divisor. Each has a slash-namespaced alias (`math/quotient`, `math/remainder`, `math/gcd`, `math/lcm`) that is the identical function.

### `quotient`

Truncated integer division: `n ÷ d` rounded **toward zero**, so `(quotient -7 2)` is `-3` (not floored to `-4`), per R7RS. Alias: `math/quotient`.

```sema
(quotient 10 3)   ; => 3
(quotient -7 2)   ; => -3
(quotient 100000000000000000000 7)  ; => 14285714285714285714
```

### `remainder`

Remainder of truncated division: the result takes the sign of the **dividend** `n`. Pairs with `quotient` so that `(+ (* (quotient n d) d) (remainder n d))` reconstructs `n`. Contrast [`mod`](#mod), whose sign follows the divisor. Alias: `math/remainder`.

```sema
(remainder 10 3)  ; => 1
(remainder -7 2)  ; => -1
(remainder 7 -2)  ; => 1
```

### `gcd`

Greatest common divisor — the largest non-negative integer dividing every argument. Variadic and sign-independent; `(gcd)` is `0`. Alias: `math/gcd`.

```sema
(gcd 12 8)     ; => 4
(gcd 15 10 25) ; => 5
(gcd -12 8)    ; => 4
(gcd)          ; => 0
```

### `lcm`

Least common multiple — the smallest non-negative integer every argument divides. Variadic and sign-independent; `0` if any argument is `0`; `(lcm)` is `1`. Alias: `math/lcm`.

```sema
(lcm 4 6)      ; => 12
(lcm 2 3 4)    ; => 12
(lcm 0 5)      ; => 0
(lcm)          ; => 1
```

## Exactness & Rationals

Utilities for moving between exact and inexact forms and for working with exact rationals. See [The Numeric Tower](#the-numeric-tower) for the underlying model.

### `exact`

Convert a number to its exact form. A finite float becomes the *exact* rational it actually represents (reduced, and normalized to an integer when the denominator is 1); already-exact numbers pass through. `inexact->exact` is the longer R7RS spelling.

```sema
(exact 0.5)       ; => 1/2
(exact 2.0)       ; => 2
(exact 1/3)       ; => 1/3
(exact 3.14159)   ; => 3537115888337719/1125899906842624
```

The last result is exact but surprising: `3.14159` is not representable in binary, so `exact` returns the precise fraction the double stores. Use [`rationalize`](#rationalize) for a tidy approximation.

### `inexact`

Convert a number to inexact (floating-point) form — an exact rational becomes its nearest `f64`, and each part of a complex becomes a float. `exact->inexact` is the longer R7RS spelling.

```sema
(inexact 1/3)   ; => 0.3333333333333333
(inexact 42)    ; => 42.0
(inexact 3+4i)  ; => 3.0+4.0i
```

### `exact->inexact`

R7RS spelling of [`inexact`](#inexact) — identical behavior.

```sema
(exact->inexact 1/3)   ; => 0.3333333333333333
(exact->inexact 42)    ; => 42.0
```

### `inexact->exact`

R7RS spelling of [`exact`](#exact) — identical behavior.

```sema
(inexact->exact 0.5)   ; => 1/2
(inexact->exact 2.0)   ; => 2
(inexact->exact 0.1)   ; => 3602879701896397/36028797018963968
```

`0.1` shows the caveat: it has no finite binary expansion, so its exact value is that large power-of-two fraction, not `1/10`.

### `numerator`

Numerator of an exact rational, taken in lowest terms (the sign lives on the numerator). An integer `n` is `n/1`. A float or complex argument raises a type error — convert with `exact` first.

```sema
(numerator 22/7)   ; => 22
(numerator -6/4)   ; => -3
(numerator 42)     ; => 42
```

### `denominator`

Denominator of an exact rational, in lowest terms. An integer's denominator is `1`.

```sema
(denominator 22/7)   ; => 7
(denominator -6/4)   ; => 2
(denominator 42)     ; => 1
```

### `rationalize`

Find the *simplest* rational within `tol` of `x` (smallest denominator in `[x-|tol|, x+|tol|]`), per R7RS. Exactness follows contagion: the result is exact only when **both** arguments are exact.

```sema
(rationalize 1/3 1/1000)            ; => 1/3
(rationalize (exact 3.14159) 1/100) ; => 22/7
(rationalize 3.14159 1/100)         ; => 3.142857142857143
(rationalize 1/2 0.01)              ; => 0.5
```

The last two show contagion — an inexact `x` *or* an inexact `tol` gives an inexact (float) answer.

### `exact-integer-sqrt`

Exact integer square root of a non-negative integer. Returns a two-element list `(s r)` with `s = ⌊√n⌋` and `s*s + r = n`; exact even for bignums.

```sema
(exact-integer-sqrt 17)   ; => (4 1)
(exact-integer-sqrt 100)  ; => (10 0)
(exact-integer-sqrt 15241578750190521) ; => (123456789 0)
```

## Complex Numbers

A complex number `a+bi` has a real and an imaginary part. A complex whose imaginary part is *exact* zero collapses to a real, so `real?` is true for `3+0i`. Polar conversions run through `sin`/`cos`/`atan2` in floating point, so [`make-polar`](#make-polar), [`magnitude`](#magnitude), and [`angle`](#angle) are always inexact.

### `make-rectangular`

Construct a complex from a real and an imaginary part. An *exact*-zero imaginary part collapses to the real; an *inexact* zero (`0.0`) stays complex.

```sema
(make-rectangular 3 4)     ; => 3+4i
(make-rectangular 1/3 1/2) ; => 1/3+1/2i
(make-rectangular 2 0)     ; => 2
(make-rectangular 3 0.0)   ; => 3+0.0i
```

### `make-polar`

Construct a complex from magnitude `r` and angle `θ` (radians): `r·cos θ + r·sin θ·i`. Always inexact.

```sema
(make-polar 2 0)                ; => 2.0+0.0i
(make-polar 5 (math/atan2 3 4)) ; => 4.0+3.0i
```

### `real-part`

Real part of a number (the number itself for a real). Preserves exactness.

```sema
(real-part 3+4i)   ; => 3
(real-part 5i)     ; => 0
(real-part 2.5)    ; => 2.5
```

### `imag-part`

Imaginary part of a number (exact `0` for any real). Preserves exactness.

```sema
(imag-part 3+4i)   ; => 4
(imag-part 5i)     ; => 5
(imag-part 2.5)    ; => 0
```

### `magnitude`

Magnitude (modulus, absolute value). For a complex `a+bi` this is `√(a²+b²)`, computed in floating point (so it is inexact); for a real it is the absolute value and preserves exactness.

```sema
(magnitude 3+4i)   ; => 5.0
(magnitude -5)     ; => 5
(magnitude 1/3)    ; => 1/3
```

### `angle`

Angle (argument) of a complex in radians, in (-π, π]: `atan2(b, a)`. Always inexact — a positive real gives `0.0`, a negative real gives π.

```sema
(angle 3+4i)   ; => 0.9272952180016122
(angle 5)      ; => 0.0
(angle -5)     ; => 3.141592653589793
```

## Number ↔ String

### `number->string`

Render any number in the tower as a string. An optional radix of 2, 8, 10, or 16 selects the output base — but a non-decimal radix accepts only exact integers.

```sema
(number->string 42)     ; => "42"
(number->string 1/3)    ; => "1/3"
(number->string 3+4i)   ; => "3+4i"
(number->string 255 16) ; => "ff"
(number->string 5 2)    ; => "101"
```

### `string->number`

Parse a string as a number, returning `#f` (never an error) on invalid input. The default radix 10 accepts the whole tower (integers, rationals, floats, complex); a radix of 2, 8, or 16 parses an integer in that base.

```sema
(string->number "42")    ; => 42
(string->number "1/3")   ; => 1/3
(string->number "3+4i")  ; => 3+4i
(string->number "ff" 16) ; => 255
(string->number "nope")  ; => #f
```

## Random Numbers

### `math/random`

Return a random float between 0.0 (inclusive) and 1.0 (exclusive).

```sema
(math/random)      ; => 0.7291... (varies)
```

### `math/random-int`

Return a random integer in a range (inclusive on both ends).

```sema
(math/random-int 1 100)  ; => 42 (varies)
(math/random-int 0 9)    ; => 7 (varies)
```

## Interpolation & Clamping

### `math/clamp`

Clamp a value to a range.

```sema
(math/clamp 15 0 10)   ; => 10
(math/clamp -5 0 10)   ; => 0
(math/clamp 5 0 10)    ; => 5
```

### `math/sign`

Return the sign of a number: -1, 0, or 1.

```sema
(math/sign -5)     ; => -1
(math/sign 0)      ; => 0
(math/sign 42)     ; => 1
```

### `math/lerp`

Linear interpolation between two values. `(math/lerp a b t)` returns `a + (b - a) * t`.

```sema
(math/lerp 0 100 0.5)   ; => 50.0
(math/lerp 0 100 0.25)  ; => 25.0
(math/lerp 10 20 0.0)   ; => 10.0
```

### `math/map-range`

Map a value from one range to another. `(math/map-range value in-min in-max out-min out-max)`.

```sema
(math/map-range 5 0 10 0 100)    ; => 50.0
(math/map-range 0.5 0 1 0 255)   ; => 127.5
```

## Angle Conversion

### `math/degrees->radians`

Convert degrees to radians.

```sema
(math/degrees->radians 180)   ; => 3.14159...
(math/degrees->radians 90)    ; => 1.5707...
```

### `math/radians->degrees`

Convert radians to degrees.

```sema
(math/radians->degrees pi)    ; => 180.0
(math/radians->degrees 1)     ; => 57.295...
```

## Numeric Predicates

### `even?`

Test if an integer is even.

```sema
(even? 4)      ; => #t
(even? 3)      ; => #f
```

### `odd?`

Test if an integer is odd.

```sema
(odd? 3)       ; => #t
(odd? 4)       ; => #f
```

### `positive?`

Test if a number is positive.

```sema
(positive? 1)  ; => #t
(positive? -1) ; => #f
(positive? 0)  ; => #f
```

### `negative?`

Test if a number is negative.

```sema
(negative? -1) ; => #t
(negative? 1)  ; => #f
```

### `zero?`

Test if a number is zero.

```sema
(zero? 0)      ; => #t
(zero? 1)      ; => #f
```

### `math/nan?`

Test if a value is NaN (not a number).

```sema
(math/nan? math/nan)       ; => #t
(math/nan? 42)             ; => #f
```

### `math/infinite?`

Test if a value is infinite.

```sema
(math/infinite? math/infinity)  ; => #t
(math/infinite? 42)             ; => #f
```

## Constants

### `pi`

The mathematical constant π (3.14159...).

```sema
pi             ; => 3.141592653589793
```

### `e`

Euler's number (2.71828...).

```sema
e              ; => 2.718281828459045
```

### `math/infinity`

Positive infinity.

```sema
math/infinity  ; => Inf
```

### `math/nan`

Not a number.

```sema
math/nan       ; => NaN
```

## Scheme Aliases

### `modulo`

Alias for `mod`.

```sema
(modulo 10 3)  ; => 1
```

### `expt`

Alias for `pow` (Scheme name for exponentiation). With exact integer arguments the result is exact and bignum-aware — no overflow — and a negative exponent yields an exact rational.

```sema
(expt 2 10)    ; => 1024
(expt 2 100)   ; => 1267650600228229401496703205376
(expt 2 -1)    ; => 1/2
```

### `ceiling`

Alias for `ceil` (exactness-preserving, so a float rounds to a float).

```sema
(ceiling 3.2)  ; => 4.0
```

### `truncate`

Round toward zero (drop the fractional part). Exactness-preserving: a float truncates to a float, an exact rational to an exact integer.

```sema
(truncate 3.7)  ; => 3.0
(truncate -3.7) ; => -3.0
(truncate 7/2)  ; => 3
```

## Bitwise Operations

### `bit/and`

Bitwise AND.

```sema
(bit/and 5 3)      ; => 1
(bit/and 15 9)     ; => 9
```

### `bit/or`

Bitwise OR.

```sema
(bit/or 5 3)       ; => 7
(bit/or 8 4)       ; => 12
```

### `bit/xor`

Bitwise XOR.

```sema
(bit/xor 5 3)      ; => 6
```

### `bit/not`

Bitwise NOT (complement).

```sema
(bit/not 5)        ; => -6
```

### `bit/shift-left`

Left bit shift.

```sema
(bit/shift-left 1 4)   ; => 16
(bit/shift-left 3 2)   ; => 12
```

### `bit/shift-right`

Right bit shift.

```sema
(bit/shift-right 16 2) ; => 4
(bit/shift-right 8 1)  ; => 4
```
