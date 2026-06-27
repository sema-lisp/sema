---
outline: [2, 3]
---

# Workflows

Sema's workflow runtime lets you define multi-phase agentic workflows as ordinary
Sema code. Every phase, step call, checkpoint, and budget charge is journaled
to a frozen JSONL run directory. Crash, edit, and resume — the runtime skips
leaves that already completed and only re-runs what changed.

## Quick start

```bash
# Run a workflow file
sema workflow run my-workflow.sema --args '{"topic":"rust"}' --view

# Resume after a crash or edit
sema workflow run my-workflow.sema --resume wf_1719494_12345

# Statically validate without calling any LLM
sema workflow check my-workflow.sema

# Open the web viewer for past runs
sema workflow view --run-dir .sema/runs
```

## The DSL

### `defworkflow`

A prelude macro that expands to `workflow/run`. The body is a thunk (an
implicit `lambda`); the form **is** the run — when Sema evaluates it, the
runtime opens a journal, emits events, and returns a `{:status …}` envelope.

```sema
(defworkflow name "doc string" meta-map
  body-form-1
  body-form-2
  ...)
```

The `meta-map` supports:

| Key | Type | Description |
|-----|------|-------------|
| `:phases` | `[:string …]` | Declared phase plan — the dashboard shows all phases up front |
| `:budget` | `{:tokens N :usd M}` | Spend caps (see [Budget Enforcement](#budget-enforcement)) |
| `:args` | map | Argument schema (informational; the actual args come from `--args`) |

The body is ordinary Sema code. `phase` markers interleave with `def`,
`step`, `checkpoint`, `parallel`, `pipeline`, and any other Sema forms. The
last value is the return envelope — if it's already a `{:status …}` map it
passes through; otherwise the runtime wraps it as `{:status :success :value …}`.

### `phase`

A **marker**, not a wrapper. `(phase "Audit")` closes the previously-open
phase and opens "Audit". Every `step`, `checkpoint`, and `budget` event that
follows attributes to it until the next `(phase …)` or the run end (which
closes the last open phase). Returns `nil`.

```sema
(phase "Inventory")
;; forms here belong to "Inventory"
(checkpoint :files (list "a.php" "b.php"))

(phase "Audit")
;; forms here belong to "Audit"
(define findings (step "Audit each file" {:name "auditor"}))
```

::: tip
`phase` takes exactly one argument — the label. It is NOT a wrapper like
`let` or `when`. A common mistake is `(phase "Audit" (do-stuff))` — the
correct form is `(phase "Audit")` followed by the body forms.
:::

### `step`

A journaled LLM leaf — the workflow's atomic orchestration unit. The `step` macro
wraps `workflow/step` and handles prompt resolution, schema validation, tool
dispatch, and `:agent` routing.

```sema
;; Without schema — returns the completion text
(step "Summarize the changelog.")

;; With schema — returns typed data (validated via llm/extract)
(step "List auth-relevant files."
      {:name "scout"
       :schema [:list :string]})

;; With tools — runs the real tool loop (llm/chat)
(step "Find TODOs in src/"
      {:name "coder"
       :tools [read-file run-command]})

;; With :agent — runs a configured defagent as this step
(step "Review this file" {:agent code-reviewer :schema verdict})
```

The opts map supports:

| Key | Type | Description |
|-----|------|-------------|
| `:name` | `:string` | Role label shown in the dashboard (default `"step"`) |
| `:schema` | schema spec | Typed extraction — the step returns a validated map, not text |
| `:tools` | `[tool …]` | Tool-calling loop — the step runs `llm/chat` with tool dispatch |
| `:agent` | `defagent` | Run a configured `defagent` as this step via `agent/run` |

When `:agent` is present, the defagent owns its own tools and model — inline
`:tools`/`:model` are ignored (the static checker warns if both are given).

The runtime emits `agent.started` before the leaf and `agent.result` after,
plus a per-step `budget` event with token/cost attribution. (The `agent.*`
event names are the frozen journal contract — they predate the `step` rename.)

### `checkpoint`

Records a keyed step value and returns it. With one argument, reads the
previously-stored value back.

```sema
;; Write: store the files list under :files, return it
(checkpoint :files (list "a.php" "b.php"))

;; Read: get the value back (nil if never set)
(let ((files (checkpoint :files)))
  (count files))
```

Checkpoints double as the run-scoped state bag — values stored in one phase
are readable in a later phase. Each checkpoint emits a `checkpoint` event
with an opaque value digest (the value itself is not in the event stream; the
memo sidecar stores it for resume).

### `parallel`

Runs a list of zero-arg thunks concurrently with bounded concurrency (default
8). A **barrier** — waits for all thunks before returning. Results come back
in input order. A thunk that throws yields `nil` in its slot (the batch never
aborts).

```sema
;; Fetch two URLs concurrently
(parallel
  (list (fn () (http/get url-a))
        (fn () (http/get url-b))))

;; Override the concurrency cap
(parallel thunks 4)
```

### `pipeline`

Each item flows through all stage functions independently — **no barrier
between stages**. Item A can be in stage 3 while item B is still in stage 1.
A stage that throws drops that item to `nil` and skips its remaining stages.
Results align to `items` (nils for dropped).

```sema
;; Each file → audit → verify
(pipeline files
  (fn (f) (step (str "Audit " f) {:name "auditor"}))
  (fn (x) (step (str "Verify " (:claim x)) {:name "verifier"})))
```

## The run directory

Every `sema workflow run` creates a run directory under `.sema/runs/<run-id>/`:

```
.sema/runs/wf_1719494_12345/
  events.jsonl              # the system of record (append-only)
  events.resume-1.jsonl     # one per --resume continuation
  memo/                     # per-leaf resume cache
    ck_a1b2c3d4.json        #   content-key → memoized value
    ck_e5f6g7h8.json
  metadata.json             # workflow name, code version, budget
  result.json               # the final {:status …} envelope
```

### Event vocabulary

The event vocabulary is **frozen** — add fields (append-only, all
`Option`/skippable) but never change existing ones. Old runs stay readable
forever.

| Event | Key fields | Description |
|-------|-----------|-------------|
| `run.started` | `workflow`, `run_id`, `code_version`, `args_json`, `phases` | First line of every run |
| `phase.started` | `phase` | A phase opened |
| `phase.ended` | `phase`, `status`, `dur_ms` | A phase closed (paired with `phase.started`) |
| `agent.started` | `agent_id`, `agent_name`, `model` | An agent leaf began |
| `agent.result` | `agent_id`, `status`, `output`, `dur_ms`, `model` | An agent leaf produced a result |
| `agent.tool_call` | `agent_id`, `tool_name`, `args_json` | An agent invoked a tool |
| `checkpoint` | `key`, `content_key`, `value_digest`, `value` | A checkpoint was recorded |
| `budget` | `agent_id`, `input_tokens`, `output_tokens`, `cost_usd`, `budget_limit` | A per-leaf budget observation |
| `run.ended` | `status`, `reason`, `dur_ms` | Last line of every run |

Each event carries a monotonic `seq` (0-based) and a `ts` (RFC3339 UTC
instant). The journal is flushed per event, so a crash mid-run leaves a valid
JSONL prefix.

## Resume

`--resume <run-id>` reuses the run directory and short-circuits any leaf whose
content-key is in the prior run's `memo/` dir. The model is **not called** for
memoized leaves — they replay for free.

### How content keys work

Each leaf's content key is a hash of `(kind, code-version, phase, prompt,
schema)`. Same inputs → same key → memo hit → no re-call. An occurrence
ordinal distinguishes identical-prompt repeats in source order.

### Automatic invalidation

Edit the workflow → the code version changes → content keys change → no memo
hits → full re-run. No guard files to maintain; the invalidation is
automatic.

### Per-leaf granularity

Delete one memo file → that leaf re-runs while others still replay. A missing
memo always re-runs conservatively (never resumes wrong).

### Resume segment

A `--resume` run writes a fresh `events.resume-N.jsonl` segment (not
appended to `events.jsonl`) so each file keeps the frozen invariants (first
line is `run.started`, `seq` monotonic from 0). The viewer merges segments.

### Resume doesn't double-charge

A `--resume` run starts spend at zero. Memoized leaves don't re-call the
model and don't recharge the budget. Only leaves that actually run count
against the cap.

## Budget enforcement

Declare `:budget {:tokens N :usd M}` in the `defworkflow` metadata. The
runtime charges each step leaf and latches a sticky `over_budget` flag when
a cap is exceeded — further step leaves are **refused** and the run ends
`{:status :failed :reason "budget exceeded"}`.

```sema
(defworkflow audit
  "Audit with a 5000-token cap."
  {:phases ["Scan" "Report"]
   :budget {:tokens 5000}}

  (phase "Scan")
  (def a (step "Find files." {}))
  ;; a burns 5200 tokens → cap trips after its Budget event

  (phase "Report")
  (def b (step "Summarize." {}))
  ;; b refused: over_budget latch is sticky

  {:status :success :a a :b b})
;; → {:status :failed :reason "budget exceeded"}
```

- **Token caps are deterministic.** `:tokens N` counts actual usage tokens.
- **USD caps are best-effort.** `:usd M` depends on the pricing table being
  available for the model.
- **Per-leaf attribution.** Each `budget` event records the `agent_id`, token
  counts, and cost — the dashboard shows per-leaf spend.
- **Sticky latch.** Once tripped, the latch stays set for the rest of the run.
  No step leaf launches after it, even under concurrent `parallel` fan-out.

## `sema workflow check`

Statically validate a workflow file **without evaluating it or calling any
LLM**. Catches arity traps, bad options, and layout issues before you spend a
token.

```bash
$ sema workflow check audit.sema
error[WF-PHASE-ARITY]: phase expects exactly 1 argument (a label), got 3
  at line 12, col 3
  hint: phase is a MARKER — use (phase "Audit") then body forms after it

$ sema workflow check audit.sema --strict  # treat warnings as errors
$ sema workflow check audit.sema --json    # machine-readable diagnostics
```

Checks fire **only inside a `defworkflow` body** — a bare `(parallel …)` in
an ordinary library file never trips a workflow-only diagnostic.

## `sema workflow view`

A read-only web viewer that renders the run journal as a live tree. Phases
nest agents; budget events show per-leaf spend; checkpoints show their
digests.

```bash
# Open the viewer for a run directory
sema workflow view --run-dir .sema/runs --port 8899

# Run a workflow and open the viewer simultaneously
sema workflow run my-workflow.sema --view

# Backfill the cross-run SQLite index (for offline/CI use)
sema workflow index --run-dir .sema/runs
```

The viewer is loopback-only by default and has no auth — the same
trusted-local-developer tool model as the notebook server. Binding a
non-loopback host exposes the run directory to the network.

## Macro cookbook

The workflow DSL is homoiconic — agent patterns from the literature are
macros that expand into `parallel`, `pipeline`, and `step` forms. These are
from `examples/workflows/cookbook.sema` — load and use them inside any
`defworkflow` body.

### ReAct

Reason → act (tool) → observe, bounded rounds.

```sema
(defmacro react (question tools max-rounds)
  `(let loop ((round 1) (scratch ""))
     (let ((answer (step (str "Question: " ,question "\n\n"
                               "Reason step-by-step, call a tool when you "
                               "need a fact, then give the final answer.\n"
                               (if (= scratch "") ""
                                 (str "Notes so far:\n" scratch "\n")))
                        {:name "react" :tools ,tools})))
       (if (or (>= round ,max-rounds)
               (not (string/contains? (string/lower answer) "next:")))
         answer
         (loop (+ round 1) (str scratch "\n" answer))))))
```

### Reflexion

Attempt → self-critique → retry with critique, bounded.

```sema
(defmacro reflexion (task max-tries)
  `(let loop ((try 1) (note ""))
     (let ((attempt (step (str ,task
                                (if (= note "") ""
                                  (str "\n\nPrevious critique:\n" note)))
                       {:name "actor"})))
       (if (>= try ,max-tries)
         attempt
         (let ((critique (agent
           (str "Critique this attempt. If it is good, reply exactly "
                "\"OK\". Otherwise list concrete fixes.\n\n" attempt)
           {:name "critic"})))
           (if (string/starts-with? (string/trim critique) "OK")
             attempt
             (loop (+ try 1) critique)))))))
```

### Tree-of-Thought

Fan out N candidates in parallel, score, keep the best.

```sema
(defmacro tree-of-thought (prompt n scorer)
  `(let ((cands (filter (fn (c) (not (nil? c)))
                  (parallel
                    (map (fn (i)
                           (fn () (agent
                             (str ,prompt "\n(Give one distinct candidate, "
                                  "attempt #" i ".)")
                             {:name "thought"})))
                         (range ,n))))))
     (if (null? cands)
       nil
       (foldl (fn (best c)
                (if (> (,scorer c) (,scorer best)) c best))
              (first cands) (rest cands)))))
```

### Debate

Two personas argue R rounds, a judge decides.

```sema
(defmacro debate (topic persona-a persona-b rounds)
  `(let loop ((r 1) (transcript (str "TOPIC: " ,topic)))
     (let* ((arg-a (step (str "You are " ,persona-a ". Argue your side.\n\n"
                               transcript)
                          {:name ,persona-a}))
            (t1 (str transcript "\n\n" ,persona-a ": " arg-a))
            (arg-b (step (str "You are " ,persona-b ". Rebut.\n\n" t1)
                          {:name ,persona-b}))
            (t2 (str t1 "\n\n" ,persona-b ": " arg-b)))
       (if (>= r ,rounds)
         (step (str "You are the judge. Read the debate and declare a "
                     "winner with one sentence of reasoning.\n\n" t2)
                {:name "judge"})
         (loop (+ r 1) t2)))))
```

## Examples

Two complete workflow examples are in `examples/workflows/`:

- **`content-pipeline.sema`** — a four-phase pipeline (Topics → Write →
  Verify → Publish) that generates short explainer articles. Uses `pipeline`
  fan-out with typed `step` leaves and a per-item verification gate.

- **`sema-docs-pipeline.sema`** — a six-phase pipeline (Topics → Draft →
  Review → Revise → Assemble → Publish) with journaled tool calls and a
  fan-in synthesis step. Exercises the full dashboard.

- **`cookbook.sema`** — the agent-pattern macros (ReAct, Reflexion,
  Tree-of-Thought, Debate). Load it, then use the macros inside any
  `defworkflow` body.

Run them:

```bash
export OPENAI_API_KEY=...
sema workflow run examples/workflows/content-pipeline.sema --view
```

## CLI reference

```bash
# Run a workflow file
sema workflow run <file> [--args <json>] [--run-dir <dir>] [--view] [--port <n>] [--resume <run-id>]

# Statically validate a workflow file
sema workflow check <file> [--strict] [--json]

# Backfill the cross-run SQLite index
sema workflow index [--run-dir <dir>]

# Open the web viewer
sema workflow view [--run-dir <dir>] [--host <addr>] [--port <n>]
```

## Internal API

The builtins that back the DSL are registered in `sema-stdlib/src/workflow.rs`.
The macros (`defworkflow`, `phase`, `step`) are in `sema-eval/src/prelude.rs`.
The runtime crate (`sema-workflow`) is a leaf — it depends only on
`sema-core` + `sema-otel` + serde, never on `sema-eval`.

| Builtin | Description |
|---------|-------------|
| `workflow/run` | Open a run scope, journal start/end, return `{:status …}` |
| `workflow/phase` | Marker — close the prior phase, open a new one |
| `workflow/step` | Run a leaf as a journaled step (started/result + budget) |
| `workflow/tool-call` | Journal a tool call by the current agent |
| `checkpoint` | Record or read a keyed step value |
