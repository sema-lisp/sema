//! Notebook file format (.sema-nb) — JSON-based cell notebook format.
//!
//! This module defines the serializable types that make up a `.sema-nb` file.
//! The format stores both source code and evaluated outputs, enabling
//! "hydration" (restoring state) without re-evaluation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level notebook document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notebook {
    /// Format version (currently 1).
    pub version: u32,
    /// Notebook metadata.
    pub metadata: NotebookMetadata,
    /// Ordered list of cells.
    pub cells: Vec<Cell>,
}

/// Notebook-level metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookMetadata {
    /// Human-readable title.
    #[serde(default)]
    pub title: String,
    /// When the notebook was created.
    #[serde(default = "now")]
    pub created: DateTime<Utc>,
    /// Last modification time.
    #[serde(default = "now")]
    pub modified: DateTime<Utc>,
    /// Sema version used to create/last-evaluate the notebook.
    #[serde(default)]
    pub sema_version: String,
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

/// A single cell in the notebook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    /// Unique cell identifier.
    pub id: String,
    /// Cell type.
    #[serde(rename = "type")]
    pub cell_type: CellType,
    /// Source content (Sema code or Markdown).
    pub source: String,
    /// Evaluation outputs (empty for markdown cells or unevaluated code cells).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<CellOutput>,
    /// Whether this cell's output is stale (upstream cell was re-evaluated).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

/// The type of a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellType {
    Code,
    Markdown,
}

/// Output produced by evaluating a code cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellOutput {
    /// The kind of output.
    #[serde(rename = "type")]
    pub output_type: OutputType,
    /// Human-readable display string.
    pub display: String,
    /// Round-trippable S-expression form of the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sema_value: Option<String>,
    /// When this output was produced.
    pub timestamp: DateTime<Utc>,
    /// Estimated LLM cost in USD, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// LLM token usage for this evaluation, if it made any LLM calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<CellUsage>,
    /// Whether this value requires re-evaluation (e.g. file handles).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub requires_reeval: bool,
    /// Duration of evaluation in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Per-cell LLM usage, mirroring the `(llm/session-usage)` map shape.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CellUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_read_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_creation_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Output type discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputType {
    Value,
    Error,
    Stdout,
}

impl Notebook {
    /// Create a new empty notebook with the given title.
    pub fn new(title: &str) -> Self {
        let now = Utc::now();
        Self {
            version: 1,
            metadata: NotebookMetadata {
                title: title.to_string(),
                created: now,
                modified: now,
                sema_version: String::new(),
            },
            cells: Vec::new(),
        }
    }

    /// Load a notebook from a `.sema-nb` JSON file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {e}", path.display()))
    }

    /// Save the notebook to a `.sema-nb` JSON file.
    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        self.metadata.modified = Utc::now();
        self.metadata.sema_version = env!("CARGO_PKG_VERSION").to_string();
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize notebook: {e}"))?;
        std::fs::write(path, json).map_err(|e| format!("Failed to write {}: {e}", path.display()))
    }

    /// Add a new code cell and return its ID.
    pub fn add_code_cell(&mut self, source: &str) -> String {
        let id = new_cell_id();
        self.cells.push(Cell {
            id: id.clone(),
            cell_type: CellType::Code,
            source: source.to_string(),
            outputs: Vec::new(),
            stale: false,
        });
        id
    }

    /// Add a new markdown cell and return its ID.
    pub fn add_markdown_cell(&mut self, source: &str) -> String {
        let id = new_cell_id();
        self.cells.push(Cell {
            id: id.clone(),
            cell_type: CellType::Markdown,
            source: source.to_string(),
            outputs: Vec::new(),
            stale: false,
        });
        id
    }

    /// Find a cell by ID, returning a mutable reference.
    pub fn cell_mut(&mut self, id: &str) -> Option<&mut Cell> {
        self.cells.iter_mut().find(|c| c.id == id)
    }

    /// Find a cell by ID.
    pub fn cell(&self, id: &str) -> Option<&Cell> {
        self.cells.iter().find(|c| c.id == id)
    }

    /// Find the index of a cell by ID.
    pub fn cell_index(&self, id: &str) -> Option<usize> {
        self.cells.iter().position(|c| c.id == id)
    }

    /// The 1-based ordinal of a code cell among code cells only (matching
    /// `render::render_notebook`'s numbering), or `None` if `id` doesn't name
    /// a code cell.
    pub fn cell_number(&self, id: &str) -> Option<usize> {
        self.cells
            .iter()
            .filter(|c| c.cell_type == CellType::Code)
            .position(|c| c.id == id)
            .map(|i| i + 1)
    }

    /// Mark all cells after `index` as stale.
    pub fn mark_downstream_stale(&mut self, index: usize) {
        for cell in self.cells.iter_mut().skip(index + 1) {
            if cell.cell_type == CellType::Code && !cell.outputs.is_empty() {
                cell.stale = true;
            }
        }
    }
}

/// Generate a short unique cell ID.
fn new_cell_id() -> String {
    let uuid = uuid::Uuid::new_v4();
    // Use first 8 hex chars for brevity
    format!("c{}", &uuid.simple().to_string()[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_notebook_has_correct_defaults() {
        let nb = Notebook::new("Test");
        assert_eq!(nb.version, 1);
        assert_eq!(nb.metadata.title, "Test");
        assert!(nb.cells.is_empty());
    }

    #[test]
    fn add_code_cell() {
        let mut nb = Notebook::new("T");
        let id = nb.add_code_cell("(+ 1 2)");
        assert_eq!(nb.cells.len(), 1);
        assert_eq!(nb.cells[0].cell_type, CellType::Code);
        assert_eq!(nb.cells[0].source, "(+ 1 2)");
        assert_eq!(nb.cells[0].id, id);
    }

    #[test]
    fn add_markdown_cell() {
        let mut nb = Notebook::new("T");
        let id = nb.add_markdown_cell("# Hello");
        assert_eq!(nb.cells.len(), 1);
        assert_eq!(nb.cells[0].cell_type, CellType::Markdown);
        assert_eq!(nb.cells[0].source, "# Hello");
        assert!(nb.cell(&id).is_some());
    }

    #[test]
    fn cell_lookup_by_id() {
        let mut nb = Notebook::new("T");
        let id1 = nb.add_code_cell("a");
        let id2 = nb.add_code_cell("b");
        assert_eq!(nb.cell_index(&id1), Some(0));
        assert_eq!(nb.cell_index(&id2), Some(1));
        assert_eq!(nb.cell_index("nonexistent"), None);
        assert!(nb.cell(&id1).is_some());
        assert!(nb.cell("nonexistent").is_none());
    }

    #[test]
    fn cell_mut_updates_source() {
        let mut nb = Notebook::new("T");
        let id = nb.add_code_cell("old");
        nb.cell_mut(&id).unwrap().source = "new".to_string();
        assert_eq!(nb.cell(&id).unwrap().source, "new");
    }

    #[test]
    fn mark_downstream_stale() {
        let mut nb = Notebook::new("T");
        let _id1 = nb.add_code_cell("a");
        let id2 = nb.add_code_cell("b");
        let id3 = nb.add_code_cell("c");
        let _md = nb.add_markdown_cell("text");

        // Give cells 2 and 3 outputs so they can become stale
        let output = CellOutput {
            output_type: OutputType::Value,
            display: "val".to_string(),
            sema_value: None,
            timestamp: Utc::now(),
            cost_usd: None,
            usage: None,
            requires_reeval: false,
            duration_ms: None,
        };
        nb.cell_mut(&id2).unwrap().outputs = vec![output.clone()];
        nb.cell_mut(&id3).unwrap().outputs = vec![output];

        // Mark downstream of cell 0
        nb.mark_downstream_stale(0);
        assert!(!nb.cells[0].stale); // cell 0 itself not stale
        assert!(nb.cells[1].stale); // cell 1 (code with output) is stale
        assert!(nb.cells[2].stale); // cell 2 (code with output) is stale
        assert!(!nb.cells[3].stale); // markdown cell not marked stale
    }

    #[test]
    fn mark_downstream_skips_empty_outputs() {
        let mut nb = Notebook::new("T");
        nb.add_code_cell("a");
        nb.add_code_cell("b"); // no outputs

        nb.mark_downstream_stale(0);
        assert!(!nb.cells[1].stale); // no outputs = not marked stale
    }

    #[test]
    fn serialization_round_trip() {
        let mut nb = Notebook::new("Round Trip");
        nb.add_code_cell("(define x 42)");
        nb.add_markdown_cell("## Notes");

        let json = serde_json::to_string(&nb).unwrap();
        let restored: Notebook = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.version, 1);
        assert_eq!(restored.metadata.title, "Round Trip");
        assert_eq!(restored.cells.len(), 2);
        assert_eq!(restored.cells[0].cell_type, CellType::Code);
        assert_eq!(restored.cells[0].source, "(define x 42)");
        assert_eq!(restored.cells[1].cell_type, CellType::Markdown);
        assert_eq!(restored.cells[1].source, "## Notes");
    }

    #[test]
    fn serialization_with_output_round_trip() {
        let mut nb = Notebook::new("T");
        let id = nb.add_code_cell("(+ 1 2)");
        nb.cell_mut(&id).unwrap().outputs = vec![CellOutput {
            output_type: OutputType::Value,
            display: "3".to_string(),
            sema_value: Some("3".to_string()),
            timestamp: Utc::now(),
            cost_usd: None,
            usage: None,
            requires_reeval: false,
            duration_ms: Some(5),
        }];

        let json = serde_json::to_string(&nb).unwrap();
        let restored: Notebook = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.cells[0].outputs.len(), 1);
        assert_eq!(restored.cells[0].outputs[0].display, "3");
        assert_eq!(restored.cells[0].outputs[0].duration_ms, Some(5));
    }

    #[test]
    fn stale_field_skipped_when_false() {
        let mut nb = Notebook::new("T");
        nb.add_code_cell("a");
        let json = serde_json::to_string(&nb).unwrap();
        assert!(!json.contains("stale"));
    }

    #[test]
    fn cell_id_format() {
        let id = new_cell_id();
        assert!(id.starts_with('c'));
        assert_eq!(id.len(), 9); // "c" + 8 hex chars
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = sema_core::testing::unique_temp_dir("nb-test");
        let path = dir.join("test.sema-nb");

        let mut nb = Notebook::new("File Test");
        nb.add_code_cell("(+ 1 2)");
        nb.save(&path).unwrap();

        let loaded = Notebook::load(&path).unwrap();
        assert_eq!(loaded.metadata.title, "File Test");
        assert_eq!(loaded.cells.len(), 1);
        assert_eq!(loaded.metadata.sema_version, env!("CARGO_PKG_VERSION"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
