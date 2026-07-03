use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

use sema_core::resolve::{packages_dir, validate_package_spec};

const DEFAULT_REGISTRY: &str = "https://pkg.sema-lang.com";
const PKG_META_FILE: &str = ".sema-pkg.json";

fn ensure_sema_toml() -> Result<(), String> {
    let toml_path = Path::new("sema.toml");
    if !toml_path.exists() {
        let project_name = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "my-project".to_string());
        let content = format!(
            "[package]\nname = \"{project_name}\"\nversion = \"0.1.0\"\ndescription = \"\"\nentrypoint = \"package.sema\"\n\n[deps]\n"
        );
        std::fs::write(toml_path, content)
            .map_err(|e| format!("Failed to write sema.toml: {e}"))?;
        println!("✓ Created sema.toml");
    }
    Ok(())
}

fn run_git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("git {} failed: {stderr}", args.join(" ")))
    }
}

fn current_git_ref(dir: &Path) -> String {
    if let Ok(tag) = run_git(Some(dir), &["describe", "--tags", "--exact-match"]) {
        return tag;
    }
    run_git(Some(dir), &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_else(|_| "unknown".to_string())
}

fn find_package_dir(pkg_dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = pkg_dir.join(name);
    if exact.is_dir() {
        return Some(exact);
    }

    find_all_packages(pkg_dir).into_iter().find(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy() == name)
            .unwrap_or(false)
    })
}

fn find_all_packages(pkg_dir: &Path) -> Vec<PathBuf> {
    let mut packages = Vec::new();
    collect_packages(pkg_dir, &mut packages);
    packages
}

fn collect_packages(dir: &Path, packages: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // Skip symlinks to avoid loops and escaping the packages directory
        if path
            .symlink_metadata()
            .map(|m| m.is_symlink())
            .unwrap_or(false)
        {
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        if path.join("sema.toml").exists()
            || path.join("package.sema").exists()
            || path.join(PKG_META_FILE).exists()
        {
            packages.push(path);
        } else {
            collect_packages(&path, packages);
        }
    }
}

pub fn cmd_add(spec: &str, registry: Option<&str>) -> Result<(), String> {
    if is_git_spec(spec) {
        cmd_add_git(spec)
    } else {
        cmd_add_registry(spec, registry)
    }
}

/// Install a git package and return (ref, commit_sha).
/// Does NOT modify sema.toml or sema.lock.
fn install_git(spec: &sema_core::resolve::PackageSpec) -> Result<(String, String), String> {
    let pkg_dir = packages_dir();
    let dest = spec.dest_dir(&pkg_dir);

    if dest.exists() {
        run_git(Some(&dest), &["fetch", "origin"])?;
        run_git(Some(&dest), &["fetch", "--tags"])?;
        run_git(Some(&dest), &["checkout", &spec.git_ref])?;
        let current = current_git_ref(&dest);
        println!("✓ Updated {} → {current}", spec.path);
    } else {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        }
        run_git(None, &["clone", &spec.clone_url(), &dest.to_string_lossy()])?;
        run_git(Some(&dest), &["checkout", &spec.git_ref])?;
        let current = current_git_ref(&dest);
        println!("✓ Installed {} → {current}", spec.path);
    }

    let commit = run_git(Some(&dest), &["rev-parse", "HEAD"])?;
    let git_ref = spec.git_ref.clone();
    Ok((git_ref, commit))
}

/// Install a git package at a specific commit (for locked installs).
fn install_git_locked(
    spec: &sema_core::resolve::PackageSpec,
    expected_commit: &str,
) -> Result<(), String> {
    let pkg_dir = packages_dir();
    let dest = spec.dest_dir(&pkg_dir);

    if dest.exists() {
        run_git(Some(&dest), &["fetch", "origin"])?;
        run_git(Some(&dest), &["fetch", "--tags"])?;
    } else {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        }
        run_git(None, &["clone", &spec.clone_url(), &dest.to_string_lossy()])?;
    }

    run_git(Some(&dest), &["checkout", "--detach", expected_commit])?;
    let actual = run_git(Some(&dest), &["rev-parse", "HEAD"])?;
    if actual != expected_commit {
        return Err(format!(
            "Lock integrity error for {}: expected commit {expected_commit}, got {actual}",
            spec.path
        ));
    }
    println!("✓ Installed {} → {expected_commit} (locked)", spec.path);
    Ok(())
}

fn cmd_add_git(spec: &str) -> Result<(), String> {
    let spec = sema_core::resolve::PackageSpec::parse(spec).map_err(|e| e.to_string())?;
    let (git_ref, commit) = install_git(&spec)?;

    ensure_sema_toml()?;
    let toml_path = Path::new("sema.toml");
    match add_dep_to_toml(toml_path, spec.path.as_str(), &git_ref) {
        Ok(true) => println!("✓ Added {} = \"{}\" to sema.toml", spec.path, git_ref),
        Ok(false) => {}
        Err(e) => eprintln!("Warning: could not update sema.toml: {e}"),
    }

    match update_lock_entry(
        spec.path.as_str(),
        LockEntry::Git {
            git_ref,
            commit,
            direct: true,
        },
    ) {
        Ok(()) => println!("✓ Updated sema.lock"),
        Err(e) => eprintln!("Warning: could not update sema.lock: {e}"),
    }

    // Pull in this package's own dependencies, if any (transitive resolution).
    cmd_install(false)
}

fn cmd_add_registry(spec: &str, registry: Option<&str>) -> Result<(), String> {
    let (name, version) = if let Some((n, v)) = spec.rsplit_once('@') {
        (n.to_string(), Some(v.to_string()))
    } else {
        (spec.to_string(), None)
    };

    let registry_url = effective_registry(registry);

    // Resolve version: use explicit version or find latest
    let version = match version {
        Some(v) => v,
        None => {
            let info = registry_package_info(&name, &registry_url)?;
            latest_version(&info)
                .ok_or_else(|| format!("No published versions found for '{name}'"))?
        }
    };

    println!("Installing {name}@{version} from registry...");
    let checksum = registry_install(&name, &version, &registry_url)?;
    println!("✓ Installed {name}@{version}");

    ensure_sema_toml()?;
    let toml_path = Path::new("sema.toml");
    match add_dep_to_toml(toml_path, &name, &version) {
        Ok(true) => println!("✓ Added {name} = \"{version}\" to sema.toml"),
        Ok(false) => {}
        Err(e) => eprintln!("Warning: could not update sema.toml: {e}"),
    }

    match update_lock_entry(
        &name,
        LockEntry::Registry {
            version,
            registry: registry_url,
            checksum,
            direct: true,
        },
    ) {
        Ok(()) => println!("✓ Updated sema.lock"),
        Err(e) => eprintln!("Warning: could not update sema.lock: {e}"),
    }

    // Pull in this package's own dependencies, if any (transitive resolution).
    cmd_install(false)
}

/// Parse a `[deps]`-shaped TOML table into a plain name → version/ref map.
fn parse_deps_table(
    table: &toml::map::Map<String, toml::Value>,
) -> Result<BTreeMap<String, String>, String> {
    let mut result = BTreeMap::new();
    for (name, value) in table {
        let version = value.as_str().ok_or_else(|| {
            format!("dep '{name}': expected a version/ref string (e.g., \"1.0.0\" or \"main\")")
        })?;
        result.insert(name.clone(), version.to_string());
    }
    Ok(result)
}

/// Resolve+install a package with no usable lock entry (fresh install or a
/// version/ref bump). Does not touch sema.toml or the lock file.
fn resolve_and_install_one(name: &str, version: &str) -> Result<LockEntry, String> {
    if is_git_spec(name) {
        let spec_str = format!("{name}@{version}");
        let spec = sema_core::resolve::PackageSpec::parse(&spec_str).map_err(|e| e.to_string())?;
        let (git_ref, commit) = install_git(&spec)?;
        Ok(LockEntry::Git {
            git_ref,
            commit,
            direct: false, // caller normalizes via LockEntry::set_direct
        })
    } else {
        let registry_url = effective_registry(None);
        let checksum = registry_install(name, version, &registry_url)?;
        Ok(LockEntry::Registry {
            version: version.to_string(),
            registry: registry_url,
            checksum,
            direct: false,
        })
    }
}

/// Install (and integrity-verify) a package from an existing, matching lock entry.
fn install_from_lock_entry(name: &str, entry: &LockEntry) -> Result<(), String> {
    match entry {
        LockEntry::Git {
            git_ref, commit, ..
        } => {
            let spec_str = format!("{name}@{git_ref}");
            let spec =
                sema_core::resolve::PackageSpec::parse(&spec_str).map_err(|e| e.to_string())?;
            install_git_locked(&spec, commit)
        }
        LockEntry::Registry {
            version,
            registry,
            checksum,
            ..
        } => registry_install_locked(name, version, registry, checksum),
    }
}

/// Read an already-installed package's own `[deps]` table. Missing manifest
/// or missing `[deps]` section both mean "no dependencies" — not an error.
fn read_dest_manifest_deps(name: &str) -> Result<BTreeMap<String, String>, String> {
    let toml_path = packages_dir().join(name).join("sema.toml");
    if !toml_path.exists() {
        return Ok(BTreeMap::new());
    }
    let content = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("Failed to read {}: {e}", toml_path.display()))?;
    let doc: toml::Value = toml::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {e}", toml_path.display()))?;
    match doc.get("deps").and_then(|d| d.as_table()) {
        Some(table) => parse_deps_table(table),
        None => Ok(BTreeMap::new()),
    }
}

/// Notable, non-fatal events produced while walking the dependency graph.
#[derive(Debug, Clone, PartialEq)]
enum ResolutionNote {
    /// A direct dependency's pinned version/ref overrode a conflicting
    /// transitive request for the same package.
    DirectOverride {
        name: String,
        direct_version: String,
        requested_by: String,
        requested_version: String,
    },
    /// Two or more transitive requesters wanted different (but
    /// semver-compatible) versions of the same registry package; the
    /// higher one was chosen.
    DiamondAutoResolved {
        name: String,
        chosen_version: String,
        requesters: Vec<(String, String)>,
    },
}

struct ResolvedPkg {
    entry: LockEntry,
    direct: bool,
    /// (requester name — "<root>" for a direct dep, requested version/ref)
    requesters: Vec<(String, String)>,
}

/// Decide how to reconcile two conflicting requests for the same transitive
/// (non-direct) package. Registry packages auto-resolve to the higher
/// version when they're semver-compatible (same major, or same 0.x minor);
/// otherwise — including all git ref conflicts, which have no ordering —
/// this is a hard error naming every known requester.
fn resolve_diamond_conflict(
    name: &str,
    current_version: &str,
    requested_version: &str,
    requesters: &[(String, String)],
) -> Result<String, String> {
    let conflict_err = |detail: &str| -> String {
        let requester_list = requesters
            .iter()
            .map(|(who, ver)| format!("  - {who} wants {ver}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Conflicting transitive dependency requirements for '{name}':\n{requester_list}\n\
             {detail}\n\
             Add an explicit \"{name}\" entry to your own sema.toml [deps] to force a version."
        )
    };

    if is_git_spec(name) {
        return Err(conflict_err(
            "git refs cannot be automatically reconciled (no version ordering).",
        ));
    }

    let current = semver::Version::parse(current_version).map_err(|_| {
        conflict_err(&format!(
            "'{current_version}' is not a valid semver version"
        ))
    })?;
    let requested = semver::Version::parse(requested_version).map_err(|_| {
        conflict_err(&format!(
            "'{requested_version}' is not a valid semver version"
        ))
    })?;

    let compatible = if current.major == 0 && requested.major == 0 {
        current.minor == requested.minor
    } else {
        current.major == requested.major
    };
    if !compatible {
        return Err(conflict_err("versions differ in a breaking (major) way."));
    }

    Ok(if requested > current {
        requested_version.to_string()
    } else {
        current_version.to_string()
    })
}

/// A hard cap on BFS iterations, as a backstop against pathological
/// oscillating diamond conflicts (a reinstall re-enqueuing children that
/// keep bouncing versions back and forth). Real dependency graphs never come
/// close to this.
const MAX_RESOLUTION_STEPS: usize = 10_000;

/// Reads a package's own `[deps]` (name → version/ref spec) by name. Injected
/// into `resolve_dependency_graph` so the graph walk stays unit-testable.
type ReadManifestDeps<'a> = dyn Fn(&str) -> Result<BTreeMap<String, String>, String> + 'a;

/// Walk the full transitive dependency graph starting from `direct_deps`,
/// resolving/installing each package exactly once and reconciling conflicts
/// per `resolve_diamond_conflict`. All I/O is injected so this is
/// unit-testable without git/network. Returns the fully-flattened new lock,
/// the names pruned from `existing_lock` (no longer reachable), and
/// human-readable notes for the caller to print.
fn resolve_dependency_graph(
    direct_deps: &BTreeMap<String, String>,
    existing_lock: &LockFile,
    install_fresh: &mut dyn FnMut(&str, &str) -> Result<LockEntry, String>,
    install_locked: &mut dyn FnMut(&str, &LockEntry) -> Result<(), String>,
    read_manifest_deps: &ReadManifestDeps,
) -> Result<(LockFile, Vec<String>, Vec<ResolutionNote>), String> {
    let mut resolved: BTreeMap<String, ResolvedPkg> = BTreeMap::new();
    let mut notes = Vec::new();
    let mut queue: VecDeque<(String, String, String)> = VecDeque::new();
    let mut steps: usize = 0;

    let mut resolve_one = |name: &str, version: &str| -> Result<LockEntry, String> {
        if let Some(existing) = existing_lock.entries.get(name) {
            if existing.requested() == version {
                install_locked(name, existing)?;
                return Ok(existing.clone());
            }
        }
        install_fresh(name, version)
    };

    // Phase 1: direct deps always resolve first and always win.
    for (name, version) in direct_deps {
        let mut entry = resolve_one(name, version)?;
        entry.set_direct(true);
        let children = read_manifest_deps(name)?;
        resolved.insert(
            name.clone(),
            ResolvedPkg {
                entry,
                direct: true,
                requesters: vec![("<root>".to_string(), version.clone())],
            },
        );
        for (child_name, child_version) in children {
            queue.push_back((child_name, child_version, name.clone()));
        }
    }

    // Phase 2: BFS over transitive requests.
    while let Some((name, version, requested_by)) = queue.pop_front() {
        steps += 1;
        if steps > MAX_RESOLUTION_STEPS {
            return Err(format!(
                "Dependency resolution did not converge after {MAX_RESOLUTION_STEPS} steps \
                 (possible circular version requirement involving '{name}'). \
                 Add an explicit \"{name}\" entry to your own sema.toml [deps] to pin it."
            ));
        }

        match resolved.get_mut(&name) {
            None => {
                let mut entry = resolve_one(&name, &version)?;
                entry.set_direct(false);
                let children = read_manifest_deps(&name)?;
                resolved.insert(
                    name.clone(),
                    ResolvedPkg {
                        entry,
                        direct: false,
                        requesters: vec![(requested_by, version)],
                    },
                );
                for (child_name, child_version) in children {
                    queue.push_back((child_name, child_version, name.clone()));
                }
            }
            Some(pkg) if pkg.direct => {
                let direct_version = pkg.entry.requested().to_string();
                if direct_version != version {
                    notes.push(ResolutionNote::DirectOverride {
                        name: name.clone(),
                        direct_version,
                        requested_by: requested_by.clone(),
                        requested_version: version.clone(),
                    });
                }
                pkg.requesters.push((requested_by, version));
            }
            Some(pkg) => {
                let current_version = pkg.entry.requested().to_string();
                pkg.requesters.push((requested_by, version.clone()));
                if current_version == version {
                    continue;
                }
                let chosen =
                    resolve_diamond_conflict(&name, &current_version, &version, &pkg.requesters)?;
                if chosen != current_version {
                    let mut new_entry = resolve_one(&name, &chosen)?;
                    new_entry.set_direct(false);
                    let new_children = read_manifest_deps(&name)?;
                    pkg.entry = new_entry;
                    for (child_name, child_version) in new_children {
                        queue.push_back((child_name, child_version, name.clone()));
                    }
                }
                notes.push(ResolutionNote::DiamondAutoResolved {
                    name: name.clone(),
                    chosen_version: chosen,
                    requesters: pkg.requesters.clone(),
                });
            }
        }
    }

    let pruned: Vec<String> = existing_lock
        .entries
        .keys()
        .filter(|name| !resolved.contains_key(name.as_str()))
        .cloned()
        .collect();

    let mut new_lock = LockFile::new();
    for (name, pkg) in resolved {
        new_lock.entries.insert(name, pkg.entry);
    }

    Ok((new_lock, pruned, notes))
}

/// Pure, install-free reachability BFS used by `cmd_remove` to decide
/// whether a package can be safely deleted. Uses whatever manifests are
/// already on disk — never touches the network, never installs anything, and
/// silently treats an unreadable manifest as "no further dependencies".
fn compute_reachable(direct_deps: &BTreeMap<String, String>) -> BTreeSet<String> {
    let mut reachable = BTreeSet::new();
    let mut queue: VecDeque<String> = direct_deps.keys().cloned().collect();
    while let Some(name) = queue.pop_front() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        for child in read_dest_manifest_deps(&name).unwrap_or_default().keys() {
            if !reachable.contains(child) {
                queue.push_back(child.clone());
            }
        }
    }
    reachable
}

fn print_resolution_note(note: &ResolutionNote) {
    match note {
        ResolutionNote::DirectOverride {
            name,
            direct_version,
            requested_by,
            requested_version,
        } => {
            println!(
                "  note: your direct pin of '{name}' ({direct_version}) overrides a transitive \
                 request for {requested_version} from '{requested_by}'"
            );
        }
        ResolutionNote::DiamondAutoResolved {
            name,
            chosen_version,
            requesters,
        } => {
            let who: Vec<String> = requesters.iter().map(|(w, v)| format!("{w}@{v}")).collect();
            println!(
                "  note: '{name}' resolved to {chosen_version} across multiple requesters ({})",
                who.join(", ")
            );
        }
    }
}

/// `--locked` orphan check: only direct-flagged lock entries must appear in
/// `sema.toml [deps]`. Transitive entries (`direct == false`) legitimately
/// have no top-level `[deps]` key — they were pulled in by something that
/// does. Pulled out as its own pure function so it's testable without
/// actually installing anything (git/network are otherwise unavoidable once
/// `cmd_install` reaches the install loop).
fn check_locked_orphans(deps: &BTreeMap<String, String>, lock: &LockFile) -> Result<(), String> {
    for (name, entry) in &lock.entries {
        if entry.is_direct() && !deps.contains_key(name) {
            return Err(format!(
                "'{name}' is in sema.lock but not in sema.toml. \
                 Run `sema pkg install` (without --locked) to update the lock file."
            ));
        }
    }
    Ok(())
}

pub fn cmd_install(locked: bool) -> Result<(), String> {
    let toml_path = Path::new("sema.toml");
    if !toml_path.exists() {
        return Err("No sema.toml found in current directory. Run `sema pkg init` first.".into());
    }

    let content =
        std::fs::read_to_string(toml_path).map_err(|e| format!("Failed to read sema.toml: {e}"))?;
    let doc: toml::Value =
        toml::from_str(&content).map_err(|e| format!("Failed to parse sema.toml: {e}"))?;

    let deps_table = match doc.get("deps").and_then(|d| d.as_table()) {
        Some(table) => table,
        None => {
            println!("No [deps] found in sema.toml, nothing to install.");
            return Ok(());
        }
    };
    let deps = parse_deps_table(deps_table)?;

    let lock = read_lock_file()?;

    if locked {
        let lock = lock.as_ref().ok_or(
            "sema.lock not found. Cannot use --locked without a lock file.\n\
             Run `sema pkg install` first to generate sema.lock.",
        )?;

        // Every direct dep in sema.toml must have a matching lock entry.
        for (name, version) in &deps {
            match lock.entries.get(name) {
                None => {
                    return Err(format!(
                        "Dep '{name}' is in sema.toml but not in sema.lock. \
                         Run `sema pkg install` (without --locked) to update the lock file."
                    ));
                }
                Some(entry) => {
                    let lock_ver = entry.requested();
                    if lock_ver != version {
                        return Err(format!(
                            "Dep '{name}' version mismatch: sema.toml has \"{version}\" \
                             but sema.lock has \"{lock_ver}\". \
                             Run `sema pkg install` (without --locked) to update the lock file."
                        ));
                    }
                }
            }
        }

        check_locked_orphans(&deps, lock)?;

        // Trust the lock's full flattened closure as-is (no network
        // re-derivation of the graph) and install every entry in it.
        for (name, entry) in &lock.entries {
            println!("Installing {name} (locked)...");
            install_from_lock_entry(name, entry)?;
        }

        return Ok(());
    }

    let existing_lock = lock.unwrap_or_else(LockFile::new);

    let (new_lock, pruned, notes) = resolve_dependency_graph(
        &deps,
        &existing_lock,
        &mut |name, version| {
            println!("Installing {name}...");
            resolve_and_install_one(name, version)
        },
        &mut |name, entry| {
            println!("Installing {name} (locked)...");
            install_from_lock_entry(name, entry)
        },
        &read_dest_manifest_deps,
    )?;

    for name in &pruned {
        eprintln!("Warning: '{name}' is no longer required, removing from sema.lock");
    }
    for note in &notes {
        print_resolution_note(note);
    }

    write_lock_file(&new_lock)?;
    println!("✓ Updated sema.lock");

    Ok(())
}

/// Is `name` a direct dependency? Checked against the lock file's `direct`
/// flag when a lock entry exists; a package not yet locked at all is treated
/// as direct (matches pre-transitive-resolution behavior: everything used to
/// be direct).
fn is_direct_dep(name: &str) -> bool {
    read_lock_file()
        .ok()
        .flatten()
        .and_then(|l| l.entries.get(name).map(|e| e.is_direct()))
        .unwrap_or(true)
}

pub fn cmd_update(name: Option<&str>) -> Result<(), String> {
    let pkg_dir = packages_dir();

    if let Some(name) = name {
        if !is_direct_dep(name) {
            return Err(format!(
                "'{name}' is a transitive dependency — its version is controlled by whichever \
                 package requires it. Add \"{name}\" to your own sema.toml [deps] to pin or \
                 override it directly."
            ));
        }
        let dir = find_package_dir(&pkg_dir, name).ok_or_else(|| {
            format!("Package '{name}' not found. Run `sema pkg list` to see installed packages.")
        })?;
        update_single_package(&pkg_dir, &dir)?;
    } else {
        let packages = find_all_packages(&pkg_dir);
        if packages.is_empty() {
            println!("No packages installed.");
            return Ok(());
        }
        // Only direct deps get force-bumped to latest here; transitive
        // packages' versions are dictated by whatever their direct-dep
        // requesters need, re-derived below by re-resolving the graph.
        for dir in &packages {
            let rel = dir.strip_prefix(&pkg_dir).unwrap_or(dir);
            let rel_str = rel.to_string_lossy().to_string();
            if !is_direct_dep(&rel_str) {
                continue;
            }
            if let Err(e) = update_single_package(&pkg_dir, dir) {
                eprintln!("✗ Failed to update {}: {e}", rel.display());
            }
        }
    }

    // Re-resolve the whole graph so transitive requirements introduced or
    // dropped by the version bump(s) above get picked up or pruned.
    if Path::new("sema.toml").exists() {
        cmd_install(false)?;
    }

    Ok(())
}

/// Update a single package to its latest version/ref. Only ever called for
/// packages already confirmed direct (see `cmd_update`) — every lock write
/// here is therefore `direct: true`, and `sema.toml` is only rewritten when
/// the package is already listed there, so a transitive-only package can
/// never get silently promoted into the root manifest.
fn update_single_package(pkg_dir: &Path, dir: &Path) -> Result<(), String> {
    let rel = dir.strip_prefix(pkg_dir).unwrap_or(dir);
    let rel_str = rel.display().to_string();

    if let Some(meta) = read_pkg_meta(dir) {
        // Registry package — check for newer version
        let name = meta
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&rel_str)
            .to_string();
        let current_ver = meta
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let registry = meta
            .get("registry")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_REGISTRY);

        let info = registry_package_info(&name, registry)?;
        let latest =
            latest_version(&info).ok_or_else(|| format!("No versions found for '{name}'"))?;

        if latest == current_ver {
            println!("  {} already at latest ({current_ver})", rel.display());
        } else {
            println!("  Updating {} {current_ver} → {latest}...", rel.display());
            let checksum = registry_install(&name, &latest, registry)?;
            println!("✓ Updated {} → {latest}", rel.display());

            // Only rewrite sema.toml when this package is already a direct
            // dependency there.
            if read_dep_ref_from_toml(&name).is_some() {
                let toml_path = Path::new("sema.toml");
                let _ = add_dep_to_toml(toml_path, &name, &latest);
            }

            let _ = update_lock_entry(
                &name,
                LockEntry::Registry {
                    version: latest,
                    registry: registry.to_string(),
                    checksum,
                    direct: true,
                },
            );
        }
    } else if dir.join(".git").is_dir() {
        // Git package — fetch and update to latest on the tracking ref
        run_git(Some(dir), &["fetch", "origin"])?;

        // Read the tracking ref from sema.toml (needed if HEAD is detached after --locked install)
        let tracking_ref = read_dep_ref_from_toml(&rel_str);
        if let Some(ref git_ref) = tracking_ref {
            // Checkout the branch/tag first so pull works
            let _ = run_git(Some(dir), &["checkout", git_ref]);
        }

        run_git(Some(dir), &["pull"])?;
        let current_ref = tracking_ref.unwrap_or_else(|| current_git_ref(dir));
        let commit =
            run_git(Some(dir), &["rev-parse", "HEAD"]).unwrap_or_else(|_| "unknown".to_string());
        println!("✓ Updated {} → {current_ref}", rel.display());

        let _ = update_lock_entry(
            &rel_str,
            LockEntry::Git {
                git_ref: current_ref,
                commit,
                direct: true,
            },
        );
    } else {
        println!("  {} — unknown source, skipping", rel.display());
    }

    Ok(())
}

pub fn cmd_remove(name: &str) -> Result<(), String> {
    let pkg_dir = packages_dir();
    let dir = find_package_dir(&pkg_dir, name).ok_or_else(|| {
        format!("Package '{name}' not found. Run `sema pkg list` to see installed packages.")
    })?;

    let rel_path = dir
        .strip_prefix(&pkg_dir)
        .unwrap_or(&dir)
        .to_string_lossy()
        .to_string();

    // Remove from sema.toml [deps] FIRST so reachability below is computed
    // against the post-removal set of direct dependencies — deleting the
    // on-disk directory before checking this would leave a dangling lock
    // entry if something else still needs the package transitively.
    let toml_path = Path::new("sema.toml");
    let mut removed_from_toml = false;
    if toml_path.exists() {
        match remove_dep_from_toml(toml_path, &rel_path) {
            Ok(true) => removed_from_toml = true,
            Ok(false) => {}
            Err(e) => eprintln!("Warning: could not update sema.toml: {e}"),
        }
    }

    let remaining_deps: BTreeMap<String, String> = toml_path
        .exists()
        .then(|| std::fs::read_to_string(toml_path).ok())
        .flatten()
        .and_then(|c| toml::from_str::<toml::Value>(&c).ok())
        .and_then(|doc| {
            doc.get("deps")
                .and_then(|d| d.as_table())
                .and_then(|t| parse_deps_table(t).ok())
        })
        .unwrap_or_default();
    let reachable = compute_reachable(&remaining_deps);

    if reachable.contains(&rel_path) {
        // Still required transitively by something else — keep it on disk
        // and demote its lock entry instead of deleting anything.
        if let Ok(Some(mut lock)) = read_lock_file() {
            if let Some(entry) = lock.entries.get_mut(&rel_path) {
                entry.set_direct(false);
                let _ = write_lock_file(&lock);
            }
        }
        if removed_from_toml {
            println!("✓ Removed {rel_path} from sema.toml");
        }
        println!("Kept {rel_path} installed — still required transitively by another dependency.");
        return Ok(());
    }

    std::fs::remove_dir_all(&dir).map_err(|e| format!("Failed to remove package: {e}"))?;
    println!("✓ Removed {rel_path}");

    // Clean up empty parent directories
    let mut parent = dir.parent();
    while let Some(p) = parent {
        if p == pkg_dir {
            break;
        }
        if p.read_dir().map(|mut d| d.next().is_none()).unwrap_or(true) {
            let _ = std::fs::remove_dir(p);
            parent = p.parent();
        } else {
            break;
        }
    }

    if removed_from_toml {
        println!("✓ Removed {rel_path} from sema.toml");
    }

    // Remove from sema.lock if present
    match remove_lock_entry(&rel_path) {
        Ok(true) => println!("✓ Removed {rel_path} from sema.lock"),
        Ok(false) => {}
        Err(e) => eprintln!("Warning: could not update sema.lock: {e}"),
    }

    Ok(())
}

/// Read a dep's version/ref from sema.toml, if present.
fn read_dep_ref_from_toml(name: &str) -> Option<String> {
    let content = std::fs::read_to_string("sema.toml").ok()?;
    let doc: toml::Value = toml::from_str(&content).ok()?;
    doc.get("deps")?.get(name)?.as_str().map(|s| s.to_string())
}

/// Add or update a dep entry in a sema.toml file.
/// Returns true if the entry was added or updated, false if already up-to-date.
fn add_dep_to_toml(toml_path: &Path, pkg_path: &str, git_ref: &str) -> Result<bool, String> {
    let content =
        std::fs::read_to_string(toml_path).map_err(|e| format!("Failed to read sema.toml: {e}"))?;
    let mut doc: toml_edit::DocumentMut = content
        .parse()
        .map_err(|e| format!("Failed to parse sema.toml: {e}"))?;

    if doc.get("deps").is_none() {
        doc["deps"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let deps = doc["deps"]
        .as_table_mut()
        .ok_or("sema.toml [deps] is not a table")?;

    if let Some(existing) = deps.get(pkg_path).and_then(|v| v.as_str()) {
        if existing == git_ref {
            return Ok(false);
        }
    }

    deps[pkg_path] = toml_edit::value(git_ref);

    std::fs::write(toml_path, doc.to_string())
        .map_err(|e| format!("Failed to write sema.toml: {e}"))?;
    Ok(true)
}

/// Remove a dep entry from a sema.toml file by package path.
/// Returns true if a matching entry was found and removed.
fn remove_dep_from_toml(toml_path: &Path, pkg_path: &str) -> Result<bool, String> {
    let content =
        std::fs::read_to_string(toml_path).map_err(|e| format!("Failed to read sema.toml: {e}"))?;
    let mut doc: toml_edit::DocumentMut = content
        .parse()
        .map_err(|e| format!("Failed to parse sema.toml: {e}"))?;

    let removed = if let Some(deps) = doc.get_mut("deps").and_then(|d| d.as_table_mut()) {
        deps.remove(pkg_path).is_some()
    } else {
        false
    };

    if removed {
        std::fs::write(toml_path, doc.to_string())
            .map_err(|e| format!("Failed to write sema.toml: {e}"))?;
    }

    Ok(removed)
}

pub fn cmd_list() -> Result<(), String> {
    let pkg_dir = packages_dir();
    let packages = find_all_packages(&pkg_dir);

    if packages.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    let lock = read_lock_file().ok().flatten();

    for dir in &packages {
        let rel = dir.strip_prefix(&pkg_dir).unwrap_or(dir);
        let rel_str = rel.to_string_lossy().to_string();
        let kind = lock
            .as_ref()
            .and_then(|l| l.entries.get(&rel_str))
            .map(|e| {
                if e.is_direct() {
                    "direct"
                } else {
                    "transitive"
                }
            });

        if let Some(meta) = read_pkg_meta(dir) {
            let version = meta.get("version").and_then(|v| v.as_str()).unwrap_or("?");
            let source = meta
                .get("registry")
                .and_then(|v| v.as_str())
                .unwrap_or("registry");
            match kind {
                Some(k) => println!("  {} ({version}) [{source}, {k}]", rel.display()),
                None => println!("  {} ({version}) [{source}]", rel.display()),
            }
        } else {
            let current = current_git_ref(dir);
            match kind {
                Some(k) => println!("  {} ({current}) [git, {k}]", rel.display()),
                None => println!("  {} ({current}) [git]", rel.display()),
            }
        }
    }

    Ok(())
}

pub fn cmd_init() -> Result<(), String> {
    let toml_path = Path::new("sema.toml");
    if toml_path.exists() {
        return Err("sema.toml already exists in current directory.".into());
    }

    let project_name = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "my-project".to_string());

    let content = format!(
        r#"[package]
name = "{project_name}"
version = "0.1.0"
description = ""
entrypoint = "package.sema"

[deps]
"#
    );

    std::fs::write(toml_path, content).map_err(|e| format!("Failed to write sema.toml: {e}"))?;
    println!("✓ Created sema.toml");

    let entry_path = Path::new("package.sema");
    if !entry_path.exists() {
        let entry_content =
            ";; package entrypoint — all top-level definitions are available to importers\n";
        std::fs::write(entry_path, entry_content)
            .map_err(|e| format!("Failed to write package.sema: {e}"))?;
        println!("✓ Created package.sema");
    }

    Ok(())
}

pub fn cmd_login(token: Option<&str>, registry: &str) -> Result<(), String> {
    let token = match token {
        Some(t) => t.to_string(),
        None => {
            eprint!("API token: ");
            let mut input = String::new();
            std::io::stdin()
                .read_line(&mut input)
                .map_err(|e| format!("Failed to read input: {e}"))?;
            let input = input.trim().to_string();
            if input.is_empty() {
                return Err("Token cannot be empty".into());
            }
            input
        }
    };

    if !token.starts_with("sema_pat_") {
        return Err("Invalid token format. Tokens start with 'sema_pat_'".into());
    }

    let creds_path = credentials_path();
    if let Some(parent) = creds_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {e}"))?;
    }

    let content = format!("[registry]\ntoken = \"{token}\"\nurl = \"{registry}\"\n");
    std::fs::write(&creds_path, &content)
        .map_err(|e| format!("Failed to write credentials: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&creds_path, perms);
    }

    println!("✓ Login saved to {}", creds_path.display());
    println!("  Registry: {registry}");
    Ok(())
}

pub fn cmd_logout() -> Result<(), String> {
    let creds_path = credentials_path();
    if creds_path.exists() {
        std::fs::remove_file(&creds_path)
            .map_err(|e| format!("Failed to remove credentials: {e}"))?;
        println!("✓ Logged out (removed {})", creds_path.display());
    } else {
        println!("Not logged in.");
    }
    Ok(())
}

fn credentials_path() -> PathBuf {
    sema_core::home::sema_home().join("credentials.toml")
}

/// Read the stored API token from credentials file, if any.
pub fn read_token() -> Option<String> {
    let path = credentials_path();
    let content = std::fs::read_to_string(path).ok()?;
    let doc: toml::Value = toml::from_str(&content).ok()?;
    doc.get("registry")?
        .get("token")?
        .as_str()
        .map(|s| s.to_string())
}

pub fn cmd_config(key: Option<&str>, value: Option<&str>) -> Result<(), String> {
    match (key, value) {
        // Show all config
        (None, _) => {
            let url = read_registry_url();
            let has_token = read_token().is_some();
            println!("registry.url = {url}");
            println!(
                "registry.token = {}",
                if has_token { "(set)" } else { "(not set)" }
            );
            println!("\nCredentials file: {}", credentials_path().display());
            Ok(())
        }
        // Get a specific key
        (Some(key), None) => match key {
            "registry.url" | "registry" => {
                println!("{}", read_registry_url());
                Ok(())
            }
            _ => Err(format!(
                "Unknown config key: {key}\nAvailable: registry.url"
            )),
        },
        // Set a key
        (Some(key), Some(value)) => match key {
            "registry.url" | "registry" => {
                set_registry_url(value)?;
                println!("✓ Default registry set to {value}");
                Ok(())
            }
            _ => Err(format!(
                "Unknown config key: {key}\nAvailable: registry.url"
            )),
        },
    }
}

/// Update the registry URL in credentials.toml, preserving the token if present.
fn set_registry_url(url: &str) -> Result<(), String> {
    let creds_path = credentials_path();
    let token = read_token().unwrap_or_default();

    if let Some(parent) = creds_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {e}"))?;
    }

    let content = if token.is_empty() {
        format!("[registry]\nurl = \"{url}\"\n")
    } else {
        format!("[registry]\ntoken = \"{token}\"\nurl = \"{url}\"\n")
    };

    std::fs::write(&creds_path, &content)
        .map_err(|e| format!("Failed to write credentials: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&creds_path, perms);
    }

    Ok(())
}

/// Read the stored registry URL from credentials file, or return default.
fn read_registry_url() -> String {
    let path = credentials_path();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return DEFAULT_REGISTRY.to_string(),
    };
    let doc: toml::Value = match toml::from_str(&content) {
        Ok(d) => d,
        Err(_) => return DEFAULT_REGISTRY.to_string(),
    };
    doc.get("registry")
        .and_then(|r| r.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or(DEFAULT_REGISTRY)
        .to_string()
}

/// Resolve the effective registry URL: explicit flag > env var > credentials > default.
fn effective_registry(flag: Option<&str>) -> String {
    match flag {
        Some(url) => url.to_string(),
        None => std::env::var("SEMA_REGISTRY_URL").unwrap_or_else(|_| read_registry_url()),
    }
}

/// Determine if a package spec looks like a git URL (has a hostname).
///
/// Heuristic: if the first path segment contains a dot, it's a hostname.
/// e.g., "github.com/user/repo" → true, "http-helpers" → false.
fn is_git_spec(spec: &str) -> bool {
    // Strip @ref suffix for the check
    let path = spec.split('@').next().unwrap_or(spec);
    path.split('/')
        .next()
        .map(|first| first.contains('.'))
        .unwrap_or(false)
}

/// Write registry package metadata to a `.sema-pkg.json` file.
fn write_pkg_meta(
    dir: &Path,
    name: &str,
    version: &str,
    registry: &str,
    checksum: &str,
) -> Result<(), String> {
    let meta = serde_json::json!({
        "source": "registry",
        "name": name,
        "version": version,
        "registry": registry,
        "checksum": checksum,
    });
    let path = dir.join(PKG_META_FILE);
    std::fs::write(&path, serde_json::to_string_pretty(&meta).unwrap())
        .map_err(|e| format!("Failed to write package metadata: {e}"))
}

/// Read registry package metadata from `.sema-pkg.json`, if present.
fn read_pkg_meta(dir: &Path) -> Option<serde_json::Value> {
    let path = dir.join(PKG_META_FILE);
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Create a tarball of the given directory, excluding .git and target.
fn create_tarball(dir: &str) -> Result<Vec<u8>, String> {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let dir_path = Path::new(dir);
    let enc = GzEncoder::new(Vec::new(), Compression::default());
    let mut ar = tar::Builder::new(enc);

    let mut files = Vec::new();
    collect_files_for_tar(dir_path, &mut files)?;

    for file in &files {
        let rel = file.strip_prefix(dir_path).unwrap_or(file);
        ar.append_path_with_name(file, rel)
            .map_err(|e| format!("Failed to add {}: {e}", file.display()))?;
    }

    let enc = ar
        .into_inner()
        .map_err(|e| format!("Failed to finalize tar: {e}"))?;
    enc.finish()
        .map_err(|e| format!("Failed to finalize gzip: {e}"))
}

fn collect_files_for_tar(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("directory entry error: {e}"))?;
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name == ".git" || name == "target" {
                continue;
            }
        }
        if path.is_dir() {
            collect_files_for_tar(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

/// Extract a tarball into a destination directory.
/// Rejects path traversal, absolute paths, and symlinks.
fn extract_tarball(data: &[u8], dest: &Path) -> Result<(), String> {
    use flate2::read::GzDecoder;

    std::fs::create_dir_all(dest).map_err(|e| format!("Failed to create directory: {e}"))?;

    let decoder = GzDecoder::new(data);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive
        .entries()
        .map_err(|e| format!("Invalid tar archive: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("Invalid tar entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("Invalid entry path: {e}"))?
            .into_owned();

        if path.is_absolute() {
            return Err(format!("Tar entry has absolute path: {}", path.display()));
        }

        for component in path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(format!(
                    "Tar entry contains path traversal: {}",
                    path.display()
                ));
            }
        }

        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(format!(
                "Tar entry is a symlink/hardlink (rejected): {}",
                path.display()
            ));
        }

        let full_path = dest.join(&path);
        if entry_type.is_dir() {
            std::fs::create_dir_all(&full_path)
                .map_err(|e| format!("Failed to create dir {}: {e}", full_path.display()))?;
        } else if entry_type.is_file() {
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent dir: {e}"))?;
            }
            entry
                .unpack(&full_path)
                .map_err(|e| format!("Failed to extract {}: {e}", path.display()))?;
        }
    }

    Ok(())
}

/// Atomically install an extracted tarball at `dest`.
///
/// BIN-4: extracting straight into `dest` (after `remove_dir_all(dest)`) means a
/// mid-extract failure leaves the destination neither old nor new — a corrupt
/// half-install that later `install` calls may treat as complete. Instead we
/// extract into a sibling temp dir, write metadata there, then atomically rename
/// it into place (replacing any existing install). The temp dir is cleaned up on
/// any failure so a broken tarball never corrupts the package store.
fn install_tarball_atomic(
    tarball: &[u8],
    dest: &Path,
    name: &str,
    version: &str,
    registry_url: &str,
    checksum: &str,
) -> Result<(), String> {
    // Place the temp dir as a sibling of `dest` so the rename stays on the same
    // filesystem (and is therefore atomic).
    let parent = dest
        .parent()
        .ok_or_else(|| format!("Invalid install destination: {}", dest.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create packages directory: {e}"))?;
    let leaf = dest
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "pkg".to_string());
    let temp_dir = parent.join(format!(".{leaf}.tmp-{}", std::process::id()));

    // A stale temp dir from a previously killed install would block extraction.
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // Run the fallible work against the temp dir; clean it up on any failure so
    // a broken tarball never leaves a corrupt tree behind.
    let build = || -> Result<(), String> {
        extract_tarball(tarball, &temp_dir)?;
        write_pkg_meta(&temp_dir, name, version, registry_url, checksum)?;
        Ok(())
    };
    if let Err(e) = build() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(e);
    }

    // Swap into place: remove the old install (if any), then rename. The window
    // between remove and rename is tiny and, unlike extraction, cannot fail
    // partway through leaving a corrupt tree.
    if dest.exists() {
        if let Err(e) = std::fs::remove_dir_all(dest) {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(format!("Failed to remove old package: {e}"));
        }
    }
    std::fs::rename(&temp_dir, dest).map_err(|e| {
        let _ = std::fs::remove_dir_all(&temp_dir);
        format!("Failed to finalize package install: {e}")
    })?;

    Ok(())
}

/// Download and install a package from the registry. Returns the checksum.
fn registry_install(name: &str, version: &str, registry_url: &str) -> Result<String, String> {
    // Registry-only names skip the git-spec validator, so guard here before the
    // name is used as a path component (`pkg_dir.join(name)`): a name like
    // `../../etc/cron.d` would otherwise escape `~/.sema/packages/`.
    validate_package_spec(name).map_err(|e| e.to_string())?;
    let (tarball, checksum) = registry_download(name, version, registry_url)?;

    // Extract to packages dir (atomically — see install_tarball_atomic / BIN-4).
    let pkg_dir = packages_dir();
    let dest = pkg_dir.join(name);
    install_tarball_atomic(&tarball, &dest, name, version, registry_url, &checksum)?;

    Ok(checksum)
}

/// Download a registry package and return (tarball_bytes, checksum).
/// The registry may return a redirect (e.g. to GitHub for meta-registry packages),
/// so we explicitly follow redirects.
fn registry_download(
    name: &str,
    version: &str,
    registry_url: &str,
) -> Result<(Vec<u8>, String), String> {
    let token = read_token();
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;
    let base = registry_url.trim_end_matches('/');

    let url = format!("{base}/api/v1/packages/{name}/{version}/download");
    let mut req = client.get(&url);
    if let Some(ref t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }

    let resp = req.send().map_err(|e| format!("Download failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body: serde_json::Value = resp.json().unwrap_or_default();
        let error = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("Download failed ({status}): {error}"));
    }

    // BIN-3: cap the download so a malicious/broken registry can't OOM us by
    // streaming an unbounded body. Read at most MAX_PKG_SIZE + 1 and reject if
    // the cap is exceeded.
    const MAX_PKG_SIZE: u64 = 200 * 1024 * 1024;
    use std::io::Read;
    let mut tarball = Vec::new();
    resp.take(MAX_PKG_SIZE + 1)
        .read_to_end(&mut tarball)
        .map_err(|e| format!("Failed to read response: {e}"))?;
    if tarball.len() as u64 > MAX_PKG_SIZE {
        return Err(format!(
            "Download too large: package exceeds the {} MiB limit",
            MAX_PKG_SIZE / (1024 * 1024)
        ));
    }

    use sha2::Digest;
    let checksum = format!("{:x}", sha2::Sha256::digest(&tarball));

    Ok((tarball, checksum))
}

/// Install a registry package with checksum verification (for locked installs).
fn registry_install_locked(
    name: &str,
    version: &str,
    registry_url: &str,
    expected_checksum: &str,
) -> Result<(), String> {
    validate_package_spec(name).map_err(|e| e.to_string())?;
    let (tarball, checksum) = registry_download(name, version, registry_url)?;

    if checksum != expected_checksum {
        return Err(format!(
            "Lock integrity error for {name}@{version}: expected checksum {expected_checksum}, got {checksum}"
        ));
    }

    let pkg_dir = packages_dir();
    let dest = pkg_dir.join(name);
    install_tarball_atomic(&tarball, &dest, name, version, registry_url, &checksum)?;

    println!("✓ Installed {name}@{version} (locked)");
    Ok(())
}

/// Fetch package info from the registry.
fn registry_package_info(name: &str, registry_url: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::blocking::Client::new();
    let base = registry_url.trim_end_matches('/');
    let url = format!("{base}/api/v1/packages/{name}");

    let mut req = client.get(&url);
    if let Some(t) = read_token() {
        req = req.header("Authorization", format!("Bearer {t}"));
    }

    let resp = req.send().map_err(|e| format!("Request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body: serde_json::Value = resp.json().unwrap_or_default();
        let error = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("Failed to fetch package ({status}): {error}"));
    }

    resp.json()
        .map_err(|e| format!("Failed to parse response: {e}"))
}

/// Get the latest non-yanked version from a package info response.
fn latest_version(info: &serde_json::Value) -> Option<String> {
    info.get("versions")?
        .as_array()?
        .iter()
        .filter(|v| !v.get("yanked").and_then(|y| y.as_bool()).unwrap_or(false))
        .filter_map(|v| v.get("version").and_then(|s| s.as_str()))
        .next()
        .map(|s| s.to_string())
}

fn validate_version(version: &str) -> Result<semver::Version, String> {
    semver::Version::parse(version).map_err(|_| {
        format!("Invalid semver version: {version} (expected X.Y.Z[-prerelease][+build])")
    })
}

pub fn cmd_publish(registry: Option<&str>) -> Result<(), String> {
    let toml_path = Path::new("sema.toml");
    if !toml_path.exists() {
        return Err("No sema.toml found. Run `sema pkg init` first.".into());
    }

    let content =
        std::fs::read_to_string(toml_path).map_err(|e| format!("Failed to read sema.toml: {e}"))?;
    let doc: toml::Value =
        toml::from_str(&content).map_err(|e| format!("Failed to parse sema.toml: {e}"))?;

    let pkg = doc
        .get("package")
        .ok_or("sema.toml missing [package] section")?;
    let name = pkg
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("sema.toml [package] missing 'name'")?;
    let version = pkg
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or("sema.toml [package] missing 'version'")?;

    validate_version(version)?;

    let token = read_token().ok_or("Not logged in. Run `sema pkg login --token <token>` first.")?;
    let registry_url = effective_registry(registry);
    let base = registry_url.trim_end_matches('/');

    // Create tarball
    println!("Packaging...");
    let tarball = create_tarball(".")?;
    println!("  {} bytes compressed", tarball.len());

    // Build metadata
    let metadata = serde_json::json!({
        "description": pkg.get("description").and_then(|v| v.as_str()).unwrap_or(""),
        "repository_url": pkg.get("repository").and_then(|v| v.as_str()),
        "sema_version_req": pkg.get("sema_version_req").and_then(|v| v.as_str()),
    });

    // Upload
    let url = format!("{base}/api/v1/packages/{name}/{version}");
    let form = reqwest::blocking::multipart::Form::new()
        .part(
            "tarball",
            reqwest::blocking::multipart::Part::bytes(tarball)
                .file_name("package.tar.gz")
                .mime_str("application/gzip")
                .unwrap(),
        )
        .part(
            "metadata",
            reqwest::blocking::multipart::Part::text(metadata.to_string()),
        );

    let client = reqwest::blocking::Client::new();
    let resp = client
        .put(&url)
        .header("Authorization", format!("Bearer {token}"))
        .multipart(form)
        .send()
        .map_err(|e| format!("Upload failed: {e}"))?;

    if resp.status().is_success() {
        let body: serde_json::Value = resp
            .json()
            .map_err(|e| format!("Failed to parse response: {e}"))?;
        let checksum = body
            .get("checksum")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let size = body.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
        println!("✓ Published {name}@{version} ({size} bytes, sha256:{checksum})");
        Ok(())
    } else {
        let status = resp.status();
        let body: serde_json::Value = resp.json().unwrap_or_default();
        let error = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        Err(format!("Publish failed ({status}): {error}"))
    }
}

pub fn cmd_search(query: &str, registry: Option<&str>) -> Result<(), String> {
    let registry_url = effective_registry(registry);
    let base = registry_url.trim_end_matches('/');
    let url = format!("{base}/api/v1/search?q={}", urlencoded(query));

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("Search failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!("Search failed ({status})"));
    }

    let body: serde_json::Value = resp
        .json()
        .map_err(|e| format!("Failed to parse response: {e}"))?;

    let packages = body
        .get("packages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if packages.is_empty() {
        println!("No packages found for '{query}'.");
        return Ok(());
    }

    let total = body.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
    println!(
        "Found {total} package{}:\n",
        if total == 1 { "" } else { "s" }
    );

    for pkg in &packages {
        let name = pkg.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let desc = pkg
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if desc.is_empty() {
            println!("  {name}");
        } else {
            println!("  {name} — {desc}");
        }
    }

    Ok(())
}

pub fn cmd_yank(spec: &str, registry: Option<&str>) -> Result<(), String> {
    let (name, version) = spec
        .rsplit_once('@')
        .ok_or("Expected format: <package>@<version> (e.g., my-package@0.1.0)")?;

    let token = read_token().ok_or("Not logged in. Run `sema pkg login --token <token>` first.")?;
    let registry_url = effective_registry(registry);
    let base = registry_url.trim_end_matches('/');

    let url = format!("{base}/api/v1/packages/{name}/{version}/yank");
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .map_err(|e| format!("Yank failed: {e}"))?;

    if resp.status().is_success() {
        println!("✓ Yanked {name}@{version}");
        Ok(())
    } else {
        let status = resp.status();
        let body: serde_json::Value = resp.json().unwrap_or_default();
        let error = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        Err(format!("Yank failed ({status}): {error}"))
    }
}

pub fn cmd_info(name: &str, registry: Option<&str>) -> Result<(), String> {
    let registry_url = effective_registry(registry);
    let info = registry_package_info(name, &registry_url)?;

    let pkg = info.get("package").unwrap_or(&info);
    let pkg_name = pkg.get("name").and_then(|v| v.as_str()).unwrap_or(name);
    let desc = pkg
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let repo = pkg
        .get("repository_url")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    println!("{pkg_name}");
    if !desc.is_empty() {
        println!("  {desc}");
    }
    if !repo.is_empty() {
        println!("  repo: {repo}");
    }

    let owners = info
        .get("owners")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if !owners.is_empty() {
        let names: Vec<&str> = owners.iter().filter_map(|v| v.as_str()).collect();
        println!("  owners: {}", names.join(", "));
    }

    let versions = info
        .get("versions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if versions.is_empty() {
        println!("\n  No versions published.");
    } else {
        println!("\n  Versions:");
        for v in &versions {
            let ver = v.get("version").and_then(|s| s.as_str()).unwrap_or("?");
            let yanked = v.get("yanked").and_then(|b| b.as_bool()).unwrap_or(false);
            let size = v.get("size_bytes").and_then(|n| n.as_i64()).unwrap_or(0);
            let published = v.get("published_at").and_then(|s| s.as_str()).unwrap_or("");
            let yank_mark = if yanked { " (yanked)" } else { "" };
            println!("    {ver} — {size} bytes, {published}{yank_mark}");
        }
    }

    Ok(())
}

/// Minimal URL encoding for query parameters.
fn urlencoded(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' => result.push_str("%20"),
            '&' => result.push_str("%26"),
            '=' => result.push_str("%3D"),
            '#' => result.push_str("%23"),
            '+' => result.push_str("%2B"),
            '%' => result.push_str("%25"),
            _ => result.push(c),
        }
    }
    result
}

// ── Lock file (sema.lock) ──────────────────────────────────────────────

const LOCK_FILE: &str = "sema.lock";

#[derive(Debug, Clone)]
enum LockEntry {
    Git {
        git_ref: String,
        commit: String,
        /// True if this package is a direct dependency in the project's own
        /// `sema.toml [deps]`; false if it was pulled in transitively by
        /// another package's manifest. Defaults to `true` when absent on
        /// read, since every lock file predating transitive resolution only
        /// ever contained direct entries.
        direct: bool,
    },
    Registry {
        version: String,
        registry: String,
        checksum: String,
        direct: bool,
    },
}

impl LockEntry {
    fn is_direct(&self) -> bool {
        match self {
            LockEntry::Git { direct, .. } | LockEntry::Registry { direct, .. } => *direct,
        }
    }

    /// The pinned version (registry) or ref (git) — the identity-independent
    /// part of what was requested for this package.
    fn requested(&self) -> &str {
        match self {
            LockEntry::Git { git_ref, .. } => git_ref,
            LockEntry::Registry { version, .. } => version,
        }
    }

    fn set_direct(&mut self, value: bool) {
        match self {
            LockEntry::Git { direct, .. } | LockEntry::Registry { direct, .. } => *direct = value,
        }
    }
}

#[derive(Debug, Clone)]
struct LockFile {
    entries: BTreeMap<String, LockEntry>,
}

impl LockFile {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

/// Read and parse `sema.lock`. Returns `Ok(None)` if the file doesn't exist,
/// `Err` for parse/format errors (so callers get actionable messages).
fn read_lock_file() -> Result<Option<LockFile>, String> {
    let path = Path::new(LOCK_FILE);
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Failed to read sema.lock: {e}")),
    };

    let doc: toml::Value =
        toml::from_str(&content).map_err(|e| format!("Failed to parse sema.lock: {e}"))?;

    let version = doc
        .get("lock_version")
        .and_then(|v| v.as_integer())
        .ok_or("sema.lock missing 'lock_version' field")?;
    if version != 1 {
        return Err(format!(
            "Unsupported sema.lock version {version} (expected 1). \
             Regenerate with `sema pkg install`."
        ));
    }

    let empty_table = toml::map::Map::new();
    let packages = doc
        .get("packages")
        .and_then(|v| v.as_table())
        .unwrap_or(&empty_table);

    let mut entries = BTreeMap::new();

    for (name, value) in packages {
        let table = value
            .as_table()
            .ok_or_else(|| format!("sema.lock: package '{name}' is not a table"))?;
        let source = table
            .get("source")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("sema.lock: package '{name}' missing 'source' field"))?;
        // Absent on every lock file written before transitive resolution
        // existed — those files only ever contained direct entries.
        let direct = table
            .get("direct")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let entry = match source {
            "git" => {
                let git_ref = table.get("ref").and_then(|v| v.as_str()).ok_or_else(|| {
                    format!("sema.lock: git package '{name}' missing 'ref' field")
                })?;
                let commit = table
                    .get("commit")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        format!("sema.lock: git package '{name}' missing 'commit' field")
                    })?;
                LockEntry::Git {
                    git_ref: git_ref.to_string(),
                    commit: commit.to_string(),
                    direct,
                }
            }
            "registry" => {
                let version = table
                    .get("version")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        format!("sema.lock: registry package '{name}' missing 'version' field")
                    })?;
                let registry = table
                    .get("registry")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        format!("sema.lock: registry package '{name}' missing 'registry' field")
                    })?;
                let checksum = table
                    .get("checksum")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        format!("sema.lock: registry package '{name}' missing 'checksum' field")
                    })?;
                LockEntry::Registry {
                    version: version.to_string(),
                    registry: registry.to_string(),
                    checksum: checksum.to_string(),
                    direct,
                }
            }
            other => {
                return Err(format!(
                    "sema.lock: package '{name}' has unknown source '{other}'"
                ));
            }
        };

        entries.insert(name.clone(), entry);
    }

    Ok(Some(LockFile { entries }))
}

fn write_lock_file(lock: &LockFile) -> Result<(), String> {
    let mut doc = toml_edit::DocumentMut::new();
    doc.decor_mut()
        .set_prefix("# sema.lock — auto-generated, do not edit manually\n");
    doc["lock_version"] = toml_edit::value(1i64);

    let mut packages = toml_edit::Table::new();
    packages.set_implicit(true);

    for (name, entry) in &lock.entries {
        let mut table = toml_edit::Table::new();
        match entry {
            LockEntry::Git {
                git_ref,
                commit,
                direct,
            } => {
                table["source"] = toml_edit::value("git");
                table["ref"] = toml_edit::value(git_ref.as_str());
                table["commit"] = toml_edit::value(commit.as_str());
                table["direct"] = toml_edit::value(*direct);
            }
            LockEntry::Registry {
                version,
                registry,
                checksum,
                direct,
            } => {
                table["source"] = toml_edit::value("registry");
                table["version"] = toml_edit::value(version.as_str());
                table["registry"] = toml_edit::value(registry.as_str());
                table["checksum"] = toml_edit::value(checksum.as_str());
                table["direct"] = toml_edit::value(*direct);
            }
        }
        packages[name] = toml_edit::Item::Table(table);
    }

    doc["packages"] = toml_edit::Item::Table(packages);

    std::fs::write(LOCK_FILE, doc.to_string())
        .map_err(|e| format!("Failed to write sema.lock: {e}"))
}

fn update_lock_entry(name: &str, entry: LockEntry) -> Result<(), String> {
    let mut lock = read_lock_file()?.unwrap_or_else(LockFile::new);
    lock.entries.insert(name.to_string(), entry);
    write_lock_file(&lock)
}

fn remove_lock_entry(name: &str) -> Result<bool, String> {
    let mut lock = match read_lock_file()? {
        Some(l) => l,
        None => return Ok(false),
    };
    let removed = lock.entries.remove(name).is_some();
    if removed {
        if lock.entries.is_empty() {
            let _ = std::fs::remove_file(LOCK_FILE);
        } else {
            write_lock_file(&lock)?;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use std::io::Write;

    #[test]
    fn registry_install_rejects_path_traversal_name() {
        // Must fail at validation, before any network/filesystem work, so the
        // name never reaches `pkg_dir.join(name)`.
        for bad in ["../../etc/cron.d", "..", "/etc/passwd", "a/../../b"] {
            let err = registry_install(bad, "1.0.0", "http://localhost:0").unwrap_err();
            assert!(
                err.contains("path traversal")
                    || err.contains("absolute paths")
                    || err.contains("invalid package spec"),
                "name {bad:?} should be rejected, got: {err}"
            );
        }
    }

    #[test]
    fn registry_install_locked_rejects_path_traversal_name() {
        // The lock-file restore path must guard the name too, before any
        // network/filesystem work reaches `pkg_dir.join(name)`.
        for bad in ["../../etc/cron.d", "..", "/etc/passwd", "a/../../b"] {
            let err = registry_install_locked(bad, "1.0.0", "http://localhost:0", "deadbeef")
                .unwrap_err();
            assert!(
                err.contains("path traversal")
                    || err.contains("absolute paths")
                    || err.contains("invalid package spec"),
                "name {bad:?} should be rejected, got: {err}"
            );
        }
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sema-pkg-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn add_dep_preserves_comments() {
        let dir = tmpdir("add-comments");
        let toml_path = dir.join("sema.toml");
        let input = "# Project config\n[package]\nname = \"my-app\"\n\n# Dependencies\n[deps]\n\"github.com/test/foo\" = \"v1.0.0\"\n";
        fs::write(&toml_path, input).unwrap();

        add_dep_to_toml(&toml_path, "github.com/test/bar", "v2.0.0").unwrap();

        let output = fs::read_to_string(&toml_path).unwrap();
        let doc: toml_edit::DocumentMut = output.parse().unwrap();

        let deps = doc["deps"].as_table().expect("deps table must exist");
        assert_eq!(
            deps.get("github.com/test/foo").and_then(|v| v.as_str()),
            Some("v1.0.0"),
            "existing dep preserved"
        );
        assert_eq!(
            deps.get("github.com/test/bar").and_then(|v| v.as_str()),
            Some("v2.0.0"),
            "new dep added"
        );

        // Comments survived
        assert!(output.contains("# Project config"), "top comment lost");
        assert!(output.contains("# Dependencies"), "deps comment lost");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_dep_creates_deps_section_if_missing() {
        let dir = tmpdir("add-no-deps");
        let toml_path = dir.join("sema.toml");
        fs::write(&toml_path, "[package]\nname = \"bare\"\n").unwrap();

        let changed = add_dep_to_toml(&toml_path, "github.com/a/b", "v1.0.0").unwrap();
        assert!(changed);

        let output = fs::read_to_string(&toml_path).unwrap();
        let doc: toml_edit::DocumentMut = output.parse().unwrap();
        assert_eq!(doc["deps"]["github.com/a/b"].as_str(), Some("v1.0.0"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_dep_updates_existing_version() {
        let dir = tmpdir("add-update");
        let toml_path = dir.join("sema.toml");
        fs::write(&toml_path, "[deps]\n\"github.com/a/b\" = \"v1.0.0\"\n").unwrap();

        let changed = add_dep_to_toml(&toml_path, "github.com/a/b", "v2.0.0").unwrap();
        assert!(changed);

        let output = fs::read_to_string(&toml_path).unwrap();
        let doc: toml_edit::DocumentMut = output.parse().unwrap();
        assert_eq!(doc["deps"]["github.com/a/b"].as_str(), Some("v2.0.0"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_dep_returns_false_if_already_set() {
        let dir = tmpdir("add-noop");
        let toml_path = dir.join("sema.toml");
        fs::write(&toml_path, "[deps]\n\"github.com/a/b\" = \"v1.0.0\"\n").unwrap();

        let changed = add_dep_to_toml(&toml_path, "github.com/a/b", "v1.0.0").unwrap();
        assert!(!changed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_dep_removes_entry_preserves_others() {
        let dir = tmpdir("remove");
        let toml_path = dir.join("sema.toml");
        fs::write(
            &toml_path,
            "[deps]\n\"github.com/a/b\" = \"v1.0.0\"\n\"github.com/c/d\" = \"v2.0.0\"\n",
        )
        .unwrap();

        let removed = remove_dep_from_toml(&toml_path, "github.com/a/b").unwrap();
        assert!(removed);

        let output = fs::read_to_string(&toml_path).unwrap();
        let doc: toml_edit::DocumentMut = output.parse().unwrap();
        let deps = doc["deps"].as_table().unwrap();
        assert!(
            deps.get("github.com/a/b").is_none(),
            "removed dep should be gone"
        );
        assert_eq!(
            deps.get("github.com/c/d").and_then(|v| v.as_str()),
            Some("v2.0.0"),
            "other dep preserved"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_dep_returns_false_if_not_found() {
        let dir = tmpdir("remove-noop");
        let toml_path = dir.join("sema.toml");
        fs::write(&toml_path, "[deps]\n\"github.com/a/b\" = \"v1.0.0\"\n").unwrap();

        let removed = remove_dep_from_toml(&toml_path, "github.com/x/y").unwrap();
        assert!(!removed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_all_packages_empty() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let packages = find_all_packages(&tmp);
        assert!(packages.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_find_all_packages_finds_package_sema() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-find2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);

        let pkg = tmp.join("github.com/user/repo");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("package.sema"), "(define x 1)").unwrap();

        let packages = find_all_packages(&tmp);
        assert_eq!(packages.len(), 1);
        assert!(packages[0].ends_with("github.com/user/repo"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_find_all_packages_finds_sema_toml() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-find3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);

        let pkg = tmp.join("github.com/user/lib");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("sema.toml"), "[package]\nname = \"lib\"\n").unwrap();

        let packages = find_all_packages(&tmp);
        assert_eq!(packages.len(), 1);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_find_package_dir_by_full_path() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-find4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);

        let pkg = tmp.join("github.com/user/repo");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("package.sema"), "(define x 1)").unwrap();

        let found = find_package_dir(&tmp, "github.com/user/repo");
        assert!(found.is_some());
        assert!(found.unwrap().ends_with("github.com/user/repo"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_find_package_dir_by_name() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-find5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);

        let pkg = tmp.join("github.com/user/mylib");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("package.sema"), "(define x 1)").unwrap();

        let found = find_package_dir(&tmp, "mylib");
        assert!(found.is_some());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_find_package_dir_not_found() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-find6-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let found = find_package_dir(&tmp, "nonexistent");
        assert!(found.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_run_git_checkout_ref_not_as_path() {
        // Verify that `git checkout <ref>` (without `--`) correctly switches
        // to a branch/tag. With `--`, git would interpret the ref as a file path.
        let tmp = std::env::temp_dir().join(format!("sema-pkg-checkout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Init a repo and create a branch
        run_git(Some(&tmp), &["init"]).unwrap();
        run_git(Some(&tmp), &["config", "user.email", "test@test.com"]).unwrap();
        run_git(Some(&tmp), &["config", "user.name", "Test"]).unwrap();
        run_git(Some(&tmp), &["checkout", "-b", "main"]).unwrap();
        std::fs::write(tmp.join("file.txt"), "hello").unwrap();
        run_git(Some(&tmp), &["add", "."]).unwrap();
        run_git(Some(&tmp), &["commit", "-m", "init"]).unwrap();
        run_git(Some(&tmp), &["branch", "test-branch"]).unwrap();

        // Checkout should succeed for a branch name
        let result = run_git(Some(&tmp), &["checkout", "test-branch"]);
        assert!(result.is_ok(), "checkout branch failed: {result:?}");

        // Verify we're on the right branch
        let branch = run_git(Some(&tmp), &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(branch, "test-branch");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_cmd_add_rejects_traversal() {
        let result = cmd_add("github.com/../../etc/passwd", None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("path traversal"), "got: {err}");
    }

    #[test]
    fn test_cmd_add_rejects_scheme() {
        let result = cmd_add("https://github.com/user/repo", None);
        assert!(result.is_err());
    }

    #[test]
    #[serial]
    fn test_cmd_init_creates_sema_toml() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Run cmd_init in the temp directory
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = cmd_init();
        std::env::set_current_dir(original_dir).unwrap();

        assert!(result.is_ok());
        let content = std::fs::read_to_string(tmp.join("sema.toml")).unwrap();
        assert!(
            content.contains("[package]"),
            "should use [package], got: {content}"
        );
        assert!(
            content.contains("version = \"0.1.0\""),
            "should have version"
        );
        assert!(
            content.contains("description = \"\""),
            "should have description"
        );
        assert!(
            content.contains("entrypoint = \"package.sema\""),
            "should have entrypoint"
        );
        assert!(content.contains("[deps]"), "should have [deps] section");

        let entry = std::fs::read_to_string(tmp.join("package.sema")).unwrap();
        assert!(
            entry.contains("entrypoint"),
            "package.sema should have comment"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_add_dep_to_toml_new_entry() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-adddep1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let toml_path = tmp.join("sema.toml");
        std::fs::write(
            &toml_path,
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\n\n[deps]\n",
        )
        .unwrap();

        let added = add_dep_to_toml(&toml_path, "github.com/user/repo", "v1.0.0").unwrap();
        assert!(added, "should have added the dep");

        let content = std::fs::read_to_string(&toml_path).unwrap();
        assert!(
            content.contains("\"github.com/user/repo\" = \"v1.0.0\""),
            "dep should be present: {content}"
        );
        assert!(content.contains("[package]"), "package section preserved");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_add_dep_to_toml_updates_existing() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-adddep2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let toml_path = tmp.join("sema.toml");
        std::fs::write(
            &toml_path,
            "[deps]\n\"github.com/user/repo\" = \"v1.0.0\"\n",
        )
        .unwrap();

        let added = add_dep_to_toml(&toml_path, "github.com/user/repo", "v2.0.0").unwrap();
        assert!(added, "should have updated the dep");

        let content = std::fs::read_to_string(&toml_path).unwrap();
        assert!(
            content.contains("\"github.com/user/repo\" = \"v2.0.0\""),
            "dep should be updated: {content}"
        );
        assert!(
            !content.contains("v1.0.0"),
            "old version should be gone: {content}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_add_dep_to_toml_already_up_to_date() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-adddep3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let toml_path = tmp.join("sema.toml");
        std::fs::write(
            &toml_path,
            "[deps]\n\"github.com/user/repo\" = \"v1.0.0\"\n",
        )
        .unwrap();

        let added = add_dep_to_toml(&toml_path, "github.com/user/repo", "v1.0.0").unwrap();
        assert!(!added, "should not change anything");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_add_dep_to_toml_no_deps_section() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-adddep4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let toml_path = tmp.join("sema.toml");
        std::fs::write(&toml_path, "[package]\nname = \"test\"\n").unwrap();

        let added = add_dep_to_toml(&toml_path, "github.com/user/repo", "main").unwrap();
        assert!(added, "should have added dep and section");

        let content = std::fs::read_to_string(&toml_path).unwrap();
        assert!(content.contains("[deps]"), "should have [deps]: {content}");
        assert!(
            content.contains("\"github.com/user/repo\" = \"main\""),
            "dep should be present: {content}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_add_dep_to_toml_preserves_existing_deps() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-adddep5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let toml_path = tmp.join("sema.toml");
        std::fs::write(
            &toml_path,
            "[deps]\n\"github.com/user/existing\" = \"v1.0.0\"\n",
        )
        .unwrap();

        let added = add_dep_to_toml(&toml_path, "github.com/user/new", "v2.0.0").unwrap();
        assert!(added);

        let content = std::fs::read_to_string(&toml_path).unwrap();
        assert!(
            content.contains("\"github.com/user/existing\" = \"v1.0.0\""),
            "existing dep preserved: {content}"
        );
        assert!(
            content.contains("\"github.com/user/new\" = \"v2.0.0\""),
            "new dep added: {content}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_remove_dep_from_toml_quoted_key() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-rmdep1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let toml_path = tmp.join("sema.toml");
        std::fs::write(
            &toml_path,
            r#"[package]
name = "myproject"
version = "0.1.0"

[deps]
"github.com/user/repo" = "v1.0.0"
"github.com/user/other" = "main"
"#,
        )
        .unwrap();

        let removed = remove_dep_from_toml(&toml_path, "github.com/user/repo").unwrap();
        assert!(removed, "should have removed the dep");

        let content = std::fs::read_to_string(&toml_path).unwrap();
        assert!(
            !content.contains("github.com/user/repo"),
            "removed dep should be gone: {content}"
        );
        assert!(
            content.contains("github.com/user/other"),
            "other dep should remain: {content}"
        );
        assert!(content.contains("[package]"), "package section preserved");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_remove_dep_from_toml_quoted_key_with_slashes() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-rmdep2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let toml_path = tmp.join("sema.toml");
        std::fs::write(
            &toml_path,
            "[deps]\n\"github.com/user/repo\" = \"v1.0.0\"\n",
        )
        .unwrap();

        let removed = remove_dep_from_toml(&toml_path, "github.com/user/repo").unwrap();
        assert!(removed);

        let content = std::fs::read_to_string(&toml_path).unwrap();
        assert!(
            !content.contains("github.com/user/repo"),
            "dep should be gone: {content}"
        );
        assert!(content.contains("[deps]"), "section header preserved");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_remove_dep_from_toml_not_found() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-rmdep3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let toml_path = tmp.join("sema.toml");
        std::fs::write(
            &toml_path,
            "[deps]\n\"github.com/user/other\" = \"v1.0.0\"\n",
        )
        .unwrap();

        let removed = remove_dep_from_toml(&toml_path, "github.com/user/repo").unwrap();
        assert!(!removed, "should not have removed anything");

        // File should be unchanged
        let content = std::fs::read_to_string(&toml_path).unwrap();
        assert!(content.contains("github.com/user/other"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_remove_dep_from_toml_no_deps_section() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-rmdep4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let toml_path = tmp.join("sema.toml");
        std::fs::write(&toml_path, "[package]\nname = \"test\"\n").unwrap();

        let removed = remove_dep_from_toml(&toml_path, "github.com/user/repo").unwrap();
        assert!(!removed);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_remove_dep_from_toml_preserves_comments() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-rmdep5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let toml_path = tmp.join("sema.toml");
        std::fs::write(
            &toml_path,
            r#"[package]
name = "myproject"

[deps]
# My core dependency
"github.com/user/core" = "v2.0.0"
"github.com/user/remove-me" = "v1.0.0"
"#,
        )
        .unwrap();

        let removed = remove_dep_from_toml(&toml_path, "github.com/user/remove-me").unwrap();
        assert!(removed);

        let content = std::fs::read_to_string(&toml_path).unwrap();
        assert!(
            content.contains("# My core dependency"),
            "comment should be preserved: {content}"
        );
        assert!(content.contains("github.com/user/core"));
        assert!(!content.contains("remove-me"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[serial]
    fn test_cmd_init_rejects_existing() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-init2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("sema.toml"), "existing").unwrap();

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = cmd_init();
        std::env::set_current_dir(original_dir).unwrap();

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_is_git_spec() {
        assert!(is_git_spec("github.com/user/repo"));
        assert!(is_git_spec("github.com/user/repo@v1.0"));
        assert!(is_git_spec("gitlab.com/org/lib@main"));
        assert!(!is_git_spec("http-helpers"));
        assert!(!is_git_spec("http-helpers@1.0.0"));
        assert!(!is_git_spec("my-package"));
    }

    #[test]
    fn test_write_and_read_pkg_meta() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-meta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        write_pkg_meta(
            &tmp,
            "test-pkg",
            "1.0.0",
            "https://registry.example.com",
            "abc123",
        )
        .unwrap();

        let meta = read_pkg_meta(&tmp);
        assert!(meta.is_some());
        let meta = meta.unwrap();
        assert_eq!(meta["source"], "registry");
        assert_eq!(meta["name"], "test-pkg");
        assert_eq!(meta["version"], "1.0.0");
        assert_eq!(meta["registry"], "https://registry.example.com");
        assert_eq!(meta["checksum"], "abc123");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_read_pkg_meta_missing() {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-meta2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        assert!(read_pkg_meta(&tmp).is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_urlencoded() {
        assert_eq!(urlencoded("hello world"), "hello%20world");
        assert_eq!(urlencoded("a&b=c"), "a%26b%3Dc");
        assert_eq!(urlencoded("simple"), "simple");
    }

    #[test]
    fn test_cmd_yank_requires_at_sign() {
        let result = cmd_yank("my-package", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Expected format"));
    }

    #[test]
    fn validate_version_accepts_standard() {
        assert!(validate_version("1.0.0").is_ok());
        assert!(validate_version("0.1.0").is_ok());
        assert!(validate_version("10.20.30").is_ok());
    }

    #[test]
    fn validate_version_accepts_prerelease() {
        assert!(validate_version("1.0.0-alpha.1").is_ok());
        assert!(validate_version("1.0.0-beta").is_ok());
        assert!(validate_version("1.0.0-rc.1").is_ok());
    }

    #[test]
    fn validate_version_accepts_build_metadata() {
        assert!(validate_version("1.0.0+build.123").is_ok());
        assert!(validate_version("1.0.0-alpha+001").is_ok());
    }

    #[test]
    fn validate_version_rejects_invalid() {
        assert!(validate_version("not-a-version").is_err());
        assert!(validate_version("1.0").is_err());
        assert!(validate_version("").is_err());
        assert!(validate_version("v1.0.0").is_err());
    }

    /// Helper: build a tar.gz with a raw path written directly into the header,
    /// bypassing the `tar` crate's own path validation.
    fn make_malicious_tarball(raw_path: &str, data: &[u8]) -> Vec<u8> {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let mut gz = GzEncoder::new(Vec::new(), Compression::default());

        // Build a 512-byte tar header manually
        let mut header_block = [0u8; 512];
        let path_bytes = raw_path.as_bytes();
        header_block[..path_bytes.len()].copy_from_slice(path_bytes);
        // mode (octal ASCII at offset 100, 8 bytes)
        header_block[100..107].copy_from_slice(b"0000644");
        // size (octal ASCII at offset 124, 12 bytes)
        let size_str = format!("{:011o}", data.len());
        header_block[124..135].copy_from_slice(size_str.as_bytes());
        // typeflag '0' = regular file at offset 156
        header_block[156] = b'0';
        // magic "ustar\0" at offset 257
        header_block[257..263].copy_from_slice(b"ustar\0");
        // version "00" at offset 263
        header_block[263..265].copy_from_slice(b"00");
        // Compute checksum (sum of all bytes, treating checksum field as spaces)
        header_block[148..156].copy_from_slice(b"        ");
        let cksum: u32 = header_block.iter().map(|&b| b as u32).sum();
        let cksum_str = format!("{:06o}\0 ", cksum);
        header_block[148..156].copy_from_slice(cksum_str.as_bytes());

        gz.write_all(&header_block).unwrap();
        gz.write_all(data).unwrap();
        // Pad to 512-byte boundary
        let padding = 512 - (data.len() % 512);
        if padding < 512 {
            gz.write_all(&vec![0u8; padding]).unwrap();
        }
        // Two zero blocks = end of archive
        gz.write_all(&[0u8; 1024]).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn extract_tarball_rejects_path_traversal() {
        let malicious = make_malicious_tarball("../pwned.txt", b"pwned!");

        let dir = tmpdir("traversal");
        let dest = dir.join("extracted");
        let parent_file = dir.join("pwned.txt");

        let result = extract_tarball(&malicious, &dest);
        assert!(result.is_err(), "path traversal should be rejected");
        assert!(
            !parent_file.exists(),
            "file must NOT be written outside dest"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_tarball_rejects_absolute_paths() {
        let malicious = make_malicious_tarball("/tmp/pwned.txt", b"pwned!");

        let dir = tmpdir("abs-path");
        let dest = dir.join("extracted");

        let result = extract_tarball(&malicious, &dest);
        assert!(result.is_err(), "absolute paths should be rejected");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_tarball_extracts_valid_archive() {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut ar = tar::Builder::new(&mut gz);
            let data = b"(define x 42)";
            let mut header = tar::Header::new_gnu();
            header.set_path("package.sema").unwrap();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append(&header, &data[..]).unwrap();
            ar.finish().unwrap();
        }
        let tarball = gz.finish().unwrap();

        let dir = tmpdir("valid-tar");
        let dest = dir.join("extracted");

        extract_tarball(&tarball, &dest).unwrap();
        let content = fs::read_to_string(dest.join("package.sema")).unwrap();
        assert_eq!(content, "(define x 42)");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_tarball_rejects_symlinks() {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut ar = tar::Builder::new(&mut gz);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_path("evil-link").unwrap();
            header.set_link_name("/etc/passwd").unwrap();
            header.set_size(0);
            header.set_cksum();
            ar.append(&header, &[][..]).unwrap();
            ar.finish().unwrap();
        }
        let malicious = gz.finish().unwrap();

        let dir = tmpdir("symlink");
        let dest = dir.join("extracted");

        let result = extract_tarball(&malicious, &dest);
        assert!(result.is_err(), "symlinks should be rejected");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_tarball_handles_nested_directories() {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut ar = tar::Builder::new(&mut gz);

            let data = b"(define deep 1)";
            let mut header = tar::Header::new_gnu();
            header.set_path("src/lib/deep.sema").unwrap();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append(&header, &data[..]).unwrap();
            ar.finish().unwrap();
        }
        let tarball = gz.finish().unwrap();

        let dir = tmpdir("nested-dirs");
        let dest = dir.join("extracted");

        extract_tarball(&tarball, &dest).unwrap();
        let content = fs::read_to_string(dest.join("src/lib/deep.sema")).unwrap();
        assert_eq!(content, "(define deep 1)");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Build a minimal valid gzipped tarball containing `package.sema`.
    fn make_pkg_tarball(content: &str) -> Vec<u8> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut ar = tar::Builder::new(&mut gz);
            let data = content.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_path("package.sema").unwrap();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append(&header, data).unwrap();
            ar.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    // BIN-4: a failed extraction must not corrupt an existing install, and a
    // successful one must atomically replace it.
    #[test]
    fn install_tarball_atomic_preserves_install_on_failure() {
        let dir = tmpdir("atomic-fail");
        let dest = dir.join("mypkg");

        // Seed an existing, valid install.
        let good = make_pkg_tarball("(define x 1)");
        install_tarball_atomic(&good, &dest, "mypkg", "1.0.0", "http://r", "abc").unwrap();
        assert_eq!(
            fs::read_to_string(dest.join("package.sema")).unwrap(),
            "(define x 1)"
        );

        // A corrupt tarball must fail without disturbing the prior install.
        let garbage = b"not a gzip tarball at all";
        let err = install_tarball_atomic(garbage, &dest, "mypkg", "2.0.0", "http://r", "def")
            .unwrap_err();
        assert!(!err.is_empty());
        assert!(dest.exists(), "old install must survive a failed update");
        assert_eq!(
            fs::read_to_string(dest.join("package.sema")).unwrap(),
            "(define x 1)",
            "old contents must be intact after a failed update"
        );

        // No leftover temp dir should remain.
        let leftover: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".mypkg.tmp-"))
            .collect();
        assert!(
            leftover.is_empty(),
            "temp dir must be cleaned up on failure"
        );

        // A valid update must atomically replace the contents.
        let good2 = make_pkg_tarball("(define x 2)");
        install_tarball_atomic(&good2, &dest, "mypkg", "2.0.0", "http://r", "def").unwrap();
        assert_eq!(
            fs::read_to_string(dest.join("package.sema")).unwrap(),
            "(define x 2)"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // ── Lock file tests ───────────────────────────────────────────────

    /// Helper to chdir into a temp directory and restore on drop.
    struct TestDir {
        prev: PathBuf,
    }

    impl TestDir {
        fn new(dir: &Path) -> Self {
            let prev = std::env::current_dir().unwrap();
            std::env::set_current_dir(dir).unwrap();
            Self { prev }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.prev);
        }
    }

    // ── Round-trip tests ──────────────────────────────────────────────

    #[test]
    #[serial]
    fn lock_file_round_trip_git() {
        let dir = tmpdir("lock-git");
        let _guard = TestDir::new(&dir);

        let mut lock = LockFile::new();
        lock.entries.insert(
            "github.com/user/repo".to_string(),
            LockEntry::Git {
                git_ref: "main".to_string(),
                commit: "abc123def456".to_string(),
                direct: true,
            },
        );

        write_lock_file(&lock).unwrap();

        let content = fs::read_to_string(dir.join(LOCK_FILE)).unwrap();
        assert!(content.contains("lock_version = 1"));
        assert!(content.contains("[packages.\"github.com/user/repo\"]"));
        assert!(content.contains("source = \"git\""));
        assert!(content.contains("commit = \"abc123def456\""));
        assert!(content.contains("direct = true"));

        let loaded = read_lock_file().unwrap().unwrap();
        assert_eq!(loaded.entries.len(), 1);
        match &loaded.entries["github.com/user/repo"] {
            LockEntry::Git {
                git_ref,
                commit,
                direct,
            } => {
                assert_eq!(git_ref, "main");
                assert_eq!(commit, "abc123def456");
                assert!(direct);
            }
            _ => panic!("Expected git entry"),
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_round_trip_registry() {
        let dir = tmpdir("lock-registry");
        let _guard = TestDir::new(&dir);

        let mut lock = LockFile::new();
        lock.entries.insert(
            "http-helpers".to_string(),
            LockEntry::Registry {
                version: "1.2.0".to_string(),
                registry: "https://pkg.sema-lang.com".to_string(),
                checksum: "deadbeef".to_string(),
                direct: true,
            },
        );

        write_lock_file(&lock).unwrap();

        let loaded = read_lock_file().unwrap().unwrap();
        assert_eq!(loaded.entries.len(), 1);
        match &loaded.entries["http-helpers"] {
            LockEntry::Registry {
                version,
                registry,
                checksum,
                direct,
            } => {
                assert_eq!(version, "1.2.0");
                assert_eq!(registry, "https://pkg.sema-lang.com");
                assert_eq!(checksum, "deadbeef");
                assert!(direct);
            }
            _ => panic!("Expected registry entry"),
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_round_trip_git_transitive() {
        let dir = tmpdir("lock-git-transitive");
        let _guard = TestDir::new(&dir);

        let mut lock = LockFile::new();
        lock.entries.insert(
            "github.com/user/repo".to_string(),
            LockEntry::Git {
                git_ref: "main".to_string(),
                commit: "abc123def456".to_string(),
                direct: false,
            },
        );

        write_lock_file(&lock).unwrap();

        let content = fs::read_to_string(dir.join(LOCK_FILE)).unwrap();
        assert!(content.contains("direct = false"));

        let loaded = read_lock_file().unwrap().unwrap();
        assert!(!loaded.entries["github.com/user/repo"].is_direct());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_round_trip_registry_transitive() {
        let dir = tmpdir("lock-registry-transitive");
        let _guard = TestDir::new(&dir);

        let mut lock = LockFile::new();
        lock.entries.insert(
            "http-helpers".to_string(),
            LockEntry::Registry {
                version: "1.2.0".to_string(),
                registry: "https://pkg.sema-lang.com".to_string(),
                checksum: "deadbeef".to_string(),
                direct: false,
            },
        );

        write_lock_file(&lock).unwrap();

        let loaded = read_lock_file().unwrap().unwrap();
        assert!(!loaded.entries["http-helpers"].is_direct());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_direct_field_defaults_true_when_absent() {
        let dir = tmpdir("lock-direct-default");
        let _guard = TestDir::new(&dir);

        fs::write(
            LOCK_FILE,
            "lock_version = 1\n\n\
             [packages.foo]\n\
             source = \"registry\"\n\
             version = \"1.0.0\"\n\
             registry = \"http://localhost\"\n\
             checksum = \"aaa\"\n",
        )
        .unwrap();

        let loaded = read_lock_file().unwrap().unwrap();
        assert!(
            loaded.entries["foo"].is_direct(),
            "missing 'direct' field must default to true for backward compatibility"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_mixed_entries() {
        let dir = tmpdir("lock-mixed");
        let _guard = TestDir::new(&dir);

        let mut lock = LockFile::new();
        lock.entries.insert(
            "github.com/user/repo".to_string(),
            LockEntry::Git {
                git_ref: "v1.0".to_string(),
                commit: "aaa111".to_string(),
                direct: true,
            },
        );
        lock.entries.insert(
            "my-pkg".to_string(),
            LockEntry::Registry {
                version: "0.1.0".to_string(),
                registry: "https://pkg.sema-lang.com".to_string(),
                checksum: "bbb222".to_string(),
                direct: true,
            },
        );

        write_lock_file(&lock).unwrap();

        let loaded = read_lock_file().unwrap().unwrap();
        assert_eq!(loaded.entries.len(), 2);
        assert!(matches!(
            &loaded.entries["github.com/user/repo"],
            LockEntry::Git { .. }
        ));
        assert!(matches!(
            &loaded.entries["my-pkg"],
            LockEntry::Registry { .. }
        ));

        let _ = fs::remove_dir_all(&dir);
    }

    // ── Update / remove entry tests ───────────────────────────────────

    #[test]
    #[serial]
    fn lock_file_update_entry() {
        let dir = tmpdir("lock-update");
        let _guard = TestDir::new(&dir);

        update_lock_entry(
            "my-pkg",
            LockEntry::Registry {
                version: "1.0.0".to_string(),
                registry: "https://pkg.sema-lang.com".to_string(),
                checksum: "aaa".to_string(),
                direct: true,
            },
        )
        .unwrap();

        let lock = read_lock_file().unwrap().unwrap();
        assert_eq!(lock.entries.len(), 1);

        // Update same entry
        update_lock_entry(
            "my-pkg",
            LockEntry::Registry {
                version: "2.0.0".to_string(),
                registry: "https://pkg.sema-lang.com".to_string(),
                checksum: "bbb".to_string(),
                direct: true,
            },
        )
        .unwrap();

        let lock = read_lock_file().unwrap().unwrap();
        assert_eq!(lock.entries.len(), 1);
        match &lock.entries["my-pkg"] {
            LockEntry::Registry { version, .. } => assert_eq!(version, "2.0.0"),
            _ => panic!("Expected registry entry"),
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_remove_entry() {
        let dir = tmpdir("lock-remove");
        let _guard = TestDir::new(&dir);

        update_lock_entry(
            "pkg-a",
            LockEntry::Registry {
                version: "1.0.0".to_string(),
                registry: "https://pkg.sema-lang.com".to_string(),
                checksum: "aaa".to_string(),
                direct: true,
            },
        )
        .unwrap();
        update_lock_entry(
            "pkg-b",
            LockEntry::Registry {
                version: "2.0.0".to_string(),
                registry: "https://pkg.sema-lang.com".to_string(),
                checksum: "bbb".to_string(),
                direct: true,
            },
        )
        .unwrap();

        let removed = remove_lock_entry("pkg-a").unwrap();
        assert!(removed);

        let lock = read_lock_file().unwrap().unwrap();
        assert_eq!(lock.entries.len(), 1);
        assert!(!lock.entries.contains_key("pkg-a"));
        assert!(lock.entries.contains_key("pkg-b"));

        // Remove last entry — file should be deleted
        let removed = remove_lock_entry("pkg-b").unwrap();
        assert!(removed);
        assert!(!Path::new(LOCK_FILE).exists());

        // Remove from nonexistent lock
        let removed = remove_lock_entry("no-such-pkg").unwrap();
        assert!(!removed);

        let _ = fs::remove_dir_all(&dir);
    }

    // ── Missing / malformed lock file tests ───────────────────────────

    #[test]
    #[serial]
    fn lock_file_missing_returns_none() {
        let dir = tmpdir("lock-missing");
        let _guard = TestDir::new(&dir);

        let result = read_lock_file().unwrap();
        assert!(result.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_invalid_toml_returns_error() {
        let dir = tmpdir("lock-invalid-toml");
        let _guard = TestDir::new(&dir);

        fs::write(LOCK_FILE, "this is not valid toml {{{}").unwrap();
        let err = read_lock_file().unwrap_err();
        assert!(err.contains("Failed to parse"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_missing_lock_version() {
        let dir = tmpdir("lock-no-version");
        let _guard = TestDir::new(&dir);

        fs::write(LOCK_FILE, "[packages]\n").unwrap();
        let err = read_lock_file().unwrap_err();
        assert!(err.contains("lock_version"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_unsupported_version() {
        let dir = tmpdir("lock-bad-version");
        let _guard = TestDir::new(&dir);

        fs::write(LOCK_FILE, "lock_version = 99\n[packages]\n").unwrap();
        let err = read_lock_file().unwrap_err();
        assert!(err.contains("Unsupported"), "got: {err}");
        assert!(err.contains("99"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_missing_packages_table_is_empty() {
        let dir = tmpdir("lock-no-packages");
        let _guard = TestDir::new(&dir);

        // lock_version present but no [packages] → treated as empty lock
        fs::write(LOCK_FILE, "lock_version = 1\n").unwrap();
        let lock = read_lock_file().unwrap().unwrap();
        assert!(lock.entries.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_malformed_entry_missing_field() {
        let dir = tmpdir("lock-bad-entry");
        let _guard = TestDir::new(&dir);

        // Registry entry missing 'checksum'
        fs::write(
            LOCK_FILE,
            "lock_version = 1\n\n\
             [packages.good]\n\
             source = \"registry\"\n\
             version = \"1.0.0\"\n\
             registry = \"http://example\"\n\
             checksum = \"abc\"\n\n\
             [packages.bad]\n\
             source = \"registry\"\n\
             version = \"1.0.0\"\n\
             registry = \"http://example\"\n",
        )
        .unwrap();

        let err = read_lock_file().unwrap_err();
        assert!(
            err.contains("bad"),
            "error should mention package name, got: {err}"
        );
        assert!(
            err.contains("checksum"),
            "error should mention missing field, got: {err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_malformed_entry_missing_source() {
        let dir = tmpdir("lock-no-source");
        let _guard = TestDir::new(&dir);

        fs::write(
            LOCK_FILE,
            "lock_version = 1\n\n\
             [packages.broken]\n\
             version = \"1.0.0\"\n",
        )
        .unwrap();

        let err = read_lock_file().unwrap_err();
        assert!(err.contains("broken"), "got: {err}");
        assert!(err.contains("source"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_unknown_source_type() {
        let dir = tmpdir("lock-unknown-source");
        let _guard = TestDir::new(&dir);

        fs::write(
            LOCK_FILE,
            "lock_version = 1\n\n\
             [packages.weird]\n\
             source = \"ftp\"\n",
        )
        .unwrap();

        let err = read_lock_file().unwrap_err();
        assert!(err.contains("ftp"), "got: {err}");
        assert!(err.contains("weird"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_git_entry_missing_commit() {
        let dir = tmpdir("lock-git-no-commit");
        let _guard = TestDir::new(&dir);

        fs::write(
            LOCK_FILE,
            "lock_version = 1\n\n\
             [packages.\"github.com/user/repo\"]\n\
             source = \"git\"\n\
             ref = \"main\"\n",
        )
        .unwrap();

        let err = read_lock_file().unwrap_err();
        assert!(err.contains("commit"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    // ── check_locked_orphans tests (pure, no install/network) ─────────

    #[test]
    fn check_locked_orphans_allows_transitive_entry_missing_from_toml() {
        let deps: BTreeMap<String, String> = [("foo".to_string(), "1.0.0".to_string())].into();
        let mut lock = LockFile::new();
        lock.entries
            .insert("foo".to_string(), registry_entry("1.0.0", true));
        lock.entries
            .insert("bar".to_string(), registry_entry("2.0.0", false));

        assert!(check_locked_orphans(&deps, &lock).is_ok());
    }

    #[test]
    fn check_locked_orphans_rejects_direct_entry_missing_from_toml() {
        let deps: BTreeMap<String, String> = [("foo".to_string(), "1.0.0".to_string())].into();
        let mut lock = LockFile::new();
        lock.entries
            .insert("foo".to_string(), registry_entry("1.0.0", true));
        lock.entries
            .insert("bar".to_string(), registry_entry("2.0.0", true));

        let err = check_locked_orphans(&deps, &lock).unwrap_err();
        assert!(err.contains("bar"), "got: {err}");
        assert!(err.contains("not in sema.toml"), "got: {err}");
    }

    // ── resolve_dependency_graph tests (pure, no install/network) ─────

    fn registry_entry(version: &str, direct: bool) -> LockEntry {
        LockEntry::Registry {
            version: version.to_string(),
            registry: "https://pkg.sema-lang.com".to_string(),
            checksum: "deadbeef".to_string(),
            direct,
        }
    }

    fn git_entry(git_ref: &str, direct: bool) -> LockEntry {
        LockEntry::Git {
            git_ref: git_ref.to_string(),
            commit: format!("sha-{git_ref}"),
            direct,
        }
    }

    fn direct_deps(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Test harness for `resolve_dependency_graph`: records install calls and
    /// serves canned manifests, so the graph-walking/conflict logic can be
    /// exercised without git/network.
    struct ResolverHarness {
        manifests: BTreeMap<String, BTreeMap<String, String>>,
        fresh_calls: std::cell::RefCell<Vec<(String, String)>>,
        locked_calls: std::cell::RefCell<Vec<String>>,
    }

    impl ResolverHarness {
        fn new() -> Self {
            Self {
                manifests: BTreeMap::new(),
                fresh_calls: std::cell::RefCell::new(vec![]),
                locked_calls: std::cell::RefCell::new(vec![]),
            }
        }

        fn with_manifest(mut self, name: &str, deps: &[(&str, &str)]) -> Self {
            self.manifests.insert(
                name.to_string(),
                deps.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            );
            self
        }

        fn run(
            &self,
            deps: &BTreeMap<String, String>,
            existing_lock: &LockFile,
        ) -> Result<(LockFile, Vec<String>, Vec<ResolutionNote>), String> {
            let fresh_calls = &self.fresh_calls;
            let locked_calls = &self.locked_calls;
            let manifests = &self.manifests;
            resolve_dependency_graph(
                deps,
                existing_lock,
                &mut |name, version| {
                    fresh_calls
                        .borrow_mut()
                        .push((name.to_string(), version.to_string()));
                    if is_git_spec(name) {
                        Ok(git_entry(version, false))
                    } else {
                        Ok(registry_entry(version, false))
                    }
                },
                &mut |name, _entry| {
                    locked_calls.borrow_mut().push(name.to_string());
                    Ok(())
                },
                &|name| Ok(manifests.get(name).cloned().unwrap_or_default()),
            )
        }
    }

    #[test]
    fn resolve_single_direct_dep_no_transitive() {
        let h = ResolverHarness::new();
        let (lock, pruned, notes) = h
            .run(&direct_deps(&[("a", "1.0.0")]), &LockFile::new())
            .unwrap();
        assert_eq!(lock.entries.len(), 1);
        assert!(lock.entries["a"].is_direct());
        assert!(pruned.is_empty());
        assert!(notes.is_empty());
    }

    #[test]
    fn resolve_pulls_in_transitive_deps() {
        let h = ResolverHarness::new()
            .with_manifest("a", &[("b", "1.0.0")])
            .with_manifest("b", &[("c", "1.0.0")]);
        let (lock, _, _) = h
            .run(&direct_deps(&[("a", "1.0.0")]), &LockFile::new())
            .unwrap();
        assert_eq!(lock.entries.len(), 3);
        assert!(lock.entries["a"].is_direct());
        assert!(!lock.entries["b"].is_direct());
        assert!(!lock.entries["c"].is_direct());
    }

    #[test]
    fn resolve_diamond_shared_dep_no_conflict_installed_once() {
        let h = ResolverHarness::new()
            .with_manifest("a", &[("shared", "1.0.0")])
            .with_manifest("b", &[("shared", "1.0.0")]);
        let (lock, _, _) = h
            .run(
                &direct_deps(&[("a", "1.0.0"), ("b", "1.0.0")]),
                &LockFile::new(),
            )
            .unwrap();
        assert_eq!(lock.entries.len(), 3);
        let calls = h
            .fresh_calls
            .borrow()
            .iter()
            .filter(|(n, _)| n == "shared")
            .count();
        assert_eq!(calls, 1, "shared dep must be installed exactly once");
    }

    #[test]
    fn resolve_diamond_registry_same_major_auto_resolves_to_higher() {
        let h = ResolverHarness::new()
            .with_manifest("a", &[("shared", "1.0.0")])
            .with_manifest("b", &[("shared", "1.2.0")]);
        let (lock, _, notes) = h
            .run(
                &direct_deps(&[("a", "1.0.0"), ("b", "1.0.0")]),
                &LockFile::new(),
            )
            .unwrap();
        assert_eq!(lock.entries["shared"].requested(), "1.2.0");
        assert!(notes.iter().any(
            |n| matches!(n, ResolutionNote::DiamondAutoResolved { name, .. } if name == "shared")
        ));
    }

    #[test]
    fn resolve_diamond_registry_different_major_hard_errors() {
        let h = ResolverHarness::new()
            .with_manifest("a", &[("shared", "1.0.0")])
            .with_manifest("b", &[("shared", "2.0.0")]);
        let err = h
            .run(
                &direct_deps(&[("a", "1.0.0"), ("b", "1.0.0")]),
                &LockFile::new(),
            )
            .unwrap_err();
        assert!(err.contains("shared"), "got: {err}");
        assert!(err.contains("breaking"), "got: {err}");
    }

    #[test]
    fn resolve_diamond_registry_unparseable_version_hard_errors() {
        let h = ResolverHarness::new()
            .with_manifest("a", &[("shared", "not-a-version")])
            .with_manifest("b", &[("shared", "1.0.0")]);
        let err = h
            .run(
                &direct_deps(&[("a", "1.0.0"), ("b", "1.0.0")]),
                &LockFile::new(),
            )
            .unwrap_err();
        assert!(err.contains("not a valid semver"), "got: {err}");
    }

    #[test]
    fn resolve_diamond_git_different_refs_hard_errors() {
        let h = ResolverHarness::new()
            .with_manifest("a", &[("github.com/x/y", "v1.0")])
            .with_manifest("b", &[("github.com/x/y", "v2.0")]);
        let err = h
            .run(
                &direct_deps(&[("a", "main"), ("b", "main")]),
                &LockFile::new(),
            )
            .unwrap_err();
        assert!(err.contains("github.com/x/y"), "got: {err}");
        assert!(err.contains("no version ordering"), "got: {err}");
    }

    #[test]
    fn resolve_diamond_git_same_ref_no_conflict() {
        let h = ResolverHarness::new()
            .with_manifest("a", &[("github.com/x/y", "main")])
            .with_manifest("b", &[("github.com/x/y", "main")]);
        let (lock, _, notes) = h
            .run(
                &direct_deps(&[("a", "main"), ("b", "main")]),
                &LockFile::new(),
            )
            .unwrap();
        assert_eq!(lock.entries["github.com/x/y"].requested(), "main");
        assert!(notes.is_empty());
    }

    #[test]
    fn resolve_direct_pin_overrides_transitive_request_silently_with_note() {
        let h = ResolverHarness::new().with_manifest("b", &[("shared", "2.0.0")]);
        let (lock, _, notes) = h
            .run(
                &direct_deps(&[("shared", "1.0.0"), ("b", "1.0.0")]),
                &LockFile::new(),
            )
            .unwrap();
        assert_eq!(lock.entries["shared"].requested(), "1.0.0");
        assert!(lock.entries["shared"].is_direct());
        let calls: Vec<_> = h
            .fresh_calls
            .borrow()
            .iter()
            .filter(|(n, _)| n == "shared")
            .cloned()
            .collect();
        assert_eq!(calls.len(), 1, "must not reinstall for the override");
        assert!(notes
            .iter()
            .any(|n| matches!(n, ResolutionNote::DirectOverride { name, .. } if name == "shared")));
    }

    #[test]
    fn resolve_cycle_does_not_infinite_loop() {
        let h = ResolverHarness::new()
            .with_manifest("a", &[("b", "1.0.0")])
            .with_manifest("b", &[("a", "1.0.0")]);
        let (lock, _, _) = h
            .run(&direct_deps(&[("a", "1.0.0")]), &LockFile::new())
            .unwrap();
        assert_eq!(lock.entries.len(), 2);
    }

    #[test]
    fn resolve_self_referential_manifest_is_noop() {
        let h = ResolverHarness::new().with_manifest("a", &[("a", "1.0.0")]);
        let (lock, _, _) = h
            .run(&direct_deps(&[("a", "1.0.0")]), &LockFile::new())
            .unwrap();
        assert_eq!(lock.entries.len(), 1);
    }

    #[test]
    fn resolve_diamond_reinstall_reenqueues_new_versions_deps() {
        // foo@1.0.0 has no deps; foo@1.2.0 (same major, auto-resolved higher)
        // depends on bar — the resolver must pick that dep up after the
        // reinstall, not leave the graph stale at foo@1.0.0's (empty) deps.
        let h = ResolverHarness::new()
            .with_manifest("a", &[("foo", "1.0.0")])
            .with_manifest("b", &[("foo", "1.2.0")]);
        let foo_1_2_0_deps: BTreeMap<String, String> =
            [("bar".to_string(), "1.0.0".to_string())].into();
        let (lock, _, _) = resolve_dependency_graph(
            &direct_deps(&[("a", "1.0.0"), ("b", "1.0.0")]),
            &LockFile::new(),
            &mut |name, version| {
                if is_git_spec(name) {
                    Ok(git_entry(version, false))
                } else {
                    Ok(registry_entry(version, false))
                }
            },
            &mut |_, _| Ok(()),
            &|name| {
                if name == "foo" {
                    Ok(foo_1_2_0_deps.clone())
                } else {
                    Ok(h.manifests.get(name).cloned().unwrap_or_default())
                }
            },
        )
        .unwrap();
        assert_eq!(lock.entries["foo"].requested(), "1.2.0");
        assert!(
            lock.entries.contains_key("bar"),
            "bar must be pulled in via foo's post-reinstall manifest"
        );
    }

    #[test]
    fn resolve_prunes_stale_lock_entries() {
        let mut old_lock = LockFile::new();
        old_lock
            .entries
            .insert("gone".to_string(), registry_entry("1.0.0", true));
        let h = ResolverHarness::new();
        let (lock, pruned, _) = h.run(&direct_deps(&[("a", "1.0.0")]), &old_lock).unwrap();
        assert!(!lock.entries.contains_key("gone"));
        assert_eq!(pruned, vec!["gone".to_string()]);
    }

    #[test]
    fn resolve_reuses_lock_when_version_matches() {
        let mut old_lock = LockFile::new();
        old_lock
            .entries
            .insert("a".to_string(), registry_entry("1.0.0", true));
        let h = ResolverHarness::new();
        let (lock, _, _) = h.run(&direct_deps(&[("a", "1.0.0")]), &old_lock).unwrap();
        assert_eq!(lock.entries["a"].requested(), "1.0.0");
        assert!(
            h.fresh_calls.borrow().is_empty(),
            "must reuse the lock, not reinstall"
        );
        assert_eq!(h.locked_calls.borrow().as_slice(), ["a".to_string()]);
    }

    #[test]
    fn resolve_unrelated_packages_resolve_independently() {
        let h = ResolverHarness::new()
            .with_manifest("a", &[("x", "1.0.0")])
            .with_manifest("b", &[("y", "1.0.0")]);
        let (lock, _, notes) = h
            .run(
                &direct_deps(&[("a", "1.0.0"), ("b", "1.0.0")]),
                &LockFile::new(),
            )
            .unwrap();
        assert_eq!(lock.entries.len(), 4);
        assert!(notes.is_empty());
    }

    #[test]
    fn resolve_install_fresh_error_propagates() {
        let result = resolve_dependency_graph(
            &direct_deps(&[("a", "1.0.0")]),
            &LockFile::new(),
            &mut |_, _| Err("boom".to_string()),
            &mut |_, _| Ok(()),
            &|_| Ok(BTreeMap::new()),
        );
        assert_eq!(result.unwrap_err(), "boom");
    }

    // ── cmd_install --locked logic tests ──────────────────────────────
    // These test the validation logic without requiring network access.

    #[test]
    #[serial]
    fn cmd_install_locked_fails_without_lock_file() {
        let dir = tmpdir("install-no-lock");
        let _guard = TestDir::new(&dir);

        fs::write(
            "sema.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[deps]\nfoo = \"1.0.0\"\n",
        )
        .unwrap();

        let err = cmd_install(true).unwrap_err();
        assert!(err.contains("sema.lock not found"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn cmd_install_locked_fails_dep_not_in_lock() {
        let dir = tmpdir("install-dep-missing");
        let _guard = TestDir::new(&dir);

        fs::write(
            "sema.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [deps]\nfoo = \"1.0.0\"\nbar = \"2.0.0\"\n",
        )
        .unwrap();

        // Lock only has foo, not bar
        fs::write(
            LOCK_FILE,
            "lock_version = 1\n\n\
             [packages.foo]\n\
             source = \"registry\"\n\
             version = \"1.0.0\"\n\
             registry = \"http://localhost\"\n\
             checksum = \"aaa\"\n",
        )
        .unwrap();

        let err = cmd_install(true).unwrap_err();
        assert!(err.contains("bar"), "got: {err}");
        assert!(err.contains("not in sema.lock"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn cmd_install_locked_fails_orphan_in_lock() {
        let dir = tmpdir("install-orphan");
        let _guard = TestDir::new(&dir);

        // sema.toml has only foo
        fs::write(
            "sema.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[deps]\nfoo = \"1.0.0\"\n",
        )
        .unwrap();

        // Lock has foo AND orphaned-pkg
        fs::write(
            LOCK_FILE,
            "lock_version = 1\n\n\
             [packages.foo]\n\
             source = \"registry\"\n\
             version = \"1.0.0\"\n\
             registry = \"http://localhost\"\n\
             checksum = \"aaa\"\n\n\
             [packages.orphaned-pkg]\n\
             source = \"registry\"\n\
             version = \"3.0.0\"\n\
             registry = \"http://localhost\"\n\
             checksum = \"bbb\"\n",
        )
        .unwrap();

        let err = cmd_install(true).unwrap_err();
        assert!(err.contains("orphaned-pkg"), "got: {err}");
        assert!(err.contains("not in sema.toml"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn cmd_install_locked_fails_version_mismatch_registry() {
        let dir = tmpdir("install-version-mismatch");
        let _guard = TestDir::new(&dir);

        // sema.toml wants 2.0.0
        fs::write(
            "sema.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[deps]\nfoo = \"2.0.0\"\n",
        )
        .unwrap();

        // Lock has 1.0.0
        fs::write(
            LOCK_FILE,
            "lock_version = 1\n\n\
             [packages.foo]\n\
             source = \"registry\"\n\
             version = \"1.0.0\"\n\
             registry = \"http://localhost\"\n\
             checksum = \"aaa\"\n",
        )
        .unwrap();

        let err = cmd_install(true).unwrap_err();
        assert!(err.contains("foo"), "got: {err}");
        assert!(err.contains("mismatch"), "got: {err}");
        assert!(err.contains("2.0.0"), "got: {err}");
        assert!(err.contains("1.0.0"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn cmd_install_locked_fails_ref_mismatch_git() {
        let dir = tmpdir("install-ref-mismatch");
        let _guard = TestDir::new(&dir);

        // sema.toml wants v2.0
        fs::write(
            "sema.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
             [deps]\n\"github.com/user/repo\" = \"v2.0\"\n",
        )
        .unwrap();

        // Lock has v1.0
        fs::write(
            LOCK_FILE,
            "lock_version = 1\n\n\
             [packages.\"github.com/user/repo\"]\n\
             source = \"git\"\n\
             ref = \"v1.0\"\n\
             commit = \"abc123\"\n",
        )
        .unwrap();

        let err = cmd_install(true).unwrap_err();
        assert!(err.contains("github.com/user/repo"), "got: {err}");
        assert!(err.contains("mismatch"), "got: {err}");
        assert!(err.contains("v2.0"), "got: {err}");
        assert!(err.contains("v1.0"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn cmd_install_locked_fails_malformed_lock() {
        let dir = tmpdir("install-malformed-lock");
        let _guard = TestDir::new(&dir);

        fs::write(
            "sema.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[deps]\nfoo = \"1.0.0\"\n",
        )
        .unwrap();

        // Invalid TOML in lock
        fs::write(LOCK_FILE, "this is garbage {{{").unwrap();

        let err = cmd_install(true).unwrap_err();
        assert!(err.contains("parse"), "got: {err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn cmd_install_no_deps_is_ok() {
        let dir = tmpdir("install-no-deps");
        let _guard = TestDir::new(&dir);

        fs::write(
            "sema.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[deps]\n",
        )
        .unwrap();

        // Should succeed even without lock file
        cmd_install(false).unwrap();
        // Lock file should be written (empty packages)
        let lock = read_lock_file().unwrap().unwrap();
        assert!(lock.entries.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn cmd_install_locked_ok_with_matching_empty() {
        let dir = tmpdir("install-locked-empty");
        let _guard = TestDir::new(&dir);

        fs::write(
            "sema.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[deps]\n",
        )
        .unwrap();

        fs::write(LOCK_FILE, "lock_version = 1\n\n[packages]\n").unwrap();

        // Empty deps + empty lock = success
        cmd_install(true).unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    // ── cmd_remove reachability tests ──────────────────────────────────
    // No network involved: cmd_remove only touches sema.toml/sema.lock and
    // whatever manifests already sit on disk under SEMA_HOME.

    #[test]
    #[serial]
    fn cmd_remove_keeps_transitively_required_package_on_disk() {
        let dir = tmpdir("remove-keep-cwd");
        let sema_home = tmpdir("remove-keep-home");
        let _guard = TestDir::new(&dir);
        std::env::set_var("SEMA_HOME", sema_home.to_str().unwrap());

        let pkg_dir = sema_home.join("packages");
        fs::create_dir_all(pkg_dir.join("A")).unwrap();
        fs::write(
            pkg_dir.join("A").join("sema.toml"),
            "[package]\nname = \"a\"\nversion = \"1.0.0\"\n\n[deps]\n",
        )
        .unwrap();
        fs::create_dir_all(pkg_dir.join("B")).unwrap();
        fs::write(
            pkg_dir.join("B").join("sema.toml"),
            "[package]\nname = \"b\"\nversion = \"1.0.0\"\n\n[deps]\nA = \"1.0.0\"\n",
        )
        .unwrap();

        fs::write(
            "sema.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[deps]\nA = \"1.0.0\"\nB = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(
            LOCK_FILE,
            "lock_version = 1\n\n\
             [packages.A]\nsource = \"registry\"\nversion = \"1.0.0\"\nregistry = \"http://localhost\"\nchecksum = \"aaa\"\ndirect = true\n\n\
             [packages.B]\nsource = \"registry\"\nversion = \"1.0.0\"\nregistry = \"http://localhost\"\nchecksum = \"bbb\"\ndirect = true\n",
        )
        .unwrap();

        cmd_remove("A").unwrap();

        assert!(
            pkg_dir.join("A").exists(),
            "A must be kept on disk — still required transitively by B"
        );

        let toml_content = fs::read_to_string("sema.toml").unwrap();
        let doc: toml::Value = toml::from_str(&toml_content).unwrap();
        assert!(
            doc.get("deps").and_then(|d| d.get("A")).is_none(),
            "A must be removed from the direct [deps]"
        );

        let lock = read_lock_file().unwrap().unwrap();
        assert!(
            !lock.entries["A"].is_direct(),
            "A's lock entry must be demoted to transitive, not dropped"
        );

        std::env::remove_var("SEMA_HOME");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&sema_home);
    }

    #[test]
    #[serial]
    fn cmd_remove_deletes_package_nothing_else_needs() {
        let dir = tmpdir("remove-delete-cwd");
        let sema_home = tmpdir("remove-delete-home");
        let _guard = TestDir::new(&dir);
        std::env::set_var("SEMA_HOME", sema_home.to_str().unwrap());

        let pkg_dir = sema_home.join("packages");
        fs::create_dir_all(pkg_dir.join("A")).unwrap();
        fs::write(
            pkg_dir.join("A").join("sema.toml"),
            "[package]\nname = \"a\"\nversion = \"1.0.0\"\n\n[deps]\n",
        )
        .unwrap();

        fs::write(
            "sema.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[deps]\nA = \"1.0.0\"\n",
        )
        .unwrap();
        fs::write(
            LOCK_FILE,
            "lock_version = 1\n\n\
             [packages.A]\nsource = \"registry\"\nversion = \"1.0.0\"\nregistry = \"http://localhost\"\nchecksum = \"aaa\"\ndirect = true\n",
        )
        .unwrap();

        cmd_remove("A").unwrap();

        assert!(
            !pkg_dir.join("A").exists(),
            "A must be deleted — nothing else needs it"
        );
        assert!(!Path::new(LOCK_FILE).exists(), "lock must be empty/removed");

        std::env::remove_var("SEMA_HOME");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&sema_home);
    }

    // ── TOML key edge cases ───────────────────────────────────────────

    #[test]
    #[serial]
    fn lock_file_preserves_dotted_slash_keys() {
        let dir = tmpdir("lock-dotted-keys");
        let _guard = TestDir::new(&dir);

        let mut lock = LockFile::new();
        // Keys with dots and slashes must survive round-trip
        lock.entries.insert(
            "github.com/org/repo.name".to_string(),
            LockEntry::Git {
                git_ref: "main".to_string(),
                commit: "deadbeef12345678".to_string(),
                direct: true,
            },
        );
        lock.entries.insert(
            "gitlab.com/deep/nested/path".to_string(),
            LockEntry::Git {
                git_ref: "v1.0.0-beta.1".to_string(),
                commit: "cafebabe".to_string(),
                direct: true,
            },
        );

        write_lock_file(&lock).unwrap();

        let loaded = read_lock_file().unwrap().unwrap();
        assert_eq!(loaded.entries.len(), 2);
        assert!(loaded.entries.contains_key("github.com/org/repo.name"));
        assert!(loaded.entries.contains_key("gitlab.com/deep/nested/path"));

        match &loaded.entries["gitlab.com/deep/nested/path"] {
            LockEntry::Git { git_ref, .. } => assert_eq!(git_ref, "v1.0.0-beta.1"),
            _ => panic!("Expected git entry"),
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn lock_file_empty_packages_round_trips() {
        let dir = tmpdir("lock-empty");
        let _guard = TestDir::new(&dir);

        let lock = LockFile::new();
        write_lock_file(&lock).unwrap();

        let loaded = read_lock_file().unwrap().unwrap();
        assert!(loaded.entries.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }
}
