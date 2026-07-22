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

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    CancelDisposition, CancelHook, CancelHookError, CompletionKind, InterruptibleResource,
    NativeOutcome, NativeResult, Trace,
};
use sema_core::{check_arity, in_runtime_quantum, Caps, SemaError, Value};

/// Completion tag for an offloaded `git` subprocess. Consistent between the
/// issued identity and the prepared op (not a uniqueness key), so one shared
/// value for every `git/*` op is correct.
const GIT_COMPLETION_KIND: u64 = 0x6769_7400; // "git\0"

/// Cancellation grace for process-group members to close inherited pipes.
/// A group kill normally closes them immediately; 100 ms absorbs scheduler and
/// pipe-delivery latency while keeping runtime shutdown cleanup bounded when a
/// foreign descendant escaped the group but retained an output descriptor.
const GIT_CANCEL_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_millis(100);

/// Hard ceiling on the bytes a single offloaded `git` invocation may buffer from
/// EACH of stdout/stderr before the process group is killed. The offload's
/// `decode` closure materializes the whole output into a `Value` on the VM
/// thread, so an unbounded `git log`/`git diff` (a pathological or hostile repo)
/// would exhaust memory; a capped incremental drain turns that into a clean,
/// structured over-cap error instead of an OOM — never a `read_to_end` of a
/// hostile pipe.
const GIT_MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

/// Chunk size for the incremental pipe drain. Large enough to keep syscall
/// overhead low, small enough that the cap is enforced within one chunk of the
/// boundary.
const GIT_DRAIN_CHUNK: usize = 64 * 1024;

thread_local! {
    /// Optional per-call output-byte cap override (lowered, never raised above
    /// the hard ceiling). Read on the VM thread pre-dispatch and captured by the
    /// offloaded job — mirrors `sqlite::DB_RESULT_CAPS_OVERRIDE`. `None` uses the
    /// module ceiling.
    static GIT_MAX_OUTPUT_OVERRIDE: Cell<Option<usize>> = const { Cell::new(None) };
}

/// The effective per-pipe output cap for the current call: the module ceiling,
/// lowered by any per-call override (never raised above it). Read on the VM
/// thread pre-dispatch, then captured by the offloaded job.
fn effective_git_max_output_bytes() -> usize {
    GIT_MAX_OUTPUT_OVERRIDE
        .with(Cell::get)
        .map_or(GIT_MAX_OUTPUT_BYTES, |over| over.min(GIT_MAX_OUTPUT_BYTES))
}

/// Lower the per-pipe output cap (clamped to the hard ceiling) for subsequent
/// offloaded `git/*` calls on this thread, or clear the override with `None`.
/// The seam the regression suite drives to exercise the over-cap path without a
/// multi-megabyte fixture; mirrors `set_db_result_caps_override`.
pub fn set_git_max_output_bytes_override(bytes: Option<usize>) {
    GIT_MAX_OUTPUT_OVERRIDE.with(|cell| cell.set(bytes));
}

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

/// Shared proof that the Git child was waited and both pipe-drain tasks were
/// joined. Cancellation cleanup stays registered until this flips to true.
#[derive(Clone, Default)]
struct GitCompletionGuard(Arc<AtomicBool>);

impl GitCompletionGuard {
    fn mark_reaped(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn disposition(&self) -> CancelDisposition {
        if self.0.load(Ordering::Acquire) {
            CancelDisposition::Reaped
        } else {
            CancelDisposition::PendingReap
        }
    }
}

/// Proves the queued-before-start case. If the executor discards the job
/// closure before invoking it, no process or pipe can exist and dropping this
/// guard publishes `Reaped`. Once started, only [`git_run_future`] may publish
/// that proof after its wait and pipe joins finish.
struct GitDispatchGuard {
    completion: Option<GitCompletionGuard>,
}

impl GitDispatchGuard {
    fn new(completion: GitCompletionGuard) -> Self {
        Self {
            completion: Some(completion),
        }
    }

    fn start(mut self) -> GitCompletionGuard {
        self.completion
            .take()
            .expect("dispatch completion is transferred exactly once")
    }
}

impl Drop for GitDispatchGuard {
    fn drop(&mut self) {
        if let Some(completion) = self.completion.take() {
            completion.mark_reaped();
        }
    }
}

/// Cancel one runtime Git invocation by waking the owned worker. The worker
/// keeps the child unreaped while pipe drains are live, so it can signal the
/// original process group without retaining a reusable raw pid in this hook.
struct GitCancelHook {
    signal: Option<crate::runtime_offload::CancelSignal>,
    completion: GitCompletionGuard,
}

impl GitCancelHook {
    fn new(signal: crate::runtime_offload::CancelSignal, completion: GitCompletionGuard) -> Self {
        Self {
            signal: Some(signal),
            completion,
        }
    }
}

impl Trace for GitCancelHook {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl CancelHook for GitCancelHook {
    fn cancel(&mut self) -> Result<CancelDisposition, CancelHookError> {
        if let Some(signal) = self.signal.take() {
            let _ = signal.send(());
        }
        Ok(self.completion.disposition())
    }

    fn reap(&mut self) -> Result<CancelDisposition, CancelHookError> {
        Ok(self.completion.disposition())
    }
}

#[cfg(unix)]
fn kill_git_process_group(pid: u32) {
    if pid == 0 {
        return;
    }
    // SAFETY: runtime Git children call `process_group(0)`, so a negative pid
    // targets only the process group owned by this invocation.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}

fn terminate_git_child(child: &mut tokio::process::Child, pid: u32) {
    #[cfg(unix)]
    kill_git_process_group(pid);
    // This is the non-Unix cancellation mechanism and a direct-child fallback
    // if a Unix group signal races process-group setup or exit.
    let _ = child.start_kill();
}

/// The over-cap signal shared by both pipe drains and the invocation future: the
/// first drain to exceed the cap sets the flag and wakes the future so it can
/// kill the process group promptly (which lets the sibling drain read EOF).
type OverCapSignal = tokio::sync::mpsc::Sender<()>;

/// Drain one pipe incrementally, never buffering more than `cap` bytes. On
/// exceeding the cap the drain stops reading, publishes the over-cap fact, wakes
/// the invocation future (which kills the process group), and returns the bytes
/// read so far (discarded on the over-cap error path). This is a bounded,
/// pre-dispatch-capped admission — never `read_to_end` of a hostile pipe.
async fn drain_git_pipe<R>(
    mut pipe: R,
    cap: usize,
    over_cap: Arc<AtomicBool>,
    signal: OverCapSignal,
) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; GIT_DRAIN_CHUNK];
    loop {
        let read = pipe.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > cap {
            // Only the first drain to trip needs to wake the future and kill the
            // group; the other drain then reads EOF as the child dies.
            if !over_cap.swap(true, Ordering::AcqRel) {
                let _ = signal.try_send(());
            }
            bytes.truncate(cap);
            return Ok(bytes);
        }
    }
    Ok(bytes)
}

/// The `Send` future that runs one `git` invocation off the VM thread on the
/// executor's blocking worker (via `io_block_on` inside `runtime_offload`).
/// Stdout and stderr are drained concurrently with the child wait, preventing a
/// full pipe from deadlocking the process. Each drain is capped at
/// `max_output_bytes`: exceeding it kills the process group (the same hook
/// cancellation uses) and resolves the invocation with a structured over-cap
/// error rather than buffering a hostile pipe to exhaustion. Cancellation kills
/// the process group on Unix (the direct child elsewhere), then still joins both
/// drains and awaits the child before publishing the completion proof.
/// `kill_on_drop` remains a fallback for executor panic/drop paths.
async fn git_run_future(
    full_args: Vec<String>,
    max_output_bytes: usize,
    mut cancel: crate::runtime_offload::CancelWaiter,
    completion: GitCompletionGuard,
) -> Result<RawGitOutput, String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(&full_args)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            completion.mark_reaped();
            return Err(format!(
                "git: failed to run `git` (is it installed and on PATH?): {error}"
            ));
        }
    };
    let pid = child.id().unwrap_or(0);

    let (stdout, stderr) = match (child.stdout.take(), child.stderr.take()) {
        (Some(stdout), Some(stderr)) => (stdout, stderr),
        _ => {
            terminate_git_child(&mut child, pid);
            let wait = child.wait().await;
            if wait.is_ok() {
                completion.mark_reaped();
            }
            return Err("git: subprocess pipes were not captured".to_string());
        }
    };
    let over_cap = Arc::new(AtomicBool::new(false));
    let (over_cap_tx, mut over_cap_rx) = tokio::sync::mpsc::channel::<()>(2);
    let stdout_drain = tokio::spawn(drain_git_pipe(
        stdout,
        max_output_bytes,
        Arc::clone(&over_cap),
        over_cap_tx.clone(),
    ));
    let stderr_drain = tokio::spawn(drain_git_pipe(
        stderr,
        max_output_bytes,
        Arc::clone(&over_cap),
        over_cap_tx,
    ));
    let stdout_abort = stdout_drain.abort_handle();
    let stderr_abort = stderr_drain.abort_handle();

    let mut cancelled = false;
    let mut cancel_resolved = false;
    let mut over_capped = false;
    let drains = async { tokio::join!(stdout_drain, stderr_drain) };
    tokio::pin!(drains);
    let (stdout, stderr) = tokio::select! {
        result = &mut drains => result,
        over = over_cap_rx.recv() => {
            // An over-cap drain woke us: kill the process group so the sibling
            // drain reads EOF, then join the drains bounded by the same grace as
            // cancellation. `None` (both senders dropped without tripping) means
            // the drains already finished — just await them.
            if over.is_some() && over_cap.load(Ordering::Acquire) {
                over_capped = true;
                terminate_git_child(&mut child, pid);
                match tokio::time::timeout(GIT_CANCEL_DRAIN_GRACE, &mut drains).await {
                    Ok(result) => result,
                    Err(_) => {
                        stdout_abort.abort();
                        stderr_abort.abort();
                        drains.await
                    }
                }
            } else {
                drains.await
            }
        }
        signal = &mut cancel => {
            cancel_resolved = true;
            if signal.is_ok() {
                cancelled = true;
                terminate_git_child(&mut child, pid);
                match tokio::time::timeout(GIT_CANCEL_DRAIN_GRACE, &mut drains).await {
                    Ok(result) => result,
                    Err(_) => {
                        stdout_abort.abort();
                        stderr_abort.abort();
                        drains.await
                    }
                }
            } else {
                drains.await
            }
        }
    };
    // The child remains unreaped until both pipes close. While an inherited-pipe
    // descendant is alive, the pid therefore still names the original group and
    // cannot be reused by an unrelated process group.
    let status = if cancel_resolved {
        child.wait().await
    } else {
        tokio::select! {
            result = child.wait() => result,
            signal = &mut cancel => {
                if signal.is_ok() {
                    cancelled = true;
                    terminate_git_child(&mut child, pid);
                }
                child.wait().await
            }
        }
    };
    let status = match status {
        Ok(status) => Ok(status),
        Err(first_error) => child.wait().await.map_err(|second_error| {
            std::io::Error::new(
                second_error.kind(),
                format!("initial wait failed: {first_error}; reap retry failed: {second_error}"),
            )
        }),
    };

    // These three awaits are the resource proof: the direct child is reaped and
    // neither pipe-drain task remains live. On cancellation, an escaped process
    // can force the bounded path to abort the drains, but both JoinHandles are
    // still awaited before this proof is published.
    if status.is_ok() {
        completion.mark_reaped();
    }

    // An over-cap kill is a hard failure independent of the (now-terminated)
    // child's exit status and of any drain the grace timeout had to abort, so
    // surface the structured error before unwrapping the drains. A genuine
    // cancellation still wins (the runtime settles it via the cancel hook).
    if over_capped && !cancelled {
        return Err(format!(
            "git: output exceeded the {max_output_bytes}-byte limit; the process group was terminated"
        ));
    }

    let status = status.map_err(|error| format!("git: failed while waiting for `git`: {error}"))?;
    let stdout = stdout
        .map_err(|error| format!("git: stdout drain task failed: {error}"))?
        .map_err(|error| format!("git: failed to read stdout: {error}"))?;
    let stderr = stderr
        .map_err(|error| format!("git: stderr drain task failed: {error}"))?
        .map_err(|error| format!("git: failed to read stderr: {error}"))?;
    if cancelled {
        return Err("git was cancelled".to_string());
    }
    Ok(RawGitOutput {
        status_code: status.code(),
        stdout,
        stderr,
    })
}

fn git_external_runtime(
    full_args: Vec<String>,
    decode: impl FnOnce(RawGitOutput) -> Result<Value, SemaError> + 'static,
) -> NativeResult {
    let kind =
        CompletionKind::try_from_raw(GIT_COMPLETION_KIND).expect("git completion kind is nonzero");
    // Resolve the output cap on the VM thread (pre-dispatch) so the offloaded job
    // carries a fixed finite admission, never reading a thread-local on a worker.
    let max_output_bytes = effective_git_max_output_bytes();
    let (cancel_tx, cancel_rx) = crate::runtime_offload::cancel_channel();
    let completion = GitCompletionGuard::default();
    let dispatch = GitDispatchGuard::new(completion.clone());
    let resource =
        InterruptibleResource::new("git", Box::new(GitCancelHook::new(cancel_tx, completion)));
    crate::runtime_offload::suspend_external_interruptible_owned_try(
        "git",
        kind,
        resource,
        decode,
        move || git_run_future(full_args, max_output_bytes, cancel_rx, dispatch.start()),
    )
}

/// Unified-runtime counterpart to running `git` off the VM thread: SUSPEND on an
/// interruptible External wait whose job runs `git` off the VM thread and, on a
/// zero exit, resumes with `decode(stdout)`; a non-zero exit / spawn failure
/// raises the SAME error text `git()` renders (so runtime and sync can't drift).
/// Cancellation kills the owned process group on Unix (the direct child on
/// other platforms), drains both pipes, and reaps the child before the runtime
/// cleanup registry releases the resource.
fn git_stdout_runtime(
    args: Vec<String>,
    decode: impl FnOnce(String) -> Value + 'static,
) -> NativeResult {
    let args_joined = args.join(" ");
    let mut full_args = vec!["-c".to_string(), "core.quotepath=false".to_string()];
    full_args.extend(args);
    git_external_runtime(
        full_args,
        move |raw: RawGitOutput| -> Result<Value, SemaError> {
            if raw.status_code == Some(0) {
                Ok(decode(String::from_utf8_lossy(&raw.stdout).to_string()))
            } else {
                let stderr = String::from_utf8_lossy(&raw.stderr).trim().to_string();
                Err(SemaError::eval(format!("git {args_joined}: {stderr}")))
            }
        },
    )
}

/// Unified-runtime counterpart to `git/ignore-matches?`'s `git_offload` use:
/// needs the RAW exit code (0 = ignored, 1 = not, >1 = error), so it bypasses
/// `git_stdout_runtime`'s zero-exit-only helper exactly like the synchronous
/// path bypasses `git()`.
fn git_ignore_matches_runtime(path: String) -> NativeResult {
    let path_for_msg = path.clone();
    let full_args = vec!["check-ignore".to_string(), "-q".to_string(), path];
    git_external_runtime(
        full_args,
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
    use sema_core::runtime::{CancelDisposition, CancelHook};

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

    #[test]
    fn cancel_hook_stays_pending_until_process_and_pipes_are_reaped() {
        let completion = GitCompletionGuard::default();
        let (cancel_tx, mut cancel_rx) = crate::runtime_offload::cancel_channel();
        let mut hook = GitCancelHook::new(cancel_tx, completion.clone());

        assert_eq!(hook.cancel().unwrap(), CancelDisposition::PendingReap);
        assert_eq!(cancel_rx.try_recv(), Ok(()), "cancel signal must fire");
        assert_eq!(hook.reap().unwrap(), CancelDisposition::PendingReap);

        completion.mark_reaped();
        assert_eq!(hook.reap().unwrap(), CancelDisposition::Reaped);
    }

    #[test]
    fn cancel_hook_reports_reaped_when_worker_already_finished() {
        let completion = GitCompletionGuard::default();
        completion.mark_reaped();
        let (cancel_tx, mut cancel_rx) = crate::runtime_offload::cancel_channel();
        let mut hook = GitCancelHook::new(cancel_tx, completion);

        assert_eq!(hook.cancel().unwrap(), CancelDisposition::Reaped);
        assert_eq!(cancel_rx.try_recv(), Ok(()), "cancel signal is one-shot");
        assert_eq!(hook.reap().unwrap(), CancelDisposition::Reaped);
    }

    #[test]
    fn dropping_unstarted_dispatch_proves_no_process_exists() {
        let completion = GitCompletionGuard::default();
        let dispatch = GitDispatchGuard::new(completion.clone());

        drop(dispatch);

        assert_eq!(completion.disposition(), CancelDisposition::Reaped);
    }

    #[test]
    fn starting_dispatch_requires_explicit_reap_proof() {
        let completion = GitCompletionGuard::default();
        let dispatch = GitDispatchGuard::new(completion.clone());

        let worker_completion = dispatch.start();
        drop(worker_completion);

        assert_eq!(
            completion.disposition(),
            CancelDisposition::PendingReap,
            "a started worker cannot be declared reaped merely because its guard dropped"
        );
    }

    #[test]
    fn git_output_cap_is_finite_and_clamps_overrides() {
        assert_eq!(effective_git_max_output_bytes(), GIT_MAX_OUTPUT_BYTES);
        set_git_max_output_bytes_override(Some(16));
        assert_eq!(effective_git_max_output_bytes(), 16);
        // An override never raises the cap above the hard ceiling.
        set_git_max_output_bytes_override(Some(usize::MAX));
        assert_eq!(effective_git_max_output_bytes(), GIT_MAX_OUTPUT_BYTES);
        set_git_max_output_bytes_override(None);
        assert_eq!(effective_git_max_output_bytes(), GIT_MAX_OUTPUT_BYTES);
    }

    #[test]
    fn drain_git_pipe_caps_output_and_signals_over_cap() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime");

        // Over-cap: the drain truncates to the cap, sets the flag, and wakes the
        // invocation future exactly once.
        let over_cap = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(2);
        let flag = Arc::clone(&over_cap);
        let data = [b'x'; 100];
        let bytes = runtime
            .block_on(drain_git_pipe(&data[..], 16, flag, tx))
            .expect("capped drain reads without error");
        assert_eq!(bytes.len(), 16, "the drain truncates to the cap");
        assert!(over_cap.load(Ordering::Acquire), "over-cap flag must be set");
        assert!(
            matches!(rx.try_recv(), Ok(())),
            "over-cap must wake the invocation future"
        );

        // Under-cap: the drain reads to EOF and never signals.
        let under = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(2);
        let flag = Arc::clone(&under);
        let data = [b'y'; 8];
        let bytes = runtime
            .block_on(drain_git_pipe(&data[..], 16, flag, tx))
            .expect("under-cap drain reads without error");
        assert_eq!(bytes, [b'y'; 8], "under-cap output is returned whole");
        assert!(
            !under.load(Ordering::Acquire),
            "under-cap must not set the over-cap flag"
        );
        assert!(
            rx.try_recv().is_err(),
            "under-cap must not wake the invocation future"
        );
    }
}
