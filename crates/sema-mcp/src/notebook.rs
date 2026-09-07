use sema_notebook::format::{Cell, CellType};
use sema_notebook::{Engine, Notebook};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

/// Maximum number of distinct notebooks kept resident in the cache at once.
///
/// The cache is process-lived; without a cap it grows unbounded as a client
/// touches more and more notebook files over a long session. When the cap is
/// exceeded we evict the least-recently-used entry. The bound is small because
/// each entry holds a full interpreter env; clients typically work on one or a
/// handful of notebooks at a time.
const MAX_CACHED_NOTEBOOKS: usize = 16;

/// A cached notebook engine plus the metadata needed to keep it coherent with
/// disk and to drive LRU eviction.
struct CacheEntry {
    engine: Rc<RefCell<Engine>>,
    /// File modification time observed when this engine was loaded from disk.
    /// `None` means the file did not exist at load time (a freshly created,
    /// not-yet-saved notebook). Used to detect out-of-band edits.
    loaded_mtime: Option<SystemTime>,
    /// Monotonic tick of last access, for LRU eviction.
    last_used: u64,
}

/// Cache state: the entries plus a monotonic access counter.
pub struct CacheState {
    entries: BTreeMap<PathBuf, CacheEntry>,
    tick: u64,
}

impl CacheState {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            tick: 0,
        }
    }

    fn next_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// Evict least-recently-used entries until at most `MAX_CACHED_NOTEBOOKS`
    /// remain. Called after an insert so the cache never exceeds the cap.
    fn evict_if_needed(&mut self) {
        while self.entries.len() > MAX_CACHED_NOTEBOOKS {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    self.entries.remove(&k);
                }
                None => break,
            }
        }
    }

    /// Number of resident entries (test/introspection helper).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub type NotebookCache = Rc<RefCell<CacheState>>;

/// Construct an empty notebook cache.
pub fn new_cache() -> NotebookCache {
    Rc::new(RefCell::new(CacheState::new()))
}

/// Current modification time of `path`, if the file exists and reports one.
fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Resolve a notebook path to a stable cache key.
///
/// When the file (or its leaf symlink) exists we canonicalize the *full* path so
/// a symlinked leaf and its target collapse to a single key — otherwise editing
/// through one name would serve a stale cached copy under the other. When the
/// file does not yet exist (e.g. the pre-create call), we fall back to
/// canonicalizing the *parent directory* and appending the raw filename. That
/// fallback yields the same key the post-create call produces as long as the
/// leaf is not itself a symlink; symlinked leaves are only reachable once their
/// target exists, at which point full canonicalization takes over and both names
/// converge on one key.
fn canonicalize_notebook_path(path_str: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path_str);

    // Prefer full canonicalization: this resolves a symlinked leaf to its real
    // target so two names for the same file share one cache key.
    if let Ok(canonical) = path.canonicalize() {
        return Ok(canonical);
    }

    // File doesn't exist yet: canonicalize the parent and append the raw name.
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let canonical_parent = parent
        .canonicalize()
        .map_err(|e| format!("Invalid parent directory {}: {e}", parent.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| "Path must have a filename".to_string())?;
    Ok(canonical_parent.join(file_name))
}

/// Resolves a path to a canonical path and obtains/caches its Engine.
///
/// If the cached engine's source file was modified out-of-band since it was
/// loaded, the stale entry is discarded and the notebook is reloaded from disk
/// so callers never operate on an outdated in-memory copy.
pub fn get_or_create_engine(
    cache: &NotebookCache,
    path_str: &str,
) -> Result<(PathBuf, Rc<RefCell<Engine>>), String> {
    let canonical = canonicalize_notebook_path(path_str)?;
    let disk_mtime = file_mtime(&canonical);

    let mut state = cache.borrow_mut();

    // Serve the cached engine unless the file changed on disk behind our back.
    let cached = state.entries.get(&canonical).map(|entry| {
        let stale = match (entry.loaded_mtime, disk_mtime) {
            // File now exists with a newer mtime than when we loaded it.
            (Some(loaded), Some(current)) => current > loaded,
            // We had a not-yet-saved notebook but a file now exists on disk.
            (None, Some(_)) => true,
            // Either no on-disk file now, or mtime unavailable: keep the cached
            // copy (the in-memory engine is the source of truth in that case).
            _ => false,
        };
        (stale, entry.engine.clone())
    });
    if let Some((stale, engine)) = cached {
        if !stale {
            let tick = state.next_tick();
            if let Some(entry) = state.entries.get_mut(&canonical) {
                entry.last_used = tick;
            }
            return Ok((canonical, engine));
        }
        // Fall through to reload from disk, replacing the stale entry.
    }

    // Load from disk or create new if it doesn't exist.
    let engine = if canonical.exists() {
        Engine::from_file(&canonical)?
    } else {
        Engine::new(Notebook::new("Untitled"))
    };

    let shared = Rc::new(RefCell::new(engine));
    let tick = state.next_tick();
    state.entries.insert(
        canonical.clone(),
        CacheEntry {
            engine: shared.clone(),
            loaded_mtime: disk_mtime,
            last_used: tick,
        },
    );
    state.evict_if_needed();
    Ok((canonical, shared))
}

/// Refresh the recorded mtime for a cached entry after we ourselves wrote it to
/// disk, so our own save is not later mistaken for an out-of-band edit (which
/// would needlessly reload and drop the in-memory interpreter env). No-op if the
/// path is not cached.
fn note_self_write(cache: &NotebookCache, canonical: &Path) {
    if let Ok(mut state) = cache.try_borrow_mut() {
        if let Some(entry) = state.entries.get_mut(canonical) {
            entry.loaded_mtime = file_mtime(canonical);
        }
    }
}

pub fn create_notebook(
    cache: &NotebookCache,
    path_str: &str,
    title: Option<&str>,
    overwrite: bool,
) -> Result<PathBuf, String> {
    // Refuse to clobber an existing notebook unless explicitly asked: this is a
    // tool named "new", and silently overwriting a populated .sema-nb on disk
    // is irreversible data loss.
    let canonical = canonicalize_notebook_path(path_str)?;
    if canonical.exists() && !overwrite {
        return Err(format!(
            "Notebook already exists at {}; pass overwrite=true to replace it",
            canonical.display()
        ));
    }

    let (canonical, engine_rc) = get_or_create_engine(cache, path_str)?;
    {
        let mut engine = engine_rc.borrow_mut();

        let t = title.unwrap_or("Untitled");
        // Replace the whole Engine, not just its notebook field, so the
        // interpreter env, snapshot, and cell timeout are all reset. A cached
        // engine for this path would otherwise leak bindings/state from the
        // previous notebook into the freshly created one.
        *engine = Engine::new(Notebook::new(t));
        engine.notebook.save(&canonical)?;
    }
    // Record our own write so the next access doesn't see it as a stale edit.
    note_self_write(cache, &canonical);
    Ok(canonical)
}

pub fn add_cell(
    cache: &NotebookCache,
    engine_rc: &Rc<RefCell<Engine>>,
    canonical_path: &Path,
    cell_type_str: &str,
    source: &str,
    after_id: Option<&str>,
) -> Result<String, String> {
    let cell_id = {
        let mut engine = engine_rc.borrow_mut();
        let cell_type = match cell_type_str {
            "code" => CellType::Code,
            "markdown" => CellType::Markdown,
            _ => return Err(format!("Invalid cell type: {cell_type_str}")),
        };

        // Generate cell ID using the uuid/hex pattern matching sema-notebook
        let uuid_str = uuid::Uuid::new_v4().simple().to_string();
        let cell_id = format!("c{}", &uuid_str[..8]);

        let new_cell = Cell {
            id: cell_id.clone(),
            cell_type,
            source: source.to_string(),
            outputs: Vec::new(),
            stale: false,
        };

        if let Some(after) = after_id {
            let idx = engine
                .notebook
                .cell_index(after)
                .ok_or_else(|| format!("Cell not found for insertion: {after}"))?;
            engine.notebook.cells.insert(idx + 1, new_cell);
        } else {
            engine.notebook.cells.push(new_cell);
        }

        engine.notebook.save(canonical_path)?;
        cell_id
    };
    note_self_write(cache, canonical_path);
    Ok(cell_id)
}

pub fn update_cell(
    cache: &NotebookCache,
    engine_rc: &Rc<RefCell<Engine>>,
    canonical_path: &Path,
    cell_id: &str,
    source: Option<&str>,
    cell_type_str: Option<&str>,
) -> Result<(), String> {
    {
        let mut engine = engine_rc.borrow_mut();
        let idx = engine
            .notebook
            .cell_index(cell_id)
            .ok_or_else(|| format!("Cell not found: {cell_id}"))?;

        let cell = &mut engine.notebook.cells[idx];
        if let Some(src) = source {
            cell.source = src.to_string();
        }
        if let Some(ct) = cell_type_str {
            cell.cell_type = match ct {
                "code" => CellType::Code,
                "markdown" => CellType::Markdown,
                _ => return Err(format!("Invalid cell type: {ct}")),
            };
        }

        // Clear outputs of modified cell and mark downstream as stale
        let cell = &mut engine.notebook.cells[idx];
        cell.outputs.clear();
        cell.stale = false;

        engine.notebook.mark_downstream_stale(idx);
        engine.notebook.save(canonical_path)?;
    }
    note_self_write(cache, canonical_path);
    Ok(())
}

pub fn delete_cell(
    cache: &NotebookCache,
    engine_rc: &Rc<RefCell<Engine>>,
    canonical_path: &Path,
    cell_id: &str,
) -> Result<(), String> {
    {
        let mut engine = engine_rc.borrow_mut();
        let idx = engine
            .notebook
            .cell_index(cell_id)
            .ok_or_else(|| format!("Cell not found: {cell_id}"))?;

        engine.notebook.cells.remove(idx);
        engine.notebook.save(canonical_path)?;
    }
    note_self_write(cache, canonical_path);
    Ok(())
}

/// Refresh the recorded mtime after an externally-triggered save (e.g. a cell
/// evaluation that writes outputs back to disk). Public so the dispatch layer
/// can keep the cache coherent with its own writes.
pub fn note_external_save(cache: &NotebookCache, canonical_path: &Path) {
    note_self_write(cache, canonical_path);
}

pub fn export_notebook(engine_rc: &Rc<RefCell<Engine>>, format: &str) -> Result<String, String> {
    let engine = engine_rc.borrow();
    match format {
        "markdown" => Ok(sema_notebook::render::export_markdown(&engine.notebook)),
        "source" => {
            let src = engine
                .notebook
                .cells
                .iter()
                .filter(|c| c.cell_type == CellType::Code)
                .map(|c| c.source.clone())
                .collect::<Vec<String>>()
                .join("\n\n");
            Ok(src)
        }
        _ => Err(format!("Unsupported export format: {format}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn unique_dir(tag: &str) -> PathBuf {
        sema_core::testing::unique_temp_dir(&format!("mcp-nb-{tag}"))
    }

    fn write_empty_notebook(path: &Path, title: &str) {
        let mut nb = Notebook::new(title);
        nb.save(path).unwrap();
    }

    /// MCP-4: the cache must be bounded and evict the least-recently-used entry
    /// once the cap is exceeded.
    #[test]
    fn test_cache_evicts_lru_beyond_cap() {
        let dir = unique_dir("evict");
        let cache = new_cache();

        // Touch MAX_CACHED_NOTEBOOKS + 4 distinct notebooks.
        let total = MAX_CACHED_NOTEBOOKS + 4;
        let mut paths = Vec::new();
        for i in 0..total {
            let p = dir.join(format!("nb_{i}.sema-nb"));
            write_empty_notebook(&p, &format!("nb{i}"));
            get_or_create_engine(&cache, p.to_str().unwrap()).unwrap();
            paths.push(p);
        }

        // Never exceeds the cap.
        assert_eq!(cache.borrow().len(), MAX_CACHED_NOTEBOOKS);

        // The earliest-touched notebooks were evicted; the most recent are kept.
        let canonical_last = paths[total - 1].canonicalize().unwrap();
        assert!(cache.borrow().entries.contains_key(&canonical_last));
        let canonical_first = paths[0].canonicalize().unwrap();
        assert!(!cache.borrow().entries.contains_key(&canonical_first));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// MCP-4: LRU recency must follow access, not insertion order.
    #[test]
    fn test_cache_lru_recency_protects_reaccessed() {
        let dir = unique_dir("recency");
        let cache = new_cache();

        let first = dir.join("first.sema-nb");
        write_empty_notebook(&first, "first");
        get_or_create_engine(&cache, first.to_str().unwrap()).unwrap();

        // Fill the rest of the cache, then re-access `first` so it is the most
        // recently used before we push it over the cap.
        for i in 0..(MAX_CACHED_NOTEBOOKS - 1) {
            let p = dir.join(format!("fill_{i}.sema-nb"));
            write_empty_notebook(&p, "fill");
            get_or_create_engine(&cache, p.to_str().unwrap()).unwrap();
        }
        // Re-access `first`.
        get_or_create_engine(&cache, first.to_str().unwrap()).unwrap();

        // Now add two more, forcing eviction of the two stalest fillers — but
        // `first` was just touched, so it must survive.
        for i in 0..2 {
            let p = dir.join(format!("extra_{i}.sema-nb"));
            write_empty_notebook(&p, "extra");
            get_or_create_engine(&cache, p.to_str().unwrap()).unwrap();
        }

        let canonical_first = first.canonicalize().unwrap();
        assert!(
            cache.borrow().entries.contains_key(&canonical_first),
            "recently re-accessed notebook must not be evicted"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// MCP-4: an out-of-band edit (newer mtime) must trigger a reload so the
    /// served engine reflects the on-disk content.
    #[test]
    fn test_cache_reloads_on_out_of_band_edit() {
        let dir = unique_dir("reload");
        let path = dir.join("doc.sema-nb");

        // Initial notebook with one cell.
        let mut nb = Notebook::new("doc");
        nb.cells.push(Cell {
            id: "c0000001".to_string(),
            cell_type: CellType::Code,
            source: "(+ 1 1)".to_string(),
            outputs: Vec::new(),
            stale: false,
        });
        nb.save(&path).unwrap();

        let cache = new_cache();
        let canonical = path.canonicalize().unwrap();
        let (_, eng) = get_or_create_engine(&cache, path.to_str().unwrap()).unwrap();
        assert_eq!(eng.borrow().notebook.cells.len(), 1);

        // Backdate the recorded load time so the on-disk mtime is guaranteed to
        // look newer regardless of filesystem timestamp granularity. This
        // exercises exactly the staleness comparison used in production.
        {
            let mut state = cache.borrow_mut();
            let entry = state.entries.get_mut(&canonical).unwrap();
            entry.loaded_mtime =
                Some(std::time::SystemTime::now() - std::time::Duration::from_secs(3600));
        }

        // Edit the file out-of-band: rewrite it with an extra cell.
        let mut nb2 = Notebook::new("doc");
        nb2.cells.push(Cell {
            id: "c0000001".to_string(),
            cell_type: CellType::Code,
            source: "(+ 1 1)".to_string(),
            outputs: Vec::new(),
            stale: false,
        });
        nb2.cells.push(Cell {
            id: "c0000002".to_string(),
            cell_type: CellType::Code,
            source: "(+ 2 2)".to_string(),
            outputs: Vec::new(),
            stale: false,
        });
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(serde_json::to_string(&nb2).unwrap().as_bytes())
                .unwrap();
            f.flush().unwrap();
        }

        let (_, eng2) = get_or_create_engine(&cache, path.to_str().unwrap()).unwrap();
        assert_eq!(
            eng2.borrow().notebook.cells.len(),
            2,
            "out-of-band edit must trigger a reload from disk"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// MCP-4: our own saves (via the mutating helpers) must NOT be mistaken for
    /// out-of-band edits — the in-memory engine identity is preserved.
    #[test]
    fn test_cache_self_write_does_not_reload() {
        let dir = unique_dir("selfwrite");
        let path = dir.join("doc.sema-nb");
        write_empty_notebook(&path, "doc");

        let cache = new_cache();
        let (canonical, eng) = get_or_create_engine(&cache, path.to_str().unwrap()).unwrap();
        let ptr_before = Rc::as_ptr(&eng);

        // Add a cell through our own helper (which saves + refreshes mtime).
        add_cell(&cache, &eng, &canonical, "code", "(+ 1 2)", None).unwrap();

        let (_, eng2) = get_or_create_engine(&cache, path.to_str().unwrap()).unwrap();
        assert_eq!(
            Rc::as_ptr(&eng2),
            ptr_before,
            "our own save must not invalidate the cached engine"
        );
        assert_eq!(eng2.borrow().notebook.cells.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// MCP-4: a symlinked leaf and its target must collapse to a single cache
    /// key once the target exists, so edits through either name are coherent.
    #[cfg(unix)]
    #[test]
    fn test_cache_symlink_leaf_single_key() {
        let dir = unique_dir("symlink");
        let target = dir.join("real.sema-nb");
        write_empty_notebook(&target, "real");
        let link = dir.join("alias.sema-nb");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let cache = new_cache();
        let (k_target, _) = get_or_create_engine(&cache, target.to_str().unwrap()).unwrap();
        let (k_link, _) = get_or_create_engine(&cache, link.to_str().unwrap()).unwrap();

        assert_eq!(
            k_target, k_link,
            "symlinked leaf must resolve to the same cache key as its target"
        );
        assert_eq!(
            cache.borrow().len(),
            1,
            "both names must share one cache entry"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
