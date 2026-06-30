//! Read-only Git helpers (`git/*`).
//!
//! These shell out to the `git` binary and parse its output. They never mutate
//! the repository — no commit, add, checkout, push. Running `git` is a
//! subprocess, so every builtin is gated behind `Caps::PROCESS`.

use std::collections::BTreeMap;

use sema_core::{check_arity, Caps, SemaError, Value};

/// Run `git` with `args`, returning trimmed-free stdout (raw, untrimmed) on a
/// zero exit. On a non-zero exit, surface git's stderr. If the `git` binary
/// can't be launched at all (not installed / not on PATH), report that clearly.
fn git(args: &[&str]) -> Result<String, SemaError> {
    let output = std::process::Command::new("git")
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

/// The path field of a porcelain-v1 line, accounting for rename/copy entries
/// which encode `old -> new` (we want the destination). The leading 3 chars of
/// a porcelain line are `XY ` (two status chars + a space); the rest is the path.
fn porcelain_path(rest: &str) -> &str {
    match rest.split_once(" -> ") {
        Some((_old, new)) => new,
        None => rest,
    }
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/root", |args| {
        check_arity!(args, "git/root", 0);
        let out = git(&["rev-parse", "--show-toplevel"])?;
        Ok(Value::string(out.trim()))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/current-branch", |args| {
        check_arity!(args, "git/current-branch", 0);
        let out = git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
        Ok(Value::string(out.trim()))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/status", |args| {
        check_arity!(args, "git/status", 0);
        let out = git(&["status", "--porcelain=v1"])?;
        let mut entries = Vec::new();
        for line in out.lines() {
            if line.is_empty() {
                continue;
            }
            // Porcelain v1: two status chars (XY), a space, then the path.
            let code: String = line.chars().take(2).collect();
            let rest = if line.len() > 3 { &line[3..] } else { "" };
            let path = porcelain_path(rest);
            let untracked = code == "??";
            // Staged = the index (X) column carries a change (non-space, non-?).
            let x = code.chars().next().unwrap_or(' ');
            let staged = !untracked && x != ' ';

            let mut m = BTreeMap::new();
            m.insert(Value::keyword("path"), Value::string(path));
            m.insert(Value::keyword("status"), Value::string(&code));
            m.insert(Value::keyword("staged"), Value::bool(staged));
            m.insert(Value::keyword("untracked"), Value::bool(untracked));
            entries.push(Value::map(m));
        }
        Ok(Value::list(entries))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/changed-files", |args| {
        check_arity!(args, "git/changed-files", 0);
        let out = git(&["status", "--porcelain=v1"])?;
        let mut files = Vec::new();
        for line in out.lines() {
            if line.is_empty() {
                continue;
            }
            let rest = if line.len() > 3 { &line[3..] } else { "" };
            let path = porcelain_path(rest);
            if !path.is_empty() {
                files.push(Value::string(path));
            }
        }
        Ok(Value::list(files))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/diff", |args| {
        check_arity!(args, "git/diff", 0..=1);
        let out = if args.is_empty() {
            git(&["diff"])?
        } else {
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            git(&["diff", "--", path])?
        };
        Ok(Value::string(&out))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/diff-files", |args| {
        check_arity!(args, "git/diff-files", 0);
        let out = git(&["diff", "--name-only"])?;
        let files: Vec<Value> = out
            .lines()
            .filter(|l| !l.is_empty())
            .map(Value::string)
            .collect();
        Ok(Value::list(files))
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
        let out = git(&["log", "--name-only", "--pretty=format:", "-n", &n_str])?;
        // Dedup preserving first-seen order.
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
        Ok(Value::list(files))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "git/ignore-matches?", |args| {
        check_arity!(args, "git/ignore-matches?", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        // `git check-ignore -q` exits 0 if the path is ignored, 1 if not,
        // and >1 on a real error. We need the raw exit code, so bypass the
        // `git()` helper (which treats any non-zero as failure).
        let output = std::process::Command::new("git")
            .args(["check-ignore", "-q", path])
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
    fn root_ends_in_sema() {
        if !git_available() {
            return;
        }
        let (env, _sb) = make_env();
        let v = call(&env, "git/root", &[]).expect("git/root");
        let s = v.as_str().expect("string");
        assert!(!s.is_empty(), "root should be non-empty");
        assert!(s.ends_with("sema"), "root {s:?} should end in 'sema'");
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
