//! Async-offload coverage for `git/*` (WP-GIT).
//!
//! Every builtin in `crates/sema-stdlib/src/git.rs` now branches on
//! `in_async_context()` and, inside `async/spawn`, offloads the `git`
//! subprocess through `git_offload`/`git_stdout_async` — mirroring
//! `shell_async` (system.rs) — instead of blocking the VM thread (and every
//! sibling task) on `std::process::Command::output()` for the subprocess's
//! whole duration. The parsing/decode logic is shared between the sync and
//! async paths, so this suite mostly proves parity plus the scheduler-not-
//! stalled property.
//!
//! `git/*` has no `-C <dir>` — it always operates on the process's current
//! working directory — so these tests chdir into a scratch repo for their
//! duration and restore the original cwd on drop. `#[serial]` because
//! `std::env::set_current_dir` is process-global and `cargo test` runs many
//! tests concurrently within one process.
//!
//! All tests skip gracefully (rather than fail) when no `git` binary is on
//! PATH, matching `git.rs`'s own unit tests.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};

use sema_core::Value;
use sema_eval::Interpreter;
use serial_test::serial;

/// True when a `git` binary is available.
fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// chdir into `dir` for the guard's lifetime, restoring the original cwd on
/// drop (also on panic/early return).
struct TestDir {
    prev: PathBuf,
}

impl TestDir {
    fn enter(dir: &Path) -> Self {
        let prev = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(dir).expect("chdir into scratch repo");
        TestDir { prev }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.prev);
    }
}

/// A throwaway repo with one commit (`a.txt`), an unstaged modification to
/// it, an untracked file (`b.txt`), and a `.gitignore` covering
/// `ignored.txt` — enough surface for every `git/*` builtin. Removed on drop.
struct ScratchRepo {
    dir: PathBuf,
}

impl ScratchRepo {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("sema-git-async-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();

        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap_or_else(|e| panic!("git {args:?} failed to launch: {e}"));
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(dir.join("a.txt"), "hello\nworld\n").unwrap();
        std::fs::write(dir.join("b.txt"), "new\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored.txt\n").unwrap();

        ScratchRepo { dir }
    }
}

impl Drop for ScratchRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// === Scheduler-not-stalled: a sibling task completes while git ops are in flight ===
//
// Pre-conversion, `git()`'s `.output()` never yields, so the ENTIRE loop below
// (25 subprocess launches, each blocking the VM thread) plus its trailing
// `channel/send` runs inside one uninterruptible scheduler step — "git" always
// wins the channel race, regardless of the sibling's sleep. Post-conversion
// each `git/current-branch` call parks on `AwaitIo`, giving the scheduler many
// chances to run the sibling — which reliably completes first given 25 real
// subprocess launches racing a single 20 ms sleep. Ordering is asserted via
// channel receive order — never a wall-clock duration assert.
#[test]
#[serial]
fn git_async_lets_sibling_run_first() {
    if !git_available() {
        eprintln!("skipping git_async_lets_sibling_run_first: no git on PATH");
        return;
    }
    let repo = ScratchRepo::new("sibling-order");
    let _cwd = TestDir::enter(&repo.dir);

    let interp = Interpreter::new();
    let program = r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (let loop ((i 0))
                  (when (< i 25)
                    (git/current-branch)
                    (loop (+ i 1))))
                (channel/send out "git")))
              (async/spawn (fn () (sleep 20) (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
    "#;
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
        received.len(),
        2,
        "expected two channel receives: {received:?}"
    );
    let sibling_pos = received
        .iter()
        .position(|v| v == "sibling")
        .expect("sibling value received");
    let git_pos = received
        .iter()
        .position(|v| v == "git")
        .expect("git value received");
    assert!(
        sibling_pos < git_pos,
        "sibling task must complete while the offloaded git loop is in flight \
         (pre-conversion the git loop always wins), got {received:?}"
    );
}

/// `git/root` returned by `async/spawn` matches the synchronous value.
#[test]
#[serial]
fn git_root_async_matches_sync() {
    if !git_available() {
        eprintln!("skipping git_root_async_matches_sync: no git on PATH");
        return;
    }
    let repo = ScratchRepo::new("root");
    let _cwd = TestDir::enter(&repo.dir);

    let interp = Interpreter::new();
    let sync_v = interp
        .eval_str_compiled("(git/root)")
        .expect("git/root sync");
    let async_v = interp
        .eval_str_compiled("(await (async/spawn (fn () (git/root))))")
        .expect("git/root async");
    assert_eq!(sync_v, async_v);

    let root = sync_v.as_str().expect("string").to_string();
    let canon_root = std::fs::canonicalize(&root).expect("canonical root");
    let canon_repo = std::fs::canonicalize(&repo.dir).expect("canonical repo dir");
    assert_eq!(canon_root, canon_repo);
}

/// Every remaining `git/*` builtin returns the identical value on the async
/// path as on the sync path, in a repo with a committed file, an unstaged
/// modification, an untracked file, and a `.gitignore` — exercising the
/// `status`/`diff`/`log`/`check-ignore` parsing each builtin's `decode`
/// closure duplicates for the offloaded path.
#[test]
#[serial]
fn git_all_builtins_async_matches_sync() {
    if !git_available() {
        eprintln!("skipping git_all_builtins_async_matches_sync: no git on PATH");
        return;
    }
    let repo = ScratchRepo::new("parity");
    let _cwd = TestDir::enter(&repo.dir);
    let interp = Interpreter::new();

    let parity = |sync_src: &str, async_src: &str, label: &str| -> Value {
        let sync_v = interp
            .eval_str_compiled(sync_src)
            .unwrap_or_else(|e| panic!("{label} sync: {e}"));
        let async_v = interp
            .eval_str_compiled(async_src)
            .unwrap_or_else(|e| panic!("{label} async: {e}"));
        assert_eq!(sync_v, async_v, "{label}: sync/async mismatch");
        sync_v
    };

    parity(
        "(git/current-branch)",
        "(await (async/spawn (fn () (git/current-branch))))",
        "git/current-branch",
    );

    let status = parity(
        "(git/status)",
        "(await (async/spawn (fn () (git/status))))",
        "git/status",
    );
    assert!(
        !status.as_list().unwrap().is_empty(),
        "repo has changes; status must be non-empty"
    );

    let changed = parity(
        "(git/changed-files)",
        "(await (async/spawn (fn () (git/changed-files))))",
        "git/changed-files",
    );
    assert!(
        !changed.as_list().unwrap().is_empty(),
        "repo has changes; changed-files must be non-empty"
    );

    parity(
        "(git/diff)",
        "(await (async/spawn (fn () (git/diff))))",
        "git/diff",
    );
    parity(
        r#"(git/diff "a.txt")"#,
        r#"(await (async/spawn (fn () (git/diff "a.txt"))))"#,
        "git/diff a.txt",
    );

    let diff_files = parity(
        "(git/diff-files)",
        "(await (async/spawn (fn () (git/diff-files))))",
        "git/diff-files",
    );
    assert_eq!(diff_files, Value::list(vec![Value::string("a.txt")]));

    let recent = parity(
        "(git/recent-files)",
        "(await (async/spawn (fn () (git/recent-files))))",
        "git/recent-files",
    );
    assert_eq!(recent, Value::list(vec![Value::string("a.txt")]));

    let ignored = parity(
        r#"(git/ignore-matches? "ignored.txt")"#,
        r#"(await (async/spawn (fn () (git/ignore-matches? "ignored.txt"))))"#,
        "git/ignore-matches? ignored.txt",
    );
    assert_eq!(ignored, Value::bool(true));

    let tracked = parity(
        r#"(git/ignore-matches? "a.txt")"#,
        r#"(await (async/spawn (fn () (git/ignore-matches? "a.txt"))))"#,
        "git/ignore-matches? a.txt",
    );
    assert_eq!(tracked, Value::bool(false));
}

/// A non-zero exit (running a git subcommand outside any repo) must reject the
/// task with the byte-identical message the sync path's `SemaError` would
/// display — not hang, not lose the error text.
#[test]
#[serial]
fn git_async_error_matches_sync_outside_repo() {
    if !git_available() {
        eprintln!("skipping git_async_error_matches_sync_outside_repo: no git on PATH");
        return;
    }
    // A fresh empty dir that is (almost certainly) not inside any git repo —
    // unlike the sema checkout itself, a bare temp dir has no ancestor repo.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("sema-git-async-norepo-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    let _cwd = TestDir::enter(&dir);

    let interp = Interpreter::new();
    let sync_err = interp
        .eval_str_compiled("(git/root)")
        .expect_err("git/root outside a repo must fail")
        .to_string();
    let async_err = interp
        .eval_str_compiled("(await (async/spawn (fn () (git/root))))")
        .expect_err("git/root outside a repo must fail (async)")
        .to_string();
    assert!(
        async_err.contains(&sync_err),
        "async rejection must embed the byte-identical sync error message\n  sync:  {sync_err}\n  async: {async_err}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
