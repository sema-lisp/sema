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
| `--align` | Column-align consecutive similar forms (defines, let bindings, cond clauses, map values) |
| `--max-blank-lines <N>` | Max consecutive blank lines to keep (default: `1`) |

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

## Project Configuration

Create a `sema.toml` file in your project root to set persistent formatting options. The formatter walks up from the current directory to find the nearest `sema.toml`.

```toml
[fmt]
width = 80
indent = 2
align = false
max-blank-lines = 1
ignore = ["vendor", "gen/**", "*.generated.sema"]
```

### Options

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `width` | integer | `80` | Maximum line width |
| `indent` | integer | `2` | Number of spaces for body indentation |
| `align` | boolean | `false` | Column-align consecutive similar forms (defines, let bindings, cond clauses, map values) |
| `max-blank-lines` | integer | `1` | Longest run of consecutive blank lines to preserve; longer runs are collapsed. `0` removes all blank lines |
| `ignore` | array | `[]` | Paths excluded from formatting. An entry with glob characters matches as a glob (`gen/**`, `*.generated.sema`); anything else is a literal path prefix that excludes a file or a whole directory (`vendor`). Matched relative to the working directory |

### Precedence

Settings are merged in this order (later wins):

1. **Defaults** — `width=80`, `indent=2`, `align=false`, `max-blank-lines=1`
2. **`sema.toml`** — project-level configuration
3. **CLI flags** — `--width`, `--indent`, `--align`, `--max-blank-lines` override everything

```bash
# sema.toml sets width=100, but CLI overrides to 120
sema fmt --width 120
```

### Excluded Files

Two rules keep `sema fmt` away from files it shouldn't touch:

- The recursive walk (both the no-argument default and glob patterns you pass) skips **hidden directories** — `.git`, editor state, worktrees — unless a pattern names one literally.
- Paths matching an `ignore` entry are excluded from discovery. Naming a file **explicitly** always formats it, ignore list or not:

```bash
# vendor/ is in the ignore list — the recursive walk skips it...
sema fmt

# ...but an explicit path always formats
sema fmt vendor/lib.sema
```

## Disabling the Formatter for a Region

Sometimes hand-made layout carries meaning the formatter can't know about — a matrix written as a grid, a lookup table with meaningful columns, ASCII art in data. Fence such a region with `@formatter:off` / `@formatter:on` comments (the IntelliJ-family convention) and `sema fmt` passes it through byte-for-byte:

```scheme
(define scale 2.0)

; @formatter:off
(define identity-matrix
  [1.0  0.0  0.0
   0.0  1.0  0.0
   0.0  0.0  1.0])
; @formatter:on

(define origin {:x 0 :y 0})
```

Everything from the start of the `@formatter:off` line through the end of the `@formatter:on` line is preserved exactly; the code before and after formats normally.

Rules:

- The fence is a line comment whose text (after the `;`s) is exactly `@formatter:off` or `@formatter:on` — any number of leading semicolons works (`;`, `;;`, `;;;`).
- Fences only take effect **at the top level**. Inside a form they are ordinary comments and the form formats normally.
- An `@formatter:off` with no matching `@formatter:on` disables formatting through the end of the file.
- A stray `@formatter:on` with no active `off` region is an ordinary comment.

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

**Clause forms** (`cond`, `case`, `match`, `match*`) indent each clause. `case`/`match` keep their subject on the head line:

```scheme
(cond
  ((= x 1) "one")
  ((= x 2) "two")
  (else "other"))

(case status
  (200 "ok")
  (404 "missing")
  (else "error"))
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

All comments are preserved. A trailing comment stays on the line of the form it annotates; a standalone comment keeps its own line, above the form it documents:

```scheme
;; Module header comment
(define x 42) ; stays on this line

;; Documents the next form
(define y 10)
```

### Bytevector Literals

A multi-line `#u8(...)` literal keeps the row breaks you wrote (with spacing normalized), so a hand-arranged grid keeps its shape. A single-line literal that exceeds the width wraps at the width:

```scheme
;; Hand-arranged rows are kept
#u8(1 2 3 4
    5 6 7 8)

;; A too-long single line wraps
(define png-header
  #u8(137 80 78 71 13 10 26 10 137 80 78 71 13 10 26 10 137 80 78 71 13 10 26 10
      137 80))
```

### Decorative Alignment

When `--align` is enabled (or `align = true` in `sema.toml`), the formatter column-aligns consecutive similar forms for visual clarity. This is opt-in because it can cause noisier git diffs.

**Defines** — consecutive one-liner defines (`define`, `def`, `defn`, `defun`, `defmacro`) align their values, and trailing comments share a column past the widest value:

```scheme
(define *rows*    24)
(define *cols*    80)
(define *cursor*  0)   ;; caret index
(define *scroll*  0)   ;; lines scrolled
```

**Map literals** — the values of a multi-line map align to the widest key:

```scheme
(define default-keymap
  {:mcp        "ctrl-o"
   :resume     "ctrl-r"
   :palette    "ctrl-k"
   :interrupt  "ctrl-c"})
```

**Let bindings and cond/case clauses:**

```scheme
(let ((x       1)
      (longer  2))
  (+ x longer))

(cond
  ((= x 1)    "one")
  ((= x 100)  "hundred")
  (else       "other"))
```

Alignment is conservative: it applies only where a shared column fits the line width and there is a width difference worth aligning. A member that is too wide, contains comments, or spans multiple lines is formatted normally and splits the alignment run; blank lines also break a run, so you control which forms align together. Alignment never rewrites structure — a `[..]` binding pair keeps its brackets, and multi-line function bodies are never joined onto one line.
