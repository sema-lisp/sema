//! MCP-call cassette hook — a crate-neutral seam so `sema-mcp` can record/replay
//! tool calls through the LLM cassette that lives in `sema-llm`, without a
//! dependency edge between the two crates.
//!
//! `sema-llm` owns the tape and installs the hook at interpreter init (a function
//! pointer into its task-scoped cassette selection, like `set_eval_callback`);
//! `sema-mcp` consults the hook around each real `tools/call`. When no cassette
//! is selected the hook returns no decision and calls pass straight through.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::Value;

/// A cassette selected at dispatch time. The capability retains the exact tape
/// independently of ambient task scope, so an asynchronous completion records
/// into the cassette that authorized the call.
pub struct McpCassetteRecorder {
    target: Rc<dyn McpCassetteRecordTarget>,
    key: String,
}

impl McpCassetteRecorder {
    pub fn new(target: Rc<dyn McpCassetteRecordTarget>, key: String) -> Self {
        Self { target, key }
    }

    /// Consume the one-shot capability and record one successful call result.
    pub fn record(self, value: &Value) {
        self.target.record(&self.key, value);
    }
}

/// Host-owned cassette state behind [`McpCassetteRecorder`]. Implementations
/// must not retain Sema `Value`/`Env` graph state; the runtime does not trace it.
pub trait McpCassetteRecordTarget {
    fn record(&self, key: &str, value: &Value);
}

/// What to do for an MCP tool call under an active cassette.
pub enum McpCassetteDecision {
    /// Serve this recorded result — do NOT touch the network.
    Replay(Value),
    /// Replay mode with no matching entry — a hard miss (surfaces drift).
    Miss,
    /// Perform the real call, then record through this dispatch-time capability.
    Record(McpCassetteRecorder),
}

type DecideFn = fn(&str) -> Option<McpCassetteDecision>;

thread_local! {
    static HOOK: RefCell<Option<DecideFn>> = const { RefCell::new(None) };
}

/// Register the cassette hook. Called by `sema-llm` during interpreter setup.
pub fn set_mcp_cassette_hook(decide: DecideFn) {
    HOOK.with(|h| *h.borrow_mut() = Some(decide));
}

/// Remove the cassette hook (calls pass straight through afterwards).
pub fn clear_mcp_cassette_hook() {
    HOOK.with(|h| *h.borrow_mut() = None);
}

/// Decide how to handle an MCP call for `key`; `None` when no cassette is active.
pub fn mcp_cassette_decide(key: &str) -> Option<McpCassetteDecision> {
    HOOK.with(|h| h.borrow().as_ref().and_then(|decide| decide(key)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    thread_local! {
        static TEST_TAPE: RefCell<HashMap<String, Value>> = RefCell::new(HashMap::new());
    }

    struct TestRecorder;

    impl McpCassetteRecordTarget for TestRecorder {
        fn record(&self, key: &str, value: &Value) {
            TEST_TAPE.with(|t| {
                t.borrow_mut().insert(key.to_string(), value.clone());
            });
        }
    }

    fn test_decide(key: &str) -> Option<McpCassetteDecision> {
        TEST_TAPE.with(|t| match t.borrow().get(key) {
            Some(v) => Some(McpCassetteDecision::Replay(v.clone())),
            None => Some(McpCassetteDecision::Record(McpCassetteRecorder::new(
                Rc::new(TestRecorder),
                key.to_string(),
            ))),
        })
    }

    #[test]
    fn hook_routes_decide_and_record() {
        // No hook → transparent.
        assert!(mcp_cassette_decide("k").is_none());

        set_mcp_cassette_hook(test_decide);
        // Unknown key → Record.
        let recorder = match mcp_cassette_decide("k") {
            Some(McpCassetteDecision::Record(recorder)) => recorder,
            _ => panic!("expected a record capability"),
        };
        // Record then replay the exact value.
        recorder.record(&serde_json::json!({"a": 1}));
        match mcp_cassette_decide("k") {
            Some(McpCassetteDecision::Replay(v)) => assert_eq!(v, serde_json::json!({"a": 1})),
            _ => panic!("expected a replay hit"),
        }

        clear_mcp_cassette_hook();
        assert!(mcp_cassette_decide("k").is_none());
    }
}
