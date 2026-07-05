# ast-grep integration

User-facing setup lives at [sema-lang.com/docs/ast-grep](https://sema-lang.com/docs/ast-grep)
(source: `website/docs/ast-grep.md`). This directory holds the pieces destined
for external repos.

## `lang-sema/` — staged `@ast-grep/lang-sema` package

A ready-to-submit package for the [`ast-grep/langs`](https://github.com/ast-grep/langs)
monorepo, which publishes the `@ast-grep/lang-*` packages consumed by
`@ast-grep/napi`'s `registerDynamicLanguage`. The files mirror the structure of
the existing packages there (`packages/toml` was the reference) and were
verified end-to-end: `node nursery.js source` (copies grammar `src/`, generates
`type.d.ts`), parser build, and `node nursery.js test` (registers the language
with `@ast-grep/napi`, parses Sema, matches `(define $NAME $VAL)` and checks
metavariable captures) all pass.

To submit upstream:

1. Fork `ast-grep/langs` and copy `lang-sema/` to `packages/sema/`.
2. Run `pnpm install` at the monorepo root, then in `packages/sema`:
   `pnpm source && pnpm build && pnpm test`.
3. Open the PR. Their CI builds prebuilds per platform and publishes to npm.

Notes for the submission:

- `tree-sitter-sema` is not yet published to npm, so the devDependency is a
  git-pinned reference (`github:sema-lisp/tree-sitter-sema#v0.2.0`). The
  nursery tooling only needs the committed `src/` directory, which the pin
  provides. If the ast-grep maintainers prefer a registry package, publish
  `tree-sitter-sema` to npm from the grammar repo first and swap the pin for
  a version range.
- Keep the pinned grammar tag in sync with the editor plugins when bumping.
