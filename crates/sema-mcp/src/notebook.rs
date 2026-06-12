use sema_notebook::format::{Cell, CellType};
use sema_notebook::{Engine, Notebook};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub type NotebookCache = Rc<RefCell<BTreeMap<PathBuf, Rc<RefCell<Engine>>>>>;

/// Resolves a path to a canonical path and obtains/caches its Engine.
pub fn get_or_create_engine(
    cache: &NotebookCache,
    path_str: &str,
) -> Result<(PathBuf, Rc<RefCell<Engine>>), String> {
    let path = PathBuf::from(path_str);
    // If the file doesn't exist yet, we can't canonicalize it directly,
    // so we canonicalize its parent directory first.
    let canonical = if path.exists() {
        path.canonicalize()
            .map_err(|e| format!("Invalid path {}: {e}", path.display()))?
    } else {
        let parent = path.parent().unwrap_or(Path::new("."));
        let canonical_parent = parent
            .canonicalize()
            .map_err(|e| format!("Invalid parent directory {}: {e}", parent.display()))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| "Path must have a filename".to_string())?;
        canonical_parent.join(file_name)
    };

    let mut map = cache.borrow_mut();
    if let Some(engine) = map.get(&canonical) {
        return Ok((canonical, engine.clone()));
    }

    // Load from disk or create new if it doesn't exist
    let engine = if canonical.exists() {
        Engine::from_file(&canonical)?
    } else {
        Engine::new(Notebook::new("Untitled"))
    };

    let shared = Rc::new(RefCell::new(engine));
    map.insert(canonical.clone(), shared.clone());
    Ok((canonical, shared))
}

pub fn create_notebook(
    cache: &NotebookCache,
    path_str: &str,
    title: Option<&str>,
) -> Result<PathBuf, String> {
    let (canonical, engine_rc) = get_or_create_engine(cache, path_str)?;
    let mut engine = engine_rc.borrow_mut();

    let t = title.unwrap_or("Untitled");
    engine.notebook = Notebook::new(t);
    engine.notebook.save(&canonical)?;
    Ok(canonical)
}

pub fn add_cell(
    engine_rc: &Rc<RefCell<Engine>>,
    canonical_path: &Path,
    cell_type_str: &str,
    source: &str,
    after_id: Option<&str>,
) -> Result<String, String> {
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
    Ok(cell_id)
}

pub fn update_cell(
    engine_rc: &Rc<RefCell<Engine>>,
    canonical_path: &Path,
    cell_id: &str,
    source: Option<&str>,
    cell_type_str: Option<&str>,
) -> Result<(), String> {
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
    Ok(())
}

pub fn delete_cell(
    engine_rc: &Rc<RefCell<Engine>>,
    canonical_path: &Path,
    cell_id: &str,
) -> Result<(), String> {
    let mut engine = engine_rc.borrow_mut();
    let idx = engine
        .notebook
        .cell_index(cell_id)
        .ok_or_else(|| format!("Cell not found: {cell_id}"))?;

    engine.notebook.cells.remove(idx);
    engine.notebook.save(canonical_path)?;
    Ok(())
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
