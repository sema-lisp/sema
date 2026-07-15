//! CLI-level tests for `sema fmt` file discovery: the `[fmt] ignore` list in
//! sema.toml, the explicit-path bypass, and the hidden-directory skip.
//! Driven through the real binary so config discovery (walk up to sema.toml)
//! and glob expansion are exercised exactly as a user hits them.

use std::path::Path;
use std::process::Command;

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
    assert_eq!(read(&dir, "api.generated.sema"), UGLY, "wildcard entry ignored");
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
    assert_eq!(read(&dir, "vendor/lib.sema"), UGLY, "ignore filters glob expansion");
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
