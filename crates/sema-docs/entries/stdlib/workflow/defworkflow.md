---
name: "defworkflow"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(defworkflow name doc meta body ...)"
---

Macro: define and run a sequential, journaled workflow. `(defworkflow name "doc" meta body…)` expands to `(workflow/run "name" "doc" meta (lambda () body…))` — so the form *is* the run: it opens the run directory, journals every event, and returns the `{:status …}` envelope. `meta` is a metadata map (`{:phases … :budget … :args …}`) recorded into `metadata.json`; list `:phases` so the dashboard can show them before they start. A `:budget` submap caps spend — `{:tokens N}` (deterministic) and/or `{:usd N}` (best-effort, pricing-table dependent); exceeding a cap latches the run, refuses to launch further `step` leaves, and ends `{:status :failed :reason "budget exceeded"}`. The body is ordinary Sema code — a flat sequence of forms with `phase` **markers** interleaved, ending in a `{:status …}` map. Shared values flow through ordinary `def`; `step` leaves return typed data that `pipeline`/`parallel` fan out. Keeping `defworkflow` a prelude macro leaves the VM untouched.

```sema
(defworkflow audit-auth
  "Audit a codebase for missing authorization checks."
  {:phases ["Inventory" "Audit" "Report"]}

  (phase "Inventory")
  (def files (step "List auth-relevant files under src/." {:schema [:list :string]}))

  (phase "Audit")
  (def findings
    (pipeline files
      (fn (f) (step (str "Audit " f) {:schema finding}))
      (fn (x) (step (str "Verify " (:claim x)) {:schema verdict}))))

  (phase "Report")
  {:status :success :confirmed (filter (fn (x) (:real x)) findings)})
```

Run a workflow file with `sema workflow run <file> --args <json>`.

See also: `workflow/run`, `phase`, `checkpoint`.
