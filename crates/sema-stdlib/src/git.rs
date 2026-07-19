//! Read-only Git helpers (`git/*`).
//!
//! These shell out to the `git` binary and parse its output. They never mutate
//! the repository — no commit, add, checkout, push. Running `git` is a
//! subprocess, so every builtin is gated behind `Caps::PROCESS`.
//!
//! Inside an `async/spawn`'d task every builtin offloads the subprocess onto
//! the process-wide I/O pool and suspends on a structural `External` wait, so
//! a slow `git log`/`git diff` does not stall sibling tasks. At top level they
//! use `git()`'s synchronous `.output()` path.
//!
//! Runtime paths pass a `decode` closure to [`git_stdout_runtime`] so only
//! `Send` facts (raw stdout/stderr/exit code) cross the thread boundary. The
//! closure builds the final `Value` on the VM thread after the wait completes.
//! Parsing and deduplication live in plain functions (`parse_status_entries`,
//! `recent_files_value`, …) shared by synchronous and runtime paths.

use std::collections::BTreeMap;

use sema_core::runtime::{CompletionKind, NativeOutcome, NativeResult};
use sema_core::{check_arity, in_runtime_quantum, Caps, SemaError, Value};

/// Completion tag for an offloaded `git` subprocess. Consistent between the
/// issued identity and the prepared op (not a uniqueness key), so one shared
/// value for every `git/*` op is correct.
const GIT_COMPLETION_KIND: u64 = 0x6769_7400; // "git\0"

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

/// The `Send` future that runs one `git` invocation off the VM thread on the
/// executor's blocking worker (via `io_block_on` inside `runtime_offload`). The
/// direct child is `kill_on_drop`, so a cancel that drops this future kills it —
/// best-effort, matching every other non-shell offload's cancellation policy.
async fn git_run_future(full_args: Vec<String>) -> Result<RawGitOutput, String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(&full_args)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = cmd
        .output()
        .await
        .map_err(|e| format!("git: failed to run `git` (is it installed and on PATH?): {e}"))?;
    Ok(RawGitOutput {
        status_code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Unified-runtime counterpart to running `git` off the VM thread: SUSPEND on an
/// interruptible External wait whose job runs `git` off the VM thread and, on a
/// zero exit, resumes with `decode(stdout)`; a non-zero exit / spawn failure
/// raises the SAME error text `git()` renders (so runtime and sync can't drift).
/// Cancellation drops the child (`kill_on_drop`). Cancellation class:
/// interruptible, best-effort kill (single direct child, no process group).
fn git_stdout_runtime(
    args: Vec<String>,
    decode: impl FnOnce(String) -> Value + 'static,
) -> NativeResult {
    let args_joined = args.join(" ");
    let mut full_args = vec!["-c".to_string(), "core.quotepath=false".to_string()];
    full_args.extend(args);
    let kind =
        CompletionKind::try_from_raw(GIT_COMPLETION_KIND).expect("git completion kind is nonzero");
    crate::runtime_offload::external_io_interruptible_try(
        "git",
        kind,
        "git",
        move |raw: RawGitOutput| -> Result<Value, SemaError> {
            if raw.status_code == Some(0) {
                Ok(decode(String::from_utf8_lossy(&raw.stdout).to_string()))
            } else {
                let stderr = String::from_utf8_lossy(&raw.stderr).trim().to_string();
                Err(SemaError::eval(format!("git {args_joined}: {stderr}")))
            }
        },
        move || git_run_future(full_args),
    )
}

/// Unified-runtime counterpart to `git/ignore-matches?`'s `git_offload` use:
/// needs the RAW exit code (0 = ignored, 1 = not, >1 = error), so it bypasses
/// `git_stdout_runtime`'s zero-exit-only helper exactly like the synchronous
/// path bypasses `git()`.
fn git_ignore_matches_runtime(path: String) -> NativeResult {
    let path_for_msg = path.clone();
    let full_args = vec!["check-ignore".to_string(), "-q".to_string(), path];
    let kind =
        CompletionKind::try_from_raw(GIT_COMPLETION_KIND).expect("git completion kind is nonzero");
    crate::runtime_offload::external_io_interruptible_try(
        "git",
        kind,
        "git",
        move |raw: RawGitOutput| -> Result<Value, SemaError> {
            match raw.status_code {
                Some(0) => Ok(Value::bool(true)),
                Some(1) => Ok(Value::bool(false)),
                other => {
                    let stderr = String::from_utf8_lossy(&raw.stderr).trim().to_string();
                    Err(SemaError::eval(format!(
                        "git check-ignore {path_for_msg}: exit {}: {stderr}",
                        other
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "signal".into())
                    )))
                }
            }
        },
        move || git_run_future(full_args),
    )
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_runtime_fn_path_gated(env, sandbox, Caps::PROCESS, "git/root", &[], |args| {
        check_arity!(args, "git/root", 0);
        if in_runtime_quantum() {
            return git_stdout_runtime(
                vec!["rev-parse".to_string(), "--show-toplevel".to_string()],
                |out| Value::string(out.trim()),
            );
        }
        let out = git(&["rev-parse", "--show-toplevel"])?;
        Ok(NativeOutcome::Return(Value::string(out.trim())))
    });

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::PROCESS,
        "git/current-branch",
        &[],
        |args| {
            check_arity!(args, "git/current-branch", 0);
            if in_runtime_quantum() {
                return git_stdout_runtime(
                    vec![
                        "rev-parse".to_string(),
                        "--abbrev-ref".to_string(),
                        "HEAD".to_string(),
                    ],
                    |out| Value::string(out.trim()),
                );
            }
            let out = git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
            Ok(NativeOutcome::Return(Value::string(out.trim())))
        },
    );

    crate::register_runtime_fn_path_gated(env, sandbox, Caps::PROCESS, "git/status", &[], |args| {
        check_arity!(args, "git/status", 0);
        if in_runtime_quantum() {
            return git_stdout_runtime(
                vec![
                    "status".to_string(),
                    "--porcelain=v1".to_string(),
                    "-z".to_string(),
                ],
                |out| status_entries_value(parse_status_entries(&out)),
            );
        }
        Ok(NativeOutcome::Return(status_entries_value(
            status_entries()?
        )))
    });

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::PROCESS,
        "git/changed-files",
        &[],
        |args| {
            check_arity!(args, "git/changed-files", 0);
            if in_runtime_quantum() {
                return git_stdout_runtime(
                    vec![
                        "status".to_string(),
                        "--porcelain=v1".to_string(),
                        "-z".to_string(),
                    ],
                    |out| changed_files_value(parse_status_entries(&out)),
                );
            }
            Ok(NativeOutcome::Return(
                changed_files_value(status_entries()?),
            ))
        },
    );

    crate::register_runtime_fn_path_gated(env, sandbox, Caps::PROCESS, "git/diff", &[], |args| {
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
        let diff_args = || match &path {
            None => vec!["diff".to_string()],
            Some(p) => vec!["diff".to_string(), "--".to_string(), p.clone()],
        };
        if in_runtime_quantum() {
            return git_stdout_runtime(diff_args(), |out| Value::string(&out));
        }
        let out = match &path {
            None => git(&["diff"])?,
            Some(p) => git(&["diff", "--", p])?,
        };
        Ok(NativeOutcome::Return(Value::string(&out)))
    });

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::PROCESS,
        "git/diff-files",
        &[],
        |args| {
            check_arity!(args, "git/diff-files", 0);
            if in_runtime_quantum() {
                return git_stdout_runtime(
                    vec!["diff".to_string(), "--name-only".to_string()],
                    |out| diff_files_value(&out),
                );
            }
            let out = git(&["diff", "--name-only"])?;
            Ok(NativeOutcome::Return(diff_files_value(&out)))
        },
    );

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::PROCESS,
        "git/recent-files",
        &[],
        |args| {
            check_arity!(args, "git/recent-files", 0..=1);
            let n = if args.is_empty() {
                20
            } else {
                args[0]
                    .as_int()
                    .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?
            };
            let n_str = n.to_string();
            let log_args = || {
                vec![
                    "log".to_string(),
                    "--name-only".to_string(),
                    "--pretty=format:".to_string(),
                    "-n".to_string(),
                    n_str.clone(),
                ]
            };
            if in_runtime_quantum() {
                return git_stdout_runtime(log_args(), |out| recent_files_value(&out));
            }
            let out = git(&["log", "--name-only", "--pretty=format:", "-n", &n_str])?;
            Ok(NativeOutcome::Return(recent_files_value(&out)))
        },
    );

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::PROCESS,
        "git/ignore-matches?",
        &[],
        |args| {
            check_arity!(args, "git/ignore-matches?", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();
            // `git check-ignore -q` exits 0 if the path is ignored, 1 if not, and
            // >1 on a real error. We need the raw exit code, so bypass the
            // helpers above (`git()`/`git_stdout_runtime`, which treat any non-zero
            // exit as failure) on ALL paths.
            if in_runtime_quantum() {
                return git_ignore_matches_runtime(path);
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
                Some(0) => Ok(NativeOutcome::Return(Value::bool(true))),
                Some(1) => Ok(NativeOutcome::Return(Value::bool(false))),
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
        },
    );
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
