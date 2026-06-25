# MCP `docs_search` tool — design & plan

**Branch:** `feat/mcp-docs-search` (worktree off `origin/main`)
**Date:** 2026-06-25
**Status:** IMPLEMENTED. `crates/sema-mcp/src/docs_search.rs` + tool wired in `tools.rs`;
24 tests pass (15 unit + 9 integration); workspace lint/fmt clean for `sema-mcp`; the
hermetic Docker gate (`make docs-search-gate`) passes — docs_search returns `map`/`json/*`
from the binary alone in a `FROM scratch` image under `--network none`.

**Decisions taken:** structured JSON payload (array of `{name, module, summary, score}`);
synonym table shipped in v1; `website/docs` deferred. `signature_of` (search-time signature
derivation) was **removed** — signatures belong in the corpus, not the search layer.

**Corpus signature backfill (separate, complementary work):** rather than derive signatures
at query time, the canonical doc corpus was enriched. The 100 highest-value entries lacking a
signature (ranked by real call-frequency across all `.sema` code) were backfilled with
`params:`/`syntax:`/`returns:` frontmatter cross-referenced to the Rust/prelude source, via a
multi-agent fan-out (fix → adversarial verify). Result: **97/100** now carry a signature (the
3 without — `e`, `pi`, `&` — are constants/reader-syntax, correctly signature-less), plus 51
neighboring entries swept along. A final read-only adversarial sweep over all 148 edited
entries found 0 high / 1 medium / 4 low issues, all fixed (return-type `any`→`nil`, key type,
variadic min-arity). Whole-corpus signature coverage rose **237 → 384**. These signatures
surface in the `docs` MCP tool and LSP/REPL hover (not in `docs_search`, which ranks on
name/module/summary/body). `cargo run -p sema-docs -- check --strict` passes; index
regenerated.

## Motivation

The RFT experiment (`~/code/sema-rft-experiment/.../results.md`, takeaway #5) found that a
frontier model with `eval_code` + `docs_search` tools scores **60–71%** on Sema tasks —
far above fine-tuning. `sema mcp` already exposes `eval` and `docs` (exact-symbol lookup).
The missing piece is **semantic/relevant search** over the docs.

The experiment's `docs_search` used the Sema prelude with `llm/embed` + `vector-store/search`
+ `llm/rerank` — **all LLM/network-dependent**. Hard requirement for this work: **no LLM,
no network at query time**, and it must work from the **compiled binary alone inside a
`FROM scratch` container** (no repo source, no uncompiled docs).

## Decision: hand-rolled in-memory BM25 (pure Rust)

Backend chosen after a feasibility + **measured** quality spike (13-agent workflow,
68-query oracle derived from the benchmark tasks, run against the real 819-entry corpus).

### Measured retrieval quality (no LLM)

| Approach | recall@1 | recall@5 | MRR |
|---|:-:|:-:|:-:|
| **BM25 (name×2 + module + section + body)** | 0.721 | **0.926** | 0.798 |
| **BM25 + conservative exact-name boost** | 0.706 | **0.926** | **0.803** |
| TF-IDF cosine | 0.706 | 0.882 | 0.785 |
| Local embedding cosine | — | — | — *(no model available; not measured)* |

`recall@5 = 0.93` is the operative number — an LLM MCP client reads the top results.
The 5/68 misses are genuine vocabulary gaps (`match`~satisfy, `let*`~"star" glyph,
`memoize`~cache, "comprehension", f-string interpolation) that **no** lexical engine
bridges; a tiny hand-curated synonym table closes ~4 of them toward recall@5 ≈ 0.98.

### Why hand-rolled BM25 over the alternatives

| Option | Verdict |
|---|---|
| **hand-rolled BM25** ✅ | feasibility 5/5; **zero new deps** for `sema-mcp`; corpus already baked in; nothing on disk → gate-safe by construction; at the measured quality ceiling |
| sqlite-fts5 | rusqlite is a *workspace* dep but **NOT** in `sema-mcp/Cargo.toml` — would add the bundled-SQLite C engine + `!Send/!Sync` Connection + musl link surface, for BM25 (same algorithm) |
| tantivy | wrong weight class: 50+ crates, `zstd-sys` C build (musl risk), mmap dir hostile to scratch; only an in-memory RamDirectory is gate-safe (which discards its advantages) |
| local-embeddings | ONNX/`ort` is a C++ dylib that won't cleanly static-link on musl → **fails the FROM-scratch gate**; needs an on-device query encoder; quality edge unmeasured & likely marginal |
| reuse-builtin-docs (apropos) | explicit floor: name-only matching, no TF/IDF, weak on the descriptive multi-word queries that dominate MCP usage |
| sema-prelude + `llm/embed` (prior experiment) | needs network/LLM → fails the gate outright |

### Verified load-bearing facts

- Corpus baked into every binary: `include_str!("../builtin_docs.generated.json")`,
  `crates/sema-docs/src/lib.rs:105`; deserialized via `sema_docs::builtin_index()`.
- `sema-mcp` deps today: `sema-{core,eval,vm,fmt,docs,notebook}`, `serde`, `serde_json`,
  `tokio`, `uuid`, `crc32fast` — **no** search/SQL/ML crate. BM25 adds **none**.
- Existing `Dockerfile`: `rust:alpine` (musl, static) → `FROM scratch`. The gate fits.
- MCP tool registration: tuple in `list_mcp_tools` (`tools.rs:~565`, after `info`) +
  match arm in `call_mcp_tool_inner` (after the `docs` arm, `tools.rs:~1006`).
  Caching idiom: `OnceLock` (mirrors `BUILTIN_DOCS` at `tools.rs:13`).

### Index lifecycle

In-memory, built **lazily on first `docs_search` call** from the baked-in JSON, stored in a
process-lifetime `static OnceLock<SearchIndex>`. **No** `build.rs`, **no** on-disk artifact,
**no** `$SEMA_HOME` (avoids the documented gotcha that `$HOME`-unset in scratch resolves
`.sema` cwd-relative under `/app`). Build cost ≈ sub-10ms for 819 entries, hidden behind the
first query. The docs *are* the index source → nothing to keep in sync, no stale-index risk.

### Pure Rust, not the prelude

New module `crates/sema-mcp/src/docs_search.rs`. Rationale: dispatch is a Rust match;
`OnceLock<SearchIndex>` caching is a native idiom (Sema would re-tokenize per query or stash
shared state in `global_env`); a Rust struct → `serde_json` result is guaranteed cleanly
serializable (the server emits a degraded `-32603` frame otherwise); BM25 params/tokenizer
edge cases are easier to unit-test deterministically in Rust.

## Implementation plan

1. **`docs_search.rs` skeleton + `SearchIndex`** — inverted index (term→postings, doc
   lengths, avgdl), tokenizer (lowercase; split whitespace/punct; split slash/dash names:
   `string/upper` → `[string, upper, string/upper]`), `static SEARCH_INDEX: OnceLock<…>`
   from `builtin_index()`. Field weights: name×2 + module + section + body. Declare in `lib.rs`.
2. **BM25 scoring + conservative boosts** — k1≈1.2, b≈0.75; exact-full-name token boost
   (+8), module-mention boost (+1.5). **No** aggressive substring/last-segment boosting
   (regressed recall@1: `regex/match` for "match a predicate"). Return top-k (default 5).
3. **Synonym table** *(fan-out)* — small `const` (`match`~satisfy, `star`~`*`,
   `memoize`~cache, `comprehension`~`map`/`filter`, `interpolate`/f-string~`str/format`),
   expand query tokens before scoring.
4. **Tool registration** *(fan-out)* — `docs_search` tuple, schema `{query: string (req),
   limit: integer (opt, default 5)}`. Distinct name from `docs` (which requires `symbol`).
5. **Dispatch arm** — extract `query`/`limit`, call `docs_search::search`, format top-k into
   one text block (name, module, summary, signature), `success_result`.
6. **Ranker unit tests** *(fan-out)* — tokenizer splits namespaced names; pin oracle queries
   ("transform every element"→`map`, "parse json string"→`json/*`, "keep only elements that
   match a predicate"→`filter` via synonym) as top-5 regressions.
7. **MCP integration test + Docker gate** — extend `mcp_test.rs`; add a CI smoke test that
   builds the scratch image and drives the MCP session under `--network none`.
8. **Full local CI-equivalent suite** — `cargo test -p sema-mcp`, `make lint`, workspace
   sweep; confirm no new dep in `sema-mcp/Cargo.toml`.

## Docker acceptance gate (un-fudgeable)

Reuse the existing `rust:alpine`→`FROM scratch` Dockerfile. A scripted test drives
`sema mcp` over stdio JSON-RPC under `--network none` and asserts:

- `tools/list` contains `docs_search` (registration)
- `tools/call docs_search {query: "transform every element of a list"}` →
  `isError == false`, `content[0].type == "text"`, non-empty trimmed text **containing `map`**
- `{query: "parse json string"}` → text contains `json/` and does **not** merely echo the
  query (anti-stub)
- process exits 0 after stdin EOF; whole session under `--network none` + `FROM scratch`

Pitfalls: keep musl/static (glibc base won't run under scratch); index baked, never built
from `entries/*.md` (absent in scratch); JSON-RPC = one object per line on **stdout**,
diagnostics on stderr; correlate by `id`; close stdin to exit.

## Decisions (resolved)

1. **Result payload shape** — structured JSON array `[{name, module, summary, signature,
   score}]`, serialized into one MCP text block. (Chosen for LLM-friendliness.)
2. **Synonym table in v1** — shipped. Small `const SYNONYMS` in `docs_search.rs`.
3. **website/docs (77 narrative files)** — deferred (different size, no frontmatter, params
   tuned to the short structured corpus); revisit as a separate corpus if needed.

## Quality findings (measured post-implementation)

**Keyword-ish queries: strong.** A 12-query battery (sharing some vocabulary with the
docs) put the ideal answer at rank 1 in 11/12 and in the top-3 in 12/12.

**Vague, intent-only queries: mixed — the real limit.** An 18-query battery that names
neither the function nor an obvious keyword: ~8 nailed rank 1, ~4 had the answer in top-3,
but **~6 missed entirely**. All missed targets exist — these are pure vocabulary gaps that
lexical search cannot bridge:
- "do something to each item and collect the results" → missed `map` (docs: "apply a
  function to each element")
- "turn an object into text I can send over the network" → missed `json/encode`
- "save data so it survives after the program exits" → missed `file/write`
- "ask a language model a question" → missed `llm/complete` / `llm/chat`
- "do several slow things at the same time" → missed `async/all`
- "scramble a password" → missed `hash/sha256`
- "repeat an action a fixed number of times" → missed `dotimes` / `for`

Cheap mitigation (aligned with the shipped synonym mechanism): extend the table with
concept→term aliases (save→write/persist, serialize→encode, concurrent/"same time"→async,
scramble/hash→sha256, "each item"→element, "N times"→dotimes/for). Principled fix: a local
(offline) embedding reranker — deferred at design time for the musl/`FROM scratch` gate, and
still the right answer if conceptual recall matters.

**Signature coverage (audit of all 819 entries):** 237 clean signatures from frontmatter
(syntax/params); 32 clean from the body's leading code block (often richer — include return
types, e.g. `(json/encode value) → string`); **~550 entries have no signature** (bytevector/*,
context/*, base64/*, `hash/sha256`, `uuid/v4`, `sleep`, time/*, csv/* …). The body-fallback
initially mis-grabbed a usage *example* as the signature for `&`; `is_signature_block` now
requires head `(name …)` and rejects `=>` result markers, guarded by two tests. The
no-signature majority is a **corpus frontmatter gap** (a sema-docs task), not a search bug —
the payload still carries name + module + summary for those.

## Follow-ups / notes

- Wire `make docs-search-gate` into CI (it needs docker; gate it on a docker-capable runner).
- Pre-existing (not from this work): `cargo clippy -p sema-vm` fails one `single_match` lint
  in `sema-vm` test code under clippy 1.95.0 — fix separately to make `make lint` green.

## Risks

- BM25 params + synonym table are corpus-specific, validated against the 68-query oracle;
  re-validate if `website/docs` is folded in. Keep oracle queries as regression tests.
- We own the tokenizer/scorer (no vetted crate) — pin representative results in unit tests.
- Synonym table is a hand-curated patch, not a general solution.
- Gate must be **tested, not assumed** — the CI smoke test makes it un-fudgeable.

## Spike artifacts

- Oracle: `scratchpad/docs_search_eval.json` (68 queries)
- Lexical eval harness + raw results: `scratchpad/eval_lexical.py`,
  `scratchpad/docs_search_results.json`
