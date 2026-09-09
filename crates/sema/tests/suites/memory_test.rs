//! `memory/*` transcript module (agent memory, transcript half).
//!
//! Covers the documented surface (`crates/sema-docs/entries/stdlib/memory/`):
//! `memory/open` (idempotent handle), `memory/append` (working-set push +
//! durable JSONL tail flush), `memory/messages` (Conversation view) — plus the
//! `agent/run {:memory h}` seam: seeding, writeback on completion, and partial
//! writeback when a run is cancelled mid-conversation (issue #87's recovery
//! leg). Persistence tests point the module at a per-test temp dir via
//! `sema_stdlib::set_memory_base_dir_override` (kv-bounds-override pattern).

use sema_core::Value;
use sema_eval::Interpreter;

/// A unique temp base dir for one test's memory files, removed on drop.
struct TempMemDir(std::path::PathBuf);

#[test]
fn memory_append_rejects_cumulative_encoded_size_before_mutation() {
    struct ResetBounds;
    impl Drop for ResetBounds {
        fn drop(&mut self) {
            sema_stdlib::set_memory_bounds_override(None);
        }
    }
    let _reset = ResetBounds;
    let content = "line\n\"quoted\"";
    let line = serde_json::json!({"role":"user", "content":content}).to_string();
    let cap = 2 * (line.len() as u64 + 1);
    for legacy in [true, false] {
        let dir = TempMemDir::new("cumulative-size");
        sema_stdlib::set_memory_bounds_override(Some((cap, 1024)));
        let interp = Interpreter::new();
        let handle = interp
            .eval_str("(define mem (memory/open {:id \"cap\"})) mem")
            .unwrap();
        let message = Value::map(std::collections::BTreeMap::from([
            (Value::keyword("role"), Value::string("user")),
            (Value::keyword("content"), Value::string(content)),
        ]));
        let append = || {
            if legacy {
                let callable = interp
                    .global_env
                    .get(sema_core::intern("memory/append"))
                    .unwrap();
                (callable.as_native_fn_ref().unwrap().func)(
                    &interp.ctx,
                    &[handle.clone(), message.clone()],
                )
            } else {
                interp.eval_str(&format!("(memory/append mem {{:content {content:?}}})"))
            }
        };
        append().unwrap();
        append().unwrap();
        let error = append().expect_err("third encoded line exceeds file cap");
        assert!(error.to_string().contains("byte cap"), "{error}");
        assert_eq!(
            interp
                .eval_str("(length (conversation/messages (memory/messages mem)))")
                .unwrap(),
            Value::int(2)
        );
        drop(interp);
        let reopened = Interpreter::new();
        assert_eq!(reopened.eval_str("(length (conversation/messages (memory/messages (memory/open {:id \"cap\"}))))").unwrap(), Value::int(2));
        assert_eq!(
            std::fs::metadata(dir.0.join("default/cap.jsonl"))
                .unwrap()
                .len(),
            cap
        );
    }
}

impl TempMemDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("sema-memory-test-{tag}-{nanos}"));
        sema_stdlib::set_memory_base_dir_override(Some(path.clone()));
        TempMemDir(path)
    }
}

impl Drop for TempMemDir {
    fn drop(&mut self) {
        sema_stdlib::set_memory_base_dir_override(None);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `memory/open` returns the documented handle map and is idempotent within a
/// process: reopening the same `(namespace, id)` returns the SAME thread (the
/// working set survives), not a fresh empty one.
#[test]
fn memory_open_returns_handle_and_is_idempotent() {
    let _dir = TempMemDir::new("open");
    let interp = Interpreter::new();
    let val = interp
        .eval_str_compiled(
            r#"
            (define mem (memory/open {:id "user-42" :namespace "support"}))
            (memory/append mem {:role "user" :content "Hello"})
            ;; Re-open the same key: must see the same working set, not reset it.
            (define mem2 (memory/open {:id "user-42" :namespace "support"}))
            (list (:memory/id mem)
                  (:memory/namespace mem)
                  (length (conversation/messages (memory/messages mem2))))
            "#,
        )
        .expect("open/append/reopen should succeed");
    let items = val.as_list().expect("result list");
    assert_eq!(items[0].as_str(), Some("user-42"));
    assert_eq!(items[1].as_str(), Some("support"));
    assert_eq!(
        items[2].as_int(),
        Some(1),
        "reopening an open thread must return the same working set"
    );
}

/// Round trip: appended turns come back from `memory/messages` as a
/// Conversation, roles normalized ("assistant" stays, anything else → "user",
/// absent role defaults to "user"), and `memory/append` returns the handle so
/// calls chain.
#[test]
fn memory_append_and_messages_round_trip() {
    let _dir = TempMemDir::new("roundtrip");
    let interp = Interpreter::new();
    let val = interp
        .eval_str_compiled(
            r#"
            (define mem (memory/open {:id "chat-1"}))
            ;; append returns the handle — chain two appends, then one with no :role.
            (memory/append (memory/append mem {:role "user" :content "Q"})
                           {:role "assistant" :content "A"})
            (memory/append mem {:content "follow-up"})
            (let ((msgs (conversation/messages (memory/messages mem))))
              (list (length msgs)
                    (message/role (first msgs))
                    (message/role (nth msgs 1))
                    (message/role (nth msgs 2))
                    (message/content (nth msgs 2))))
            "#,
        )
        .expect("append/messages round trip should succeed");
    let items = val.as_list().expect("result list");
    assert_eq!(items[0].as_int(), Some(3));
    assert_eq!(items[1], Value::keyword("user"));
    assert_eq!(items[2], Value::keyword("assistant"));
    assert_eq!(
        items[3],
        Value::keyword("user"),
        "absent :role must default to user"
    );
    assert_eq!(items[4].as_str(), Some("follow-up"));
}

/// Turns appended in one interpreter are visible after `memory/open` in a
/// fresh interpreter pointed at the same base dir — the JSONL sidecar is the
/// durable source reloaded by open.
#[test]
fn memory_persists_across_interpreters() {
    let _dir = TempMemDir::new("persist");
    {
        let interp = Interpreter::new();
        interp
            .eval_str_compiled(
                r#"
                (define mem (memory/open {:id "s1" :namespace "persist"}))
                (memory/append mem {:role "user" :content "first"})
                (memory/append mem {:role "assistant" :content "second"})
                "#,
            )
            .expect("first-run appends should succeed");
    }
    let interp = Interpreter::new();
    let val = interp
        .eval_str_compiled(
            r#"
            (define mem (memory/open {:id "s1" :namespace "persist"}))
            (let ((msgs (conversation/messages (memory/messages mem))))
              (list (length msgs)
                    (message/content (first msgs))
                    (message/role (nth msgs 1))))
            "#,
        )
        .expect("reopen in a fresh interpreter should succeed");
    let items = val.as_list().expect("result list");
    assert_eq!(
        items[0].as_int(),
        Some(2),
        "prior run's turns must be loaded by memory/open"
    );
    assert_eq!(items[1].as_str(), Some("first"));
    assert_eq!(items[2], Value::keyword("assistant"));
}

/// Ops on a never-opened thread fail with a clear error naming `memory/open`
/// — a hand-built handle map is not a substitute for opening.
#[test]
fn memory_ops_on_unopened_thread_error() {
    let _dir = TempMemDir::new("unopened");
    let interp = Interpreter::new();
    let err = interp
        .eval_str_compiled(
            r#"(memory/append {:memory/id "ghost" :memory/namespace "default"}
                             {:content "x"})"#,
        )
        .expect_err("append on an unopened thread must error");
    assert!(
        err.hint().is_some_and(|h| h.contains("memory/open")),
        "error hint must point at memory/open, got: {err} (hint: {:?})",
        err.hint()
    );
}

/// `memory/open` without `:id` is a clear argument error.
#[test]
fn memory_open_requires_id() {
    let _dir = TempMemDir::new("no-id");
    let interp = Interpreter::new();
    let err = interp
        .eval_str_compiled(r#"(memory/open {:namespace "x"})"#)
        .expect_err("open without :id must error");
    assert!(
        err.to_string().contains(":id"),
        "error must name the missing :id opt, got: {err}"
    );
}

/// Path-shaped `:id`/`:namespace` values are rejected — they become file
/// names under the base dir, never path expressions (pins the traversal
/// guard).
#[test]
fn memory_rejects_path_traversal_ids() {
    let _dir = TempMemDir::new("traversal");
    let interp = Interpreter::new();
    for bad in [
        r#"(memory/open {:id "../evil"})"#,
        r#"(memory/open {:id "a/b"})"#,
    ] {
        let err = interp
            .eval_str_compiled(bad)
            .expect_err("path-shaped id must be rejected");
        assert!(
            err.to_string().contains("plain name"),
            "expected the plain-name error for {bad}, got: {err}"
        );
    }
}

/// Unified-runtime contract: inside a quantum, `memory/append`'s sidecar
/// flush is offloaded — a zero-delay sibling task completes while the flush
/// is in flight (kv_async_lets_sibling_run_first's mechanism). Durability is
/// preserved: the appended line is on disk by the time the call resolves.
#[test]
fn memory_append_lets_sibling_run_first() {
    let dir = TempMemDir::new("sib-order");
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(
            r#"
            (define mem (memory/open {:id "chat" :namespace "race"}))
            (let ((out (channel/new 8)))
              (async/all
                (list
                  (async/spawn (fn ()
                    (memory/append mem {:role "user" :content "raced"})
                    (channel/send out "mem")))
                  (async/spawn (fn () (channel/send out "sibling")))))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("sibling-ordering program evaluated");
    let received: Vec<String> = result
        .as_list()
        .expect("channel receives list")
        .iter()
        .map(|v| v.as_str().expect("string value").to_string())
        .collect();
    assert_eq!(
        received,
        vec!["sibling".to_string(), "mem".to_string()],
        "sibling task must complete while the offloaded memory/append flush is in flight"
    );
    // Not fire-and-forget: the line must already be durable on disk.
    let sidecar = dir.0.join("race").join("chat.jsonl");
    let text = std::fs::read_to_string(&sidecar).expect("sidecar written");
    assert_eq!(text.lines().count(), 1, "exactly the appended turn on disk");
    assert!(text.contains("raced"));
}

// ── agent/run {:memory h} seam ───────────────────────────────────────────────

use std::sync::Arc;

use sema_llm::builtins::{register_test_provider, reset_runtime_state};
use sema_llm::fake::{FakeProvider, FakeRecorder};

/// Build an interpreter with `fake` installed as the default provider
/// (llm_fake_test's harness shape).
fn interp_with_fake(fake: FakeProvider) -> (Interpreter, Arc<FakeRecorder>) {
    let interp = Interpreter::new();
    reset_runtime_state();
    let recorder = fake.recorder();
    register_test_provider(Box::new(fake));
    (interp, recorder)
}

/// `agent/run` with `{:memory h}` seeds the provider request with the
/// thread's prior turns, ahead of the new user input.
#[test]
fn agent_run_with_memory_seeds_prior_turns() {
    let _dir = TempMemDir::new("seed");
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply("noted")
        .build();
    let (interp, recorder) = interp_with_fake(fake);
    interp
        .eval_str_compiled(
            r#"
            (defagent bot {:model "fake-model" :max-turns 3})
            (define mem (memory/open {:id "seeded" :namespace "agents"}))
            (memory/append mem {:role "user" :content "earlier question"})
            (memory/append mem {:role "assistant" :content "earlier answer"})
            (agent/run bot "new question" {:memory mem})
            "#,
        )
        .expect("agent run with memory should complete");
    let reqs = recorder.requests();
    let req = reqs.last().expect("one provider round");
    let contents: Vec<String> = req.messages.iter().map(|m| m.content.to_text()).collect();
    let earlier_q = contents
        .iter()
        .position(|c| c == "earlier question")
        .expect("seeded user turn must be in the request");
    let earlier_a = contents
        .iter()
        .position(|c| c == "earlier answer")
        .expect("seeded assistant turn must be in the request");
    let new_q = contents
        .iter()
        .position(|c| c == "new question")
        .expect("new user input must be in the request");
    assert!(
        earlier_q < earlier_a && earlier_a < new_q,
        "seed order: {contents:?}"
    );
    assert_eq!(
        req.messages[earlier_a].role, "assistant",
        "seeded assistant turn must keep its role"
    );
}

/// A completed `agent/run` writes the new turns back: the user input and the
/// assistant's text reply land in the thread (visible to `memory/messages`
/// and durable for the next process).
#[test]
fn agent_run_with_memory_writes_back_on_completion() {
    let _dir = TempMemDir::new("writeback");
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply("the answer")
        .build();
    let (interp, _recorder) = interp_with_fake(fake);
    let val = interp
        .eval_str_compiled(
            r#"
            (defagent bot {:model "fake-model" :max-turns 3})
            (define mem (memory/open {:id "wb" :namespace "agents"}))
            (agent/run bot "the question" {:memory mem})
            (let ((msgs (conversation/messages (memory/messages mem))))
              (list (length msgs)
                    (message/content (first msgs))
                    (message/role (nth msgs 1))
                    (message/content (nth msgs 1))))
            "#,
        )
        .expect("agent run + readback should succeed");
    let items = val.as_list().expect("result list");
    assert_eq!(items[0].as_int(), Some(2), "user turn + assistant reply");
    assert_eq!(items[1].as_str(), Some("the question"));
    assert_eq!(items[2], Value::keyword("assistant"));
    assert_eq!(items[3].as_str(), Some("the answer"));
}

#[test]
fn agent_memory_writeback_obeys_cumulative_size_cap() {
    let _dir = TempMemDir::new("agent-byte-cap");
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .reply("answer")
        .build();
    let (interp, recorder) = interp_with_fake(fake);
    sema_stdlib::set_memory_bounds_override(Some((40, 1024)));
    // The first turn fits; the second must not make the transcript impossible
    // to reopen. Agent completion may report the writeback error to the caller.
    let _ = interp.eval_str(
        r#"
        (defagent bot {:model "fake-model" :max-turns 3})
        (define mem (memory/open {:id "bounded"}))
        (agent/run bot "question" {:memory mem})
    "#,
    );
    assert_eq!(recorder.requests().len(), 1);
    assert_eq!(
        interp
            .eval_str("(length (conversation/messages (memory/messages mem)))")
            .unwrap(),
        Value::int(1)
    );
    drop(interp);
    let reopened = Interpreter::new();
    let result = reopened.eval_str(
        "(length (conversation/messages (memory/messages (memory/open {:id \"bounded\"}))))",
    );
    sema_stdlib::set_memory_bounds_override(None);
    assert_eq!(result.unwrap(), Value::int(1));
}

/// Issue #87's persistence leg: a spawned `agent/run {:memory h}` cancelled
/// mid-conversation still writes the turns it HAD back to the thread — the
/// user input survives the cancellation (tool-protocol turns are not part of
/// the text transcript), and the thread stays clean for a follow-up run.
#[test]
fn cancelled_agent_run_writes_partial_turns_to_memory() {
    let _dir = TempMemDir::new("cancel-wb");
    // 8 tool rounds (9 calls) at 100 ms each ⇒ ~900 ms full; cancel at 250 ms.
    let fake = FakeProvider::builder("fake")
        .model("fake-model")
        .chat_delay(100)
        .tool_loop(8, "ping", serde_json::json!({ "n": 1 }), "done")
        .build();
    let (interp, recorder) = interp_with_fake(fake);
    let val = interp
        .eval_str_compiled(
            r#"
            (deftool ping "ping" {:n {:type :number}} (fn (n) "pong"))
            (defagent bot {:model "fake-model" :tools [ping] :max-turns 12})
            (define mem (memory/open {:id "partial" :namespace "agents"}))
            (let ((p (async/spawn (fn () (agent/run bot "interrupted question" {:memory mem})))))
              (async/spawn (fn () (async/sleep 250) (async/cancel p)))
              (try (async/await p) (catch e nil)))
            (let ((msgs (conversation/messages (memory/messages mem))))
              (list (length msgs) (message/content (first msgs))))
            "#,
        )
        .expect("cancelled agent run + readback should succeed");
    let items = val.as_list().expect("result list");
    assert!(
        recorder.call_count() < 9,
        "the cancel must actually cut the loop short (got {} rounds)",
        recorder.call_count()
    );
    assert_eq!(
        items[0].as_int(),
        Some(1),
        "the cancelled run's user turn must be written back"
    );
    assert_eq!(items[1].as_str(), Some("interrupted question"));
}

/// Unified-runtime contract for `memory/open`: the initial sidecar read+parse
/// offloads, so a zero-delay sibling completes first while the load is in
/// flight — and the loaded turns are still correct afterwards.
#[test]
fn memory_open_lets_sibling_run_first() {
    let _dir = TempMemDir::new("open-race");
    {
        let interp = Interpreter::new();
        interp
            .eval_str_compiled(
                r#"
                (define mem (memory/open {:id "prior" :namespace "race"}))
                (memory/append mem {:role "user" :content "from before"})
                "#,
            )
            .expect("seed the sidecar");
    }
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(
            r#"
            (let ((out (channel/new 8)))
              (async/all
                (list
                  (async/spawn (fn ()
                    (define mem (memory/open {:id "prior" :namespace "race"}))
                    (channel/send out (length (conversation/messages (memory/messages mem))))))
                  (async/spawn (fn () (channel/send out "sibling")))))
              (list (channel/recv out) (channel/recv out)))
            "#,
        )
        .expect("open-race program evaluated");
    let items = result.as_list().expect("receive order list");
    assert_eq!(
        items[0].as_str(),
        Some("sibling"),
        "sibling must complete while the offloaded memory/open load is in flight"
    );
    assert_eq!(
        items[1].as_int(),
        Some(1),
        "the offloaded open must still load the prior turn"
    );
}

/// An oversized sidecar is rejected by `memory/open` (finite-work bound) —
/// via the lowered test-seam cap.
#[test]
fn memory_open_rejects_oversized_sidecar() {
    let dir = TempMemDir::new("big-file");
    sema_stdlib::set_memory_bounds_override(Some((64, usize::MAX)));
    let sidecar_dir = dir.0.join("default");
    std::fs::create_dir_all(&sidecar_dir).expect("mk sidecar dir");
    std::fs::write(
        sidecar_dir.join("big.jsonl"),
        format!("{}\n", r#"{"role":"user","content":"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}"#),
    )
    .expect("write oversized sidecar");
    let interp = Interpreter::new();
    let err = interp
        .eval_str_compiled(r#"(memory/open {:id "big"})"#)
        .expect_err("oversized sidecar must be rejected");
    sema_stdlib::set_memory_bounds_override(None);
    assert!(
        err.to_string().contains("exceeds"),
        "expected the size-cap error, got: {err}"
    );
}

/// An oversized turn is rejected pre-dispatch by `memory/append`, with the
/// working set and sidecar untouched.
#[test]
fn memory_append_rejects_oversized_turn() {
    let dir = TempMemDir::new("big-turn");
    sema_stdlib::set_memory_bounds_override(Some((u64::MAX, 32)));
    let interp = Interpreter::new();
    let val = interp
        .eval_str_compiled(
            r#"
            (define mem (memory/open {:id "turns"}))
            (let ((e (try (memory/append mem {:content "this content is far longer than the 32-byte cap"})
                          (catch e :rejected))))
              (list e (length (conversation/messages (memory/messages mem)))))
            "#,
        )
        .expect("append rejection program evaluated");
    sema_stdlib::set_memory_bounds_override(None);
    let items = val.as_list().expect("result list");
    assert_eq!(items[0], Value::keyword("rejected"));
    assert_eq!(
        items[1].as_int(),
        Some(0),
        "a rejected append must leave the working set untouched"
    );
    assert!(
        !dir.0.join("default").join("turns.jsonl").exists(),
        "a rejected append must not create the sidecar"
    );
}

/// Sandbox: `memory/open` mints a persistent READ-WRITE resource (appends,
/// agent writeback, teardown flush all write its sidecar), so it is gated on
/// FS_WRITE like `kv/open` — a write-denied sandbox cannot open a thread.
#[test]
fn memory_open_requires_fs_write_cap() {
    let _dir = TempMemDir::new("sandbox");
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_WRITE);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let err = interp
        .eval_str_compiled(r#"(memory/open {:id "denied"})"#)
        .expect_err("write-denied sandbox must not mint a memory handle");
    assert!(
        err.to_string().contains("sandbox") || err.to_string().contains("denied"),
        "expected a sandbox denial, got: {err}"
    );
}

/// A flush that fails MID-WRITE (partial bytes on disk) must roll back, so the
/// retry appends each turn exactly once — no duplicated lines, no torn JSON
/// mashed with the next line. Driven by the flush fault-injection seam.
/// Also the cross-platform oracle for the rollback path itself: `write_lines`
/// truncates via a fresh write-mode handle because an append-mode handle
/// lacks the `FILE_WRITE_DATA` right on Windows.
#[test]
fn memory_partial_flush_failure_retries_without_duplicates() {
    let dir = TempMemDir::new("partial-flush");
    let interp = Interpreter::new();
    interp
        .eval_str_compiled(
            r#"
            (define mem (memory/open {:id "pf"}))
            (memory/append mem {:role "user" :content "turn-one"})
            "#,
        )
        .expect("baseline append");
    // Next flush: write a few bytes, then fail (simulates ENOSPC mid-tail).
    sema_stdlib::set_memory_flush_partial_fail_override(Some(5));
    let err = interp
        .eval_str_compiled(r#"(memory/append (memory/open {:id "pf"}) {:content "turn-two"})"#)
        .expect_err("mid-write flush failure must surface");
    sema_stdlib::set_memory_flush_partial_fail_override(None);
    assert!(
        err.to_string().contains("cannot write") || err.to_string().contains("injected"),
        "expected the flush write error, got: {err}"
    );
    // Retry: the tail (turn-two) flushes again, plus a fresh turn.
    interp
        .eval_str_compiled(r#"(memory/append (memory/open {:id "pf"}) {:content "turn-three"})"#)
        .expect("retry after failed flush");
    let text =
        std::fs::read_to_string(dir.0.join("default").join("pf.jsonl")).expect("sidecar readable");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "exactly three turns on disk (no duplicates, no torn fragments): {text:?}"
    );
    for (i, want) in ["turn-one", "turn-two", "turn-three"].iter().enumerate() {
        assert!(
            lines[i].contains(want),
            "line {i} must be {want}, got {:?}",
            lines[i]
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(lines[i]).is_ok(),
            "line {i} must be intact JSON, got {:?}",
            lines[i]
        );
    }
}

/// A sandbox that grants FS_WRITE but restricts `allowed_paths` elsewhere must
/// not let `memory/open` mint a thread — the derived sidecar path is checked
/// like `kv/open` checks its path argument.
#[test]
fn memory_open_respects_sandbox_allowed_paths() {
    let _dir = TempMemDir::new("allowed-paths");
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::NONE)
        .with_allowed_paths(vec![std::path::PathBuf::from("/nonexistent-allowed-root")]);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let err = interp
        .eval_str_compiled(r#"(memory/open {:id "outside"})"#)
        .expect_err("path-restricted sandbox must reject the derived sidecar path");
    assert!(
        err.to_string().contains("sandbox") || err.to_string().contains("path"),
        "expected a path denial, got: {err}"
    );
}

/// Windows-hostile segments are rejected on every platform: drive prefixes
/// (`C:notes` re-roots the join on Windows), NTFS alternate-data-stream
/// colons, and trailing dots/spaces (silently stripped by Win32, colliding
/// distinct ids onto one file).
#[test]
fn memory_rejects_windows_reserved_segments() {
    let _dir = TempMemDir::new("win-segments");
    let interp = Interpreter::new();
    for bad in [
        r#"(memory/open {:id "C:notes"})"#,
        r#"(memory/open {:id "chat:hidden"})"#,
        r#"(memory/open {:id "chat."})"#,
        r#"(memory/open {:id "chat "})"#,
    ] {
        let err = interp
            .eval_str_compiled(bad)
            .expect_err("reserved segment must be rejected");
        assert!(
            err.to_string().contains("plain name"),
            "expected the plain-name error for {bad}, got: {err}"
        );
    }
}
