---
name: "workflow/approval"
module: "workflow"
section: "Dynamic Workflows"
syntax: "(workflow/approval key opts)"
returns: "bool"
see_also: ["approval", "workflow/run", "defworkflow"]
---

Backend for the `approval` macro. Atomically creates or reads the approval request and immutable, Ed25519-signed decision sidecars under the active run's `approvals/` directory. Filesystem work is quarantined off the cooperative VM. Pending, rejected, invalid, or misplaced requests stop the workflow with an internal, uncatchable control transfer. Only the outer owning `workflow/run` converts that control into a terminal envelope. An approved decision emits `approval.granted` and `approval.applied`, then returns `#t`.

Use the public `approval` macro in workflow source.

See also: `approval`, `workflow/run`, `defworkflow`.
