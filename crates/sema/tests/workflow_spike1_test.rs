//! Spike 1 acceptance oracle: a fixture workflow produces a BYTE-IDENTICAL golden
//! `events.jsonl` under the fixed-clock / fixed-run-id test seam, run twice
//! identically, jq-inspectable, with the Audit phase reading back the value the
//! Inventory phase checkpointed.
//!
//! Driven end-to-end through the real CLI binary (`env!("CARGO_BIN_EXE_sema")`),
//! so it exercises the `sema workflow run` dispatch arm, the `--args` JSON parse,
//! the `--run-dir` seam, and the frozen JSONL journal exactly as a user would.
//!
//! Seam contract (read by the workflow runtime, set here):
//!   SEMA_WORKFLOW_FIXED_TS=0   -> every event `ts` is "0" and every `dur_ms` is 0
//!   SEMA_WORKFLOW_RUN_ID=...   -> deterministic run-id (else a fresh ULID/uuid)
//!   --run-dir <dir>            -> base dir; the run lands in <dir>/<run-id>/

use std::path::PathBuf;
use std::process::Command;

const RUN_ID: &str = "wf_test_0001";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/workflow")
        .join(name)
}

/// Run `sema workflow run` against the fixture into a fresh temp base dir and
/// return the bytes of the produced `events.jsonl`.
fn run_workflow_into(base_dir: &std::path::Path) -> Vec<u8> {
    let status = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("workflow")
        .arg("run")
        .arg(fixture("hello-wf.sema"))
        .arg("--args")
        .arg(r#"{"name":"x"}"#)
        .arg("--run-dir")
        .arg(base_dir)
        // Test seam: deterministic clock + run-id so the journal is byte-stable.
        .env("SEMA_WORKFLOW_FIXED_TS", "0")
        .env("SEMA_WORKFLOW_RUN_ID", RUN_ID)
        .status()
        .expect("failed to spawn sema workflow run");
    assert!(status.success(), "workflow run exited non-zero: {status:?}");

    let events = base_dir.join(RUN_ID).join("events.jsonl");
    std::fs::read(&events)
        .unwrap_or_else(|e| panic!("could not read journal {}: {e}", events.display()))
}

#[test]
fn spike1_golden_journal_byte_identical() {
    let golden = std::fs::read(fixture("hello-wf.events.jsonl")).expect("read golden");

    // --- run once: must byte-match the committed golden ---
    let tmp1 = tempdir();
    let produced = run_workflow_into(&tmp1);
    assert_eq!(
        String::from_utf8_lossy(&produced),
        String::from_utf8_lossy(&golden),
        "events.jsonl drifted from committed golden hello-wf.events.jsonl.\n\
         NEGATIVE ORACLE: deleting any one line from the golden makes this diff non-empty."
    );

    // --- run twice: byte-identical across independent runs (determinism gate) ---
    let tmp2 = tempdir();
    let produced2 = run_workflow_into(&tmp2);
    assert_eq!(produced, produced2, "two runs were not byte-identical");

    // --- structural assertions over the journal ---
    let lines: Vec<serde_json::Value> = produced
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).expect("each journal line is valid JSON"))
        .collect();

    // seq is monotonic starting at 0 (jq -c 'select(.event=="run.ended")|.status' == "success").
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(
            line["seq"].as_u64(),
            Some(i as u64),
            "seq not monotonic at line {i}: {line}"
        );
        assert_eq!(
            line["ts"].as_str(),
            Some("0"),
            "ts not zeroed under fixed seam"
        );
    }

    // run.started is first, run.ended is last and reports success.
    assert_eq!(lines.first().unwrap()["event"], "run.started");
    let last = lines.last().unwrap();
    assert_eq!(last["event"], "run.ended");
    assert_eq!(
        last["status"], "success",
        "run.ended status must be \"success\""
    );

    // The phase sequence is exactly Inventory then Audit, each started+ended.
    let phase_events: Vec<(&str, &str)> = lines
        .iter()
        .filter_map(|l| {
            let ev = l["event"].as_str()?;
            if ev == "phase.started" || ev == "phase.ended" {
                Some((ev, l["phase"].as_str().unwrap_or("")))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        phase_events,
        vec![
            ("phase.started", "Inventory"),
            ("phase.ended", "Inventory"),
            ("phase.started", "Audit"),
            ("phase.ended", "Audit"),
        ]
    );

    // Two checkpoints were journaled: :files (Inventory) then :findings (Audit).
    let checkpoint_keys: Vec<&str> = lines
        .iter()
        .filter(|l| l["event"] == "checkpoint")
        .map(|l| l["key"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(checkpoint_keys, vec!["files", "findings"]);
}

/// The Audit phase reads `(checkpoint :files)` back — i.e. the value recorded in
/// Inventory survives into Audit. We prove it via the *final envelope* the CLI
/// writes (result.json), which echoes both checkpoints; :findings == (count files)
/// can only hold if Audit saw Inventory's 3-element list.
#[test]
fn spike1_checkpoint_read_in_audit_returns_inventory_value() {
    let tmp = tempdir();
    run_workflow_into(&tmp);

    let result_path = tmp.join(RUN_ID).join("result.json");
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&result_path).expect("read result.json"))
            .expect("result.json is valid JSON");

    assert_eq!(result["status"], "success");
    // Inventory checkpointed ["a.php","b.php","c.php"]; Audit read it back.
    assert_eq!(
        result["files"],
        serde_json::json!(["a.php", "b.php", "c.php"]),
        "Audit did not read back the Inventory :files checkpoint"
    );
    // :findings = (count files) = 3, computed in Audit from the read-back value.
    assert_eq!(result["findings"], serde_json::json!(3));
}

/// Minimal, dependency-free temp dir (avoids pulling `tempfile` into dev-deps just
/// for this test). Lands under the target dir so `cargo` cleanup sweeps it.
fn tempdir() -> PathBuf {
    let mut p = std::env::temp_dir();
    let nonce = format!(
        "sema-wf-spike1-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    p.push(nonce);
    std::fs::create_dir_all(&p).expect("create temp dir");
    p
}
