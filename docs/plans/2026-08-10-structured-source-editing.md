# Structured source editing and MCP tools

**Status:** Approved implementation plan for
[issue #77](https://github.com/sema-lisp/sema/issues/77). Implementation has not started.
The owner approved D1-D3 on 2026-08-11.

## Decision status

| ID | Decision | Approved design | Other choices and effect |
|---|---|---|---|
| D1 | Source preservation | Preserve untouched bytes and replace or insert only the affected byte ranges. Render an edited form as canonical source when it cannot be represented as smaller exact edits. | Whole-file canonical rendering removes most origin tracking but changes comments, blank lines, and reader sugar. Full concrete-syntax editing requires a larger trivia-preserving tree and delays the first release. |
| D2 | Form path model | Paths address the semantic forms returned by `read/all`. Synthetic forms created by reader desugaring point to the nearest source form that can be rewritten. | Concrete paths do not match `read/all`. Supporting concrete syntax too requires a separate `SyntaxPath` and translation errors. |
| D3 | `rename-binding` scope | Rename one resolved binding and its references in one file. A top-level rename reports that references in other files were not checked. | Local-only rename rejects top-level bindings. Workspace rename requires an explicit file set, a multi-file transaction, and a separate failure model. |

No product decision blocks implementation. Section
[Effect of alternate decisions](#effect-of-alternate-decisions) records the designs that
were considered but not selected.

## Problem

Sema can parse source into `Value` forms, format one form, and compile-check a string.
It cannot safely connect those steps into a source edit:

1. `read/all` drops comments and blank lines and desugars quote prefixes, f-strings,
   regex literals, and short lambdas.
2. There is no stable path contract for a nested form.
3. There is no common operation engine for constrained edits, ambiguity checks, no-change
   checks, validation, diff generation, and file writes.
4. `sema mcp` exposes only text results and has no structured source tools.
5. A file edit must apply both sandbox capabilities and allowed-path checks before it
   reads or writes.

The result must let a Sema program or an MCP client inspect a file, select forms by a
deterministic path, request a constrained edit, preview it, validate it, and write it
without changing unrelated files.

## Evidence from the current code

- `crates/sema-stdlib/src/reflect.rs` implements `read/string`, `read/all`,
  `format/form`, `sema/check-string`, and `sema/check-file`. `format/form` formats
  `Value::to_string()` and falls back to unformatted text, so it is not a strict
  source serializer.
- `crates/sema-reader/src/lexer.rs` already records `byte_start`, `byte_end`, and a
  line/column `Span` for every token, including comments and newlines. This is enough
  to create exact source origins without changing `Value`.
- `crates/sema-reader/src/reader.rs` records compound-form spans by `Rc` identity and
  symbol spans separately. It does not record origins for every semantic child.
- `crates/sema-fmt/src/formatter.rs` has a private source-faithful `Node` tree and
  preserves comments, shebangs, reader sugar, and literal spelling. It formats a full
  source string; it does not map semantic `Value` paths to byte ranges.
- `crates/sema-lsp/src/scope.rs` already resolves bindings and scope-aware references.
  It depends only on `sema-core` data and reader output, but it currently belongs to
  the LSP crate and cannot be reused by `sema-stdlib` or `sema-mcp`.
- `crates/sema-core/src/sandbox.rs` provides capability checks and canonical
  allowed-path checks. Existing-file canonicalization detects `..` and symlink escapes.
- `crates/sema-mcp/src/tools.rs` registers default tools in one list and dispatches
  them in one match. Its `CallToolResult` contains text plus `isError`, but not
  `structuredContent`. The server announces MCP `2024-11-05`; the MCP client already
  uses `2025-11-25` and passes unknown structured result fields through as JSON.
- `scripts/test-packaged-sema-web.sh` packages the whole workspace and has an explicit
  patch list of Sema crates. A new published crate must be added to that list.

## Goals

1. Add strict `form->source` and `forms->source` conversions.
2. Define deterministic paths over all source-representable Sema forms.
3. Add pure form traversal and replacement APIs.
4. Add one shared structured edit engine for Sema and MCP.
5. Implement every initial operation named in issue #77:
   `replace-symbol`, `rename-binding`, `insert-definition`,
   `replace-form-at-path`, `wrap-form`, `unwrap-form`, `append-to-list`,
   `replace-literal`, and `rewrite-call`.
6. Preserve untouched source bytes under D1 and explain every case that must render an
   enclosing form as canonical source.
7. Detect ambiguous targets and unchanged results instead of guessing or reporting false
   success.
8. Parse-check and compile-check the result before a write.
9. Apply capability, allowed-path, traversal, and symlink checks to every new file tool.
10. Provide dry-run, diff preview, content hashes, and structured result data in Sema
    and MCP.
11. Keep the shipped binary independent of the repository and development tools.

## Non-goals for this issue

- A general lossless parser that lets callers rearrange comments as syntax nodes.
- Formatting arbitrary source ranges independently of the normal formatter.
- Cross-file or project-wide transactions under D3.
- Macro expansion before matching. Operations inspect the source forms, not expanded
  CoreExpr or bytecode.
- Editing malformed files. Inspection can return parse diagnostics, but semantic paths
  require a successful full parse.
- Replacing the LSP rename protocol or changing its current workspace behavior.
- Generic text patches. The new edit tool accepts only named structured operations.
- Preserving inode identity, hard-link relationships, ACLs, or extended attributes
  across atomic replacement. The writer copies the target's `std::fs::Permissions`
  value and documents this metadata contract.

## Terminology and naming contract

Use these terms consistently in code, schemas, documentation, and errors:

| Term | Definition |
|---|---|
| source document | One complete UTF-8 source string and the forms, origins, diagnostics, and content hash derived from it. |
| form | One semantic `Value` returned by the reader. Comments, blank lines, and a shebang are source text but are not forms. |
| node | One public inspection record that combines a form path, form summary, and origin. It is not a concrete-syntax node. |
| trivia | Whitespace, comments, and a shebang that remain source text and have no `FormPath`. |
| `FormPath` | A non-empty absolute path into a source document. Its first segment selects a top-level form. |
| `SubformPath` | A path relative to one form. The empty path selects that form. |
| origin | The exact byte range, rewrite byte range, span, and source representation kind associated with a form. |
| target path | The `FormPath` that an operation will edit. |
| within path | An optional `FormPath` that limits a search without selecting one exact target. |
| edit proposal | The pure in-memory result of applying an edit request to source text. It contains proposed source and validation data but no filesystem status. |
| text edit | One non-overlapping replacement of a half-open UTF-8 byte range. |
| rewrite range | The smallest exact source range that must be rendered when a synthetic form changes. |
| canonical source | Deterministic source emitted by the strict renderer. It need not retain the original comments, whitespace, reader sugar, or literal spelling. |
| content hash | `sha256:` followed by 64 lowercase hexadecimal digits for the exact source bytes. |
| newline style | `lf`, `crlf`, `none`, or `mixed`, derived from the source bytes. |
| validation mode | The check that gates a result: `parse` or `compile`. |

Public names follow one mechanical mapping:

| Context | Convention | Examples |
|---|---|---|
| Sema functions | slash namespaces or established arrow conversions | `source/edit`, `form/at-path`, `forms->source` |
| Sema operation values, other enum values, option keys, result keys, and error kinds | kebab-case keywords | `:replace-symbol`, `:target-path`, `:stale-content` |
| MCP tool names and JSON field names | snake_case | `structured_edit`, `target_path`, `content_hash_before` |
| MCP operation, enum, and error-kind values | the same kebab-case strings as Sema, without the keyword colon | `"replace-symbol"`, `"stale-content"` |
| Rust types and enum variants | `UpperCamelCase` | `FormPath`, `EditRequest`, `SourceErrorKind::StaleContent` |
| Rust modules, functions, and fields | snake_case | `text_edit`, `propose_edit`, `target_path` |

The positional `path` argument and the MCP `path` field always mean a filesystem path.
Fields that contain a `FormPath` use a role-specific name such as `target-path`,
`within-path`, `root-path`, `before-path`, or `after-path`. Do not use the unqualified
name `path` for a form path.

## Architecture

Add a workspace crate named `sema-source`:

Using the repository's “dependency ← consumer” notation, the new edges are:

```text
sema-core/sema-reader/sema-vm/sema-fmt ← sema-source
sema-source ← sema-stdlib
sema-source ← sema-lsp
sema-source ← sema-mcp
```

The concrete Cargo dependencies are:

- `sema-source`: `sema-core`, `sema-reader`, `sema-vm`, `sema-fmt`, `similar`,
  `sha2`, `hex`, `serde`, `serde_json`, and `thiserror`.
- `sema-source` on Windows: `windows-sys` with `Win32_Foundation`,
  `Win32_Storage_FileSystem`, and `Win32_System_IO` for replacement and flush APIs.
- `sema-stdlib`, `sema-lsp`, and `sema-mcp`: add `sema-source`.
- `sema-source` must not depend on `sema-eval`, `sema-stdlib`, `sema-lsp`, or
  `sema-mcp`.

This preserves the rule that `sema-stdlib` does not depend on `sema-eval`. It also
keeps one implementation of paths, checks, operations, scope resolution, result data,
and file transactions.

Suggested crate layout:

| File | Responsibility |
|---|---|
| `crates/sema-source/src/path.rs` | `FormPath`, `SubformPath`, path parsing, traversal, immutable replacement |
| `document.rs` | parsed forms, line index, origins, inspection records |
| `render.rs` | strict single-form and multi-form source rendering |
| `query.rs` | pre-order traversal, find queries, quote classification |
| `scope.rs` | shared definition classification, binding extraction, and reference resolution extracted from `sema-lsp` |
| `operation.rs` | typed requests and the nine edit operations |
| `text_edit.rs` | byte edits, overlap coalescing, indentation, edit application |
| `check.rs` | separate parse and compile reports and diagnostics |
| `diff.rs` | bounded unified diff preview and truncation metadata |
| `file.rs` | native-only sandboxed read, optimistic content-hash check, and atomic replacement |
| `result.rs` | `EditProposal`, file results, errors, diagnostics, spans, and change records |

The pure engine has no filesystem dependency. Its main Rust API uses borrowed inputs,
owned results, and typed errors:

```rust
pub fn inspect_source(
    source: &str,
    options: &InspectOptions,
) -> Result<InspectResult, SourceError>;
pub fn propose_edit(
    source: &str,
    request: &EditRequest,
) -> Result<EditProposal, SourceError>;
pub fn check_source(source: &str) -> Result<CheckReport, SourceError>;
pub fn form_to_source(form: &Value) -> Result<String, SourceError>;
pub fn forms_to_source(forms: &[Value]) -> Result<String, SourceError>;
```

`file.rs` adds `inspect_file`, `edit_file`, and `check_file`. These functions compose
the pure engine with `Sandbox` and filesystem operations and return
`FileInspectResult`, `FileEditResult`, or `CheckReport`. Both the stdlib and MCP
wrappers call these file functions.
`SourceError` is a `thiserror` enum
with structured fields and a stable `SourceErrorKind`; no public engine function uses
`Result<T, String>`. The wrappers translate each error into the common failure schema.
The pure modules and pure Sema APIs compile on native targets and `wasm32`. `file.rs`,
the two `source/*` file APIs, and all host filesystem imports use
`#[cfg(not(target_arch = "wasm32"))]`, consistent with `sema/check-file`.

## Form path contracts

### Representation

A `FormPath` is a non-empty vector of non-negative integers. Sema uses a vector, such
as `[2 1 0]`. MCP uses a JSON array, such as `[2, 1, 0]`.

The first segment selects a top-level form. Every later segment selects a semantic
child:

- list: item index;
- vector: item index;
- map: a flattened canonical sequence in `BTreeMap` key order, where `2 * n`
  selects entry `n`'s key and `2 * n + 1` selects its value;
- atom: has no children.

Examples:

```sema
;; Source forms: (define x {:a [1 2]}) (+ x 1)
[0]          ; the define form
[0 2]        ; the map
[0 2 1]      ; value for the first canonical map entry
[0 2 1 0]    ; integer 1
[1 0]        ; symbol +
```

A `SubformPath` uses the same child rules but is relative to one form. Its empty path
`[]` selects that form. `form/walk` uses `SubformPath` because it receives one form.
All file inspection, query, and edit APIs use `FormPath`. The two Rust types are
distinct newtypes so an API cannot accept the wrong path kind by accident.

Map ordering is independent of source insertion order because Sema maps are
`BTreeMap<Value, Value>` data after reading. Replacing a map key must reject a result
that would collide with an existing key. Duplicate equal keys in input maps are
reported during inspection and cannot be addressed independently because `read/all`
already collapses them. An exact path into such a map returns `duplicate-map-key`. A
query whose search boundary contains such a map returns the same error because a
semantic search could otherwise omit a source occurrence. An edit outside that map can
continue. The error reports the equal key and every source range that declared it.
Its `source-state` field is `before` for duplicate input keys and `after` for a collision
created by an edit.
Inspection retains parse status `valid` and adds one warning diagnostic with code
`duplicate-map-key` and all declaration ranges.

### Stability

A path is stable only for the content hash returned by the inspection that produced
it. Inserting or removing an earlier sequence item or changing map key order can change
later paths. Every file inspection therefore returns `content-hash`, and a real write
requires that content hash unless the caller explicitly sets
`allow-write-without-content-hash`.

### Errors

An invalid path reports:

- error kind `invalid-path`;
- `requested-path`: the complete rejected path;
- `resolved-prefix`: the longest prefix that resolved;
- `value-type` and `child-count`: the type and child count at that prefix.

The MCP names are `requested_path`, `resolved_prefix`, `value_type`, and
`child_count`.

The engine rejects an empty `FormPath`, negative segments, non-integer segments, paths
deeper than the configured limit, and paths that enter an atom. An empty
`SubformPath` is valid.

## Source origins and preservation

### Reader change

Add `read_many_with_origins` to `sema-reader`. It returns the same forms as
`read_many`, plus one origin tree per top-level form. Each origin node mirrors the
semantic child order defined above and records:

```rust
pub struct FormOrigin {
    pub exact_range: Option<Range<usize>>,
    pub rewrite_range: Range<usize>,
    pub span: Span,
    pub kind: OriginKind,
    pub children: Vec<FormOrigin>,
}

pub enum OriginKind {
    Exact,
    Synthetic,
}
```

- `exact_range` identifies source bytes that represent exactly that form.
- `rewrite_range` identifies the smallest exact source form that can represent a
  change to the form.
- `Exact` requires `exact_range: Some`; `Synthetic` requires `exact_range: None` and is
  used for forms introduced by desugaring that have no independent source range.
- Byte ranges are half-open UTF-8 byte ranges. Line and column values remain
  one-based Sema `Span` values. A column counts Unicode scalar values, and a tab counts
  as one column, matching the lexer. MCP conversion uses explicit fields and does not
  use LSP's zero-based UTF-16 positions.

The parser must build map origins in the same canonical key order as the final
`BTreeMap`. It must keep enough temporary entry data to associate each key and value
with its original range before it inserts them into the map.

### Reader sugar

Origin behavior is explicit:

| Source | Semantic form | Origin behavior |
|---|---|---|
| `'x` | `(quote x)` | The list covers `'x`; synthetic `quote` rewrites the whole expression; `x` keeps its exact token range. |
| `` `x ``, `,x`, `,@x`, `@x` | corresponding desugared list | Same rule as quote. |
| `#(+ % 1)` | generated `lambda` form | The outer form covers the complete short lambda. Generated `lambda` and parameter forms are synthetic and use the outer rewrite range; source body forms retain exact ranges. |
| `f"x=${x}"` | generated `__vm-str` call | The outer form covers the complete f-string. Generated head and static-segment children use the outer rewrite range; interpolated source forms retain exact ranges. |
| `#"\\d+"` | string value | The value has the exact regex token range. An unchanged form keeps regex spelling; an edited value renders as a normal string. |
| comments and blank lines | no `Value` | They have no form path and remain in untouched byte ranges. |
| shebang | no `Value` | It remains before the first top-level form. |

Tests must prove that `read_many_with_origins(...).forms` is structurally equal to
`read_many(...)` for the reader corpus.

### Lowering semantic changes to text edits

Operations first produce semantic changes and touched paths. The source layer lowers
them to sorted, non-overlapping byte edits:

1. An exact atom replacement replaces only its token.
2. An exact compound replacement replaces that compound range with canonical source.
3. A change to a synthetic form replaces its `rewrite_range` with canonical source for
   the changed enclosing form.
4. `insert-definition` inserts at a top-level boundary and leaves all existing bytes
   unchanged.
5. `wrap-form` inserts canonical wrapper text before and after the exact target when
   possible, so the original target bytes remain unchanged.
6. `append-to-list` inserts before the target list's closing delimiter. It does not
   re-render existing children.
7. Symbol and binding operations replace exact symbol tokens. If one occurrence is
   synthetic, all changes in the same rewrite range are combined and that range is
   rendered once.
8. Overlapping edits are either combined by rendering the smallest enclosing rewrite
   range once or rejected as `overlapping-edits`. The engine never applies overlapping
   ranges in an arbitrary order.
9. Apply edits from the highest byte offset to the lowest so earlier offsets remain
   valid.

For canonical multi-line replacement text, each line after the first receives the
target line's exact space/tab prefix when every byte before the target is whitespace.
For a target after another form on the same line, it receives `col - 1` ASCII spaces.
The final full source is validated, so indentation logic cannot hide malformed output.

Strict `form->source` and `forms->source` output uses LF. A source document records its
newline style. File edits convert new canonical line endings to LF or CRLF to match a
uniform existing file; `none` uses LF. For `mixed`, an edit that introduces a line
ending returns `ambiguous-newline-style`. An exact single-line replacement can proceed
because it does not choose a newline style. The engine never normalizes untouched line
endings.

This design preserves comments inside an unchanged target. Replacing a complete form or
rewriting a synthetic form can remove comments inside that rewrite range. Each change
record reports its render mode and rewrite range so the caller can see this before a
write.

### Trivia attachment and insertion boundaries

The source document computes top-level `InsertionBoundary` records from comment and
newline tokens. It does not expose comments as forms or add comment segments to
`FormPath`:

- a same-line comment after a form is trailing trivia for that form;
- a consecutive block of full-line comments with no blank line before the next form is
  leading trivia for that next form;
- a shebang is document header trivia and always stays first;
- other comment blocks are document trivia and do not attach to a form.

`before-path` inserts before the target's leading trivia. `after-path` inserts after the
target's trailing trivia. Index placement uses the same boundaries. Append inserts
before document trailer trivia, if present. If token layout gives one byte range two
incompatible attachments, inspection marks that boundary ambiguous and
`insert-definition` returns `ambiguous-insertion-point`. The engine never moves or
rewrites existing trivia to make an insertion succeed.

## Strict rendering

### `form->source`

`form->source` accepts one source-representable data value and returns one complete
form without a trailing newline. It must:

1. accept only `nil`, booleans, all reader-supported numbers, strings, symbols,
   keywords, chars, lists, vectors, maps, and bytevectors, recursively;
2. reject every other `Value` type, including hash maps, numeric arrays, records,
   native functions, lambdas, macros, agents, streams, channels, mutable values,
   promises, and handles;
3. reject a symbol or keyword name that cannot be read back as the same type and value;
4. render the value through a dedicated strict serializer, not `Display` fallback;
5. format the serialized source with the default `sema-fmt` options;
6. parse the result as exactly one form and compare it with the input by
   `source_equivalent` before returning it.

`source_equivalent` is recursive structural equality with one deliberate exception:
two floating-point NaN values are equivalent because Sema's normal `=` follows IEEE
rules and reports NaN as unequal to itself. This rule keeps readable `NaN` values in
the supported set without weakening equality for another type.

`format/form` remains for compatibility. It can delegate for supported forms and retain
its current fallback behavior for unsupported values.

### `forms->source`

`forms->source` accepts a list or vector whose elements are top-level forms. It renders
each with the strict serializer, joins non-empty forms with one blank line, and emits
exactly one final newline. An empty collection returns `""`.

For a list without NaN, the public round trip is:

```sema
(= forms (read/all (forms->source forms)))
```

Reader sugar and literal spelling can become canonical source, but the resulting forms
must be `source_equivalent`. For vector input, compare `(vector->list forms)` with the
`read/all` result. Collections that contain NaN use the engine's `source_equivalent`
oracle because normal Sema equality cannot express that comparison. Canonical map
order follows `BTreeMap` order.

## Public Sema APIs

### Pure form APIs

```sema
(form->source form)                         ; -> string
(forms->source forms)                       ; -> string
(form/at-path forms form-path)                   ; -> form
(form/replace-at-path forms form-path replacement) ; -> new forms
(form/walk f form)                          ; -> rewritten form
(form/find forms predicate-or-query)        ; -> match records
```

`form/replace-at-path` is immutable. It preserves the outer list/vector collection
kind. Replacing a map key that creates a duplicate returns an error.

`form/walk` is post-order. It calls `f` with `(form subform-path)` after it rewrites
the children. The return value replaces the form, including `nil`. The root
`SubformPath` is `[]` because this function walks one form, not a source document. All
callback paths refer to positions in the original input, including when a rewritten map
key changes final map order. A callback result that creates a duplicate map key returns
`duplicate-map-key`.

`form/find` walks top-level forms in pre-order and returns:

```sema
({:form-path [0 2] :form (+ x 1)} ...)
```

If the second argument is callable, it receives `(form form-path)`; a truthy return
value selects the form. A query map supports these keys, combined with AND semantics:

```sema
{:type :list
 :head 'map
 :within-path [0 2]}
```

The allowed `type` values are `:list`, `:vector`, `:map`, `:bytevector`, `:symbol`,
`:keyword`, `:string`, `:number`, `:bool`, `:char`, and `:nil`. `symbol` selects an
exact symbol form, `head` selects a non-empty list with that exact head symbol,
`literal` uses structural value equality, and `within-path` limits the traversal as
to that form and its descendants, including the form itself.

Callback calls use `sema_core::call_callback`; they do not implement a small evaluator.
Native function closures must not capture `Value` or `Env`, in accordance with I2.

### File APIs

```sema
(source/inspect path [opts])
(source/edit path operation args [opts])
```

`source/inspect` returns this source document map:

```sema
{:path "/canonical/src/foo.sema"
 :content-hash "sha256:..."
 :parse-status :valid
 :compile-status :valid
 :forms ((define x 1) (+ x 2))
 :root-path [0]
 :max-depth 0
 :nodes [{:form-path [0]
          :type :list
          :head "define"
          :origin-kind :exact
          :exact-range {:byte-start 0 :byte-end 12}
          :rewrite-range {:byte-start 0 :byte-end 12}
          :span {:line 1 :col 1 :end-line 1 :end-col 13}}]
 :nodes-truncated #t
 :diagnostics []}
```

`nodes` is a flat `FormPath` pre-order list. `exact-range` is `nil` for a synthetic
form. `head` is present only for a non-empty list whose first item is a symbol. The
`root-path` and `max-depth` options filter only `nodes`; `forms` always contains all
top-level forms. If `:include-source #t`, the map contains the complete source once at
top level. A node never embeds a source substring; callers use its byte ranges against
that one string. `nodes-truncated` is true when `max-depth` or the returned-node limit
omits a form from the selected subtree.

A parse or compile diagnostic is data, not an inspection transport error. On a parse
error, `parse-status` is `:invalid`, `compile-status` is `:not-run`, `forms` and `nodes`
are empty, and `diagnostics` contains the parse diagnostic. The engine does not expose
partial forms or paths. `inspect_source` returns `SourceError` only for request,
resource, or host failures that prevent an inspection report.

`source/inspect` is the language equivalent of the MCP `inspect_forms` tool.

`source/edit` accepts one operation keyword and returns the file edit result. It is the
language equivalent of the MCP `structured_edit` tool.

Both file APIs require an existing regular UTF-8 file. File creation and edits to
directories, devices, sockets, and other file types are outside this issue.

`source/inspect` options are:

```sema
{:root-path nil
 :max-depth 4
 :include-source #f}
```

A nil `root-path` inspects every top-level form. `max-depth` counts the selected root as
depth zero and limits only returned node data; it does not change parsing
or validation. Inspection always requests the compile phase.

`source/edit` options are:

```sema
{:dry-run #t
 :expected-content-hash nil
 :allow-write-without-content-hash #f
 :validation :compile
 :allow-invalid #f
 :allow-no-change #f
 :include-source #f
 :diff-context 3}
```

If `include-source` is true, an edit result contains `source-before` and `source-after`
once at top level. `diff-context` is a non-negative unified-diff context line count.

Example dry run:

```sema
(source/edit "src/foo.sema"
             :replace-symbol
             {:old-symbol 'x
              :new-symbol 'renamed-x
              :within-path [0]
              :expected-match-count 2}
             {:dry-run #t})
```

A real write uses `:dry-run #f` and requires `:expected-content-hash`, unless
`:allow-write-without-content-hash #t` is explicit. If the caller supplies
`:expected-content-hash`, a mismatch always fails.
`:allow-write-without-content-hash` permits only an absent content hash from
inspection; it does not bypass a stale content hash, sandbox checks, or validation.

## Edit operations

Operation data uses keywords in Sema and kebab-case strings in MCP. MCP replacement
forms are source strings that `parse_one_complete` must parse as exactly one form.
This helper parses the complete string with `read_many`, rejects zero or multiple forms,
and permits leading, trailing, or internal trivia. The engine then renders the parsed
value as canonical source, so Sema `Value` inputs and MCP source-string inputs produce
the same replacement text.

### Common selection rules

An operation that supports exact and query selection uses exactly one of these modes:

- exact mode supplies `target-path` and forbids `within-path` and
  `expected-match-count`;
- query mode omits `target-path`, optionally supplies `within-path`, and searches the
  complete source document or the subtree rooted at `within-path`, including that root.

Query mode changes exactly one match by default. `expected-match-count` is a positive
integer that explicitly permits and requires that exact number of changes. Zero
matches returns `target-not-found`. A different positive count returns
`ambiguous-target`; its error data reports the total count and a bounded list of
candidate paths. Search order is `FormPath` pre-order. Options and operation arguments
that do not apply to the selected mode return `invalid-arguments` instead of being
ignored.

Sema symbol arguments are symbol values, such as `'old-name`. MCP symbol arguments are
strings that must parse as exactly one symbol token. Sema form arguments are `Value`
forms. MCP form arguments are source strings that must parse as exactly one form.

### `replace-form-at-path`

Required arguments: `target-path`, `replacement`.

Replace exactly one semantic form. `target-path` selects the complete target, so there
is no ambiguity search. An equal replacement is a `no-change` failure unless
`allow-no-change` is true.

### `replace-symbol`

Required arguments: `old-symbol`, `new-symbol`. Optional selection arguments are
`target-path`, `within-path`, and `expected-match-count`. `include-quoted` defaults to
false.

- With `target-path`, the target must be one symbol equal to `old-symbol`.
- Without `target-path`, the query searches within `within-path`, if supplied, and must
  find exactly `expected-match-count` occurrences.
- If `expected-match-count` is omitted, exactly one match is required.
- Quoted data is excluded unless requested.
- A quoted region is the body of `quote` or `quasiquote`, except for an active
  `unquote` or `unquote-splicing` region at the matching quasiquote depth.
- This operation matches structural symbol forms but does not resolve bindings. Use
  `rename-binding` for a binding and its references.

### `rename-binding`

Required arguments: `target-path`, `new-symbol`. Optional `expected-symbol` prevents a
stale request.

`target-path` must resolve to a binding site recognized by the shared scope analyzer.
Rename the definition and every unquoted reference that resolves to the same binding
in the same file. Reject:

- an occurrence that is not a binding site;
- unresolved or multiply defined binding data;
- a new name that is not a valid Sema symbol token;
- a capture where the new name already resolves to a different visible binding;
- an unchanged name.

For a top-level binding, set `external-references-unchecked` to true and add a warning.
Only the named file can be written under D3.

Move `ScopeTree`, quote-region filtering, and definition helpers from `sema-lsp` into
`sema-source::scope`. Replace the separate head matches in `scope.rs`, `state.rs`, and
`helpers.rs` with the shared typed classifier described under `insert-definition`.
Extend tests before adding edit behavior. Preserve existing LSP behavior except where
the shared classifier adds a definition form that the LSP currently misses.

### `insert-definition`

Required argument: `definition`. Optional placement is exactly one of
`top-level-index`, `before-path`, or `after-path`; the default is append.

The new form must pass `classify_definition_form`. The classifier returns a
`DefinitionFormKind`, not a boolean head list, because binding positions differ:

- `SingleBinding`: `define`, `def`, `defun`, `defn`, `defmacro`, `define-syntax`,
  `defagent`, `deftool`, `defworkflow`, `defpolicy`, and `defmulti`;
- `MultipleBindings`: `define-values`;
- `RecordBindings`: `define-record-type`;
- `MethodAttachment`: `defmethod`, which refers to an existing multimethod and creates
  no new top-level binding.

The classifier validates each form's binding shape and returns its binding paths.
`insert-definition` accepts all four kinds. Scope-aware rename accepts only returned
binding paths, so a `defmethod` target is not a binding. Placement paths must identify
top-level forms.
`top-level-index` is in `0..=form-count`; `form-count` means append. `before-path` and
`after-path` must contain exactly one segment. Insertion respects a shebang, preserves
all existing bytes, and normalizes only the separator bytes around the new canonical
form. An empty source document accepts only index zero or the default append placement.

### `wrap-form`

Required arguments: `target-path`, `wrapper-head`. Optional `prefix-forms` and
`suffix-forms` are form collections.

It produces `(wrapper-head prefix-forms... target suffix-forms...)`. `wrapper-head`
must be a symbol. When the target has an exact byte range, the engine inserts around
its original bytes. This keeps comments and literal spelling in the target.

### `unwrap-form`

Required arguments: `target-path`, `child-index`.

The target must be a non-empty list or vector. Replace it with the selected direct
child. The child index is always explicit; the engine does not guess which child is the
body.

### `append-to-list`

Required arguments: `target-path`, `new-form`.

The target must be a list. Insert the canonical new form immediately before its closing
parenthesis. Respect the existing one-line or multi-line indentation style. Comments
before the closing parenthesis remain in place; if their position makes insertion
ambiguous, return `ambiguous-insertion-point` rather than moving the comment.

### `replace-literal`

Required arguments: `old-literal`, `new-literal`. Optional selection arguments are
`target-path`, `within-path`, and `expected-match-count`. `allow-type-change` defaults to
false.

The literal kinds are `nil`, `bool`, `number`, `keyword`, `string`, `char`, and
`bytevector`; all reader-supported numeric types have kind `number`. Literal kinds must
match unless `allow-type-change` is true. Symbols and compound forms are not literals
for this operation. Matching uses structural value equality and the common selection
rules.

### `rewrite-call`

Supply either exact `target-path` or query `callee-symbol`. Query mode also accepts
`within-path` and `expected-match-count`. Supply `new-callee-symbol`, a non-empty
ordered `argument-edits` collection, or both; a request with neither returns
`no-change`.

```sema
{:target-path [2 3]
 :new-callee-symbol 'map
 :argument-edits [{:operation :replace :index 0 :new-form f}
                  {:operation :insert :index 1 :new-form xs}
                  {:operation :remove :index 2}]}
```

The target must be a non-empty list whose head is a symbol. Query mode searches within
`within-path`, if supplied, and follows the common selection rules.
Argument indices exclude the callee. Validate all indices against the original call
before applying any edit. A replace or remove index is in `0..argument-count`; an
insert index is in `0..=argument-count`. Apply removals from highest to lowest, then
replacements, then insertions in request order. Reject two replacements, two removals,
or a replace and remove for one original argument. Multiple insertions at one boundary
are allowed and retain request order.

### `forms->source`

This is a conversion, not a file mutation. It is exposed through the language function
and the MCP `forms_to_source` tool. `source/edit` does not accept it as an operation.
The Sema name is always `forms->source`; only the MCP tool identifier uses
`forms_to_source`.

## File edit result and error contract

`source/edit` and MCP `structured_edit` use one file edit result. A successful Sema
result uses kebab-case keyword keys. MCP returns the same data with snake_case JSON
keys. The logical fields are:

```sema
{:ok #t
 :operation :replace-symbol
 :path "/canonical/src/foo.sema"
 :changed #t
 :dry-run #t
 :write-status :not-requested
 :content-hash-before "sha256:..."
 :content-hash-after "sha256:..."
 :parse-status-before :valid
 :compile-status-before :valid
 :parse-status-after :valid
 :compile-status-after :valid
 :diagnostics []
 :diff-preview "--- a/...\n+++ b/...\n..."
 :diff-preview-truncated #f
 :diff-summary {:bytes-before 120
                :bytes-after 128
                :common-prefix-bytes 42
                :common-suffix-bytes 63}
 :forms-touched 2
 :match-count 2
 :changes [{:form-path-before [0 2]
            :form-path-after [0 2]
            :kind :replace-symbol
            :span-before {:line 1 :col 12 :end-line 1 :end-col 13}
            :span-after {:line 1 :col 12 :end-line 1 :end-col 16}
            :render-mode :exact-replacement
            :rewrite-range-before nil}]
 :warnings []}
```

Each parse or compile status is `:valid`, `:invalid`, or `:not-run`. Parsing runs for
every source document. Compilation is `:not-run` when parsing fails or the selected
validation mode is `parse`. MCP uses the same values as strings.
`forms-touched` is the number of distinct top-level forms inserted, removed, or changed.
`match-count` is the number of semantic forms selected by exact or query selection. It
is zero for `insert-definition`, whose placement path is not an edit target. Each
change record identifies the affected form before and after the edit. Either form path
can be `nil` for an insertion or removal. `kind` repeats the top-level operation value;
it does not introduce a second change-name vocabulary. Each change uses one render
mode:

- `exact-replacement`: replace only the target's exact byte range;
- `range-rewrite`: render and replace `rewrite-range` because the target is synthetic
  or several edits must be combined;
- `insertion`: insert at a byte boundary without replacing an existing range.

`path` is the canonical filesystem path. Both content hashes use the format defined in
the terminology table. The after content hash describes the proposed source in a dry
run and the committed source after a write. `changed` states whether the proposed bytes
differ from the original bytes; it does not claim that a write occurred. `write-status`
is one of:

- `not-requested`: the request is a dry run;
- `not-written`: a real-write request did not reach atomic replacement, or an allowed
  no-change result required no replacement;
- `committed`: atomic replacement, directory flush where supported, and readback
  verification succeeded;
- `commit-unverified`: atomic replacement succeeded, but a later durability or readback
  check failed.

If `include-source` is true, the result also includes `source-before` and
`source-after` once at top level.

`diff-summary` is always present and takes linear time. Its common suffix excludes the
common prefix, so the two counts never overlap. `diff-preview` is a unified diff with no
timestamps. It is `nil` when full diff computation exceeds its input limit. If only the
returned preview exceeds its output limit, truncate at a UTF-8 boundary, append a fixed
`\n... diff preview truncated ...\n` marker, and set `diff-preview-truncated` to true.

`rewrite-range-before` is `nil` unless the render mode is `range-rewrite`. It always
uses offsets in the source before the edit. A non-null byte range is
`{:byte-start n :byte-end n}` in Sema and
`{"byte_start": n, "byte_end": n}` in MCP. Both offsets define one half-open UTF-8
byte range.

Every diagnostic uses one schema:

```sema
{:source-state :after
 :phase :compile
 :level :error
 :code "stable-code"
 :message "human-readable text"
 :span {:line 1 :col 1 :end-line 1 :end-col 4}}
```

`source-state` is `:before`, `:after`, or `:input`; `phase` is `:parse` or
`:compile`; and `level` is `:error` or `:warning`. `span` is `nil` when no source span
is available. `related-spans` is omitted when empty and uses the same span schema.
`check_source` uses `:input`; edit results use `:before` or `:after`. A
`rename-binding` result also has `external-references-unchecked`, which is false for a
local binding and true for a top-level binding under D3. When true, `warnings` contains
a stable warning code and message.

Failure uses `ok false` and an error map:

```sema
{:ok #f
 :operation :replace-symbol
 :path "/canonical/src/foo.sema"
 :changed #f
 :dry-run #t
 :write-status :not-requested
 :error {:kind :ambiguous-target
         :message "expected one target, found 3"
         :match-count 3
         :candidate-paths [[0 2] [1 3] [4 1]]}}
```

An ambiguity error uses `candidate-paths` for the bounded list of matching
`FormPath` values and `match-count` for the total count. It never overloads one field
as both a count and a collection.

Every failure contains `ok`, `operation`, `path` when path resolution succeeded,
`changed`, `write-status`, and `error`. It also retains every result field computed
before the failure. For example, an after-validation failure has the proposed content
hash, statuses, diagnostics, change records, and diff preview, with `changed true` and
`write-status :not-written`. A failure before an edit proposal has `changed false` and
omits after-only fields. A `commit-verification-failed` result has
`write-status :commit-unverified` and tells the caller to inspect the file before any
retry.

Required error kinds include:

- `invalid-arguments`, `invalid-path`, `invalid-index`, `unsupported-form`,
  `invalid-symbol`;
- `parse-error-before`, `parse-error-after`, `compile-error-after`;
- `ambiguous-target`, `target-not-found`, `stale-content`, `no-change`;
- `target-type-mismatch`, `expected-value-mismatch`, `not-definition-form`;
- `not-a-binding`, `rename-capture`, `external-scope-not-allowed`;
- `duplicate-map-key`, `conflicting-edits`, `overlapping-edits`,
  `ambiguous-insertion-point`, `ambiguous-newline-style`;
- `permission-denied`, `path-denied`, `symlink-escape`, `not-regular-file`,
  `invalid-utf8`, `file-too-large`, `resource-limit`;
- `read-failed`, `write-failed`, `atomic-replace-failed`,
  `commit-verification-failed`.

`invalid-arguments` is reserved for request shape, missing fields, unknown fields, and
mutually exclusive options. `target-type-mismatch` means a valid path selected the
wrong form type. `expected-value-mismatch` means an exact target did not equal an
explicit old or expected value. `invalid-index` reports the index and valid half-open or
closed interval. `conflicting-edits` applies to incompatible semantic edits in one
request; `overlapping-edits` applies only after valid semantic changes lower to byte
ranges.
`stale-content` reports `expected-content-hash`, `actual-content-hash`, and stage
`initial-read` or `pre-commit`.

The Rust engine returns typed `SourceError` values. Sema wrappers convert expected
domain and validation errors into the structured failure map. Sema builtin arity
errors, invalid option types, and callback failures retain normal `SemaError` behavior.
The MCP wrapper converts every `SourceError` into the same failure object and sets
`isError: true` whenever `ok` is false.

A `no-change` result is an error by default. If `allow-no-change` is true, it returns
`ok true`, `changed false`, `write-status :not-requested` for a dry run or
`:not-written` for a real-write request, equal content hashes, and an empty diff.

## Validation

Replace the private all-in-one check logic in `reflect.rs` with a shared checker that
returns `sema_source::CheckReport`:

1. parse all top-level forms;
2. if the requested maximum phase is `compile` and parsing succeeds, compile the forms
   with `sema_vm::compile_program`;
3. keep parse and compile status separate;
4. retain structured diagnostic codes, messages, and spans.

The expanded report's logical fields are parse status, compile status, diagnostics,
and the canonical filesystem path in file mode. Rust uses `parse_status` and
`compile_status`; MCP uses the same snake_case names. In-memory mode omits `path`. `ok`
is a compatibility projection used only by `sema/check-string` and
`sema/check-file`; it is true exactly when compile status is `valid`.

`sema/check-string` and `sema/check-file` retain their current public
`{:ok :diagnostics}` contract and delegate with maximum phase `compile`.
`check_source` and the MCP `check_source` tool also request `compile` and return the
expanded report.
The compatibility projection for the two Sema functions keeps the current diagnostic
keys (`level`, `code`, `message`, and optional `span`) and does not add `source-state`
or `phase`.

`validation` accepts:

- `parse`: require only a successful full parse;
- `compile` (default): require the equivalent of `sema/check-string`.

The engine always computes the selected check. `allow-invalid` is false by default. If
the selected check for the proposed source fails, the operation returns a validation
error unless
`allow-invalid` is true. The override does not skip the check: the result still reports
the invalid status and diagnostics. A malformed original source cannot be edited by a
semantic operation because paths cannot be built, even when `allow-invalid` is true.
A compile-invalid original source can be edited so the tool can repair it; only the
selected validation status after the edit gates success.

## Safe file transaction

The file layer performs these steps in order:

1. Check `FS_READ` and the allowed path before reading.
2. Canonicalize the existing target. Reject a canonical path outside the allowed roots.
   If the user path is a symlink to an allowed target, operate on the canonical target
   so an atomic replacement does not replace the symlink itself.
3. Acquire a process-local transaction lock keyed by the canonical path. Hold it until
   the dry-run result or committed write is complete. Remove unused lock entries so
   requests for many paths do not grow the registry without a bound.
4. Require a regular file and check its metadata length. Read at most the configured
   byte limit plus one byte so concurrent growth cannot cause an unbounded allocation.
   Reject oversized or non-UTF-8 input. Record a platform `FileIdentity` from the open
   file metadata.
5. Compute `sha256:` plus 64 lowercase hexadecimal digits over the exact original
   bytes.
6. If `expected-content-hash` is present, compare it. A mismatch returns `stale-content`
   before edit work and cannot be overridden. If it is absent, a real write requires
   `allow-write-without-content-hash`.
7. Build the edit, validate it, and generate the preview entirely in memory.
8. Return here for dry-run.
9. Check `FS_WRITE` and the canonical allowed path.
10. Resolve and check the user path again. Require the same canonical path, a regular
    file, and the same `FileIdentity`. Re-read with the bounded helper and recompute the
    content hash immediately before commit. Return `stale-content` if identity or
    content changed.
11. Create a new file in the same directory with exclusive creation and restrictive
    initial permissions. Apply the target's `std::fs::Permissions` before writing the
    proposed bytes. Flush the file before replacement.
12. Atomically replace the canonical target. Use `rename` on Unix and a replacement API
    with equivalent existing-target behavior on Windows. Do not implement Windows by
    deleting the target first.
13. Flush the parent directory where the platform supports it, read back the file, and
    confirm the final content hash.
14. Remove an uncommitted temporary file on every error path.

Restrictive initial permissions mean mode `0o600` on Unix and the parent directory's
inherited ACL on Windows. The metadata non-goals above still apply after replacement.

The keyed lock serializes transactions for one canonical path within the Sema process.
There remains a check-to-replacement race with an unrelated process, as there is for
the existing sandbox path check. Document that cross-process limit. `FileIdentity` is
device and inode on Unix and volume serial plus file index on Windows.

`source/edit` must not block the cooperative VM runtime on file I/O or unified-diff CPU.
The stdlib wrapper converts request data to owned Rust strings, runs the bounded file
transaction through the existing quarantined worker pattern, and converts the owned
result back to `Value` on the VM thread. `source/inspect` can read on a worker and parse
the returned source on the VM thread because `Value` is not `Send`.

## Resource limits

Use named constants and test overrides. Initial limits should be measured and then set
high enough for real Sema files:

- source bytes per operation: proposed 16 MiB;
- replacement source bytes: proposed 1 MiB;
- semantic forms: proposed 200,000;
- form path depth: proposed 256;
- returned inspection nodes: proposed 10,000;
- returned matches: proposed 10,000;
- full diff computation: proposed 4 MiB per side;
- returned diff preview: proposed 256 KiB.

Exceeding a hard parsing, traversal, path, replacement, or match limit returns
`resource-limit` with `resource`, `limit`, and `observed-at-least` fields. The source
byte limit retains the more specific `file-too-large` error. The returned inspection
node limit truncates `nodes` and sets `nodes-truncated`; it does not fail inspection.
Preview truncation is not an operation failure and uses `diff-preview-truncated`.

If a changed file is too large for full diff computation, return `diff-preview nil`,
the linear `diff-summary`, and `diff-preview-truncated true`; do not run an unbounded
super-linear diff. Parsing still uses the reader's nesting limit. Traversal must use an
explicit stack or the existing stack-growth helper so adversarial nesting cannot
overflow the process stack.

## MCP tools

Add four default tools to `crates/sema-mcp/src/tools.rs`.

### `inspect_forms`

Input:

```json
{
  "path": "src/foo.sema",
  "root_path": [0],
  "max_depth": 4,
  "include_source": true
}
```

Only `path` is required. The result includes the canonical path, content hash, parse
and compile status, diagnostics, and a bounded flat `nodes` list. Each node uses the
snake_case equivalents of `form-path`, `type`, `head`, `origin-kind`, `exact-range`,
`rewrite-range`, and `span` from `source/inspect`. MCP omits the raw `forms` field because
JSON has no unambiguous encoding for every Sema value. The result also echoes
`root_path` and `max_depth` and reports `nodes_truncated`. If `include_source` is true,
the result includes the complete `source` string once. It never writes.
Invalid source returns a normal inspection result with invalid status and diagnostics;
it does not set `isError` unless the request, resource, or host operation fails.

### `structured_edit`

Input:

```json
{
  "path": "src/foo.sema",
  "operation": "replace-form-at-path",
  "args": {
    "target_path": [0, 2, 1],
    "replacement": "(+ x 1)"
  },
  "dry_run": true,
  "expected_content_hash": "sha256:...",
  "validation": "compile",
  "allow_invalid": false,
  "allow_no_change": false,
  "allow_write_without_content_hash": false
}
```

The input schema uses `oneOf`: each branch has one constant `operation` value and its
corresponding closed `args` schema. Rust performs the same validation because clients
can bypass schema validation. Unknown, missing, or mode-incompatible fields return
`invalid-arguments` with the expected field names. A real write requires
`dry_run: false` and the content hash returned by inspection, or explicit
`allow_write_without_content_hash: true`. A supplied but stale content hash always
fails.

### `check_source`

Accept exactly one of `code` or `path`. Path mode checks `FS_READ` and allowed paths.
Return separate parse and compile status plus diagnostics. This tool maps to
`sema/check-string` or `sema/check-file` but returns the expanded structured report.
Invalid source is a successful tool call with invalid status; request and host failures
set `isError`.

### `forms_to_source`

Input uses an array of strings, where `parse_one_complete` must accept each string:

```json
{"forms": ["(define x 1)", "(+ x 2)"]}
```

This avoids an ambiguous JSON encoding for Sema symbols, keywords, lists, and maps. The
result contains `source` and `form_count`. Its description states that it maps to the
Sema `forms->source` API.

## MCP protocol result support

MCP `2025-11-25` supports a tool `outputSchema` and a tool result
`structuredContent` in the
[official tools specification](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2025-11-25/server/tools.mdx).
Add optional fields to the protocol types:

```rust
pub struct Tool {
    // existing fields
    #[serde(rename = "outputSchema", default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
}

pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    #[serde(
        rename = "structuredContent",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub structured_content: Option<serde_json::Value>,
    pub is_error: bool,
}
```

Keep compact JSON text in `content` for old clients and humans. Put the same logical
object in `structuredContent` for clients that negotiated a version that supports it.
The object must match `outputSchema` when present.

The server currently announces `2024-11-05` unconditionally. Add a `ProtocolSession`
per connection. It stores initialization state and the negotiated protocol version and
is passed through both the synchronous and asynchronous server loops. Support
`2024-11-05` and `2025-11-25`:

- a client that requests `2025-11-25` receives it, tool output schemas, and structured
  content;
- a legacy `2024-11-05` client receives the legacy tool descriptions and JSON text
  content without the newer fields;
- the server rejects an unsupported version through initialization instead of claiming
  a version it does not implement.

Do not make issue #77 depend on adopting the newer `2026-07-28` result-type changes.
That protocol upgrade needs its own compatibility review.

All four tools participate in the existing `--include` and `--exclude` filters.

## Sandbox and path rules

The new tools call a shared host-path guard before each file operation. They must not
copy the direct `std::fs` pattern used by older default MCP tools.
As part of the shared-check refactor, `sema/check-file` moves from
`register_fn_gated` to the path-gated worker wrapper and starts enforcing allowed
paths. Its capability requirement and result shape do not change.

| Tool or API | Required capability |
|---|---|
| `form->source`, `forms->source`, `form/*`, in-memory `check_source` | none |
| `source/inspect`, `inspect_forms`, path-mode `check_source` | `FS_READ` |
| dry-run `source/edit` or `structured_edit` | `FS_READ` |
| real `source/edit` or `structured_edit` | `FS_READ` and `FS_WRITE` |

For every path mode:

- call `Sandbox::check` for each required capability;
- call `Sandbox::check_path` on the user path before the first access;
- canonicalize and check the resolved existing target again;
- reject `..` and symlink targets that resolve outside allowed roots;
- never broaden access because `allow-write-without-content-hash`, `allow-invalid`, or a
  dry-run flag is present;
- include only the canonical allowed path in result metadata.

Tests must cover an allowed symlink target, a symlink escape, `../` traversal, relative
paths, absolute paths, separate read/write denial, and a path changed between inspect
and write.

## Implementation sequence

### Phase 0: approve contracts

1. Record the approved D1-D3 designs in [Decision status](#decision-status). Complete.
2. Confirm the `FormPath` and `SubformPath` examples, newline contract, no-change rule,
   and real-write content hash rule.
3. Add this plan's result and operation examples as test fixtures before implementation.

Checkpoint: D1-D3 are recorded at the top of this document. Complete the remaining
contract fixtures before the first Rust change.

### Phase 1: add `sema-source` and origin data

1. Add the crate to workspace members, default members, workspace dependencies, and
   all internal exact-version dependency declarations.
2. Add `read_many_with_origins` and origin-tree unit tests in `sema-reader`.
3. Prove form equality with `read_many` across the reader and formatter corpora.
4. Implement distinct `FormPath` and `SubformPath` traversal and immutable replacement,
   including map ordering and duplicate-key errors.
5. Move scope analysis and quote filtering from `sema-lsp` to `sema-source`; keep LSP
   tests green before adding rename behavior.

Checkpoint:

```bash
cargo nextest run -p sema-reader
cargo nextest run -p sema-source
cargo nextest run -p sema-lsp
```

### Phase 2: strict rendering and pure APIs

1. Implement the strict source-representable type check and renderer.
2. Add `form->source`, `forms->source`, `form/at-path`, and
   `form/replace-at-path` registrations.
3. Add `form/walk` through `call_callback` and `form/find` predicate/query modes.
4. Delegate supported `format/form` calls without changing unsupported fallback
   compatibility.
5. Add docs index entries for every new builtin.

Checkpoint: round-trip tests cover lists, vectors, maps, map order, strings, chars,
numbers, bytevectors, quote data, f-string desugaring, regex strings, short lambdas,
empty forms, multiple top-level forms, and rejected runtime-only values.

### Phase 3: edit engine and minimum issue operations

1. Implement source inspection, detailed checks, text-edit lowering, content hashes, and
   bounded diff previews.
2. Implement `replace-form-at-path`, `replace-symbol`, and `insert-definition` first.
3. Implement the structured result and error maps.
4. Add the safe file transaction and the `source/inspect` and `source/edit` wrappers.
5. Add dry-run, no-change, stale-content, validation, render-mode, and rewrite-range
   tests.

Checkpoint: a Sema integration test performs the exact issue acceptance flow over a
temporary multi-form file and passes `sema/check-string` after the write.

### Phase 4: complete the operation set

1. Implement `wrap-form`, `unwrap-form`, `append-to-list`, `replace-literal`, and
   `rewrite-call`.
2. Implement `rename-binding` with capture checks and one-file top-level warnings.
3. Add one success, target-not-found, ambiguous-target, no-change, invalid argument, and validation
   failure test for each operation.
4. Add combined-edit tests that exercise edit coalescing inside one synthetic reader
   form.

Checkpoint: every operation has a typed request, documented Sema example, MCP argument
example, stable result fields, and deterministic failures.

### Phase 5: MCP protocol and tools

1. Add protocol negotiation state and optional output/result fields.
2. Add `inspect_forms`, `structured_edit`, `check_source`, and `forms_to_source` schemas.
3. Use the shared `sema-source` file functions and sandbox guards.
4. Return compact JSON text for legacy clients and matching `structuredContent` for
   `2025-11-25` clients.
5. Add include/exclude, panic containment, schema serialization, old-client, and
   new-client tests.

Checkpoint: in-memory JSON-RPC tests inspect a file, dry-run an edit, apply it with the
returned content hash, inspect it again, and observe the new content hash and validated
source.

### Phase 6: documentation and release checks

1. Expand `website/docs/stdlib/reflect.md` and retitle it “Source inspection and
   editing.” Keep its existing `/docs/stdlib/reflect` URL.
2. Add a structured editing section and tool table to `website/docs/mcp.md`.
3. Update `website/docs/cli.md`, the architecture page, the glossary default-tool list,
   and `crates/sema-docs/entries/`.
4. Add `sema-source` to the explicit patch list in
   `scripts/test-packaged-sema-web.sh` so unreleased workspace package builds resolve it.
5. Confirm `cargo package --workspace --allow-dirty --no-verify` creates a
   `sema-source` crate and that no source edit path reads the checkout at runtime.
6. Add changelog text when the feature is scheduled for release.

Checkpoint:

```bash
jake docs-check
jake scripts.check
jake ci
```

`jake ci` is the final gate because it includes lint, workspace tests, LSP E2E, examples,
bytecode smoke tests, runtime freshness, and the packaged-crate boundary test.

## Test matrix

### Reader and path tests

- exact UTF-8 byte ranges and one-based spans;
- LF, CRLF, no-newline, and mixed-newline classification;
- comments before, inside, and after forms;
- blank lines and shebangs;
- lists, vectors, maps, dotted pairs, bytevectors;
- quote, quasiquote, unquote, unquote splice, and deref prefixes;
- short lambdas, f-strings with interpolation, regex literals;
- canonical map path order and duplicate keys;
- rejected empty `FormPath`, accepted empty `SubformPath`, invalid indices, atom
  descent, excessive depth, and changed paths after insertion.

### Rendering tests

- one form has no trailing newline;
- multiple forms have one blank line between forms and one final newline;
- empty form collection is empty text;
- `source_equivalent` after canonical rendering for list and vector inputs, including
  nested NaN;
- hash maps and other values that parse as a different `Value` type are rejected;
- unsupported runtime values fail instead of emitting unreadable text;
- formatter idempotence remains green.

### Operation tests

- unique selection and explicit expected match count;
- ambiguity, missing target, stale expected value, and no-change;
- quoted-data exclusion and opt-in inclusion;
- exact token edits preserve nearby comments and literal spelling;
- inserted and rewritten text follows LF or CRLF, while a mixed-style edit that needs a
  line ending returns `ambiguous-newline-style`;
- multi-line replacement indentation with a leading tab and with a target after another
  form on the same line;
- synthetic-form edits report `range-rewrite` and the exact rewrite range;
- map key collision;
- binding shadowing, destructuring, named let, `let*`, `letrec`, match patterns,
  function parameters, catch bindings, and rename capture;
- every `DefinitionFormKind`, including multiple record bindings and rejection of a
  `defmethod` path as a binding;
- top-level rename warning and no other file write;
- call argument edit ordering and conflicts;
- compile-invalid output blocked by default and explicitly allowed only when requested.

### File and sandbox tests

- dry-run never changes mtime or bytes;
- real write requires a matching content hash unless
  `allow-write-without-content-hash` is explicit;
- stale content between inspection and commit;
- two concurrent writes with one expected content hash produce one success and one
  `stale-content` failure;
- original permissions retained;
- injected post-replacement verification failure returns `commit-unverified` and tells
  the caller to inspect before retry;
- failed validation leaves the original bytes intact;
- temporary files removed after failure;
- read-only, write-only, and all-capability denials;
- allowed root, `..` traversal, symlink within root, and symlink outside root;
- Unicode paths and source;
- file size and diff preview limits.

### MCP tests

- all four tool schemas and output schemas;
- old and new protocol negotiation;
- JSON text and structured content represent the same object;
- structured failures set `isError`;
- invalid-source reports from `inspect_forms` and `check_source` do not set `isError`;
- include and exclude filters;
- inspect -> dry run -> write -> inspect sequence;
- no write to a second file;
- sandbox behavior through the real `sema mcp` CLI configuration;
- tool listing and calls through both `sema mcp` and a built standalone executable
  launched with `--mcp`; both modes use `list_mcp_tools`.

## Acceptance traceability

| Issue #77 requirement | Planned proof |
|---|---|
| Parse a full file into all forms | `read_many_with_origins`, `source/inspect`, and `inspect_forms` corpus tests |
| Address forms deterministically by path | documented `FormPath` contract and reader/path unit tests |
| Edit form data instead of arbitrary text | typed operation requests; MCP has no patch-text field |
| Render with `form->source` / `forms->source` | strict renderer and parse-equality tests |
| Validate before write or success | common `CheckReport`; blocked-write integration tests |
| Return metadata about changes | file edit result model and MCP `structuredContent` tests |
| `forms->source` handles multiple forms | newline and round-trip tests |
| At least replace symbol, replace path, insert definition | Phase 3 acceptance integration test |
| MCP dry-run and diff preview | Phase 5 JSON-RPC sequence test |
| Sandbox and allowed paths | capability, traversal, and symlink test matrix |
| Maps, vectors, short lambdas, f-strings, regex, comments, multiple forms | reader, renderer, operation, and MCP fixtures for each named case |
| Ambiguous operations do not guess | default unique-target rule and structured error tests |
| No unrelated file rewrite | single canonical target in the request/result and filesystem assertion over sibling files |

Issue #77 is complete only when every row has direct test evidence and all final gates
pass. Phase 3 meets the issue's minimum operation list, but it is not the complete
planned feature because the remaining named operations and MCP compatibility work still
remain.

## Risks and controls

| Risk | Control |
|---|---|
| Semantic paths do not match reader sugar text | origins identify synthetic forms and report the enclosing rewrite range before a write |
| A map path changes after a key edit | bind paths to content hashes and define canonical `BTreeMap` ordering |
| Scope logic diverges between LSP and structured edits | move one tested implementation to `sema-source` and make both consume it |
| A broad search changes too many sites | unique match by default; explicit `expected-match-count`; include every target path in the preview |
| An unchanged result appears successful | `no-change` is a failure unless explicitly allowed |
| Invalid output reaches disk | full in-memory check before commit and readback content hash after commit |
| External edit is overwritten | expected content hash, a process-local path lock, and an immediate pre-commit re-read |
| Symlink or traversal escapes allowed roots | check user path and canonical target for read and write |
| Diff generation consumes unbounded CPU or output | source caps, bounded computation, preview cap, truncation metadata |
| New crate fails packaged builds before publication | workspace package invocation plus explicit package-script patch-list update |
| MCP structured fields break legacy clients | per-connection protocol negotiation and JSON text fallback |

## Effect of alternate decisions

### If D1 selects whole-file canonical rendering

- Remove per-form exact-edit lowering and most origin rewrite metadata.
- `source/edit` applies the semantic operation and writes `forms->source` for the full
  form collection.
- Comments, blank lines, shebang handling, and reader sugar need explicit loss tests and
  user documentation.
- The issue's comment coverage can prove documented loss, but not preservation.

### If D1 selects full concrete-syntax editing

- Promote the formatter's private `Node` tree into a public shared syntax model, retain
  all trivia as nodes, and add stable node identities.
- Define edit semantics for comment attachment, blank-line ownership, prefix sugar,
  delimiter trivia, and literal spelling.
- Replace the origin/text-edit phase with concrete tree edits plus a lossless printer.
- Delay all write operations until parser -> tree -> printer byte equality passes for
  the complete formatter corpus.

### If D2 selects concrete syntax paths

- Define a second syntax-node kind and path order that includes prefix forms but excludes
  or includes trivia explicitly.
- `form/at-path` cannot use the same paths as `inspect_forms`; either rename it or return
  syntax wrapper data instead of plain `Value`.
- MCP inspection must state whether a returned node is syntax or semantic data.

### If D2 exposes both path types

- Add tagged `semantic-path` and `syntax-path` fields in Sema and `semantic_path` and
  `syntax_path` fields in MCP.
- Every operation declares which type it accepts; conversion can fail for synthetic
  nodes and trivia.
- Content hashes remain required because both path types are positional.

### If D3 selects local-only rename

- Reject `ResolvedSymbol::is_top_level` with `external-scope-not-allowed`.
- Remove the top-level warning field and its tests.

### If D3 selects workspace opt-in rename

- Add an explicit root and file allowlist; never infer all workspace files for a write.
- Inspect every file and compute its content hash first, return a multi-file preview,
  and require all content hashes for commit.
- Check every path against the sandbox.
- Stage every output, validate all outputs, then commit all or report partial-commit
  recovery data. Cross-platform all-or-nothing replacement needs a journal because the
  filesystem does not provide a multi-file atomic rename.
- This is a separate implementation phase and result schema, not a small extension to
  the one-file operation.
