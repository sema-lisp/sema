//! Rendering helpers for notebook output.
//!
//! This module provides UI-agnostic rendering primitives that convert
//! notebook data structures into display-ready formats. These helpers
//! are designed to be reusable across different UI implementations
//! (browser, terminal, export formats, etc.).

use crate::format::{Cell, CellOutput, CellType, Notebook, OutputType};
use std::collections::BTreeMap;

// ── Structured output types for UI consumption ───────────────────

/// A fully rendered cell, ready for a UI to display.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RenderedCell {
    pub id: String,
    pub cell_type: CellType,
    pub source: String,
    pub rendered_outputs: Vec<RenderedOutput>,
    pub stale: bool,
    pub cell_number: Option<usize>,
}

/// A rendered output block.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RenderedOutput {
    pub output_type: OutputType,
    /// The display content (may be plain text, HTML, etc. depending on renderer).
    pub content: String,
    /// MIME type hint for the content.
    pub mime_type: String,
    /// Metadata about the output (duration, cost, etc.).
    pub meta: OutputMeta,
}

/// Metadata about a cell output.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct OutputMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::format::CellUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    pub requires_reeval: bool,
}

// ── Rendering functions ──────────────────────────────────────────

/// Render all cells in a notebook into UI-ready structures.
pub fn render_notebook(notebook: &Notebook) -> Vec<RenderedCell> {
    let mut code_counter = 0usize;
    notebook
        .cells
        .iter()
        .map(|cell| {
            let cell_number = if cell.cell_type == CellType::Code {
                code_counter += 1;
                Some(code_counter)
            } else {
                None
            };
            render_cell(cell, cell_number)
        })
        .collect()
}

/// Render a single cell.
pub fn render_cell(cell: &Cell, cell_number: Option<usize>) -> RenderedCell {
    let rendered_outputs = cell.outputs.iter().map(render_output).collect();
    RenderedCell {
        id: cell.id.clone(),
        cell_type: cell.cell_type,
        source: cell.source.clone(),
        rendered_outputs,
        stale: cell.stale,
        cell_number,
    }
}

/// Render a single output.
pub fn render_output(output: &CellOutput) -> RenderedOutput {
    let (content, mime_type) = match output.output_type {
        OutputType::Value => {
            let content = if output.display.is_empty() {
                String::new()
            } else {
                output.display.clone()
            };
            (content, "text/plain".to_string())
        }
        OutputType::Error => (output.display.clone(), "text/x-sema-error".to_string()),
        OutputType::Stdout => (output.display.clone(), "text/plain".to_string()),
    };

    RenderedOutput {
        output_type: output.output_type,
        content,
        mime_type,
        meta: OutputMeta {
            duration_ms: output.duration_ms,
            cost_usd: output.cost_usd,
            usage: output.usage.clone(),
            timestamp: Some(output.timestamp.to_rfc3339()),
            requires_reeval: output.requires_reeval,
        },
    }
}

// ── Markdown export ──────────────────────────────────────────────

/// Export a notebook to Markdown format.
pub fn export_markdown(notebook: &Notebook) -> String {
    let mut out = String::new();

    if !notebook.metadata.title.is_empty() {
        out.push_str(&format!("# {}\n\n", notebook.metadata.title));
    }

    let mut code_counter = 0usize;
    for cell in &notebook.cells {
        match cell.cell_type {
            CellType::Markdown => {
                out.push_str(&cell.source);
                out.push_str("\n\n");
            }
            CellType::Code => {
                code_counter += 1;
                out.push_str(&format!("```sema\n{}\n```\n\n", cell.source));

                for output in &cell.outputs {
                    match output.output_type {
                        OutputType::Value if !output.display.is_empty() => {
                            out.push_str(&format!(
                                "**Out [{}]:**\n```\n{}\n```\n\n",
                                code_counter, output.display
                            ));
                        }
                        OutputType::Error => {
                            out.push_str(&format!(
                                "**Error [{}]:**\n```\n{}\n```\n\n",
                                code_counter, output.display
                            ));
                        }
                        OutputType::Stdout if !output.display.is_empty() => {
                            out.push_str(&format!(
                                "**Stdout [{}]:**\n```\n{}\n```\n\n",
                                code_counter, output.display
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    out
}

// ── JSON API response types ──────────────────────────────────────

/// API response for a cell evaluation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EvalResponse {
    pub id: String,
    pub output: RenderedOutput,
    /// Captured stdout from println/display/print calls.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    /// Whether the last cell evaluation can be undone.
    pub can_undo: bool,
}

/// API response for listing environment bindings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EnvResponse {
    pub bindings: BTreeMap<String, String>,
}

/// API response for listing all cells.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NotebookResponse {
    pub title: String,
    pub cells: Vec<RenderedCell>,
    /// Whether the last cell evaluation can be undone.
    pub can_undo: bool,
    /// Cumulative LLM cost of this engine session in USD; reset zeroes it.
    pub session_cost_usd: f64,
}

/// API response for undoing the last cell evaluation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UndoResponse {
    /// The cell whose evaluation was undone.
    pub undone_cell_id: String,
    /// Whether another undo is available.
    pub can_undo: bool,
}

/// API response data for a newly created cell.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CreateCellData {
    pub id: String,
    #[serde(flatten)]
    pub cell: RenderedCell,
}

/// Produce the full notebook response for the API.
pub fn notebook_response(
    notebook: &Notebook,
    can_undo: bool,
    session_cost_usd: f64,
) -> NotebookResponse {
    NotebookResponse {
        title: notebook.metadata.title.clone(),
        cells: render_notebook(notebook),
        can_undo,
        session_cost_usd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Notebook;

    #[test]
    fn render_notebook_numbers_code_cells() {
        let mut nb = Notebook::new("T");
        nb.add_code_cell("a");
        nb.add_markdown_cell("text");
        nb.add_code_cell("b");

        let rendered = render_notebook(&nb);
        assert_eq!(rendered.len(), 3);
        assert_eq!(rendered[0].cell_number, Some(1));
        assert_eq!(rendered[1].cell_number, None); // markdown
        assert_eq!(rendered[2].cell_number, Some(2));
    }

    #[test]
    fn export_markdown_basic() {
        let mut nb = Notebook::new("My Notebook");
        nb.add_markdown_cell("Some prose here.");
        nb.add_code_cell("(+ 1 2)");

        let md = export_markdown(&nb);
        assert!(md.contains("# My Notebook"));
        assert!(md.contains("Some prose here."));
        assert!(md.contains("```sema\n(+ 1 2)\n```"));
    }

    #[test]
    fn export_markdown_with_output() {
        let mut nb = Notebook::new("T");
        let id = nb.add_code_cell("(+ 1 2)");
        nb.cell_mut(&id).unwrap().outputs = vec![CellOutput {
            output_type: OutputType::Value,
            display: "3".to_string(),
            sema_value: None,
            timestamp: chrono::Utc::now(),
            cost_usd: None,
            usage: None,
            requires_reeval: false,
            duration_ms: None,
        }];

        let md = export_markdown(&nb);
        assert!(md.contains("**Out [1]:**"));
        assert!(md.contains("3"));
    }

    #[test]
    fn export_markdown_with_error() {
        let mut nb = Notebook::new("T");
        let id = nb.add_code_cell("(bad)");
        nb.cell_mut(&id).unwrap().outputs = vec![CellOutput {
            output_type: OutputType::Error,
            display: "unbound: bad".to_string(),
            sema_value: None,
            timestamp: chrono::Utc::now(),
            cost_usd: None,
            usage: None,
            requires_reeval: false,
            duration_ms: None,
        }];

        let md = export_markdown(&nb);
        assert!(md.contains("**Error [1]:**"));
        assert!(md.contains("unbound: bad"));
    }

    #[test]
    fn render_output_value() {
        let output = CellOutput {
            output_type: OutputType::Value,
            display: "42".to_string(),
            sema_value: Some("42".to_string()),
            timestamp: chrono::Utc::now(),
            cost_usd: None,
            usage: None,
            requires_reeval: false,
            duration_ms: Some(10),
        };
        let rendered = render_output(&output);
        assert_eq!(rendered.content, "42");
        assert_eq!(rendered.mime_type, "text/plain");
        assert_eq!(rendered.meta.duration_ms, Some(10));
    }

    #[test]
    fn render_output_error() {
        let output = CellOutput {
            output_type: OutputType::Error,
            display: "boom".to_string(),
            sema_value: None,
            timestamp: chrono::Utc::now(),
            cost_usd: None,
            usage: None,
            requires_reeval: false,
            duration_ms: None,
        };
        let rendered = render_output(&output);
        assert_eq!(rendered.mime_type, "text/x-sema-error");
    }

    #[test]
    fn notebook_response_includes_title() {
        let nb = Notebook::new("Hello");
        let resp = notebook_response(&nb, false, 0.0);
        assert_eq!(resp.title, "Hello");
        assert!(resp.cells.is_empty());
        assert!(!resp.can_undo);
    }
}
