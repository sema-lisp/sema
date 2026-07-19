//! Root-tagged output pump for the Promise-driven eval seam (P6-3 step 2).
//!
//! `evalPromise` (`driver.rs`) submits its root with
//! `RootOptions { capture_output: true, .. }`, so its `println`/`print-err`
//! writes land in the interpreter's output sink (drained via
//! `Interpreter::take_output`) instead of process stdout/stderr — required so
//! two concurrent `evalPromise` roots' output stays attributable instead of
//! interleaving into one undifferentiated stream. This module drains that
//! sink once per drive turn and forwards each event to an optional JS
//! callback, tagged with the originating root id.
//!
//! The pre-existing `OUTPUT`/`LINE_BUF`/`OUTPUT_SINK` batching in `lib.rs`
//! (used by `eval`/`evalAsync`/`evalVM`/`runEntryAsync`/…) is untouched: those
//! entry points never opt into `capture_output`, so nothing they do reaches
//! this sink, and nothing here reaches theirs.

use std::cell::RefCell;

use js_sys::Function;
use sema_core::runtime::RootId;
use sema_eval::Interpreter;
use sema_vm::runtime::OutputEvent;
use wasm_bindgen::prelude::*;

/// One output event privately buffered for a Promise-driven debugger root.
/// The compatibility `output` array uses `text`; debugger clients that need
/// stream fidelity can consume both fields through `outputEvents`.
pub(crate) struct PromiseOutputEvent {
    pub(crate) stream: &'static str,
    pub(crate) text: String,
}

/// One interpreter's root-tagged output sink. The callback is instance state:
/// two `SemaInterpreter` objects may use the same local root numbers without
/// replacing or receiving each other's output callbacks.
#[derive(Default)]
pub(crate) struct PromiseOutput {
    sink: RefCell<Option<Function>>,
}

impl PromiseOutput {
    /// Install (or clear, with `None`) the JS sink for root-tagged output.
    pub(crate) fn set_sink(&self, sink: Option<Function>) {
        *self.sink.borrow_mut() = sink;
    }

    /// Remove and return whatever sink is currently installed. Used by an OLD
    /// entry point's promise-driven wrapper (`lib.rs`'s
    /// `eval_once_via_promise_seam`) to install a private root-tagged sink for
    /// one call without permanently replacing a real caller's sink.
    pub(crate) fn take_sink(&self) -> Option<Function> {
        self.sink.borrow_mut().take()
    }

    /// Drain every `OutputEvent` captured since the last call. Events belonging
    /// to `captured_root` are returned to that root's private debugger buffer;
    /// every other event is forwarded to the ordinary Promise sink in order.
    /// This keeps debugger action output out of an unrelated `evalPromise`
    /// callback without changing that callback's behavior for ordinary roots.
    pub(crate) fn pump(
        &self,
        interp: &Interpreter,
        captured_root: Option<RootId>,
        retain_captured: impl FnOnce(Vec<PromiseOutputEvent>),
    ) {
        let events = interp.take_output();
        if events.is_empty() {
            retain_captured(Vec::new());
            return;
        }
        let mut captured = Vec::new();
        let mut forwarded = Vec::new();
        for event in events {
            let (root, stream, text) = match event {
                OutputEvent::Stdout { root, text } => (root, "stdout", text),
                OutputEvent::Stderr { root, text } => (root, "stderr", text),
            };
            if captured_root == Some(root) {
                captured.push(PromiseOutputEvent { stream, text });
                continue;
            }
            forwarded.push((root, stream, text));
        }

        // Retain private debugger output before invoking an ordinary sink: a
        // sink is arbitrary JS and may re-enter `debugStopPromise`, which must
        // still settle with every event produced before that stop.
        retain_captured(captured);
        let sink = self.sink.borrow().clone();
        for (root, stream, text) in forwarded {
            let Some(sink) = &sink else {
                continue;
            };
            let root_val = JsValue::from_f64(root_id_as_f64(root));
            let stream_val = JsValue::from_str(stream);
            let text_val = JsValue::from_str(&text);
            let _ = sink.call3(&JsValue::NULL, &root_val, &stream_val, &text_val);
        }
    }
}

/// `RootId`'s local (per-runtime) numeric component, wide enough for a JS
/// `number` correlation tag (well under `Number.MAX_SAFE_INTEGER`) — not a
/// stable wire format, just a same-session identity JS can compare/group by.
fn root_id_as_f64(root: RootId) -> f64 {
    root.get() as f64
}
