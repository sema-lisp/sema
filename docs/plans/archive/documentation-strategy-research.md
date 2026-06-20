# Sema LSP Documentation Strategy — Research & Options

> ✅ **ARCHIVED (2026-06-20) — superseded by the `sema-docs` crate.** The fragility
> this research targeted (the LSP re-parsing website markdown via regex) was
> solved by making `crates/sema-docs` the structured source of truth for
> builtin/stdlib docs. The companion plan (`2026-06-09-lsp-followups-and-docs-research.md`)
> is already archived as superseded. Kept for historical context.

**Date:** 2026-06-09 · **Status:** research only (no implementation) · **Scope:** how to maintain and
deliver builtin/stdlib documentation to the Sema LSP more robustly than the current "import the
website markdown" approach, ideally as a single source of truth shared by the website **and** the
LSP.

This is a competitive survey of how 9 language toolchains solve the same problem, followed by a
cross-system comparison and concrete options for Sema. Companion: the parser-bug catalogue in
`docs/plans/2026-06-09-lsp-followups-and-docs-research.md`.

---

## 1. Problem & current state

Today `crates/sema-lsp/src/builtin_docs.rs::build_builtin_docs()` `include_str!`s 22 of the 28
VitePress markdown files under `website/docs/stdlib/` and runs `parse_stdlib_md` — a line-based
regex that only matches headings of the *exact* shape `` ### `name` `` — to recover a `name → doc`
map. An inline table adds special-form docs.

This is the wrong coupling: the LSP re-parses the website's **human-prose markdown source files**
(authored website-first, with structure optimized for reading) using an ad-hoc regex, making the LSP
a second-class consumer that breaks whenever the prose changes. (Note: it reads the markdown *source*
via compile-time `include_str!`, not the rendered HTML — but it's still re-deriving structure from
prose written for a different purpose.) Consequences already found (see companion doc): trailing-text
headings silently drop ~12 list functions; VitePress `:::` containers leak into hover; 5 stdlib files
are unwired; `assoc` collides across two files; a doc is keyed to a non-existent symbol; 7 special
forms ship bare. The fragility is inherent to recovering structure from prose with a regex.

**The question:** what's a more robust, maintainable source-of-truth model — and can one source serve
both the website and the LSP?

---

## 2. The one universal lesson

Across **all nine** systems surveyed, the editor is **never** fed by re-parsing human-prose docs
with an ad-hoc regex. Every one of them feeds the editor from a **structured source authored in or
beside the code** (and the best-in-class ones generate the *website from that same source*). Sema
points the dependency the wrong way: the LSP depends on website-first prose. Universal practice is
the reverse — a machine-first structured source that the website is rendered from.

The winning shape, stated once:

> **One structured doc source (co-located with the API) → consumed identically by the LSP, the REPL,
> and the website generator. Examples are executed in CI so they can't rot. Symbols cross-link by
> name.**

Rust, Elixir, Clojure, Go, and Deno all embody this. PHP/Python/Lua use the *stub-file* variant of
it (because they document a runtime they don't own). Racket is the outlier (separate doc DSL) and is
explicitly *not* recommended for Sema.

---

## 3. The spectrum of approaches

| Model | Exemplars | Where docs live | Authoring cost | Drift risk | Cross-link richness |
|---|---|---|---|---|---|
| **Comment-above-decl** | gopls, Rust `///`, Clojure docstring | In source, on the item | Lowest | Low (co-located) | Medium (Rust/Elixir), low (Go) |
| **In-source attr → compiled artifact** | Elixir `@doc`→EEP-48 chunk | In source → `.beam` chunk | Low | Low (doctests) | Medium (autolink) |
| **Decl/stub files w/ annotations** | TS `.d.ts`+JSDoc, typeshed `.pyi`, LuaLS `meta/*.lua`, phpstorm-stubs `.php` | Separate annotated files | Medium | Medium (needs drift check) | Medium |
| **Separate doc DSL → doc DB** | Racket Scribble | Separate `.scrbl` | **High** | Medium–high | **Highest** |
| **Regex-parse website markdown source (Sema today)** | — | website markdown | n/a | **High (fragile)** | none |

Sema's builtins are Rust functions, so they resemble the "document a foreign runtime" case
(PHP/Python/Lua stubs) — **except Sema owns the Rust source**, so it can attach docs at the
registration site (Rust model) rather than maintaining phantom stub files. User-defined Sema
functions are the pure in-source case (Clojure docstring slot).

---

## 4. Per-system findings (condensed)

Confidence: 🟢 primary source · 🟡 inferred · 🔴 unknown. Full agent reports retained in the research
transcript; the essentials:

### Rust — rustdoc + rust-analyzer 🟢 (the gold standard)
- **Source:** `///`/`//!` markdown doc comments on items (desugar to `#[doc="..."]`).
- **Dual-use:** the *same* comments → rustdoc HTML (docs.rs) **and** rust-analyzer hover, both via the
  HIR — zero re-parsing, can never disagree.
- **Cross-linking:** intra-doc links `` [`Vec`] ``, `[std::collections::HashMap]` resolved against the
  real item graph; `broken_intra_doc_links` lint fails the build on dead links.
- **Sync:** **doctests** — examples are compiled+run by `cargo test --doc`; `missing_docs` lint gates
  coverage.
- **Most relevant detail:** `#[doc = include_str!("docs/foo.md")]` binds an external markdown file to
  *one item* — a structured per-item include, the robust opposite of Sema's whole-file regex parse.

### Elixir — `@doc` / ExDoc / EEP-48 🟢 (best "single source" pipeline)
- **Source:** `@doc`/`@moduledoc` markdown attributes on definitions, with keyword metadata
  (`since:`, `deprecated:`, `group:`).
- **Delivery:** compiler serializes docs into a **Docs chunk in the `.beam`** (EEP-48). IEx `h`,
  ElixirLS hover, and ExDoc *all* read that one chunk via `Code.fetch_docs/1`.
- **Dual-use:** `mix docs` (ExDoc) generates hexdocs.pm from the same `@doc`.
- **Cross-linking:** backtick `` `Mod.fun/arity` `` auto-links (incl. into deps).
- **Sync:** `iex>` examples are doctests run by ExUnit.
- **Most relevant detail:** docs compiled into the artifact alongside code → every consumer queries one
  structured store. Sema's `.semac` already has optional debug sections — a "Docs" section is the
  direct analogue.

### Clojure — docstrings + clojure-lsp + cljdoc 🟢 (closest Lisp analog)
- **Source:** docstring is a first-class slot — `(defn f "doc" [x] ...)` → `:doc`/`:arglists` var
  metadata; `(doc f)` reads it.
- **Delivery:** **clj-kondo** statically extracts `:doc`/`:arglist-strs` (no running REPL) over the
  whole classpath incl. `clojure.core`; clojure-lsp serves it on hover. Same pipeline for stdlib and
  user code.
- **Dual-use:** same docstrings → REPL `doc`, hover, **and** cljdoc.org (rendered as CommonMark).
- **Separation:** canonical docs in-source; **clojuredocs.org** is a *separate, community* layer of
  examples + see-also that editors (Calva) optionally merge into hover.
- **Most relevant detail:** the docstring-slot idiom is exactly what Sema should adopt for
  user-defined functions; the canonical-vs-community split is a clean model for an examples layer.

### Go — gopls 🟢 (zero-ceremony end)
- Plain `//` comments above declarations; the same comments feed gopls hover **and** pkg.go.dev. No
  attributes, no build step — docs re-derived from source on the fly. Hover links out to pkg.go.dev.

### Deno 🟢
- JSDoc in TypeScript `.d.ts` lib files is the single source for both the type-checker/LSP and the
  generated `/api` reference site (built via `deno doc`).

### TypeScript — lib.d.ts + DefinitelyTyped 🟢 (cautionary on dual-use)
- **Source:** JSDoc in `.d.ts` declaration files, bundled with the compiler; `@types/*` community
  packages via the **separate DefinitelyTyped repo**.
- **Robustness:** tsserver parses JSDoc into a structured description + tag list + `@deprecated` flag —
  far better than regex, but still convention-structured prose, not a rigid schema.
- **Cross-linking:** `{@link}`/`{@linkcode}`.
- **Sync:** DefinitelyTyped CI = `dtslint` + `// $ExpectType` tests + `attw` drift check.
- **Cautionary finding:** TS does **not** generate a web reference from `lib.d.ts` — MDN is separate.
  So TS is a model for the *editor* side, not for single-source dual-use. The lesson: invert it — make
  the structured artifact canonical and *generate* the website from it (which TS itself never did).

### Python — typeshed `.pyi` + Pylance/stubtest 🟢 (cautionary on splitting)
- **Types** live in the separate `python/typeshed` repo (`.pyi` stubs) — which **deliberately omit
  docstrings**. **Hover prose** comes from a *different* place: the runtime `__doc__` (Pylance scrapes
  it; basedpyright "docifies" it; Jedi parses source). Website is a *third* artifact (hand-written
  `.rst`/Sphinx).
- **Sync:** `stubtest` introspects the live runtime and diffs against the stubs in CI (+ nightly bot,
  allowlists).
- **Cautionary finding:** splitting types from docs created the docstring-scraping mess everyone
  works around. For a project that owns everything, **keep signature + doc together.** But **steal
  `stubtest`**: Sema can enumerate registered builtins from its own embedded interpreter and assert
  doc coverage — trivially trusted (your own runtime), version-accurate.

### Lua — LuaLS `meta/*.lua` (LuaCATS) 🟢 (closest dynamic-lang analog)
- **Source:** stdlib shipped as **annotated Lua definition files** (`meta/template/*.lua`) in the real
  **LuaCATS** grammar (`---@param`, `---@return`, `---@class`, `@overload`, `@version`, `@deprecated`,
  `@see`) — parsed into structured records, *not* regex-over-prose.
- **Key idea:** **signatures and prose are separated.** Type signatures live in `meta/template`; the
  human descriptions live in **`locale/<lang>/meta.lua`** as a `symbol → string` table
  (`string.format = "..."`), stitched by a `---#DES 'name'` build directive. This also gives
  localization (en/zh/ja/pt) for free.
- **Cross-linking:** `@see`, `[text](lua://Symbol)`, and `$symbol` interpolation inside the locale
  prose.
- **Caveat:** LuaLS does **not** generate its website from `meta/` (manual→meta only). Sema would be
  going *beyond* LuaLS by generating both.
- **Most relevant detail:** the structured-annotation grammar + symbol-keyed prose table is the exact
  robustness Sema's regex lacks; the symbol-keyed table is easy to render to both hover and website.

### PHP — phpstorm-stubs 🟢 (stub-repo + generated-index + ground-truth CI)
- **Source:** a **separate repo** of `.php` files with rich PHPDoc (`@param`/`@return`/`@since`/
  `@deprecated`/`@see`/`@link`), empty bodies. Originally seeded from php.net, now hand-maintained.
- **Delivery:** bundled in the IDE; a **generated `PhpStormStubsMap.php`** maps every symbol → its stub
  file (the LSP consumes the index, never scans prose).
- **Two-tier docs:** concise hover from the stub PHPDoc; **`@link` → "External Documentation" opens
  php.net** for the full reference. (WebStorm similarly falls back JSDoc → MDN.)
- **Sync:** CI validates stubs against **per-version PHP reflection snapshots** — documented signature
  must match real runtime.
- **Most relevant details for Sema:** (1) a **generated symbol→doc index** the LSP loads (vs scanning
  markdown); (2) the **two-tier** concise-hover + link-out pattern; (3) **validate docs against the
  real symbol table** in CI. Skip the executable-stub-files machinery (only needed for static type
  analysis of a foreign runtime).

### Racket — Scribble 🟡 (not recommended; narrow lessons)
- Docs in a **separate `.scrbl` DSL** → `raco setup` renders HTML **and** `blueboxes.rktd` (a
  structured plain-text doc DB the editor reads — never the website). Richest cross-link graph
  (`for-label` binds doc to code), but **highest authoring cost** and **drift risk** (hand-written
  `defproc` contracts) — splits the source of truth, the opposite of the goal.
- **Worth stealing only:** (1) editors read a structured doc *DB*, never the rendered site;
  (2) `scribble/srcdoc`'s principle that one signature spec drives both runtime and docs.

---

## 5. Cross-system comparison (key design questions)

### Q1 — Where does the doc source-of-truth live?
| Approach | Systems | Verdict for Sema |
|---|---|---|
| On the item, in source | Rust, Go, Clojure, Elixir | Best for **user-defined** Sema fns (docstring slot) |
| At the registration site (host lang) | (Rust-style, applied to a registry) | Best for **Rust-implemented builtins** |
| Separate annotated/stub files | TS, Python, Lua, PHP | Viable for builtins if a `doc!` macro feels too heavy |
| Separate doc DSL | Racket | Avoid |
| Scraped website | **Sema today** | Replace |

**Pattern:** co-locate with the API. **Recommendation:** builtins → structured docs at the Rust
registration site (or a per-symbol sidecar file); user fns → docstring slot.

### Q2 — How is it delivered to the LSP?
Bundled-with-the-tool is universal (TS bundles lib.d.ts; PHP/Lua bundle stubs; Elixir reads the beam
chunk). **Recommendation:** keep Sema's `include_str!`/embed approach, but embed a **structured
compiled artifact** (a generated doc registry / serde blob / `.semac` Docs section), not raw markdown.

### Q3 — Can one source serve both website and LSP?
Yes — Rust, Elixir, Clojure, Go, Deno all do it. TS and Lua notably *don't* (their websites are
separate), which is the trap to avoid. **Recommendation:** make the structured doc store canonical and
**generate the VitePress markdown from it** — invert today's flow.

### Q4 — Authoring robustness (vs Sema's regex)?
Everyone uses a **structured schema** parsed into records: LuaCATS grammar, JSDoc tags, PHPDoc tags,
Elixir keyword metadata. **Recommendation:** define a small fixed schema — `summary`, `params[]`,
`returns`, `since`, `deprecated`, `see_also[]`, `examples[]`, `body` (markdown) — and parse once.

### Q5 — Cross-linking & rich content?
Intra-doc links by symbol name are standard: Rust `` [`Vec`] ``, Elixir `` `Mod.fun/arity` ``, cljdoc
`[[wikilink]]`, LuaLS `lua://`/`$symbol`. **Recommendation:** adopt `` [`string/split`] `` →
website-URL (site) + go-to-definition/hover (LSP). Add a broken-link check.

### Q6 — Sync / coverage with the real API?
Two high-leverage ideas: **doctests** (Rust/Elixir/Racket execute examples) and **drift detection**
(Python `stubtest`, PHP reflection CI). **Recommendation:** adopt both — Sema's dual evaluators make
doctests trivial (eval the example, assert result), and its embedded interpreter makes a
"every registered builtin has a doc; every doc maps to a real builtin" CI test trivial. Going further,
making `doc` a **required field** on the `NativeFn` registration gives *compile-time* coverage — stronger than any surveyed system.

### Q7 — Curation model (in-repo vs separate stubs repo)?
Separate stub repos (DefinitelyTyped, typeshed, LuaCATS org) exist only to scale *third-party*
ecosystems. For a small language, **one in-repo source is the win.** Optionally add a Clojure-style
*separate community examples layer* later.

---

## 6. Feature matrix

| Capability | Rust | Elixir | Clojure | Go | TS | Python | Lua | PHP | **Sema today** |
|---|---|---|---|---|---|---|---|---|---|
| Docs co-located with API | ✅ | ✅ | ✅ | ✅ | ⚠️ stubs | ⚠️ split | ⚠️ stubs | ⚠️ stub repo | ❌ website-first md |
| One source → website + LSP | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ⚠️ link-out | ❌ |
| Structured schema (not regex) | ✅ | ✅ | ✅ | ⚠️ | ✅ | ✅ | ✅ | ✅ | ❌ |
| Intra-doc cross-linking | ✅ | ✅ | ✅ (cljdoc) | ⚠️ | ✅ | ✅ (Sphinx) | ✅ | ✅ | ❌ |
| Executable doc examples (doctests) | ✅ | ✅ | ❌ | ❌ | ❌ | ⚠️ | ❌ | ❌ | ❌ |
| Coverage / drift gate | ✅ missing_docs | ⚠️ Doctor | ⚠️ kondo lint | ❌ | ✅ dtslint | ✅ stubtest | ❌ | ✅ reflection | ❌ |
| Two-tier (hover + link-out) | ⚠️ | ⚠️ | ⚠️ | ✅ | ⚠️ MDN | ❌ | ❌ | ✅ | ❌ |
| Bundled with the tool | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ (raw md) |

---

## 7. Recommendations for Sema

### 7.1 Cross-cutting principles (adopt regardless of which option)
1. **Stop deriving LSP docs from website-first markdown.** Make a structured, machine-first doc source
   canonical; *generate* the VitePress markdown from it. (Universal practice; fixes the regex class of
   bugs at the root.)
2. **Structured schema, parsed once** into a `BuiltinDoc { summary, params, returns, since, deprecated,
   see_also, examples, body }` — no hover-time regex.
3. **Bundle a compiled artifact** with the LSP (generated registry / serde blob / `.semac` Docs
   section), not markdown.
4. **Doctests:** execute `examples` via the dual evaluator in CI so docs can't rot.
5. **Coverage gate:** assert registered-builtins == documented-symbols (and special-forms). Ideally
   make `doc` a required registration field for compile-time coverage.
6. **Cross-linking:** `` [`symbol`] `` → URL (web) + symbol (LSP); add a broken-link check.
7. **Two-tier hover:** concise summary + signature inline; "Open full docs ↗" link to the website page
   (phpstorm-stubs/`@link` + WebStorm/MDN model). Keeps hover payloads small.

### 7.2 Options for the builtin doc store (where the source lives)

**Option A — At the Rust registration site (Rust/Elixir model).** Attach a structured doc (inline, or
`include_str!("docs/string-split.md")` per symbol) to each `NativeFn` registration via a `doc!`/builder
field. A build step harvests it into (a) the compiled LSP registry and (b) the website markdown.
- ➕ Strongest: can't drift, compile-time coverage, docs live with the implementation.
- ➖ Authoring rich prose in Rust is awkward (mitigated by per-symbol `include_str!` markdown files);
  needs a harvest macro because registration is imperative.

**Option B — A dedicated structured doc folder (LuaLS/typeshed model).** One file per symbol (or a
data file) keyed by name under a root/crate folder, e.g. `docs/stdlib/string-split.md` with YAML
frontmatter (`params`, `returns`, `since`, `see_also`) + markdown body. Both the LSP build and the
VitePress build read it.
- ➕ Pleasant authoring (markdown + frontmatter), decoupled from both Rust and the rendered site,
  trivially cross-linkable, no macro magic.
- ➖ Separate from code → needs the coverage test (cheap) to prevent drift.

**Option C — Minimal interim (invert the current flow).** Keep the website markdown but formalize it:
one symbol per file with frontmatter, parsed by a real frontmatter+markdown parser (not the regex),
and have the LSP load a **generated index** (phpstorm `PhpStormStubsMap` style) rather than regex-parsing
markdown.
- ➕ Lowest migration; fixes the regex fragility immediately.
- ➖ Still website-centric; doesn't unify with the Rust source.

**Recommendation:** **Option B as the source of truth, harvested into a compiled index for the LSP and
rendered into the website** — it gives single-source dual-use, robust structured parsing, easy
authoring + cross-linking, and avoids burying prose in Rust string literals. Pair it with the coverage
gate (7.1 #5) so it can't drift, which neutralizes Option B's only weakness and gets ~80% of Option A's
guarantee. (Option A is the "purest" if you'd rather docs live in Rust and want compile-time coverage;
Option C is the fast interim if you want to fix the bugs this week without a bigger refactor.)

### 7.3 User-defined function docstrings (Clojure model)
Independent of the builtin store: add a **docstring slot** to `defun`/`define`/`defn`/`defmacro` —
`(defun f "docstring" (args) ...)` — captured by the reader as metadata on the binding. The LSP reads
it from the AST (completion-resolve + hover); a REPL `(doc f)` reads the same. This is the natural Lisp
idiom and resolves the deferred "completion-resolve docstrings" follow-up. (Tracked as a language
feature in `living-code.md` Layer 0; use the after-params position.)

### 7.4 Migration sketch (if Option B is chosen)
1. Define the `BuiltinDoc` schema + a per-symbol file format (frontmatter + markdown body).
2. Migrate the existing 22 stdlib markdown files into per-symbol files (script-assisted), fixing the
   known parser bugs along the way.
3. Build step: generate (a) a compiled doc index `include!`'d by `sema-lsp`, (b) the VitePress stdlib
   pages.
4. Replace `parse_stdlib_md`/`include_str!` with a load of the compiled index.
5. Add the coverage CI test + doctest runner + broken-cross-link check.
6. Add the `defun` docstring slot for user code.

---

## 8. Open questions / decisions for you
- **Source location:** Option A (Rust registration), B (dedicated doc folder), or C (interim website
  formalization)? (Recommend B.)
- **Where to bundle for the LSP:** a generated `include!`'d Rust file vs a `.semac` Docs section — the
  latter matters only if Sema ships precompiled libraries that need docs *without* source present.
- **Localization:** worth the LuaLS-style symbol-keyed indirection now, or defer? (The indirection is
  cheap and useful even mono-lingually.)
- **Community examples layer:** adopt a clojuredocs-style separate examples store later, or keep all
  docs canonical/in-repo?
- **Doctests scope:** which example blocks are executed vs display-only (Rust's `no_run`/`ignore`
  equivalents).

---

## 9. Sources

Primary sources are cited per-system in the research transcript; the load-bearing ones:
- **Rust:** rustdoc book (doc comments, intra-doc links, doctests, `missing_docs`); `#[doc = include_str!]`; rust-analyzer manual.
- **Elixir:** EEP-48 (erlang.org); Elixir "Writing documentation"; ExDoc; ExUnit.DocTest.
- **Clojure:** clojure.org (vars/metadata); clj-kondo analysis README; clojure-lsp features; cljdoc; clojuredocs; Calva clojuredocs integration.
- **Go:** go.dev/gopls/features/passive; gopls v0.16 (linksInHover).
- **Deno:** docs.deno.com (TypeScript/lib files); denoland/docs (deno doc).
- **TypeScript:** TS handbook (JSDoc); TS `lib`/`target`; TypeScript-DOM-lib-generator; DefinitelyTyped + dtslint.
- **Python:** python/typeshed; mypy stubtest; pyright typed-libraries; Argument Clinic (PEP 436); PEP 561.
- **Lua:** LuaLS/lua-language-server `meta/template/*.lua` + `locale/*/meta.lua`; luals.github.io annotations/addons; LuaCATS org; LLS-Addons.
- **PHP:** JetBrains/phpstorm-stubs (`standard_1.php`, `PhpStormStubsMap.php`, reflection-cache CI); JetBrains stubs/quick-doc help pages.
- **Racket:** Scribble guide (`defproc`/`defform`); scribble/srcdoc; scribble/blueboxes.

**Honest gaps:** TS's exact MDN-injection build step and Pylance's reST→Markdown internals are
documented only at a high level; Racket's undocumented-export coverage tooling was not verified;
the serialize-to-`.semac`-vs-in-memory choice for Sema depends on whether precompiled libraries
ever need docs without source present.
