//! Structural and informational endpoints: code lens, document/workspace
//! symbols, signature help, folding ranges, selection ranges, document links,
//! call hierarchy, and inlay hints.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tower_lsp::lsp_types::*;

use sema_core::SpanMap;

use crate::builtin_docs;
use crate::helpers::*;
use crate::state::{
    build_selection_range, collect_call_sites, collect_outgoing_calls, def_of_form,
    position_in_range, quoted_string_range, BackendState, CachedParse, ImportCache,
};

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
                let title = if self.run_sandbox_mode == "strict" {
                    "▶ Run (strict)".to_string()
                } else {
                    "▶ Run".to_string()
                };
                let command = Command {
                    title,
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
        let mut searched_uris: HashSet<String> = HashSet::new();

        for (doc_uri_str, cached) in &self.cached_parses {
            let doc_uri = match Url::parse(doc_uri_str) {
                Ok(u) => u,
                Err(_) => continue,
            };
            searched_uris.insert(doc_uri_str.clone());

            let doc_lines: Vec<&str> = self
                .documents
                .get(doc_uri_str)
                .map(|t| t.lines().collect())
                .unwrap_or_default();
            let symbols = document_symbols_from_ast(
                &cached.ast,
                &cached.span_map,
                &cached.symbol_spans,
                &doc_lines,
            );

            for sym in symbols {
                if query.is_empty() || sym.name.to_lowercase().contains(&query_lower) {
                    results.push(SymbolInformation {
                        name: sym.name,
                        kind: sym.kind,
                        tags: None,
                        deprecated: None,
                        location: Location {
                            uri: doc_uri.clone(),
                            range: sym.selection_range,
                        },
                        container_name: None,
                    });
                }
            }
        }

        // Also search workspace files not currently open (import_cache)
        for (path, import_cached) in &self.import_cache {
            let import_uri = match Url::from_file_path(path) {
                Ok(u) => u,
                Err(_) => continue,
            };
            // Skip files already returned via cached_parses
            if searched_uris.contains(import_uri.as_str()) {
                continue;
            }

            let import_lines: Vec<&str> = import_cached.source.lines().collect();
            let symbols = document_symbols_from_ast(
                &import_cached.ast,
                &import_cached.span_map,
                &import_cached.symbol_spans,
                &import_lines,
            );

            for sym in symbols {
                if query.is_empty() || sym.name.to_lowercase().contains(&query_lower) {
                    results.push(SymbolInformation {
                        name: sym.name,
                        kind: sym.kind,
                        tags: None,
                        deprecated: None,
                        location: Location {
                            uri: import_uri.clone(),
                            range: sym.selection_range,
                        },
                        container_name: None,
                    });
                }
            }
        }

        results
    }

    pub(crate) fn handle_signature_help(
        &mut self,
        uri: &Url,
        position: &Position,
    ) -> Option<SignatureHelp> {
        let uri_str = uri.as_str();
        let text = self.documents.get(uri_str)?;

        let (func_name, active_param) =
            find_enclosing_call(text, position.line, position.character)?;

        // Try user definitions in current document (use cached parse)
        let cached = self.cached_parses.get(uri_str)?;

        if let Some(params_str) = extract_params_from_ast(&cached.ast, &func_name) {
            let param_names = parse_param_names(&params_str);
            let label = format!("({func_name} {})", param_names.join(" "));
            let parameters: Vec<ParameterInformation> = param_names
                .iter()
                .map(|p| ParameterInformation {
                    label: ParameterLabel::Simple(p.clone()),
                    documentation: None,
                })
                .collect();

            return Some(SignatureHelp {
                signatures: vec![SignatureInformation {
                    label,
                    documentation: None,
                    parameters: Some(parameters),
                    active_parameter: Some(active_param as u32),
                }],
                active_signature: Some(0),
                active_parameter: Some(active_param as u32),
            });
        }

        // Try imported files
        let import_paths = import_paths_from_ast(&cached.ast);
        for path_str in &import_paths {
            let resolved = match resolve_import_path(uri, path_str) {
                Some(p) if p.exists() => p,
                _ => continue,
            };
            let cached = match self.get_import_cache(&resolved) {
                Some(c) => c,
                None => continue,
            };
            if let Some(params_str) = extract_params_from_ast(&cached.ast, &func_name) {
                let param_names = parse_param_names(&params_str);
                let label = format!("({func_name} {})", param_names.join(" "));
                let parameters: Vec<ParameterInformation> = param_names
                    .iter()
                    .map(|p| ParameterInformation {
                        label: ParameterLabel::Simple(p.clone()),
                        documentation: None,
                    })
                    .collect();

                return Some(SignatureHelp {
                    signatures: vec![SignatureInformation {
                        label,
                        documentation: None,
                        parameters: Some(parameters),
                        active_parameter: Some(active_param as u32),
                    }],
                    active_signature: Some(0),
                    active_parameter: Some(active_param as u32),
                });
            }
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

        let mut ranges = Vec::new();
        Self::collect_folding_ranges(&cached.ast, &cached.span_map, &mut ranges);
        ranges
    }

    fn collect_folding_ranges(
        exprs: &[sema_core::Value],
        span_map: &SpanMap,
        ranges: &mut Vec<FoldingRange>,
    ) {
        for expr in exprs {
            if let Some(items) = expr.as_list() {
                if let Some(span) = expr_span(expr, span_map) {
                    // Only emit a fold when the form spans at least 2 visible
                    // lines (`end_line - line >= 2`). Tiny 1-2-line forms add
                    // folding noise without any benefit.
                    if span.end_line.saturating_sub(span.line) >= 2 {
                        ranges.push(FoldingRange {
                            start_line: (span.line - 1) as u32,
                            start_character: Some((span.col - 1) as u32),
                            end_line: (span.end_line - 1) as u32,
                            end_character: Some((span.end_col - 1) as u32),
                            kind: Some(FoldingRangeKind::Region),
                            collapsed_text: None,
                        });
                    }
                }
                // Recurse into sub-expressions
                Self::collect_folding_ranges(items, span_map, ranges);
            }
        }
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
                    if position_in_range(pos, &r) {
                        ranges.push(r);
                    }
                }
                // Enclosing list forms (recursively).
                Self::collect_selection_list_ranges(
                    &cached.ast,
                    &cached.span_map,
                    &lines,
                    pos,
                    &mut ranges,
                );
                build_selection_range(ranges, pos)
            })
            .collect();
        Some(result)
    }

    fn collect_selection_list_ranges(
        exprs: &[sema_core::Value],
        span_map: &SpanMap,
        lines: &[&str],
        pos: &Position,
        out: &mut Vec<Range>,
    ) {
        for expr in exprs {
            if let Some(items) = expr.as_list() {
                if let Some(r) = expr_range(expr, span_map, lines) {
                    if position_in_range(pos, &r) {
                        out.push(r);
                    }
                }
                Self::collect_selection_list_ranges(items, span_map, lines, pos, out);
            }
        }
    }

    /// Document links for `import`/`load` path strings → the resolved file.
    pub(crate) fn handle_document_links(&self, uri: &Url) -> Option<Vec<DocumentLink>> {
        let cached = self.cached_parses.get(uri.as_str())?;
        let lines: Vec<&str> = cached.source.lines().collect();
        let mut links = Vec::new();
        for expr in &cached.ast {
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
            let form_range = span_to_range(span, &lines);
            let range = quoted_string_range(&lines, &form_range, path).unwrap_or(form_range);
            if let Some(resolved) = resolve_import_path(uri, path) {
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

    /// Who calls this function: every definition whose body contains a call to `item.name`.
    pub(crate) fn handle_call_hierarchy_incoming(
        &self,
        item: &CallHierarchyItem,
    ) -> Option<Vec<CallHierarchyIncomingCall>> {
        let target = &item.name;
        let mut result = Vec::new();
        for (uri_str, cached) in &self.cached_parses {
            let uri = match Url::parse(uri_str) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let lines: Vec<&str> = cached.source.lines().collect();
            for expr in &cached.ast {
                let items = match expr.as_list() {
                    Some(i) => i,
                    None => continue,
                };
                let (name, form_range, name_range) =
                    match def_of_form(expr, &cached.span_map, &cached.symbol_spans, &lines) {
                        Some(d) => d,
                        None => continue,
                    };
                // Search only the body (skip the head + signature) to avoid matching `(name ...)`
                // definition shorthands as calls.
                let body: &[sema_core::Value] = items.get(2..).unwrap_or(&[]);
                let mut sites = Vec::new();
                collect_call_sites(
                    body,
                    &cached.span_map,
                    &cached.symbol_spans,
                    &lines,
                    target,
                    &mut sites,
                );
                if !sites.is_empty() {
                    result.push(CallHierarchyIncomingCall {
                        from: Self::call_hierarchy_item(&name, &uri, form_range, name_range),
                        from_ranges: sites,
                    });
                }
            }
        }
        Some(result)
    }

    /// Which functions this function calls: known definitions invoked from `item.name`'s body.
    pub(crate) fn handle_call_hierarchy_outgoing(
        &self,
        item: &CallHierarchyItem,
    ) -> Option<Vec<CallHierarchyOutgoingCall>> {
        let name = &item.name;
        let index = self.def_index();
        for cached in self.cached_parses.values() {
            let lines: Vec<&str> = cached.source.lines().collect();
            for expr in &cached.ast {
                let items = match expr.as_list() {
                    Some(i) => i,
                    None => continue,
                };
                let def = match def_of_form(expr, &cached.span_map, &cached.symbol_spans, &lines) {
                    Some(d) => d,
                    None => continue,
                };
                if &def.0 != name {
                    continue;
                }
                let body: &[sema_core::Value] = items.get(2..).unwrap_or(&[]);
                let mut calls: std::collections::HashMap<String, Vec<Range>> = Default::default();
                collect_outgoing_calls(
                    body,
                    &cached.span_map,
                    &cached.symbol_spans,
                    &lines,
                    &index,
                    &mut calls,
                );
                let mut out = Vec::new();
                for (callee, sites) in calls {
                    if callee == *name {
                        continue; // skip self-recursion in outgoing view
                    }
                    if let Some((curi, crange, cname_range)) = index.get(&callee) {
                        out.push(CallHierarchyOutgoingCall {
                            to: Self::call_hierarchy_item(&callee, curi, *crange, *cname_range),
                            from_ranges: sites,
                        });
                    }
                }
                return Some(out);
            }
        }
        Some(Vec::new())
    }

    pub(crate) fn handle_inlay_hints(
        &mut self,
        uri: &Url,
        range: &Range,
    ) -> Option<Vec<InlayHint>> {
        let uri_str = uri.as_str();

        // Pre-populate import caches before the immutable borrow phase,
        // so resolve_param_names can be called without &mut self.
        if let Some(cached) = self.cached_parses.get(uri_str) {
            let import_paths = import_paths_from_ast(&cached.ast);
            let paths_to_cache: Vec<PathBuf> = import_paths
                .iter()
                .filter_map(|p| resolve_import_path(uri, p))
                .filter(|p| p.exists())
                .collect();
            for path in &paths_to_cache {
                let _ = self.get_import_cache(path);
            }
        }

        let text = self.documents.get(uri_str)?;
        let cached = self.cached_parses.get(uri_str)?;

        let mut hints = Vec::new();
        Self::collect_inlay_hints_inner(
            &cached.ast,
            &cached.span_map,
            text,
            uri,
            range,
            &self.cached_parses,
            &self.import_cache,
            &self.builtin_docs,
            &mut hints,
        );
        if hints.is_empty() {
            None
        } else {
            Some(hints)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_inlay_hints_inner(
        exprs: &[sema_core::Value],
        span_map: &SpanMap,
        text: &str,
        uri: &Url,
        range: &Range,
        cached_parses: &HashMap<String, CachedParse>,
        import_cache: &HashMap<PathBuf, ImportCache>,
        builtin_docs: &builtin_docs::BuiltinDocs,
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
                        items,
                        span_map,
                        text,
                        uri,
                        range,
                        cached_parses,
                        import_cache,
                        builtin_docs,
                        hints,
                    );
                    continue;
                }
            };
            let form_start_line = form_span.line.saturating_sub(1) as u32;
            let form_end_line = form_span.end_line.saturating_sub(1) as u32;
            if form_end_line < range.start.line || form_start_line > range.end.line {
                continue;
            }

            // Get the function name
            let func_name = match items[0].as_symbol() {
                Some(name) => name,
                None => {
                    Self::collect_inlay_hints_inner(
                        items,
                        span_map,
                        text,
                        uri,
                        range,
                        cached_parses,
                        import_cache,
                        builtin_docs,
                        hints,
                    );
                    continue;
                }
            };

            // Skip special forms — they don't have positional params
            if sema_eval::SPECIAL_FORM_NAMES.contains(&func_name.as_str()) {
                for item in &items[1..] {
                    if let Some(sub) = item.as_list() {
                        Self::collect_inlay_hints_inner(
                            sub,
                            span_map,
                            text,
                            uri,
                            range,
                            cached_parses,
                            import_cache,
                            builtin_docs,
                            hints,
                        );
                    }
                }
                continue;
            }

            // Try to resolve parameter names
            let param_names = Self::resolve_param_names_immut(
                uri,
                &func_name,
                cached_parses,
                import_cache,
                builtin_docs,
            );

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
                        hints.push(InlayHint {
                            position: Position {
                                line: line as u32,
                                character,
                            },
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
                    Self::collect_inlay_hints_inner(
                        sub,
                        span_map,
                        text,
                        uri,
                        range,
                        cached_parses,
                        import_cache,
                        builtin_docs,
                        hints,
                    );
                }
            }
        }
    }

    /// Resolve parameter names for a function, checking current document,
    /// imported modules, and builtin docs. Immutable version — import caches
    /// must be pre-populated before calling.
    fn resolve_param_names_immut(
        uri: &Url,
        func_name: &str,
        cached_parses: &HashMap<String, CachedParse>,
        import_cache: &HashMap<PathBuf, ImportCache>,
        builtin_docs: &builtin_docs::BuiltinDocs,
    ) -> Option<Vec<String>> {
        let uri_str = uri.as_str();

        // 1. Check current document
        if let Some(cached) = cached_parses.get(uri_str) {
            if let Some(params_str) = extract_params_from_ast(&cached.ast, func_name) {
                let names = parse_param_names(&params_str);
                if !names.is_empty() {
                    return Some(names);
                }
            }
        }

        // 2. Check imported modules (from pre-populated cache)
        if let Some(cached) = cached_parses.get(uri_str) {
            let paths = import_paths_from_ast(&cached.ast);
            for path_str in &paths {
                let resolved = match resolve_import_path(uri, path_str) {
                    Some(p) if p.exists() => p,
                    _ => continue,
                };
                if let Some(import_cached) = import_cache.get(&resolved) {
                    if let Some(params_str) = extract_params_from_ast(&import_cached.ast, func_name)
                    {
                        let names = parse_param_names(&params_str);
                        if !names.is_empty() {
                            return Some(names);
                        }
                    }
                }
            }
        }

        // 3. Try builtin docs — structured params, or parsed from the entry's example.
        if let Some(e) = builtin_docs.get(func_name) {
            if let Some(params) = builtin_docs::param_names(e) {
                if !params.is_empty() {
                    return Some(params);
                }
            }
        }

        None
    }
}
