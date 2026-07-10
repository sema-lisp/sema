//! Read-only Git helpers (`git/*`).
//!
//! These shell out to the `git` binary and parse its output. They never mutate
//! the repository — no commit, add, checkout, push. Running `git` is a
//! subprocess, so every builtin is gated behind `Caps::PROCESS`.
//!
//! Inside an `async/spawn`'d task every builtin offloads the subprocess onto
//! the process-wide I/O pool and yields `AwaitIo` (the `shell_async` pattern —
//! see `system.rs`), so a slow `git log`/`git diff` doesn't stall sibling
//! tasks on the cooperative scheduler. At top level (no scheduler) they stay
//! exactly as they were: `git()`'s synchronous `.output()`.
//!
//! The shared sync helper `git()` can't simply grow an async branch: a native
//! that yields `AwaitIo` is NOT re-invoked on resume (the VM substitutes the
//! call's return value directly), so any Rust code that would otherwise run
//! AFTER `git()` returns (parsing `status --porcelain`, deduping `log
//! --name-only`, …) would never execute for a parked call. Instead the async
//! branches call [`git_offload`] / [`git_stdout_async`] directly, passing a
//! `decode`/`finish` closure that does that post-processing — mirroring
//! `fs_offload` (io.rs): only `Send` facts (raw stdout/stderr/exit code) cross
//! the thread boundary, and the closure builds the final `Value` on the VM
//! thread when the scheduler polls the completed offload. The parsing/dedup
//! logic itself is factored into plain functions (`parse_status_entries`,
//! `recent_files_value`, …) shared by both paths so sync and async can't drift.

use std::collections::BTreeMap;

use sema_core::{check_arity, in_async_context, Caps, SemaError, Value};

/// Run `git` with `args`, returning raw (untrimmed) stdout on a zero exit. On a
/// non-zero exit, surface git's stderr. If the `git` binary can't be launched at
/// all (not installed / not on PATH), report that clearly.
///
/// `core.quotepath=false` is forced so non-ASCII paths come back as real UTF-8
/// rather than octal-escaped, double-quoted strings.
fn git(args: &[&str]) -> Result<String, SemaError> {
    let output = std::process::Command::new("git")
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .output()
        .map_err(|e| {
            SemaError::Io(format!(
                "git: failed to run `git` (is it installed and on PATH?): {e}"
            ))
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(SemaError::eval(format!(
            "git {}: {}",
            args.join(" "),
            stderr
        )))
    }
}

/// Parse `git status --porcelain=v1 -z` output into `(code, path)` pairs. The
/// NUL (`-z`) format is unambiguous: each record is `XY PATH\0`, and a
/// rename/copy record is `XY NEW\0OLD\0` — so the destination path is the
/// record itself and the following NUL field (the old path) is consumed. This
/// avoids the `" -> "` arrow-parsing and quoting pitfalls of the newline
/// format with paths that contain spaces, arrows, or non-ASCII bytes.
///
/// Pure (no I/O) so the sync and async paths share it: the sync path parses
/// straight off `git()`'s return value; the async path parses inside the
/// offload's `decode` closure, where the raw stdout only becomes available.
fn parse_status_entries(out: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    let mut fields = out.split('\0');
    while let Some(tok) = fields.next() {
        if tok.len() < 3 {
            continue; // empty trailing field or malformed
        }
        let code = tok[..2].to_string();
        let path = tok[3..].to_string();
        let bytes = code.as_bytes();
        // Rename (R) / copy (C) records carry the old path as the next field.
        if bytes[0] == b'R' || bytes[0] == b'C' || bytes[1] == b'R' || bytes[1] == b'C' {
            let _old = fields.next();
        }
        entries.push((code, path));
    }
    entries
}

/// Sync-only: run `git status --porcelain=v1 -z` and parse it.
fn status_entries() -> Result<Vec<(String, String)>, SemaError> {
    let out = git(&["status", "--porcelain=v1", "-z"])?;
    Ok(parse_status_entries(&out))
}

/// Build `git/status`'s result list from parsed `(code, path)` entries —
/// shared by the sync and async paths so they can't drift.
fn status_entries_value(entries: Vec<(String, String)>) -> Value {
    let mut out = Vec::new();
    for (code, path) in entries {
        let untracked = code == "??";
        // Staged = the index (X) column carries a change (non-space, non-?).
        let x = code.chars().next().unwrap_or(' ');
        let staged = !untracked && x != ' ';

        let mut m = BTreeMap::new();
        m.insert(Value::keyword("path"), Value::string(&path));
        m.insert(Value::keyword("status"), Value::string(&code));
        m.insert(Value::keyword("staged"), Value::bool(staged));
        m.insert(Value::keyword("untracked"), Value::bool(untracked));
        out.push(Value::map(m));
    }
    Value::list(out)
}

/// Build `git/changed-files`'s result list from parsed `(code, path)` entries.
fn changed_files_value(entries: Vec<(String, String)>) -> Value {
    Value::list(
        entries
            .into_iter()
            .filter(|(_, path)| !path.is_empty())
            .map(|(_, path)| Value::string(&path))
            .collect(),
    )
}

/// Build `git/diff-files`'s result list from raw `git diff --name-only` stdout.
fn diff_files_value(out: &str) -> Value {
    Value::list(
        out.lines()
            .filter(|l| !l.is_empty())
            .map(Value::string)
            .collect(),
    )
}

/// Build `git/recent-files`'s result list from raw `git log --name-only
/// --pretty=format:` stdout, deduped preserving first-seen order.
fn recent_files_value(out: &str) -> Value {
    let mut seen = std::collections::HashSet::new();
    let mut files = Vec::new();
    for line in out.lines() {
        if line.is_empty() {
            continue;
        }
        if seen.insert(line.to_string()) {
            files.push(Value::string(line));
        }
    }
    Value::list(files)
}

/// The subprocess facts that cross the thread boundary back from the I/O pool
/// to the VM thread for an offloaded `git` invocation. Only plain `Send` data
/// — never a `Value`/`Rc`.
struct RawGitOutput {
    status_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Render `msg` exactly like `SemaError::eval(msg)` would display, without
/// constructing (and immediately dropping) the error — used to pre-render an
/// async rejection so it's byte-identical to the sync path's error text (the
/// `async/await: task rejected: {e}` envelope embeds this string verbatim).
fn eval_msg(msg: String) -> String {
    SemaError::eval(msg).to_string()
}

/// Render `msg` exactly like `SemaError::Io(msg)` would display. See
/// [`eval_msg`].
fn io_msg(msg: String) -> String {
    SemaError::Io(msg).to_string()
}

/// Offload one `git` subprocess invocation onto the process-wide I/O pool and
/// yield `AwaitIo`, mirroring `shell_async` (system.rs) exactly but scoped to
/// the fixed `git` binary — so a `git/*` call inside `async/spawn` parks the
/// task instead of blocking the VM thread (and every sibling task) for the
/// subprocess's whole duration. `full_args` is the complete argv passed to
/// `git` (callers decide whether to include the `-c core.quotepath=false`
/// prefix `git()` forces on the sync path — `git/ignore-matches?` doesn't,
/// matching its sync bypass of `git()` below). `finish` turns the raw result
/// into the poller's `IoPoll` on the VM thread — same division of labor as
/// `fs_offload`'s `decode`: only `Send` facts cross the boundary, the
/// `Value`/error text is built here, on resume.
///
/// No abort-time process-group kill (unlike `shell_async`): `git` runs as a
/// single direct child, never a `sh -c` pipeline forking grandchildren, so
/// `kill_on_drop` dropping the child on abort is sufficient — best-effort,
/// matching every other non-shell offload's cancellation policy.
///
/// Returns `Ok(nil)` after arming the yield signal; the scheduler delivers the
/// real value on resume.
fn git_offload(
    full_args: Vec<String>,
    finish: impl Fn(Result<RawGitOutput, String>) -> sema_core::IoPoll + 'static,
) -> Result<Value, SemaError> {
    use std::rc::Rc;
    use tokio::sync::oneshot::error::TryRecvError;

    // Vestigial under CALL_NATIVE (the scheduler delivers the resume value via
    // `replace_stack_top`, not by re-invoking this native), but kept for
    // symmetry with the shipped `async/await` yield pattern.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let (tx, mut rx) = tokio::sync::oneshot::channel::<Result<RawGitOutput, String>>();
    let abort_task = sema_io::io_spawn(async move {
        let result = async {
            let mut cmd = tokio::process::Command::new("git");
            cmd.args(&full_args)
                .kill_on_drop(true)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            let output = cmd.output().await.map_err(|e| {
                io_msg(format!(
                    "git: failed to run `git` (is it installed and on PATH?): {e}"
                ))
            })?;
            Ok::<RawGitOutput, String>(RawGitOutput {
                status_code: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        }
        .await;
        let _ = tx.send(result);
        // Wake the parked VM thread so it re-polls promptly.
        sema_core::notify_io_complete();
    });

    let handle = Rc::new(sema_core::IoHandle::with_abort(
        move || match rx.try_recv() {
            Err(TryRecvError::Empty) => sema_core::IoPoll::Pending,
            Ok(r) => finish(r),
            // No sync equivalent for a dropped worker — plain descriptive
            // string, matching `fs_offload`'s/`shell_async`'s own novel-case
            // (non-parity) error text.
            Err(TryRecvError::Closed) => finish(Err("git: subprocess worker dropped".to_string())),
        },
        move || {
            abort_task();
        },
    ));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
}

/// Offload a git invocation whose Sema-visible result is `decode(stdout)` on a
/// zero exit, and an error matching `git()`'s exact rendered text (via
/// [`eval_msg`]) on a non-zero exit or a spawn failure. `args` are the
/// CALLER-visible args (no `core.quotepath` prefix — `git_offload` adds it to
/// the real argv), used verbatim in the error text exactly like `git()`'s
/// `args.join(" ")`, so async and sync error messages can't drift.
fn git_stdout_async(
    args: Vec<String>,
    decode: impl Fn(String) -> Value + 'static,
) -> Result<Value, SemaError> {
    let args_joined = args.join(" ");
    let mut full_args = vec!["-c".to_string(), "core.quotepath=false".to_string()];
    full_args.extend(args);
    git_offload(full_args, move |raw| match raw {
        Err(msg) => sema_core::IoPoll::Ready(Err(msg)),
        Ok(raw) if raw.status_code == Some(0) => {
            let out = String::from_utf8_lossy(&raw.stdout).to_string();
            sema_core::IoPoll::Ready(Ok(decode(out)))
        }
        Ok(raw) => {
            let stderr = String::from_utf8_lossy(&raw.stderr).trim().to_string();
            sema_core::IoPoll::Ready(Err(eval_msg(format!("git {args_joined}: {stderr}"))))
        }
    })
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/root", |args| {
        check_arity!(args, "git/root", 0);
        if in_async_context() {
            return git_stdout_async(
                vec!["rev-parse".to_string(), "--show-toplevel".to_string()],
                |out| Value::string(out.trim()),
            );
        }
        let out = git(&["rev-parse", "--show-toplevel"])?;
        Ok(Value::string(out.trim()))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/current-branch", |args| {
        check_arity!(args, "git/current-branch", 0);
        if in_async_context() {
            return git_stdout_async(
                vec![
                    "rev-parse".to_string(),
                    "--abbrev-ref".to_string(),
                    "HEAD".to_string(),
                ],
                |out| Value::string(out.trim()),
            );
        }
        let out = git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
        Ok(Value::string(out.trim()))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/status", |args| {
        check_arity!(args, "git/status", 0);
        if in_async_context() {
            return git_stdout_async(
                vec![
                    "status".to_string(),
                    "--porcelain=v1".to_string(),
                    "-z".to_string(),
                ],
                |out| status_entries_value(parse_status_entries(&out)),
            );
        }
        Ok(status_entries_value(status_entries()?))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/changed-files", |args| {
        check_arity!(args, "git/changed-files", 0);
        if in_async_context() {
            return git_stdout_async(
                vec![
                    "status".to_string(),
                    "--porcelain=v1".to_string(),
                    "-z".to_string(),
                ],
                |out| changed_files_value(parse_status_entries(&out)),
            );
        }
        Ok(changed_files_value(status_entries()?))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/diff", |args| {
        check_arity!(args, "git/diff", 0..=1);
        let path = if args.is_empty() {
            None
        } else {
            Some(
                args[0]
                    .as_str()
                    .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                    .to_string(),
            )
        };
        if in_async_context() {
            let diff_args = match &path {
                None => vec!["diff".to_string()],
                Some(p) => vec!["diff".to_string(), "--".to_string(), p.clone()],
            };
            return git_stdout_async(diff_args, |out| Value::string(&out));
        }
        let out = match &path {
            None => git(&["diff"])?,
            Some(p) => git(&["diff", "--", p])?,
        };
        Ok(Value::string(&out))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/diff-files", |args| {
        check_arity!(args, "git/diff-files", 0);
        if in_async_context() {
            return git_stdout_async(vec!["diff".to_string(), "--name-only".to_string()], |out| {
                diff_files_value(&out)
            });
        }
        let out = git(&["diff", "--name-only"])?;
        Ok(diff_files_value(&out))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/recent-files", |args| {
        check_arity!(args, "git/recent-files", 0..=1);
        let n = if args.is_empty() {
            20
        } else {
            args[0]
                .as_int()
                .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?
        };
        let n_str = n.to_string();
        if in_async_context() {
            return git_stdout_async(
                vec![
                    "log".to_string(),
                    "--name-only".to_string(),
                    "--pretty=format:".to_string(),
                    "-n".to_string(),
                    n_str.clone(),
                ],
                |out| recent_files_value(&out),
            );
        }
        let out = git(&["log", "--name-only", "--pretty=format:", "-n", &n_str])?;
        Ok(recent_files_value(&out))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/ignore-matches?", |args| {
        check_arity!(args, "git/ignore-matches?", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        // `git check-ignore -q` exits 0 if the path is ignored, 1 if not, and
        // >1 on a real error. We need the raw exit code, so bypass the
        // helpers above (`git()`/`git_stdout_async`, which treat any non-zero
        // exit as failure) on BOTH paths.
        if in_async_context() {
            let path_for_msg = path.clone();
            return git_offload(
                vec!["check-ignore".to_string(), "-q".to_string(), path],
                move |raw| match raw {
                    Err(msg) => sema_core::IoPoll::Ready(Err(msg)),
                    Ok(raw) => match raw.status_code {
                        Some(0) => sema_core::IoPoll::Ready(Ok(Value::bool(true))),
                        Some(1) => sema_core::IoPoll::Ready(Ok(Value::bool(false))),
                        other => {
                            let stderr = String::from_utf8_lossy(&raw.stderr).trim().to_string();
                            sema_core::IoPoll::Ready(Err(eval_msg(format!(
                                "git check-ignore {path_for_msg}: exit {}: {stderr}",
                                other
                                    .map(|c| c.to_string())
                                    .unwrap_or_else(|| "signal".into())
                            ))))
                        }
                    },
                },
            );
        }
        let output = std::process::Command::new("git")
            .args(["check-ignore", "-q", &path])
            .output()
            .map_err(|e| {
                SemaError::Io(format!(
                    "git: failed to run `git` (is it installed and on PATH?): {e}"
                ))
            })?;
        match output.status.code() {
            Some(0) => Ok(Value::bool(true)),
            Some(1) => Ok(Value::bool(false)),
            other => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                Err(SemaError::eval(format!(
                    "git check-ignore {path}: exit {}: {stderr}",
                    other
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "signal".into())
                )))
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// True when a `git` binary is available; tests early-return otherwise so a
    /// machine without git doesn't hard-fail the suite.
    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn make_env() -> (sema_core::Env, sema_core::Sandbox) {
        let env = sema_core::Env::new();
        let sandbox = sema_core::Sandbox::allow_all();
        register(&env, &sandbox);
        (env, sandbox)
    }

    /// Call a registered native fn by name with the given args.
    fn call(env: &sema_core::Env, name: &str, args: &[Value]) -> Result<Value, SemaError> {
        let f = env
            .get(sema_core::intern(name))
            .unwrap_or_else(|| panic!("{name} not registered"));
        let nf = f.as_native_fn_ref().expect("native fn");
        let ctx = sema_core::EvalContext::default();
        (nf.func)(&ctx, args)
    }

    #[test]
    fn root_is_ancestor_of_cwd() {
        if !git_available() {
            return;
        }
        let (env, _sb) = make_env();
        let v = call(&env, "git/root", &[]).expect("git/root");
        let s = v.as_str().expect("string");
        assert!(!s.is_empty(), "root should be non-empty");
        // git/root is the repo toplevel, so the cwd (the crate dir under `cargo
        // test`) must live inside it. Assert containment rather than a hardcoded
        // directory name, which isn't portable across checkouts and worktrees.
        let root = std::fs::canonicalize(s).expect("canonical root");
        let cwd = std::fs::canonicalize(std::env::current_dir().expect("cwd")).expect("canon cwd");
        assert!(
            cwd.starts_with(&root),
            "cwd {cwd:?} should be inside git root {root:?}"
        );
    }

    #[test]
    fn current_branch_non_empty() {
        if !git_available() {
            return;
        }
        let (env, _sb) = make_env();
        let v = call(&env, "git/current-branch", &[]).expect("git/current-branch");
        let s = v.as_str().expect("string");
        assert!(!s.is_empty(), "branch should be non-empty");
    }

    #[test]
    fn status_returns_list() {
        if !git_available() {
            return;
        }
        let (env, _sb) = make_env();
        let v = call(&env, "git/status", &[]).expect("git/status");
        assert!(v.as_list().is_some(), "git/status should return a list");
    }

    #[test]
    fn ignore_matches_target_but_not_cargo_toml() {
        if !git_available() {
            return;
        }
        let (env, _sb) = make_env();
        // `git check-ignore` resolves relative to the cwd (the crate dir under
        // `cargo test`), where the root-anchored `/target` rule does NOT apply.
        // Anchor on the repo root with an absolute path so the test is location
        // independent.
        let root = call(&env, "git/root", &[])
            .expect("git/root")
            .as_str()
            .unwrap()
            .to_string();
        let target = format!("{root}/target/x");
        let ignored = call(&env, "git/ignore-matches?", &[Value::string(&target)])
            .expect("git/ignore-matches? target");
        assert_eq!(ignored, Value::bool(true), "{target} should be ignored");

        let cargo = format!("{root}/Cargo.toml");
        let tracked = call(&env, "git/ignore-matches?", &[Value::string(&cargo)])
            .expect("git/ignore-matches? Cargo.toml");
        assert_eq!(tracked, Value::bool(false), "{cargo} should not be ignored");
    }
}
