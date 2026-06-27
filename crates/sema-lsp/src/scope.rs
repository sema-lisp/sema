//! Scope tree for lexical scope analysis.
//!
//! Built from the parsed AST + SpanMap + symbol_spans to determine which
//! definition a symbol occurrence belongs to. Used by rename and references
//! to avoid matching unrelated symbols that happen to share a name.

use sema_core::{Span, SpanMap, Value};
use std::rc::Rc;

// ── Data structures ──────────────────────────────────────────────

/// A single lexical scope (top-level, let body, lambda body, etc.).
#[derive(Debug)]
struct Scope {
    /// Index of the parent scope, or `None` for the top-level scope.
    parent: Option<usize>,
    /// Textual extent of this scope (1-indexed Sema Span).
    span: Span,
    /// Names bound in this scope, with the span of the binding site.
    bindings: Vec<Binding>,
}

/// A single name binding (e.g. a `let` variable, a function parameter).
#[derive(Debug)]
struct Binding {
    name: String,
    /// Where the name is defined (1-indexed Sema Span).
    def_span: Span,
}

/// A scope tree built from a parsed AST.
#[derive(Debug)]
pub struct ScopeTree {
    scopes: Vec<Scope>,
}

/// The result of resolving a symbol occurrence to its definition scope.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSymbol {
    /// The scope index that defines this symbol.
    pub scope_idx: usize,
    /// The span of the definition site.
    pub def_span: Span,
    /// Whether this is a top-level definition.
    pub is_top_level: bool,
}

// ── Helpers ──────────────────────────────────────────────────────

/// Look up the Span for a list expression via its Rc pointer in the SpanMap.
fn expr_span(expr: &Value, span_map: &SpanMap) -> Option<Span> {
    let rc = expr.as_list_rc()?;
    let ptr = Rc::as_ptr(&rc) as usize;
    span_map.get(&ptr).copied()
}

/// Check if position (1-indexed line, col) falls within a span.
fn span_contains_pos(span: &Span, line: usize, col: usize) -> bool {
    span.contains_pos(line, col)
}

/// Check if `inner` span is fully contained within `outer` span.
fn span_contains_span(outer: &Span, inner: &Span) -> bool {
    outer.contains(inner)
}

/// Find the span of a symbol name within a parent span, using symbol_spans.
fn find_symbol_span(name: &str, within: &Span, symbol_spans: &[(String, Span)]) -> Option<Span> {
    symbol_spans
        .iter()
        .find(|(n, s)| n == name && span_contains_span(within, s))
        .map(|(_, s)| *s)
}

// ── Builder ──────────────────────────────────────────────────────

impl ScopeTree {
    /// Build a scope tree from a parsed AST.
    pub fn build(ast: &[Value], span_map: &SpanMap, symbol_spans: &[(String, Span)]) -> Self {
        let mut tree = ScopeTree { scopes: Vec::new() };

        // Create the implicit top-level scope spanning the entire file.
        let top = Scope {
            parent: None,
            span: Span::new(1, 1, usize::MAX / 2, usize::MAX / 2),
            bindings: Vec::new(),
        };
        tree.scopes.push(top);

        // Walk top-level forms: collect top-level definitions and recurse.
        for expr in ast {
            tree.walk_expr(expr, 0, span_map, symbol_spans);
        }

        tree
    }

    /// Recursively walk an expression, creating scopes for binding forms.
    fn walk_expr(
        &mut self,
        expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        let items = match expr.as_list() {
            Some(items) if !items.is_empty() => items,
            _ => return,
        };

        let head = match items[0].as_symbol() {
            Some(s) => s,
            None => {
                // Not a special form — recurse into sub-expressions.
                for item in items {
                    self.walk_expr(item, parent_scope, span_map, symbol_spans);
                }
                return;
            }
        };

        match head.as_str() {
            // ── Top-level definitions ────────────────────────────
            "define" | "def" => self.walk_define(items, expr, parent_scope, span_map, symbol_spans),
            "defun" | "defn" => self.walk_defun(items, expr, parent_scope, span_map, symbol_spans),
            "defmacro" => self.walk_defmacro(items, expr, parent_scope, span_map, symbol_spans),
            "defagent" | "deftool" => {
                // These define a name at the parent scope level.
                if items.len() >= 2 {
                    if let Some(name) = items[1].as_symbol() {
                        let form_span = expr_span(expr, span_map);
                        if let Some(fs) = &form_span {
                            if let Some(ns) = find_symbol_span(&name, fs, symbol_spans) {
                                self.scopes[parent_scope]
                                    .bindings
                                    .push(Binding { name, def_span: ns });
                            }
                        }
                    }
                }
                // Recurse into body
                for item in &items[2..] {
                    self.walk_expr(item, parent_scope, span_map, symbol_spans);
                }
            }

            // ── Lambda / fn ──────────────────────────────────────
            "lambda" | "fn" => self.walk_lambda(items, expr, parent_scope, span_map, symbol_spans),

            // ── Let forms ────────────────────────────────────────
            "let" => self.walk_let(items, expr, parent_scope, span_map, symbol_spans),
            "let*" => self.walk_let_star(items, expr, parent_scope, span_map, symbol_spans),
            "letrec" => self.walk_letrec(items, expr, parent_scope, span_map, symbol_spans),

            // ── Match ────────────────────────────────────────────
            "match" => self.walk_match(items, expr, parent_scope, span_map, symbol_spans),

            // ── Do ───────────────────────────────────────────────
            "do" => self.walk_do(items, expr, parent_scope, span_map, symbol_spans),

            // ── Try/catch ────────────────────────────────────────
            "try" => self.walk_try(items, expr, parent_scope, span_map, symbol_spans),

            // ── For forms ────────────────────────────────────────
            "for" | "for/list" | "for/map" | "for/filter" | "for/fold" => {
                self.walk_for(items, expr, parent_scope, span_map, symbol_spans)
            }

            // ── Everything else: recurse ─────────────────────────
            _ => {
                for item in items {
                    self.walk_expr(item, parent_scope, span_map, symbol_spans);
                }
            }
        }
    }

    /// `(define x val)` or `(define (f x y) body...)`
    fn walk_define(
        &mut self,
        items: &[Value],
        expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if items.len() < 2 {
            return;
        }
        let form_span = match expr_span(expr, span_map) {
            Some(s) => s,
            None => return,
        };

        if let Some(name) = items[1].as_symbol() {
            // (define x val) — simple binding at parent scope
            if let Some(ns) = find_symbol_span(&name, &form_span, symbol_spans) {
                self.scopes[parent_scope]
                    .bindings
                    .push(Binding { name, def_span: ns });
            }
            // Recurse into the value expression
            if items.len() > 2 {
                self.walk_expr(&items[2], parent_scope, span_map, symbol_spans);
            }
        } else if let Some(sig) = items[1].as_list() {
            // (define (f x y) body...) — function shorthand
            if sig.is_empty() {
                return;
            }
            if let Some(fname) = sig[0].as_symbol() {
                // Bind the function name at parent scope
                let sig_span = expr_span(&items[1], span_map);
                if let Some(ss) = &sig_span {
                    if let Some(ns) = find_symbol_span(&fname, ss, symbol_spans) {
                        self.scopes[parent_scope].bindings.push(Binding {
                            name: fname,
                            def_span: ns,
                        });
                    }
                }

                // Create a child scope for the body with params bound
                let body_scope_idx = self.scopes.len();
                self.scopes.push(Scope {
                    parent: Some(parent_scope),
                    span: form_span,
                    bindings: Vec::new(),
                });

                // Bind parameters in the body scope
                for param in &sig[1..] {
                    self.collect_param_binding(
                        param,
                        body_scope_idx,
                        &form_span,
                        span_map,
                        symbol_spans,
                    );
                }

                // Recurse into body
                for item in &items[2..] {
                    self.walk_expr(item, body_scope_idx, span_map, symbol_spans);
                }
            }
        } else {
            // Destructuring define — recurse into value
            if items.len() > 2 {
                self.walk_expr(&items[2], parent_scope, span_map, symbol_spans);
            }
        }
    }

    /// `(defun f (params...) body...)` or `(defn f (params...) body...)`
    fn walk_defun(
        &mut self,
        items: &[Value],
        expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if items.len() < 3 {
            return;
        }
        let form_span = match expr_span(expr, span_map) {
            Some(s) => s,
            None => return,
        };

        if let Some(fname) = items[1].as_symbol() {
            // Bind name at parent scope
            if let Some(ns) = find_symbol_span(&fname, &form_span, symbol_spans) {
                self.scopes[parent_scope].bindings.push(Binding {
                    name: fname,
                    def_span: ns,
                });
            }

            // Create child scope for body
            let body_scope_idx = self.scopes.len();
            self.scopes.push(Scope {
                parent: Some(parent_scope),
                span: form_span,
                bindings: Vec::new(),
            });

            // Bind parameters
            if let Some(params) = items[2].as_list() {
                for param in params {
                    self.collect_param_binding(
                        param,
                        body_scope_idx,
                        &form_span,
                        span_map,
                        symbol_spans,
                    );
                }
            }

            // Recurse into body
            for item in &items[3..] {
                self.walk_expr(item, body_scope_idx, span_map, symbol_spans);
            }
        }
    }

    /// `(defmacro name (params...) body...)`
    fn walk_defmacro(
        &mut self,
        items: &[Value],
        expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if items.len() < 3 {
            return;
        }
        let form_span = match expr_span(expr, span_map) {
            Some(s) => s,
            None => return,
        };

        if let Some(name) = items[1].as_symbol() {
            // Bind name at parent scope
            if let Some(ns) = find_symbol_span(&name, &form_span, symbol_spans) {
                self.scopes[parent_scope]
                    .bindings
                    .push(Binding { name, def_span: ns });
            }

            // Create child scope for body with params
            let body_scope_idx = self.scopes.len();
            self.scopes.push(Scope {
                parent: Some(parent_scope),
                span: form_span,
                bindings: Vec::new(),
            });

            if let Some(params) = items[2].as_list() {
                for param in params {
                    self.collect_param_binding(
                        param,
                        body_scope_idx,
                        &form_span,
                        span_map,
                        symbol_spans,
                    );
                }
            }

            for item in &items[3..] {
                self.walk_expr(item, body_scope_idx, span_map, symbol_spans);
            }
        }
    }

    /// `(lambda (params...) body...)` or `(fn (params...) body...)`
    fn walk_lambda(
        &mut self,
        items: &[Value],
        expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if items.len() < 2 {
            return;
        }
        let form_span = match expr_span(expr, span_map) {
            Some(s) => s,
            None => return,
        };

        let body_scope_idx = self.scopes.len();
        self.scopes.push(Scope {
            parent: Some(parent_scope),
            span: form_span,
            bindings: Vec::new(),
        });

        // Bind parameters (from list or vector)
        let params: Option<&[Value]> = items[1].as_list().or_else(|| items[1].as_vector());
        if let Some(params) = params {
            for param in params {
                self.collect_param_binding(
                    param,
                    body_scope_idx,
                    &form_span,
                    span_map,
                    symbol_spans,
                );
            }
        }

        // Recurse into body
        for item in &items[2..] {
            self.walk_expr(item, body_scope_idx, span_map, symbol_spans);
        }
    }

    /// `(let ((x 1) (y 2)) body...)` or `(let name ((x 1)) body...)` (named let)
    fn walk_let(
        &mut self,
        items: &[Value],
        expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if items.len() < 2 {
            return;
        }
        let form_span = match expr_span(expr, span_map) {
            Some(s) => s,
            None => return,
        };

        // Check for named let: (let name ((var init) ...) body...).
        // The distinguishing feature is items[1] being a symbol (the loop name)
        // AND items[2] being the bindings list. `items[0]` is always the `let`
        // symbol, so guarding on it is meaningless and would misclassify a
        // malformed `(let name non-list ...)` as a named let.
        if items.len() >= 3 && items[2].as_list().is_some() {
            if let Some(loop_name) = items[1].as_symbol() {
                let body_scope_idx = self.scopes.len();
                self.scopes.push(Scope {
                    parent: Some(parent_scope),
                    span: form_span,
                    bindings: Vec::new(),
                });

                // Bind the loop name
                if let Some(ns) = find_symbol_span(&loop_name, &form_span, symbol_spans) {
                    self.scopes[body_scope_idx].bindings.push(Binding {
                        name: loop_name,
                        def_span: ns,
                    });
                }

                // Bind variables from bindings list
                if let Some(bindings) = items[2].as_list() {
                    for binding in bindings {
                        if let Some(pair) = binding.as_list() {
                            if pair.len() >= 2 {
                                // Init exprs are evaluated in the OUTER scope
                                self.walk_expr(&pair[1], parent_scope, span_map, symbol_spans);
                                self.collect_param_binding(
                                    &pair[0],
                                    body_scope_idx,
                                    &form_span,
                                    span_map,
                                    symbol_spans,
                                );
                            }
                        }
                    }
                }

                // Body in the new scope
                for item in &items[3..] {
                    self.walk_expr(item, body_scope_idx, span_map, symbol_spans);
                }
                return;
            }
        }

        // Regular let: items[0] is "let", items[1] is bindings list
        let body_scope_idx = self.scopes.len();
        self.scopes.push(Scope {
            parent: Some(parent_scope),
            span: form_span,
            bindings: Vec::new(),
        });

        if let Some(bindings) = items[1].as_list() {
            for binding in bindings {
                if let Some(pair) = binding.as_list() {
                    if pair.len() >= 2 {
                        // Init exprs are evaluated in the OUTER scope for let
                        self.walk_expr(&pair[1], parent_scope, span_map, symbol_spans);
                        self.collect_param_binding(
                            &pair[0],
                            body_scope_idx,
                            &form_span,
                            span_map,
                            symbol_spans,
                        );
                    }
                }
            }
        }

        for item in &items[2..] {
            self.walk_expr(item, body_scope_idx, span_map, symbol_spans);
        }
    }

    /// `(let* ((x 1) (y x)) body...)`
    ///
    /// Each binding's init expression sees only the *previous* bindings.
    /// We model this by creating a nested scope per binding.
    fn walk_let_star(
        &mut self,
        items: &[Value],
        expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if items.len() < 2 {
            return;
        }
        let form_span = match expr_span(expr, span_map) {
            Some(s) => s,
            None => return,
        };

        let mut current_scope = parent_scope;

        if let Some(bindings) = items[1].as_list() {
            for binding in bindings {
                if let Some(pair) = binding.as_list() {
                    if pair.len() >= 2 {
                        // Init expression is evaluated in the *current* scope
                        // (before this binding is added).
                        self.walk_expr(&pair[1], current_scope, span_map, symbol_spans);

                        // Create a new nested scope for this binding.
                        let new_scope_idx = self.scopes.len();
                        self.scopes.push(Scope {
                            parent: Some(current_scope),
                            span: form_span,
                            bindings: Vec::new(),
                        });
                        self.collect_param_binding(
                            &pair[0],
                            new_scope_idx,
                            &form_span,
                            span_map,
                            symbol_spans,
                        );
                        current_scope = new_scope_idx;
                    }
                }
            }
        }

        for item in &items[2..] {
            self.walk_expr(item, current_scope, span_map, symbol_spans);
        }
    }

    /// `(letrec ((x ...) (y ...)) body...)`
    ///
    /// All bindings are visible to all init expressions and the body
    /// (supports mutual recursion).
    fn walk_letrec(
        &mut self,
        items: &[Value],
        expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if items.len() < 2 {
            return;
        }
        let form_span = match expr_span(expr, span_map) {
            Some(s) => s,
            None => return,
        };

        let body_scope_idx = self.scopes.len();
        self.scopes.push(Scope {
            parent: Some(parent_scope),
            span: form_span,
            bindings: Vec::new(),
        });

        // First pass: collect ALL binding names into the scope.
        if let Some(bindings) = items[1].as_list() {
            for binding in bindings {
                if let Some(pair) = binding.as_list() {
                    if pair.len() >= 2 {
                        self.collect_param_binding(
                            &pair[0],
                            body_scope_idx,
                            &form_span,
                            span_map,
                            symbol_spans,
                        );
                    }
                }
            }

            // Second pass: walk init expressions with all bindings visible.
            for binding in bindings {
                if let Some(pair) = binding.as_list() {
                    if pair.len() >= 2 {
                        self.walk_expr(&pair[1], body_scope_idx, span_map, symbol_spans);
                    }
                }
            }
        }

        for item in &items[2..] {
            self.walk_expr(item, body_scope_idx, span_map, symbol_spans);
        }
    }

    /// `(match val (pattern body)...)`
    fn walk_match(
        &mut self,
        items: &[Value],
        _expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if items.len() < 2 {
            return;
        }

        // Recurse into the match value
        self.walk_expr(&items[1], parent_scope, span_map, symbol_spans);

        // Each clause creates a scope
        for clause in &items[2..] {
            if let Some(clause_items) = clause.as_list() {
                if clause_items.len() >= 2 {
                    let clause_span = expr_span(clause, span_map);
                    if let Some(cs) = clause_span {
                        let clause_scope_idx = self.scopes.len();
                        self.scopes.push(Scope {
                            parent: Some(parent_scope),
                            span: cs,
                            bindings: Vec::new(),
                        });

                        // Collect pattern bindings
                        self.collect_pattern_bindings(
                            &clause_items[0],
                            clause_scope_idx,
                            &cs,
                            span_map,
                            symbol_spans,
                        );

                        // Recurse into clause body
                        for item in &clause_items[1..] {
                            self.walk_expr(item, clause_scope_idx, span_map, symbol_spans);
                        }
                    }
                }
            }
        }
    }

    /// `(do ((var init step)...) (test result...) body...)`
    fn walk_do(
        &mut self,
        items: &[Value],
        expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if items.len() < 3 {
            return;
        }
        let form_span = match expr_span(expr, span_map) {
            Some(s) => s,
            None => return,
        };

        let body_scope_idx = self.scopes.len();
        self.scopes.push(Scope {
            parent: Some(parent_scope),
            span: form_span,
            bindings: Vec::new(),
        });

        if let Some(bindings) = items[1].as_list() {
            for binding in bindings {
                if let Some(parts) = binding.as_list() {
                    if parts.len() >= 2 {
                        if let Some(name) = parts[0].as_symbol() {
                            if let Some(ns) = find_symbol_span(&name, &form_span, symbol_spans) {
                                self.scopes[body_scope_idx]
                                    .bindings
                                    .push(Binding { name, def_span: ns });
                            }
                        }
                        // Init exprs in outer scope
                        self.walk_expr(&parts[1], parent_scope, span_map, symbol_spans);
                        // Step exprs in body scope
                        if parts.len() >= 3 {
                            self.walk_expr(&parts[2], body_scope_idx, span_map, symbol_spans);
                        }
                    }
                }
            }
        }

        // Test and result exprs
        self.walk_expr(&items[2], body_scope_idx, span_map, symbol_spans);

        // Body
        for item in &items[3..] {
            self.walk_expr(item, body_scope_idx, span_map, symbol_spans);
        }
    }

    /// `(try body... (catch var handler...))`
    fn walk_try(
        &mut self,
        items: &[Value],
        expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        let form_span = expr_span(expr, span_map);

        for item in &items[1..] {
            if let Some(clause) = item.as_list() {
                if clause.len() >= 3 {
                    if let Some(head) = clause[0].as_symbol() {
                        if head == "catch" {
                            // (catch var handler...)
                            if let Some(var_name) = clause[1].as_symbol() {
                                let catch_span = expr_span(item, span_map).or(form_span);
                                if let Some(cs) = catch_span {
                                    let catch_scope_idx = self.scopes.len();
                                    self.scopes.push(Scope {
                                        parent: Some(parent_scope),
                                        span: cs,
                                        bindings: Vec::new(),
                                    });
                                    if let Some(ns) = find_symbol_span(&var_name, &cs, symbol_spans)
                                    {
                                        self.scopes[catch_scope_idx].bindings.push(Binding {
                                            name: var_name,
                                            def_span: ns,
                                        });
                                    }
                                    for handler in &clause[2..] {
                                        self.walk_expr(
                                            handler,
                                            catch_scope_idx,
                                            span_map,
                                            symbol_spans,
                                        );
                                    }
                                }
                                continue;
                            }
                        }
                    }
                }
            }
            // Non-catch body expressions
            self.walk_expr(item, parent_scope, span_map, symbol_spans);
        }
    }

    /// `(for ((var expr)...) body...)` and similar for-variants.
    fn walk_for(
        &mut self,
        items: &[Value],
        expr: &Value,
        parent_scope: usize,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if items.len() < 2 {
            return;
        }
        let form_span = match expr_span(expr, span_map) {
            Some(s) => s,
            None => return,
        };

        let body_scope_idx = self.scopes.len();
        self.scopes.push(Scope {
            parent: Some(parent_scope),
            span: form_span,
            bindings: Vec::new(),
        });

        if let Some(bindings) = items[1].as_list() {
            for binding in bindings {
                if let Some(pair) = binding.as_list() {
                    if pair.len() >= 2 {
                        // Iterator exprs in outer scope
                        self.walk_expr(&pair[1], parent_scope, span_map, symbol_spans);
                        self.collect_param_binding(
                            &pair[0],
                            body_scope_idx,
                            &form_span,
                            span_map,
                            symbol_spans,
                        );
                    }
                }
            }
        }

        for item in &items[2..] {
            self.walk_expr(item, body_scope_idx, span_map, symbol_spans);
        }
    }

    /// Collect a binding for a single parameter (symbol or destructuring pattern).
    #[allow(clippy::only_used_in_recursion)]
    fn collect_param_binding(
        &mut self,
        param: &Value,
        scope_idx: usize,
        enclosing_span: &Span,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if let Some(name) = param.as_symbol() {
            // Skip the dot separator in rest params
            if name == "." {
                return;
            }
            if let Some(ns) = find_symbol_span(&name, enclosing_span, symbol_spans) {
                self.scopes[scope_idx]
                    .bindings
                    .push(Binding { name, def_span: ns });
            }
        } else if let Some(items) = param.as_vector() {
            // Vector destructuring: [a b c]
            for item in items {
                self.collect_param_binding(item, scope_idx, enclosing_span, span_map, symbol_spans);
            }
        } else if let Some(items) = param.as_list() {
            // List pattern — could be nested destructuring
            for item in items {
                self.collect_param_binding(item, scope_idx, enclosing_span, span_map, symbol_spans);
            }
        } else if let Some(map) = param.as_map_ref() {
            // Map destructuring: {:keys [a b c]} or {:name name, :age age}
            let keys_kw = Value::keyword("keys");
            for (k, v) in map.iter() {
                if *k == keys_kw {
                    // {:keys [a b c]} — each symbol in the vector/list is a binding
                    let syms = v.as_vector().or_else(|| v.as_list());
                    if let Some(syms) = syms {
                        for sym in syms {
                            self.collect_param_binding(
                                sym,
                                scope_idx,
                                enclosing_span,
                                span_map,
                                symbol_spans,
                            );
                        }
                    }
                } else {
                    // Explicit key-pattern pair: the value is a sub-pattern
                    self.collect_param_binding(
                        v,
                        scope_idx,
                        enclosing_span,
                        span_map,
                        symbol_spans,
                    );
                }
            }
        }
    }

    /// Collect symbol bindings from a match pattern.
    #[allow(clippy::only_used_in_recursion)]
    fn collect_pattern_bindings(
        &mut self,
        pattern: &Value,
        scope_idx: usize,
        enclosing_span: &Span,
        span_map: &SpanMap,
        symbol_spans: &[(String, Span)],
    ) {
        if let Some(name) = pattern.as_symbol() {
            // In match patterns, bare symbols are bindings unless they're
            // literals like `_`, `true`, `false`, `nil`.
            if name != "_" && name != "true" && name != "false" && name != "nil" {
                if let Some(ns) = find_symbol_span(&name, enclosing_span, symbol_spans) {
                    self.scopes[scope_idx]
                        .bindings
                        .push(Binding { name, def_span: ns });
                }
            }
        } else if let Some(items) = pattern.as_list() {
            // (cons h t) or (list a b c) — skip the head keyword
            if !items.is_empty() {
                if let Some(head) = items[0].as_symbol() {
                    if matches!(head.as_str(), "cons" | "list" | "quote" | "vector") {
                        for item in &items[1..] {
                            self.collect_pattern_bindings(
                                item,
                                scope_idx,
                                enclosing_span,
                                span_map,
                                symbol_spans,
                            );
                        }
                        return;
                    }
                }
                // Generic list pattern
                for item in items {
                    self.collect_pattern_bindings(
                        item,
                        scope_idx,
                        enclosing_span,
                        span_map,
                        symbol_spans,
                    );
                }
            }
        } else if let Some(items) = pattern.as_vector() {
            for item in items {
                self.collect_pattern_bindings(
                    item,
                    scope_idx,
                    enclosing_span,
                    span_map,
                    symbol_spans,
                );
            }
        } else if let Some(map) = pattern.as_map_ref() {
            // Map destructuring in match: {:keys [a b c]} or {:key pattern}
            let keys_kw = Value::keyword("keys");
            for (k, v) in map.iter() {
                if *k == keys_kw {
                    let syms = v.as_vector().or_else(|| v.as_list());
                    if let Some(syms) = syms {
                        for sym in syms {
                            self.collect_pattern_bindings(
                                sym,
                                scope_idx,
                                enclosing_span,
                                span_map,
                                symbol_spans,
                            );
                        }
                    }
                } else {
                    self.collect_pattern_bindings(
                        v,
                        scope_idx,
                        enclosing_span,
                        span_map,
                        symbol_spans,
                    );
                }
            }
        }
    }

    // ── Query methods ────────────────────────────────────────────

    /// Resolve which definition a symbol at the given position (1-indexed) belongs to.
    /// Returns the scope index and definition span, or `None` if the symbol is
    /// not defined in any local scope (i.e., it's a global/builtin).
    pub fn resolve_at(&self, name: &str, line: usize, col: usize) -> Option<ResolvedSymbol> {
        // Find the innermost scope containing this position
        let scope_idx = self.innermost_scope_at(line, col);

        // Walk up the scope chain looking for a binding of `name`
        let mut idx = scope_idx;
        loop {
            let scope = &self.scopes[idx];
            for binding in &scope.bindings {
                if binding.name == name {
                    return Some(ResolvedSymbol {
                        scope_idx: idx,
                        def_span: binding.def_span,
                        is_top_level: idx == 0,
                    });
                }
            }
            match scope.parent {
                Some(parent) => idx = parent,
                None => return None,
            }
        }
    }

    /// Find all occurrences of `name` that resolve to the same definition as
    /// the occurrence at `(line, col)`. Uses symbol_spans to enumerate all
    /// occurrences, then filters to those that resolve to the same scope+binding.
    pub fn find_scope_aware_references(
        &self,
        name: &str,
        line: usize,
        col: usize,
        symbol_spans: &[(String, Span)],
    ) -> Vec<Span> {
        let resolved = match self.resolve_at(name, line, col) {
            Some(r) => r,
            None => {
                // Not a locally-defined symbol. For top-level/global symbols,
                // return None to signal the caller should fall back to the
                // global (all-occurrences) behavior.
                return Vec::new();
            }
        };

        // Collect all occurrences of `name` that resolve to the same definition
        let mut refs = Vec::new();
        for (sym_name, sym_span) in symbol_spans {
            if sym_name != name {
                continue;
            }
            // Check if this occurrence resolves to the same definition
            if let Some(other_resolved) = self.resolve_at(name, sym_span.line, sym_span.col) {
                if other_resolved.scope_idx == resolved.scope_idx
                    && other_resolved.def_span == resolved.def_span
                {
                    refs.push(*sym_span);
                }
            }
        }
        refs
    }

    /// Check if the symbol at the given position is locally scoped (not top-level).
    pub fn is_locally_scoped(&self, name: &str, line: usize, col: usize) -> bool {
        matches!(self.resolve_at(name, line, col), Some(r) if !r.is_top_level)
    }

    /// Return all bindings visible at the given position (1-indexed).
    /// Walks from the innermost scope outward, collecting binding names.
    /// Stops before adding shadowed names (if a name was already seen in a
    /// more inner scope, skip it from outer scopes).
    /// Excludes bindings from scope index 0 (top-level).
    pub fn visible_bindings_at(&self, line: usize, col: usize) -> Vec<(String, Span)> {
        let scope_idx = self.innermost_scope_at(line, col);
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        let mut idx = scope_idx;
        loop {
            if idx == 0 {
                break;
            }
            let scope = &self.scopes[idx];
            for binding in &scope.bindings {
                if seen.insert(binding.name.clone()) {
                    result.push((binding.name.clone(), binding.def_span));
                }
            }
            match scope.parent {
                Some(parent) => idx = parent,
                None => break,
            }
        }

        result
    }

    /// Find the innermost scope that contains the given position.
    /// Scopes are appended in tree-walk order (nested scopes have higher indices),
    /// so iterating in reverse finds the most nested match first.
    fn innermost_scope_at(&self, line: usize, col: usize) -> usize {
        for idx in (1..self.scopes.len()).rev() {
            if span_contains_pos(&self.scopes[idx].span, line, col) {
                return idx;
            }
        }
        0 // top-level scope
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: parse source, build scope tree, return it with symbol_spans.
    fn build_scope(src: &str) -> (ScopeTree, Vec<(String, Span)>) {
        let (ast, span_map, symbol_spans) = sema_reader::read_many_with_symbol_spans(src).unwrap();
        let tree = ScopeTree::build(&ast, &span_map, &symbol_spans);
        (tree, symbol_spans)
    }

    // ── Basic resolution ─────────────────────────────────────────

    #[test]
    fn top_level_define_resolves() {
        let (tree, _) = build_scope("(define x 42)");
        let resolved = tree.resolve_at("x", 1, 9);
        assert!(resolved.is_some());
        assert!(resolved.unwrap().is_top_level);
    }

    #[test]
    fn unknown_symbol_returns_none() {
        let (tree, _) = build_scope("(define x 42)");
        assert!(tree.resolve_at("y", 1, 1).is_none());
    }

    #[test]
    fn lambda_param_shadows_top_level() {
        // (define x 1)
        // (lambda (x) x)
        let src = "(define x 1)\n(lambda (x) x)";
        let (tree, _) = build_scope(src);

        // Top-level x
        let top = tree.resolve_at("x", 1, 9).unwrap();
        assert!(top.is_top_level);

        // x inside lambda body — should resolve to the parameter, not top-level
        let inner = tree.resolve_at("x", 2, 13).unwrap();
        assert!(!inner.is_top_level);
        assert_ne!(top.scope_idx, inner.scope_idx);
    }

    #[test]
    fn let_binding_scoped() {
        let src = "(define x 1)\n(let ((x 2)) x)";
        let (tree, _) = build_scope(src);

        let top = tree.resolve_at("x", 1, 9).unwrap();
        assert!(top.is_top_level);

        // x in let body
        let inner = tree.resolve_at("x", 2, 14).unwrap();
        assert!(!inner.is_top_level);
    }

    // ── Scope-aware references ───────────────────────────────────

    #[test]
    fn references_only_same_scope() {
        let src = "(define x 1)\n(defun f (x) (+ x 1))";
        let (tree, sym_spans) = build_scope(src);

        // References for the param `x` inside the defun body
        // The `x` param is at line 2, and the body `x` is also at line 2
        let param_x_span = sym_spans
            .iter()
            .find(|(n, s)| n == "x" && s.line == 2 && s.col == 11)
            .map(|(_, s)| s);
        assert!(param_x_span.is_some(), "should find param x");

        let refs = tree.find_scope_aware_references("x", 2, 11, &sym_spans);
        // Should include the param definition and the body usage, but NOT the top-level define
        assert!(
            refs.len() >= 2,
            "expected at least 2 refs, got {}",
            refs.len()
        );
        // None of the refs should be on line 1 (the top-level define)
        for r in &refs {
            assert_eq!(
                r.line, 2,
                "expected all refs on line 2, got line {}",
                r.line
            );
        }
    }

    #[test]
    fn top_level_references_exclude_shadowed() {
        let src = "(define x 1)\n(defun f (x) x)\n(+ x 1)";
        let (tree, sym_spans) = build_scope(src);

        // References for top-level x (line 1, col 9 or line 3)
        let refs = tree.find_scope_aware_references("x", 3, 4, &sym_spans);
        // Should include line 1 (define x) and line 3 (+ x 1)
        // but NOT line 2 (param x or body x)
        for r in &refs {
            assert_ne!(r.line, 2, "top-level x should not include shadowed param x");
        }
    }

    #[test]
    fn defun_name_is_top_level() {
        let src = "(defun foo (x) x)";
        let (tree, _) = build_scope(src);
        let resolved = tree.resolve_at("foo", 1, 8);
        assert!(resolved.is_some());
        assert!(resolved.unwrap().is_top_level);
    }

    #[test]
    fn nested_lambda_scoping() {
        let src = "(lambda (x) (lambda (y) (+ x y)))";
        let (tree, _) = build_scope(src);

        // x should resolve in the outer lambda scope
        let x_outer = tree.resolve_at("x", 1, 10).unwrap();
        // x should also resolve inside the inner lambda (from outer scope)
        let x_inner = tree.resolve_at("x", 1, 28).unwrap();
        assert_eq!(x_outer.scope_idx, x_inner.scope_idx);

        // y should only resolve inside the inner lambda
        let y_inner = tree.resolve_at("y", 1, 22).unwrap();
        assert_ne!(y_inner.scope_idx, x_outer.scope_idx);
    }

    #[test]
    fn let_star_sequential() {
        let src = "(let* ((x 1) (y x)) y)";
        let (tree, _) = build_scope(src);

        let x = tree.resolve_at("x", 1, 9).unwrap();
        let y = tree.resolve_at("y", 1, 15).unwrap();
        // Each let* binding creates a nested scope: y's scope is a child of x's scope
        assert_ne!(x.scope_idx, y.scope_idx);
        // The x referenced in y's init `(y x)` should resolve to the same def as x's binding
        let x_in_y_init = tree.resolve_at("x", 1, 18).unwrap();
        assert_eq!(x.def_span, x_in_y_init.def_span);
    }

    #[test]
    fn define_function_shorthand() {
        let src = "(define (square x) (* x x))";
        let (tree, _) = build_scope(src);

        // square is top-level
        let sq = tree.resolve_at("square", 1, 10).unwrap();
        assert!(sq.is_top_level);

        // x is in the function body scope
        let x = tree.resolve_at("x", 1, 23).unwrap();
        assert!(!x.is_top_level);
    }

    #[test]
    fn unresolved_returns_empty_refs() {
        let src = "(+ x 1)";
        let (tree, sym_spans) = build_scope(src);
        // x is not defined anywhere — should return empty
        let refs = tree.find_scope_aware_references("x", 1, 4, &sym_spans);
        assert!(refs.is_empty());
    }

    // ── Edge-case tests ──────────────────────────────────────────

    #[test]
    fn named_let_scoping() {
        let src = "(let loop ((i 0)) (if (< i 10) (loop (+ i 1)) i))";
        let (tree, _) = build_scope(src);

        // `loop` should be locally scoped inside the let body
        assert!(tree.is_locally_scoped("loop", 1, 32));
        // `i` should be locally scoped inside the let body
        assert!(tree.is_locally_scoped("i", 1, 25));
    }

    #[test]
    fn destructuring_let_scoping() {
        let src = "(let (([a b] (list 1 2))) (+ a b))";
        let (tree, _) = build_scope(src);

        // `a` and `b` should be locally scoped
        assert!(tree.is_locally_scoped("a", 1, 30));
        assert!(tree.is_locally_scoped("b", 1, 32));
    }

    #[test]
    fn letrec_mutual_recursion() {
        let src = "(letrec ((f (lambda (x) (g x))) (g (lambda (x) x))) (f 1))";
        let (tree, _) = build_scope(src);

        // `f` and `g` should both be locally scoped
        let f = tree.resolve_at("f", 1, 54).unwrap();
        let g = tree.resolve_at("g", 1, 26).unwrap();
        assert!(!f.is_top_level);
        assert!(!g.is_top_level);
        // Both should be in the same scope
        assert_eq!(f.scope_idx, g.scope_idx);
    }

    #[test]
    fn defmacro_params_scoping() {
        let src = "(defmacro when (test . body) body)";
        let (tree, _) = build_scope(src);

        // `when` should be top-level
        let when_resolved = tree.resolve_at("when", 1, 11).unwrap();
        assert!(when_resolved.is_top_level);

        // `test` and `body` should be locally scoped
        assert!(tree.is_locally_scoped("test", 1, 17));
        assert!(tree.is_locally_scoped("body", 1, 30));
    }

    #[test]
    fn multiple_definitions_same_name_different_scopes() {
        let src = "(defun f (x) x)\n(defun g (x) x)";
        let (tree, _) = build_scope(src);

        // `x` in f body (line 1)
        let x_in_f = tree.resolve_at("x", 1, 14).unwrap();
        // `x` in g body (line 2)
        let x_in_g = tree.resolve_at("x", 2, 14).unwrap();

        assert!(!x_in_f.is_top_level);
        assert!(!x_in_g.is_top_level);
        // They should be in different scopes
        assert_ne!(x_in_f.scope_idx, x_in_g.scope_idx);
    }

    #[test]
    fn visible_bindings_at_nested_let() {
        let src = "(let ((a 1)) (let ((b 2)) (+ a b)))";
        let (tree, _) = build_scope(src);

        // At the inner position (+ a b), both `a` and `b` should be visible
        let inner_bindings = tree.visible_bindings_at(1, 28);
        let inner_names: Vec<&str> = inner_bindings.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            inner_names.contains(&"a"),
            "inner should see 'a', got {:?}",
            inner_names
        );
        assert!(
            inner_names.contains(&"b"),
            "inner should see 'b', got {:?}",
            inner_names
        );

        // Use a multi-line version to test outer-only position
        let src2 = "(define z 0)\n(let ((a 1))\n  (let ((b 2))\n    (+ a b)))";
        let (tree2, _) = build_scope(src2);

        // Line 4 col 8 is inside both lets — should see a and b
        let inner2 = tree2.visible_bindings_at(4, 8);
        let inner2_names: Vec<&str> = inner2.iter().map(|(n, _)| n.as_str()).collect();
        assert!(inner2_names.contains(&"a"), "inner should see 'a'");
        assert!(inner2_names.contains(&"b"), "inner should see 'b'");

        // Line 1 is top-level (before the let) — no local bindings
        let top = tree2.visible_bindings_at(1, 1);
        assert!(top.is_empty(), "top-level should have no local bindings");
    }

    #[test]
    fn visible_bindings_at_shadowing() {
        let src = "(let ((x 1)) (let ((x 2)) x))";
        let (tree, _) = build_scope(src);

        // At the inner `x` usage, only one `x` should appear (the inner one)
        let bindings = tree.visible_bindings_at(1, 27);
        let x_bindings: Vec<_> = bindings.iter().filter(|(n, _)| n == "x").collect();
        assert_eq!(
            x_bindings.len(),
            1,
            "shadowed x should appear only once, got {:?}",
            x_bindings
        );
    }

    // ── try/catch scoping ────────────────────────────────────────

    #[test]
    fn try_catch_binds_error_var() {
        let src = "(try (/ 1 0) (catch e (println e)))";
        let (tree, _) = build_scope(src);
        // 'e' should be locally scoped inside the catch clause
        assert!(tree.is_locally_scoped("e", 1, 31));
    }

    #[test]
    fn try_catch_error_var_not_visible_outside() {
        let src = "(try (/ 1 0) (catch e (println e)))\n(+ 1 e)";
        let (tree, _) = build_scope(src);
        // 'e' on line 2 should NOT resolve (not in catch scope)
        assert!(!tree.is_locally_scoped("e", 2, 6));
    }

    // ── for-loop scoping ─────────────────────────────────────────

    #[test]
    fn for_binds_loop_variable() {
        let src = "(for ((x (list 1 2 3))) (println x))";
        let (tree, _) = build_scope(src);
        // 'x' should be locally scoped inside the for body
        assert!(tree.is_locally_scoped("x", 1, 34));
    }

    #[test]
    fn for_list_binds_variable() {
        let src = "(for/list ((x (range 10))) (* x x))";
        let (tree, _) = build_scope(src);
        assert!(tree.is_locally_scoped("x", 1, 30));
    }

    #[test]
    fn for_variable_not_visible_outside() {
        let src = "(for ((x (list 1 2))) x)\n(+ 1 x)";
        let (tree, _) = build_scope(src);
        assert!(!tree.is_locally_scoped("x", 2, 6));
    }

    // ── do-loop scoping ──────────────────────────────────────────

    #[test]
    fn do_binds_iteration_vars() {
        let src = "(do ((i 0 (+ i 1))) ((= i 10) i) (println i))";
        let (tree, _) = build_scope(src);
        // 'i' should be locally scoped in the body
        assert!(tree.is_locally_scoped("i", 1, 44));
    }

    // ── match scoping ────────────────────────────────────────────

    #[test]
    fn match_binds_pattern_variables() {
        let src = "(match x (y (+ y 1)))";
        let (tree, _) = build_scope(src);
        // 'y' should be locally scoped in the match clause body
        assert!(tree.is_locally_scoped("y", 1, 16));
    }

    #[test]
    fn match_wildcard_not_bound() {
        let src = "(match x (_ 42))";
        let (tree, _) = build_scope(src);
        // '_' should NOT be bound
        assert!(!tree.is_locally_scoped("_", 1, 11));
    }

    #[test]
    fn match_cons_pattern_binds_parts() {
        let src = "(match lst ((cons h t) (+ h t)))";
        let (tree, _) = build_scope(src);
        assert!(tree.is_locally_scoped("h", 1, 25));
        assert!(tree.is_locally_scoped("t", 1, 27));
    }

    #[test]
    fn match_clauses_have_separate_scopes() {
        let src = "(match x (a (+ a 1)) (b (* b 2)))";
        let (tree, _) = build_scope(src);
        let a_resolved = tree.resolve_at("a", 1, 15).unwrap();
        let b_resolved = tree.resolve_at("b", 1, 29).unwrap();
        // Each clause has its own scope
        assert_ne!(a_resolved.scope_idx, b_resolved.scope_idx);
    }

    // ── defagent/deftool scoping ─────────────────────────────────

    #[test]
    fn defagent_name_is_top_level() {
        let src = "(defagent my-agent :model \"claude\")";
        let (tree, _) = build_scope(src);
        let resolved = tree.resolve_at("my-agent", 1, 12);
        assert!(resolved.is_some());
        assert!(resolved.unwrap().is_top_level);
    }

    #[test]
    fn deftool_name_is_top_level() {
        let src = "(deftool weather (loc) \"Get weather\" loc)";
        let (tree, _) = build_scope(src);
        let resolved = tree.resolve_at("weather", 1, 10);
        assert!(resolved.is_some());
        assert!(resolved.unwrap().is_top_level);
    }

    // ── fn (lambda alias) scoping ────────────────────────────────

    #[test]
    fn fn_params_scoped() {
        let src = "(fn (x y) (+ x y))";
        let (tree, _) = build_scope(src);
        assert!(tree.is_locally_scoped("x", 1, 15));
        assert!(tree.is_locally_scoped("y", 1, 17));
    }

    // ── visible_bindings_at edge cases ───────────────────────────

    #[test]
    fn visible_bindings_at_lambda_params() {
        let src = "(lambda (a b) (+ a b))";
        let (tree, _) = build_scope(src);
        let bindings = tree.visible_bindings_at(1, 16);
        let names: Vec<&str> = bindings.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"a"), "should see 'a', got {names:?}");
        assert!(names.contains(&"b"), "should see 'b', got {names:?}");
    }

    #[test]
    fn visible_bindings_empty_for_top_level() {
        let src = "(define x 42)";
        let (tree, _) = build_scope(src);
        // Top-level has no local bindings
        let bindings = tree.visible_bindings_at(1, 1);
        assert!(bindings.is_empty());
    }

    // ── scope-aware references edge cases ────────────────────────

    #[test]
    fn references_include_definition_site() {
        let src = "(defun f (x) (+ x 1))";
        let (tree, sym_spans) = build_scope(src);
        // References for 'x' at the body usage
        let refs = tree.find_scope_aware_references("x", 1, 18, &sym_spans);
        // Should include both the param definition and the body usage
        assert!(refs.len() >= 2, "expected >= 2 refs, got {}", refs.len());
    }

    #[test]
    fn references_nested_scopes_independent() {
        // Two lambdas with the same param name — refs in one should not leak to other
        let src = "(lambda (x) x)\n(lambda (x) x)";
        let (tree, sym_spans) = build_scope(src);
        let refs_first = tree.find_scope_aware_references("x", 1, 13, &sym_spans);
        let refs_second = tree.find_scope_aware_references("x", 2, 13, &sym_spans);
        // Each should only contain refs from its own scope
        for r in &refs_first {
            assert_eq!(r.line, 1, "first lambda refs should be on line 1");
        }
        for r in &refs_second {
            assert_eq!(r.line, 2, "second lambda refs should be on line 2");
        }
    }

    // ── def alias ────────────────────────────────────────────────

    #[test]
    fn def_alias_resolves() {
        let src = "(def y 99)";
        let (tree, _) = build_scope(src);
        let resolved = tree.resolve_at("y", 1, 6);
        assert!(resolved.is_some());
        assert!(resolved.unwrap().is_top_level);
    }

    // ── map destructuring ────────────────────────────────────────

    #[test]
    fn map_destructuring_keys_shorthand() {
        // {:keys [a b]} — both a and b should be bound in the lambda scope
        let src = "(fn ({:keys [a b]}) (+ a b))";
        let (tree, _) = build_scope(src);
        let a = tree.resolve_at("a", 1, 24);
        let b = tree.resolve_at("b", 1, 26);
        assert!(a.is_some(), "a should be bound via :keys destructuring");
        assert!(b.is_some(), "b should be bound via :keys destructuring");
        assert!(!a.unwrap().is_top_level);
        assert!(!b.unwrap().is_top_level);
    }

    #[test]
    fn map_destructuring_explicit_pairs() {
        // {:name n} — n should be bound in the lambda scope
        let src = "(fn ({:name n}) n)";
        let (tree, _) = build_scope(src);
        let n = tree.resolve_at("n", 1, 17);
        assert!(
            n.is_some(),
            "n should be bound via explicit map destructuring"
        );
        assert!(!n.unwrap().is_top_level);
    }

    #[test]
    fn map_destructuring_in_let() {
        let src = "(let (({:keys [x]} (hash-map :x 1))) x)";
        let (tree, _) = build_scope(src);
        let x = tree.resolve_at("x", 1, 38);
        assert!(x.is_some(), "x should be bound via :keys in let");
    }
}
