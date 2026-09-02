---
name: "policy/without"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(policy/without reason body ...)"
returns: "any"
see_also: ["workflow/policy-without", "defpolicy", "defworkflow", "workflow/check"]
---

Bypass active model and tool policies for a trusted lexical scope. `reason`
must be a non-empty literal string of at most 256 characters, and the form must
contain at least one body expression. Each protected boundary emits a
`policy.bypassed` journal event with the reason.

```sema
(policy/without "read the migration fixture"
  (step "Inspect the legacy fixture." {:tools [read-file]}))
```

The form requires an enclosing `workflow/run` with at least one active policy.
Without one it raises `policy/without requires an active workflow policy`, so
the example above only works inside a workflow whose `:policy` (or an enclosing
step's `:policy`) is set.

The bypass is task-local and applies only to its body. It does not bypass
workflow `:permissions`, the CLI sandbox, or allowed-path limits.

See also: `defpolicy`, `defworkflow`, `workflow/check`.
