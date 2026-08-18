//! Engine bridge — async API for communicating with the `!Send` interpreter.
//!
//! The Sema interpreter uses `Rc` and is single-threaded. This module
//! bridges async callers (like the HTTP server) to the interpreter via
//! a dedicated OS thread and an mpsc channel. All evaluation happens on
//! the engine thread; callers get typed async methods that hide the
//! channel plumbing.

use std::fmt;
use std::path::PathBuf;

use tokio::sync::{mpsc, oneshot};

use crate::format::{CellType, Notebook};
use crate::render;

// ── Error type ──────────────────────────────────────────────────

/// Errors returned by [`EngineHandle`] methods.
#[derive(Debug)]
pub enum BridgeError {
    /// The engine thread has exited or the channel is closed.
    EngineDown,
    /// The engine processed the request but returned an error.
    Request(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeError::EngineDown => write!(f, "Engine thread died"),
            BridgeError::Request(msg) => write!(f, "{msg}"),
        }
    }
}

// ── Internal request enum (private) ─────────────────────────────

type EngineResult<T> = Result<T, String>;

enum EngineRequest {
    GetNotebook {
        reply: oneshot::Sender<render::NotebookResponse>,
    },
    CreateCell {
        cell_type: CellType,
        source: String,
        after: Option<String>,
        reply: oneshot::Sender<EngineResult<render::CreateCellData>>,
    },
    GetCell {
        id: String,
        reply: oneshot::Sender<EngineResult<render::RenderedCell>>,
    },
    UpdateCell {
        id: String,
        source: Option<String>,
        cell_type: Option<String>,
        reply: oneshot::Sender<EngineResult<render::RenderedCell>>,
    },
    EvalCell {
        id: String,
        reply: oneshot::Sender<EngineResult<render::EvalResponse>>,
    },
    DeleteCell {
        id: String,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    ReorderCells {
        cell_ids: Vec<String>,
        reply: oneshot::Sender<EngineResult<()>>,
    },
    EvalAll {
        /// Optional map of cell_id -> source to sync before evaluating.
        sources: Vec<(String, String)>,
        reply: oneshot::Sender<Vec<render::EvalResponse>>,
    },
    GetEnv {
        reply: oneshot::Sender<render::EnvResponse>,
    },
    Reset {
        reply: oneshot::Sender<()>,
    },
    SetTitle {
        title: String,
        reply: oneshot::Sender<()>,
    },
    Save {
        reply: oneshot::Sender<EngineResult<String>>,
    },
    UndoCell {
        reply: oneshot::Sender<EngineResult<render::UndoResponse>>,
    },
}

// ── Engine handle ───────────────────────────────────────────────

/// Async handle to the notebook engine running on a dedicated thread.
pub struct EngineHandle {
    tx: mpsc::Sender<EngineRequest>,
}

impl EngineHandle {
    /// Spawn the engine on a dedicated OS thread and return a handle.
    ///
    /// Returns an error if the tokio runtime that drives the engine thread
    /// cannot be built. Building the runtime here (rather than inside the
    /// spawned thread) lets the failure surface to the caller instead of
    /// panicking on a detached thread and leaving a silent, dead server.
    pub fn spawn(notebook: Notebook, notebook_path: Option<PathBuf>) -> Result<Self, String> {
        let (tx, mut rx) = mpsc::channel::<EngineRequest>(64);

        // Build the runtime up front so a build failure is reported
        // synchronously to the caller rather than panicking on the detached
        // engine thread (which would leave a silent, dead server).
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .map_err(|e| format!("Failed to build notebook engine runtime: {e}"))?;

        // A LARGE stack, explicitly. Evaluation recurses over the expression
        // tree, and `MAX_RESOLVE_DEPTH` (256) is calibrated against a main-
        // thread-sized stack — `resolve.rs`'s own depth test spawns 8MiB for
        // exactly this reason. A default `std::thread::spawn` gets ~2MiB, so a
        // cell nested only ~200 deep overflowed and took the whole SERVER down
        // ("fatal runtime error: stack overflow, aborting") before the guard
        // could report the error. That loses every unsaved cell in the
        // notebook, since saving is explicit. The same input through the CLI
        // reports "maximum resolution depth exceeded" and carries on.
        std::thread::Builder::new()
            .name("sema-notebook-engine".to_string())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                use crate::engine::Engine;

                let mut engine = Engine::new(notebook);
                let nb_path = notebook_path;

                while let Some(req) = rt.block_on(rx.recv()) {
                    match req {
                        EngineRequest::GetNotebook { reply } => {
                            let _ = reply.send(render::notebook_response(
                                &engine.notebook,
                                engine.can_undo(),
                                sema_llm::builtins::session_cost_snapshot(),
                            ));
                        }
                        EngineRequest::CreateCell {
                            cell_type,
                            source,
                            after,
                            reply,
                        } => {
                            let result = (|| {
                                let id = match cell_type {
                                    CellType::Code => engine.notebook.add_code_cell(&source),
                                    CellType::Markdown => {
                                        engine.notebook.add_markdown_cell(&source)
                                    }
                                };
                                if let Some(after_id) = &after {
                                    let after_idx = engine
                                        .notebook
                                        .cell_index(after_id)
                                        .ok_or_else(|| format!("Cell not found: {after_id}"))?;
                                    let new_idx = engine.notebook.cell_index(&id).unwrap();
                                    let cell = engine.notebook.cells.remove(new_idx);
                                    let insert_at = if after_idx < new_idx {
                                        after_idx + 1
                                    } else {
                                        after_idx
                                    };
                                    engine.notebook.cells.insert(insert_at, cell);
                                }
                                let cell = engine.notebook.cell(&id).unwrap();
                                let cell_number = engine.notebook.cell_number(&id);
                                let rendered = render::render_cell(cell, cell_number);
                                Ok(render::CreateCellData { id, cell: rendered })
                            })();
                            let _ = reply.send(result);
                        }
                        EngineRequest::GetCell { id, reply } => {
                            let result = match engine.notebook.cell(&id) {
                                Some(cell) => {
                                    let cell_number = engine.notebook.cell_number(&id);
                                    Ok(render::render_cell(cell, cell_number))
                                }
                                None => Err(format!("Cell not found: {id}")),
                            };
                            let _ = reply.send(result);
                        }
                        EngineRequest::UpdateCell {
                            id,
                            source,
                            cell_type,
                            reply,
                        } => {
                            let result = (|| {
                                let cell = engine
                                    .notebook
                                    .cell_mut(&id)
                                    .ok_or_else(|| format!("Cell not found: {id}"))?;
                                if let Some(s) = source {
                                    // React only to a genuine change. The UI re-syncs
                                    // every cell's source on blur and before save, so
                                    // treating an identical resend as an edit would
                                    // spuriously mark evaluated cells stale.
                                    if cell.source != s {
                                        cell.source = s;
                                        if !cell.outputs.is_empty() {
                                            cell.stale = true;
                                        }
                                    }
                                }
                                if let Some(ct) = cell_type {
                                    cell.cell_type = match ct.as_str() {
                                        "code" => CellType::Code,
                                        "markdown" => CellType::Markdown,
                                        other => return Err(format!("Unknown type: {other}")),
                                    };
                                }
                                let cell = engine.notebook.cell(&id).unwrap();
                                let cell_number = engine.notebook.cell_number(&id);
                                Ok(render::render_cell(cell, cell_number))
                            })();
                            let _ = reply.send(result);
                        }
                        EngineRequest::EvalCell { id, reply } => {
                            let result = engine.eval_cell(&id).map(|r| {
                                let output = render::render_output(&r.output);
                                render::EvalResponse {
                                    id: id.clone(),
                                    output,
                                    stdout: r.stdout,
                                    can_undo: engine.can_undo(),
                                }
                            });
                            let _ = reply.send(result);
                        }
                        EngineRequest::DeleteCell { id, reply } => {
                            let result = match engine.notebook.cell_index(&id) {
                                Some(idx) => {
                                    engine.notebook.cells.remove(idx);
                                    Ok(())
                                }
                                None => Err(format!("Cell not found: {id}")),
                            };
                            let _ = reply.send(result);
                        }
                        EngineRequest::ReorderCells { cell_ids, reply } => {
                            let result = (|| {
                                // Reject duplicate IDs up front: a duplicate would clone
                                // the same cell into the result twice while a real cell
                                // gets dropped, permanently corrupting the notebook.
                                let mut seen =
                                    std::collections::HashSet::with_capacity(cell_ids.len());
                                for id in &cell_ids {
                                    if !seen.insert(id.as_str()) {
                                        return Err(format!("Duplicate cell ID in reorder: {id}"));
                                    }
                                }
                                let mut reordered = Vec::with_capacity(engine.notebook.cells.len());
                                for id in &cell_ids {
                                    let idx = engine
                                        .notebook
                                        .cell_index(id)
                                        .ok_or_else(|| format!("Cell not found: {id}"))?;
                                    reordered.push(engine.notebook.cells[idx].clone());
                                }
                                for cell in &engine.notebook.cells {
                                    if !seen.contains(cell.id.as_str()) {
                                        reordered.push(cell.clone());
                                    }
                                }
                                engine.notebook.cells = reordered;
                                Ok(())
                            })();
                            let _ = reply.send(result);
                        }
                        EngineRequest::EvalAll { sources, reply } => {
                            for (id, source) in sources {
                                if let Some(cell) = engine.notebook.cell_mut(&id) {
                                    cell.source = source;
                                }
                            }
                            let results = engine.eval_all();
                            let can_undo = engine.can_undo();
                            let responses = results
                                .into_iter()
                                .map(|(id, result)| {
                                    let (output, stdout) = match result {
                                        Ok(r) => (render::render_output(&r.output), r.stdout),
                                        Err(e) => (
                                            render::RenderedOutput {
                                                output_type: crate::format::OutputType::Error,
                                                content: e,
                                                mime_type: "text/x-sema-error".to_string(),
                                                meta: render::OutputMeta::default(),
                                            },
                                            String::new(),
                                        ),
                                    };
                                    render::EvalResponse {
                                        id,
                                        output,
                                        stdout,
                                        can_undo,
                                    }
                                })
                                .collect();
                            let _ = reply.send(responses);
                        }
                        EngineRequest::GetEnv { reply } => {
                            let _ = reply.send(render::EnvResponse {
                                bindings: engine.env_bindings(),
                            });
                        }
                        EngineRequest::Reset { reply } => {
                            engine.reset();
                            let _ = reply.send(());
                        }
                        EngineRequest::SetTitle { title, reply } => {
                            engine.notebook.metadata.title = title;
                            let _ = reply.send(());
                        }
                        EngineRequest::Save { reply } => {
                            let result = match &nb_path {
                                Some(path) => engine
                                    .notebook
                                    .save(path)
                                    .map(|_| path.display().to_string()),
                                None => Err("No file path".to_string()),
                            };
                            let _ = reply.send(result);
                        }
                        EngineRequest::UndoCell { reply } => {
                            let result = engine.undo_last_cell().map(|info| render::UndoResponse {
                                undone_cell_id: info.cell_id,
                                can_undo: engine.can_undo(),
                            });
                            let _ = reply.send(result);
                        }
                    }
                }
            })
            .map_err(|e| format!("Failed to spawn notebook engine thread: {e}"))?;

        Ok(Self { tx })
    }

    // ── Private send helper ─────────────────────────────────────

    async fn send<T>(
        &self,
        make_req: impl FnOnce(oneshot::Sender<T>) -> EngineRequest,
    ) -> Result<T, BridgeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(make_req(reply_tx))
            .await
            .map_err(|_| BridgeError::EngineDown)?;
        reply_rx.await.map_err(|_| BridgeError::EngineDown)
    }

    // ── Public typed methods ────────────────────────────────────

    pub async fn get_notebook(&self) -> Result<render::NotebookResponse, BridgeError> {
        self.send(|reply| EngineRequest::GetNotebook { reply })
            .await
    }

    pub async fn create_cell(
        &self,
        cell_type: CellType,
        source: String,
        after: Option<String>,
    ) -> Result<render::CreateCellData, BridgeError> {
        self.send(|reply| EngineRequest::CreateCell {
            cell_type,
            source,
            after,
            reply,
        })
        .await?
        .map_err(BridgeError::Request)
    }

    pub async fn get_cell(&self, id: String) -> Result<render::RenderedCell, BridgeError> {
        self.send(|reply| EngineRequest::GetCell { id, reply })
            .await?
            .map_err(BridgeError::Request)
    }

    pub async fn update_cell(
        &self,
        id: String,
        source: Option<String>,
        cell_type: Option<String>,
    ) -> Result<render::RenderedCell, BridgeError> {
        self.send(|reply| EngineRequest::UpdateCell {
            id,
            source,
            cell_type,
            reply,
        })
        .await?
        .map_err(BridgeError::Request)
    }

    pub async fn eval_cell(&self, id: String) -> Result<render::EvalResponse, BridgeError> {
        self.send(|reply| EngineRequest::EvalCell { id, reply })
            .await?
            .map_err(BridgeError::Request)
    }

    pub async fn delete_cell(&self, id: String) -> Result<(), BridgeError> {
        self.send(|reply| EngineRequest::DeleteCell { id, reply })
            .await?
            .map_err(BridgeError::Request)
    }

    pub async fn reorder_cells(&self, cell_ids: Vec<String>) -> Result<(), BridgeError> {
        self.send(|reply| EngineRequest::ReorderCells { cell_ids, reply })
            .await?
            .map_err(BridgeError::Request)
    }

    pub async fn eval_all(
        &self,
        sources: Vec<(String, String)>,
    ) -> Result<Vec<render::EvalResponse>, BridgeError> {
        self.send(|reply| EngineRequest::EvalAll { sources, reply })
            .await
    }

    pub async fn get_env(&self) -> Result<render::EnvResponse, BridgeError> {
        self.send(|reply| EngineRequest::GetEnv { reply }).await
    }

    pub async fn reset(&self) -> Result<(), BridgeError> {
        self.send(|reply| EngineRequest::Reset { reply }).await
    }

    pub async fn set_title(&self, title: String) -> Result<(), BridgeError> {
        self.send(|reply| EngineRequest::SetTitle { title, reply })
            .await
    }

    pub async fn save(&self) -> Result<String, BridgeError> {
        self.send(|reply| EngineRequest::Save { reply })
            .await?
            .map_err(BridgeError::Request)
    }

    pub async fn undo_cell(&self) -> Result<render::UndoResponse, BridgeError> {
        self.send(|reply| EngineRequest::UndoCell { reply })
            .await?
            .map_err(BridgeError::Request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a handle over a notebook seeded with three code cells.
    /// Returns the handle plus the (a, b, c) cell IDs.
    fn handle_with_three_cells() -> (EngineHandle, String, String, String) {
        let mut nb = Notebook::new("Reorder Test");
        let a = nb.add_code_cell("(+ 1 1)");
        let b = nb.add_code_cell("(+ 2 2)");
        let c = nb.add_code_cell("(+ 3 3)");
        let handle = EngineHandle::spawn(nb, None).expect("engine runtime should build");
        (handle, a, b, c)
    }

    async fn cell_ids(handle: &EngineHandle) -> Vec<String> {
        handle
            .get_notebook()
            .await
            .expect("get_notebook")
            .cells
            .into_iter()
            .map(|c| c.id)
            .collect()
    }

    #[tokio::test]
    async fn reorder_valid_permutation_reorders_cells() {
        let (handle, a, b, c) = handle_with_three_cells();
        handle
            .reorder_cells(vec![c.clone(), a.clone(), b.clone()])
            .await
            .expect("valid reorder should succeed");
        assert_eq!(cell_ids(&handle).await, vec![c, a, b]);
    }

    #[tokio::test]
    async fn reorder_partial_list_keeps_unlisted_cells() {
        let (handle, a, b, c) = handle_with_three_cells();
        // Only mention the last cell; the rest must be preserved in place.
        handle
            .reorder_cells(vec![c.clone()])
            .await
            .expect("partial reorder should succeed");
        assert_eq!(cell_ids(&handle).await, vec![c, a, b]);
    }

    #[tokio::test]
    async fn reorder_with_duplicate_ids_is_rejected_without_corruption() {
        let (handle, a, b, c) = handle_with_three_cells();
        let err = handle
            .reorder_cells(vec![a.clone(), a.clone()])
            .await
            .expect_err("duplicate IDs must be rejected");
        match err {
            BridgeError::Request(msg) => assert!(
                msg.contains("Duplicate cell ID"),
                "unexpected error message: {msg}"
            ),
            other => panic!("expected Request error, got {other:?}"),
        }
        // The notebook must be untouched: same cells, same order, no drops/dupes.
        assert_eq!(cell_ids(&handle).await, vec![a, b, c]);
    }

    #[tokio::test]
    async fn reorder_with_unknown_id_is_rejected_without_corruption() {
        let (handle, a, b, c) = handle_with_three_cells();
        let err = handle
            .reorder_cells(vec![a.clone(), "does-not-exist".to_string()])
            .await
            .expect_err("unknown IDs must be rejected");
        match err {
            BridgeError::Request(msg) => assert!(
                msg.contains("Cell not found"),
                "unexpected error message: {msg}"
            ),
            other => panic!("expected Request error, got {other:?}"),
        }
        // The notebook must remain intact in original order.
        assert_eq!(cell_ids(&handle).await, vec![a, b, c]);
    }
}
