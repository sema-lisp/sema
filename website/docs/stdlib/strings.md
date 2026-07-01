---
outline: [2, 3]
---

# Strings & Characters

## Core String Operations

### `string/split`

Split a string by a delimiter.

```sema
(string/split "a,b,c" ",")        ; => ("a" "b" "c")
(string/split "hello world" " ")  ; => ("hello" "world")
```

### `string/lines`

Split into lines on `\n` or `\r\n` (Clojure `split-lines` semantics). A trailing newline does not produce a final empty line — handy for processing logs, config, or file contents. Use `string/split` when you need a literal separator instead.

```sema
(string/lines "a\nb\r\nc\n")   ; => ("a" "b" "c")
(string/lines "single")        ; => ("single")
```

### `string/join`

Join a list of strings with a separator.

```sema
(string/join '("a" "b" "c") ", ")  ; => "a, b, c"
(string/join '("x" "y") "-")      ; => "x-y"
```

### `string/trim`

Remove whitespace from both ends.

```sema
(string/trim "  hello  ")   ; => "hello"
(string/trim "\thello\n")   ; => "hello"
```

### `string/trim-left`

Remove whitespace from the left.

```sema
(string/trim-left "  hi")   ; => "hi"
```

### `string/trim-right`

Remove whitespace from the right.

```sema
(string/trim-right "hi  ")  ; => "hi"
```

### `string/upper`

Convert string to uppercase.

```sema
(string/upper "hello")   ; => "HELLO"
```

### `string/lower`

Convert string to lowercase.

```sema
(string/lower "HELLO")   ; => "hello"
```

### `string/capitalize`

Uppercase the first character and lowercase the rest.

```sema
(string/capitalize "hello")   ; => "Hello"
(string/capitalize "hELLO")   ; => "Hello"
```

### `string/title-case`

Capitalize the first character of each word.

```sema
(string/title-case "hello world")   ; => "Hello World"
```

### `string/contains?`

Test if a string contains a substring.

```sema
(string/contains? "hello" "ell")   ; => #t
(string/contains? "hello" "xyz")   ; => #f
```

### `string/starts-with?`

Test if a string starts with a prefix.

```sema
(string/starts-with? "hello" "he")   ; => #t
(string/starts-with? "hello" "lo")   ; => #f
```

### `string/ends-with?`

Test if a string ends with a suffix.

```sema
(string/ends-with? "hello" "lo")   ; => #t
(string/ends-with? "hello" "he")   ; => #f
```

### `string/replace`

Replace all occurrences of a substring.

```sema
(string/replace "hello" "l" "r")   ; => "herro"
(string/replace "aaa" "a" "b")    ; => "bbb"
```

### `string/index-of`

Return the character index of the first occurrence of a substring, or `nil` if not found.

```sema
(string/index-of "hello" "ll")   ; => 2
(string/index-of "hello" "xyz")  ; => nil
```

### `string/last-index-of`

Find the last occurrence of a substring. Returns the character index or `nil` if not found.

```sema
(string/last-index-of "abcabc" "abc")   ; => 3
(string/last-index-of "hello" "xyz")    ; => nil
```

### `string/chars`

Convert a string to a list of characters.

```sema
(string/chars "abc")   ; => (#\a #\b #\c)
```

### `string/repeat`

Repeat a string N times.

```sema
(string/repeat "ab" 3)   ; => "ababab"
(string/repeat "-" 5)    ; => "-----"
```

### `string/pad-left`

Pad a string on the left to a given width.

```sema
(string/pad-left "42" 5 "0")   ; => "00042"
(string/pad-left "hi" 5)       ; => "   hi"
```

### `string/pad-right`

Pad a string on the right to a given width.

```sema
(string/pad-right "hi" 5)       ; => "hi   "
(string/pad-right "42" 5 "0")   ; => "42000"
```

### `string/width`

Terminal **display width** in columns (not character count): wide characters
(CJK, most emoji) count as 2, combining marks as 0, and ANSI escape sequences as
0. Use it for terminal layout, padding, and alignment, where `string-length` is
wrong for non-ASCII or styled text.

```sema
(string/width "hello")   ; => 5
(string/width "日本語")   ; => 6   (string-length is 3)
(string/width "👋")       ; => 2
```

### `string/word-wrap`

Word-wrap text to a list of lines of at most N display columns. Wraps on spaces,
hard-breaks over-long words on grapheme boundaries, preserves newlines, and
measures with `string/width` (correct for non-ASCII). Distinct from `string/wrap`,
which wraps a string in delimiters.

```sema
(string/word-wrap "the quick brown fox" 10)   ; => ("the quick" "brown fox")
(string/word-wrap "日本語 の テスト" 8)         ; => ("日本語" "の" "テスト")
```

### `string/number?`

Test if a string represents a valid number.

```sema
(string/number? "42")      ; => #t
(string/number? "3.14")   ; => #t
(string/number? "hello")  ; => #f
```

### `string/empty?`

Test if a string is empty.

```sema
(string/empty? "")      ; => #t
(string/empty? "hello") ; => #f
```

### `string/map`

Apply a character function to each character in a string, returning a new string.

```sema
(string/map char/upcase "hello")   ; => "HELLO"
```

### `string/reverse`

Reverse a string.

```sema
(string/reverse "hello")   ; => "olleh"
```

## Unicode & Encoding

### `string/byte-length`

Return the UTF-8 byte length of a string (as opposed to character count from `string/length`). Useful for understanding the actual memory footprint — emoji and CJK characters use more bytes than ASCII.

```sema
(string/byte-length "hello")   ; => 5   (ASCII: 1 byte each)
(string/byte-length "héllo")   ; => 6   (é is 2 bytes in UTF-8)
(string/byte-length "日本語")   ; => 9   (CJK: 3 bytes each)
(string/byte-length "😀")      ; => 4   (emoji: 4 bytes)
```

Compare with `string/length` which counts characters:

```sema
(string/length "😀")           ; => 1   (one character)
(string/byte-length "😀")      ; => 4   (four bytes)
```

### `string/codepoints`

Return a list of Unicode codepoint integers for each character in a string. This reveals the internal structure of composed characters and emoji sequences.

```sema
(string/codepoints "ABC")      ; => (65 66 67)
(string/codepoints "é")        ; => (233)
(string/codepoints "😀")       ; => (128512)
```

Emoji that appear as a single glyph are often multiple codepoints joined by Zero Width Joiner (U+200D = 8205):

```sema
;; 👨‍👩‍👦 is actually 👨 + ZWJ + 👩 + ZWJ + 👦
(string/codepoints "👨‍👩‍👦")   ; => (128104 8205 128105 8205 128102)

;; 👋🏽 is 👋 + skin tone modifier
(string/codepoints "👋🏽")      ; => (128075 127997)
```

### `string/from-codepoints`

Construct a string from a list of Unicode codepoint integers. This is the inverse of `string/codepoints` and enables building emoji programmatically by combining codepoints.

```sema
(string/from-codepoints (list 65 66 67))   ; => "ABC"
(string/from-codepoints (list 233))        ; => "é"
```

Build emoji by combining people with ZWJ (8205):

```sema
;; Build a family: 👨 + ZWJ + 👩 + ZWJ + 👧
(string/from-codepoints (list 128104 8205 128105 8205 128103))
;; => 👨‍👩‍👧

;; Build a profession: 👩 + ZWJ + 💻
(string/from-codepoints (list 128105 8205 128187))
;; => 👩‍💻

;; Add skin tone: 👋 + modifier
(string/from-codepoints (list 128075 127997))
;; => 👋🏽

;; Build flags from Regional Indicators (A=127462):
(string/from-codepoints (list 127475 127476))
;; => 🇳🇴 (NO = Norway)
```

Roundtrip any string through codepoints:

```sema
(string/from-codepoints (string/codepoints "Hello 世界"))
;; => "Hello 世界"
```

### `string/normalize`

Normalize a string to a Unicode normalization form. Supported forms: `:nfc`, `:nfd`, `:nfkc`, `:nfkd` (as keywords or strings).

- **NFC** — Canonical Decomposition, followed by Canonical Composition (most common)
- **NFD** — Canonical Decomposition
- **NFKC** — Compatibility Decomposition, followed by Canonical Composition
- **NFKD** — Compatibility Decomposition

```sema
;; NFC: combine decomposed characters
;; e + combining acute accent → é
(string/normalize "e\u0301" :nfc)    ; => "é"

;; NFD: decompose composed characters
(string/length (string/normalize "é" :nfd))  ; => 2 (e + combining accent)

;; NFKC/NFKD: compatibility decomposition (ligatures, etc.)
(string/normalize "\uFB01" :nfkc)    ; => "fi" (ﬁ ligature → two letters)

;; String form names also work
(string/normalize "e\u0301" "NFC")   ; => "é"
```

### `string/foldcase`

Apply Unicode case folding to a string. Useful for case-insensitive comparisons and normalization. Uses full Unicode-aware lowercasing.

```sema
(string/foldcase "HELLO")        ; => "hello"
(string/foldcase "Hello World")  ; => "hello world"
(string/foldcase "Straße")       ; => "strasse"  (full folding maps ß → ss)
(string/foldcase "ΩΜΕΓΑ")        ; => "ωμεγα"
```

### `string-ci=?`

Case-insensitive string equality comparison. Compares two strings after applying case folding to both.

```sema
(string-ci=? "Hello" "hello")   ; => #t
(string-ci=? "ABC" "abc")       ; => #t
(string-ci=? "CAFÉ" "café")     ; => #t
(string-ci=? "hello" "world")   ; => #f
```

## Scheme Compatibility Aliases

These functions use legacy Scheme/R7RS naming conventions. They work identically to their modern equivalents and are kept for compatibility. Prefer the `string/` namespaced variants in new code.

### `string/append`

Concatenate strings together.

```sema
(string/append "hello" " " "world")   ; => "hello world"
(string/append "a" "b" "c")           ; => "abc"
```

### `string/length`

Return the number of characters in a string.

```sema
(string/length "hello")   ; => 5
(string/length "")        ; => 0
(string/length "héllo")   ; => 5
(string/length "日本語")   ; => 3
```

### `string/ref`

Return the character at a given index.

```sema
(string/ref "hello" 0)    ; => #\h
(string/ref "hello" 4)    ; => #\o
```

### `string/slice`

Extract a substring by start and end character index.

```sema
(string/slice "hello" 1 3)   ; => "el"
(string/slice "hello" 0 5)   ; => "hello"
(string/slice "héllo" 1 2)   ; => "é"
```

### `str`

Convert any value to its string representation.

```sema
(str 42)           ; => "42"
(str #t)           ; => "#t"
(str '(1 2 3))    ; => "(1 2 3)"
```

### `format`

Format a string with `~a` placeholders.

```sema
(format "~a is ~a" "Sema" "great")   ; => "Sema is great"
(format "~a + ~a = ~a" 1 2 3)        ; => "1 + 2 = 3"
```

## Characters

Character literals are written with the `#\` prefix.

```sema
#\a                ; character literal
#\space            ; named character: space
#\newline          ; named character: newline
#\tab              ; named character: tab
```

### `char/to-integer`

Convert a character to its Unicode code point.

```sema
(char/to-integer #\A)   ; => 65
(char/to-integer #\a)   ; => 97
```

### `integer/to-char`

Convert a Unicode code point to a character.

```sema
(integer/to-char 65)    ; => #\A
(integer/to-char 955)   ; => #\λ
```

### `char/alphabetic?`

Test if a character is alphabetic.

```sema
(char/alphabetic? #\a)   ; => #t
(char/alphabetic? #\5)   ; => #f
```

### `char/numeric?`

Test if a character is numeric.

```sema
(char/numeric? #\5)      ; => #t
(char/numeric? #\a)      ; => #f
```

### `char/whitespace?`

Test if a character is whitespace.

```sema
(char/whitespace? #\space)   ; => #t
(char/whitespace? #\a)       ; => #f
```

### `char/upper-case?`

Test if a character is uppercase.

```sema
(char/upper-case? #\A)   ; => #t
(char/upper-case? #\a)   ; => #f
```

### `char/upcase`

Convert a character to uppercase.

```sema
(char/upcase #\a)   ; => #\A
```

### `char/downcase`

Convert a character to lowercase.

```sema
(char/downcase #\Z)   ; => #\z
```

### `char/to-string`

Convert a character to a single-character string.

```sema
(char/to-string #\a)   ; => "a"
```

### `string/to-char`

Convert a single-character string to a character.

```sema
(string/to-char "a")   ; => #\a
```

## Character Comparison (R7RS)

### `char=?`

Character equality.

```sema
(char=? #\a #\a)   ; => #t
(char=? #\a #\b)   ; => #f
```

### `char<?`

Character less-than (by code point).

```sema
(char<? #\a #\b)   ; => #t
```

### `char>?`

Character greater-than.

```sema
(char>? #\b #\a)   ; => #t
```

### `char<=?`

Character less-than-or-equal.

```sema
(char<=? #\a #\b)   ; => #t
(char<=? #\a #\a)   ; => #t
```

### `char>=?`

Character greater-than-or-equal.

```sema
(char>=? #\b #\a)   ; => #t
```

### `char-ci=?`

Case-insensitive character equality.

```sema
(char-ci=? #\A #\a)   ; => #t
```

## Type Conversions

### `string/to-number`

Parse a string as a number.

```sema
(string/to-number "42")     ; => 42
(string/to-number "3.14")  ; => 3.14
```

### `number/to-string`

Convert a number to a string.

```sema
(number/to-string 42)      ; => "42"
(number/to-string 3.14)   ; => "3.14"
```

### `string/to-symbol`

Convert a string to a symbol.

```sema
(string/to-symbol "foo")   ; => foo
```

### `symbol/to-string`

Convert a symbol to a string.

```sema
(symbol/to-string 'foo)   ; => "foo"
```

### `string/to-keyword`

Convert a string to a keyword.

```sema
(string/to-keyword "name")   ; => :name
```

### `keyword/to-string`

Convert a keyword to a string.

```sema
(keyword/to-string :name)   ; => "name"
```

### `string/to-list`

Convert a string to a list of characters.

```sema
(string/to-list "abc")   ; => (#\a #\b #\c)
```

### `list->string`

Convert a list of characters to a string.

```sema
(list->string '(#\h #\i))   ; => "hi"
```

## Slicing & Extraction

### `string/after`

Everything after the first occurrence of a needle. Returns the original string if needle not found.

```sema
(string/after "hello@world.com" "@")  ; => "world.com"
(string/after "no-match" "@")         ; => "no-match"
```

### `string/after-last`

Everything after the last occurrence of a needle.

```sema
(string/after-last "a.b.c" ".")  ; => "c"
```

### `string/before`

Everything before the first occurrence of a needle.

```sema
(string/before "hello@world.com" "@")  ; => "hello"
(string/before "no-match" "@")         ; => "no-match"
```

### `string/before-last`

Everything before the last occurrence of a needle.

```sema
(string/before-last "a.b.c" ".")  ; => "a.b"
```

### `string/between`

Extract the portion between two delimiters.

```sema
(string/between "[hello]" "[" "]")  ; => "hello"
(string/between "start:middle:end" "start:" ":end")  ; => "middle"
```

### `string/take`

Take the first N characters (positive) or last N characters (negative).

```sema
(string/take "hello" 3)   ; => "hel"
(string/take "hello" -2)  ; => "lo"
```

## Prefix & Suffix

### `string/chop-start`

Remove a prefix if present, otherwise return unchanged.

```sema
(string/chop-start "Hello World" "Hello ")  ; => "World"
(string/chop-start "Hello" "Bye")           ; => "Hello"
```

### `string/chop-end`

Remove a suffix if present.

```sema
(string/chop-end "file.txt" ".txt")  ; => "file"
(string/chop-end "file.txt" ".md")   ; => "file.txt"
```

### `string/ensure-start`

Ensure a string starts with a prefix (adds it if missing).

```sema
(string/ensure-start "/path" "/")   ; => "/path"
(string/ensure-start "path" "/")    ; => "/path"
```

### `string/ensure-end`

Ensure a string ends with a suffix.

```sema
(string/ensure-end "path" "/")   ; => "path/"
(string/ensure-end "path/" "/")  ; => "path/"
```

### `string/wrap`

Wrap a string with left and right delimiters.

```sema
(string/wrap "hello" "(" ")")   ; => "(hello)"
(string/wrap "hello" "**")      ; => "**hello**"
```

### `string/unwrap`

Remove surrounding delimiters if both present.

```sema
(string/unwrap "(hello)" "(" ")")  ; => "hello"
(string/unwrap "hello" "(" ")")    ; => "hello"
```

## Replacement

### `string/replace-first`

Replace only the first occurrence of a substring.

```sema
(string/replace-first "aaa" "a" "b")  ; => "baa"
```

### `string/replace-last`

Replace only the last occurrence.

```sema
(string/replace-last "aaa" "a" "b")  ; => "aab"
```

### `string/remove`

Remove all occurrences of a substring.

```sema
(string/remove "hello world" "o")  ; => "hell wrld"
```

## Case Conversion

### `string/snake-case`

Convert to snake_case.

```sema
(string/snake-case "helloWorld")     ; => "hello_world"
(string/snake-case "Hello World")   ; => "hello_world"
```

### `string/kebab-case`

Convert to kebab-case.

```sema
(string/kebab-case "helloWorld")     ; => "hello-world"
(string/kebab-case "Hello World")   ; => "hello-world"
```

### `string/camel-case`

Convert to camelCase.

```sema
(string/camel-case "hello_world")    ; => "helloWorld"
(string/camel-case "Hello World")    ; => "helloWorld"
```

### `string/pascal-case`

Convert to PascalCase.

```sema
(string/pascal-case "hello_world")   ; => "HelloWorld"
(string/pascal-case "hello world")   ; => "HelloWorld"
```

### `string/headline`

Convert to Title Case headline.

```sema
(string/headline "hello_world")   ; => "Hello World"
(string/headline "helloWorld")    ; => "Hello World"
```

### `string/words`

Split an identifier into words. Breaks on `_`, `-`, spaces, and `.`, plus
camelCase / acronym case transitions. Other punctuation stays attached.

```sema
(string/words "hello_world")     ; => ("hello" "world")
(string/words "helloWorld")      ; => ("hello" "World")   ; case transition
(string/words "Hello World!")    ; => ("Hello" "World!")  ; "!" is not a boundary
```
