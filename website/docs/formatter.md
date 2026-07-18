---
outline: [2, 3]
---

# Formatter

Sema includes a built-in code formatter that enforces consistent style across your codebase. It preserves all comments, handles shebang lines, and produces idempotent output.

## Usage

```
sema fmt [OPTIONS] [FILES...]
```

With no arguments, `sema fmt` formats all `.sema` files in the current directory recursively.

### Options

| Flag | Description |
| --- | --- |
| `--check` | Check formatting without writing changes (exit 1 if unformatted) |
| `--diff` | Print diff of formatting changes |
| `--width <N>` | Max line width (default: `80`) |
| `--indent <N>` | Indentation width for body forms (default: `2`) |
| `--align` | Align consecutive similar forms (defines, cond clauses, let bindings) |
| `--json` | Emit read-only NDJSON results for editor integrations |

### Examples

```bash
# Format all .sema files in current directory
sema fmt

# Format specific files
sema fmt src/main.sema lib/utils.sema

# Format with glob patterns
sema fmt "src/**/*.sema"

# Check formatting in CI (exits 1 if changes needed)
sema fmt --check

# Preview changes without writing
sema fmt --diff

# Use wider lines and 4-space indent
sema fmt --width 100 --indent 4

# Enable decorative alignment
sema fmt --align
```

## JSON output

`--json` formats without rewriting files and emits one JSON object per file:

```bash
sema fmt --json src/main.sema
```

```json
{"file":"src/main.sema","formatted":true,"changed":true,"source":"(define x 1)\n"}
```

`formatted` reports whether formatting succeeded. `changed` reports whether `source` differs from the input file. Multiple files produce newline-delimited JSON (NDJSON), with no human-readable summary. If no files match, stdout is empty.

Read and formatting failures emit `{"file":"...","formatted":false,"error":"..."}` and cause a non-zero exit status after all resolved files are processed. `--check --json` also exits with status 1 when any file would change. `--diff` cannot be combined with `--json`.

For stdin, `sema fmt --json -` emits one object with `formatted` and either `source` or `error`; stdin results do not include `file` or `changed`.

## Project Configuration

Create a `sema.toml` file in your project root to set persistent formatting options. The formatter walks up from the current directory to find the nearest `sema.toml`.

```toml
[fmt]
width = 80
indent = 2
align = false
```

### Options

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `width` | integer | `80` | Maximum line width |
| `indent` | integer | `2` | Number of spaces for body indentation |
| `align` | boolean | `false` | Enable decorative column alignment |

### Precedence

Settings are merged in this order (later wins):

1. **Defaults** — `width=80`, `indent=2`, `align=false`
2. **`sema.toml`** — project-level configuration
3. **CLI flags** — `--width`, `--indent`, `--align` override everything

```bash
# sema.toml sets width=100, but CLI overrides to 120
sema fmt --width 120
```

## Formatting Rules

### Line Breaking

The formatter uses a "try flat, then multi-line" strategy. If a form fits within the line width, it stays on one line. Otherwise, it breaks across multiple lines with appropriate indentation.

```scheme
;; Fits on one line
(+ 1 2 3)

;; Too long — breaks with body indentation
(define (calculate-fibonacci-sequence n)
  (if (< n 2)
    n
    (+ (calculate-fibonacci-sequence (- n 1))
      (calculate-fibonacci-sequence (- n 2)))))
```

### Form-Aware Indentation

The formatter recognizes Sema's special forms and applies context-appropriate indentation:

**Body forms** (`define`, `defn`, `fn`, `lambda`, `do`, `when`, `unless`, etc.) place the head and key arguments on the first line, then indent the body:

```scheme
(defn factorial (n)
  (if (< n 2)
    n
    (* n (factorial (- n 1)))))
```

**Binding forms** (`let`, `let*`, `letrec`, `when-let`, `if-let`) keep bindings aligned:

```scheme
(let ((x 1)
      (y 2)
      (z 3))
  (+ x y z))
```

**Clause forms** (`cond`, `case`, `match`) indent each clause:

```scheme
(cond
  ((= x 1) "one")
  ((= x 2) "two")
  (else "other"))
```

**Threading macros** (`->`, `->>`, `as->`, `some->`) indent each step:

```scheme
(-> data
  (filter even?)
  (map square)
  (reduce +))
```

**Conditionals** (`if`) place condition, then-branch, and else-branch on separate lines when they don't fit:

```scheme
(if (> x 0)
  "positive"
  "non-positive")
```

### Comment Preservation

All comments are preserved in their original positions — inline, trailing, and standalone:

```scheme
;; Module header comment
(define x 42) ; inline comment

;; Between forms
(define y 10)
```

### Decorative Alignment

When `--align` is enabled (or `align = true` in `sema.toml`), the formatter column-aligns consecutive similar forms for visual clarity. This is opt-in because it can cause noisier git diffs.

**Aligned defines:**

```scheme
(define x         1)
(define longer-y  2)
(define z         3)
```

**Aligned cond clauses:**

```scheme
(cond
  ((= x 1)    "one")
  ((= x 100)  "hundred")
  (else       "other"))
```

Alignment groups are broken by blank lines, so you can control which forms get aligned together.
