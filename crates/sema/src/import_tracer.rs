//! Static import/load tracing for `sema build`.
//!
//! Walks the AST of a root source file and all transitively imported files,
//! collecting their contents into a map suitable for bundling into a VFS
//! archive. Only literal string paths can be resolved statically; dynamic
//! imports produce a warning on stderr.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use sema_core::resolve::{is_package_import, resolve_package_import};
use sema_core::Value;

/// Trace all transitive `(import "...")` and `(load "...")` dependencies
/// starting from `root_file`.
///
/// Returns a map of `relative_path -> file_contents` for every discovered
/// dependency. The root file itself is **not** included (it is compiled to
/// bytecode separately).
pub fn trace_imports(root_file: &Path) -> Result<HashMap<String, Vec<u8>>, String> {
    let root_file = root_file
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize root file {}: {e}", root_file.display()))?;

    let root_dir = root_file
        .parent()
        .ok_or_else(|| format!("root file has no parent directory: {}", root_file.display()))?
        .to_path_buf();

    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut result: HashMap<String, Vec<u8>> = HashMap::new();

    // Mark the root file as visited so we never add it to the result map.
    visited.insert(root_file.clone());

    // Read and parse the root file, then trace its imports.
    let source = std::fs::read_to_string(&root_file)
        .map_err(|e| format!("cannot read root file {}: {e}", root_file.display()))?;

    let exprs = sema_reader::read_many(&source)
        .map_err(|e| format!("parse error in {}: {}", root_file.display(), e.inner()))?;

    trace_file_imports(&exprs, &root_file, &root_dir, &mut visited, &mut result)?;

    Ok(result)
}

/// Parse the expressions from a single file and extract all import/load paths,
/// recursively tracing each discovered dependency.
fn trace_file_imports(
    exprs: &[Value],
    current_file: &Path,
    root_dir: &Path,
    visited: &mut HashSet<PathBuf>,
    result: &mut HashMap<String, Vec<u8>>,
) -> Result<(), String> {
    for expr in exprs {
        extract_imports(expr, current_file, root_dir, visited, result)?;
    }
    Ok(())
}

/// Recursively walk an AST expression looking for `(import "path" ...)`
/// and `(load "path")` forms. For any other list form, recurse into all
/// children to catch imports nested inside `begin`, `let`, `define`, etc.
fn extract_imports(
    expr: &Value,
    current_file: &Path,
    root_dir: &Path,
    visited: &mut HashSet<PathBuf>,
    result: &mut HashMap<String, Vec<u8>>,
) -> Result<(), String> {
    let items = match expr.as_list() {
        Some(items) if !items.is_empty() => items,
        _ => return Ok(()),
    };

    // Check the head of the list.
    if let Some(head) = items[0].as_symbol() {
        match head.as_str() {
            // Quoted data is not evaluated — don't trace imports inside it.
            "quote" | "quasiquote" => return Ok(()),
            "import" | "load" => {
                if items.len() >= 2 {
                    if let Some(path_str) = items[1].as_str() {
                        process_import(path_str, current_file, root_dir, visited, result)?;
                    } else {
                        // Dynamic import -- cannot resolve statically.
                        eprintln!(
                            "warning: dynamic {} in {} cannot be resolved statically; \
                             use --include to add it manually",
                            head,
                            current_file.display()
                        );
                    }
                }
                // Don't recurse further into import/load forms.
                return Ok(());
            }
            "module" => {
                // (module name ... body ...)
                // The body starts after the module name (index 2+), but there
                // may be an (export ...) form in there too -- just recurse
                // into everything after the name.
                for item in items.iter().skip(2) {
                    extract_imports(item, current_file, root_dir, visited, result)?;
                }
                return Ok(());
            }
            _ => {}
        }
    }

    // For any other list, recurse into all children.
    for item in items.iter() {
        extract_imports(item, current_file, root_dir, visited, result)?;
    }

    Ok(())
}

/// Resolve an import path relative to the importing file, read its contents,
/// add it to the result map, and recursively trace its own imports.
fn process_import(
    import_path: &str,
    current_file: &Path,
    root_dir: &Path,
    visited: &mut HashSet<PathBuf>,
    result: &mut HashMap<String, Vec<u8>>,
) -> Result<(), String> {
    if is_package_import(import_path) {
        return process_package_import(import_path, root_dir, visited, result);
    }

    // Resolve relative to the directory of the importing file.
    let base_dir = current_file
        .parent()
        .ok_or_else(|| format!("file has no parent directory: {}", current_file.display()))?;

    let resolved = base_dir.join(import_path);

    // An import the tracer can't resolve at build time is NOT a build failure:
    // the path may be generated/written at runtime (e.g. `(file/write p ...)`
    // then `(import p)`), or simply live outside the project tree. The runtime
    // `import`/`load` resolves the VFS first and then the real filesystem, so we
    // warn that it won't be bundled and leave it to be resolved when the binary
    // runs. This keeps `sema build` working for dynamic/runtime-module programs.
    let canonical = match resolved.canonicalize() {
        Ok(c) => c,
        Err(_) => {
            eprintln!(
                "  warning: import \"{}\" (from {}) couldn't be resolved at build time; \
                 not bundled — it will be resolved at runtime (filesystem/VFS)",
                import_path,
                current_file.display()
            );
            return Ok(());
        }
    };

    // Circular import protection.
    if visited.contains(&canonical) {
        return Ok(());
    }
    visited.insert(canonical.clone());

    // Read file contents.
    let contents = match std::fs::read(&canonical) {
        Ok(c) => c,
        Err(_) => {
            eprintln!(
                "  warning: import \"{}\" couldn't be read at build time; not bundled \
                 (resolved at runtime)",
                canonical.display()
            );
            return Ok(());
        }
    };

    // Compute relative path for the VFS key.
    // Check packages_dir FIRST — package files must get package-relative keys
    // (e.g., "json-utils/helpers.sema"), not project-relative keys that would
    // leak the SEMA_HOME path.
    let rel_path = {
        let pkg_dir = sema_core::resolve::packages_dir();
        let canon_pkg = pkg_dir.canonicalize().ok();
        if let Some(ref cpkg) = canon_pkg {
            if let Ok(rel) = canonical.strip_prefix(cpkg) {
                rel.to_string_lossy().replace('\\', "/")
            } else if let Ok(rel) = canonical.strip_prefix(root_dir) {
                rel.to_string_lossy().replace('\\', "/")
            } else {
                eprintln!(
                    "  warning: imported file {} is outside the project and packages \
                     directories; not bundled (resolved at runtime)",
                    canonical.display()
                );
                return Ok(());
            }
        } else if let Ok(rel) = canonical.strip_prefix(root_dir) {
            rel.to_string_lossy().replace('\\', "/")
        } else {
            eprintln!(
                "  warning: imported file {} is outside the project directory; not \
                 bundled (resolved at runtime)",
                canonical.display()
            );
            return Ok(());
        }
    };

    // Validate the VFS key before inserting
    sema_core::vfs::validate_vfs_path(&rel_path)
        .map_err(|e| format!("invalid VFS key for {}: {e}", canonical.display()))?;

    // Detect collisions: if a key already exists with different content, error
    if let Some(existing) = result.get(&rel_path) {
        if *existing != contents {
            return Err(format!(
                "VFS key collision: \"{}\" maps to two different files with different content",
                rel_path
            ));
        }
        // Same content — skip reinserting (diamond dependency)
    } else {
        result.insert(rel_path, contents.clone());
    }

    // Recursively trace the imported file's own imports.
    // Only parse if it looks like a text file (sema source).
    if let Ok(source) = std::str::from_utf8(&contents) {
        if let Ok(exprs) = sema_reader::read_many(source) {
            trace_file_imports(&exprs, &canonical, root_dir, visited, result)?;
        }
        // If parsing fails, we still included the file -- just don't trace deeper.
    }

    Ok(())
}

/// Resolve a package import via `resolve_package_import`, read its contents,
/// and add it to the result map using the package path as the VFS key.
fn process_package_import(
    import_path: &str,
    root_dir: &Path,
    visited: &mut HashSet<PathBuf>,
    result: &mut HashMap<String, Vec<u8>>,
) -> Result<(), String> {
    let resolved = match resolve_package_import(import_path) {
        Ok(p) => p,
        Err(_) => {
            return Err(format!(
                "package \"{}\" is not installed (hint: sema pkg add {})",
                import_path, import_path
            ));
        }
    };

    let canonical = resolved.canonicalize().map_err(|e| {
        format!(
            "cannot canonicalize package import \"{}\": {e}",
            import_path
        )
    })?;

    if visited.contains(&canonical) {
        return Ok(());
    }
    visited.insert(canonical.clone());

    let contents = std::fs::read(&canonical)
        .map_err(|e| format!("cannot read {}: {e}", canonical.display()))?;

    // Use the package path as the VFS key for portability.
    // Validate and check for collisions.
    sema_core::vfs::validate_vfs_path(import_path)
        .map_err(|e| format!("invalid VFS key for package \"{import_path}\": {e}"))?;

    if let Some(existing) = result.get(import_path) {
        if *existing != contents {
            return Err(format!(
                "VFS key collision: \"{}\" maps to two different files with different content",
                import_path
            ));
        }
    } else {
        result.insert(import_path.to_string(), contents.clone());
    }

    // Recursively trace the package file's own imports.
    if let Ok(source) = std::str::from_utf8(&contents) {
        if let Ok(exprs) = sema_reader::read_many(source) {
            trace_file_imports(&exprs, &canonical, root_dir, visited, result)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    /// Serialize all tests that set SEMA_HOME to avoid env var races.
    static SEMA_HOME_LOCK: Mutex<()> = Mutex::new(());

    fn tmpdir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("sema-tracer-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Run a test body in a dedicated thread with an isolated SEMA_HOME.
    ///
    /// Creates a project dir and a separate sema-home dir, sets `SEMA_HOME`
    /// to point at the sema-home, runs the closure, then cleans up both dirs
    /// and restores the env var. The thread isolation prevents env var races
    /// with parallel tests.
    fn with_fake_sema_home<F>(name: &str, f: F)
    where
        F: FnOnce(&Path, &Path) + Send + 'static,
    {
        let name = name.to_owned();
        std::thread::spawn(move || {
            let _lock = SEMA_HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = tmpdir(&name);
            let sema_home = tmpdir(&format!("{name}-home"));
            fs::create_dir_all(sema_home.join("packages")).unwrap();
            std::env::set_var("SEMA_HOME", &sema_home);

            f(&dir, &sema_home);

            std::env::remove_var("SEMA_HOME");
            let _ = fs::remove_dir_all(&dir);
            let _ = fs::remove_dir_all(&sema_home);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn test_trace_nonexistent_root() {
        let result = trace_imports(Path::new("/nonexistent/file.sema"));
        assert!(result.is_err());
    }

    #[test]
    fn test_trace_no_imports() {
        let dir = tmpdir("no-imports");
        fs::write(dir.join("main.sema"), "(define x 42)").unwrap();
        let result = trace_imports(&dir.join("main.sema")).unwrap();
        assert!(result.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_trace_single_import() {
        let dir = tmpdir("single");
        fs::write(dir.join("lib.sema"), "(define y 1)").unwrap();
        fs::write(dir.join("main.sema"), r#"(import "lib.sema")"#).unwrap();
        let result = trace_imports(&dir.join("main.sema")).unwrap();
        assert!(
            result.contains_key("lib.sema"),
            "expected lib.sema in result: {result:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_trace_circular_imports() {
        let dir = tmpdir("circular");
        fs::write(dir.join("a.sema"), r#"(import "b.sema")"#).unwrap();
        fs::write(dir.join("b.sema"), r#"(import "a.sema")"#).unwrap();
        let result = trace_imports(&dir.join("a.sema")).unwrap();
        assert!(result.contains_key("b.sema"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_trace_dynamic_import_warns() {
        let dir = tmpdir("dynamic");
        fs::write(dir.join("main.sema"), "(import some-var)").unwrap();
        let result = trace_imports(&dir.join("main.sema")).unwrap();
        assert!(result.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_trace_missing_import_warns_not_errors() {
        // A literal import the tracer can't resolve at build time is NOT a build
        // failure — the path may be generated/written at runtime, and `import`
        // resolves the filesystem/VFS when the binary runs. The tracer warns and
        // leaves it unbundled rather than aborting the build.
        let dir = tmpdir("missing");
        fs::write(dir.join("main.sema"), r#"(import "nonexistent.sema")"#).unwrap();
        let result =
            trace_imports(&dir.join("main.sema")).expect("missing import must not abort the build");
        assert!(result.is_empty(), "unresolved import must not be bundled");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_trace_module_form() {
        let dir = tmpdir("module");
        fs::write(dir.join("lib.sema"), "(define z 99)").unwrap();
        fs::write(
            dir.join("main.sema"),
            r#"(module mymod (export z) (import "lib.sema"))"#,
        )
        .unwrap();
        let result = trace_imports(&dir.join("main.sema")).unwrap();
        assert!(result.contains_key("lib.sema"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_trace_load_form() {
        let dir = tmpdir("load");
        fs::write(dir.join("defs.sema"), "(define loaded 1)").unwrap();
        fs::write(dir.join("main.sema"), r#"(load "defs.sema")"#).unwrap();
        let result = trace_imports(&dir.join("main.sema")).unwrap();
        assert!(result.contains_key("defs.sema"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_trace_binary_file_included() {
        let dir = tmpdir("binary");
        fs::write(dir.join("data.bin"), [0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        fs::write(dir.join("main.sema"), r#"(import "data.bin")"#).unwrap();
        let result = trace_imports(&dir.join("main.sema")).unwrap();
        assert!(
            result.contains_key("data.bin"),
            "binary file should be included"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_trace_nested_imports() {
        let dir = tmpdir("nested");
        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::write(dir.join("lib/deep.sema"), "(define deep 1)").unwrap();
        fs::write(dir.join("lib/mid.sema"), r#"(import "deep.sema")"#).unwrap();
        fs::write(dir.join("main.sema"), r#"(import "lib/mid.sema")"#).unwrap();
        let result = trace_imports(&dir.join("main.sema")).unwrap();
        assert!(result.contains_key("lib/mid.sema"));
        assert!(result.contains_key("lib/deep.sema"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_trace_import_in_begin() {
        let dir = tmpdir("begin");
        fs::write(dir.join("lib.sema"), "(define y 1)").unwrap();
        fs::write(
            dir.join("main.sema"),
            r#"(begin (import "lib.sema") (+ 1 2))"#,
        )
        .unwrap();
        let result = trace_imports(&dir.join("main.sema")).unwrap();
        assert!(result.contains_key("lib.sema"));
        let _ = fs::remove_dir_all(&dir);
    }

    // All package import-tracer tests mutate the process-global SEMA_HOME env
    // var, as do pkg.rs's #[serial] tests — so these join the SAME serial_test
    // global group. The module-local SEMA_HOME_LOCK alone cannot serialize
    // against pkg.rs (two different locks guarding one global).
    #[test]
    #[serial_test::serial]
    fn test_trace_package_imports() {
        with_fake_sema_home("pkg-all", |dir, sema_home| {
            // --- Set up fake packages ---
            let pkgs = sema_home.join("packages");

            let mylib = pkgs.join("github.com/test/mylib");
            fs::create_dir_all(&mylib).unwrap();
            fs::write(mylib.join("package.sema"), "(define pkg-val 42)").unwrap();

            let translib = pkgs.join("github.com/test/translib");
            fs::create_dir_all(&translib).unwrap();
            fs::write(
                translib.join("package.sema"),
                r#"(import "helpers.sema") (define main-val 1)"#,
            )
            .unwrap();
            fs::write(translib.join("helpers.sema"), "(define helper-val 2)").unwrap();

            let custom = pkgs.join("github.com/test/custom");
            fs::create_dir_all(&custom).unwrap();
            fs::write(custom.join("sema.toml"), "entrypoint = \"lib.sema\"\n").unwrap();
            fs::write(custom.join("lib.sema"), "(define custom-val 99)").unwrap();

            let utils = pkgs.join("github.com/test/utils");
            fs::create_dir_all(&utils).unwrap();
            fs::write(utils.join("package.sema"), "(define util-fn 1)").unwrap();

            // --- Basic package import ---
            {
                fs::write(dir.join("main.sema"), r#"(import "github.com/test/mylib")"#).unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.contains_key("github.com/test/mylib"),
                    "package import should be traced: {result:?}"
                );
                assert_eq!(
                    result.get("github.com/test/mylib").unwrap(),
                    b"(define pkg-val 42)"
                );
            }

            // --- Transitive imports through package with portable VFS keys ---
            {
                fs::write(
                    dir.join("main.sema"),
                    r#"(import "github.com/test/translib")"#,
                )
                .unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.contains_key("github.com/test/translib"),
                    "package should be traced: {result:?}"
                );
                assert!(
                    result.contains_key("github.com/test/translib/helpers.sema"),
                    "transitive import should have portable key 'github.com/test/translib/helpers.sema': {result:?}"
                );

                for key in result.keys() {
                    assert!(!key.starts_with('/'), "VFS key should be relative: {key}");
                    assert!(
                        !key.contains(&*sema_home.to_string_lossy()),
                        "VFS key must not contain SEMA_HOME path: {key}"
                    );
                }
            }

            // --- Uninstalled package is a hard error ---
            {
                fs::write(
                    dir.join("main.sema"),
                    r#"(import "github.com/nonexistent/pkg")"#,
                )
                .unwrap();
                let result = trace_imports(&dir.join("main.sema"));
                assert!(result.is_err(), "missing package should be a hard error");
                let err = result.unwrap_err();
                assert!(err.contains("not installed"), "got: {err}");
                assert!(err.contains("sema pkg add"), "got: {err}");
            }

            // --- Mixed local and package imports ---
            {
                fs::write(dir.join("local.sema"), "(define local-val 2)").unwrap();
                fs::write(
                    dir.join("main.sema"),
                    "(import \"local.sema\")\n(import \"github.com/test/utils\")",
                )
                .unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(result.contains_key("local.sema"), "{result:?}");
                assert!(result.contains_key("github.com/test/utils"), "{result:?}");
            }

            // --- Custom entrypoint via sema.toml ---
            {
                fs::write(
                    dir.join("main.sema"),
                    r#"(import "github.com/test/custom")"#,
                )
                .unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(result.contains_key("github.com/test/custom"), "{result:?}");
                assert_eq!(
                    result.get("github.com/test/custom").unwrap(),
                    b"(define custom-val 99)"
                );
            }
        });
    }

    #[test]
    fn test_trace_quoted_import_not_traced() {
        let dir = tmpdir("quoted");
        fs::write(
            dir.join("main.sema"),
            r#"(quote (import "nonexistent.sema"))"#,
        )
        .unwrap();
        let result = trace_imports(&dir.join("main.sema")).unwrap();
        assert!(result.is_empty(), "quoted imports should not be traced");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_trace_quasiquoted_import_not_traced() {
        let dir = tmpdir("quasiquoted");
        fs::write(
            dir.join("main.sema"),
            r#"(quasiquote (import "nonexistent.sema"))"#,
        )
        .unwrap();
        let result = trace_imports(&dir.join("main.sema")).unwrap();
        assert!(
            result.is_empty(),
            "quasiquoted imports should not be traced"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial_test::serial]
    fn test_trace_package_advanced_scenarios() {
        with_fake_sema_home("pkg-advanced", |dir, sema_home| {
            let pkgs = sema_home.join("packages");

            // --- Create all package directories up front ---

            // 1. Registry short-name package
            let json_utils = pkgs.join("json-utils");
            fs::create_dir_all(&json_utils).unwrap();
            fs::write(json_utils.join("package.sema"), "(define json-val 42)").unwrap();

            // 2. Registry package with transitive relative imports
            let json_utils2 = pkgs.join("json-utils");
            // Already created above, just add helpers.sema
            fs::write(
                json_utils2.join("package.sema"),
                r#"(import "helpers.sema") (define json-val 1)"#,
            )
            .unwrap();
            fs::write(json_utils2.join("helpers.sema"), "(define helper-fn 2)").unwrap();

            // 3. Package-to-package chain
            let a_lib = pkgs.join("github.com/a/lib");
            fs::create_dir_all(&a_lib).unwrap();
            fs::write(
                a_lib.join("package.sema"),
                r#"(import "github.com/b/util") (define a-val 1)"#,
            )
            .unwrap();
            let b_util = pkgs.join("github.com/b/util");
            fs::create_dir_all(&b_util).unwrap();
            fs::write(b_util.join("package.sema"), "(define b-val 2)").unwrap();

            // 4. Registry package importing a git-style package (cross-type)
            let json_tools = pkgs.join("json-tools");
            fs::create_dir_all(&json_tools).unwrap();
            fs::write(
                json_tools.join("package.sema"),
                r#"(import "github.com/x/parser") (define tool-val 1)"#,
            )
            .unwrap();
            let x_parser = pkgs.join("github.com/x/parser");
            fs::create_dir_all(&x_parser).unwrap();
            fs::write(x_parser.join("package.sema"), "(define parser-val 2)").unwrap();

            // 5. Diamond dependency
            let da_lib = pkgs.join("github.com/a/lib");
            // Already created, overwrite for this scenario later
            let db_lib = pkgs.join("github.com/b/lib");
            fs::create_dir_all(&db_lib).unwrap();
            fs::write(
                db_lib.join("package.sema"),
                r#"(import "github.com/c/shared") (define b-val 2)"#,
            )
            .unwrap();
            let c_shared = pkgs.join("github.com/c/shared");
            fs::create_dir_all(&c_shared).unwrap();
            fs::write(c_shared.join("package.sema"), "(define shared-val 99)").unwrap();

            // 6. Package with nested subdirectory imports
            let deeplib = pkgs.join("github.com/x/deeplib");
            fs::create_dir_all(deeplib.join("src")).unwrap();
            fs::write(
                deeplib.join("package.sema"),
                r#"(import "src/utils.sema") (define deep-val 1)"#,
            )
            .unwrap();
            fs::write(deeplib.join("src/utils.sema"), "(define util-val 2)").unwrap();

            // 7. Custom entrypoint with transitive deps
            let customdeps = pkgs.join("github.com/x/customdeps");
            fs::create_dir_all(&customdeps).unwrap();
            fs::write(customdeps.join("sema.toml"), "entrypoint = \"lib.sema\"\n").unwrap();
            fs::write(
                customdeps.join("lib.sema"),
                r#"(import "internal.sema") (define val 1)"#,
            )
            .unwrap();
            fs::write(customdeps.join("internal.sema"), "(define internal-val 2)").unwrap();

            // 8. Package using load instead of import
            let loadpkg = pkgs.join("github.com/x/loadpkg");
            fs::create_dir_all(&loadpkg).unwrap();
            fs::write(
                loadpkg.join("package.sema"),
                r#"(load "defs.sema") (define val (+ loaded 1))"#,
            )
            .unwrap();
            fs::write(loadpkg.join("defs.sema"), "(define loaded 10)").unwrap();

            // 10. Deeply nested chain (3 levels)
            let l1 = pkgs.join("github.com/l1/pkg");
            fs::create_dir_all(&l1).unwrap();
            fs::write(
                l1.join("package.sema"),
                r#"(import "github.com/l2/pkg") (define l1-val 1)"#,
            )
            .unwrap();
            let l2 = pkgs.join("github.com/l2/pkg");
            fs::create_dir_all(&l2).unwrap();
            fs::write(
                l2.join("package.sema"),
                r#"(import "github.com/l3/pkg") (define l2-val 2)"#,
            )
            .unwrap();
            let l3 = pkgs.join("github.com/l3/pkg");
            fs::create_dir_all(&l3).unwrap();
            fs::write(l3.join("package.sema"), "(define l3-val 3)").unwrap();

            // Helper closure to assert all VFS keys are portable
            let assert_portable_keys = |result: &HashMap<String, Vec<u8>>, scenario: &str| {
                for key in result.keys() {
                    assert!(
                        !key.starts_with('/'),
                        "[{scenario}] VFS key should not start with '/': {key}"
                    );
                    assert!(
                        !key.contains(&*sema_home.to_string_lossy()),
                        "[{scenario}] VFS key must not contain SEMA_HOME path: {key}"
                    );
                    assert!(
                        !key.contains('\\'),
                        "[{scenario}] VFS key must not contain backslashes: {key}"
                    );
                }
            };

            // --- 1. Registry short-name package ---
            {
                // We need to reset json-utils to simple content for this test
                fs::write(json_utils.join("package.sema"), "(define json-val 42)").unwrap();
                fs::remove_file(json_utils.join("helpers.sema")).ok();

                fs::write(dir.join("main.sema"), r#"(import "json-utils")"#).unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.contains_key("json-utils"),
                    "[1] expected key 'json-utils' in result: {result:?}"
                );
                assert_eq!(
                    result.get("json-utils").unwrap(),
                    b"(define json-val 42)",
                    "[1] content mismatch for json-utils"
                );
                assert_portable_keys(&result, "1-registry-short-name");
            }

            // --- 2. Registry package with transitive relative imports ---
            {
                fs::write(
                    json_utils.join("package.sema"),
                    r#"(import "helpers.sema") (define json-val 1)"#,
                )
                .unwrap();
                fs::write(json_utils.join("helpers.sema"), "(define helper-fn 2)").unwrap();

                fs::write(dir.join("main.sema"), r#"(import "json-utils")"#).unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.contains_key("json-utils"),
                    "[2] expected key 'json-utils' in result: {result:?}"
                );
                assert!(
                    result.contains_key("json-utils/helpers.sema"),
                    "[2] expected key 'json-utils/helpers.sema' in result: {result:?}"
                );
                assert_portable_keys(&result, "2-registry-transitive");
            }

            // --- 3. Package-to-package chain ---
            {
                fs::write(dir.join("main.sema"), r#"(import "github.com/a/lib")"#).unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.contains_key("github.com/a/lib"),
                    "[3] expected key 'github.com/a/lib': {result:?}"
                );
                assert!(
                    result.contains_key("github.com/b/util"),
                    "[3] expected key 'github.com/b/util': {result:?}"
                );
                assert_portable_keys(&result, "3-pkg-to-pkg-chain");
            }

            // --- 4. Registry package importing a git-style package (cross-type) ---
            {
                fs::write(dir.join("main.sema"), r#"(import "json-tools")"#).unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.contains_key("json-tools"),
                    "[4] expected key 'json-tools': {result:?}"
                );
                assert!(
                    result.contains_key("github.com/x/parser"),
                    "[4] expected key 'github.com/x/parser': {result:?}"
                );
                assert_portable_keys(&result, "4-cross-type");
            }

            // --- 5. Diamond dependency ---
            {
                // Overwrite github.com/a/lib to import c/shared for this scenario
                fs::write(
                    da_lib.join("package.sema"),
                    r#"(import "github.com/c/shared") (define a-val 1)"#,
                )
                .unwrap();

                fs::write(
                    dir.join("main.sema"),
                    r#"(import "github.com/a/lib") (import "github.com/b/lib")"#,
                )
                .unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.contains_key("github.com/a/lib"),
                    "[5] expected key 'github.com/a/lib': {result:?}"
                );
                assert!(
                    result.contains_key("github.com/b/lib"),
                    "[5] expected key 'github.com/b/lib': {result:?}"
                );
                assert!(
                    result.contains_key("github.com/c/shared"),
                    "[5] expected key 'github.com/c/shared': {result:?}"
                );
                assert_eq!(
                    result.get("github.com/c/shared").unwrap(),
                    b"(define shared-val 99)",
                    "[5] shared package content mismatch"
                );
                assert_portable_keys(&result, "5-diamond");
            }

            // --- 6. Package with nested subdirectory imports ---
            {
                fs::write(dir.join("main.sema"), r#"(import "github.com/x/deeplib")"#).unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.contains_key("github.com/x/deeplib"),
                    "[6] expected key 'github.com/x/deeplib': {result:?}"
                );
                assert!(
                    result.contains_key("github.com/x/deeplib/src/utils.sema"),
                    "[6] expected key 'github.com/x/deeplib/src/utils.sema': {result:?}"
                );
                assert_portable_keys(&result, "6-nested-subdir");
            }

            // --- 7. Custom entrypoint with transitive deps ---
            {
                fs::write(
                    dir.join("main.sema"),
                    r#"(import "github.com/x/customdeps")"#,
                )
                .unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.contains_key("github.com/x/customdeps"),
                    "[7] expected key 'github.com/x/customdeps': {result:?}"
                );
                // Should contain lib.sema content, not sema.toml content
                let content = result.get("github.com/x/customdeps").unwrap();
                assert!(
                    content != b"entrypoint = \"lib.sema\"\n",
                    "[7] should contain lib.sema content, not sema.toml"
                );
                assert_eq!(
                    content, br#"(import "internal.sema") (define val 1)"#,
                    "[7] content should be lib.sema"
                );
                assert!(
                    result.contains_key("github.com/x/customdeps/internal.sema"),
                    "[7] expected key 'github.com/x/customdeps/internal.sema': {result:?}"
                );
                assert_portable_keys(&result, "7-custom-entrypoint");
            }

            // --- 8. Package using load instead of import ---
            {
                fs::write(dir.join("main.sema"), r#"(import "github.com/x/loadpkg")"#).unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.contains_key("github.com/x/loadpkg"),
                    "[8] expected key 'github.com/x/loadpkg': {result:?}"
                );
                assert!(
                    result.contains_key("github.com/x/loadpkg/defs.sema"),
                    "[8] expected key 'github.com/x/loadpkg/defs.sema': {result:?}"
                );
                assert_portable_keys(&result, "8-load-form");
            }

            // --- 9. Quoted import is NOT traced ---
            {
                fs::write(
                    dir.join("main.sema"),
                    r#"(quote (import "nonexistent.sema"))"#,
                )
                .unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.is_empty(),
                    "[9] quoted import should not be traced: {result:?}"
                );
            }

            // --- 10. Deeply nested chain (3 levels of packages) ---
            {
                fs::write(dir.join("main.sema"), r#"(import "github.com/l1/pkg")"#).unwrap();
                let result = trace_imports(&dir.join("main.sema")).unwrap();
                assert!(
                    result.contains_key("github.com/l1/pkg"),
                    "[10] expected key 'github.com/l1/pkg': {result:?}"
                );
                assert!(
                    result.contains_key("github.com/l2/pkg"),
                    "[10] expected key 'github.com/l2/pkg': {result:?}"
                );
                assert!(
                    result.contains_key("github.com/l3/pkg"),
                    "[10] expected key 'github.com/l3/pkg': {result:?}"
                );
                assert_portable_keys(&result, "10-deeply-nested");
            }
        });
    }

    #[test]
    #[serial_test::serial]
    fn test_trace_package_internal_files_have_portable_keys() {
        // When a package has transitive relative imports (e.g., helpers.sema),
        // the VFS key for the internal file must be relative to the package
        // (e.g., "json-utils/helpers.sema"), NOT contain the absolute
        // SEMA_HOME path.
        with_fake_sema_home("pkg-internal-keys", |dir, sema_home| {
            let pkgs = sema_home.join("packages");
            fs::create_dir_all(pkgs.join("json-utils")).unwrap();
            fs::write(
                pkgs.join("json-utils/package.sema"),
                r#"(import "helpers.sema") (define pkg-val 1)"#,
            )
            .unwrap();
            fs::write(
                pkgs.join("json-utils/helpers.sema"),
                "(define helper-val 99)",
            )
            .unwrap();

            fs::write(dir.join("main.sema"), r#"(import "json-utils")"#).unwrap();

            let result = trace_imports(&dir.join("main.sema")).unwrap();
            assert!(
                result.contains_key("json-utils"),
                "expected key 'json-utils': {:?}",
                result.keys().collect::<Vec<_>>()
            );
            assert!(
                result.contains_key("json-utils/helpers.sema"),
                "expected portable key, got: {:?}",
                result.keys().collect::<Vec<_>>()
            );
            for key in result.keys() {
                assert!(!key.starts_with('/'), "absolute key: {key}");
                assert!(!key.contains("sema-home"), "SEMA_HOME leak: {key}");
            }
        });
    }

    #[test]
    fn test_trace_distinct_files_get_distinct_keys() {
        // Two files with the same name in different directories should get
        // distinct VFS keys (root-relative), not collide.
        std::thread::spawn(|| {
            let dir = tmpdir("distinct-keys");

            fs::write(dir.join("utils.sema"), "(define a 1)").unwrap();
            fs::create_dir_all(dir.join("sub")).unwrap();
            fs::write(dir.join("sub/utils.sema"), "(define b 2)").unwrap();

            fs::write(dir.join("sub/lib.sema"), r#"(load "utils.sema")"#).unwrap();
            fs::write(
                dir.join("main.sema"),
                r#"(load "utils.sema") (load "sub/lib.sema")"#,
            )
            .unwrap();

            let result = trace_imports(&dir.join("main.sema")).unwrap();

            assert!(
                result.contains_key("utils.sema"),
                "missing root utils.sema: {:?}",
                result.keys().collect::<Vec<_>>()
            );
            assert!(
                result.contains_key("sub/utils.sema"),
                "missing sub/utils.sema: {:?}",
                result.keys().collect::<Vec<_>>()
            );
            // Verify they have different content — proves they're truly distinct files
            assert_ne!(
                result.get("utils.sema").unwrap(),
                result.get("sub/utils.sema").unwrap(),
                "distinct files should have different content"
            );

            let _ = fs::remove_dir_all(&dir);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn test_trace_diamond_local_deduplication() {
        // Diamond dependency: main→a→c and main→b→c.
        // c.sema should appear exactly once in the result (same canonical file).
        std::thread::spawn(|| {
            let dir = tmpdir("diamond-local");

            fs::write(dir.join("c.sema"), "(define shared 1)").unwrap();
            fs::write(dir.join("a.sema"), r#"(import "c.sema") (define a 1)"#).unwrap();
            fs::write(dir.join("b.sema"), r#"(import "c.sema") (define b 2)"#).unwrap();
            fs::write(
                dir.join("main.sema"),
                r#"(import "a.sema") (import "b.sema")"#,
            )
            .unwrap();

            let result = trace_imports(&dir.join("main.sema")).unwrap();

            assert!(result.contains_key("c.sema"));
            assert_eq!(result.get("c.sema").unwrap(), b"(define shared 1)");
            // Should have a, b, and c — c only once
            assert_eq!(
                result.len(),
                3,
                "expected 3 entries: {:?}",
                result.keys().collect::<Vec<_>>()
            );

            let _ = fs::remove_dir_all(&dir);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn test_trace_validates_vfs_keys() {
        // All VFS keys produced by the tracer should pass validate_vfs_path.
        // Uses only local file imports to avoid SEMA_HOME dependency.
        let dir = tmpdir("validate-keys");

        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::write(dir.join("lib/utils.sema"), "(define y 2)").unwrap();
        fs::write(dir.join("data.sema"), "(define z 3)").unwrap();
        fs::write(
            dir.join("main.sema"),
            r#"(load "lib/utils.sema") (load "data.sema")"#,
        )
        .unwrap();

        let result = trace_imports(&dir.join("main.sema")).unwrap();
        for key in result.keys() {
            assert!(
                sema_core::vfs::validate_vfs_path(key).is_ok(),
                "VFS key failed validation: {key}"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
