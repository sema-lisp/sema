//! Cooperative runtime coverage for the write form of `workflow/checkpoint`.

mod workflow_common;

use sema_llm::fake::FakeProvider;
use workflow_common::{events_of, run_workflow, temp_run_dir, RunOpts};

fn fresh_fake() -> FakeProvider {
    FakeProvider::builder("fake").model("fake-model").build()
}

#[test]
fn checkpoint_write_is_cooperative_atomic_and_occurrence_ordered() {
    let base = temp_run_dir("checkpoint-async");
    let run_id = "wf_checkpoint_async";
    let src = r#"
        (defworkflow checkpoint-async
          "checkpoint callbacks run as structural runtime calls"
          {:phases ["Run"]}
          (phase "Run")

          (def out (channel/new 4))
          (def slow
            (async/spawn
              (fn ()
                (workflow/checkpoint
                  :slow
                  (fn ()
                    (async/sleep 100)
                    (channel/send out :slow)
                    42)))))
          (def sibling
            (async/spawn
              (fn ()
                (async/sleep 10)
                (channel/send out :sibling))))
          (async/await slow)
          (async/await sibling)
          (def first (channel/recv out))

          (def direct
            (channel? (workflow/checkpoint :direct channel/new)))

          (def failed
            (try
              (workflow/checkpoint
                :failed
                (fn ()
                  (async/sleep 1)
                  (error "checkpoint stopped")))
              (catch error :caught)))

          (def ready (channel/new 1))
          (def pending
            (async/spawn
              (fn ()
                (workflow/checkpoint
                  :cancelled
                  (fn ()
                    (channel/send ready true)
                    (async/sleep 250)
                    99)))))
          (channel/recv ready)
          (async/cancel pending)
          (try (async/await pending) (catch error nil))

          (workflow/checkpoint :repeat (fn () 1))
          (workflow/checkpoint :repeat (fn () 2))

          {:status :success
           :first first
           :stored (workflow/checkpoint :slow)
           :direct direct
           :failed-result failed
           :failed-read (workflow/checkpoint :failed)
           :cancelled-read (workflow/checkpoint :cancelled)
           :repeat (workflow/checkpoint :repeat)})
    "#;

    let output = run_workflow(src, fresh_fake(), RunOpts::fresh(run_id, &base));
    assert_eq!(output.result["status"], "success");
    assert_eq!(output.result["first"], "sibling");
    assert_eq!(output.result["stored"], 42);
    assert_eq!(output.result["direct"], true);
    assert_eq!(output.result["failed-result"], "caught");
    assert!(output.result["failed-read"].is_null());
    assert!(output.result["cancelled-read"].is_null());
    assert_eq!(output.result["repeat"], 2);

    let checkpoints = events_of(&output.events, "checkpoint");
    let keys: Vec<&str> = checkpoints
        .iter()
        .filter_map(|event| event["key"].as_str())
        .collect();
    assert_eq!(keys, ["slow", "direct", "repeat", "repeat"]);
    assert_ne!(
        checkpoints[2]["content_key"], checkpoints[3]["content_key"],
        "same-key writes must advance their occurrence ordinal"
    );

    let _ = std::fs::remove_dir_all(base);
}
