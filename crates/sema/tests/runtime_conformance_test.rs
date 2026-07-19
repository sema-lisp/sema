//! Architectural conformance gate for ADR #69 (one I/O pool behind one seam).
//!
//! Scans every `crates/*/src/**/*.rs` for tokio-runtime creation and for bypasses
//! of the sanctioned `sema_io::io_*` wrappers, and fails on any hit outside an
//! explicit allowlist. This is what makes "no ad-hoc runtimes" an invariant CI
//! enforces rather than a review convention: adding a runtime anywhere new — or
//! calling the raw sema-core seam directly from a consumer crate — fails this test
//! with the offending file:line, and the fix is to go through `sema-io` or to add
//! an allowlist entry WITH a written reason in the same commit.
//!
//! Matching is comment-stripped and path-free (`Runtime::new(`,
//! `new_multi_thread(`, …) so a `use tokio::runtime::Builder;` import doesn't
//! evade the scan. Test code is exempt: `crates/*/tests/**` isn't scanned, and
//! in-file `#[cfg(test)]` fixtures are carried as counted allowlist entries.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Forbidden tokens (post comment-stripping). Path-free so imports can't evade.
const RUNTIME_TOKENS: &[&str] = &[
    "Runtime::new(",
    "new_multi_thread(",
    "new_current_thread(",
    "#[tokio::main]",
];

/// Raw-seam bypasses: consumer crates must go through the sanctioned
/// `sema_io::io_*` install-then-delegate wrappers, never the raw sema-core seam.
const RAW_SEAM_TOKENS: &[&str] = &[
    "sema_core::io_spawn(",
    "sema_core::io_spawn_blocking(",
    "sema_core::io_block_on(",
];

/// (path suffix, max allowed hits, reason). Anything not listed allows 0.
const ALLOWLIST: &[(&str, usize, &str)] = &[
    (
        "sema-io/src/lib.rs",
        1,
        "the blessed backend: THE one pool builder lives here",
    ),
    (
        "sema-otel/src/imp.rs",
        1,
        "isolated OTLP export reactor — telemetry must not contend with or tear \
         down with user I/O (ADR #69 scope)",
    ),
    (
        "sema/src/main.rs",
        7,
        "subcommand entry-point drivers (lsp/dap/mcp/workflow/notebook) ARE main()",
    ),
    (
        "sema-mcp/src/client_auth.rs",
        1,
        "out of Slice A: same as sema-mcp builtins",
    ),
    (
        "sema-notebook/src/bridge.rs",
        1,
        "out of Slice A: blocking-recv shim; right fix is std::sync::mpsc, no seam",
    ),
    (
        "sema-stdlib/src/server.rs",
        2,
        "in-file #[cfg(test)] fixtures asserting inside-runtime behavior",
    ),
    (
        "sema-eval/src/eval.rs",
        1,
        "the interpreter's persistent cooperative unified Runtime \
         (sema_vm::runtime::Runtime), NOT a tokio runtime — this is the canonical \
         async engine the unified-runtime migration is built on",
    ),
];

/// Strip `//` line comments, tracking double-quote string state per line so a
/// `//` inside a string (e.g. `"https://…"`) survives. Deliberately does NOT
/// strip `/* */` block comments: a string literal containing `/*` (server.rs has
/// them) would otherwise swallow real code and silently UNDER-report — the one
/// failure direction this gate must never have. A commented-out forbidden token
/// inside a block comment therefore false-POSITIVES, which fails loudly and is
/// resolved by deleting the dead code. Line-scoped state also bounds any
/// char-literal (`'"'`) mis-parse to a single line.
fn strip_line_comments(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1, // skip escaped char inside a string
            b'"' => in_string = !in_string,
            b'/' if !in_string && i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                return &line[..i];
            }
            _ => {}
        }
        i += 1;
    }
    line
}

fn rs_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            // Skip build artifacts and fuzz corpora.
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" || name == "fuzz" {
                continue;
            }
            rs_files_under(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_adhoc_tokio_runtimes_outside_allowlist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let crates_dir = root.join("crates");
    assert!(crates_dir.is_dir(), "workspace crates/ dir not found");

    // Collect every crates/*/src tree.
    let mut files = Vec::new();
    for entry in fs::read_dir(&crates_dir).expect("read crates/").flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            rs_files_under(&src, &mut files);
        }
    }
    assert!(
        files.len() > 50,
        "scan looks broken: only {} source files found",
        files.len()
    );

    let mut violations: Vec<String> = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        // Test code is exempt (same as the `crates/*/tests/**` exemption): an
        // in-src `#[cfg(test)] mod tests` lives in `tests.rs` and legitimately
        // constructs the cooperative `Runtime` (and tokio fixtures) many times.
        if rel.ends_with("tests.rs") {
            continue;
        }
        let Ok(src) = fs::read_to_string(file) else {
            continue;
        };

        let in_sema_io = rel.contains("sema-io/src/");
        let mut hits = 0usize;
        let mut hit_lines: Vec<String> = Vec::new();
        for (lineno, raw_line) in src.lines().enumerate() {
            let line = strip_line_comments(raw_line);
            for tok in RUNTIME_TOKENS {
                if line.contains(tok) {
                    hits += 1;
                    hit_lines.push(format!("{}:{}: {}", rel, lineno + 1, tok));
                }
            }
            if !in_sema_io {
                for tok in RAW_SEAM_TOKENS {
                    if line.contains(tok) {
                        hits += 1;
                        hit_lines.push(format!("{}:{}: raw-seam bypass {}", rel, lineno + 1, tok));
                    }
                }
            }
        }
        if hits == 0 {
            continue;
        }
        let allowed = ALLOWLIST
            .iter()
            .find(|(suffix, _, _)| rel.ends_with(suffix))
            .map(|(_, max, _)| *max)
            .unwrap_or(0);
        if hits > allowed {
            violations.push(format!(
                "{rel}: {hits} runtime-creation/bypass hit(s), {allowed} allowed:\n    {}",
                hit_lines.join("\n    ")
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "ad-hoc tokio runtimes (or raw-seam bypasses) outside the ADR #69 allowlist \
         — route through sema-io, or add an allowlist entry WITH a reason:\n\n{}",
        violations.join("\n\n")
    );
}

/// Zero-tolerance removal gate for the legacy async-scheduler purge (P5).
///
/// The old change-detector (a committed `legacy-symbols.baseline` the scan was
/// diffed against) is retired: `scheduler.rs`, the cooperative-debug driver, and
/// the legacy `async_signal.rs` seams are DELETED, so their identifiers must have
/// ZERO hits in shipped, comment-stripped code. `--check` fails on any
/// reintroduction outside the script's exact-file allowlist (currently empty).
#[test]
fn unified_runtime_purged_legacy_symbols_absent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(root.join("scripts/check-unified-runtime-legacy.sh"))
        .arg("--check")
        .current_dir(&root)
        .output()
        .expect("run unified runtime legacy scanner");

    assert!(
        output.status.success(),
        "purged legacy-scheduler symbols reintroduced\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unified_runtime_scanner_detects_raw_blocking_recv_fixture() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture =
        root.join("crates/sema/tests/fixtures/unified_runtime_legacy/raw_blocking_recv.rs");
    let output = Command::new(root.join("scripts/check-unified-runtime-legacy.sh"))
        .args([
            "--scan-path",
            fixture.to_str().expect("fixture path is UTF-8"),
        ])
        .current_dir(&root)
        .output()
        .expect("scan raw blocking recv fixture");

    assert!(
        output.status.success(),
        "fixture scan failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("raw_blocking_recv.rs:4:    let _ = receiver.recv();"),
        "raw blocking recv was not reported; stdout:\n{stdout}"
    );
}

#[test]
fn unified_runtime_inventory_mapping_covers_exact_current_matches() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(root.join("scripts/check-unified-runtime-inventory.sh"))
        .arg("--check")
        .current_dir(&root)
        .output()
        .expect("run unified runtime inventory checker");

    assert!(
        output.status.success(),
        "inventory checker failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unified_runtime_inventory_checker_rejects_invalid_fixture_states() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture_dir = std::env::temp_dir().join(format!(
        "sema-runtime-inventory-checker-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create inventory checker fixture directory");
    let mapping = fixture_dir.join("mapping.tsv");
    let current = fixture_dir.join("current.txt");
    let inventory = fixture_dir.join("inventory.md");
    fs::write(&current, "crates/example/src/lib.rs:1:match\n").expect("write current fixture");
    fs::write(
        &inventory,
        "| Area | Path | Status |\n| --- | --- | --- |\n| R01A valid row | R99 appears outside the ID column | MIGRATED |\n",
    )
    .expect("write inventory fixture");

    let cases = [
        (
            "valid",
            Some("R01A\tcrates/example/src/lib.rs:1:match\n"),
            true,
        ),
        ("missing", None, false),
        ("empty", Some(""), false),
        (
            "stale",
            Some("R01A\tcrates/example/src/lib.rs:2:stale\n"),
            false,
        ),
        (
            "duplicate",
            Some(
                "R01A\tcrates/example/src/lib.rs:1:match\nR01A\tcrates/example/src/lib.rs:1:match\n",
            ),
            false,
        ),
        ("malformed", Some("not-a-tsv-row\n"), false),
        (
            "unreviewed",
            Some("UNREVIEWED\tcrates/example/src/lib.rs:1:match\n"),
            false,
        ),
        (
            "nonexistent-row",
            Some("R99\tcrates/example/src/lib.rs:1:match\n"),
            false,
        ),
    ];

    for (name, contents, should_pass) in cases {
        match contents {
            Some(contents) => fs::write(&mapping, contents).expect("write mapping fixture"),
            None => {
                let _ = fs::remove_file(&mapping);
            }
        }
        let output = Command::new(root.join("scripts/check-unified-runtime-inventory.sh"))
            .args(["--check-files"])
            .arg(&mapping)
            .arg(&current)
            .arg(&inventory)
            .current_dir(&root)
            .output()
            .expect("run inventory checker fixture");
        assert_eq!(
            output.status.success(),
            should_pass,
            "fixture {name} had unexpected status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fs::write(&mapping, "R01A\tcrates/example/src/lib.rs:1:match\n")
        .expect("write mapped nonterminal fixture");
    for status in ["LEGACY", "ADAPTER"] {
        fs::write(
            &inventory,
            format!(
                "| Area | Path | Status |\n| --- | --- | --- |\n| R01A valid row | match | {status} |\n"
            ),
        )
        .expect("write nonterminal inventory fixture");
        let output = Command::new(root.join("scripts/check-unified-runtime-inventory.sh"))
            .args(["--check-files"])
            .arg(&mapping)
            .arg(&current)
            .arg(&inventory)
            .current_dir(&root)
            .output()
            .expect("run inventory checker nonterminal fixture");
        assert!(
            !output.status.success(),
            "mapped {status} ledger row unexpectedly passed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("nonterminal ledger row R01A"),
            "mapped {status} ledger row did not report its status\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fs::remove_dir_all(&fixture_dir).expect("remove inventory checker fixture directory");
}

#[test]
fn unified_runtime_inventory_writer_preserves_reviews_and_marks_only_new_matches() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture_dir = std::env::temp_dir().join(format!(
        "sema-runtime-inventory-writer-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create inventory writer fixture directory");
    let mapping = fixture_dir.join("mapping.tsv");
    let script = root.join("scripts/check-unified-runtime-inventory.sh");

    let first_write = Command::new(&script)
        .arg("--write-mapping")
        .env("UNIFIED_RUNTIME_MAPPING_FILE", &mapping)
        .current_dir(&root)
        .output()
        .expect("bootstrap inventory writer fixture");
    assert!(
        first_write.status.success(),
        "inventory writer bootstrap failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&first_write.stdout),
        String::from_utf8_lossy(&first_write.stderr)
    );

    let bootstrap = fs::read_to_string(&mapping).expect("read bootstrapped mapping fixture");
    let first = bootstrap.lines().next().expect("bootstrap has a match");
    let payload = first
        .strip_prefix("UNREVIEWED\t")
        .expect("new matches are explicitly unreviewed");
    let reviewed = format!("R01A\t{payload}");
    let mut seeded = bootstrap.replacen(first, &reviewed, 1);
    seeded.push_str("F01A\tcrates/removed/src/lib.rs:1:stale\n");
    fs::write(&mapping, seeded).expect("seed reviewed and vanished mapping entries");

    let second_write = Command::new(&script)
        .arg("--write-mapping")
        .env("UNIFIED_RUNTIME_MAPPING_FILE", &mapping)
        .current_dir(&root)
        .output()
        .expect("refresh inventory writer fixture");
    assert!(
        second_write.status.success(),
        "inventory writer refresh failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&second_write.stdout),
        String::from_utf8_lossy(&second_write.stderr)
    );

    let refreshed = fs::read_to_string(&mapping).expect("read refreshed mapping fixture");
    assert!(
        refreshed.lines().any(|line| line == reviewed),
        "inventory writer did not preserve the reviewed assignment"
    );
    assert!(
        !refreshed.contains("crates/removed/src/lib.rs:1:stale"),
        "inventory writer retained a vanished payload"
    );
    assert!(
        refreshed
            .lines()
            .any(|line| line.starts_with("UNREVIEWED\t")),
        "inventory writer heuristically assigned every new payload"
    );

    fs::remove_dir_all(&fixture_dir).expect("remove inventory writer fixture directory");
}

#[test]
fn unified_runtime_inventory_checker_rejects_discovery_scan_failure() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture_dir =
        std::env::temp_dir().join(format!("sema-runtime-missing-rg-{}", std::process::id()));
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create missing rg fixture directory");
    let mapping = fixture_dir.join("mapping.tsv");

    for mode in ["--check", "--write-mapping"] {
        let output = Command::new(root.join("scripts/check-unified-runtime-inventory.sh"))
            .arg(mode)
            .env("UNIFIED_RUNTIME_RG_BIN", "/definitely/missing/rg")
            .env("UNIFIED_RUNTIME_MAPPING_FILE", &mapping)
            .current_dir(&root)
            .output()
            .expect("run inventory checker with missing discovery scanner");

        assert!(
            !output.status.success(),
            "inventory {mode} swallowed discovery scan failure\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("runtime inventory discovery scan failed"),
            "inventory {mode} did not report the discovery failure\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !mapping.exists(),
            "inventory {mode} wrote a mapping after a missing scanner failure"
        );
    }

    fs::remove_dir_all(&fixture_dir).expect("remove missing rg fixture directory");
}

#[cfg(unix)]
#[test]
fn unified_runtime_inventory_checker_rejects_partial_discovery_scan_failure() {
    use std::os::unix::fs::PermissionsExt;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture_dir = std::env::temp_dir().join(format!(
        "sema-runtime-partial-rg-failure-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create partial rg failure fixture directory");
    let fixture = fixture_dir.join("rg");
    let mapping = fixture_dir.join("mapping.tsv");
    fs::write(
        &fixture,
        "#!/bin/sh\ncase \"$*\" in *'IoHandle|IoPoll'*) exit 1;; *) exit 0;; esac\n",
    )
    .expect("write partial rg failure fixture");
    let mut permissions = fs::metadata(&fixture)
        .expect("read partial rg failure fixture metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&fixture, permissions).expect("make partial rg failure fixture executable");

    for mode in ["--check", "--write-mapping"] {
        let output = Command::new(root.join("scripts/check-unified-runtime-inventory.sh"))
            .arg(mode)
            .env("UNIFIED_RUNTIME_RG_BIN", &fixture)
            .env("UNIFIED_RUNTIME_MAPPING_FILE", &mapping)
            .current_dir(&root)
            .output()
            .expect("run inventory checker with partial discovery failure");

        assert!(
            !output.status.success(),
            "inventory {mode} swallowed the first discovery scan failure\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("runtime inventory discovery scan failed"),
            "inventory {mode} did not report the discovery failure\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !mapping.exists(),
            "inventory {mode} wrote a mapping after a partial discovery failure"
        );
    }
    fs::remove_dir_all(&fixture_dir).expect("remove partial rg failure fixture directory");
}
