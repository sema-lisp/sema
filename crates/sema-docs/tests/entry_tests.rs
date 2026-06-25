use sema_docs::{build_index, dedupe, parse_entry, validate, DocIndex};
use std::path::PathBuf;

fn p() -> PathBuf {
    PathBuf::from("strings/split.md")
}

#[test]
fn parses_entry_with_params_and_example() {
    let src = "---\nname: \"string/split\"\nparams: [{ name: s, type: string }, { name: sep, type: string }]\nreturns: \"list<string>\"\nsee_also: [\"string/join\"]\n---\nSplit `s` on `sep`.\n\n```sema\n(string/split \"a,b\" \",\") ; => (\"a\" \"b\")\n```\n";
    let e = parse_entry(&p(), src, "strings", false).unwrap();
    assert_eq!(e.name, "string/split");
    assert_eq!(e.module, "strings");
    assert_eq!(e.params.len(), 2);
    assert_eq!(e.params[1].name, "sep");
    assert_eq!(e.params[0].ty.as_deref(), Some("string"));
    assert_eq!(e.returns.as_deref(), Some("list<string>"));
    assert_eq!(e.see_also, vec!["string/join"]);
    assert_eq!(e.summary, "Split `s` on `sep`.");
    assert_eq!(e.examples.len(), 1);
}

#[test]
fn summary_skips_leading_signature_block() {
    // async/* style: a signature code block precedes the prose.
    let src = "---\nname: \"async/spawn\"\n---\n```sema\n(async/spawn thunk) → promise\n```\n\nSpawn a zero-argument function as an async task.\n";
    let e = parse_entry(&p(), src, "concurrency", false).unwrap();
    assert_eq!(
        e.summary,
        "Spawn a zero-argument function as an async task."
    );
}

#[test]
fn round_trips_through_index_json() {
    let src = "---\nname: \"+\"\n---\nAdd numbers.\n";
    let e = parse_entry(&p(), src, "arithmetic", false).unwrap();
    let json = serde_json::to_string(&build_index(vec![e])).unwrap();
    let back: DocIndex = serde_json::from_str(&json).unwrap();
    assert_eq!(back.entries[0].name, "+");
    assert_eq!(back.entries[0].summary, "Add numbers.");
}

#[test]
fn db_exec_batch_warns_about_sql_injection() {
    // STD-10: db/exec-batch runs raw SQL with no parameterization. Its docs must
    // warn it is for static SQL only and steer user input to parameterized db/exec.
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("entries/stdlib/sqlite/db-exec-batch.md");
    let src = std::fs::read_to_string(&path).unwrap();
    let e = parse_entry(
        &PathBuf::from("sqlite/db-exec-batch.md"),
        &src,
        "sqlite",
        false,
    )
    .unwrap();
    assert_eq!(e.name, "db/exec-batch");
    let lower = e.body.to_lowercase();
    assert!(
        lower.contains("static sql only"),
        "db/exec-batch docs must state it is static SQL only"
    );
    assert!(
        lower.contains("db/exec"),
        "db/exec-batch docs must steer user input to parameterized db/exec"
    );
    // The static-SQL warning is the headline, so it must surface in the summary too.
    assert!(e.summary.to_lowercase().contains("static sql only"));
}

#[test]
fn validate_and_dedupe() {
    // Cross-module: same name in different modules is kept.
    let a = parse_entry(&p(), "---\nname: \"length\"\n---\nLen.\n", "lists", false).unwrap();
    let b = parse_entry(
        &p(),
        "---\nname: \"length\"\n---\nLen v.\n",
        "vectors",
        false,
    )
    .unwrap();
    let mut v = vec![a, b];
    let warns = dedupe(&mut v);
    assert_eq!(v.len(), 2);
    assert!(warns.is_empty());
    validate(&v, true).unwrap();

    // Same-module: duplicate name in the same module is dropped.
    let c = parse_entry(&p(), "---\nname: \"dup\"\n---\nFirst.\n", "m", false).unwrap();
    let d = parse_entry(&p(), "---\nname: \"dup\"\n---\nSecond.\n", "m", false).unwrap();
    let mut v2 = vec![c, d];
    let warns2 = dedupe(&mut v2);
    assert_eq!(v2.len(), 1);
    assert!(warns2[0].contains("dropped duplicate `dup` in module `m`"));

    // Empty summary warning / strict error.
    let bare = parse_entry(&p(), "---\nname: \"x\"\n---\n", "m", false).unwrap();
    assert!(!validate(std::slice::from_ref(&bare), false)
        .unwrap()
        .is_empty()); // warn
    assert!(validate(&[bare], true).is_err()); // strict error
}

#[test]
fn signature_only_body_derives_summary_from_the_signature() {
    // A body that is ONLY a signature block (no prose) must still get a non-empty
    // summary (else it would fail the strict gate despite being documented).
    let src = "---\nname: \"x/y\"\n---\n```sema\n(x/y a b) → result\n```\n";
    let e = parse_entry(&PathBuf::from("x/y.md"), src, "m", false).unwrap();
    assert_eq!(e.summary, "(x/y a b) → result");
}

#[test]
fn leading_heading_is_skipped_for_summary() {
    let src = "---\nname: \"z\"\n---\n## Overview\n\nDoes the thing.\n";
    let e = parse_entry(&PathBuf::from("z.md"), src, "m", false).unwrap();
    assert_eq!(e.summary, "Does the thing.");
}
