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

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
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

    #[cfg(unix)]
    fn install_blocking_external_diff(&self) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let helper = self.dir.join("external-diff-helper.sh");
        let started = self.dir.join("external-diff-started");
        let descendant = self.dir.join("external-diff-descendant");
        std::fs::write(
            &helper,
            "#!/bin/sh\nprintf started > external-diff-started\n( sleep 1; printf leaked > external-diff-descendant ) &\nwait\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&helper).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&helper, permissions).unwrap();
        std::fs::write(self.dir.join(".gitattributes"), "a.txt diff=sema-cancel\n").unwrap();

        let output = std::process::Command::new("git")
            .args([
                "config",
                "diff.sema-cancel.command",
                "./external-diff-helper.sh",
            ])
            .current_dir(&self.dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "failed to configure external diff: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        (started, descendant)
    }

    #[cfg(unix)]
    fn install_exiting_git_wrapper(&self) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let bin = self.dir.join("fake-bin");
        std::fs::create_dir(&bin).unwrap();
        let wrapper = bin.join("git");
        std::fs::write(
            &wrapper,
            "#!/bin/sh\nparent=$$\n/bin/sh -c '\nparent=$1\nwhile [ \"$(ps -o ppid= -p $$ | tr -d \" \" )\" = \"$parent\" ]; do sleep 0.01; done\nprintf exited > direct-child-exited\nsleep 1\nprintf leaked > inherited-pipe-descendant\n' child \"$parent\" &\nexit 0\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&wrapper, permissions).unwrap();
        bin
    }

    /// A fake `git` on PATH that forks a descendant into its own process group
    /// (a delayed marker writer) and then floods stdout far past the (lowered)
    /// output cap. The over-cap drain must SIGKILL the whole group — reaping the
    /// descendant before it can write its marker — and reject the task with the
    /// structured over-cap error. Returns the wrapper's PATH dir and the marker a
    /// direct-child-only kill would leak.
    #[cfg(unix)]
    fn install_over_cap_git_wrapper(&self) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let bin = self.dir.join("over-cap-fake-bin");
        std::fs::create_dir(&bin).unwrap();
        let wrapper = bin.join("git");
        let descendant = self.dir.join("over-cap-descendant");
        std::fs::write(
            &wrapper,
            "#!/bin/sh\n( sleep 1; printf leaked > over-cap-descendant ) &\ni=0\nwhile [ $i -lt 8192 ]; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n'; i=$((i+1)); done\nwait\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&wrapper, permissions).unwrap();
        (bin, descendant)
    }

    #[cfg(unix)]
    fn install_escaped_git_wrapper(&self) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let bin = self.dir.join("escaped-fake-bin");
        std::fs::create_dir(&bin).unwrap();
        let wrapper = bin.join("git");
        std::fs::write(
            &wrapper,
            "#!/bin/sh\n\"$SEMA_GIT_HELPER_BIN\" --exact escaped_process_group_pipe_holder --ignored --nocapture &\nexit 0\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&wrapper, permissions).unwrap();
        bin
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

/// Cancelling a runtime `git/diff` kills the entire Git process group. The
/// external diff helper forks a descendant that writes a delayed marker; a
/// direct-child-only kill leaves that descendant alive and fails this test.
#[cfg(unix)]
#[test]
#[serial]
fn git_diff_cancel_kills_external_diff_descendant() {
    if !git_available() {
        eprintln!("skipping git_diff_cancel_kills_external_diff_descendant: no git on PATH");
        return;
    }
    let repo = ScratchRepo::new("cancel-descendant");
    let (started, descendant) = repo.install_blocking_external_diff();
    let _cwd = TestDir::enter(&repo.dir);

    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(
            r#"
            (let ((pending (async/spawn (fn () (git/diff)))))
              (let wait-for-helper ((remaining 4000))
                (cond
                  ((file/exists? "external-diff-started") nil)
                  ((= remaining 0) (error "external diff helper did not start"))
                  (else
                    (async/sleep 5)
                    (wait-for-helper (- remaining 1)))))
              (let ((requested (async/cancel pending))
                    (settled (try (async/await pending) (catch error :cancelled))))
                (list requested settled)))
            "#,
        )
        .expect("cancelled git/diff settles");
    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::keyword("cancelled")])
    );
    assert!(started.exists(), "external diff helper must have started");

    std::thread::sleep(std::time::Duration::from_millis(1_300));
    assert!(
        !descendant.exists(),
        "external diff descendant survived cancellation and wrote its marker"
    );
}

/// The Git leader may exit before a descendant that inherited its output pipes.
/// Cancellation must still target the original Unix process group while that
/// descendant holds the drains open.
#[cfg(unix)]
#[test]
#[serial]
fn git_cancel_after_direct_exit_kills_inherited_pipe_descendant() {
    if !git_available() {
        eprintln!(
            "skipping git_cancel_after_direct_exit_kills_inherited_pipe_descendant: no git on PATH"
        );
        return;
    }
    let repo = ScratchRepo::new("cancel-after-exit");
    let bin = repo.install_exiting_git_wrapper();
    let prior_path = std::env::var_os("PATH").unwrap_or_default();
    let joined_path =
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&prior_path)))
            .unwrap();
    let _path = EnvVarGuard::set("PATH", joined_path);
    let _cwd = TestDir::enter(&repo.dir);

    let result = Interpreter::new()
        .eval_str_compiled(
            r#"
            (let ((pending (async/spawn (fn () (git/diff)))))
              (let wait-for-direct-exit ((remaining 4000))
                (cond
                  ((file/exists? "direct-child-exited") nil)
                  ((= remaining 0) (error "git wrapper did not exit"))
                  (else
                    (async/sleep 5)
                    (wait-for-direct-exit (- remaining 1)))))
              (let ((requested (async/cancel pending))
                    (settled (try (async/await pending) (catch error :cancelled))))
                (list requested settled)))
            "#,
        )
        .expect("cancelled git wrapper settles");
    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::keyword("cancelled")])
    );

    std::thread::sleep(std::time::Duration::from_millis(1_300));
    assert!(
        !repo.dir.join("inherited-pipe-descendant").exists(),
        "inherited-pipe descendant survived after the direct child exited"
    );
}

/// An inherited-pipe descendant can escape Git's process group. Cancellation
/// must finish bounded cleanup without waiting for that foreign process to
/// close stdout/stderr naturally.
#[cfg(unix)]
#[test]
#[serial]
fn git_cancel_bounds_drains_held_by_escaped_descendant() {
    if !git_available() {
        eprintln!("skipping git_cancel_bounds_drains_held_by_escaped_descendant: no git on PATH");
        return;
    }
    let repo = ScratchRepo::new("cancel-escaped");
    let bin = repo.install_escaped_git_wrapper();
    let prior_path = std::env::var_os("PATH").unwrap_or_default();
    let joined_path =
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&prior_path)))
            .unwrap();
    let ready = repo.dir.join("escaped-helper-ready");
    let pid_path = repo.dir.join("escaped-helper-pid");
    let leaked = repo.dir.join("escaped-helper-natural-release");
    let _path = EnvVarGuard::set("PATH", joined_path);
    let _helper = EnvVarGuard::set("SEMA_GIT_HELPER_BIN", std::env::current_exe().unwrap());
    let _ready = EnvVarGuard::set("SEMA_GIT_ESCAPED_READY", &ready);
    let _pid = EnvVarGuard::set("SEMA_GIT_ESCAPED_PID", &pid_path);
    let _leaked = EnvVarGuard::set("SEMA_GIT_ESCAPED_LEAKED", &leaked);
    let _cwd = TestDir::enter(&repo.dir);

    let interp = Interpreter::new();
    let started = std::time::Instant::now();
    let result = interp
        .eval_str_compiled(
            r#"
            (let ((pending (async/spawn (fn () (git/diff)))))
              (let wait-for-escaped-helper ((remaining 4000))
                (cond
                  ((file/exists? "escaped-helper-ready") nil)
                  ((= remaining 0) (error "escaped helper did not start"))
                  (else
                    (async/sleep 5)
                    (wait-for-escaped-helper (- remaining 1)))))
              (let ((requested (async/cancel pending))
                    (settled (try (async/await pending) (catch error :cancelled))))
                (list requested settled)))
            "#,
        )
        .expect("cancelled git with escaped descendant settles");
    drop(interp);
    let elapsed = started.elapsed();

    let helper_pid: i32 = std::fs::read_to_string(&pid_path).unwrap().parse().unwrap();
    // SAFETY: the helper writes its own pid after successfully entering a new
    // session; this test owns that short-lived helper process.
    unsafe {
        libc::kill(helper_pid, libc::SIGKILL);
    }
    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::keyword("cancelled")])
    );
    assert!(
        elapsed < std::time::Duration::from_millis(1_500),
        "escaped-pipe cancellation plus interpreter shutdown took {elapsed:?}"
    );
    assert!(
        !leaked.exists(),
        "cancellation waited for an escaped descendant to release inherited pipes"
    );
}

/// A runtime `git/*` whose subprocess floods stdout past the pre-dispatch output
/// cap must kill the entire Git process group (via the same hook cancellation
/// uses) and reject the task with a structured over-cap error — never buffer a
/// hostile pipe to exhaustion. The fake `git` forks a descendant that would
/// write a delayed marker; a direct-child-only kill would leak it.
#[cfg(unix)]
#[test]
#[serial]
fn git_output_over_cap_kills_group_and_errors() {
    if !git_available() {
        eprintln!("skipping git_output_over_cap_kills_group_and_errors: no git on PATH");
        return;
    }
    let repo = ScratchRepo::new("over-cap");
    let (bin, descendant) = repo.install_over_cap_git_wrapper();
    let prior_path = std::env::var_os("PATH").unwrap_or_default();
    let joined_path =
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&prior_path)))
            .unwrap();
    let _path = EnvVarGuard::set("PATH", joined_path);
    let _cwd = TestDir::enter(&repo.dir);

    // Lower the per-pipe cap so the wrapper's output trips it within one chunk —
    // no multi-megabyte fixture required. Cleared before any assertion can unwind.
    sema_stdlib::set_git_max_output_bytes_override(Some(64));
    let interp = Interpreter::new();
    let result = interp.eval_str_compiled("(await (async/spawn (fn () (git/diff))))");
    sema_stdlib::set_git_max_output_bytes_override(None);

    let err = result.expect_err("git output over the cap must reject the task");
    let message = err.to_string();
    assert!(
        message.contains("output exceeded") && message.contains("64"),
        "expected a structured over-cap error naming the cap, got: {message}"
    );

    // The group SIGKILL must also reap the wrapper's forked descendant before its
    // delayed marker write — a direct-child-only kill would let it leak.
    std::thread::sleep(std::time::Duration::from_millis(1_300));
    assert!(
        !descendant.exists(),
        "over-cap cleanup left a process-group descendant alive"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess fixture invoked by git_cancel_bounds_drains_held_by_escaped_descendant"]
fn escaped_process_group_pipe_holder() {
    // SAFETY: this isolated helper process intentionally leaves the Git-owned
    // process group to exercise cancellation of foreign inherited-pipe holders.
    let session = unsafe { libc::setsid() };
    assert_ne!(
        session,
        -1,
        "setsid failed: {}",
        std::io::Error::last_os_error()
    );

    let ready = std::env::var_os("SEMA_GIT_ESCAPED_READY").unwrap();
    let pid_path = std::env::var_os("SEMA_GIT_ESCAPED_PID").unwrap();
    let leaked = std::env::var_os("SEMA_GIT_ESCAPED_LEAKED").unwrap();
    std::fs::write(pid_path, std::process::id().to_string()).unwrap();
    std::fs::write(ready, "ready").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(10));
    std::fs::write(leaked, "natural release").unwrap();
}
