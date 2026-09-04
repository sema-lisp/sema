use crate::build_interpreter;
use crate::drain_async_scheduler;
use crate::import_tracer;
use crate::print_cli_warning;
use crate::print_error;
use crate::read_source_file;
use crate::resolve_approval_mode;
use crate::validate_interactive_approval_authority;
use crate::workflow_check;
use crate::ApprovalMode;
use crate::ApprovalPromptCtrlCGuard;
use crate::WorkflowCommands;
use crate::{die, print_cli_error};
use sema::workflow_view;
use sema_core::pretty_print;
use sema_core::Value;
use std::io::IsTerminal;
use std::path::Path;
use std::path::PathBuf;

fn default_approval_actor() -> String {
    std::env::var("SEMA_APPROVAL_ACTOR")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "local-user".to_string())
}

fn read_approval_key_file(path: &Path, private: bool) -> Result<String, String> {
    use std::io::Read as _;

    const MAX_KEY_BYTES: u64 = 16 * 1024;
    let file = if private {
        sema_workflow::approval::open_private_file(path)
    } else {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        options.open(path)
    }
    .map_err(|error| format!("cannot open approval key {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect approval key {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > MAX_KEY_BYTES {
        return Err(format!(
            "{} is too large to be an approval key",
            path.display()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_KEY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read approval key {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_KEY_BYTES {
        return Err(format!(
            "{} is too large to be an approval key",
            path.display()
        ));
    }
    let value = String::from_utf8(bytes)
        .map_err(|_| format!("approval key {} is not UTF-8", path.display()))?;
    if private && value.trim().is_empty() {
        return Err("approval signing key file is empty".to_string());
    }
    Ok(value.trim().to_string())
}

fn load_approval_signing_key(
    path: &Path,
) -> Result<sema_workflow::approval::ApprovalSigningKey, String> {
    let encoded = read_approval_key_file(path, true)?;
    sema_workflow::approval::ApprovalSigningKey::from_base64(&encoded)
        .map_err(|error| error.to_string())
}

fn create_approval_key_pair(private_path: &Path, public_path: &Path) -> Result<(), String> {
    use std::io::Write as _;

    if private_path == public_path {
        return Err("private and public approval key paths must differ".to_string());
    }
    let key = sema_workflow::approval::ApprovalSigningKey::generate()
        .map_err(|error| error.to_string())?;
    let mut private = sema_workflow::approval::create_private_file_new(private_path)
        .map_err(|error| format!("cannot create {}: {error}", private_path.display()))?;
    let mut public_file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(public_path)
    {
        Ok(file) => file,
        Err(error) => {
            drop(private);
            let _ = std::fs::remove_file(private_path);
            return Err(format!("cannot create {}: {error}", public_path.display()));
        }
    };
    let write_result = private
        .write_all(format!("{}\n", key.to_base64()).as_bytes())
        .and_then(|()| private.sync_all())
        .map_err(|error| format!("cannot write {}: {error}", private_path.display()))
        .and_then(|()| {
            let public = key.public_key_base64().map_err(|error| error.to_string())?;
            public_file
                .write_all(format!("{public}\n").as_bytes())
                .and_then(|()| public_file.sync_all())
                .map_err(|error| format!("cannot write {}: {error}", public_path.display()))
        });
    drop(private);
    drop(public_file);
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(public_path);
        let _ = std::fs::remove_file(private_path);
        return Err(error);
    }
    Ok(())
}

struct WorkflowApprovalRevision {
    digest: String,
    embedded_dependencies: Vec<(PathBuf, Vec<u8>)>,
}

fn workflow_approval_revision(
    file: &Path,
    content: &[u8],
) -> Result<WorkflowApprovalRevision, String> {
    use sha2::{Digest as _, Sha256};

    const MAX_FILES: usize = 4096;
    const MAX_BYTES: usize = 64 * 1024 * 1024;

    let snapshot = import_tracer::trace_imports_strict(file)?;
    let mut dependencies = snapshot.files.into_iter().collect::<Vec<_>>();
    dependencies.sort_by(|left, right| left.0.cmp(&right.0));
    if dependencies.len() > MAX_FILES {
        return Err(format!(
            "workflow dependency closure exceeds {MAX_FILES} files"
        ));
    }

    let mut manifests = Vec::with_capacity(2);

    let mut ancestor = file
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize {}: {error}", file.display()))?
        .parent()
        .map(Path::to_path_buf);
    while let Some(directory) = ancestor {
        for name in ["sema.toml", "sema.lock"] {
            let path = directory.join(name);
            if path.is_file()
                && !dependencies.iter().any(|(key, _)| key == name)
                && !manifests.iter().any(|(key, _)| key == name)
            {
                let bytes = std::fs::read(&path)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
                manifests.push((name.to_string(), bytes));
            }
        }
        ancestor = directory.parent().map(Path::to_path_buf);
    }

    let mut inputs = Vec::with_capacity(dependencies.len() + manifests.len() + 1);
    inputs.push(("<entry>", content));
    inputs.extend(
        dependencies
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
    );
    inputs.extend(
        manifests
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
    );
    inputs.sort_by(|left, right| left.0.cmp(right.0));

    let total_bytes = inputs.iter().try_fold(0usize, |total, (_, bytes)| {
        total
            .checked_add(bytes.len())
            .ok_or_else(|| "workflow dependency closure byte count overflowed".to_string())
    })?;
    if total_bytes > MAX_BYTES {
        return Err(format!(
            "workflow dependency closure exceeds {MAX_BYTES} bytes"
        ));
    }

    let mut digest = Sha256::new();
    digest.update(b"sema-workflow-approval-revision-v1");
    for (name, bytes) in inputs {
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }

    let mut embedded_dependencies = std::collections::BTreeMap::new();
    for (name, bytes) in dependencies {
        embedded_dependencies.insert(PathBuf::from(name), bytes);
    }
    for (filesystem_path, bytes) in snapshot.filesystem_files {
        match embedded_dependencies.entry(filesystem_path.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(bytes.clone());
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() == &bytes => {}
            std::collections::btree_map::Entry::Occupied(entry) => {
                return Err(format!(
                    "dependency snapshot key collision at {}",
                    entry.key().display()
                ));
            }
        }
        let alias = sema_core::vfs::normalize_path(&filesystem_path).ok_or_else(|| {
            format!(
                "cannot normalize imported file identity {}",
                filesystem_path.display()
            )
        })?;
        match embedded_dependencies.entry(PathBuf::from(alias)) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(bytes);
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() == &bytes => {}
            std::collections::btree_map::Entry::Occupied(entry) => {
                return Err(format!(
                    "dependency snapshot key collision at {}",
                    entry.key().display()
                ));
            }
        }
    }

    Ok(WorkflowApprovalRevision {
        digest: format!("{:x}", digest.finalize()),
        embedded_dependencies: embedded_dependencies.into_iter().collect(),
    })
}

struct WorkflowApprovalInput<'a> {
    runs_root: &'a Path,
    run_id: &'a str,
    approval_id: &'a str,
    signing_key: &'a sema_workflow::approval::ApprovalSigningKey,
    kind: sema_workflow::approval::ApprovalDecisionKind,
    actor: String,
    comment: Option<String>,
    reason: Option<String>,
}

fn write_workflow_approval(input: WorkflowApprovalInput<'_>) {
    match sema_workflow::approval::decide(
        input.runs_root,
        input.run_id,
        input.approval_id,
        input.signing_key,
        input.kind,
        input.actor,
        "cli".to_string(),
        input.comment,
        input.reason,
    ) {
        Ok(sema_workflow::approval::DecisionWrite::Created(decision)) => println!(
            "{} {} ({})",
            approval_decision_past_tense(decision.decision),
            decision.approval_id,
            decision.decision_id
        ),
        Ok(sema_workflow::approval::DecisionWrite::AlreadyExists(decision)) => println!(
            "already {} {} ({})",
            approval_decision_past_tense(decision.decision),
            decision.approval_id,
            decision.decision_id
        ),
        Err(error) => {
            die(format!(
                "cannot {} approval {}: {error}",
                input.kind, input.approval_id
            ));
        }
    }
}

fn approval_decision_past_tense(
    decision: sema_workflow::approval::ApprovalDecisionKind,
) -> &'static str {
    match decision {
        sema_workflow::approval::ApprovalDecisionKind::Approve => "approved",
        sema_workflow::approval::ApprovalDecisionKind::Reject => "rejected",
    }
}

fn approval_resolution_json(
    resolution: &sema_workflow::approval::ApprovalResolution,
) -> serde_json::Value {
    use sema_workflow::approval::ApprovalResolution;
    let (status, request, decision) = match resolution {
        ApprovalResolution::Pending(request) => ("pending", request, None),
        ApprovalResolution::Approved(request, decision) => ("approved", request, Some(decision)),
        ApprovalResolution::Rejected(request, decision) => ("rejected", request, Some(decision)),
    };
    serde_json::json!({
        "status": status,
        "request": request,
        "decision": decision,
    })
}

fn list_workflow_approvals(runs_root: &Path, run_id: &str, json: bool) {
    let resolutions = match sema_workflow::approval::list_requests(runs_root, run_id) {
        Ok(resolutions) => resolutions,
        Err(error) => {
            die(format!("cannot list approvals for {run_id}: {error}"));
        }
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &resolutions
                    .iter()
                    .map(approval_resolution_json)
                    .collect::<Vec<_>>()
            )
            .expect("approval list is JSON serializable")
        );
        return;
    }
    if resolutions.is_empty() {
        println!("no approvals for run {run_id}");
        return;
    }
    for resolution in &resolutions {
        let json = approval_resolution_json(resolution);
        let request = &json["request"];
        println!(
            "{}  {:<8}  {}",
            request["approval_id"].as_str().unwrap_or(""),
            json["status"].as_str().unwrap_or("unknown"),
            terminal_safe(request["reason"].as_str().unwrap_or(""))
        );
        if let Some(preview) = request["preview"].as_str() {
            println!("  {}", terminal_safe(preview));
        }
    }
}

enum ApprovalPromptAction {
    Approve,
    Reject(String),
    AlreadyDecided,
    LeavePending,
}

fn is_terminal_control(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{206f}'
        )
}

pub(crate) fn terminal_safe(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for ch in value.chars() {
        if is_terminal_control(ch) {
            use std::fmt::Write as _;
            let _ = write!(safe, "\\u{{{:x}}}", ch as u32);
        } else {
            safe.push(ch);
        }
    }
    safe
}

fn prompt_for_workflow_approval(
    runs_root: &Path,
    run_id: &str,
    approval_id: &str,
) -> Result<ApprovalPromptAction, String> {
    use std::io::Write;

    let resolution = sema_workflow::approval::list_requests(runs_root, run_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|resolution| {
            let request = match resolution {
                sema_workflow::approval::ApprovalResolution::Pending(request)
                | sema_workflow::approval::ApprovalResolution::Approved(request, _)
                | sema_workflow::approval::ApprovalResolution::Rejected(request, _) => request,
            };
            request.approval_id == approval_id
        })
        .ok_or_else(|| format!("approval {approval_id} was not found"))?;
    let request = match resolution {
        sema_workflow::approval::ApprovalResolution::Pending(request) => request,
        sema_workflow::approval::ApprovalResolution::Approved(_, decision)
        | sema_workflow::approval::ApprovalResolution::Rejected(_, decision) => {
            eprintln!(
                "approval {approval_id} was already {} by {}",
                decision.decision,
                terminal_safe(&decision.actor)
            );
            return Ok(ApprovalPromptAction::AlreadyDecided);
        }
    };

    let _ctrlc_guard = ApprovalPromptCtrlCGuard::enter();
    eprintln!("\nHuman approval required");
    eprintln!("  id:      {}", request.approval_id);
    eprintln!("  reason:  {}", terminal_safe(&request.reason));
    if let Some(preview) = request.preview {
        eprintln!("  preview: {}", terminal_safe(&preview));
    }
    loop {
        eprint!("Approve? [y]es / [n]o / [q]uit pending: ");
        std::io::stderr()
            .flush()
            .map_err(|error| error.to_string())?;
        let mut answer = String::new();
        let read = std::io::stdin()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(ApprovalPromptAction::LeavePending);
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(ApprovalPromptAction::Approve),
            "n" | "no" => {
                eprint!("Rejection reason: ");
                std::io::stderr()
                    .flush()
                    .map_err(|error| error.to_string())?;
                let mut reason = String::new();
                if std::io::stdin()
                    .read_line(&mut reason)
                    .map_err(|error| error.to_string())?
                    == 0
                {
                    return Ok(ApprovalPromptAction::LeavePending);
                }
                if reason.trim().is_empty() {
                    eprintln!("A rejection reason is required.");
                    continue;
                }
                return Ok(ApprovalPromptAction::Reject(reason.trim().to_string()));
            }
            "q" | "quit" => return Ok(ApprovalPromptAction::LeavePending),
            _ => eprintln!("Enter y, n, or q."),
        }
    }
}

fn approval_was_decided(runs_root: &Path, run_id: &str, approval_id: &str) -> bool {
    sema_workflow::approval::list_requests(runs_root, run_id)
        .ok()
        .is_some_and(|resolutions| {
            resolutions.into_iter().any(|resolution| match resolution {
                sema_workflow::approval::ApprovalResolution::Pending(_) => false,
                sema_workflow::approval::ApprovalResolution::Approved(request, _)
                | sema_workflow::approval::ApprovalResolution::Rejected(request, _) => {
                    request.approval_id == approval_id
                }
            })
        })
}

fn approval_envelope_field(envelope: &Value, key: &str) -> Option<String> {
    envelope
        .as_map_rc()?
        .get(&Value::keyword(key))?
        .as_str()
        .map(str::to_string)
}

pub(crate) fn shell_quote(value: &str) -> String {
    #[cfg(windows)]
    {
        return format!("\"{}\"", value.replace('"', "\\\""));
    }
    #[cfg(not(windows))]
    {
        use std::fmt::Write as _;

        // ANSI-C quotes preserve exact control/bidi characters without rendering them
        // into the terminal. Sema's supported Unix shells (bash/zsh) understand this
        // form, including \xHH/\uHHHH/\UHHHHHHHH escapes.
        let mut quoted = String::with_capacity(value.len() + 3);
        quoted.push_str("$'");
        for ch in value.chars() {
            match ch {
                '\\' => quoted.push_str("\\\\"),
                '\'' => quoted.push_str("\\'"),
                _ if is_terminal_control(ch) => {
                    let scalar = ch as u32;
                    if scalar <= 0xff {
                        let _ = write!(quoted, "\\x{scalar:02x}");
                    } else if scalar <= 0xffff {
                        let _ = write!(quoted, "\\u{scalar:04x}");
                    } else {
                        let _ = write!(quoted, "\\U{scalar:08x}");
                    }
                }
                _ => quoted.push(ch),
            }
        }
        quoted.push('\'');
        quoted
    }
}

pub(crate) fn format_needs_approval_guidance(
    envelope: &Value,
    run_dir: &str,
    file: &str,
    args: &str,
    public_key_file: Option<&str>,
    signing_key_file: Option<&str>,
) -> String {
    let run_id = approval_envelope_field(envelope, "run-id").unwrap_or_default();
    let approval_id = approval_envelope_field(envelope, "approval-id").unwrap_or_default();
    let mut out = format!(
        "run needs human approval {approval_id}:\n  sema workflow approve {} {} --run-dir {} --signing-key-file \"$SEMA_APPROVAL_PRIVATE_KEY\"\n  sema workflow reject {} {} --run-dir {} --reason 'explain why' --signing-key-file \"$SEMA_APPROVAL_PRIVATE_KEY\"\n",
        shell_quote(&run_id),
        shell_quote(&approval_id),
        shell_quote(run_dir),
        shell_quote(&run_id),
        shell_quote(&approval_id),
        shell_quote(run_dir),
    );
    if let Some(signing_key_file) = signing_key_file {
        out.push_str(&format!(
            "or decide in the loopback viewer, then resume exactly:\n  sema workflow run {} --args {} --resume {} --run-dir {} --view --approval-signing-key-file {}\n",
            shell_quote(file),
            shell_quote(args),
            shell_quote(&run_id),
            shell_quote(run_dir),
            shell_quote(signing_key_file),
        ));
    } else if let Some(public_key_file) = public_key_file {
        out.push_str(&format!(
            "then resume exactly:\n  sema workflow run {} --args {} --resume {} --run-dir {} --approval-public-key-file {}\n",
            shell_quote(file),
            shell_quote(args),
            shell_quote(&run_id),
            shell_quote(run_dir),
            shell_quote(public_key_file),
        ));
    } else {
        out.push_str(
            "this interactive request used an ephemeral authority; if left pending, start a fresh run instead of resuming it.\n",
        );
    }
    out
}

/// `sema workflow run <file>` — evaluate a workflow `.sema` file (which
/// `defworkflow`s and runs it) with the run-directory + args seams wired, then
/// exit non-zero if the run's `{:status …}` envelope reports failure.
pub(crate) fn run_workflow_command(command: WorkflowCommands, sandbox: &sema_core::Sandbox) {
    let (
        file,
        args,
        run_dir,
        view,
        view_port,
        resume,
        no_auth_prompt,
        approval_mode,
        approval_public_key_file,
        approval_signing_key_file,
        approval_actor,
    ) = match command {
        WorkflowCommands::Run {
            file,
            args,
            run_dir,
            view,
            port,
            resume,
            no_auth_prompt,
            approval_mode,
            approval_public_key_file,
            approval_signing_key_file,
            approval_actor,
        } => (
            file,
            args,
            run_dir,
            view,
            port,
            resume,
            no_auth_prompt,
            approval_mode,
            approval_public_key_file,
            approval_signing_key_file,
            approval_actor,
        ),
        WorkflowCommands::Approvals {
            run_id,
            run_dir,
            json,
        } => {
            list_workflow_approvals(Path::new(&run_dir), &run_id, json);
            return;
        }
        WorkflowCommands::Approve {
            run_id,
            approval_id,
            run_dir,
            comment,
            actor,
            signing_key_file,
        } => {
            let signing_key = load_approval_signing_key(Path::new(&signing_key_file))
                .unwrap_or_else(|error| {
                    die(error);
                });
            write_workflow_approval(WorkflowApprovalInput {
                runs_root: Path::new(&run_dir),
                run_id: &run_id,
                approval_id: &approval_id,
                signing_key: &signing_key,
                kind: sema_workflow::approval::ApprovalDecisionKind::Approve,
                actor: actor.unwrap_or_else(default_approval_actor),
                comment,
                reason: None,
            });
            return;
        }
        WorkflowCommands::Reject {
            run_id,
            approval_id,
            run_dir,
            reason,
            actor,
            signing_key_file,
        } => {
            let signing_key = load_approval_signing_key(Path::new(&signing_key_file))
                .unwrap_or_else(|error| {
                    die(error);
                });
            write_workflow_approval(WorkflowApprovalInput {
                runs_root: Path::new(&run_dir),
                run_id: &run_id,
                approval_id: &approval_id,
                signing_key: &signing_key,
                kind: sema_workflow::approval::ApprovalDecisionKind::Reject,
                actor: actor.unwrap_or_else(default_approval_actor),
                comment: None,
                reason: Some(reason),
            });
            return;
        }
        WorkflowCommands::ApprovalKeygen {
            private_key_file,
            public_key_file,
        } => {
            if let Err(error) =
                create_approval_key_pair(Path::new(&private_key_file), Path::new(&public_key_file))
            {
                die(error);
            }
            println!("created private approval key {private_key_file}");
            println!("created public approval key  {public_key_file}");
            return;
        }
        WorkflowCommands::View {
            run_dir,
            host,
            port,
            approval_signing_key_file,
            approval_actor,
        } => {
            let approval_authority = approval_signing_key_file
                .as_deref()
                .map(|path| {
                    let signing_key = load_approval_signing_key(Path::new(path))?;
                    workflow_view::ApprovalAuthority::new(
                        signing_key,
                        approval_actor.unwrap_or_else(default_approval_actor),
                    )
                    .map_err(|error| error.to_string())
                })
                .transpose()
                .unwrap_or_else(|error| {
                    die(format!("cannot enable viewer approval controls: {error}"));
                });
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime")
                .block_on(workflow_view::serve_with_approval(
                    PathBuf::from(run_dir),
                    &host,
                    port,
                    approval_authority,
                ));
            return;
        }
        WorkflowCommands::Index { run_dir } => {
            let root = PathBuf::from(&run_dir);
            match workflow_view::ingest::open(&root.join(sema_workflow::INDEX_DB)) {
                Ok(conn) => {
                    workflow_view::ingest::backfill_all(&conn, &root);
                    match workflow_view::ingest::runs_summary(&conn) {
                        Ok(rows) => println!(
                            "indexed {} run(s) → {}",
                            rows.len(),
                            root.join(sema_workflow::INDEX_DB).display()
                        ),
                        Err(e) => print_cli_warning(format!("could not summarize index: {e}")),
                    }
                }
                Err(e) => {
                    die(format!("cannot open index database: {e}"));
                }
            }
            return;
        }
        WorkflowCommands::Export {
            run_id,
            run_dir,
            out_dir,
        } => {
            match sema::workflow_evidence::export(
                &PathBuf::from(run_dir),
                &run_id,
                out_dir.as_deref().map(std::path::Path::new),
            ) {
                Ok(bundle) => {
                    println!(
                        "exported workflow evidence → {}",
                        bundle.directory.display()
                    );
                    println!("  {}", bundle.evidence_json.display());
                    println!("  {}", bundle.evidence_markdown.display());
                    println!("  {}", bundle.manifest_json.display());
                }
                Err(error) => {
                    die(format!("cannot export workflow evidence: {error}"));
                }
            }
            return;
        }
        WorkflowCommands::Check { file, strict, json } => {
            let src = match read_source_file(&file) {
                Ok(s) => s,
                Err(msg) => {
                    print_cli_error(msg);
                    std::process::exit(2);
                }
            };
            let diags = workflow_check::check_run_source(&src);
            std::process::exit(workflow_check::report(&file, &diags, strict, json));
        }
    };

    let workspace_root = std::env::current_dir().unwrap_or_else(|error| {
        die(format!("cannot resolve workflow workspace: {error}"));
    });
    let run_dir = {
        let path = PathBuf::from(&run_dir);
        let absolute = if path.is_absolute() {
            path
        } else {
            workspace_root.join(path)
        };
        absolute.to_string_lossy().to_string()
    };
    let file = std::fs::canonicalize(&file).unwrap_or_else(|error| {
        die(format!("cannot resolve workflow file {file}: {error}"));
    });
    let file = file.to_string_lossy().to_string();
    let approval_public_key_file = approval_public_key_file.map(|value| {
        let path = PathBuf::from(value);
        let absolute = if path.is_absolute() {
            path
        } else {
            workspace_root.join(path)
        };
        absolute.to_string_lossy().to_string()
    });
    let approval_signing_key_file = approval_signing_key_file.map(|value| {
        let path = PathBuf::from(value);
        let absolute = if path.is_absolute() {
            path
        } else {
            workspace_root.join(path)
        };
        absolute.to_string_lossy().to_string()
    });

    // Interactive MCP auth (docs/plans/2026-06-24-workflow-mcp-auth.md §3): on a
    // real terminal, a needs-auth gate logs in inline instead of exiting 2. See
    // `should_enable_interactive_auth` for the exact decision and
    // `sema::workflow_mcp::set_interactive_auth` for what enabling it does.
    sema::workflow_mcp::set_interactive_auth(should_enable_interactive_auth(
        std::io::stdin().is_terminal(),
        std::io::stderr().is_terminal(),
        std::env::var("CI").ok().as_deref(),
        no_auth_prompt,
    ));

    // Backward-compatible host seam for deterministic tests/embedders. Read exactly
    // once before evaluation and copy into host-owned state; a workflow's later
    // `sys/set-env` calls cannot alter it.
    let fresh_run_id = std::env::var("SEMA_WORKFLOW_RUN_ID")
        .ok()
        .filter(|value| !value.is_empty());
    if let Some(run_id) = &fresh_run_id {
        if run_id.contains('/') || run_id.contains('\\') || run_id.contains("..") {
            die("SEMA_WORKFLOW_RUN_ID must be a bare directory name without path separators");
        }
    }

    // `--resume <run-id>`: reuse that run's dir + memo cache. Sanitize the operator-
    // supplied id against path traversal (it joins into a filesystem path) and require
    // the prior run's events.jsonl to exist. The values are installed below in immutable
    // host state rather than mutable process environment variables.
    if let Some(run_id) = &resume {
        if run_id.is_empty()
            || run_id.contains('/')
            || run_id.contains('\\')
            || run_id.contains("..")
        {
            die("--resume run-id must be a bare directory name without path separators");
        }
        let prior = PathBuf::from(&run_dir).join(run_id).join("events.jsonl");
        if !prior.exists() {
            die(format!("no prior run to resume at {}", prior.display()));
        }
    }

    let content = match read_source_file(&file) {
        Ok(c) => c,
        Err(msg) => {
            die(msg);
        }
    };
    let run_diags = workflow_check::check_run_source(&content);
    if run_diags
        .iter()
        .any(|diag| diag.severity == workflow_check::Severity::Error)
    {
        let code = workflow_check::report(&file, &run_diags, false, false);
        std::process::exit(code.max(1));
    }

    let approval_revision = workflow_approval_revision(Path::new(&file), content.as_bytes())
        .unwrap_or_else(|error| {
            die(format!(
                "cannot bind workflow approval dependency closure: {error}"
            ));
        });
    let approval_code_version = approval_revision.digest.clone();

    let mut effective_sandbox = sandbox.clone();
    let permission_specs = match workflow_check::declared_permission_specs(&content) {
        Ok(specs) => specs,
        Err(e) => {
            die(format!("invalid workflow permissions: {e}"));
        }
    };
    for spec in permission_specs {
        let declared = sema_core::Sandbox::parse_cli(&spec).unwrap_or_else(|e| {
            die(format!("invalid defworkflow :permissions {spec:?}: {e}"));
        });
        effective_sandbox = effective_sandbox.with_more_denied(declared.denied);
    }

    // Bind the parsed --args JSON object to the global `*workflow-args*` so the
    // workflow body can read its inputs.
    let args_value = match serde_json::from_str::<serde_json::Value>(&args) {
        Ok(json) => sema_core::json::json_to_value(&json),
        Err(e) => {
            die(format!("--args is not valid JSON: {e}"));
        }
    };
    // Preserve the established short source hash for memo compatibility. Approval
    // decisions use a separate collision-resistant source binding below.
    let code_version = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    };

    // The file's last form is the `defworkflow` (which expands to `workflow/run`),
    // so eval returns the `{:status …}` envelope; journaling is its side effect. A TTY
    // approval decision starts a fresh interpreter and resumes the same run. Completed
    // leaves replay from their memos; no interpreter-local continuation is trusted.
    let stdin_tty = std::io::stdin().is_terminal();
    let stderr_tty = std::io::stderr().is_terminal();
    let interactive_approval = stdin_tty
        && stderr_tty
        && std::env::var("CI")
            .ok()
            .as_deref()
            .is_none_or(str::is_empty);
    let viewer_signing_key = approval_signing_key_file
        .as_deref()
        .map(|path| load_approval_signing_key(Path::new(path)))
        .transpose()
        .unwrap_or_else(|error| {
            die(format!("cannot enable viewer approval controls: {error}"));
        });
    let viewer_public_key = viewer_signing_key.as_ref().map(|key| {
        key.public_key_base64().unwrap_or_else(|error| {
            die(format!("cannot derive viewer approval key: {error}"));
        })
    });
    let configured_public_key = approval_public_key_file.as_deref().map(|path| {
        let encoded = read_approval_key_file(Path::new(path), false).unwrap_or_else(|error| {
            die(error);
        });
        sema_workflow::approval::normalize_public_key_base64(&encoded).unwrap_or_else(|error| {
            die(format!("invalid approval public key: {error}"));
        })
    });
    if viewer_public_key
        .as_ref()
        .zip(configured_public_key.as_ref())
        .is_some_and(|(viewer, configured)| viewer != configured)
    {
        die("--approval-signing-key-file does not match --approval-public-key-file");
    }
    let has_durable_authority = viewer_signing_key.is_some() || configured_public_key.is_some();
    let effective_approval_mode =
        resolve_approval_mode(approval_mode, interactive_approval, has_durable_authority);
    if let Err(error) = validate_interactive_approval_authority(
        effective_approval_mode,
        interactive_approval,
        configured_public_key.is_some(),
        viewer_signing_key.is_some(),
    ) {
        die(error);
    }
    let needs_ephemeral_authority = (effective_approval_mode == ApprovalMode::Prompt
        && interactive_approval)
        || (effective_approval_mode == ApprovalMode::Deny && !has_durable_authority);
    let inline_signing_key = if needs_ephemeral_authority {
        Some(viewer_signing_key.clone().unwrap_or_else(|| {
            sema_workflow::approval::ApprovalSigningKey::generate().unwrap_or_else(|error| {
                die(format!("cannot generate interactive approval key: {error}"));
            })
        }))
    } else {
        None
    };
    let approval_public_key = if let Some(key) = &viewer_public_key {
        key.clone()
    } else if let Some(key) = &configured_public_key {
        key.clone()
    } else if let Some(key) = &inline_signing_key {
        key.public_key_base64().unwrap_or_else(|error| {
            die(format!("cannot derive interactive approval key: {error}"));
        })
    } else {
        String::new()
    };
    // An in-memory prompt authority is ephemeral unless it came from the explicit
    // viewer key file. Guidance only promises a resumable authority when the operator
    // supplied one of those durable files.
    let resumable_public_key_file = viewer_signing_key
        .is_none()
        .then_some(approval_public_key_file.as_deref())
        .flatten();
    let resumable_signing_key_file = viewer_signing_key
        .is_some()
        .then_some(approval_signing_key_file.as_deref())
        .flatten();

    // Start the live viewer after loading the host-only signing authority but before
    // evaluation, so the browser can observe the flush-per-event journal immediately.
    // The key remains in server state and is never installed in the interpreter.
    if view {
        let vd = run_dir.clone();
        let viewer_authority = viewer_signing_key.clone().map(|signing_key| {
            workflow_view::ApprovalAuthority::new(
                signing_key,
                approval_actor
                    .clone()
                    .unwrap_or_else(default_approval_actor),
            )
            .unwrap_or_else(|error| {
                die(format!("cannot enable viewer approval controls: {error}"));
            })
        });
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime")
                .block_on(async {
                    if let Err(error) = workflow_view::serve_result_with_approval(
                        PathBuf::from(vd),
                        "127.0.0.1",
                        view_port,
                        false,
                        viewer_authority,
                    )
                    .await
                    {
                        print_cli_warning(format!("--view could not start the viewer: {error}"));
                    }
                });
        });
        let url = format!("http://127.0.0.1:{view_port}");
        println!("Live viewer: {url}");
        open_in_browser(&url);
        // Give the listener a moment to bind before the run starts producing events.
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    // Snapshot audit identity before evaluating untrusted workflow code. A workflow may
    // have env-write permission, but cannot rewrite the actor attached to a later host
    // terminal decision.
    let terminal_approval_actor = default_approval_actor();
    let mut active_resume = resume.clone();
    let exit_code = loop {
        let interpreter = build_interpreter(&effective_sandbox);
        for (path, bytes) in &approval_revision.embedded_dependencies {
            interpreter
                .ctx
                .set_embedded_file(path.clone(), bytes.clone());
        }
        let _host_config = sema_workflow::context::install_host_config(
            sema_workflow::context::WorkflowHostConfig {
                runs_root: run_dir.clone(),
                explicit_run_id: active_resume.clone().or_else(|| fresh_run_id.clone()),
                resuming: active_resume.is_some(),
                code_version: code_version.clone(),
                approval_code_version: approval_code_version.clone(),
                args_json: args.clone(),
                approval_public_key: approval_public_key.clone(),
                entry_file: file.clone(),
                workspace_root: workspace_root.to_string_lossy().to_string(),
            },
        );

        // Auto-configure an LLM provider from the environment (mirrors the default run
        // path), so a workflow whose leaves call `llm/*` works without self-configuring.
        // Best-effort: a workflow with no LLM leaves needs no provider, so ignore errors.
        let _ = interpreter.eval_str("(llm/auto-configure)");
        // Evaluate against the same complete static dependency snapshot that was
        // hashed into the approval revision. Macro-generated or runtime-selected
        // imports outside that closure fail closed instead of escaping the binding.
        interpreter.ctx.set_embedded_files_only(true);
        interpreter
            .global_env
            .set(sema_core::intern("*workflow-args*"), args_value.clone());

        let envelope = match interpreter.eval_str_compiled(&content) {
            Ok(envelope) => {
                drain_async_scheduler(&interpreter);
                envelope
            }
            Err(e) => {
                eprint!("Error running workflow {file}: ");
                print_error(&e);
                break 1;
            }
        };
        // Tear down the evaluator (including detached descendants) before any private
        // inline signing key is used at the terminal prompt.
        drop(interpreter);
        let status = envelope
            .as_map_rc()
            .and_then(|m| m.get(&Value::keyword("status")).cloned())
            .and_then(|s| s.as_keyword());
        match status.as_deref() {
            Some("failed") => {
                print_cli_error(format!("workflow failed: {}", pretty_print(&envelope, 80)));
                break 1;
            }
            Some("rejected") => {
                print_cli_error(format!(
                    "workflow approval rejected: {}",
                    pretty_print(&envelope, 80)
                ));
                break 1;
            }
            // The headless-precursor gate (docs/plans/2026-06-24-workflow-mcp-auth.md
            // §3/§5): a declared `:mcp` server had no usable session. Distinct exit
            // code so a CI/orchestrator script can branch on "needs a human to log
            // in" vs. a genuine failure.
            Some("needs-auth") => {
                eprint!("{}", format_needs_auth_guidance(&envelope));
                break 2;
            }
            Some("needs-approval") => {
                let Some(run_id) = approval_envelope_field(&envelope, "run-id") else {
                    print_cli_error("needs-approval envelope is missing :run-id");
                    break 1;
                };
                let Some(approval_id) = approval_envelope_field(&envelope, "approval-id") else {
                    print_cli_error("needs-approval envelope is missing :approval-id");
                    break 1;
                };
                match effective_approval_mode {
                    ApprovalMode::Pause | ApprovalMode::Auto => {
                        eprint!(
                            "{}",
                            format_needs_approval_guidance(
                                &envelope,
                                &run_dir,
                                &file,
                                &args,
                                resumable_public_key_file,
                                resumable_signing_key_file,
                            )
                        );
                        break 3;
                    }
                    ApprovalMode::Deny => {
                        print_cli_error(format!(
                            "workflow reached approval {approval_id}; --approval-mode deny refuses interactive approval"
                        ));
                        break 1;
                    }
                    ApprovalMode::Prompt if !interactive_approval => {
                        print_cli_error(
                            "--approval-mode prompt requires terminal stdin/stderr and is disabled in CI",
                        );
                        eprint!(
                            "{}",
                            format_needs_approval_guidance(
                                &envelope,
                                &run_dir,
                                &file,
                                &args,
                                resumable_public_key_file,
                                resumable_signing_key_file,
                            )
                        );
                        break 3;
                    }
                    ApprovalMode::Prompt => {
                        let action = match prompt_for_workflow_approval(
                            Path::new(&run_dir),
                            &run_id,
                            &approval_id,
                        ) {
                            Ok(action) => action,
                            Err(error) => {
                                print_cli_error(format!("cannot prompt for approval: {error}"));
                                break 3;
                            }
                        };
                        let decision = match action {
                            ApprovalPromptAction::Approve => sema_workflow::approval::decide(
                                Path::new(&run_dir),
                                &run_id,
                                &approval_id,
                                inline_signing_key.as_ref().expect("interactive key exists"),
                                sema_workflow::approval::ApprovalDecisionKind::Approve,
                                terminal_approval_actor.clone(),
                                "terminal-prompt".to_string(),
                                None,
                                None,
                            ),
                            ApprovalPromptAction::Reject(reason) => {
                                sema_workflow::approval::decide(
                                    Path::new(&run_dir),
                                    &run_id,
                                    &approval_id,
                                    inline_signing_key.as_ref().expect("interactive key exists"),
                                    sema_workflow::approval::ApprovalDecisionKind::Reject,
                                    terminal_approval_actor.clone(),
                                    "terminal-prompt".to_string(),
                                    None,
                                    Some(reason),
                                )
                            }
                            ApprovalPromptAction::AlreadyDecided => {
                                active_resume = Some(run_id);
                                continue;
                            }
                            ApprovalPromptAction::LeavePending => {
                                eprint!(
                                    "{}",
                                    format_needs_approval_guidance(
                                        &envelope,
                                        &run_dir,
                                        &file,
                                        &args,
                                        resumable_public_key_file,
                                        resumable_signing_key_file,
                                    )
                                );
                                break 3;
                            }
                        };
                        if let Err(error) = decision {
                            if approval_was_decided(Path::new(&run_dir), &run_id, &approval_id) {
                                eprintln!(
                                    "approval {approval_id} was decided concurrently; resuming with the recorded decision"
                                );
                                active_resume = Some(run_id);
                                continue;
                            }
                            print_cli_error(format!("cannot record approval decision: {error}"));
                            break 1;
                        }
                        active_resume = Some(run_id);
                        continue;
                    }
                }
            }
            _ => break 0,
        }
    };

    // With `--view`, keep the viewer up so the finished run can be inspected.
    if view {
        println!("\nRun complete — viewer live at http://127.0.0.1:{view_port}  (Ctrl-C to stop)");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    std::process::exit(exit_code);
}

/// Render the stderr guidance for a `{:status :needs-auth :auth [{:server :url
/// :persist} …]}` run envelope: terse, terminal-quiet (no banners), one `sema mcp
/// login` line per server, aliases column-aligned. A malformed/missing `:auth`
/// vector degrades to a 0-server header rather than panicking — the exit code
/// alone (2) is still meaningful to a script even if the guidance text is thin.
fn format_needs_auth_guidance(envelope: &Value) -> String {
    let entries: Vec<(String, String)> = envelope
        .as_map_rc()
        .and_then(|m| m.get(&Value::keyword("auth")).cloned())
        .and_then(|a| a.as_list_rc().or_else(|| a.as_vector_rc()))
        .map(|list| {
            list.iter()
                .filter_map(|entry| {
                    let m = entry.as_map_rc()?;
                    let server = m.get(&Value::keyword("server"))?.as_str()?.to_string();
                    let url = m.get(&Value::keyword("url"))?.as_str()?.to_string();
                    Some((server, url))
                })
                .collect()
        })
        .unwrap_or_default();

    let width = entries.iter().map(|(s, _)| s.len()).max().unwrap_or(0);
    let mut out = format!(
        "run needs authentication for {} MCP server(s):\n",
        entries.len()
    );
    for (server, url) in &entries {
        out.push_str(&format!("  {server:<width$}  sema mcp login {url}\n"));
    }
    out.push_str("then re-run this workflow. (or authenticate from `sema workflow view`)\n");
    out
}

/// Whether `run_workflow_command` should enable inline interactive MCP auth
/// (`sema::workflow_mcp::set_interactive_auth`) for this run: a needs-auth
/// gate logs in right there instead of exiting 2. Pure over its inputs — no
/// direct `IsTerminal`/`env::var` calls here — so this is unit-testable
/// without a real TTY; the caller supplies `std::io::stdin().is_terminal()`,
/// `std::io::stderr().is_terminal()`, `std::env::var("CI").ok()`, and the
/// `--no-auth-prompt` flag.
///
/// Both stdin AND stderr must be TTYs: stdin implies a human is actually at
/// the keyboard to complete a browser (or, if this run were headless, a
/// device-code) flow; stderr is where the "opening browser…" line and any
/// failure reason land, so it must be a place a human will actually see them
/// — an interactive stdin with redirected stderr (e.g. `sema workflow run x
/// 2>log.txt`) is exactly the case that should NOT pop a browser unannounced.
/// `--no-auth-prompt` and a non-empty `CI` both force the headless path
/// unconditionally, regardless of the TTY checks.
fn should_enable_interactive_auth(
    stdin_is_tty: bool,
    stderr_is_tty: bool,
    ci_env: Option<&str>,
    no_auth_prompt: bool,
) -> bool {
    if no_auth_prompt {
        return false;
    }
    if ci_env.is_some_and(|v| !v.is_empty()) {
        return false;
    }
    stdin_is_tty && stderr_is_tty
}

/// Best-effort: open `url` in the default browser via the platform opener. Silent
/// no-op if the opener isn't present (e.g. headless) — the URL is always printed.
fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn approval_revision_embeds_absolute_import_identity() {
        let root = std::env::temp_dir().join(format!(
            "sema-approval-revision-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let helper = root.join("helper.sema");
        let workflow = root.join("workflow.sema");
        let helper_bytes = b"(define target \"v1\")\n";
        std::fs::write(&helper, helper_bytes).unwrap();
        let source = format!(
            "(import \"helper.sema\")\n(import {})",
            serde_json::to_string(&helper).unwrap()
        );
        std::fs::write(&workflow, &source).unwrap();

        let revision = workflow_approval_revision(&workflow, source.as_bytes()).unwrap();
        assert!(revision
            .embedded_dependencies
            .iter()
            .any(|(path, bytes)| path == &helper && bytes == helper_bytes));

        let _ = std::fs::remove_dir_all(root);
    }

    // ── format_needs_auth_guidance ─────────────────────────────────────────

    #[test]
    fn needs_auth_guidance_matches_the_brief_verbatim() {
        let envelope = sema_reader::read(
            r#"{:status :needs-auth
                :servers ["asana" "linear"]
                :auth [{:server "asana" :url "https://mcp.asana.com/mcp" :persist "workflow"}
                       {:server "linear" :url "https://mcp.linear.app/mcp" :persist "workflow"}]}"#,
        )
        .expect("valid sema literal");

        let expected = concat!(
            "run needs authentication for 2 MCP server(s):\n",
            "  asana   sema mcp login https://mcp.asana.com/mcp\n",
            "  linear  sema mcp login https://mcp.linear.app/mcp\n",
            "then re-run this workflow. (or authenticate from `sema workflow view`)\n",
        );
        assert_eq!(format_needs_auth_guidance(&envelope), expected);
    }

    #[test]
    fn needs_auth_guidance_single_server() {
        let envelope = sema_reader::read(
            r#"{:status :needs-auth
                :servers ["gated"]
                :auth [{:server "gated" :url "http://127.0.0.1:1/mcp" :persist "run"}]}"#,
        )
        .expect("valid sema literal");

        let out = format_needs_auth_guidance(&envelope);
        assert!(out.starts_with("run needs authentication for 1 MCP server(s):\n"));
        assert!(out.contains("  gated  sema mcp login http://127.0.0.1:1/mcp\n"));
        assert!(out
            .ends_with("then re-run this workflow. (or authenticate from `sema workflow view`)\n"));
    }

    #[test]
    fn needs_auth_guidance_missing_auth_vector_degrades_to_zero_servers() {
        let envelope = sema_reader::read(r#"{:status :needs-auth}"#).expect("valid sema literal");
        let out = format_needs_auth_guidance(&envelope);
        assert!(out.starts_with("run needs authentication for 0 MCP server(s):\n"));
    }

    // ── should_enable_interactive_auth ─────────────────────────────────────

    #[test]
    fn interactive_auth_enabled_only_when_both_streams_are_ttys() {
        assert!(should_enable_interactive_auth(true, true, None, false));
        assert!(!should_enable_interactive_auth(false, true, None, false));
        assert!(!should_enable_interactive_auth(true, false, None, false));
        assert!(!should_enable_interactive_auth(false, false, None, false));
    }

    #[test]
    fn interactive_auth_disabled_by_no_auth_prompt_even_on_a_tty() {
        assert!(!should_enable_interactive_auth(true, true, None, true));
    }

    #[test]
    fn interactive_auth_disabled_by_nonempty_ci_even_on_a_tty() {
        assert!(!should_enable_interactive_auth(
            true,
            true,
            Some("true"),
            false
        ));
        assert!(!should_enable_interactive_auth(
            true,
            true,
            Some("1"),
            false
        ));
    }

    #[test]
    fn interactive_auth_ignores_an_empty_ci_value() {
        // `CI=` (set but empty) is treated the same as unset — matches the
        // brief's "env CI is unset/empty" wording exactly.
        assert!(should_enable_interactive_auth(true, true, Some(""), false));
    }
}
