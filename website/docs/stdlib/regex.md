---
outline: [2, 3]
---

# Regex

Regular expression functions for pattern matching, searching, replacement, and splitting. Sema uses the Rust [`regex`](https://docs.rs/regex) engine.

::: warning Rust regex limitations
Rust regex intentionally does **not** support features that require backtracking:

- No lookahead / lookbehind (`(?=...)`, `(?!...)`, `(?<=...)`, `(?<!...)`)
- No backreferences (`\1`, `\k<name>`)

If you need those, consider a multi-step approach using string functions.
:::

## Regex Literals: `#"..."`

Normal strings require double-escaping backslashes (`"\\d+"`). Sema's regex literal syntax avoids this:

```sema
(regex/match? "\\d+" "abc123")   ; normal string — needs \\
(regex/match? #"\d+" "abc123")   ; regex literal — cleaner
```

Inside `#"..."`, backslashes are literal (no escape processing). The only special case is `\"` to insert a quote character.

::: tip
Prefer `#"..."` for regex patterns. It's easier to read and avoids escaping mistakes.
:::

## Matching

### `regex/match?`

Test if a pattern matches anywhere in a string. Returns `#t` or `#f`.

```sema
(regex/match? #"\d+" "abc123")       ; => #t
(regex/match? #"\d+" "no digits")    ; => #f
(regex/match? #"^\d+$" "abc123")     ; => #f  (anchored — must match entire string)
(regex/match? #"^\d+$" "123")        ; => #t
```

### `regex/match`

Match a pattern and return match details as a map, or `nil` if no match.

**Signature:** `(regex/match pattern text) → map | nil`

The returned map contains:

| Key | Value |
|-----|-------|
| `:match` | The full matched substring |
| `:groups` | List of capture groups (group 1, 2, …) |
| `:start` | Start byte offset in the input |
| `:end` | End byte offset in the input |

```sema
(regex/match #"(\d+)-(\w+)" "item-42-foo")
; => {:match "42-foo" :groups ("42" "foo") :start 5 :end 11}

(regex/match #"xyz" "abc")
; => nil
```

Optional capture groups that don't participate in the match become `nil`:

```sema
(regex/match #"(\d+)(?:-(\d+))?" "42")
; => {:match "42" :groups ("42" nil) :start 0 :end 2}
```

::: info Byte offsets
`:start` and `:end` are byte offsets (UTF-8). For ASCII text they match character indices, but for non-ASCII they may differ.
:::

### `regex/find-all`

Find all non-overlapping matches of a pattern.

```sema
(regex/find-all #"\d+" "a1b2c3")          ; => ("1" "2" "3")
(regex/find-all #"[A-Z]" "Hello World")   ; => ("H" "W")
```

## Replacement

### `regex/replace`

Replace the **first** match of a pattern.

**Signature:** `(regex/replace pattern replacement text) → string`

Note the argument order: the pattern comes first and the text last, the
opposite of `string/replace` (`text from to`). `string/replace` and
`string/split` treat a regex literal as plain text — use `regex/replace-all`
and `regex/split` for pattern matching.

```sema
(regex/replace #"\d+" "X" "a1b2c3")    ; => "aXb2c3"
```

Capture group references (`$1`, `$2`, …) work in the replacement string:

```sema
(regex/replace #"(\d+)-(\w+)" "$2:$1" "item-42-foo")
; => "item-foo:42"
```

Named capture groups also work:

```sema
(regex/replace #"(?P<num>\d+)-(?P<word>\w+)" "$word:$num" "item-42-foo")
; => "item-foo:42"
```

### `regex/replace-all`

Replace **all** matches of a pattern.

```sema
(regex/replace-all #"\d" "X" "a1b2")        ; => "aXbX"
(regex/replace-all #"\s+" " " "a  b  c")    ; => "a b c"
```

## Splitting

### `regex/split`

Split a string by a regex delimiter.

```sema
(regex/split #"," "a,b,c")           ; => ("a" "b" "c")
(regex/split #"\s+" "hello  world")  ; => ("hello" "world")
(regex/split #"[,;]" "a,b;c,d")     ; => ("a" "b" "c" "d")
```

## Supported Syntax

Sema uses Rust regex syntax. Common constructs:

| Pattern | Meaning |
|---------|---------|
| `.` | Any character (except newline by default) |
| `\d`, `\w`, `\s` | Digit, word char, whitespace |
| `\D`, `\W`, `\S` | Negated versions |
| `+`, `*`, `?` | One+, zero+, optional |
| `{m,n}` | Between m and n repetitions |
| `^`, `$` | Start/end anchors |
| `(...)` | Capture group |
| `(?:...)` | Non-capturing group |
| `(?P<name>...)` | Named capture group |
| `[abc]`, `[^abc]` | Character class |
| `a\|b` | Alternation |

See the [Rust regex docs](https://docs.rs/regex) for the full reference.

## Escaping Guide

### Regex literals vs normal strings

| Intent | Normal string | Regex literal |
|--------|---------------|---------------|
| One or more digits | `"\\d+"` | `#"\d+"` |
| A literal dot | `"\\."` | `#"\."` |
| A backslash | `"\\\\\\\\"` | `#"\\"` |

### Matching a literal `"` in a regex literal

Inside `#"..."`, use `\"`:

```sema
(regex/match? #"\"[^\"]+\"" "say \"hello\"")
; => #t
```

## Regex vs String Functions

Prefer string functions when possible — they're simpler and faster:

| Need | String function | Regex equivalent |
|------|----------------|------------------|
| Contains? | `string/contains?` | `regex/match?` |
| Starts with? | `string/starts-with?` | `regex/match?` with `^` |
| Simple split | `string/split` | `regex/split` |
| Simple replace | `string/replace` | `regex/replace` |

Use regex when you need character classes, repetition, alternation, or capture groups.

## Practical Examples

### Validate an identifier

```sema
(define (identifier? s)
  (regex/match? #"^[A-Za-z_][A-Za-z0-9_]*$" s))

(identifier? "foo_1")   ; => #t
(identifier? "1foo")    ; => #f
```

### Extract a number from text

```sema
(define (extract-first-int s)
  (let ((m (regex/match #"\d+" s)))
    (if (nil? m)
        nil
        (:match m))))

(extract-first-int "x=42; y=9")  ; => "42"
```

### Normalize whitespace

```sema
(regex/replace-all #"\s+" " " "a   b\n\nc\t\t d")
; => "a b c d"
```

### Parse key-value pairs

```sema
(define (parse-kv line)
  (let ((m (regex/match #"^(\w+)\s*=\s*(.+)$" line)))
    (if (nil? m)
        nil
        (let ((groups (:groups m)))
          {:key (first groups) :value (first (rest groups))}))))

(parse-kv "name = Alice")
; => {:key "name" :value "Alice"}
```

### Find all email-like strings

```sema
(regex/find-all #"[\w.+-]+@[\w-]+\.[\w.]+" "Contact ada@example.com or bob@test.org")
; => ("ada@example.com" "bob@test.org")
```

## Performance Notes

- Each function call **compiles the regex pattern** internally
- For occasional use, this is fine
- For hot loops, consider using `regex/find-all` once instead of many `regex/match?` calls
- Rust regex guarantees **linear-time** matching — no catastrophic backtracking
