//! Sema-level dynamic-workflow surface (Spike 1: sequential runtime + frozen journal).
//!
//! Three builtins, all thin wrappers over `sema_workflow` that dispatch a thunk back
//! into the SAME VM via `crate::list::call_function` (the `otel/with-session` idiom):
//!
//! - `(workflow/run name doc meta thunk)` — opens a `WorkflowCtx` (journal sink under
//!   `./.sema/runs/<run-id>/`), emits `run.started`, runs the body thunk, emits
//!   `run.ended` with status derived from Ok/Err, writes `result.json`, and returns a
//!   discriminated-union `{:status ...}` envelope value.
//! - `(workflow/phase label thunk)` — emits `phase.started`, runs the thunk, then emits
//!   `phase.ended` BEFORE propagating Ok OR Err (the emit is ordered before the Err
//!   short-circuit; the Drop guard covers the rare panic case).
//! - `(checkpoint :k v)` records and returns `v` (emitting a `checkpoint` event);
//!   `(checkpoint :k)` reads the previously-stored value.
//!
//! The macros `defworkflow`/`phase` (prelude) expand to these — see
//! `crates/sema-eval/src/prelude.rs`.

use sema_core::{SemaError, Value};
use sema_workflow::context;
use sema_workflow::event::WorkflowEvent;
use std::collections::BTreeMap;

/// A keyword/string argument as a plain `String` (checkpoint keys, phase labels).
fn as_name(v: &Value) -> Option<String> {
    v.as_keyword().or_else(|| v.as_str().map(|s| s.to_string()))
}

/// Build the success envelope returned by `workflow/run`. PASS-THROUGH: if the
/// workflow body's last value is already a `{:status …}` map (the idiomatic shape —
/// e.g. `{:status :success :files … :findings …}`), it is returned verbatim so its
/// fields land at the top level of `result.json`. Otherwise the value is wrapped in
/// `{:status :success :value <v>}`.
fn success_envelope(value: Value) -> Value {
    if let Some(m) = value.as_map_rc() {
        if m.keys()
            .any(|k| k.as_keyword().as_deref() == Some("status"))
        {
            return value;
        }
    }
    let mut m = BTreeMap::new();
    m.insert(Value::keyword("status"), Value::keyword("success"));
    m.insert(Value::keyword("value"), value);
    Value::map(m)
}

fn failed_envelope(msg: &str) -> Value {
    let mut m = BTreeMap::new();
    m.insert(Value::keyword("status"), Value::keyword("failed"));
    m.insert(Value::keyword("error"), Value::string(msg));
    Value::map(m)
}

pub fn register(env: &sema_core::Env) {
    // (workflow/run name doc meta thunk) — open a run scope, journal start/end, return
    // the {:status ...} envelope. `name`/`doc` are strings; `meta` is the workflow's
    // metadata map ({:args ... :budget ... :perms ...}); `thunk` is the (lambda () ...)
    // wrapping the workflow body.
    crate::register_fn(env, "workflow/run", |args| {
        if args.len() != 4 {
            return Err(SemaError::arity("workflow/run", "4", args.len()));
        }
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        // doc (args[1]) and meta (args[2]) are carried for the journal/metadata.json but
        // not otherwise interpreted in Spike 1; tolerate any shape.
        let doc = args[1].as_str().unwrap_or("").to_string();
        let meta = args[2].clone();
        let thunk = &args[3];

        // Open the run scope: sets up the journal sink under ./.sema/runs/<run-id>/,
        // installs the thread-local WorkflowCtx, and returns a panic-safe Drop guard
        // that reaps the previous scope. `set_workflow_scope` reads the
        // SEMA_WORKFLOW_RUN_ID / SEMA_WORKFLOW_FIXED_TS test seam internally.
        let guard = context::set_workflow_scope(&name, &doc, &meta)
            .map_err(|e| SemaError::eval(format!("workflow/run: {e}")))?;

        // run.started — emitted inside the live scope so seq starts at 0.
        {
            let ctx = context::current()
                .ok_or_else(|| SemaError::eval("workflow/run: scope not established"))?;
            ctx.emit(WorkflowEvent::RunStarted {
                seq: ctx.next_seq(),
                ts: ctx.ts(),
                workflow: name.clone(),
                run_id: ctx.run_id(),
                code_version: String::new(),
            });
        }

        // Run the body thunk in the same VM.
        let result = crate::list::call_function(thunk, &[]);

        // Derive the envelope + status, then journal run.ended and write result.json
        // BEFORE the guard drops (so the sink is still open).
        let (status, envelope) = match &result {
            Ok(v) => ("success", success_envelope(v.clone())),
            Err(e) => ("failed", failed_envelope(&e.to_string())),
        };

        if let Some(ctx) = context::current() {
            ctx.emit(WorkflowEvent::RunEnded {
                seq: ctx.next_seq(),
                ts: ctx.ts(),
                status: status.into(),
                reason: if result.is_ok() {
                    None
                } else {
                    Some("workflow body returned an error".to_string())
                },
                dur_ms: ctx.dur_ms(),
            });
            // result.json — the final envelope. Lossy/best-effort; swallow write errors
            // the same way the journal writer does.
            ctx.write_result(&envelope);
        }

        drop(guard);
        Ok(envelope)
    });

    // (workflow/phase label thunk) — journaled labeled scope. Emits phase.ended on BOTH
    // the Ok and Err paths, ordered BEFORE propagating the Err (per the spec sketch).
    crate::register_fn(env, "workflow/phase", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("workflow/phase", "2", args.len()));
        }
        let label = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let ctx = context::current()
            .ok_or_else(|| SemaError::eval("workflow/phase outside a workflow/run"))?;

        ctx.emit(WorkflowEvent::PhaseStarted {
            seq: ctx.next_seq(),
            ts: ctx.ts(),
            phase: label.clone(),
        });

        // Dispatches into the SAME VM. Sema errors are Result-valued, so the dominant
        // failure mode is an Err short-circuit, NOT a Rust panic.
        let result = crate::list::call_function(&args[1], &[]);
        let status = if result.is_ok() { "success" } else { "failed" };

        ctx.emit(WorkflowEvent::PhaseEnded {
            seq: ctx.next_seq(),
            ts: ctx.ts(),
            phase: label,
            status: status.into(),
            dur_ms: ctx.dur_ms(), // 0 under the fixed-ts seam
        });

        // Propagate Ok OR Err AFTER phase.ended is journaled. The phase body's last
        // value flows out unchanged (the enclosing workflow/run wraps it in :value).
        result
    });

    // (checkpoint :k v) records+returns v and emits a checkpoint event;
    // (checkpoint :k) reads the stored value (nil if unset).
    crate::register_fn(env, "checkpoint", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("checkpoint", "1-2", args.len()));
        }
        let key = as_name(&args[0])
            .ok_or_else(|| SemaError::type_error("keyword or string", args[0].type_name()))?;
        let ctx = context::current()
            .ok_or_else(|| SemaError::eval("checkpoint outside a workflow/run"))?;

        if args.len() == 2 {
            // Write: store, journal, return the value (so it threads through `let`).
            let value = args[1].clone();
            ctx.store_checkpoint(&key, value.clone());
            ctx.emit(WorkflowEvent::Checkpoint {
                seq: ctx.next_seq(),
                ts: ctx.ts(),
                key: key.clone(),
                value_digest: ctx.value_digest(&value),
            });
            Ok(value)
        } else {
            // Read: return the stored value or nil.
            Ok(ctx.read_checkpoint(&key).unwrap_or_else(Value::nil))
        }
    });
}
