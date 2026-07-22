//! Async-offload + bounded-CPU coverage for the `diff/*` and secret/PII
//! (`secret/*`, `pii/*`, `redact/*`, `hash/digest`) families (B8, ledger rows
//! R03/R13) and the csv/markup/crypto families (B9, ledger row R21).
//!
//! Two contracts are asserted:
//!
//! * **Offloaded arms (`QUARANTINED-BOUNDED`)** — `diff/unified`, the
//!   `secret/detect`/`secret/redact`/`pii/detect`/`redact/spans` scans, and
//!   `csv/parse`(`-maps`) / `html/parse`/`select`/`text`/`select-text` run a
//!   super-linear or heavy CPU pass, so inside a runtime quantum they capture a
//!   per-input byte cap BEFORE dispatch and offload through `quarantined_compute`
//!   (`io.rs`). The offload yields `AwaitIo` the instant it is called, so a
//!   zero-delay sibling task reliably completes first (the same mechanism the
//!   archive/pdf/patch and db/kv/git async suites rely on). Ordering is asserted
//!   via channel receive order, never a wall-clock duration.
//!
//! * **Synchronous split arms (`SYNCHRONOUS-PROOF`)** — `diff/stat`/`diff/hunks`/
//!   `diff/parse`/`diff/apply`, `hash/digest`, `csv/encode`, the `markdown/*`
//!   helpers, and the `crypto.rs` hashing/base64 ops are O(input), so they stay
//!   synchronous but are capped by a pre-dispatch input-byte (and, for the patch
//!   consumers, hunk-count; for `csv/encode`, row-count) bound inside a quantum:
//!   bounded input ⇒ bounded VM-thread CPU, never a fake async wrap over unbounded
//!   work.
//!
//! Every capped op rejects one byte over the captured limit BEFORE it snapshots
//! or dispatches the input, so the rejected path allocates nothing extra. The
//! cap is lowered for these tests via the `set_*_input_byte_cap_override` seams
//! (clamped to the hard ceiling) so a boundary can be exercised without a
//! multi-megabyte literal.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;

/// Run `program`, expecting a `(list (channel/recv out) (channel/recv out))`
/// race between an offloaded op (tagged `winner_if_blocking`) and a zero-delay
/// sibling (tagged `"sibling"`). Asserts the sibling won — i.e. the offloaded op
/// yielded instead of blocking the VM thread for its whole duration.
fn assert_sibling_wins(program: &str, winner_if_blocking: &str) {
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(program)
        .expect("sibling-ordering program evaluated");
    let received: Vec<String> = result
        .as_list()
        .expect("channel receives list")
        .iter()
        .map(|v| v.as_str().expect("string value").to_string())
        .collect();
    assert_eq!(
        received,
        vec!["sibling".to_string(), winner_if_blocking.to_string()],
        "sibling task must complete while the offloaded op is in flight \
         (pre-conversion {winner_if_blocking:?} always wins), got {received:?}"
    );
}

// === offloaded arm: sibling-runs-first while the CPU op is in flight ===

#[test]
fn diff_unified_async_lets_sibling_run_first() {
    let program = r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (diff/unified "line1\nline2\nline3\n" "line1\nCHANGED\nline3\n")
                (channel/send out "diff")))
              (async/spawn (fn () (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
    "#;
    assert_sibling_wins(program, "diff");
}

#[test]
fn csv_parse_async_lets_sibling_run_first() {
    let program = r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (csv/parse "a,b,c\nd,e,f\ng,h,i\n")
                (channel/send out "csv")))
              (async/spawn (fn () (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
    "#;
    assert_sibling_wins(program, "csv");
}

#[test]
fn html_select_async_lets_sibling_run_first() {
    let program = r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (html/select "<p class=x>a</p><p>b</p><p class=x>c</p>" "p.x")
                (channel/send out "html")))
              (async/spawn (fn () (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
    "#;
    assert_sibling_wins(program, "html");
}

#[test]
fn secret_detect_async_lets_sibling_run_first() {
    let program = r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (secret/detect "AWS key AKIAIOSFODNN7EXAMPLE and more text here")
                (channel/send out "secret")))
              (async/spawn (fn () (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
    "#;
    assert_sibling_wins(program, "secret");
}

// === offloaded arm: async result matches the synchronous result ===

#[test]
fn csv_parse_async_matches_sync() {
    let interp = Interpreter::new();
    let sync_v = interp
        .eval_str_compiled(r#"(csv/parse "a,b\nc,d\n")"#)
        .expect("sync csv/parse");
    let async_v = interp
        .eval_str_compiled(r#"(await (async/spawn (fn () (csv/parse "a,b\nc,d\n"))))"#)
        .expect("async csv/parse");
    assert_eq!(sync_v, async_v);
    assert_eq!(sync_v.as_list().expect("rows").len(), 2);
}

#[test]
fn html_select_async_matches_sync() {
    let interp = Interpreter::new();
    let html = "<p class=x>alpha</p><p>beta</p><p class=x>gamma</p>";
    let sync_v = interp
        .eval_str_compiled(&format!(r#"(html/select "{html}" "p.x")"#))
        .expect("sync html/select");
    let async_v = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn () (html/select "{html}" "p.x"))))"#
        ))
        .expect("async html/select");
    assert_eq!(sync_v, async_v);
    assert_eq!(sync_v.as_list().expect("matches").len(), 2);
}

#[test]
fn diff_unified_async_matches_sync() {
    let interp = Interpreter::new();
    let sync_v = interp
        .eval_str_compiled(r#"(diff/unified "a\nb\nc\n" "a\nc\nd\n")"#)
        .expect("sync diff/unified");
    let async_v = interp
        .eval_str_compiled(
            r#"(await (async/spawn (fn () (diff/unified "a\nb\nc\n" "a\nc\nd\n"))))"#,
        )
        .expect("async diff/unified");
    assert_eq!(sync_v, async_v);
    assert!(sync_v.as_str().expect("diff string").contains("@@"));
}

#[test]
fn secret_detect_async_matches_sync() {
    let interp = Interpreter::new();
    let text = "key AKIAIOSFODNN7EXAMPLE and me@example.com";
    let sync_v = interp
        .eval_str_compiled(&format!(r#"(secret/detect "{text}")"#))
        .expect("sync secret/detect");
    let async_v = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn () (secret/detect "{text}"))))"#
        ))
        .expect("async secret/detect");
    assert_eq!(sync_v, async_v);
    assert_eq!(sync_v.as_list().expect("findings").len(), 1);
}

#[test]
fn secret_redact_async_matches_sync() {
    let interp = Interpreter::new();
    let text = "key AKIAIOSFODNN7EXAMPLE done";
    let sync_v = interp
        .eval_str_compiled(&format!(r#"(secret/redact "{text}")"#))
        .expect("sync secret/redact");
    let async_v = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn () (secret/redact "{text}"))))"#
        ))
        .expect("async secret/redact");
    assert_eq!(sync_v, async_v);
    assert!(!sync_v.as_str().unwrap().contains("AKIAIOSFODNN7EXAMPLE"));
}

// === cap boundary + one-over rejection, each capped op ===

/// The offloaded `csv/parse` accepts inputs at the captured cap and rejects one
/// byte over BEFORE it snapshots or dispatches (no worker job created).
#[test]
fn csv_parse_cap_rejects_one_over_before_dispatch() {
    sema_stdlib::set_csv_input_byte_cap_override(Some(8));
    let interp = Interpreter::new();

    // Boundary: 8-byte input → accepted (one row, one cell).
    let ok = interp.eval_str_compiled(r#"(await (async/spawn (fn () (csv/parse "12345678"))))"#);
    // One-over: a 9-byte input is rejected before dispatch.
    let over = interp.eval_str_compiled(r#"(await (async/spawn (fn () (csv/parse "123456789"))))"#);
    sema_stdlib::set_csv_input_byte_cap_override(None);

    ok.expect("8-byte input sits at the boundary and is accepted");
    let error = over.expect_err("9-byte input is one over the captured cap");
    assert!(error.to_string().contains("input bytes"), "{error}");
    assert!(error.to_string().contains('9'), "{error}");
    assert!(error.to_string().contains('8'), "{error}");
}

/// The offloaded `html/select` accepts inputs at the captured cap and rejects one
/// byte over before dispatch. The cap applies to the html argument.
#[test]
fn html_select_cap_rejects_one_over_before_dispatch() {
    sema_stdlib::set_markup_input_byte_cap_override(Some(8));
    let interp = Interpreter::new();

    // Boundary: 8-byte html → accepted (empty match list).
    let ok =
        interp.eval_str_compiled(r#"(await (async/spawn (fn () (html/select "12345678" "p"))))"#);
    // One-over: a 9-byte html is rejected before dispatch.
    let over =
        interp.eval_str_compiled(r#"(await (async/spawn (fn () (html/select "123456789" "p"))))"#);
    sema_stdlib::set_markup_input_byte_cap_override(None);

    ok.expect("8-byte html sits at the boundary and is accepted");
    let error = over.expect_err("9-byte html is one over the captured cap");
    assert!(error.to_string().contains("input bytes"), "{error}");
    assert!(error.to_string().contains('9'), "{error}");
    assert!(error.to_string().contains('8'), "{error}");
}

/// The offloaded `diff/unified` accepts inputs at the captured cap and rejects
/// one byte over BEFORE it snapshots or dispatches (no worker job created).
#[test]
fn diff_unified_cap_rejects_one_over_before_dispatch() {
    sema_stdlib::set_diff_input_byte_cap_override(Some(8));
    let interp = Interpreter::new();

    // Boundary: both inputs are exactly 8 bytes → accepted (empty diff).
    let ok = interp.eval_str_compiled(
        r#"(await (async/spawn (fn () (diff/unified "12345678" "12345678"))))"#,
    );
    // One-over: a 9-byte `old` is rejected before dispatch.
    let over = interp.eval_str_compiled(
        r#"(await (async/spawn (fn () (diff/unified "123456789" "12345678"))))"#,
    );
    sema_stdlib::set_diff_input_byte_cap_override(None);

    ok.expect("8-byte inputs sit at the boundary and are accepted");
    let error = over.expect_err("9-byte old is one over the captured cap");
    assert!(error.to_string().contains("old bytes"), "{error}");
    assert!(error.to_string().contains('9'), "{error}");
    assert!(error.to_string().contains('8'), "{error}");
}

/// The synchronous `diff/stat` split arm is capped inside a quantum: it accepts
/// an input at the cap and rejects one byte over.
#[test]
fn diff_stat_sync_cap_rejects_one_over() {
    sema_stdlib::set_diff_input_byte_cap_override(Some(8));
    let interp = Interpreter::new();

    let ok = interp.eval_str_compiled(r#"(await (async/spawn (fn () (diff/stat "12345678"))))"#);
    let over =
        interp.eval_str_compiled(r#"(await (async/spawn (fn () (diff/stat "123456789"))))"#);
    sema_stdlib::set_diff_input_byte_cap_override(None);

    ok.expect("8-byte patch sits at the boundary and is accepted");
    let error = over.expect_err("9-byte patch is one over the captured cap");
    assert!(error.to_string().contains("input bytes"), "{error}");
    assert!(error.to_string().contains('9'), "{error}");
}

/// `diff/apply`'s two-input (content + patch) synchronous cap arm rejects an
/// over-cap `content` before parsing, and accepts inputs at the boundary.
#[test]
fn diff_apply_sync_cap_rejects_one_over() {
    sema_stdlib::set_diff_input_byte_cap_override(Some(8));
    let interp = Interpreter::new();

    // Boundary: 8-byte content, empty patch → content unchanged.
    let ok = interp
        .eval_str_compiled(r#"(await (async/spawn (fn () (diff/apply "12345678" ""))))"#);
    // One-over: a 9-byte content is rejected before the patch is parsed.
    let over = interp
        .eval_str_compiled(r#"(await (async/spawn (fn () (diff/apply "123456789" ""))))"#);
    sema_stdlib::set_diff_input_byte_cap_override(None);

    ok.expect("8-byte content sits at the boundary and is accepted");
    let error = over.expect_err("9-byte content is one over the captured cap");
    assert!(error.to_string().contains("content bytes"), "{error}");
    assert!(error.to_string().contains('9'), "{error}");
}

/// The offloaded `secret/detect` accepts inputs at the captured cap and rejects
/// one byte over before dispatch.
#[test]
fn secret_detect_cap_rejects_one_over_before_dispatch() {
    sema_stdlib::set_secret_input_byte_cap_override(Some(8));
    let interp = Interpreter::new();

    let ok =
        interp.eval_str_compiled(r#"(await (async/spawn (fn () (secret/detect "12345678"))))"#);
    let over =
        interp.eval_str_compiled(r#"(await (async/spawn (fn () (secret/detect "123456789"))))"#);
    sema_stdlib::set_secret_input_byte_cap_override(None);

    ok.expect("8-byte input sits at the boundary and is accepted");
    let error = over.expect_err("9-byte input is one over the captured cap");
    assert!(error.to_string().contains("input bytes"), "{error}");
    assert!(error.to_string().contains('9'), "{error}");
}

/// The synchronous `hash/digest` split arm is capped inside a quantum.
#[test]
fn hash_digest_sync_cap_rejects_one_over() {
    sema_stdlib::set_secret_input_byte_cap_override(Some(8));
    let interp = Interpreter::new();

    let ok =
        interp.eval_str_compiled(r#"(await (async/spawn (fn () (hash/digest "12345678"))))"#);
    let over =
        interp.eval_str_compiled(r#"(await (async/spawn (fn () (hash/digest "123456789"))))"#);
    sema_stdlib::set_secret_input_byte_cap_override(None);

    ok.expect("8-byte input sits at the boundary and is accepted");
    let error = over.expect_err("9-byte input is one over the captured cap");
    assert!(error.to_string().contains("input bytes"), "{error}");
    assert!(error.to_string().contains('9'), "{error}");
}
