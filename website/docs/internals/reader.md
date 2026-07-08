# Reader Internals

Sema's reader is a two-phase pipeline: a lexer tokenizes source text into `SpannedToken`s, then a recursive descent parser produces `Value` nodes directly — there is no intermediate AST. Source locations are tracked per-token and attached to compound values via an `Rc::as_ptr` trick that avoids growing the NaN-boxed `Value`.

This page documents the lexer, parser, token types, quote desugaring, span tracking, and how the evaluator recovers source positions for error reporting.

## The Lexer

The lexer in `crates/sema-reader/src/lexer.rs` is a single-pass tokenizer that walks a `Vec<char>` with a manual index `i` and tracks `line`/`col` for span information.

Character-level dispatch drives the lexer. Each iteration inspects the current character and branches:

- **Spaces/tabs** — skipped, advances `col`
- **Newline** — emits `Token::Newline` (trivia: the parser skips it, but the formatter and LSP use it)
- **`;`** — comment to end of line, emitted as `Token::Comment` (also trivia)
- **`(`/`)`/`[`/`]`/`{`/`}`** — emit the corresponding bracket token
- **`'`** — emit `Token::Quote`
- **`` ` ``** — emit `Token::Quasiquote`
- **`,`** — peek ahead: `,@` emits `Token::UnquoteSplice`, otherwise `Token::Unquote`
- **`"`** — enter string mode, handle escape sequences
- **`#`** — dispatch on next char: `#t`/`#f` for booleans, `#\` for character literals, `#u8(` for bytevector start, `#(` for short lambdas, `#"` for regex literals (raw strings, no escape processing), `#!` for a shebang line (line 1 only), and the numeric radix (`#x`/`#o`/`#b`/`#d`) and exactness (`#e`/`#i`) prefixes (see [Numeric Literals](#numeric-literals))
- **`:`** — keyword (Clojure-style `:foo`)
- **`f` followed by `"`** — f-string, accumulating literal parts and `${expr}` interpolations
- **Digit or `-` followed by digit** — number: an integer, bignum, rational (`1/3`), float, or complex (`3+4i`), per [Numeric Literals](#numeric-literals)
- **Otherwise** — symbol character, accumulate until delimiter

Every token is wrapped in a `SpannedToken` that records where it begins and ends — both as line/column positions and as byte offsets into the source string (the byte offsets enable exact source extraction for the formatter and LSP). This is the only place source positions enter the system — everything downstream inherits or discards them.

```rust
// crates/sema-reader/src/lexer.rs
pub struct SpannedToken {
    pub token: Token,
    pub span: Span,
    /// Byte offset of the start of this token in the source string.
    pub byte_start: usize,
    /// Byte offset past the end of this token in the source string.
    pub byte_end: usize,
}

// crates/sema-core/src/error.rs
pub struct Span {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
}
```

## Token Zoo

The full `Token` enum:

| Token                   | Syntax           | Example                 |
| ----------------------- | ---------------- | ----------------------- |
| `LParen` / `RParen`     | `(` `)`          | `(+ 1 2)`               |
| `LBracket` / `RBracket` | `[` `]`          | `[1 2 3]`               |
| `LBrace` / `RBrace`     | `{` `}`          | `{:a 1 :b 2}`           |
| `Quote`                 | `'`              | `'foo`                  |
| `Quasiquote`            | `` ` ``          | `` `(a ,b) ``           |
| `Unquote`               | `,`              | `,x`                    |
| `UnquoteSplice`         | `,@`             | `,@xs`                  |
| `Int(i64)`              | digits           | `42`, `-7`              |
| `BigInt(BigInt)`        | out-of-range digits | `99999999999999999999` |
| `Rational(BigRational)` | `n/d`            | `1/3`, `3/6`            |
| `Float(f64)`            | digits with `.`  | `3.14`, `-0.5`          |
| `Complex(re, im)`       | `re±imi`         | `3+4i`, `+i`, `2i`      |
| `String(String)`        | `"..."`          | `"hello"`               |
| `FString(Vec<FStringPart>)` | `f"..."`     | `f"hi ${name}"`         |
| `Regex(String)`         | `#"..."`         | `#"\d+"`                |
| `ShortLambdaStart`      | `#(`             | `#(+ % 1)`              |
| `Symbol(String)`        | identifier       | `define`, `string/trim` |
| `Keyword(String)`       | `:` + name       | `:key`, `:name`         |
| `Bool(bool)`            | `#t` / `#f`      | `#t`                    |
| `Char(char)`            | `#\` + char/name | `#\a`, `#\space`        |
| `BytevectorStart`       | `#u8(`           | `#u8(1 2 3)`            |
| `Dot`                   | `.`              | `(a . b)`               |
| `Comment(String)`       | `;...`           | `; note` (trivia)       |
| `Newline`               | line break       | (trivia)                |

Symbol characters include alphanumeric plus `+ - * / ! ? < > = _ & % ^ ~ .` — a superset of Scheme's identifier syntax that allows operators and predicates like `nil?` or `string/to-number` as plain symbols. After the first character, `#` is also accepted, which is what makes auto-gensym names like `x#` (used inside quasiquote templates) lex as plain symbols.

Booleans accept both `#t`/`#f` (R7RS) and `true`/`false` (as symbol aliases resolved during tokenization).

## The Parser

The parser in `crates/sema-reader/src/reader.rs` is a recursive descent parser that consumes the `Vec<SpannedToken>` produced by the lexer. It's structured as a `Parser` struct with a position index, dispatching on the current token type:

```
parse_expr
  ├── LParen    → parse_list     → Value::List
  ├── LBracket  → parse_vector   → Value::Vector
  ├── LBrace    → parse_map      → Value::Map
  ├── Quote     → desugar        → Value::List [quote, x]
  ├── Quasiquote→ desugar        → Value::List [quasiquote, x]
  ├── Unquote   → desugar        → Value::List [unquote, x]
  ├── UnquoteSplice → desugar    → Value::List [unquote-splicing, x]
  ├── BytevectorStart → parse_bytevector → Value::Bytevector
  ├── ShortLambdaStart → parse_short_lambda → (lambda (%1 …) body)
  ├── Int       → Value::Int
  ├── BigInt    → Value::bigint
  ├── Rational  → Value::rational
  ├── Complex   → Value::complex
  ├── Float     → Value::Float
  ├── String    → Value::String
  ├── FString   → desugar        → Value::List [str, part, …]
  ├── Regex     → Value::String  (raw, no escape processing)
  ├── Symbol    → Value::Symbol
  ├── Keyword   → Value::Keyword
  ├── Bool      → Value::Bool
  └── Char      → Value::Char
```

Each compound form has its own parsing method:

- **`parse_list`** — collects expressions until `)`, handling dotted pairs (see below)
- **`parse_vector`** — collects expressions until `]`, wraps in a vector value via `Value::vector_from_rc`
- **`parse_map`** — collects key-value pairs until `}`, wraps a `BTreeMap` via `Value::map`. Odd element count is a parse error
- **`parse_bytevector`** — collects integers until `)`, validates each is 0–255, wraps in a bytevector via `Value::bytevector`
- **`parse_short_lambda`** — collects the body until `)`, scans it for `%`/`%1`/`%2`… (rewriting bare `%` to `%1`), and produces `(lambda (%1 … %N) body)`

F-strings and regex literals are desugared in `parse_atom`: an f-string becomes a `(str "literal" expr …)` call with each `${...}` interpolation parsed recursively, and a regex literal becomes a plain string value with its contents taken raw (no escape processing).

The parser produces `Value` nodes directly. There is no separate AST type — the same `Value` type used at runtime is the representation of parsed code. This is the Lisp tradition: code is data, and the reader produces data.

> **Comparison:** Racket's reader is configurable with [readtables](https://docs.racket-lang.org/reference/readtables.html) — user code can define new reader syntax. Common Lisp goes further with [reader macros](https://www.lispworks.com/documentation/HyperSpec/Body/02_d.htm) that can override any character's parsing behavior. Sema has neither — quote sugar is hardcoded in the lexer, and there's no mechanism for user-defined reader extensions. This is a deliberate simplicity trade-off: the reader is predictable, the implementation is ~1,400 lines (lexer + parser, excluding tests), and all syntax is documented in one place. See Nystrom's [_Crafting Interpreters_](https://craftinginterpreters.com/parsing-expressions.html) for a thorough treatment of recursive descent parsing, or Aho et al., _Compilers: Principles, Techniques, and Tools_ (the Dragon Book), §4.4 for the theory.

## Numeric Literals

Sema implements the full [numeric tower](/docs/stdlib/math#the-numeric-tower), and the lexer parses each tower type directly from source — the parser only lifts the resulting `Token` into a `Value`. Numeric lexing lives in `crates/sema-reader/src/lexer.rs`.

### Integers and bignums

A plain integer lexes as `Token::Int(i64)` when it fits in an `i64`; an out-of-range integer literal is lexed as an arbitrary-precision `Token::BigInt` instead of overflowing. There is no separate bignum syntax — width is decided by the value.

```sema
42                          ; Int
-7                          ; Int
99999999999999999999999999  ; BigInt (out of i64 range)
```

### Rationals

A `/` immediately followed by one or more digits makes a rational literal, lexed as `Token::Rational` and reduced to lowest terms at read time.

```sema
1/3    ; => 1/3
3/6    ; => 1/2   (reduced)
```

### Complex literals

Complex numbers are written in rectangular form with a trailing `i`, lexed as `Token::Complex(re, im)` whose components are themselves tower reals (int, rational, or float):

```sema
3+4i       ; real 3, imaginary 4
1.5+2.5i   ; float components
3-4i       ; negative imaginary
+i  -i     ; 0+1i, 0-1i
2i  -2i    ; pure imaginary: 0+2i, 0-2i
```

A trailing `i` is only read as imaginary when a **delimiter** follows, so `3ix` still lexes as `Int(3)` followed by the symbol `ix` — the imaginary form does not swallow identifier characters.

### Radix and exactness prefixes

A number body may be preceded by any combination of a **radix** prefix and an **exactness** prefix, in either order:

| Prefix | Meaning |
| --- | --- |
| `#x` / `#o` / `#b` / `#d` | radix 16 / 8 / 2 / 10 (integers only for non-decimal bases) |
| `#e` / `#i` | force the value exact / inexact |

```sema
#xFF     ; => 255
#o17     ; => 15
#b1010   ; => 10
#d42     ; => 42
#e1.5    ; => 3/2    (float forced exact)
#i1/2    ; => 0.5    (rational forced inexact)
#e#xFF   ; => 255    (combined, either order)
#x#eFF   ; => 255
```

Duplicate radix or exactness prefixes are a reader error.

### The `#` delimiter

Because `#` always begins a reader construct (`#(`, `#"…"`, `#x`, `#t`, …), it also **ends** a number, exactly like `(` does. This matters for the trailing `i` of a complex literal: `3+4i#(…)` splits into the complex `3+4i` and a short-lambda `#(…)`, the same way `123#(…)` splits into `Int(123)` and `#(…)`.

## Quote Desugaring

The reader desugars quote syntax into real lists _before the evaluator ever sees them_. This is important: `'x` is not a special syntactic form that the evaluator handles — it's reader sugar that produces a `(quote x)` list.

| Syntax   | Desugars to            | Reader token           |
| -------- | ---------------------- | ---------------------- |
| `'x`     | `(quote x)`            | `Token::Quote`         |
| `` `x `` | `(quasiquote x)`       | `Token::Quasiquote`    |
| `,x`     | `(unquote x)`          | `Token::Unquote`       |
| `,@x`    | `(unquote-splicing x)` | `Token::UnquoteSplice` |

When the parser encounters a `Quote` token, it:

1. Consumes the next expression (recursive `parse_expr` call)
2. Wraps it: `make_list_with_span(vec![Value::symbol("quote"), expr], span)`
3. Attaches the quote token's span to the resulting list

The evaluator then sees `(quote x)` as a normal list whose `car` is the symbol `quote` — which it handles as a special form. The same applies to `quasiquote`, which the evaluator expands recursively (handling nested `unquote` and `unquote-splicing` within templates).

The key distinction: the _syntax_ (`` ` , ,@ ' ``) is reader-level, but the _semantics_ (what `quasiquote` does with its template) is evaluator-level. The reader's job is just to produce the list structure.

## Dotted Pairs

Sema supports dotted pair notation `(a . b)` for compatibility with Scheme's cons-cell tradition, but the representation is unconventional. Since `Value::List` wraps a `Vec<Value>` (not a linked list of cons cells), dotted pairs are represented using a marker symbol:

```sema
(a . b)    ;; parses as a list of three elements: [a, ".", b]
(1 2 . 3)  ;; parses as: [1, 2, ".", 3]
```

The parser's `parse_list` method detects `Token::Dot` and inserts `Value::symbol(".")` into the element list. The evaluator and printer check for this marker when they need to distinguish `(a b c)` from `(a b . c)`.

This is a pragmatic compromise. Real Scheme implementations use linked cons cells where `(a . b)` is `cons(a, b)` — the dot is the _absence_ of a list, not a marker within one. Sema's Vec-based representation can't express improper lists natively, so the dot marker serves as an escape hatch for the few places that need it (mostly association lists and Scheme compatibility).

## String Escapes

The lexer handles common R7RS escape sequences plus Unicode extensions:

| Escape       | Character       | Notes                                       |
| ------------ | --------------- | ------------------------------------------- |
| `\n`         | newline         |                                             |
| `\t`         | tab             |                                             |
| `\r`         | carriage return |                                             |
| `\\`         | backslash       |                                             |
| `\"`         | double quote    |                                             |
| `\0`         | null            |                                             |
| `\$`         | dollar sign     | suppresses `${...}` interpolation in f-strings |
| `\x41;`      | `A` (hex 0x41)  | R7RS — note the trailing semicolon          |
| `\u0041`     | `A`             | 4-digit Unicode escape                      |
| `\U00000041` | `A`             | 8-digit Unicode escape (full Unicode range) |

The R7RS hex escape `\x<hex>;` uses a semicolon terminator, which is unusual — most languages use a fixed digit count. This allows variable-length hex sequences: `\x41;` and `\x041;` are both valid and produce the same character. The semicolon disambiguates where the hex digits end.

The `\uNNNN` and `\UNNNNNNNN` forms follow the C/Java/Rust convention of fixed-width escapes. These are Sema extensions not found in R7RS.

Character literals follow a similar pattern:

| Literal     | Character         |
| ----------- | ----------------- |
| `#\a`       | the character `a` |
| `#\space`   | space             |
| `#\newline` | newline           |
| `#\tab`     | tab               |
| `#\return`  | carriage return   |
| `#\nul`     | null              |

## Span Tracking

This is the most architecturally interesting part of the reader. The problem: error messages need source locations ("line 12, column 5"), but `Value` is a NaN-boxed 8-byte handle (a single `u64`) — there is no room for a `Span` inside it, and growing every value in the system to make room would defeat the point of NaN-boxing, including for runtime values that were never parsed from source.

**The solution:** spans are stored in a side table keyed by `Rc` pointer addresses.

```rust
// crates/sema-reader/src/reader.rs
fn make_list_with_span(&mut self, items: Vec<Value>, span: Span) -> Result<Value, SemaError> {
    let rc = Rc::new(items);
    let ptr = Rc::as_ptr(&rc) as usize;
    self.span_map.insert(ptr, span);
    Ok(Value::list_from_rc(rc))
}
```

The `SpanMap` is a `HashMap<usize, Span>` — it maps `Rc::as_ptr()` cast to `usize` to the source span. This works because:

1. **`Rc::as_ptr` is stable** — for a given `Rc`, the inner pointer doesn't change as long as the `Rc` (or any clone of it) is alive
2. **Clones share the pointer** — `Rc::clone()` increments the refcount but doesn't change the underlying pointer, so a cloned list still maps to the same span
3. **No cost to non-compound values** — atoms (integers, strings, symbols) don't get spans. Both `Value::List` and `Value::Vector` participate in span tracking — the reader inserts their `Rc::as_ptr()` addresses into the span map. However, the evaluator's `span_of_expr` currently only recovers spans from `Value::List`; vector spans are tracked by the reader but not used during error reporting

**The trade-off:** when the `Rc` is deallocated, its pointer address could be reused by a new allocation, producing a stale span lookup. In practice this is a minor diagnostic risk rather than a correctness issue — a wrong span in an error message is better than no span. The span table accumulates entries across parsed inputs; in long-running processes (REPL, embedding), stale entries could theoretically produce misleading source locations, though this has not been observed in practice. Also, only list-shaped values get spans. An error in evaluating an atom like `undefined-var` won't have a direct span — the evaluator must use the span of the enclosing list expression instead.

### Span Recovery in the Evaluator

The span table is a field in `EvalContext`, populated when source is parsed via `ctx.merge_span_table(spans)`:

```rust
// crates/sema-eval/src/eval.rs
fn span_of_expr(ctx: &EvalContext, expr: &Value) -> Option<Span> {
    if let Some(items) = expr.as_list_rc() {
        let ptr = Rc::as_ptr(&items) as usize;
        ctx.lookup_span(ptr)
    } else {
        None
    }
}
```

The evaluator calls `span_of_expr` when constructing error messages, attaching the source position of the failing expression to the `SemaError`. This flows through the call stack and ultimately appears in error output like:

```
Error at line 12, col 5: undefined variable 'foo'
```

## Error Reporting

Errors flow through two mechanisms:

1. **`SemaError::Reader`** — carries a `Span` directly for parse-time errors (unmatched brackets, invalid escape sequences, unexpected tokens). These are produced by the lexer and parser before evaluation begins.

2. **`CallFrame` + span table** — for runtime errors, the evaluator maintains a call stack of `CallFrame`s. When an error occurs, it walks the stack, using `span_of_expr` to find source positions for each frame. This produces stack traces with source locations even though `Value` itself carries no span.

The combination means parse errors report exact positions (the lexer knows where every token starts), while runtime errors report the position of the enclosing list expression (the best available approximation from the span table).

## Public API

The reader exposes five entry points:

```rust
// crates/sema-reader/src/reader.rs

/// Parse a single expression from input
pub fn read(input: &str) -> Result<Value, SemaError>

/// Parse all expressions from input
pub fn read_many(input: &str) -> Result<Vec<Value>, SemaError>

/// Parse all expressions and return the span map for error reporting
pub fn read_many_with_spans(input: &str) -> Result<(Vec<Value>, SpanMap), SemaError>

/// Parse all expressions, also returning per-symbol spans
/// (enables precise go-to-definition in the LSP)
pub fn read_many_with_symbol_spans(input: &str)
    -> Result<(Vec<Value>, SpanMap, Vec<(String, Span)>), SemaError>

/// Parse with error recovery: on a parse error, skip to the next
/// top-level form and continue, collecting all errors
pub fn read_many_with_spans_recover(input: &str)
    -> (Vec<Value>, SpanMap, Vec<(String, Span)>, Vec<SemaError>)
```

`read_many_with_spans` is what the evaluator uses — it needs the span map to populate the `EvalContext`'s span table. `read_many_with_spans_recover` is what the LSP uses: it never bails on the first error, so diagnostics, completions, and navigation keep working while the file is mid-edit. The simpler `read` and `read_many` are convenience wrappers for contexts where error positions aren't needed (tests, REPL one-liners).

## Pipeline Summary

```
Source text
  │
  ▼
tokenize()          crates/sema-reader/src/lexer.rs
  │                 "single-pass"
  │                 "produces Vec<SpannedToken>"
  ▼
Parser::parse()     crates/sema-reader/src/reader.rs
  │                 "recursive descent"
  │                 "produces Vec<Value> + SpanMap"
  ▼
ctx.merge_span_table()  crates/sema-eval/src/eval.rs
  │                 "populates EvalContext span table"
  ▼
eval()              crates/sema-eval/src/eval.rs
  │                 "trampoline-based TCO evaluator"
  │                 "recovers spans via Rc::as_ptr lookup"
  ▼
Value               result
```
