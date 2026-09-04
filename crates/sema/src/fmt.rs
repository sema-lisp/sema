use crate::output::human_output;
use crate::read_source_file;
use crate::{die, print_cli_error};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SemaConfig {
    #[serde(default)]
    pub(crate) fmt: FmtConfig,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FmtConfig {
    #[serde(default = "default_width")]
    pub(crate) width: usize,
    #[serde(default = "default_indent")]
    pub(crate) indent: usize,
    #[serde(default)]
    pub(crate) align: bool,
    #[serde(
        default = "default_max_blank_lines",
        alias = "max_blank_lines",
        rename = "max-blank-lines"
    )]
    pub(crate) max_blank_lines: usize,
    /// Glob patterns (or literal path prefixes) excluded from formatting.
    /// Explicitly named files bypass this list.
    #[serde(default)]
    pub(crate) ignore: Vec<String>,
}

impl Default for FmtConfig {
    fn default() -> Self {
        Self {
            width: 80,
            indent: 2,
            align: false,
            max_blank_lines: 1,
            ignore: Vec::new(),
        }
    }
}

fn default_width() -> usize {
    sema_fmt::FormatOptions::default().width
}
fn default_indent() -> usize {
    sema_fmt::FormatOptions::default().indent
}
fn default_max_blank_lines() -> usize {
    sema_fmt::FormatOptions::default().max_blank_lines
}

/// Walk up from cwd to find sema.toml
pub(crate) fn find_config() -> Option<SemaConfig> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("sema.toml");
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate).ok()?;
            return toml::from_str(&text).ok();
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub(crate) fn run_fmt(
    patterns: &[String],
    check: bool,
    show_diff: bool,
    opts: &sema_fmt::FormatOptions,
    ignore: &[String],
    json: bool,
) {
    // A path is ignored when it matches an `ignore` entry from sema.toml.
    // An entry with glob characters matches as a glob; anything else is a
    // literal path prefix (file or directory). Paths are matched relative to
    // the working directory, `./`-stripped.
    let is_ignored = |path: &str| -> bool {
        // Walked paths carry the host separator (`\` on Windows) while ignore
        // entries are written with `/`; compare in `/` form or literal-prefix
        // entries never match there (globs matched either way).
        let unified = path.replace('\\', "/");
        let normalized = unified.strip_prefix("./").unwrap_or(&unified);
        ignore.iter().any(|pat| {
            if pat.contains('*') || pat.contains('?') || pat.contains('[') {
                glob::Pattern::new(pat)
                    .map(|g| g.matches(normalized))
                    .unwrap_or(false)
            } else {
                let prefix = pat.trim_end_matches('/');
                normalized == prefix || normalized.starts_with(&format!("{prefix}/"))
            }
        })
    };
    // Wildcards don't cross a leading dot: the recursive walk stays out of
    // hidden directories (.git, .worktrees, ...) unless a pattern names one
    // literally.
    let match_opts = glob::MatchOptions {
        require_literal_leading_dot: true,
        ..Default::default()
    };
    // Handle stdin ("-")
    if patterns.len() == 1 && patterns[0] == "-" {
        let mut source = String::new();
        if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut source) {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "formatted": false,
                        "error": format!("Error reading stdin: {e}")
                    })
                );
            } else {
                print_cli_error(format!("could not read stdin: {e}"));
            }
            std::process::exit(1);
        }
        match sema_fmt::format_source(&source, opts) {
            Ok(formatted) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "formatted": true,
                            "source": formatted
                        })
                    );
                } else {
                    print!("{formatted}");
                }
            }
            Err(e) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "formatted": false,
                            "error": format!("{e}")
                        })
                    );
                } else {
                    print_cli_error(format!("could not format stdin: {e}"));
                }
                std::process::exit(1);
            }
        }
        return;
    }

    // Determine which files to format
    let files = if patterns.is_empty() {
        // Default: all .sema files in current directory recursively
        match glob::glob_with("**/*.sema", match_opts) {
            Ok(paths) => paths
                .filter_map(|p| p.ok())
                .map(|p| p.to_string_lossy().to_string())
                .filter(|p| !is_ignored(p))
                .collect::<Vec<_>>(),
            Err(e) => {
                die(format!("invalid glob pattern: {e}"));
            }
        }
    } else {
        // Expand each pattern
        let mut all_files = Vec::new();
        for pattern in patterns {
            // If it contains glob characters, expand it
            if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
                match glob::glob_with(pattern, match_opts) {
                    Ok(paths) => {
                        for path in paths.filter_map(|p| p.ok()) {
                            let path = path.to_string_lossy().to_string();
                            if !is_ignored(&path) {
                                all_files.push(path);
                            }
                        }
                    }
                    Err(e) => {
                        die(format!("invalid glob pattern '{pattern}': {e}"));
                    }
                }
            } else if std::path::Path::new(pattern).is_dir() {
                // A directory means every .sema file under it (`sema fmt .`).
                let dir_glob = format!("{}/**/*.sema", pattern.trim_end_matches(['/', '\\']));
                match glob::glob_with(&dir_glob, match_opts) {
                    Ok(paths) => {
                        for path in paths.filter_map(|p| p.ok()) {
                            let path = path.to_string_lossy().to_string();
                            if !is_ignored(&path) {
                                all_files.push(path);
                            }
                        }
                    }
                    Err(e) => {
                        die(format!("invalid glob pattern '{dir_glob}': {e}"));
                    }
                }
            } else {
                // An explicitly named file always formats, ignore list or not
                all_files.push(pattern.clone());
            }
        }
        all_files
    };

    if files.is_empty() {
        human_output!(json, "No .sema files found");
        return;
    }

    let mut checked = 0;
    let mut changed = 0;
    let mut errors = 0;

    for file in &files {
        let source = match read_source_file(file) {
            Ok(s) => s,
            Err(msg) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "file": file,
                            "formatted": false,
                            "error": msg,
                        })
                    );
                } else {
                    print_cli_error(msg);
                }
                errors += 1;
                continue;
            }
        };

        let formatted = match sema_fmt::format_source(&source, opts) {
            Ok(f) => f,
            Err(e) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "file": file,
                            "formatted": false,
                            "error": format!("Error formatting {file}: {e}")
                        })
                    );
                } else {
                    print_cli_error(format!("could not format {file}: {e}"));
                }
                errors += 1;
                continue;
            }
        };

        checked += 1;
        let file_changed = source != formatted;
        if file_changed {
            changed += 1;
        }

        if json {
            println!(
                "{}",
                serde_json::json!({
                    "file": file,
                    "formatted": true,
                    "changed": file_changed,
                    "source": formatted
                })
            );
            continue;
        }

        if file_changed {
            if check {
                println!("Would reformat: {file}");
            } else if show_diff {
                // Simple line-by-line diff
                print_simple_diff(file, &source, &formatted);
            } else {
                // Write formatted output back
                if let Err(e) = std::fs::write(file, &formatted) {
                    print_cli_error(format!("could not write {file}: {e}"));
                    errors += 1;
                    continue;
                }
                println!("Formatted: {file}");
            }
        }
    }

    // Print summary
    if !json {
        if check {
            if changed > 0 {
                println!("\n{changed} file(s) would be reformatted, {checked} file(s) checked");
                std::process::exit(1);
            } else {
                println!("{checked} file(s) already formatted");
            }
        } else if show_diff {
            println!("\n{changed} file(s) would change, {checked} file(s) checked");
        } else if changed > 0 {
            println!(
                "\n{changed} file(s) formatted, {} file(s) unchanged",
                checked - changed
            );
        } else {
            println!("{checked} file(s) already formatted");
        }
    }

    if errors > 0 {
        die(format!("{errors} file(s) could not be formatted"));
    }

    if check && changed > 0 {
        std::process::exit(1);
    }
}

fn print_simple_diff(filename: &str, old: &str, new: &str) {
    println!("--- {filename}");
    println!("+++ {filename}");
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Simple context diff: show lines that differ
    let max_lines = old_lines.len().max(new_lines.len());
    let mut in_diff = false;
    let mut diff_start = 0;

    for i in 0..max_lines {
        let old_line = old_lines.get(i).copied().unwrap_or("");
        let new_line = new_lines.get(i).copied().unwrap_or("");

        if old_line != new_line {
            if !in_diff {
                diff_start = i;
                in_diff = true;
                println!("@@ -{} +{} @@", i + 1, i + 1);
            }
            if i < old_lines.len() {
                println!("-{old_line}");
            }
            if i < new_lines.len() {
                println!("+{new_line}");
            }
        } else if in_diff && i - diff_start < 3 {
            println!(" {old_line}");
        } else {
            in_diff = false;
        }
    }
}
