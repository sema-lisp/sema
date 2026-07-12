//! Async-offload coverage for `zip/*`/`tar/*` (WP-ARCHIVE-PDF-PATCH), `pdf/*`,
//! and `patch/apply-file`.
//!
//! `archive.rs`, `pdf.rs`, and `diff.rs` now branch on `in_async_context()`:
//! each builtin's real work (already factored into a plain `*_work`
//! function) runs synchronously at top level, or — inside `async/spawn` —
//! offloads onto the process-wide I/O pool via `fs_offload` (io.rs) and
//! yields `AwaitIo`, so a large archive/PDF/patch operation doesn't block the
//! VM thread (and every sibling task) for its whole duration. At top level
//! every builtin keeps the original synchronous shape.
//!
//! Every fixture here is tiny — no real disk/CPU latency needed for these
//! tests to be meaningful: the offload yields `AwaitIo` the instant it's
//! called (before the `spawn_blocking` closure has any chance to run), so a
//! zero-delay sibling task reliably completes first — the same mechanism
//! `db_async_test.rs`/`kv_async_test.rs`/`git_async_test.rs` rely on.
//! Ordering is asserted via channel receive order — never a wall-clock
//! duration assert.

#![cfg(not(target_arch = "wasm32"))]

use sema_core::Value;
use sema_eval::Interpreter;

/// A unique temp scratch dir for one test, removed on drop (also on panic).
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("sema-archive-pdf-patch-async-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().to_string()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn pdf_fixture() -> String {
    format!(
        "{}/tests/fixtures/sample-invoice.pdf",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Run `program`, expecting it to return a 2-element list of strings from a
/// `(list (channel/recv out) (channel/recv out))` race between an offloaded
/// op (tagged `winner_if_blocking`) and a zero-delay sibling (tagged
/// `"sibling"`). Asserts the sibling won — i.e. the offloaded op actually
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

// === archive (zip/tar) family ===

#[test]
fn archive_async_lets_sibling_run_first() {
    let dir = TempDir::new("sib-order");
    let f1 = dir.path("a.txt");
    let f2 = dir.path("b.txt");
    std::fs::write(&f1, b"alpha contents").unwrap();
    std::fs::write(&f2, b"beta contents").unwrap();
    let out_zip = dir.path("bundle.zip");

    let program = format!(
        r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (zip/create "{out_zip}" (list "{f1}" "{f2}"))
                (zip/list "{out_zip}")
                (channel/send out "archive")))
              (async/spawn (fn () (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
    "#
    );
    assert_sibling_wins(&program, "archive");
}

#[test]
fn archive_async_zip_matches_sync() {
    let interp = Interpreter::new();
    let dir = TempDir::new("zip-parity");
    let f1 = dir.path("one.txt");
    let f2 = dir.path("two.txt");
    std::fs::write(&f1, b"first").unwrap();
    std::fs::write(&f2, b"second").unwrap();

    let sync_zip = dir.path("sync.zip");
    let sync_v = interp
        .eval_str_compiled(&format!(
            r#"
            (let ((n (zip/create "{sync_zip}" (list "{f1}" "{f2}"))))
              (list n (zip/list "{sync_zip}")))
            "#
        ))
        .expect("sync zip create+list");

    let async_zip = dir.path("async.zip");
    let async_v = interp
        .eval_str_compiled(&format!(
            r#"
            (await (async/spawn (fn ()
              (let ((n (zip/create "{async_zip}" (list "{f1}" "{f2}"))))
                (list n (zip/list "{async_zip}"))))))
            "#
        ))
        .expect("async zip create+list");

    let sync_parts = sync_v.as_list().expect("list").to_vec();
    let async_parts = async_v.as_list().expect("list").to_vec();
    assert_eq!(sync_parts[0], Value::int(2));
    assert_eq!(async_parts[0], Value::int(2));

    let mut sync_names: Vec<String> = sync_parts[1]
        .as_list()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let mut async_names: Vec<String> = async_parts[1]
        .as_list()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    sync_names.sort();
    async_names.sort();
    assert_eq!(sync_names, async_names);
    assert_eq!(
        sync_names,
        vec!["one.txt".to_string(), "two.txt".to_string()]
    );
}

#[test]
fn archive_async_tar_matches_sync() {
    let interp = Interpreter::new();
    let dir = TempDir::new("tar-parity");
    let f1 = dir.path("alpha.txt");
    let f2 = dir.path("beta.txt");
    std::fs::write(&f1, b"alpha payload").unwrap();
    std::fs::write(&f2, b"beta payload").unwrap();

    let sync_tar = dir.path("sync.tar.gz");
    let sync_dest = dir.path("sync-out");
    let sync_v = interp
        .eval_str_compiled(&format!(
            r#"
            (let ((n (tar/create "{sync_tar}" (list "{f1}" "{f2}"))))
              (list n (tar/extract "{sync_tar}" "{sync_dest}")))
            "#
        ))
        .expect("sync tar create+extract");

    let async_tar = dir.path("async.tar.gz");
    let async_dest = dir.path("async-out");
    let async_v = interp
        .eval_str_compiled(&format!(
            r#"
            (await (async/spawn (fn ()
              (let ((n (tar/create "{async_tar}" (list "{f1}" "{f2}"))))
                (list n (tar/extract "{async_tar}" "{async_dest}"))))))
            "#
        ))
        .expect("async tar create+extract");

    let sync_parts = sync_v.as_list().expect("list").to_vec();
    let async_parts = async_v.as_list().expect("list").to_vec();
    assert_eq!(sync_parts, vec![Value::int(2), Value::int(2)]);
    assert_eq!(async_parts, vec![Value::int(2), Value::int(2)]);

    let sync_alpha = std::fs::read(dir.0.join("sync-out").join("alpha.txt")).unwrap();
    let async_alpha = std::fs::read(dir.0.join("async-out").join("alpha.txt")).unwrap();
    assert_eq!(sync_alpha, b"alpha payload");
    assert_eq!(async_alpha, b"alpha payload");
    let sync_beta = std::fs::read(dir.0.join("sync-out").join("beta.txt")).unwrap();
    let async_beta = std::fs::read(dir.0.join("async-out").join("beta.txt")).unwrap();
    assert_eq!(sync_beta, b"beta payload");
    assert_eq!(async_beta, b"beta payload");
}

// === pdf family ===

#[test]
fn pdf_async_lets_sibling_run_first() {
    let path = pdf_fixture();
    let program = format!(
        r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (pdf/extract-text "{path}")
                (pdf/page-count "{path}")
                (channel/send out "pdf")))
              (async/spawn (fn () (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
    "#
    );
    assert_sibling_wins(&program, "pdf");
}

#[test]
fn pdf_async_matches_sync() {
    let interp = Interpreter::new();
    let path = pdf_fixture();

    let sync_v = interp
        .eval_str_compiled(&format!(
            r#"
            (list (pdf/extract-text "{path}")
                  (pdf/extract-text-pages "{path}")
                  (pdf/page-count "{path}")
                  (pdf/metadata "{path}"))
            "#
        ))
        .expect("sync pdf reads");
    let async_v = interp
        .eval_str_compiled(&format!(
            r#"
            (await (async/spawn (fn ()
              (list (pdf/extract-text "{path}")
                    (pdf/extract-text-pages "{path}")
                    (pdf/page-count "{path}")
                    (pdf/metadata "{path}")))))
            "#
        ))
        .expect("async pdf reads");

    assert_eq!(sync_v, async_v);
    let parts = sync_v.as_list().expect("list");
    let text = parts[0].as_str().expect("text");
    assert!(text.contains("Invoice"), "got: {text}");
    assert_eq!(parts[2], Value::int(1));
}

// === patch/apply-file family ===

/// Sync-context regression baseline: `patch/apply-file` had NO prior test
/// coverage at all (sync or async) before this WP — this is that missing
/// baseline, asserted first against the synchronous path.
#[test]
fn patch_apply_file_sync_regression() {
    let interp = Interpreter::new();
    let dir = TempDir::new("patch-sync");
    let target = dir.path("target.txt");
    std::fs::write(&target, "line1\nline2\nline3\n").unwrap();

    let result = interp
        .eval_str_compiled(&format!(
            r#"
            (let ((patch (diff/unified "line1\nline2\nline3\n" "line1\nCHANGED\nline3\n")))
              (patch/apply-file "{target}" patch))
            "#
        ))
        .expect("sync patch/apply-file");
    assert_eq!(result, Value::int(1), "one hunk applied");
    let patched = std::fs::read_to_string(&target).unwrap();
    assert_eq!(patched, "line1\nCHANGED\nline3\n");
}

#[test]
fn patch_async_lets_sibling_run_first() {
    let dir = TempDir::new("patch-sib-order");
    let target = dir.path("target.txt");
    std::fs::write(&target, "line1\nline2\nline3\n").unwrap();

    let program = format!(
        r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (let ((patch (diff/unified "line1\nline2\nline3\n" "line1\nCHANGED\nline3\n")))
                  (patch/apply-file "{target}" patch))
                (channel/send out "patch")))
              (async/spawn (fn () (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
    "#
    );
    assert_sibling_wins(&program, "patch");

    let patched = std::fs::read_to_string(&target).unwrap();
    assert_eq!(patched, "line1\nCHANGED\nline3\n");
}

#[test]
fn patch_async_matches_sync() {
    let interp = Interpreter::new();
    let dir = TempDir::new("patch-parity");
    let sync_target = dir.path("sync.txt");
    let async_target = dir.path("async.txt");
    std::fs::write(&sync_target, "line1\nline2\nline3\n").unwrap();
    std::fs::write(&async_target, "line1\nline2\nline3\n").unwrap();

    let sync_v = interp
        .eval_str_compiled(&format!(
            r#"
            (let ((patch (diff/unified "line1\nline2\nline3\n" "line1\nCHANGED\nline3\n")))
              (patch/apply-file "{sync_target}" patch))
            "#
        ))
        .expect("sync patch/apply-file");
    let async_v = interp
        .eval_str_compiled(&format!(
            r#"
            (await (async/spawn (fn ()
              (let ((patch (diff/unified "line1\nline2\nline3\n" "line1\nCHANGED\nline3\n")))
                (patch/apply-file "{async_target}" patch)))))
            "#
        ))
        .expect("async patch/apply-file");

    assert_eq!(sync_v, async_v);
    assert_eq!(sync_v, Value::int(1));
    let sync_patched = std::fs::read_to_string(&sync_target).unwrap();
    let async_patched = std::fs::read_to_string(&async_target).unwrap();
    assert_eq!(sync_patched, async_patched);
    assert_eq!(async_patched, "line1\nCHANGED\nline3\n");
}
