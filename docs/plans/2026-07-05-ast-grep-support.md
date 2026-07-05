# ast-grep support for Sema

**Status: investigated & verified working end-to-end (2026-07-05). No changes to Sema or the grammar required.**

> **Update (same day):** deferred — see `docs/deferred.md` (AST-GREP-1). The docs
> page (tier 1) was published then pulled back; tier 2 (`@ast-grep/lang-sema`
> upstream PR) was attempted and parked after the `ast-grep/langs` monorepo's
> full install broke on an unrelated package. Revisit per the deferred entry.

ast-grep is a tree-sitter-based structural search/lint/rewrite tool. It supports
arbitrary languages via its *custom language* mechanism: point `sgconfig.yml` at a
tree-sitter grammar compiled as a dynamic library. Since the canonical grammar in
`sema-lisp/tree-sitter-sema` commits its generated `src/parser.c` + `src/scanner.c`,
Sema works with ast-grep today with a one-line `gcc` build and four lines of YAML.

## Verified setup (ast-grep 0.44.1, tree-sitter-sema v0.2.0)

```bash
git clone https://github.com/sema-lisp/tree-sitter-sema
cd tree-sitter-sema
gcc -shared -fPIC -fno-exceptions -O2 -I src -o sema.so src/parser.c src/scanner.c
```

Both `parser.c` and `scanner.c` must be linked — the grammar has an external
scanner (`block_comment`). The resulting `.so` exports `tree_sitter_sema`, which is
the symbol name ast-grep derives from the language key below, so no
`languageSymbol` override is needed.

`sgconfig.yml` in the project to scan:

```yaml
ruleDirs:
  - rules
customLanguages:
  sema:
    libraryPath: ./sema.so   # path to the compiled grammar
    extensions: [sema]
    expandoChar: _
```

Everything in ast-grep's feature surface was exercised against `examples/*.sema`
and works:

- **Patterns with metavariables**: `ast-grep run -p '(define $NAME $VAL)' --lang sema .`
- **Multi-node metavariables**: `(fn ($$$ARGS) $$$BODY)` matches lambdas of any arity
- **Rewrites**: `-p '(println $$$ARGS)' -r '(log/info $$$ARGS)'` produces correct diffs
- **YAML rules** (`ast-grep scan` with `ruleDirs`), including `kind:`-based rules
- **Grammar health**: a `kind: ERROR` rule swept all 67 `examples/*.sema` files —
  zero ERROR nodes. F-strings, regex literals (`#"..."`), short lambdas (`#(...)`),
  byte vectors, and block comments all parse cleanly.

## The one design decision: `expandoChar: _`

ast-grep metavariables are written `$VAR`, and the *pattern itself* is parsed with
the language grammar — so `$VAR` must lex as a single identifier-like node. In Sema,
`$` is not a valid symbol character (the reader rejects bare `$`; the tree-sitter
symbol charset is `[a-zA-ZÀ-ɏ+\-*/!?<>=_&%^~.]` + digits/`#` in rest
position). `expandoChar` exists exactly for this: ast-grep substitutes it for `$`
internally before parsing the pattern. Users still type `$VAR`/`$$$ARGS`.

Choice of character, from the symbol start-charset:

- `_` — **chosen.** `_VAR`/`___ARGS` lex as single symbols; underscore-prefixed
  SCREAMING_CASE symbols don't occur in idiomatic Sema, so no collision. Same
  convention ast-grep documents for Python.
- `%` — rejected: reserved for short-lambda args (`%`, `%1`, `%2`).
- `~`, `^`, `&` — would work, but `_` matches upstream precedent.

## How to ship it

Three tiers, independent of each other:

1. **Docs recipe (cheapest, do first).** A short page under `website/docs/`
   (e.g. `/docs/ast-grep`) with the build command, `sgconfig.yml` snippet, and a
   couple of example patterns/rules. This is the whole integration for CLI users.
2. **`@ast-grep/lang-sema` npm package.** ast-grep maintains the
   [`ast-grep/langs`](https://github.com/ast-grep/langs) monorepo of prebuilt
   grammar packages consumed by `@ast-grep/napi` via `registerDynamicLanguage`.
   Scaffolding is generated with `pnpm create @ast-grep/lang <dir>` (prompts for
   language name, extension, and the source grammar repo); the package ships
   prebuilds with a build-from-source postinstall fallback. This serves JS/TS
   tooling authors and is the standard contribution path — ast-grep does not add
   niche languages to the CLI's built-in set.
3. **Starter rule pack (optional).** A `contrib/ast-grep/` dir in this repo with
   `sgconfig.yml` + a few lint rules for Sema codebases (e.g.
   `(if $C (do $$$B))` → `(when $C $$$B)`, `(if $C $T nil)` → `(when $C $T)`,
   legacy-name nudges like `(string-append ...)` → `string/...` counterparts).
   Rules double as regression tests for the grammar against idiomatic code.

## Caveats

- Matching is structural over the grammar's 18 node kinds (list/vector/hash_map/
  symbol/keyword/...). That's plenty for lint/rewrite, but patterns are
  shape-based, not semantic — `(define $N $V)` inside a quoted form matches too.
  Rules can use `inside:`/`not:` relational constraints where that matters.
- The website's TextMate grammar and the tree-sitter grammar are separate
  artifacts; ast-grep only needs the latter. When `grammar.js` changes upstream,
  consumers just rebuild the `.so` (pin a tag, as the editor plugins do).
- ast-grep custom languages don't get the playground at ast-grep.github.io;
  tier 2 (`@ast-grep/lang-sema`) is what enables programmatic napi use.
