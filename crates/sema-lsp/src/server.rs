//! The tower-lsp [`Backend`], the backend actor loop, request dispatch, and the
//! stdin/stdout transport setup (including LSP frame normalization).
//!
//! The backend runs on a dedicated `std::thread` that owns all `Rc`-based,
//! non-`Send` state ([`BackendState`]). The async tower-lsp `Backend` is a thin
//! shim: it converts each LSP method call into an [`LspRequest`] sent over an
//! mpsc channel and awaits a oneshot reply. This keeps the single-threaded
//! evaluator/parser state confined to one thread while still serving the async
//! LSP protocol.

use std::collections::HashMap;
use std::path::PathBuf;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::helpers::*;
use crate::scope;
use crate::state::{semantic_token_legend, BackendState, CachedParse, WorkspaceScanner};

// ── Backend thread messages ──────────────────────────────────────

pub(crate) enum LspRequest {
    /// Document opened or changed — reparse and publish diagnostics.
    DocumentChanged { uri: Url, text: String },
    /// Document closed — remove from cache and clear diagnostics.
    DocumentClosed { uri: Url },
    /// Completion request.
    Complete {
        uri: Url,
        position: Position,
        reply: tokio::sync::oneshot::Sender<Vec<CompletionItem>>,
    },
    /// Go-to-definition request.
    GotoDefinition {
        uri: Url,
        position: Position,
        reply: tokio::sync::oneshot::Sender<Option<GotoDefinitionResponse>>,
    },
    /// Hover request.
    Hover {
        uri: Url,
        position: Position,
        reply: tokio::sync::oneshot::Sender<Option<Hover>>,
    },
    /// CodeLens request.
    CodeLens {
        uri: Url,
        reply: tokio::sync::oneshot::Sender<Vec<CodeLens>>,
    },
    /// Find all references request.
    References {
        uri: Url,
        position: Position,
        reply: tokio::sync::oneshot::Sender<Vec<Location>>,
    },
    /// Document symbols request.
    DocumentSymbols {
        uri: Url,
        reply: tokio::sync::oneshot::Sender<DocumentSymbolResponse>,
    },
    /// Workspace symbols request.
    WorkspaceSymbols {
        query: String,
        #[allow(deprecated)]
        reply: tokio::sync::oneshot::Sender<Vec<SymbolInformation>>,
    },
    /// Signature help request.
    SignatureHelp {
        uri: Url,
        position: Position,
        reply: tokio::sync::oneshot::Sender<Option<SignatureHelp>>,
    },
    /// Rename request.
    Rename {
        uri: Url,
        position: Position,
        new_name: String,
        reply: tokio::sync::oneshot::Sender<Option<WorkspaceEdit>>,
    },
    /// Prepare rename request.
    PrepareRename {
        uri: Url,
        position: Position,
        reply: tokio::sync::oneshot::Sender<Option<PrepareRenameResponse>>,
    },
    /// Semantic tokens request.
    SemanticTokensFull {
        uri: Url,
        reply: tokio::sync::oneshot::Sender<Option<SemanticTokensResult>>,
    },
    /// Folding ranges request.
    FoldingRange {
        uri: Url,
        reply: tokio::sync::oneshot::Sender<Vec<FoldingRange>>,
    },
    /// Document formatting request.
    Formatting {
        uri: Url,
        options: FormattingOptions,
        reply: tokio::sync::oneshot::Sender<Option<Vec<TextEdit>>>,
    },
    /// Document range formatting request.
    RangeFormatting {
        uri: Url,
        range: Range,
        options: FormattingOptions,
        reply: tokio::sync::oneshot::Sender<Option<Vec<TextEdit>>>,
    },
    /// Selection range request (structural s-expression selection).
    SelectionRange {
        uri: Url,
        positions: Vec<Position>,
        reply: tokio::sync::oneshot::Sender<Option<Vec<SelectionRange>>>,
    },
    /// Document links (import/load path → file).
    DocumentLinks {
        uri: Url,
        reply: tokio::sync::oneshot::Sender<Option<Vec<DocumentLink>>>,
    },
    /// Prepare call hierarchy at a position.
    CallHierarchyPrepare {
        uri: Url,
        position: Position,
        reply: tokio::sync::oneshot::Sender<Option<Vec<CallHierarchyItem>>>,
    },
    /// Incoming calls for a call-hierarchy item.
    CallHierarchyIncoming {
        item: Box<CallHierarchyItem>,
        reply: tokio::sync::oneshot::Sender<Option<Vec<CallHierarchyIncomingCall>>>,
    },
    /// Outgoing calls for a call-hierarchy item.
    CallHierarchyOutgoing {
        item: Box<CallHierarchyItem>,
        reply: tokio::sync::oneshot::Sender<Option<Vec<CallHierarchyOutgoingCall>>>,
    },
    /// Resolve additional detail/documentation for a completion item.
    CompletionResolve {
        item: Box<CompletionItem>,
        reply: tokio::sync::oneshot::Sender<CompletionItem>,
    },
    /// Inlay hints request.
    InlayHints {
        uri: Url,
        range: Range,
        reply: tokio::sync::oneshot::Sender<Option<Vec<InlayHint>>>,
    },
    /// Document highlight request.
    DocumentHighlight {
        uri: Url,
        position: Position,
        reply: tokio::sync::oneshot::Sender<Option<Vec<DocumentHighlight>>>,
    },
    /// Execute command (sema.runTopLevel).
    ExecuteCommand {
        command: String,
        arguments: Vec<serde_json::Value>,
    },
    /// Set the sema binary path (from initializationOptions).
    SetSemaBinary { path: String },
    /// Scan workspace for .sema files (triggered on initialized).
    ScanWorkspace { root: PathBuf },
    /// Continue incremental workspace scanning (directory-by-directory with yielding).
    ScanWorkspaceContinue { scanner: WorkspaceScanner },
    /// Shutdown the backend thread.
    Shutdown,
}

// ── tower-lsp Backend ────────────────────────────────────────────

pub(crate) struct Backend {
    #[allow(dead_code)]
    pub(crate) client: Client,
    tx: tokio::sync::mpsc::UnboundedSender<LspRequest>,
    /// Workspace root extracted from InitializeParams, used for workspace scanning.
    workspace_root: tokio::sync::Mutex<Option<PathBuf>>,
}

impl Backend {
    fn new(client: Client, tx: tokio::sync::mpsc::UnboundedSender<LspRequest>) -> Self {
        Backend {
            client,
            tx,
            workspace_root: tokio::sync::Mutex::new(None),
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Extract sema binary path from initializationOptions
        if let Some(opts) = &params.initialization_options {
            if let Some(path) = opts.get("semaPath").and_then(|v| v.as_str()) {
                let _ = self.tx.send(LspRequest::SetSemaBinary {
                    path: path.to_string(),
                });
            }
        }

        // Store workspace root for scanning in `initialized`
        let root = params
            .root_uri
            .as_ref()
            .and_then(|uri| uri.to_file_path().ok())
            .or_else(|| {
                #[allow(deprecated)]
                params.root_path.as_ref().map(PathBuf::from)
            });
        *self.workspace_root.lock().await = root;

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec!["(".to_string(), " ".to_string()]),
                    resolve_provider: Some(true),
                    ..Default::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(false),
                }),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), " ".to_string()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
                }),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["sema.runTopLevel".to_string()],
                    ..Default::default()
                }),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: semantic_token_legend(),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            range: None,
                            work_done_progress_options: Default::default(),
                        },
                    ),
                ),
                document_highlight_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(true)),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                declaration_provider: Some(DeclarationCapability::Simple(true)),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(false),
                    work_done_progress_options: Default::default(),
                }),
                call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // Scan workspace for .sema files to populate the definition cache
        if let Some(root) = self.workspace_root.lock().await.take() {
            let _ = self.tx.send(LspRequest::ScanWorkspace { root });
        }
    }

    async fn shutdown(&self) -> Result<()> {
        let _ = self.tx.send(LspRequest::Shutdown);
        // TODO(tower-lsp#399): drop this once the upstream bug is fixed.
        // See https://github.com/ebkalderon/tower-lsp/issues/399 — without this
        // force-exit, the LSP server can hang after shutdown when pending writes
        // or cache flushes haven't been flushed.
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            std::process::exit(0);
        });
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let _ = self.tx.send(LspRequest::DocumentChanged { uri, text });
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        // We use FULL sync, so there's exactly one content change with the full text.
        if let Some(change) = params.content_changes.into_iter().last() {
            let _ = self.tx.send(LspRequest::DocumentChanged {
                uri,
                text: change.text,
            });
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let _ = self.tx.send(LspRequest::DocumentClosed {
            uri: params.text_document.uri,
        });
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::Complete {
            uri,
            position,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(items) => Ok(Some(CompletionResponse::Array(items))),
            Err(_) => Ok(None),
        }
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::GotoDefinition {
            uri,
            position,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(response) => Ok(response),
            Err(_) => Ok(None),
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::Hover {
            uri,
            position,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(response) => Ok(response),
            Err(_) => Ok(None),
        }
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::References {
            uri,
            position,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(locations) if locations.is_empty() => Ok(None),
            Ok(locations) => Ok(Some(locations)),
            Err(_) => Ok(None),
        }
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::DocumentHighlight {
            uri: params.text_document_position_params.text_document.uri,
            position: params.text_document_position_params.position,
            reply: tx,
        });
        Ok(rx.await.unwrap_or(None))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::InlayHints {
            uri: params.text_document.uri,
            range: params.range,
            reply: tx,
        });
        Ok(rx.await.unwrap_or(None))
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = params.text_document.uri;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::CodeLens {
            uri,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(lenses) => Ok(Some(lenses)),
            Err(_) => Ok(None),
        }
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        let _ = self.tx.send(LspRequest::ExecuteCommand {
            command: params.command,
            arguments: params.arguments,
        });
        Ok(None)
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::DocumentSymbols {
            uri,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(response) => Ok(Some(response)),
            Err(_) => Ok(None),
        }
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::WorkspaceSymbols {
            query: params.query,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(symbols) if symbols.is_empty() => Ok(None),
            Ok(symbols) => Ok(Some(symbols)),
            Err(_) => Ok(None),
        }
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::SignatureHelp {
            uri,
            position,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(response) => Ok(response),
            Err(_) => Ok(None),
        }
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = params.new_name;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::Rename {
            uri,
            position,
            new_name,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(response) => Ok(response),
            Err(_) => Ok(None),
        }
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let position = params.position;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::PrepareRename {
            uri,
            position,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(response) => Ok(response),
            Err(_) => Ok(None),
        }
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::SemanticTokensFull {
            uri,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(response) => Ok(response),
            Err(_) => Ok(None),
        }
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = params.text_document.uri;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::FoldingRange {
            uri,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(ranges) => Ok(Some(ranges)),
            Err(_) => Ok(None),
        }
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::Formatting {
            uri,
            options: params.options,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(edits) => Ok(edits),
            Err(_) => Ok(None),
        }
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::RangeFormatting {
            uri,
            range: params.range,
            options: params.options,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(edits) => Ok(edits),
            Err(_) => Ok(None),
        }
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let uri = params.text_document.uri;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::SelectionRange {
            uri,
            positions: params.positions,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(ranges) => Ok(ranges),
            Err(_) => Ok(None),
        }
    }

    async fn goto_declaration(
        &self,
        params: request::GotoDeclarationParams,
    ) -> Result<Option<request::GotoDeclarationResponse>> {
        // For Sema there is no separate forward declaration — declaration == definition.
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::GotoDefinition {
            uri,
            position,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(response) => Ok(response),
            Err(_) => Ok(None),
        }
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = params.text_document.uri;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::DocumentLinks {
            uri,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(links) => Ok(links),
            Err(_) => Ok(None),
        }
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::CallHierarchyPrepare {
            uri,
            position,
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(items) => Ok(items),
            Err(_) => Ok(None),
        }
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::CallHierarchyIncoming {
            item: Box::new(params.item),
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(calls) => Ok(calls),
            Err(_) => Ok(None),
        }
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(LspRequest::CallHierarchyOutgoing {
            item: Box::new(params.item),
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(calls) => Ok(calls),
            Err(_) => Ok(None),
        }
    }

    async fn completion_resolve(&self, item: CompletionItem) -> Result<CompletionItem> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let fallback = item.clone();
        let _ = self.tx.send(LspRequest::CompletionResolve {
            item: Box::new(item),
            reply: reply_tx,
        });

        match reply_rx.await {
            Ok(resolved) => Ok(resolved),
            // If the backend is unavailable, return the item unchanged rather than failing.
            Err(_) => Ok(fallback),
        }
    }
}

// ── Server entry point ───────────────────────────────────────────

pub async fn run_server() {
    let raw_stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (stdin, mut stdin_writer) = tokio::io::duplex(64 * 1024);

    tokio::spawn(async move {
        let _ = normalize_lsp_input(raw_stdin, &mut stdin_writer).await;
    });

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<LspRequest>();

    let (service, socket) = LspService::new(|client| Backend::new(client, tx.clone()));

    // Extract the client for publishing diagnostics from the backend thread.
    let client = service.inner().client.clone();

    // Capture a handle to the tokio runtime so the backend thread can call async methods.
    let handle = tokio::runtime::Handle::current();

    // Spawn the backend thread (owns Rc / non-Send state).
    let backend_handle = std::thread::spawn(move || {
        let mut state = BackendState::new();
        // Deferred messages from document change batching, processed
        // before reading from the channel to preserve ordering.
        let mut deferred: std::collections::VecDeque<LspRequest> =
            std::collections::VecDeque::new();

        while let Some(req) = if deferred.is_empty() {
            rx.blocking_recv()
        } else {
            // Process deferred DocumentChanged events first to ensure
            // interactive requests always see the latest AST. Only
            // yield to new interactive requests when the deferred queue
            // contains non-document-change items (e.g. scan continuations).
            let front_is_doc_change =
                matches!(deferred.front(), Some(LspRequest::DocumentChanged { .. }));
            if front_is_doc_change {
                // Must process document updates before any interactive
                // request to prevent stale-AST responses.
                deferred.pop_front()
            } else {
                // Deferred item is a scan continuation or similar low-priority
                // work — yield to interactive requests if any arrived.
                match rx.try_recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => deferred.pop_front(),
                }
            }
        } {
            match req {
                LspRequest::DocumentChanged { uri, text } => {
                    // Batch document changes: drain any consecutive pending
                    // changes for the same URI so we only parse the latest
                    // version. Stops as soon as a non-matching message appears,
                    // preserving message ordering.
                    let (uri, text) = {
                        let latest_uri = uri;
                        let mut latest_text = text;
                        loop {
                            match rx.try_recv() {
                                Ok(LspRequest::DocumentChanged { uri: u, text: t })
                                    if u == latest_uri =>
                                {
                                    latest_text = t;
                                }
                                Ok(other) => {
                                    // Non-matching message: push it to the front
                                    // of the deferred queue and process it next,
                                    // preserving strict ordering.
                                    deferred.push_front(other);
                                    break;
                                }
                                Err(_) => break,
                            }
                        }
                        (latest_uri, latest_text)
                    };

                    // Parse once, cache the result, and derive diagnostics
                    let (ast, span_map, symbol_spans, errors) =
                        sema_reader::read_many_with_spans_recover(&text);
                    let lines: Vec<&str> = text.lines().collect();

                    let mut diags: Vec<Diagnostic> = errors
                        .iter()
                        .map(|err| error_diagnostic(err, DiagnosticSeverity::ERROR, &lines))
                        .collect();
                    if diags.is_empty() {
                        diags.extend(compile_diagnostics(&ast, &lines));
                    }

                    let uri_str = uri.as_str().to_string();
                    // Names only here; ranges discarded → &[] skips UTF-16 mapping.
                    let defs: Vec<String> =
                        user_definitions_from_ast(&ast, &span_map, &symbol_spans, &[])
                            .into_iter()
                            .map(|(name, _)| name)
                            .collect();
                    if !defs.is_empty() || diags.is_empty() {
                        state.cached_user_defs.insert(uri_str.clone(), defs);
                    }

                    // Drop quoted (data) symbol occurrences so rename/references/highlight
                    // never rewrite quoted literals (a silent program-meaning change).
                    let symbol_spans = filter_quoted_symbol_spans(&ast, &span_map, symbol_spans);
                    let scope_tree = scope::ScopeTree::build(&ast, &span_map, &symbol_spans);
                    state.cached_parses.insert(
                        uri_str.clone(),
                        CachedParse {
                            ast,
                            span_map,
                            symbol_spans,
                            scope_tree,
                            source: text.clone(),
                        },
                    );
                    state.documents.insert(uri_str, text);

                    let client = client.clone();
                    handle.block_on(async {
                        client.publish_diagnostics(uri, diags, None).await;
                    });
                }
                LspRequest::DocumentClosed { uri } => {
                    state.documents.remove(uri.as_str());
                    state.cached_user_defs.remove(uri.as_str());
                    state.cached_parses.remove(uri.as_str());

                    let client = client.clone();
                    handle.block_on(async {
                        client.publish_diagnostics(uri, vec![], None).await;
                    });
                }
                LspRequest::Complete {
                    uri,
                    position,
                    reply,
                } => {
                    let items = state.handle_complete(&uri, &position);
                    let _ = reply.send(items);
                }
                LspRequest::GotoDefinition {
                    uri,
                    position,
                    reply,
                } => {
                    let result = state.handle_goto_definition(&uri, &position);
                    let _ = reply.send(result);
                }
                LspRequest::Hover {
                    uri,
                    position,
                    reply,
                } => {
                    let result = state.handle_hover(&uri, &position);
                    let _ = reply.send(result);
                }
                LspRequest::CodeLens { uri, reply } => {
                    let lenses = state.handle_code_lens(&uri);
                    let _ = reply.send(lenses);
                }
                LspRequest::References {
                    uri,
                    position,
                    reply,
                } => {
                    let result = state.handle_references(&uri, &position);
                    let _ = reply.send(result);
                }
                LspRequest::DocumentSymbols { uri, reply } => {
                    let result = state.handle_document_symbols(&uri);
                    let _ = reply.send(result);
                }
                LspRequest::WorkspaceSymbols { query, reply } => {
                    let result = state.handle_workspace_symbols(&query);
                    let _ = reply.send(result);
                }
                LspRequest::SignatureHelp {
                    uri,
                    position,
                    reply,
                } => {
                    let result = state.handle_signature_help(&uri, &position);
                    let _ = reply.send(result);
                }
                LspRequest::Rename {
                    uri,
                    position,
                    new_name,
                    reply,
                } => {
                    let result = state.handle_rename(&uri, &position, &new_name);
                    let _ = reply.send(result);
                }
                LspRequest::PrepareRename {
                    uri,
                    position,
                    reply,
                } => {
                    let result = state.handle_prepare_rename(&uri, &position);
                    let _ = reply.send(result);
                }
                LspRequest::SemanticTokensFull { uri, reply } => {
                    let result = state.handle_semantic_tokens_full(&uri);
                    let _ = reply.send(result);
                }
                LspRequest::FoldingRange { uri, reply } => {
                    let result = state.handle_folding_ranges(&uri);
                    let _ = reply.send(result);
                }
                LspRequest::Formatting {
                    uri,
                    options,
                    reply,
                } => {
                    let result = state.handle_formatting(&uri, &options);
                    let _ = reply.send(result);
                }
                LspRequest::RangeFormatting {
                    uri,
                    range,
                    options,
                    reply,
                } => {
                    let result = state.handle_range_formatting(&uri, &range, &options);
                    let _ = reply.send(result);
                }
                LspRequest::SelectionRange {
                    uri,
                    positions,
                    reply,
                } => {
                    let result = state.handle_selection_range(&uri, &positions);
                    let _ = reply.send(result);
                }
                LspRequest::DocumentLinks { uri, reply } => {
                    let result = state.handle_document_links(&uri);
                    let _ = reply.send(result);
                }
                LspRequest::CallHierarchyPrepare {
                    uri,
                    position,
                    reply,
                } => {
                    let result = state.handle_call_hierarchy_prepare(&uri, &position);
                    let _ = reply.send(result);
                }
                LspRequest::CallHierarchyIncoming { item, reply } => {
                    let result = state.handle_call_hierarchy_incoming(&item);
                    let _ = reply.send(result);
                }
                LspRequest::CallHierarchyOutgoing { item, reply } => {
                    let result = state.handle_call_hierarchy_outgoing(&item);
                    let _ = reply.send(result);
                }
                LspRequest::CompletionResolve { item, reply } => {
                    let result = state.handle_completion_resolve(*item);
                    let _ = reply.send(result);
                }
                LspRequest::DocumentHighlight {
                    uri,
                    position,
                    reply,
                } => {
                    let result = state.handle_document_highlight(&uri, &position);
                    let _ = reply.send(result);
                }
                LspRequest::InlayHints { uri, range, reply } => {
                    let result = state.handle_inlay_hints(&uri, &range);
                    let _ = reply.send(result);
                }
                LspRequest::ExecuteCommand { command, arguments } => {
                    // Run subprocess on a separate thread to avoid blocking
                    // the backend (which would freeze diagnostics/completions).
                    // Only clone the document text needed for this command.
                    let target_uri = arguments
                        .first()
                        .and_then(|a| a.get("uri"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let mut docs = HashMap::new();
                    if let Some(text) = state.documents.get(target_uri) {
                        docs.insert(target_uri.to_string(), text.clone());
                    }
                    let sema_binary = state.sema_binary.clone();
                    let client = client.clone();
                    let handle = handle.clone();
                    std::thread::spawn(move || {
                        let tmp = BackendState::new_without_builtins(docs, sema_binary);
                        tmp.handle_execute_command(&command, &arguments, &client, &handle);
                    });
                }
                LspRequest::SetSemaBinary { path } => {
                    state.sema_binary = path;
                }
                LspRequest::ScanWorkspace { root } => {
                    // Start incremental workspace scanning. The scanner
                    // processes one directory at a time, yielding to
                    // interactive requests between directories.
                    let scanner = WorkspaceScanner::new(&root);
                    deferred.push_back(LspRequest::ScanWorkspaceContinue { scanner });
                }
                LspRequest::ScanWorkspaceContinue { mut scanner } => {
                    const BATCH_SIZE: usize = 10;

                    // Process pending files first (from a previous large directory)
                    // before discovering new directories, so files are parsed in
                    // the order they're discovered.
                    if !scanner.pending_files.is_empty() {
                        let to_parse = scanner.pending_files.len().min(BATCH_SIZE);
                        let batch: Vec<PathBuf> = scanner.pending_files.drain(..to_parse).collect();
                        for path in &batch {
                            let _ = state.get_import_cache(path);
                        }
                    } else if let Some(files) = scanner.next_dir() {
                        let to_parse = files.len().min(BATCH_SIZE);
                        for path in &files[..to_parse] {
                            let _ = state.get_import_cache(path);
                        }
                        if to_parse < files.len() {
                            scanner.pending_files = files[to_parse..].to_vec();
                        }
                    }

                    // Re-enqueue if more directories or files remain.
                    if !scanner.dir_stack.is_empty() || !scanner.pending_files.is_empty() {
                        deferred.push_back(LspRequest::ScanWorkspaceContinue { scanner });
                    }
                }
                LspRequest::Shutdown => break,
            }
        }
    });

    Server::new(stdin, stdout, socket).serve(service).await;

    // Wait for backend thread to finish.
    let _ = backend_handle.join();
}

async fn normalize_lsp_input<R, W>(mut input: R, mut output: W) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut pending = Vec::new();
    let mut chunk = [0; 8192];

    loop {
        let read = input.read(&mut chunk).await?;
        if read == 0 {
            if !pending.is_empty() {
                output.write_all(&pending).await?;
            }
            output.shutdown().await?;
            return Ok(());
        }

        pending.extend_from_slice(&chunk[..read]);

        while let Some(separator) = find_subslice(&pending, b"\r\n\r\n") {
            let body_start = separator + 4;
            let Some(content_length) = lsp_content_length(&pending[..separator]) else {
                output.write_all(&pending).await?;
                pending.clear();
                break;
            };
            let frame_len = body_start + content_length;
            if pending.len() < frame_len {
                break;
            }

            let frame = pending[..frame_len].to_vec();
            pending.drain(..frame_len);

            let body = &frame[body_start..];
            let normalized = normalize_lsp_message_body(body);
            if normalized == body {
                output.write_all(&frame).await?;
            } else {
                let header = format!("Content-Length: {}\r\n\r\n", normalized.len());
                output.write_all(header.as_bytes()).await?;
                output.write_all(&normalized).await?;
            }
        }
    }
}

pub(crate) fn normalize_lsp_message_body(body: &[u8]) -> Vec<u8> {
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return body.to_vec();
    };

    let Some(object) = value.as_object_mut() else {
        return body.to_vec();
    };

    let is_shutdown = object
        .get("method")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|method| method == "shutdown");
    let has_null_params = object.get("params").is_some_and(serde_json::Value::is_null);

    if is_shutdown && has_null_params {
        object.remove("params");
        serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec())
    } else {
        body.to_vec()
    }
}

fn lsp_content_length(header: &[u8]) -> Option<usize> {
    std::str::from_utf8(header).ok()?.lines().find_map(|line| {
        line.strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse().ok())
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
