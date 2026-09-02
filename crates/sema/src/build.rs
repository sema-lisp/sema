use crate::cross_compile;
use crate::import_tracer;
use crate::print_cli_warning;
use crate::print_error;
use crate::read_source_file;
use crate::{build_interpreter, resolve_mcp_tool_timeout};
use crate::{die, print_cli_error};
use sema_core::archive;
use sema_eval::Interpreter;
use std::path::Path;

pub(crate) fn run_compile(file: &str, output: Option<&str>) {
    let path = std::path::Path::new(file);
    let source = match read_source_file(path) {
        Ok(s) => s,
        Err(msg) => {
            die(msg);
        }
    };

    // Compute source hash (CRC-32)
    let source_hash = crc32fast::hash(source.as_bytes());

    // Use Interpreter for macro expansion before compilation
    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = build_interpreter(&sandbox);

    let result = match interpreter.compile_to_bytecode(&source) {
        Ok(r) => r,
        Err(e) => {
            die(format!("compilation failed: {}", e.format_plain()));
        }
    };

    // Serialize
    let bytes = match sema_vm::serialize_to_bytes(&result, source_hash) {
        Ok(b) => b,
        Err(e) => {
            die(format!("serialization failed: {}", e.format_plain()));
        }
    };

    // Write output
    let out_path = match output {
        Some(o) => std::path::PathBuf::from(o),
        None => path.with_extension("semac"),
    };
    if let Err(e) = std::fs::write(&out_path, &bytes) {
        die(format!("could not write {}: {e}", out_path.display()));
    }
}

pub(crate) fn try_run_embedded() -> Option<i32> {
    let exe_path = std::env::current_exe().ok()?;

    // Try named section first (macOS Mach-O / Windows PE via libsui),
    // fall back to trailer scan (Linux ELF raw append).
    let archive_data = if let Ok(Some(data)) = libsui::find_section("semaexec") {
        data.to_vec()
    } else if archive::has_embedded_archive(&exe_path).ok()? {
        match std::fs::read(&exe_path) {
            Ok(data) => {
                let len = data.len();
                let trailer = &data[len - 16..];
                let archive_size = u64::from_le_bytes(trailer[0..8].try_into().unwrap()) as usize;
                data[len - 16 - archive_size..len - 16].to_vec()
            }
            Err(_) => return None,
        }
    } else {
        return None;
    };

    let arch = match archive::deserialize_archive_from_bytes(&archive_data) {
        Ok(a) => a,
        Err(e) => {
            print_cli_error(format!("could not load embedded archive: {e}"));
            return Some(1);
        }
    };

    let entry_point = arch
        .metadata
        .get("entry-point")
        .and_then(|v| std::str::from_utf8(v).ok())
        .unwrap_or("__main__.semac")
        .to_string();

    let bytecode = match arch.files.get(&entry_point) {
        Some(b) => b.clone(),
        None => {
            print_cli_error(format!(
                "entry point '{entry_point}' was not found in the embedded archive"
            ));
            return Some(1);
        }
    };

    sema_core::vfs::init_vfs(arch.files);

    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = build_interpreter(&sandbox);

    let _ = interpreter.eval_str("(llm/auto-configure)");

    let args: Vec<String> = std::env::args().collect();
    let is_mcp = args
        .iter()
        .any(|arg| arg == "--mcp" || arg.starts_with("--mcp="));

    if is_mcp {
        let mut include = None;
        let mut exclude = None;
        let mut timeout_ms = None;
        for window in args.windows(2) {
            if window[0] == "--include" {
                include = Some(window[1].clone());
            } else if window[0] == "--exclude" {
                exclude = Some(window[1].clone());
            } else if window[0] == "--timeout-ms" {
                timeout_ms = window[1].trim().parse::<u64>().ok();
            }
        }
        for arg in &args {
            if let Some(rest) = arg.strip_prefix("--include=") {
                include = Some(rest.to_string());
            } else if let Some(rest) = arg.strip_prefix("--exclude=") {
                exclude = Some(rest.to_string());
            } else if let Some(rest) = arg.strip_prefix("--timeout-ms=") {
                timeout_ms = rest.trim().parse::<u64>().ok();
            }
        }

        let inc_tools = include.map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .collect::<Vec<String>>()
        });
        let exc_tools = exclude.map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .collect::<Vec<String>>()
        });
        let tool_timeout = resolve_mcp_tool_timeout(timeout_ms);

        if let Err(e) = interpreter.run_bytecode_bytes(&bytecode) {
            print_error(&e);
            return Some(1);
        }

        // Same no-ambient-runtime rule as the CLI mcp arm (llm/* + io_block_on).
        if let Err(e) =
            sema_mcp::run_mcp_server_sync(interpreter, inc_tools, exc_tools, tool_timeout)
        {
            die(format!("MCP server failed: {e}"));
        }
        Some(0)
    } else {
        match interpreter.run_bytecode_bytes(&bytecode) {
            Ok(_) => Some(0),
            Err(e) => {
                print_error(&e);
                Some(1)
            }
        }
    }
}

/// Expand a leading `~` to the user's home directory. Shells do not tilde-expand
/// inside `--output=~/x` (the `~` follows `=`), so without this the path would be
/// created literally as a directory named `~` in the cwd — a deletion hazard.
fn expand_tilde(path: &str) -> std::path::PathBuf {
    let home = || std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"));
    if path == "~" {
        if let Ok(h) = home() {
            return std::path::PathBuf::from(h);
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(h) = home() {
            return std::path::Path::new(&h).join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

/// The default output filename for a build: source stem, plus `.exe` for Windows
/// targets (or a host build on Windows).
fn default_output_name(source: &std::path::Path, target: Option<&str>) -> String {
    let stem = source
        .file_stem()
        .unwrap_or(source.as_os_str())
        .to_string_lossy();
    let needs_exe = target
        .and_then(|t| cross_compile::resolve_target(t).ok())
        .is_some_and(cross_compile::is_windows_target)
        || (target.is_none() && cfg!(windows));
    if needs_exe {
        format!("{stem}.exe")
    } else {
        stem.into_owned()
    }
}

/// Resolve `--output` to the actual file path a single-target build writes.
/// A path that IS a directory (or ends in a separator) means "default filename
/// inside this directory"; anything else is the file path itself.
fn resolve_output_path(
    output: Option<&str>,
    source: &std::path::Path,
    target: Option<&str>,
) -> std::path::PathBuf {
    match output {
        None => std::path::PathBuf::from(default_output_name(source, target)),
        Some(o) => {
            let p = expand_tilde(o);
            if p.is_dir() || o.ends_with('/') || o.ends_with(std::path::MAIN_SEPARATOR) {
                p.join(default_output_name(source, target))
            } else {
                p
            }
        }
    }
}

/// Plan the per-target output paths for `--target all`, honoring `--output`:
/// a directory output gets `<dir>/<stem>-<target>[.exe]`; a file-ish output is
/// used as the base name, `<base>-<target>[.exe]` (a trailing `.exe` on the base
/// is dropped first); no output means `<stem>-<target>[.exe]` in the cwd.
fn plan_all_target_outputs(
    output: Option<&str>,
    source: &std::path::Path,
) -> Vec<(&'static str, std::path::PathBuf)> {
    let stem = source
        .file_stem()
        .unwrap_or(source.as_os_str())
        .to_string_lossy()
        .into_owned();
    let (dir, base): (std::path::PathBuf, String) = match output {
        None => (std::path::PathBuf::new(), stem),
        Some(o) => {
            let p = expand_tilde(o);
            if p.is_dir() || o.ends_with('/') || o.ends_with(std::path::MAIN_SEPARATOR) {
                (p, stem)
            } else {
                let base = p
                    .file_name()
                    .map(|f| {
                        let f = f.to_string_lossy();
                        f.strip_suffix(".exe").unwrap_or(&f).to_string()
                    })
                    .unwrap_or(stem);
                (
                    p.parent().map(|d| d.to_path_buf()).unwrap_or_default(),
                    base,
                )
            }
        }
    };
    cross_compile::SUPPORTED_TARGETS
        .iter()
        .map(|&t| {
            let ext = if cross_compile::is_windows_target(t) {
                ".exe"
            } else {
                ""
            };
            (t, dir.join(format!("{base}-{t}{ext}")))
        })
        .collect()
}

/// Output-mode flags for `sema build`, threaded through the pipeline.
#[derive(Clone, Copy, Default)]
pub(crate) struct BuildOutputOpts {
    pub(crate) verbose: bool,
    pub(crate) json: bool,
}

/// The target-independent build product — one archive, embedded into every
/// per-target executable.
struct BuildArchive {
    files_count: usize,
    archive_bytes: Vec<u8>,
}

/// One successfully written artifact (native executable or web .vfs archive).
struct BuiltArtifact {
    /// Resolved target triple, or "web".
    target: String,
    /// Absolute output path.
    path: std::path::PathBuf,
    bytes: u64,
    /// How the runtime was obtained: "host" | "cached" | "downloaded" | "custom",
    /// or None for web (no runtime is embedded).
    runtime: Option<&'static str>,
    duration: std::time::Duration,
}

/// `29289346` → `27.9 MB` (base-1024, one decimal above bytes).
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn human_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        format!("{}m {:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

/// Split a target triple into user-facing (os, arch) — `aarch64-apple-darwin`
/// → ("macos", "arm64"). Unknown triples fall back to the raw string.
fn triple_os_arch(target: &str) -> (&'static str, String) {
    if target == "web" {
        return ("web", "-".to_string());
    }
    let arch = match target.split('-').next().unwrap_or(target) {
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    };
    let os = if target.contains("apple-darwin") {
        "macos"
    } else if target.contains("linux") {
        "linux"
    } else if target.contains("windows") {
        "windows"
    } else {
        "?"
    };
    (os, arch)
}

fn sha256_hex_of_file(path: &std::path::Path) -> Option<String> {
    use sha2::Digest;
    let data = std::fs::read(path).ok()?;
    let hash = sha2::Sha256::digest(&data);
    Some(hash.iter().map(|b| format!("{b:02x}")).collect())
}

/// Refuse to clobber the source file itself (`-o hello.sema` would otherwise
/// silently replace the program with its own binary).
fn check_output_not_source(
    source: &std::path::Path,
    output_path: &std::path::Path,
) -> Result<(), String> {
    let a = std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    let b = std::fs::canonicalize(output_path).unwrap_or_else(|_| output_path.to_path_buf());
    if a == b {
        return Err(format!(
            "Output path would overwrite the source file '{}'.\n  Hint: use `-o <output>` to specify a different output path, or rename your source file to use a .sema extension.",
            source.display()
        ));
    }
    Ok(())
}

/// Steps 1–4: compile → trace imports → collect assets → serialize archive.
/// Fully target-independent; `--target all` runs this ONCE. Prints the per-step
/// detail only in verbose mode, plus a one-line completion note always (stderr).
fn build_archive(
    file: &str,
    includes: &[String],
    opts: BuildOutputOpts,
) -> Result<BuildArchive, String> {
    let path = std::path::Path::new(file);
    let source = read_source_file(path)?;

    if opts.verbose {
        eprintln!("[1/4] Compiling {file}...");
    }

    let source_hash = crc32fast::hash(source.as_bytes());
    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = build_interpreter(&sandbox);

    let result = interpreter
        .compile_to_bytecode(&source)
        .map_err(|e| format!("compile failed: {}", e.format_plain()))?;
    let bytecode = sema_vm::serialize_to_bytes(&result, source_hash)
        .map_err(|e| format!("serialization failed: {}", e.format_plain()))?;

    if opts.verbose {
        eprintln!("[2/4] Tracing imports...");
    }
    let imports =
        import_tracer::trace_imports(path).map_err(|e| format!("tracing imports: {e}"))?;

    if opts.verbose {
        eprintln!("[3/4] Collecting assets...");
    }
    let mut files = std::collections::HashMap::new();
    files.insert("__main__.semac".to_string(), bytecode);

    for (rel_path, contents) in &imports {
        if let Err(e) = sema_core::vfs::validate_vfs_path(rel_path) {
            print_cli_warning(format!("skipping import with invalid VFS path: {e}"));
            continue;
        }
        files.insert(rel_path.clone(), contents.clone());
    }

    for include in includes {
        let inc_path = std::path::Path::new(include);
        if inc_path.is_dir() {
            let base = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            collect_directory_files(inc_path, &base, &mut files);
        } else if inc_path.is_file() {
            let rel = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            if let Err(e) = sema_core::vfs::validate_vfs_path(&rel) {
                print_cli_warning(format!("skipping {include}: {e}"));
                continue;
            }
            match std::fs::read(inc_path) {
                Ok(data) => {
                    files.insert(rel, data);
                }
                Err(e) => {
                    print_cli_warning(format!("cannot read {include}: {e}"));
                }
            }
        } else {
            print_cli_warning(format!("--include path not found: {include}"));
        }
    }

    if opts.verbose {
        eprintln!(
            "[4/4] Building archive ({} file{})...",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
    }
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "sema-version".to_string(),
        env!("CARGO_PKG_VERSION").as_bytes().to_vec(),
    );
    metadata.insert(
        "build-timestamp".to_string(),
        build_timestamp().into_bytes(),
    );
    metadata.insert("entry-point".to_string(), b"__main__.semac".to_vec());

    let canonical_root = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    metadata.insert(
        "build-root".to_string(),
        canonical_root.to_string_lossy().into_owned().into_bytes(),
    );

    let archive_bytes = archive::serialize_archive(&metadata, &files);
    eprintln!(
        "Compiled {file} → archive ({} file{}, {})",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        human_size(archive_bytes.len() as u64)
    );

    Ok(BuildArchive {
        files_count: files.len(),
        archive_bytes,
    })
}

/// Step 5: resolve the runtime for one target and write the executable.
fn write_target_executable(
    source: &std::path::Path,
    archive: &BuildArchive,
    output_path: &std::path::Path,
    runtime: Option<&str>,
    target: Option<&str>,
    no_cache: bool,
    opts: BuildOutputOpts,
) -> Result<BuiltArtifact, String> {
    let started = std::time::Instant::now();
    check_output_not_source(source, output_path)?;
    probe_output_writable(output_path)?;

    let resolved_target = target.and_then(|t| cross_compile::resolve_target(t).ok());

    let (runtime_path, runtime_source): (std::path::PathBuf, &'static str) = if let Some(r) =
        runtime
    {
        // Validate runtime binary format against target if both are specified
        if let Some(resolved) = resolved_target {
            let runtime_bytes =
                std::fs::read(r).map_err(|e| format!("cannot read --runtime file '{}': {e}", r))?;
            let detected = cross_compile::detect_binary_format(&runtime_bytes);
            let expected = cross_compile::expected_format(resolved);
            if let Some(det) = detected {
                if det != expected {
                    return Err(format!(
                        "Runtime binary format mismatch: {resolved} expects {expected} but --runtime file is {det}\n  Hint: provide a {expected} binary built for {resolved}, or omit --runtime to download automatically."
                    ));
                }
            }
        }
        (std::path::PathBuf::from(r), "custom")
    } else if let Some(t) = target {
        let resolved = cross_compile::resolve_target(t).map_err(|e| e.to_string())?;
        if cross_compile::is_host_target(resolved) {
            if opts.verbose {
                eprintln!("  Target {resolved} matches host — using local runtime (no download)");
            }
            let exe = std::env::current_exe()
                .map_err(|e| format!("cannot determine current executable path: {e}"))?;
            (exe, "host")
        } else {
            let (path, fetch) = cross_compile::ensure_runtime(resolved, no_cache, opts.verbose)
                .map_err(|e| e.to_string())?;
            let label = match fetch {
                cross_compile::RuntimeFetch::Cached => "cached",
                cross_compile::RuntimeFetch::Downloaded => "downloaded",
            };
            (path, label)
        }
    } else {
        let exe = std::env::current_exe()
            .map_err(|e| format!("cannot determine current executable path: {e}"))?;
        (exe, "host")
    };

    write_executable_platform(&runtime_path, output_path, &archive.archive_bytes)
        .map_err(|e| format!("writing executable: {e}"))?;

    let bytes = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);
    let abs = std::path::absolute(output_path).unwrap_or_else(|_| output_path.to_path_buf());
    Ok(BuiltArtifact {
        target: resolved_target
            .unwrap_or_else(cross_compile::host_target)
            .to_string(),
        path: abs,
        bytes,
        runtime: Some(runtime_source),
        duration: started.elapsed(),
    })
}

/// Print the machine-readable manifest for any build (single, all, or web).
fn print_build_json(
    source: &str,
    bundled_files: usize,
    archive_bytes: usize,
    artifacts: &[BuiltArtifact],
    failures: &[(String, String)],
    elapsed: std::time::Duration,
) {
    let mut targets: Vec<serde_json::Value> = Vec::new();
    for a in artifacts {
        let (os, arch) = triple_os_arch(&a.target);
        targets.push(serde_json::json!({
            "target": a.target,
            "os": os,
            "arch": arch,
            "path": a.path,
            "bytes": a.bytes,
            "sha256": sha256_hex_of_file(&a.path),
            "runtime": a.runtime,
            "ok": true,
            "error": null,
        }));
    }
    for (t, e) in failures {
        let (os, arch) = triple_os_arch(t);
        targets.push(serde_json::json!({
            "target": t,
            "os": os,
            "arch": arch,
            "path": null,
            "bytes": null,
            "sha256": null,
            "runtime": null,
            "ok": false,
            "error": e,
        }));
    }
    let manifest = serde_json::json!({
        "source": std::path::absolute(source).unwrap_or_else(|_| source.into()),
        "bundled_files": bundled_files,
        "archive_bytes": archive_bytes,
        "duration_ms": elapsed.as_millis() as u64,
        "targets": targets,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".to_string())
    );
}

/// Render the aligned human summary table for `--target all`.
fn render_build_summary(
    artifacts: &[BuiltArtifact],
    failures: &[(String, String)],
    elapsed: std::time::Duration,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let total = artifacts.len() + failures.len();
    let _ = writeln!(
        out,
        "Built {}/{} target{} in {}:",
        artifacts.len(),
        total,
        if total == 1 { "" } else { "s" },
        human_duration(elapsed)
    );
    let _ = writeln!(out);
    // rows: (os, arch, size-or-FAILED, path-or-error)
    let mut rows: Vec<(String, String, String, String)> = Vec::new();
    for a in artifacts {
        let (os, arch) = triple_os_arch(&a.target);
        rows.push((
            os.to_string(),
            arch,
            human_size(a.bytes),
            a.path.display().to_string(),
        ));
    }
    for (t, e) in failures {
        let (os, arch) = triple_os_arch(t);
        let first = e.lines().next().unwrap_or("build failed").to_string();
        rows.push((os.to_string(), arch, "FAILED".to_string(), first));
    }
    let w_os = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let w_arch = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
    let w_size = rows.iter().map(|r| r.2.len()).max().unwrap_or(0);
    for (os, arch, size, path) in rows {
        let _ = writeln!(
            out,
            "  {os:<w_os$}  {arch:<w_arch$}  {size:>w_size$}   {path}"
        );
    }
    out
}

/// Build every supported native target from one shared archive.
fn run_build_all(
    file: &str,
    output: Option<&str>,
    includes: &[String],
    runtime: Option<&str>,
    no_cache: bool,
    opts: BuildOutputOpts,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    if runtime.is_some() {
        // clap's conflicts_with should prevent this; belt and braces.
        return Err("--runtime cannot be combined with --target all".to_string());
    }
    let source = std::path::Path::new(file);
    let archive = build_archive(file, includes, opts)?;
    let plans = plan_all_target_outputs(output, source);
    let name_w = plans.iter().map(|(t, _)| t.len()).max().unwrap_or(0);

    let mut artifacts: Vec<BuiltArtifact> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    for (t, target_output) in &plans {
        match write_target_executable(
            source,
            &archive,
            target_output,
            None,
            Some(t),
            no_cache,
            opts,
        ) {
            Ok(a) => {
                eprintln!(
                    "  {t:<name_w$}  ✓ {:>6}  ({} runtime)",
                    human_duration(a.duration),
                    a.runtime.unwrap_or("host"),
                );
                artifacts.push(a);
            }
            Err(e) => {
                let first = e.lines().next().unwrap_or("build failed");
                eprintln!("  {t:<name_w$}  ✗ {first}");
                failures.push((t.to_string(), e));
            }
        }
    }

    eprintln!();
    if opts.json {
        print_build_json(
            file,
            archive.files_count,
            archive.archive_bytes.len(),
            &artifacts,
            &failures,
            started.elapsed(),
        );
    } else {
        print!(
            "{}",
            render_build_summary(&artifacts, &failures, started.elapsed())
        );
    }

    if !failures.is_empty() {
        let failed: Vec<&str> = failures.iter().map(|(t, _)| t.as_str()).collect();
        return Err(format!(
            "failed to build for {} target(s): {}\n  Hint: re-run a single target for details: `sema build --target <target> {}`\n  Hint: use `--runtime /path/to/sema` if downloads fail, or install a released version of sema.",
            failed.len(),
            failed.join(", "),
            file
        ));
    }
    Ok(())
}

pub(crate) fn run_build(
    file: &str,
    output: Option<&str>,
    includes: &[String],
    runtime: Option<&str>,
    target: Option<&str>,
    no_cache: bool,
    opts: BuildOutputOpts,
) -> Result<(), String> {
    if target == Some("all") {
        return run_build_all(file, output, includes, runtime, no_cache, opts);
    }
    if target == Some("web") {
        return run_build_web(file, output, includes, opts);
    }

    let started = std::time::Instant::now();
    let path = std::path::Path::new(file);

    // Pre-flight before any compilation: resolve the output path, refuse to
    // clobber the source, and probe the parent for writability (creating missing
    // directories). Avoids "failed at the last step" after a full compile.
    let output_path = resolve_output_path(output, path, target);
    check_output_not_source(path, &output_path)?;
    probe_output_writable(&output_path)?;

    let archive = build_archive(file, includes, opts)?;
    let artifact = write_target_executable(
        path,
        &archive,
        &output_path,
        runtime,
        target,
        no_cache,
        opts,
    )?;

    if opts.json {
        print_build_json(
            file,
            archive.files_count,
            archive.archive_bytes.len(),
            std::slice::from_ref(&artifact),
            &[],
            started.elapsed(),
        );
    } else {
        println!(
            "Built {} ({}, {} bundled file{}) for {} in {}",
            artifact.path.display(),
            human_size(artifact.bytes),
            archive.files_count,
            if archive.files_count == 1 { "" } else { "s" },
            artifact.target,
            human_duration(started.elapsed())
        );
    }
    Ok(())
}

fn compile_source_to_bytecode(source: &str) -> Result<Vec<u8>, String> {
    let source_hash = crc32fast::hash(source.as_bytes());
    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = Interpreter::new_with_sandbox(&sandbox);
    interpreter
        .eval_str_in_global(include_str!("web_prelude.sema"))
        .map_err(|e| format!("web prelude failed: {}", e.format_plain()))?;
    let result = interpreter
        .compile_to_bytecode(source)
        .map_err(|e| format!("compile failed: {}", e.format_plain()))?;
    sema_vm::serialize_to_bytes(&result, source_hash)
        .map_err(|e| format!("serialization failed: {}", e.format_plain()))
}

fn should_compile_traced_import(rel_path: &str) -> bool {
    rel_path.ends_with(".sema") || sema_core::resolve::is_package_import(rel_path)
}

fn web_output_path(input: &std::path::Path, output: Option<&str>) -> std::path::PathBuf {
    let default_name = format!(
        "{}.vfs",
        input
            .file_stem()
            .unwrap_or(input.as_os_str())
            .to_string_lossy()
    );

    match output {
        Some(raw) => {
            let path = expand_tilde(raw);
            if path.is_dir() || raw.ends_with(std::path::MAIN_SEPARATOR) {
                path.join(default_name)
            } else if path.extension().is_none() {
                path.with_extension("vfs")
            } else {
                path
            }
        }
        None => std::path::PathBuf::from(default_name),
    }
}

/// Compile an entry `.sema` plus its traced imports (and any `includes`) into a
/// web `.vfs` archive. Returns the archive bytes and the number of traced
/// imports (0 = single-file). Shared by `sema build --target web` and the
/// `sema web` dev server, which builds an archive on the fly for multi-file apps.
pub(crate) fn build_web_archive(
    path: &std::path::Path,
    includes: &[String],
    opts: BuildOutputOpts,
) -> Result<(Vec<u8>, usize), String> {
    let source =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    if opts.verbose {
        eprintln!("[1/4] Compiling {} (web prelude)...", path.display());
    }
    let entry_bytecode = compile_source_to_bytecode(&source)?;

    if opts.verbose {
        eprintln!("[2/4] Tracing imports...");
    }
    let imports =
        import_tracer::trace_imports(path).map_err(|e| format!("tracing imports: {e}"))?;

    let mut files = std::collections::HashMap::new();
    files.insert("__main__.semac".to_string(), entry_bytecode);

    for (rel_path, contents) in &imports {
        if let Err(e) = sema_core::vfs::validate_vfs_path(rel_path) {
            print_cli_warning(format!("skipping import with invalid VFS path: {e}"));
            continue;
        }

        let bundled = if should_compile_traced_import(rel_path) {
            let import_source = String::from_utf8(contents.clone()).map_err(|e| {
                format!("compile error in {rel_path}: import is not valid UTF-8: {e}")
            })?;
            compile_source_to_bytecode(&import_source).map_err(|e| format!("{e} in {rel_path}"))?
        } else {
            contents.clone()
        };

        files.insert(rel_path.clone(), bundled);
    }

    if opts.verbose {
        eprintln!("[3/4] Collecting assets...");
    }
    for include in includes {
        let inc_path = std::path::Path::new(include);
        if inc_path.is_dir() {
            let base = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            collect_directory_files(inc_path, &base, &mut files);
        } else if inc_path.is_file() {
            let rel = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            if let Err(e) = sema_core::vfs::validate_vfs_path(&rel) {
                print_cli_warning(format!("skipping {include}: {e}"));
                continue;
            }
            match std::fs::read(inc_path) {
                Ok(data) => {
                    files.insert(rel, data);
                }
                Err(e) => {
                    print_cli_warning(format!("cannot read {include}: {e}"));
                }
            }
        } else {
            print_cli_warning(format!("--include path not found: {include}"));
        }
    }

    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "sema-version".to_string(),
        env!("CARGO_PKG_VERSION").as_bytes().to_vec(),
    );
    metadata.insert(
        "build-timestamp".to_string(),
        build_timestamp().into_bytes(),
    );
    metadata.insert("entry-point".to_string(), b"__main__.semac".to_vec());
    metadata.insert("build-target".to_string(), b"web".to_vec());

    let canonical_root = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    metadata.insert(
        "build-root".to_string(),
        canonical_root.to_string_lossy().into_owned().into_bytes(),
    );

    if opts.verbose {
        eprintln!(
            "[4/4] Building archive ({} file{})...",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
    }
    let files_count = files.len();
    Ok((archive::serialize_archive(&metadata, &files), files_count))
}

fn run_build_web(
    file: &str,
    output: Option<&str>,
    includes: &[String],
    opts: BuildOutputOpts,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let path = std::path::Path::new(file);
    if !path.exists() {
        return Err(format!("source file not found: {file}"));
    }

    let (archive_bytes, files_count) = build_web_archive(path, includes, opts)?;
    eprintln!(
        "Compiled {file} → web archive ({} file{}, {})",
        files_count,
        if files_count == 1 { "" } else { "s" },
        human_size(archive_bytes.len() as u64)
    );

    let output_path = web_output_path(path, output);
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating output directory {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(&output_path, &archive_bytes)
        .map_err(|e| format!("writing {}: {e}", output_path.display()))?;

    let abs = std::path::absolute(&output_path).unwrap_or_else(|_| output_path.clone());
    if opts.json {
        let artifact = BuiltArtifact {
            target: "web".to_string(),
            path: abs,
            bytes: archive_bytes.len() as u64,
            runtime: None,
            duration: started.elapsed(),
        };
        print_build_json(
            file,
            files_count,
            archive_bytes.len(),
            std::slice::from_ref(&artifact),
            &[],
            started.elapsed(),
        );
    } else {
        println!(
            "Built {} ({}, {} bundled file{}) for web in {}",
            abs.display(),
            human_size(archive_bytes.len() as u64),
            files_count,
            if files_count == 1 { "" } else { "s" },
            human_duration(started.elapsed())
        );
    }

    Ok(())
}

/// Probe whether we can write to the directory that will hold `output_path`.
///
/// Creates and immediately deletes a tiny probe file in the parent directory.
/// Returns a clear error before the build commits to any work if the directory
/// doesn't exist or denies writes (e.g. /readonly/sema, /no/such/dir/sema).
fn probe_output_writable(output_path: &Path) -> Result<(), String> {
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    if !parent.exists() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create output directory {}: {e}", parent.display()))?;
    }
    let probe_name = format!(
        ".sema-build-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    let probe = parent.join(probe_name);
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => match e.kind() {
            std::io::ErrorKind::PermissionDenied => Err(format!(
                "permission denied writing to {} (for output {})",
                parent.display(),
                output_path.display()
            )),
            std::io::ErrorKind::NotFound => Err(format!(
                "output directory does not exist: {}",
                parent.display()
            )),
            _ => Err(format!(
                "cannot write to {}: {}",
                parent.display(),
                strip_os_error(&e.to_string())
            )),
        },
    }
}

/// Strip trailing " (os error N)" from a system error string for nicer output.
fn strip_os_error(s: &str) -> String {
    if let Some(idx) = s.rfind(" (os error ") {
        if s.ends_with(')') {
            return s[..idx].to_string();
        }
    }
    s.to_string()
}

/// Add a `VERSIONINFO` resource to a `sema build` Windows executable so Explorer's
/// Details tab shows the program name and the Sema runtime version. The resource
/// directory already contains the payload + icons written by libsui; editpe rebuilds
/// it in place, preserving them.
fn set_windows_version_info(
    pe_bytes: Vec<u8>,
    output_path: &std::path::Path,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use editpe::types::{VersionU16, VersionU32};

    let program = output_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "program".to_string());
    let sema_version = env!("CARGO_PKG_VERSION");
    let (maj, min, pat) = {
        let mut it = sema_version
            .split('.')
            .map(|p| p.parse::<u16>().unwrap_or(0));
        (
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
        )
    };
    // VS_FIXEDFILEINFO packs each version as two dwords: MS = major<<16|minor,
    // LS = patch<<16|build.
    let version = VersionU32 {
        major: ((maj as u32) << 16) | min as u32,
        minor: (pat as u32) << 16,
    };

    let mut info = editpe::VersionInfo::default();
    info.info.file_version = version;
    info.info.product_version = version;
    info.info.file_os = 0x0004_0004; // VOS_NT_WINDOWS32
    info.info.file_type = 0x1; // VFT_APP

    // en-US (0x0409), Unicode codepage (0x04B0) — key format Windows expects.
    let mut table = editpe::VersionStringTable {
        key: "040904b0".to_string(),
        strings: Default::default(),
    };
    for (k, v) in [
        ("ProductName", program.clone()),
        ("FileDescription", format!("{program} (built with Sema)")),
        ("FileVersion", sema_version.to_string()),
        ("ProductVersion", sema_version.to_string()),
        ("OriginalFilename", format!("{program}.exe")),
    ] {
        table.strings.insert(k.to_string(), v);
    }
    info.strings.push(table);
    info.vars.push(VersionU16 {
        major: 0x0409,
        minor: 0x04B0,
    });

    let mut image = editpe::Image::parse(&pe_bytes[..])?;
    let mut dir = image.resource_directory().cloned().unwrap_or_default();
    dir.set_version_info(&info)?;
    image.set_resource_directory(dir)?;
    Ok(image.data().to_vec())
}

/// Write the executable using format-aware injection.
///
/// Detects the binary format at runtime (not compile-time) so that
/// cross-compilation works: e.g. injecting into an ELF binary from macOS.
/// Note: libsui uses pure Rust for Mach-O ad-hoc signing (sha2 + object crate),
/// so cross-injecting Mach-O from Linux works without macOS tools.
fn write_executable_platform(
    runtime_path: &std::path::Path,
    output_path: &std::path::Path,
    archive_bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = std::fs::read(runtime_path)?;

    let format = cross_compile::detect_binary_format(&runtime).ok_or_else(|| {
        if runtime.len() < 4 {
            "runtime binary too small to detect format".to_string()
        } else if runtime[..4].iter().all(|&b| b.is_ascii_graphic() || b.is_ascii_whitespace())
            || (runtime.len() >= 3 && runtime[..3] == [0xEF, 0xBB, 0xBF])
        {
            format!(
                "unrecognized binary format (magic: {:02X} {:02X} {:02X} {:02X})\n  \
                 This looks like a source file, not a compiled binary.",
                runtime[0], runtime[1], runtime[2], runtime[3]
            )
        } else if (runtime[0] == 0x50 && runtime[1] == 0x4B)  // ZIP (PK)
            || (runtime[0] == 0x1F && runtime[1] == 0x8B)      // gzip
            || (runtime[..4] == [0xFD, 0x37, 0x7A, 0x58])      // xz
        {
            format!(
                "unrecognized binary format (magic: {:02X} {:02X} {:02X} {:02X})\n  \
                 This looks like an archive. Extract it first, or omit --runtime to let sema download automatically.",
                runtime[0], runtime[1], runtime[2], runtime[3]
            )
        } else {
            format!(
                "unrecognized binary format (magic: {:02X} {:02X} {:02X} {:02X})\n  \
                 The --runtime file doesn't appear to be a valid sema executable.",
                runtime[0], runtime[1], runtime[2], runtime[3]
            )
        }
    })?;

    match format {
        cross_compile::BinaryFormat::MachO => {
            let mut out = std::fs::File::create(output_path)?;
            libsui::Macho::from(runtime)?
                .write_section("semaexec", archive_bytes.to_vec())?
                .build_and_sign(&mut out)?;
        }
        cross_compile::BinaryFormat::Pe => {
            // Ordering invariant: the LAST writer must be one that serializes
            // the resource directory SORTED (named-before-ID, ascending) —
            // the PE spec requirement behind FindResource's binary search.
            // libsui's own writer emits insertion order, which the Win32 API
            // cannot see (ERROR_RESOURCE_TYPE_NOT_FOUND) even though linear
            // parsers can — every `sema build` exe booted as the bare REPL on
            // Windows (docs/bugs/archive/2026-07-29-windows-product-bugs.md bug 1).
            // So: libsui embeds the payload + icon first, and the editpe
            // (>=0.2, sorting) version-info pass runs last, re-serializing
            // the whole tree — payload, icons, and VERSIONINFO all survive
            // and are API-visible.
            let mut branded = Vec::with_capacity(runtime.len() + archive_bytes.len());
            libsui::PortableExecutable::from(&runtime)?
                .write_resource("semaexec", archive_bytes.to_vec())?
                // The rounded mark carries its own dark tile, so it reads correctly on
                // both light and dark backgrounds — PE icons cannot adapt to theme.
                // In-crate copy, not the repo-root `assets/`: `cargo package` ships
                // only files under the package root, so embedding across the crate
                // boundary compiles here and breaks the published crate. Synced from
                // canonical by `scripts/gen-icon-assets.py` (`jake icons-assets`).
                .set_icon(include_bytes!("../assets/sema-mark-rounded-512.png"))?
                .build(&mut branded)?;
            let branded = set_windows_version_info(branded, output_path)?;
            std::fs::write(output_path, branded)?;
        }
        cross_compile::BinaryFormat::Elf => {
            archive::write_bundled_executable_from_bytes(&runtime, output_path, archive_bytes)?;
            return Ok(());
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(output_path, perms)?;
    }

    Ok(())
}

/// Recursively collect files from a directory into the VFS files map.
fn collect_directory_files(
    dir: &std::path::Path,
    base: &str,
    files: &mut std::collections::HashMap<String, Vec<u8>>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            print_cli_warning(format!("cannot read directory {}: {e}", dir.display()));
            return;
        }
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let vfs_path = if base.is_empty() {
            name.clone()
        } else {
            format!("{base}/{name}")
        };

        if entry_path.is_dir() {
            collect_directory_files(&entry_path, &vfs_path, files);
        } else if entry_path.is_file() {
            if let Err(e) = sema_core::vfs::validate_vfs_path(&vfs_path) {
                print_cli_warning(format!("skipping {}: {e}", entry_path.display()));
                continue;
            }
            match std::fs::read(&entry_path) {
                Ok(data) => {
                    files.insert(vfs_path, data);
                }
                Err(e) => {
                    print_cli_warning(format!("cannot read {}: {e}", entry_path.display()));
                }
            }
        }
    }
}

/// Return current Unix timestamp as a string (seconds since epoch).
fn build_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

pub(crate) fn run_check(file: &str) {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            die(format!("could not read {file}: {e}"));
        }
    };

    if !sema_vm::is_bytecode_file(&bytes) {
        die(format!("{file} is not a valid .semac bytecode file"));
    }

    // Read header info before full deserialization
    let format_version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let major = u16::from_le_bytes([bytes[8], bytes[9]]);
    let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
    let patch = u16::from_le_bytes([bytes[12], bytes[13]]);

    match sema_vm::deserialize_from_bytes(&bytes) {
        Ok(result) => {
            let n_funcs = result.functions.len();
            println!(
                "✓ {file}: valid (format v{format_version}, sema {major}.{minor}.{patch}, {n_funcs} function{}, {} bytes)",
                if n_funcs == 1 { "" } else { "s" },
                bytes.len()
            );
        }
        Err(e) => {
            die(format!("{file} is invalid: {}", e.format_plain()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_summary_helpers() {
        assert_eq!(
            triple_os_arch("aarch64-apple-darwin"),
            ("macos", "arm64".into())
        );
        assert_eq!(
            triple_os_arch("x86_64-unknown-linux-gnu"),
            ("linux", "x86_64".into())
        );
        assert_eq!(
            triple_os_arch("x86_64-pc-windows-msvc"),
            ("windows", "x86_64".into())
        );
        assert_eq!(triple_os_arch("web"), ("web", "-".into()));

        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(12_700), "12.4 KB");
        assert_eq!(human_size(29_289_346), "27.9 MB");
        assert_eq!(human_size(2_147_483_648), "2.0 GB");

        assert_eq!(
            human_duration(std::time::Duration::from_millis(640)),
            "0.6s"
        );
        assert_eq!(human_duration(std::time::Duration::from_secs(92)), "1m 32s");
    }

    #[test]
    fn build_summary_table_aligns_and_reports_failures() {
        let artifacts = vec![
            BuiltArtifact {
                target: "aarch64-apple-darwin".into(),
                path: "/x/game-aarch64-apple-darwin".into(),
                bytes: 29_289_346,
                runtime: Some("host"),
                duration: std::time::Duration::from_secs(1),
            },
            BuiltArtifact {
                target: "x86_64-unknown-linux-gnu".into(),
                path: "/x/game-x86_64-unknown-linux-gnu".into(),
                bytes: 32_462_497,
                runtime: Some("downloaded"),
                duration: std::time::Duration::from_secs(3),
            },
        ];
        let failures = vec![(
            "x86_64-pc-windows-msvc".to_string(),
            "download failed: 404".to_string(),
        )];
        let table = render_build_summary(&artifacts, &failures, std::time::Duration::from_secs(14));
        assert!(table.starts_with("Built 2/3 targets in 14.0s:"), "{table}");
        // every data row aligns: the path column starts at the same byte offset
        let rows: Vec<&str> = table.lines().skip(2).collect();
        assert_eq!(rows.len(), 3);
        let col = rows[0].find("/x/").unwrap();
        assert_eq!(rows[1].find("/x/"), Some(col), "{table}");
        assert!(rows[2].contains("FAILED"));
        assert!(rows[2].contains("download failed: 404"));
        assert!(!table.contains("won't run"));
    }

    #[test]
    fn build_output_default_name_adds_exe_for_windows_targets() {
        let src = std::path::Path::new("examples/game-of-life.sema");
        // No --target builds for the host, so the default name carries the
        // HOST's exe suffix (".exe" on a Windows host, "" elsewhere).
        assert_eq!(
            default_output_name(src, None),
            format!("game-of-life{}", std::env::consts::EXE_SUFFIX)
        );
        assert_eq!(
            default_output_name(src, Some("windows")),
            "game-of-life.exe"
        );
        assert_eq!(
            default_output_name(src, Some("x86_64-pc-windows-msvc")),
            "game-of-life.exe"
        );
        assert_eq!(default_output_name(src, Some("linux")), "game-of-life");
    }

    #[test]
    fn build_output_into_existing_dir_uses_default_filename() {
        let dir = std::env::temp_dir().join(format!("sema-out-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = std::path::Path::new("hello.sema");
        let out = resolve_output_path(Some(dir.to_str().unwrap()), src, None);
        assert_eq!(
            out,
            dir.join(format!("hello{}", std::env::consts::EXE_SUFFIX))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_output_trailing_slash_means_directory_even_if_missing() {
        let src = std::path::Path::new("hello.sema");
        let out = resolve_output_path(Some("no/such/dir/"), src, None);
        assert_eq!(
            out,
            std::path::PathBuf::from(format!("no/such/dir/hello{}", std::env::consts::EXE_SUFFIX))
        );
    }

    #[test]
    fn build_output_plain_path_is_the_file_itself() {
        let src = std::path::Path::new("hello.sema");
        let out = resolve_output_path(Some("dist/game"), src, None);
        assert_eq!(out, std::path::Path::new("dist/game"));
    }

    #[test]
    fn build_output_expands_leading_tilde() {
        // `--output=~/x` reaches us unexpanded (the ~ follows `=`); it must never
        // be treated as a literal `./~` directory.
        if let Ok(home) = std::env::var("HOME") {
            let src = std::path::Path::new("hello.sema");
            let out = resolve_output_path(Some("~/sema-out/game"), src, None);
            assert_eq!(out, std::path::Path::new(&home).join("sema-out/game"));
        }
    }

    #[test]
    fn build_all_targets_suffixes_filenames_and_honors_output_file_base() {
        let src = std::path::Path::new("examples/game-of-life.sema");
        // file-ish output: base name is taken from the output path
        let plans = plan_all_target_outputs(Some("dist/game"), src);
        assert_eq!(plans.len(), cross_compile::SUPPORTED_TARGETS.len());
        for (t, path) in &plans {
            let expect_ext = if cross_compile::is_windows_target(t) {
                ".exe"
            } else {
                ""
            };
            assert_eq!(
                path,
                &std::path::PathBuf::from(format!("dist/game-{t}{expect_ext}")),
                "unexpected plan for {t}"
            );
        }
        // every planned path is distinct — this is the bug that motivated the plan
        let unique: std::collections::HashSet<_> = plans.iter().map(|(_, p)| p).collect();
        assert_eq!(unique.len(), plans.len());
    }

    #[test]
    fn build_all_targets_strips_exe_from_output_base() {
        let src = std::path::Path::new("hello.sema");
        let plans = plan_all_target_outputs(Some("dist/game.exe"), src);
        assert!(plans
            .iter()
            .all(|(_, p)| !p.to_string_lossy().contains(".exe-")));
        assert!(plans
            .iter()
            .any(|(t, p)| cross_compile::is_windows_target(t)
                && p.to_string_lossy().ends_with(".exe")));
    }

    #[test]
    fn build_all_targets_into_directory_uses_source_stem() {
        let dir = std::env::temp_dir().join(format!("sema-all-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = std::path::Path::new("examples/game-of-life.sema");
        let plans = plan_all_target_outputs(Some(dir.to_str().unwrap()), src);
        for (t, path) in &plans {
            assert!(path.starts_with(&dir));
            assert!(path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("game-of-life-{t}")));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_all_targets_without_output_uses_cwd_stem() {
        let src = std::path::Path::new("examples/game-of-life.sema");
        let plans = plan_all_target_outputs(None, src);
        assert_eq!(
            plans[0].1,
            std::path::PathBuf::from(format!("game-of-life-{}", plans[0].0))
        );
    }

    /// Pins the zsh-completion repair (`fix_zsh_root_completion`): position 1
    /// must dispatch subcommands (clap_complete emits the FILE/SCRIPT_ARGS
    /// positionals first, which swallows the subcommand word — `sema notebook
    /// <TAB>` completed files). If this fails after a clap_complete upgrade,
    /// refresh the anchors in `fix_zsh_root_completion`.
    use super::compile_source_to_bytecode;
    use sema_core::{intern, NativeFn, Sandbox, Value};
    use sema_eval::Interpreter;

    #[test]
    fn web_build_prelude_expands_defcomponent_into_callable_global() {
        let source = r##"
            (defcomponent counter-view ()
              [:div "ok"])
            (mount! "#app" counter-view)
        "##;

        let bytes = compile_source_to_bytecode(source).expect("compile should succeed");

        let interp = Interpreter::new_with_sandbox(&Sandbox::allow_all());
        interp.global_env.set(
            intern("component/mount!"),
            Value::native_fn(NativeFn::simple("component/mount!", |_args| {
                Ok(Value::nil())
            })),
        );

        interp
            .run_bytecode_bytes(&bytes)
            .expect("compiled program should execute");

        let counter_view = interp
            .global_env
            .get(intern("counter-view"))
            .expect("defcomponent should define counter-view");
        let rendered = sema_eval::call_value(&interp.ctx, &counter_view, &[])
            .expect("counter-view should be callable");

        assert!(!rendered.is_nil(), "component should return SIP markup");
    }

    #[test]
    fn web_build_prelude_expands_reactive_macros() {
        let source = r#"
            (def doubled (computed 42))
            (def batched (batch 1 2 3))
        "#;

        let bytes = compile_source_to_bytecode(source).expect("compile should succeed");

        let interp = Interpreter::new_with_sandbox(&Sandbox::allow_all());
        interp.global_env.set(
            intern("__state/computed-create"),
            Value::native_fn(NativeFn::simple("__state/computed-create", |_args| {
                Ok(Value::string("computed-ok"))
            })),
        );
        interp.global_env.set(
            intern("__state/batch-run"),
            Value::native_fn(NativeFn::simple("__state/batch-run", |_args| {
                Ok(Value::string("batch-ok"))
            })),
        );

        interp
            .run_bytecode_bytes(&bytes)
            .expect("compiled program should execute");

        let doubled = interp
            .global_env
            .get(intern("doubled"))
            .expect("computed should define doubled");
        let batched = interp
            .global_env
            .get(intern("batched"))
            .expect("batch should define batched");

        assert_eq!(doubled, Value::string("computed-ok"));
        assert_eq!(batched, Value::string("batch-ok"));
    }
}
