# Workflow Redesign — DSL + Runtime Consolidation

**Date:** 2026-06-24
**Branch:** `feat/dynamic-workflows-spike1`
**Status:** Track #1 **SHIPPED** 2026-06-24 — **Variant C (hybrid)** chosen and
implemented. Defaults locked: typed `agent` reuses the `llm/extract` validator;
`checkpoint` kept but demoted to optional; combinators get bare names
(`parallel`/`pipeline`). Tracks #2–#4 (resume/tools/budgets → SQLite → polish) remain.

**Track #1 acceptance gate — all green (evidence):**
- Both examples live-verified with real `gpt-5.4-mini`: content-pipeline (4 typed
  `agent`s, published 4/4) and sema-docs-pipeline (13 agents, 12 tool calls, 6 phases in
  order, published 4/4 + index).
- `agent` `:schema` returns typed data consumed downstream without parsing
  (content-pipeline Verify/Publish read `(:title)`/`(:body)`; the docs pipeline threads
  typed `{:topic :draft :notes :body}` maps through a 3-stage `pipeline`).
- `parallel` + `pipeline` share one `__fanout-tagged` engine; `workflow/foreach` removed
  (no remaining refs outside history docs).
- `hello-wf` golden **byte-identical** (marker phases emit the same ordering); Playwright
  9/9; full `cargo test --workspace` green (96 binaries, 0 failures); `make lint` clean;
  doc-coverage green.

This is the recalibration doc. We built a lot reactively (runtime + journal + a full
dashboard + two examples) and it works, but the surface sprawled and drifted from the
mental model the user actually wants: **the `workflow.js` orchestrator semantics they
already use in Claude Code**. Before adding runtime depth (resume / tools / budgets) or
the SQLite layer, we stop and lock a clean base.

## 1. What the user decided (the brief)

From three rounds of clarifying questions:

- **Purpose:** "a tool I'll actually use." Resume, real tool-using agents, budgets, and
  durable storage are first-class — not demo dressing.
- **Top gap:** polish what exists **and consolidate the code** — less sprawl, more
  reusability/composition, a well-thought-out codebase — **and redesign the DSL to match
  `workflow.js` semantics**.
- **Sequence (locked):**
  1. **DSL redesign + runtime consolidation** ← *this doc*
  2. Runtime depth: resume, real tool agents, budget enforcement
  3. SQLite + cross-run dashboard ("store everything by default")
  4. Polish: all-phases-upfront, tighten the two examples, drill-in, run ergonomics
- **Process:** design-first. This doc gets signed off before any code. The DSL section
  shows **multiple variants** to choose from.

## 2. Goals / non-goals

**Goals**

- A Sema workflow reads like the `workflow.js` the user writes in Claude Code:
  `phase` / `agent` / `parallel` / `pipeline` / typed agent results.
- One small, composable set of verbs. No two primitives that do almost the same thing.
- `agent` returns **typed data**, not an opaque string — so the next stage can consume it
  without re-parsing. This is the spine of composition.
- The runtime is layered so each layer has one job and is reusable in isolation.
- The frozen journal (`event.rs` wire shape, run-dir layout) is **preserved** — the
  redesign is a surface + plumbing change, not a journal-format break.

**Non-goals (this slice)**

- Resume, real MCP-tool-using agents, budget *enforcement* (the runtime-depth track #2).
  This doc designs the surface so they slot in cleanly, but does not build them.
- SQLite (track #3). The journal stays JSONL; SQLite ingests it later.
- Dashboard rework beyond keeping it green against the preserved journal.

## 3. The `workflow.js` model we're emulating

The Claude Code `Workflow` tool is a deterministic orchestrator. Its shape:

- `phase(title)` — a **marker**. Subsequent `agent()` calls belong to this phase until
  the next `phase()`. Not a wrapper, not control flow.
- `agent(prompt, {schema})` — a leaf. Without a schema it returns text; **with** a schema
  it returns validated typed data. This is what makes stages composable.
- `parallel(thunks)` — **barrier** fan-out: run all, await all, results together. Used
  when stage N needs every stage N-1 result at once (dedup, merge, count).
- `pipeline(items, ...stages)` — **no-barrier** fan-out: each item flows through all
  stages independently; item A can be in stage 3 while item B is still in stage 1. The
  default for staged work.
- `log(msg)` — narrator line.

Two principles we inherit: **deterministic orchestration** (the control flow is plain
code — loops, conditionals, fan-out), and **typed leaves** (agents return data).

### What's already good and stays

- `sema-workflow` crate split (`event` / `journal` / `context`) — clean, keep.
- Frozen `WorkflowEvent` vocab + run-dir layout — keep (the golden + dashboard depend on
  it; it already has the fields we need: `agent_id`, `model`, `phase_seq`, budget, etc.).
- `defworkflow` as a prelude **macro** (no VM change) — keep the approach.
- The viewer (variant-5b three-pane) — keep; it reads the journal.

## 4. DSL variants — same workflow, three ways

The worked example throughout: **audit a repo for missing authorization** —
inventory the auth-relevant files, audit each, verify each finding, report the confirmed
ones. It exercises every verb (typed leaf, fan-out, staged pipeline, final envelope).

Assume two schemas are in scope (defined with the existing schema literal syntax that
`llm/extract` already validates against):

```sema
(def finding {:file :string :claim :string :severity :keyword})
(def verdict {:claim :string :real :bool :why :string})
```

### Variant A — `workflow.js`-faithful (marker phases, lexical `def`)

```sema
(defworkflow audit-auth
  "Audit a repo for missing authorization checks."
  {:phases ["Inventory" "Audit" "Report"]}

  (phase "Inventory")
  (def files
    (agent "List the auth-relevant source files under src/."
           {:name "scout" :schema [:list :string]}))

  (phase "Audit")
  (def findings
    (pipeline files
      (fn (f) (agent (str "Audit " f " for missing authz.")
                     {:name "auditor" :schema finding}))
      (fn (x) (agent (str "Verify this claim: " (:claim x))
                     {:name "verifier" :schema verdict}))))

  (phase "Report")
  {:status :success :confirmed (filter :real findings)})
```

- `phase` is a marker; the body is a flat sequence of top-level forms.
- Values flow through ordinary `def` — exactly like `const files = await agent(...)`.
- Reads almost identically to the Workflow tool. Lowest friction for the user.
- **Resume note:** lexical `def`s are not journaled state. Resume works *only* if the
  runtime memoizes each `agent`/`pipeline` step by content-key (track #2) — which is the
  plan anyway. So "no explicit checkpoint" does not mean "no resume."

### Variant B — Sema-idiomatic, explicit state bag (wrapper phases, `checkpoint`)

```sema
(defworkflow audit-auth
  "Audit a repo for missing authorization checks."
  {:phases ["Inventory" "Audit" "Report"]}

  (phase "Inventory"
    (checkpoint :files
      (agent "List the auth-relevant source files under src/."
             {:name "scout" :schema [:list :string]})))

  (phase "Audit"
    (checkpoint :findings
      (pipeline (state :files)
        (fn (f) (agent (str "Audit " f) {:name "auditor" :schema finding}))
        (fn (x) (agent (str "Verify: " (:claim x)) {:name "verifier" :schema verdict})))))

  (phase "Report"
    {:status :success :confirmed (filter :real (state :findings))}))
```

- `phase` **wraps** its body (current behaviour). Every shared value goes through
  `checkpoint`, read back with `state`.
- State is explicit, journaled, and inspectable — every shared value shows in the
  dashboard for free, and resume has a natural unit (the checkpoint).
- More verbose and *less* like `workflow.js` (the user threads a bag instead of `def`).

### Variant C — hybrid (marker phases + lexical `def`, auto-memoized agents, optional `checkpoint`) — **recommended**

Surface is identical to **A**. The difference is in the runtime contract and one
*optional* affordance:

```sema
(defworkflow audit-auth
  "Audit a repo for missing authorization checks."
  {:phases ["Inventory" "Audit" "Report"]}

  (phase "Inventory")
  (def files
    (agent "List the auth-relevant source files under src/."
           {:name "scout" :schema [:list :string]}))

  (phase "Audit")
  (def findings
    (pipeline files
      (fn (f) (agent (str "Audit " f) {:name "auditor" :schema finding}))
      (fn (x) (agent (str "Verify: " (:claim x)) {:name "verifier" :schema verdict}))))

  (phase "Report")
  ;; checkpoint is OPTIONAL — only when you want to pin a value in the dashboard
  ;; (and give resume an explicit anchor) for something that isn't an agent call.
  (checkpoint :confirmed (filter :real findings)))
```

Runtime contract that makes C work:

- **Every `agent` call is content-key memoized** by `(prompt, schema, name, code-version)`.
  On a resumed run, an agent whose key is already journaled returns its recorded value
  instead of re-calling the model. This is the `workflow.js` resume model and it makes
  lexical `def` fully resumable **without** a state bag — track #2 builds it.
- `checkpoint` survives, but demoted from "the way you share state" to "an explicit pin":
  use it to surface a non-agent computed value in the dashboard or to give resume an
  anchor. It is never *required* to pass data between phases.

| Axis | A (faithful) | B (state bag) | C (hybrid) ★ |
|---|---|---|---|
| Phase | marker | wrapper | marker |
| Value sharing | lexical `def` | `checkpoint`/`state` | lexical `def` |
| Resume mechanism | agent content-key memo | explicit checkpoint | agent memo (+ optional checkpoint anchor) |
| Reads like `workflow.js` | ◎ | △ | ◎ |
| Verbosity | lowest | highest | lowest |
| Dashboard visibility of shared values | only via agent events | every value (checkpointed) | agent events + opt-in pins |
| Resume granularity | per agent step | per checkpoint | per agent step + opt-in anchors |

**Why C:** it gives the user the exact `workflow.js` surface they asked for (so their
mental model transfers 1:1), makes resume automatic at the natural unit (the agent call),
and keeps `checkpoint` as a deliberate dashboard/anchor tool rather than mandatory
plumbing — which also resolves the earlier "why is this an opaque digest" friction: a
`checkpoint` is now something you reach for *when you want the value shown*, and it stores
the real (capped) value. B's explicit bag is strictly more typing for the same power once
agent memoization exists; A is C without the optional pin.

## 5. Consolidated runtime architecture

The sprawl to fix: `workflow/foreach` (prelude) is a near-verbatim copy of
`async/pool-map` — the only difference is error-tagging vs re-raise. And the fan-out
story is split across `workflow/foreach`, `async/pool-map`, `async/map`, `async/spawn-all`
with no `pipeline` at all. We collapse this into a clean layer cake.

```
L4  viewer            crates/sema/src/workflow_view/*        reads the journal
L3  DSL               prelude: defworkflow, phase            surface (Variant C)
L2  workflow builtins sema-stdlib/workflow.rs                workflow/run · phase · agent · checkpoint · tool-call
L1  workflow runtime  sema-workflow: context · journal · event · (new) memo
L0  async combinators prelude: parallel · pipeline           reusable, NOT workflow-specific
```

**L0 — async combinators (general-purpose, reusable outside workflows).**
Introduce two combinators built on the existing scheduler (`async/spawn` + `async/all` +
a bounded semaphore):

- `parallel` — barrier fan-out. Run all thunks, await all, return results in order.
  Failure policy is a parameter (re-raise *or* tag) so it subsumes both
  `async/pool-map` (re-raise) and `workflow/foreach` (tag).
- `pipeline` — no-barrier staged fan-out. Each item flows through all stage fns
  independently; a stage that throws drops that item to a tagged failure and skips its
  rest. This is the new capability the current code lacks.

Both share one bounded-semaphore helper (extracted from the duplicated
`async/pool-map`/`workflow/foreach` bodies). `async/pool-map` / `async/map` /
`async/spawn-all` stay as-is (they're public async API), but **`workflow/foreach` is
deleted** — workflows use `parallel`/`pipeline`.

**L1 — workflow runtime (`sema-workflow`).** Unchanged in shape; gains a **memo store**
(content-key → recorded value/digest) so track #2's resume has a home. `context.rs`
already computes `content_key` and `value_digest` — the memo store is the read side.

**L2 — workflow builtins (`sema-stdlib/workflow.rs`).** The single leaf verb is **`agent`**
(rename of `workflow/agent`), and it now **returns typed data**: when `:schema` is
present it validates the model output through the *existing* `llm/extract` validator (no
new schema engine) and returns the parsed value; without `:schema` it returns text. `phase`,
`checkpoint`, `tool-call`, `workflow/run` stay. No new builtins.

**L3 — DSL (prelude).** `defworkflow` keeps expanding to `workflow/run`. Under Variant C,
`phase` is a marker macro (emits `phase.started`, and the runtime closes the prior phase
when the next marker or `run.ended` fires) — a small change from today's wrapper `phase`.

### Migration map (current → new)

| Current | Fate | Notes |
|---|---|---|
| `workflow/run` | keep | unchanged envelope + journal |
| `workflow/phase` (wrapper) | **becomes marker** | Variant C; or keep wrapper if B is chosen |
| `workflow/agent` | **rename → `agent`, returns typed data** | `:schema` → `llm/extract` validation |
| `checkpoint` / `state` | keep, **demoted to optional** | stores real capped value (already does) |
| `workflow/tool-call` | keep | folds into real tool agents in track #2 |
| `workflow/foreach` (prelude) | **delete** | replaced by `parallel`/`pipeline` |
| `async/pool-map` body | refactor | share the bounded-semaphore helper with `parallel` |
| `WorkflowEvent` vocab | **unchanged** | frozen; golden stays valid |
| run-dir layout | **unchanged** | frozen |

### Examples rewritten

Both examples move to the chosen surface:

- `examples/workflows/content-pipeline.sema` — Topics → Write (`pipeline` over topics) →
  Verify → Publish, agents typed (`{:title :string :body :string}`).
- `examples/workflows/sema-docs-pipeline.sema` — the richer 6-phase demo, same treatment;
  its `workflow/tool-call` rows stay until track #2 gives agents real tools.

The golden fixtures (`hello-wf.events.jsonl`, viewer-runs) only need regeneration if the
chosen variant changes phase event ordering (marker vs wrapper emits `phase.started` at a
different point). That regeneration is part of this slice's acceptance gate.

## 6. How tracks #2–#4 land on this base (forward-looking, not built here)

- **Resume (#2):** the agent content-key memo store (L1) + a `--resume <run-id>` flag that
  re-runs the deterministic body, short-circuiting any `agent`/`checkpoint` whose key is in
  the prior journal. Variant C is designed for exactly this.
- **Real tool agents (#2):** `agent` gains `:tools [...]`, running the existing agent loop
  (`run_tool_loop`) and emitting `agent.tool_call` per real call — replacing the manual
  `workflow/tool-call` rows.
- **Budgets (#2):** `:budget` in `defworkflow` meta, enforced via the per-event `budget`
  events already journaled (`last_usage_snapshot` is wired); exceed → fail the run.
- **SQLite (#3):** an ingester tails `events.jsonl` into SQLite (schema already drafted in
  `2026-06-23-workflow-dashboard-scope.md` §3); the dashboard switches to cross-run queries.
- **Polish (#4):** all-phases-upfront (the stashed `__wf-phases` WIP — `meta :phases` is
  already declared, so the viewer can render pending phases before they start), drill-in,
  run ergonomics.

## 7. Acceptance gate for this slice

Green when:

1. A workflow written in the chosen surface runs end-to-end with real `gpt-5.4-mini` and
   produces a valid journal (run.started … run.ended, status success).
2. `agent` with `:schema` returns typed data a downstream stage consumes without parsing
   (proven by an example whose later stage reads `(:claim x)` directly).
3. `parallel` and `pipeline` exist, share one semaphore helper, and `workflow/foreach` is
   gone — `grep` confirms no remaining references.
4. Both examples rewritten and live-verified; goldens regenerated; Playwright 9/9 green;
   `sema-workflow` unit tests + full `cargo test --workspace` green; `make lint` clean.

## 8. Open questions for sign-off

1. **Which variant — A, B, or C?** (Recommendation: **C**.)
2. **Typed `agent` validation:** reuse the `llm/extract` schema validator (recommended,
   no new surface) vs. a lighter "parse JSON, no validation" mode? Recommendation: reuse.
3. **Keep `checkpoint` at all under A/C?** Recommendation: keep it, demoted to the optional
   dashboard-pin / resume-anchor role (§4 C).
4. **`parallel`/`pipeline` namespace:** bare (`parallel`, `pipeline`, matching
   `workflow.js`) vs. `async/` prefix? Recommendation: bare — they're the workflow vocab,
   and the async/ family stays for low-level use.
