//! Structural and informational endpoints: code lens, document/workspace
//! symbols, signature help, folding ranges, selection ranges, document links,
//! call hierarchy, and inlay hints.

use tower_lsp::lsp_types::*;

use sema_core::SpanMap;

use crate::builtin_docs;
use crate::definitions::*;
use crate::helpers::*;
use crate::state::{build_selection_range, BackendState};

fn position_in_selection_range(position: &Position, range: &Range) -> bool {
    let position = (position.line, position.character);
    let start = (range.start.line, range.start.character);
    let end = (range.end.line, range.end.character);
    position >= start && (position < end || start == end)
}

impl BackendState {
    pub(crate) fn handle_code_lens(&self, uri: &Url) -> Vec<CodeLens> {
        let uri_str = uri.as_str();

        // Prefer the cached parse populated by didChange; only re-parse if
        // the cache misses (should be rare for open documents).
        let indexed_ranges = if let Some(cached) = self.cached_parses.get(uri_str) {
            let lines: Vec<&str> = cached.source.lines().collect();
            top_level_ranges(&cached.ast, &cached.span_map, &lines)
        } else {
            let text = match self.documents.get(uri_str) {
                Some(t) => t,
                None => return vec![],
            };
            let lines: Vec<&str> = text.lines().collect();
            let (exprs, span_map) = match sema_reader::read_many_with_spans(text) {
                Ok(r) => r,
                Err(_) => return vec![],
            };
            top_level_ranges(&exprs, &span_map, &lines)
        };

        indexed_ranges
            .into_iter()
            .map(|(form_index, range)| {
                let command = Command {
                    title: "▶ Run".to_string(),
                    command: "sema.runTopLevel".to_string(),
                    arguments: Some(vec![serde_json::json!({
                        "uri": uri.as_str(),
                        "formIndex": form_index,
                    })]),
                };
                CodeLens {
                    range,
                    command: Some(command),
                    data: None,
                }
            })
            .collect()
    }

    pub(crate) fn handle_document_symbols(&self, uri: &Url) -> DocumentSymbolResponse {
        let cached = match self.cached_parses.get(uri.as_str()) {
            Some(c) => c,
            None => return DocumentSymbolResponse::Nested(vec![]),
        };
        let lines: Vec<&str> = cached.source.lines().collect();
        let symbols =
            document_symbols_from_ast(&cached.ast, &cached.span_map, &cached.symbol_spans, &lines);
        DocumentSymbolResponse::Nested(symbols)
    }

    #[allow(deprecated)]
    pub(crate) fn handle_workspace_symbols(&self, query: &str) -> Vec<SymbolInformation> {
        let mut results = Vec::new();
        let query_lower = query.to_lowercase();

        for file in self.workspace_index.iter() {
            if !self.indexed_file_is_current(file) {
                continue;
            }
            for definition in &file.definitions {
                if query.is_empty() || definition.name.to_lowercase().contains(&query_lower) {
                    results.push(SymbolInformation {
                        name: definition.name.clone(),
                        kind: definition.kind,
                        tags: None,
                        deprecated: None,
                        location: Location {
                            uri: file.uri.clone(),
                            range: definition.name_range,
                        },
                        container_name: None,
                    });
                }
            }
        }

        results
    }

    /// Build the SignatureHelp for a user-defined function from its params
    /// string (as extracted by `extract_params_from_ast`).
    fn user_signature_help(
        func_name: &str,
        params_str: &str,
        active_param: usize,
    ) -> SignatureHelp {
        let param_names = parse_param_names(params_str);
        let label = format!("({func_name} {})", param_names.join(" "));
        let parameters: Vec<ParameterInformation> = param_names
            .iter()
            .map(|p| ParameterInformation {
                label: ParameterLabel::Simple(p.clone()),
                documentation: None,
            })
            .collect();
        let active_param = (!param_names.is_empty())
            .then(|| active_param.min(param_names.len().saturating_sub(1)) as u32);
        SignatureHelp {
            signatures: vec![SignatureInformation {
                label,
                documentation: None,
                parameters: Some(parameters),
                active_parameter: active_param,
            }],
            active_signature: Some(0),
            active_parameter: active_param,
        }
    }

    pub(crate) fn handle_signature_help(
        &mut self,
        uri: &Url,
        position: &Position,
    ) -> Option<SignatureHelp> {
        let uri_str = uri.as_str();
        let text = self.documents.get(uri_str)?.clone();

        let (func_name, active_param) =
            find_enclosing_call(&text, position.line, position.character)?;

        // Try user definitions in current document (use cached parse)
        let cached = self.cached_parses.get(uri_str)?;

        let cursor_line = line_at(&text, position.line as usize)?;
        let cursor_col = utf16_to_char_col(cursor_line, position.character as usize);
        if cached
            .scope_tree
            .resolve_at(&func_name, position.line as usize + 1, cursor_col)
            .is_some_and(|resolved| !resolved.is_top_level)
        {
            return None;
        }

        let non_file_params = uri
            .to_file_path()
            .is_err()
            .then(|| extract_params_from_ast(&cached.ast, &func_name))
            .flatten();

        // Resolve current-file, imported, and re-exported definitions through
        // the same source-ordered binding model used by navigation.
        self.prepare_navigation_index(uri);
        if let Some(params_str) = self
            .visible_indexed_definition(uri, &func_name, *position)
            .and_then(|(_, definition)| definition.params.as_deref())
        {
            return Some(Self::user_signature_help(
                &func_name,
                params_str,
                active_param,
            ));
        }
        if let Some(params_str) = non_file_params {
            return Some(Self::user_signature_help(
                &func_name,
                &params_str,
                active_param,
            ));
        }

        // Builtin docs — with parameter highlighting when the entry carries (or its example
        // yields) parameter names. For special forms with an explicit `syntax` template, use that
        // as the display label and skip parameter positions (syntax forms don't map to flat args).
        if let Some(e) = self.builtin_docs.get(&func_name) {
            let doc = builtin_docs::render_markdown(e);
            let names = builtin_docs::param_names(e).unwrap_or_default();
            let (parameters, active, label) = if let Some(syn) = &e.syntax {
                (None, None, syn.clone())
            } else if names.is_empty() {
                (None, None, func_name.clone())
            } else {
                let params = names
                    .iter()
                    .map(|p| ParameterInformation {
                        label: ParameterLabel::Simple(p.clone()),
                        documentation: None,
                    })
                    .collect();
                let label = format!("({} {})", func_name, names.join(" "));
                let active_param = active_param.min(names.len().saturating_sub(1));
                (Some(params), Some(active_param as u32), label)
            };
            return Some(SignatureHelp {
                signatures: vec![SignatureInformation {
                    label,
                    documentation: Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: doc,
                    })),
                    parameters,
                    active_parameter: active,
                }],
                active_signature: Some(0),
                active_parameter: active,
            });
        }

        None
    }

    pub(crate) fn handle_folding_ranges(&self, uri: &Url) -> Vec<FoldingRange> {
        let cached = match self.cached_parses.get(uri.as_str()) {
            Some(c) => c,
            None => return vec![],
        };

        let lines: Vec<&str> = cached.source.lines().collect();
        let mut ranges = Vec::new();
        Self::collect_folding_ranges(&cached.ast, &cached.span_map, &lines, &mut ranges);
        ranges
    }

    fn collect_folding_ranges(
        exprs: &[sema_core::Value],
        span_map: &SpanMap,
        lines: &[&str],
        ranges: &mut Vec<FoldingRange>,
    ) {
        walk_values(exprs, |expr| {
            if let Some(span) = expr_span(expr, span_map) {
                // Only emit a fold when the form spans at least 2 visible
                // lines (`end_line - line >= 2`). Tiny 1-2-line forms add
                // folding noise without any benefit.
                if span.end_line.saturating_sub(span.line) >= 2 {
                    // Span columns are chars; LSP characters are UTF-16
                    // code units.
                    ranges.push(FoldingRange {
                        start_line: (span.line - 1) as u32,
                        start_character: Some(char_col_to_utf16(
                            lines.get(span.line - 1).copied(),
                            span.col,
                        )),
                        end_line: (span.end_line - 1) as u32,
                        end_character: Some(char_col_to_utf16(
                            lines.get(span.end_line - 1).copied(),
                            span.end_col,
                        )),
                        kind: Some(FoldingRangeKind::Region),
                        collapsed_text: None,
                    });
                }
            }
        });
    }

    /// Compute structural selection ranges: for each requested position, the chain of enclosing
    /// s-expressions from the symbol under the cursor outward to the top-level form. Powers
    /// Extend/Shrink Selection in editors.
    pub(crate) fn handle_selection_range(
        &self,
        uri: &Url,
        positions: &[Position],
    ) -> Option<Vec<SelectionRange>> {
        let cached = self.cached_parses.get(uri.as_str())?;
        let lines: Vec<&str> = cached.source.lines().collect();
        let result = positions
            .iter()
            .map(|pos| {
                let mut ranges: Vec<Range> = Vec::new();
                // Innermost candidates: the symbol token under the cursor.
                for (_, span) in &cached.symbol_spans {
                    let r = span_to_range(span, &lines);
                    if position_in_selection_range(pos, &r) {
                        ranges.push(r);
                    }
                }
                // Enclosing list forms (recursively).
                walk_values(&cached.ast, |expr| {
                    if let Some(r) = expr_range(expr, &cached.span_map, &lines) {
                        if position_in_selection_range(pos, &r) {
                            ranges.push(r);
                        }
                    }
                });
                build_selection_range(ranges, pos)
            })
            .collect();
        Some(result)
    }

    /// Document links for `import`/`load` path strings → the resolved file.
    pub(crate) fn handle_document_links(&self, uri: &Url) -> Option<Vec<DocumentLink>> {
        let cached = self.cached_parses.get(uri.as_str())?;
        let lines: Vec<&str> = cached.source.lines().collect();
        let tokens = sema_reader::lexer::tokenize(&cached.source).ok()?;
        let mut links = Vec::new();
        for expr in flatten_module_forms(&cached.ast) {
            let items = match expr.as_list() {
                Some(i) if i.len() >= 2 => i,
                _ => continue,
            };
            let head = match items[0].as_symbol() {
                Some(h) if h == "import" || h == "load" => h,
                _ => continue,
            };
            let _ = head;
            let path = match items[1].as_str() {
                Some(p) => p,
                None => continue,
            };
            let span = match expr_span(expr, &cached.span_map) {
                Some(s) => s,
                None => continue,
            };
            let Some(string_token) = tokens.iter().find(|token| {
                span.contains(&token.span)
                    && matches!(&token.token, sema_reader::lexer::Token::String(value) if value == path)
            }) else {
                continue;
            };
            let token_range = span_to_range(&string_token.span, &lines);
            let range = Range {
                start: Position {
                    line: token_range.start.line,
                    character: token_range.start.character.saturating_add(1),
                },
                end: Position {
                    line: token_range.end.line,
                    character: token_range.end.character.saturating_sub(1),
                },
            };
            // Only link paths that resolve to an existing file (the import
            // jump in goto-definition applies the same filter).
            if let Some(resolved) = resolve_import_path(uri, path).filter(|p| p.exists()) {
                if let Ok(target) = Url::from_file_path(&resolved) {
                    links.push(DocumentLink {
                        range,
                        target: Some(target),
                        tooltip: Some(format!("Open {path}")),
                        data: None,
                    });
                }
            }
        }
        Some(links)
    }

    /// Resolve the definition under the cursor into a call-hierarchy root item.
    pub(crate) fn handle_call_hierarchy_prepare(
        &self,
        uri: &Url,
        position: &Position,
    ) -> Option<Vec<CallHierarchyItem>> {
        let cached = self.cached_parses.get(uri.as_str())?;
        let lines: Vec<&str> = cached.source.lines().collect();
        let line = lines.get(position.line as usize).copied()?;
        let byte_offset = utf16_to_byte_offset(line, position.character);
        let symbol = extract_symbol_at(line, byte_offset);
        if symbol.is_empty() {
            return None;
        }
        let index = self.def_index();
        let (def_uri, form_range, name_range) = index.get(symbol)?;
        Some(vec![Self::call_hierarchy_item(
            symbol,
            def_uri,
            *form_range,
            *name_range,
        )])
    }

    /// Who calls this function: every definition whose body contains a call to
    /// `item.name`, across open documents and still-fresh scanned files.
    pub(crate) fn handle_call_hierarchy_incoming(
        &self,
        item: &CallHierarchyItem,
    ) -> Option<Vec<CallHierarchyIncomingCall>> {
        let target = &item.name;
        let mut result = Vec::new();
        for file in self.workspace_index.iter() {
            if !self.indexed_file_is_current(file) {
                continue;
            }
            for definition in &file.definitions {
                let Some(sites) = definition.calls.get(target) else {
                    continue;
                };
                result.push(CallHierarchyIncomingCall {
                    from: Self::call_hierarchy_item(
                        &definition.name,
                        &file.uri,
                        definition.form_range,
                        definition.name_range,
                    ),
                    from_ranges: sites.clone(),
                });
            }
        }
        Some(result)
    }

    /// Which functions this function calls: known definitions invoked from
    /// `item.name`'s body. The definition is looked up in open documents
    /// first, then still-fresh scanned files.
    pub(crate) fn handle_call_hierarchy_outgoing(
        &self,
        item: &CallHierarchyItem,
    ) -> Option<Vec<CallHierarchyOutgoingCall>> {
        let name = &item.name;
        let index = self.def_index();
        let item_path = item
            .uri
            .to_file_path()
            .ok()
            .map(|path| canonicalize_or_raw(&path));
        let definition = self.workspace_index.iter().find_map(|file| {
            let same_file =
                item_path.as_ref().is_some_and(|path| path == &file.path) || file.uri == item.uri;
            (same_file && self.indexed_file_is_current(file))
                .then(|| {
                    file.definitions
                        .iter()
                        .find(|definition| definition.name == *name)
                })
                .flatten()
        });
        let Some(definition) = definition else {
            return Some(Vec::new());
        };
        let mut result = Vec::new();
        for (callee, sites) in &definition.calls {
            if callee == name {
                continue;
            }
            if let Some((uri, range, name_range)) = index.get(callee) {
                result.push(CallHierarchyOutgoingCall {
                    to: Self::call_hierarchy_item(callee, uri, *range, *name_range),
                    from_ranges: sites.clone(),
                });
            }
        }
        Some(result)
    }

    pub(crate) fn handle_inlay_hints(
        &mut self,
        uri: &Url,
        range: &Range,
    ) -> Option<Vec<InlayHint>> {
        let uri_str = uri.as_str();

        self.prepare_navigation_index(uri);

        let text = self.documents.get(uri_str)?;
        let cached = self.cached_parses.get(uri_str)?;

        let mut hints = Vec::new();
        Self::collect_inlay_hints_inner(
            self,
            &cached.ast,
            &cached.span_map,
            text,
            uri,
            range,
            &mut hints,
        );
        if hints.is_empty() {
            None
        } else {
            Some(hints)
        }
    }

    fn collect_inlay_hints_inner(
        state: &BackendState,
        exprs: &[sema_core::Value],
        span_map: &SpanMap,
        text: &str,
        uri: &Url,
        range: &Range,
        hints: &mut Vec<InlayHint>,
    ) {
        let lines: Vec<&str> = text.lines().collect();

        for expr in exprs {
            let items = match expr.as_list() {
                Some(items) if items.len() >= 2 => items,
                _ => continue,
            };

            // Check if this form's span intersects the requested range
            let form_span = match expr_span(expr, span_map) {
                Some(s) => s,
                None => {
                    Self::collect_inlay_hints_inner(
                        state, items, span_map, text, uri, range, hints,
                    );
                    continue;
                }
            };
            let form_range = span_to_range(form_span, &lines);
            if (form_range.end.line, form_range.end.character)
                <= (range.start.line, range.start.character)
                || (range.end.line, range.end.character)
                    <= (form_range.start.line, form_range.start.character)
            {
                continue;
            }

            // Get the function name
            let func_name = match items[0].as_symbol() {
                Some(name) => name,
                None => {
                    Self::collect_inlay_hints_inner(
                        state, items, span_map, text, uri, range, hints,
                    );
                    continue;
                }
            };

            // Skip special forms — they don't have positional params
            if sema_eval::SPECIAL_FORM_NAMES.contains(&func_name.as_str()) {
                for item in &items[1..] {
                    if let Some(sub) = item.as_list() {
                        Self::collect_inlay_hints_inner(
                            state, sub, span_map, text, uri, range, hints,
                        );
                    }
                }
                continue;
            }

            // Try to resolve parameter names
            let param_names =
                Self::resolve_param_names_immut(state, uri, &func_name, form_range.start);

            if let Some(params) = &param_names {
                // Find argument positions by scanning the source text within the form.
                let arg_positions = find_arg_positions_in_form(form_span, &lines, items.len() - 1);

                let args = &items[1..];
                for (i, _arg) in args.iter().enumerate() {
                    if i >= params.len() {
                        break;
                    }
                    let param = &params[i];
                    if param == "." || param == "..." {
                        break;
                    }
                    if let Some(&(line, col)) = arg_positions.get(i) {
                        // `col` is a byte offset into the source line; LSP
                        // `character` must be a UTF-16 code-unit offset.
                        let character = lines
                            .get(line)
                            .map(|l| byte_offset_to_utf16(l, col))
                            .unwrap_or(col as u32);
                        let position = Position {
                            line: line as u32,
                            character,
                        };
                        if (position.line, position.character)
                            < (range.start.line, range.start.character)
                            || (position.line, position.character)
                                >= (range.end.line, range.end.character)
                        {
                            continue;
                        }
                        hints.push(InlayHint {
                            position,
                            label: InlayHintLabel::String(format!("{}:", param)),
                            kind: Some(InlayHintKind::PARAMETER),
                            text_edits: None,
                            tooltip: None,
                            padding_left: None,
                            padding_right: Some(true),
                            data: None,
                        });
                    }
                }
            }

            // Recurse into arguments (they may contain nested calls)
            for item in &items[1..] {
                if let Some(sub) = item.as_list() {
                    Self::collect_inlay_hints_inner(state, sub, span_map, text, uri, range, hints);
                }
            }
        }
    }

    /// Resolve parameter names for a function, checking current document,
    /// imported modules, and builtin docs. Immutable version — import caches
    /// must be pre-populated before calling.
    fn resolve_param_names_immut(
        state: &BackendState,
        uri: &Url,
        func_name: &str,
        occurrence: Position,
    ) -> Option<Vec<String>> {
        let uri_str = uri.as_str();

        // Resolve current-file and imported bindings through the same
        // source-ordered model used by navigation.
        if let Some(params_str) = state
            .visible_indexed_definition(uri, func_name, occurrence)
            .and_then(|(_, definition)| definition.params.as_deref())
        {
            let names = parse_param_names(params_str);
            if !names.is_empty() {
                return Some(names);
            }
        }

        // Non-file documents are not represented in the workspace index.
        if uri.to_file_path().is_err() {
            if let Some(params_str) = state
                .cached_parses
                .get(uri_str)
                .and_then(|cached| extract_params_from_ast(&cached.ast, func_name))
            {
                let names = parse_param_names(&params_str);
                if !names.is_empty() {
                    return Some(names);
                }
            }
        }

        // Try builtin docs — structured params, or parsed from the entry's example.
        if let Some(e) = state.builtin_docs.get(func_name) {
            if let Some(params) = builtin_docs::param_names(e) {
                if !params.is_empty() {
                    return Some(params);
                }
            }
        }

        None
    }
}
