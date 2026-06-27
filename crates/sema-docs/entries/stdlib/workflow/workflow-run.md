---
name: "workflow/run"
module: "workflow"
section: "Dynamic Workflows"
---

Run a sequential, journaled workflow and return its discriminated-union `{:status …}` result. `(workflow/run name doc meta thunk)` opens a run directory under `./.sema/runs/<run-id>/`, emits a `run.started` event, evaluates `thunk` (the workflow body), then emits `run.ended` and writes `result.json`. If the body's last value is already a `{:status …}` map it is returned verbatim (its keys land at the top level of `result.json`); otherwise the value is wrapped as `{:status :success :value …}`. An error in the body produces `{:status :failed :error "…"}`. Usually written via the `defworkflow` macro rather than called directly.

```sema
(defworkflow hello "demo" {:args {:name :string}}
  (phase "Inventory")                       ; marker — body forms follow as siblings
  (checkpoint :files (list "a" "b"))
  {:status :success :files (checkpoint :files)})
```

The run journal (`events.jsonl`) is the system of record; run with `sema workflow run <file> --args <json>`.

See also: `defworkflow`, `workflow/phase`, `checkpoint`.
