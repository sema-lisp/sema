---
outline: [2, 3]
---

# Structural Search with ast-grep

[ast-grep](https://ast-grep.github.io) is a tree-sitter-based tool for structural code search, linting, and rewriting. Instead of matching text like `grep`, it matches syntax trees — so a pattern like `(define $NAME $VAL)` finds every top-level definition regardless of formatting, and rewrites preserve everything the pattern didn't capture.

Sema plugs into ast-grep as a [custom language](https://ast-grep.github.io/advanced/custom-language.html) using the official [`tree-sitter-sema`](https://github.com/sema-lisp/tree-sitter-sema) grammar. The whole setup is one compiled grammar file and a few lines of YAML.

## Setup

**1. Build the grammar as a dynamic library.** The grammar repo commits its generated parser sources, so a C compiler is all you need:

```bash
git clone https://github.com/sema-lisp/tree-sitter-sema
cd tree-sitter-sema
gcc -shared -fPIC -fno-exceptions -O2 -I src -o sema.so src/parser.c src/scanner.c
```

Both `parser.c` and `scanner.c` must be linked — the grammar uses an external scanner for block comments. If you have the tree-sitter CLI installed, `tree-sitter build -o sema.so` does the same thing.

**2. Register the language.** In the project you want to search, create `sgconfig.yml`:

```yaml
ruleDirs:
  - rules
customLanguages:
  sema:
    libraryPath: ./sema.so   # wherever you put the compiled grammar
    extensions: [sema]
    expandoChar: _
```

That's it — `ast-grep` now understands `.sema` files.

::: tip Why `expandoChar`?
ast-grep parses patterns with the language's own grammar, and `$` isn't a valid symbol character in Sema. `expandoChar: _` tells ast-grep to substitute `_` internally — **you still write `$VAR` and `$$$ARGS` in patterns**; the substitution is invisible.
:::

## Searching

Metavariables (`$NAME`) match any single node; `$$$ARGS` matches zero or more:

```bash
# every definition
ast-grep run -p '(define $NAME $VAL)' --lang sema .

# lambdas of any arity
ast-grep run -p '(fn ($$$ARGS) $$$BODY)' --lang sema .

# every call to a specific function, however many arguments
ast-grep run -p '(http/get $$$ARGS)' --lang sema .
```

Repeating a metavariable constrains matches to be structurally equal — `(if $C $X $X)` finds `if` forms whose branches are identical.

## Rewriting

`--rewrite` (`-r`) replaces matches, splicing captured metavariables into the replacement:

```bash
# migrate println calls to a logging function
ast-grep run -p '(println $$$ARGS)' -r '(log/info $$$ARGS)' --lang sema --interactive .
```

`--interactive` shows each diff and asks before applying; add `--update-all` to apply everything at once.

## Lint rules

Rules are YAML files in `ruleDirs`, run with `ast-grep scan`:

```yaml
id: prefer-when-over-if-do
language: sema
severity: warning
message: Prefer (when cond ...) over (if cond (do ...))
rule:
  pattern: (if $COND (do $$$BODY))
fix: (when $COND $$$BODY)
```

Rules can also match by node kind and combine relational constraints (`inside`, `has`, `not`, …). The grammar's named kinds are: `program`, `list`, `vector`, `hash_map`, `byte_vector`, `short_lambda`, `quote`, `quasiquote`, `unquote`, `unquote_splicing`, `deref`, `symbol`, `keyword`, `string`, `regex`, `number`, `boolean`, `character`, `comment`, `block_comment`, `shebang`. For example, to flag string literals directly inside a vector:

```yaml
rule:
  kind: string
  inside:
    kind: vector
```

## JavaScript API

For programmatic use from Node.js, the [`@ast-grep/lang-sema`](https://github.com/ast-grep/langs) package registers Sema with [`@ast-grep/napi`](https://www.npmjs.com/package/@ast-grep/napi):

```js
import sema from '@ast-grep/lang-sema'
import { registerDynamicLanguage, parse } from '@ast-grep/napi'

registerDynamicLanguage({ sema })

const sg = parse('sema', '(define greeting "hello")')
const def = sg.root().find('(define $NAME $VAL)')
console.log(def.getMatch('NAME').text()) // => greeting
```

## Notes

- Matching is **structural, not semantic**: `(define $N $V)` also matches inside quoted data. Use `not: { inside: ... }` constraints where that matters.
- Patterns must be complete, parseable forms — the pattern is parsed with the Sema grammar itself.
- Pin a tagged release of `tree-sitter-sema` and rebuild the `.so` when you bump it, the same way the editor plugins consume the grammar.
