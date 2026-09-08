//! CLI-level tests for `sema fmt` file discovery: the `[fmt] ignore` list in
//! sema.toml, the explicit-path bypass, and the hidden-directory skip.
//! Driven through the real binary so config discovery (walk up to sema.toml)
//! and glob expansion are exercised exactly as a user hits them.

use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;

const UGLY: &str = "(define   x   1)\n";
const PRETTY: &str = "(define x 1)\n";

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

fn run_fmt(dir: &Path, args: &[&str]) {
    let status = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("fmt")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("failed to spawn sema fmt");
    assert!(status.success(), "sema fmt exited non-zero: {status:?}");
}

fn tempdir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sema-fmt-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn fmt_ignore_list_skips_globs_and_prefixes() {
    let dir = tempdir("ignore");
    write(
        &dir,
        "sema.toml",
        "[fmt]\nignore = [\"vendor\", \"gen/**\", \"*.generated.sema\"]\n",
    );
    write(&dir, "main.sema", UGLY);
    write(&dir, "vendor/lib.sema", UGLY);
    write(&dir, "gen/deep/out.sema", UGLY);
    write(&dir, "api.generated.sema", UGLY);

    run_fmt(&dir, &[]);

    assert_eq!(read(&dir, "main.sema"), PRETTY, "normal file formats");
    assert_eq!(read(&dir, "vendor/lib.sema"), UGLY, "prefix entry ignored");
    assert_eq!(read(&dir, "gen/deep/out.sema"), UGLY, "glob entry ignored");
    assert_eq!(
        read(&dir, "api.generated.sema"),
        UGLY,
        "wildcard entry ignored"
    );
}

#[test]
fn fmt_explicit_path_bypasses_ignore_list() {
    let dir = tempdir("explicit");
    write(&dir, "sema.toml", "[fmt]\nignore = [\"vendor\"]\n");
    write(&dir, "vendor/lib.sema", UGLY);

    run_fmt(&dir, &["vendor/lib.sema"]);

    assert_eq!(
        read(&dir, "vendor/lib.sema"),
        PRETTY,
        "an explicitly named file formats even when the ignore list matches it"
    );
}

#[test]
fn fmt_ignore_applies_to_user_globs() {
    let dir = tempdir("userglob");
    write(&dir, "sema.toml", "[fmt]\nignore = [\"vendor\"]\n");
    write(&dir, "src/a.sema", UGLY);
    write(&dir, "vendor/lib.sema", UGLY);

    run_fmt(&dir, &["**/*.sema"]);

    assert_eq!(read(&dir, "src/a.sema"), PRETTY);
    assert_eq!(
        read(&dir, "vendor/lib.sema"),
        UGLY,
        "ignore filters glob expansion"
    );
}

#[test]
fn fmt_default_walk_skips_hidden_directories() {
    let dir = tempdir("hidden");
    write(&dir, "main.sema", UGLY);
    write(&dir, ".worktrees/wip/file.sema", UGLY);

    run_fmt(&dir, &[]);

    assert_eq!(read(&dir, "main.sema"), PRETTY);
    assert_eq!(
        read(&dir, ".worktrees/wip/file.sema"),
        UGLY,
        "the recursive walk must not enter hidden directories"
    );
}

#[test]
fn fmt_check_stdin_reports_unformatted_input() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["fmt", "--check", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(UGLY.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn fmt_check_stdin_json_reports_success_and_change() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["fmt", "--check", "--json", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(UGLY.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());

    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["formatted"], true);
    assert_eq!(result["changed"], true);
    assert_eq!(result["source"], PRETTY);
}

#[test]
fn fmt_rejects_malformed_config() {
    let dir = tempdir("bad-config");
    write(&dir, "sema.toml", "[fmt\nwidth = 80\n");
    write(&dir, "main.sema", UGLY);
    let status = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["fmt", "main.sema"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn fmt_rejects_unsafe_indent_from_config() {
    let dir = tempdir("unsafe-indent");
    write(&dir, "sema.toml", "[fmt]\nindent = 18446744073709551615\n");
    write(&dir, "main.sema", UGLY);
    let status = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["fmt", "main.sema"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn fmt_rejects_unsafe_indent_from_cli() {
    let dir = tempdir("unsafe-indent-cli");
    write(&dir, "main.sema", UGLY);
    let status = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["fmt", "--indent", "18446744073709551615", "main.sema"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn fmt_accepts_literal_paths_with_glob_characters() {
    let dir = tempdir("literal-glob");
    write(&dir, "name[1].sema", UGLY);
    run_fmt(&dir, &["name[1].sema"]);
    assert_eq!(read(&dir, "name[1].sema"), PRETTY);
}

#[test]
fn fmt_accepts_literal_directories_with_glob_characters() {
    let dir = tempdir("literal-glob-directory");
    write(&dir, "name[1]/nested/main.sema", UGLY);
    run_fmt(&dir, &["name[1]"]);
    assert_eq!(read(&dir, "name[1]/nested/main.sema"), PRETTY);
}

#[test]
fn fmt_cli_can_disable_configured_alignment() {
    let dir = tempdir("disable-align");
    write(&dir, "sema.toml", "[fmt]\nalign = true\n");
    write(&dir, "main.sema", "(define x 1)\n(define longer 2)\n");
    run_fmt(&dir, &["--align=false", "main.sema"]);
    assert_eq!(read(&dir, "main.sema"), "(define x 1)\n(define longer 2)\n");
}
