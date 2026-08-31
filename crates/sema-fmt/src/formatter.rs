//! The formatting pipeline, in order:
//!
//! 1. **Tokenize** — `sema_reader::lexer::tokenize` produces a flat token
//!    stream that includes comments and newlines (unlike the reader proper).
//! 2. **Build nodes** — [`build_nodes`] turns tokens into a lightweight
//!    [`Node`] tree. String-like and numeric literals keep their original
//!    source text so they round-trip byte-for-byte.
//! 3. **Classify** — each list form is classified by its head symbol into a
//!    [`FormKind`], which selects the layout strategy.
//! 4. **Measure** — [`measure_width`] computes the single-line width of a
//!    node so the formatter can decide between flat and multi-line layout.
//! 5. **Emit** — [`Formatter`] walks the tree and appends to its output
//!    buffer, dispatching per [`FormKind`].
//!
//! Two invariants the tests enforce: formatting is **idempotent**
//! (`fmt(fmt(x)) == fmt(x)`) and **comment-preserving**.

use std::borrow::Cow;

use sema_core::SemaError;
use sema_reader::lexer::{tokenize, FStringPart, SpannedToken, Token};

// ---------------------------------------------------------------------------
// Node tree — lightweight structure built from the flat token stream
// ---------------------------------------------------------------------------

/// A source-faithful syntax tree node.
///
/// Unlike the reader's `Value` AST, this tree keeps comments, blank lines,
/// and the original source text of literals, so the formatter can reproduce
/// anything it doesn't deliberately rewrite.
#[derive(Debug, Clone)]
enum Node {
    /// A single semantic token (symbol, number, string, keyword, bool, char, dot, etc.)
    Atom(Token),
    /// A string/fstring/regex token with original source text preserved exactly.
    /// This ensures multi-line strings, f-strings, and regex literals round-trip correctly.
    StringAtom(String),
    /// A comment (already includes leading semicolons)
    Comment(String),
    /// A newline separator (used to track blank lines between forms)
    Newline,
    /// `(` ... `)`
    List(Vec<Node>),
    /// `[` ... `]`
    Vector(Vec<Node>),
    /// `{` ... `}`
    Map(Vec<Node>),
    /// `#(` ... `)`
    ShortLambda(Vec<Node>),
    /// `#u8(` ... `)`
    ByteVector(Vec<Node>),
    /// Quote / quasiquote / unquote / unquote-splice prefix attached to the
    /// following node.
    Prefix(Token, Box<Node>),
}

// ---------------------------------------------------------------------------
// Building the node tree from the flat token stream
// ---------------------------------------------------------------------------

/// Maximum nesting depth for parsing and formatting. Keeps the recursive node
/// builder and formatter from overflowing the stack on adversarial input.
/// Deliberately lower than the reader's 1024: the formatter's stack frames
/// are much larger than the reader's, and 2 MiB threads (Rust test/worker
/// default) overflow well below 400 levels in debug builds. No real program
/// nests anywhere near this deep.
const MAX_DEPTH: usize = 200;

/// Build the [`Node`] tree for a whole token stream (one node per top-level
/// form, comment, or newline). `source` is needed to recover the original
/// text of string/number literals via token byte spans.
fn build_nodes(tokens: &[SpannedToken], source: &str) -> Result<Vec<Node>, SemaError> {
    let mut pos = 0;
    let mut nodes = Vec::new();
    while pos < tokens.len() {
        let (node, next) = build_one(tokens, pos, source, 0)?;
        nodes.push(node);
        pos = next;
    }
    Ok(nodes)
}

/// Parse one node starting at `pos`, returning `(node, next_pos)`.
fn build_one(
    tokens: &[SpannedToken],
    pos: usize,
    source: &str,
    depth: usize,
) -> Result<(Node, usize), SemaError> {
    if depth > MAX_DEPTH {
        return Err(SemaError::eval(format!(
            "input nested too deeply (limit {MAX_DEPTH})"
        )));
    }
    if pos >= tokens.len() {
        return Err(SemaError::eval("unexpected end of token stream"));
    }
    let st = &tokens[pos];
    match &st.token {
        Token::Comment(text) => Ok((Node::Comment(text.clone()), pos + 1)),
        Token::Newline => Ok((Node::Newline, pos + 1)),

        // String/FString/Regex/Numbers — preserve original source text for exact round-tripping
        Token::String(_)
        | Token::FString(_)
        | Token::Regex(_)
        | Token::Int(_)
        | Token::Float(_) => {
            let raw = &source[st.byte_start..st.byte_end];
            Ok((Node::StringAtom(raw.to_string()), pos + 1))
        }

        // Prefix tokens — attach to the following node. Newlines between the
        // prefix and its target are skipped (the reader lets a quote apply
        // across a line break); a comment still becomes the inner node so it
        // is preserved, and emission keeps a space before it.
        Token::Quote | Token::Quasiquote | Token::Unquote | Token::UnquoteSplice | Token::Deref => {
            let prefix_tok = st.token.clone();
            let mut next_pos = pos + 1;
            while next_pos < tokens.len() && matches!(tokens[next_pos].token, Token::Newline) {
                next_pos += 1;
            }
            if next_pos >= tokens.len() {
                return Err(SemaError::eval("prefix token at end of input"));
            }
            let (inner, next) = build_one(tokens, next_pos, source, depth + 1)?;
            Ok((Node::Prefix(prefix_tok, Box::new(inner)), next))
        }

        // Grouped forms
        Token::LParen => build_group(tokens, pos + 1, Token::RParen, source, depth, Node::List),
        Token::LBracket => build_group(
            tokens,
            pos + 1,
            Token::RBracket,
            source,
            depth,
            Node::Vector,
        ),
        Token::LBrace => build_group(tokens, pos + 1, Token::RBrace, source, depth, Node::Map),
        Token::ShortLambdaStart => build_group(
            tokens,
            pos + 1,
            Token::RParen,
            source,
            depth,
            Node::ShortLambda,
        ),
        Token::BytevectorStart => build_group(
            tokens,
            pos + 1,
            Token::RParen,
            source,
            depth,
            Node::ByteVector,
        ),

        // Closing delimiters — should not appear here at top-level
        Token::RParen | Token::RBracket | Token::RBrace => {
            Err(SemaError::eval("unexpected closing delimiter"))
        }

        // Everything else is an atom
        _ => Ok((Node::Atom(st.token.clone()), pos + 1)),
    }
}

fn build_group<F>(
    tokens: &[SpannedToken],
    start: usize,
    closer: Token,
    source: &str,
    depth: usize,
    make: F,
) -> Result<(Node, usize), SemaError>
where
    F: FnOnce(Vec<Node>) -> Node,
{
    let mut pos = start;
    let mut children = Vec::new();
    while pos < tokens.len() {
        if std::mem::discriminant(&tokens[pos].token) == std::mem::discriminant(&closer) {
            return Ok((make(children), pos + 1));
        }
        let (node, next) = build_one(tokens, pos, source, depth + 1)?;
        children.push(node);
        pos = next;
    }
    Err(SemaError::eval("unclosed delimiter"))
}

// ---------------------------------------------------------------------------
// Form classification
// ---------------------------------------------------------------------------

/// Layout strategy for a list form, selected by its head symbol.
/// Each variant maps to one `Formatter::format_*` method.
#[derive(Debug, Clone, Copy, PartialEq)]
enum FormKind {
    Body,      // define, defn, fn, lambda, do, begin, when, unless, module, etc.
    Binding,   // let, let*, letrec, when-let, if-let
    Clause,    // cond, case, match
    Threading, // ->, ->>, as->, some->
    TryCatch,  // try
    Cond,      // if
    Import,    // import, load, require
    Call,      // default function call
}

/// Heads of the simple define family: one binding name/signature plus one
/// value/body. The single source of truth for these keywords — shared by form
/// classification, first-line layout, and `--align` define grouping.
/// (Structurally different definers — `defmulti`, `deftool`, `defagent`,
/// `define-record-type`, `define-syntax`, `define-values` — are handled
/// separately where their shapes need it.)
fn is_define_head(name: &str) -> bool {
    matches!(name, "define" | "def" | "defn" | "defun" | "defmacro")
}

/// How many semantic nodes form a define's "signature" (everything left of
/// the value/body): `(define name value)` / `(define (f x) body)` have a
/// 2-node signature; `(defn name (params) body)` has 3. A one-liner define
/// therefore has exactly this many semantics plus one body.
fn define_signature_len(name: &str) -> usize {
    match name {
        "defn" | "defun" | "defmacro" => 3,
        _ => 2,
    }
}

/// Classify a list form by its first non-trivia child. Anything whose head
/// is not a recognized symbol formats as a plain [`FormKind::Call`].
fn classify_form(children: &[Node]) -> FormKind {
    // Find the first non-trivia child; only classify if it's a symbol
    let head = children
        .iter()
        .find(|n| !is_trivia(n))
        .and_then(|n| match n {
            Node::Atom(Token::Symbol(s)) => Some(s.as_str()),
            _ => None,
        });

    if head.is_some_and(is_define_head) {
        return FormKind::Body;
    }

    match head {
        Some(
            "fn"
            | "lambda"
            | "do"
            | "begin"
            | "progn"
            | "async"
            | "when"
            | "unless"
            | "guard"
            | "module"
            | "defagent"
            | "deftool"
            | "defmulti"
            | "defmethod"
            | "prompt"
            | "message"
            | "export"
            | "for"
            | "for-each"
            | "for-range"
            | "for-list"
            | "for-map"
            | "for-filter"
            | "for-fold"
            | "dotimes"
            | "while"
            | "with-open-file"
            | "with-exception-handler"
            | "define-values"
            | "define-record-type"
            | "define-syntax"
            | "syntax-rules",
        ) => FormKind::Body,
        Some(
            "let" | "let*" | "letrec" | "let-values" | "let*-values" | "when-let" | "if-let"
            | "parameterize",
        ) => FormKind::Binding,
        Some("cond" | "case" | "match" | "match*") => FormKind::Clause,
        Some("->" | "->>" | "as->" | "some->") => FormKind::Threading,
        Some("try") => FormKind::TryCatch,
        Some("if") => FormKind::Cond,
        Some("import" | "load" | "require") => FormKind::Import,
        _ => FormKind::Call,
    }
}

/// How many semantic nodes precede a clause form's clauses: `cond` has none,
/// `case`/`match`/`match*` scrutinize a subject that belongs on the head line.
fn clause_subject_count(head_name: &str) -> usize {
    if head_name == "cond" {
        0
    } else {
        1
    }
}

/// Gutter between the aligned left column and the right column (and between
/// the right column and an aligned trailing comment).
const ALIGN_GAP: usize = 2;

fn is_trivia(n: &Node) -> bool {
    matches!(n, Node::Comment(_) | Node::Newline)
}

/// The non-trivia (semantic) children of a form, in order.
fn semantic_children(children: &[Node]) -> Vec<&Node> {
    children.iter().filter(|n| !is_trivia(n)).collect()
}

/// Check if a node or any of its descendants contains comments.
fn has_any_comments(node: &Node) -> bool {
    match node {
        Node::Comment(_) => true,
        Node::List(children)
        | Node::Vector(children)
        | Node::Map(children)
        | Node::ShortLambda(children)
        | Node::ByteVector(children) => children.iter().any(has_any_comments),
        Node::Prefix(_, inner) => has_any_comments(inner),
        _ => false,
    }
}

/// Check if a node or any of its descendants contains newlines.
fn has_any_newlines(node: &Node) -> bool {
    match node {
        Node::Newline => true,
        Node::List(children)
        | Node::Vector(children)
        | Node::Map(children)
        | Node::ShortLambda(children)
        | Node::ByteVector(children) => children.iter().any(has_any_newlines),
        Node::Prefix(_, inner) => has_any_newlines(inner),
        _ => false,
    }
}

/// How many "distinguished" args go on the first line for a body form.
fn body_first_line_count(head_name: &str, semantic: &[&Node]) -> usize {
    match head_name {
        "define" | "def" => {
            if semantic.len() > 1 && matches!(semantic[1], Node::List(_)) {
                2 // (define (f x) body...)
            } else {
                semantic.len().min(3) // (define x val)
            }
        }
        // (defn name (params) body...) — head + name + params
        "defn" | "defun" | "defmacro" => 3.min(semantic.len()),
        // (define-record-type Name (ctor ...) pred? (field accessor)...)
        "define-record-type" => 4.min(semantic.len()),
        // (define-syntax name rules...)
        "define-syntax" => 2.min(semantic.len()),
        // deftool/defagent: only head + name on first line (docstring goes on its own line)
        "deftool" | "defagent" => 2.min(semantic.len()),
        // (defmulti name dispatch-fn)
        "defmulti" => 3.min(semantic.len()),
        // (defmethod name dispatch-val (params) body...)
        "defmethod" => 4.min(semantic.len()),
        // (for-fold (accum init) (clause) body...) — both specs on the first line
        "for-fold" => 3.min(semantic.len()),
        // fn/lambda: head + params
        "fn" | "lambda" if semantic.len() > 1 => 2,
        "fn" | "lambda" => 1,
        // head + condition/spec/name on the first line, body below
        "when"
        | "unless"
        | "while"
        | "guard"
        | "define-values"
        | "dotimes"
        | "for"
        | "for-range"
        | "for-list"
        | "for-map"
        | "for-filter"
        | "module"
        | "with-open-file"
        | "with-exception-handler" => 2.min(semantic.len()),
        _ => 1,
    }
}

/// Check if a form should be forced to multi-line layout for structural reasons,
/// even if it would fit on one line.
fn should_force_multiline(kind: FormKind, semantic: &[&Node]) -> bool {
    match kind {
        FormKind::Body => {
            let head_name = match semantic.first() {
                Some(Node::Atom(Token::Symbol(s))) => s.as_str(),
                _ => return false,
            };
            let first_line_count = body_first_line_count(head_name, semantic);
            // Force multi-line if there are 2+ body expressions
            semantic.len() > first_line_count + 1
        }
        FormKind::Binding => {
            // Force multi-line if bindings list has 2+ bindings
            let bindings_idx = if is_named_let(semantic) { 2 } else { 1 };
            if semantic.len() > bindings_idx {
                if let Some(count) = count_bindings(semantic[bindings_idx]) {
                    return count >= 2;
                }
            }
            false
        }
        _ => false,
    }
}

/// Check if this is a named let: (let NAME BINDINGS body...)
fn is_named_let(semantic: &[&Node]) -> bool {
    if semantic.len() >= 3 {
        if let Node::Atom(Token::Symbol(s)) = semantic[0] {
            if s == "let" {
                if let Node::Atom(Token::Symbol(_)) = semantic[1] {
                    return matches!(semantic[2], Node::List(_) | Node::Vector(_));
                }
            }
        }
    }
    false
}

/// Count the number of bindings in a binding list node.
fn count_bindings(node: &Node) -> Option<usize> {
    match node {
        Node::List(children) | Node::Vector(children) => Some(
            children
                .iter()
                .filter(|n| matches!(n, Node::List(_) | Node::Vector(_)))
                .count(),
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Measuring the flat (single-line) width of a node
// ---------------------------------------------------------------------------

const TOO_WIDE: usize = 10_000;

/// Measure the flat width of a node, short-circuiting if it exceeds `budget`.
/// Returns `None` if the node cannot fit (multiline content or exceeds budget).
fn measure_width(node: &Node, budget: usize) -> Option<usize> {
    match node {
        Node::Atom(tok) => {
            let w = token_width(tok);
            if w <= budget {
                Some(w)
            } else {
                None
            }
        }
        Node::StringAtom(raw) => {
            if raw.contains('\n') {
                None
            } else if raw.len() <= budget {
                Some(raw.len())
            } else {
                None
            }
        }
        Node::Comment(text) => {
            if text.len() <= budget {
                Some(text.len())
            } else {
                None
            }
        }
        Node::Newline => Some(0),
        Node::List(children) => grouped_measure_width(children, 1, 1, budget),
        Node::Vector(children) => grouped_measure_width(children, 1, 1, budget),
        Node::Map(children) => grouped_measure_width(children, 1, 1, budget),
        Node::ShortLambda(children) => grouped_measure_width(children, 2, 1, budget),
        Node::ByteVector(children) => grouped_measure_width(children, 4, 1, budget),
        Node::Prefix(tok, inner) => {
            let prefix_w = prefix_text(tok).len();
            if prefix_w > budget {
                return None;
            }
            measure_width(inner, budget - prefix_w).map(|w| prefix_w + w)
        }
    }
}

fn grouped_measure_width(
    children: &[Node],
    open_len: usize,
    close_len: usize,
    budget: usize,
) -> Option<usize> {
    let mut total = open_len + close_len;
    if total > budget {
        return None;
    }
    let mut first = true;
    for child in children {
        if is_trivia(child) {
            continue;
        }
        if !first {
            total += 1; // space separator
            if total > budget {
                return None;
            }
        }
        let remaining = budget - total;
        let w = measure_width(child, remaining)?;
        total += w;
        if total > budget {
            return None;
        }
        first = false;
    }
    Some(total)
}

/// Convenience wrapper: returns the flat width or TOO_WIDE if it doesn't fit.
fn flat_width(node: &Node) -> usize {
    measure_width(node, TOO_WIDE).unwrap_or(TOO_WIDE)
}

// ---------------------------------------------------------------------------
// Token → source text
// ---------------------------------------------------------------------------

/// Compute the flat width of a token without allocating a String.
fn token_width(tok: &Token) -> usize {
    match tok {
        Token::Symbol(s) => s.len(),
        Token::Keyword(s) => s.len() + 1, // ":" prefix
        Token::String(s) => escape_string(s).len() + 2, // quotes
        Token::Int(n) => n.to_string().len(),
        Token::BigInt(n) => n.to_string().len(),
        Token::Rational(r) => r.to_string().len(),
        Token::Float(f) => format_float(*f).len(),
        Token::Bool(true) => 2,
        Token::Bool(false) => 2,
        Token::Char(c) => format_char(*c).len(),
        Token::Dot => 1,
        Token::LParen | Token::RParen => 1,
        Token::LBracket | Token::RBracket => 1,
        Token::LBrace | Token::RBrace => 1,
        Token::Quote | Token::Quasiquote | Token::Unquote => 1,
        Token::UnquoteSplice => 2,
        Token::Deref => 1,
        Token::ShortLambdaStart => 2,
        Token::BytevectorStart => 4,
        Token::Comment(text) => text.len(),
        Token::Newline => 1,
        // FString, Regex, and Complex have variable-length formatted output —
        // fall back to token_text for correctness (rare in width measurement).
        Token::FString(_) | Token::Regex(_) | Token::Complex(_, _) => token_text(tok).len(),
    }
}

fn token_text(tok: &Token) -> Cow<'_, str> {
    match tok {
        Token::Symbol(s) => Cow::Borrowed(s.as_str()),
        Token::Keyword(s) => Cow::Owned(format!(":{s}")),
        Token::String(s) => Cow::Owned(format!("\"{}\"", escape_string(s))),
        Token::FString(parts) => Cow::Owned(format_fstring(parts)),
        Token::Regex(s) => Cow::Owned(format!("#\"{}\"", escape_regex(s))),
        Token::Int(n) => Cow::Owned(n.to_string()),
        Token::BigInt(n) => Cow::Owned(n.to_string()),
        Token::Rational(r) => Cow::Owned(r.to_string()),
        Token::Complex(re, im) => {
            use sema_core::number::{Complex, SemaNumber};
            Cow::Owned(
                SemaNumber::Complex(Box::new(Complex {
                    re: re.clone(),
                    im: im.clone(),
                }))
                .to_string(),
            )
        }
        Token::Float(f) => Cow::Owned(format_float(*f)),
        Token::Bool(true) => Cow::Borrowed("#t"),
        Token::Bool(false) => Cow::Borrowed("#f"),
        Token::Char(c) => Cow::Owned(format_char(*c)),
        Token::Dot => Cow::Borrowed("."),
        Token::LParen => Cow::Borrowed("("),
        Token::RParen => Cow::Borrowed(")"),
        Token::LBracket => Cow::Borrowed("["),
        Token::RBracket => Cow::Borrowed("]"),
        Token::LBrace => Cow::Borrowed("{"),
        Token::RBrace => Cow::Borrowed("}"),
        Token::Quote => Cow::Borrowed("'"),
        Token::Quasiquote => Cow::Borrowed("`"),
        Token::Unquote => Cow::Borrowed(","),
        Token::UnquoteSplice => Cow::Borrowed(",@"),
        Token::Deref => Cow::Borrowed("@"),
        Token::ShortLambdaStart => Cow::Borrowed("#("),
        Token::BytevectorStart => Cow::Borrowed("#u8("),
        Token::Comment(text) => Cow::Borrowed(text.as_str()),
        Token::Newline => Cow::Borrowed("\n"),
    }
}

fn prefix_text(tok: &Token) -> &'static str {
    match tok {
        Token::Quote => "'",
        Token::Quasiquote => "`",
        Token::Unquote => ",",
        Token::UnquoteSplice => ",@",
        Token::Deref => "@",
        _ => "",
    }
}

fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\0' => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out
}

fn escape_regex(s: &str) -> String {
    // For regex, we only need to escape literal double-quotes
    s.replace('"', "\\\"")
}

fn format_fstring(parts: &[FStringPart]) -> String {
    let mut out = String::from("f\"");
    for part in parts {
        match part {
            FStringPart::Literal(s) => {
                // Escape string content but also need to preserve $ that isn't
                // before { in the original.
                for c in s.chars() {
                    match c {
                        '\n' => out.push_str("\\n"),
                        '\t' => out.push_str("\\t"),
                        '\r' => out.push_str("\\r"),
                        '\\' => out.push_str("\\\\"),
                        '"' => out.push_str("\\\""),
                        '\0' => out.push_str("\\0"),
                        _ => out.push(c),
                    }
                }
            }
            FStringPart::Expr(expr) => {
                out.push_str("${");
                out.push_str(expr);
                out.push('}');
            }
        }
    }
    out.push('"');
    out
}

fn format_float(f: f64) -> String {
    if f.fract() == 0.0 && !f.is_infinite() && !f.is_nan() {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

fn format_char(c: char) -> String {
    match c {
        ' ' => "#\\space".to_string(),
        '\n' => "#\\newline".to_string(),
        '\t' => "#\\tab".to_string(),
        '\r' => "#\\return".to_string(),
        '\0' => "#\\nul".to_string(),
        _ => format!("#\\{c}"),
    }
}

// ---------------------------------------------------------------------------
// Formatting engine
// ---------------------------------------------------------------------------

/// The emitter: walks a [`Node`] tree and appends formatted text to `output`.
///
/// Layout decisions follow one rule everywhere: try the flat (single-line)
/// rendering first, and fall back to a multi-line layout chosen by the form's
/// [`FormKind`] when the flat form exceeds `width`, contains comments, was
/// originally multi-line, or is structurally forced (e.g. 2+ body forms).
struct Formatter {
    /// Target maximum line width in columns.
    width: usize,
    /// Spaces per indentation level for body forms.
    indent_size: usize,
    /// When true, column-align consecutive defines, cond clauses, and let bindings.
    align: bool,
    /// Longest run of consecutive blank lines to preserve.
    max_blank_lines: usize,
    /// The accumulated formatted source.
    output: String,
}

impl Formatter {
    fn new(opts: &FormatOptions) -> Self {
        Self {
            width: opts.width,
            indent_size: opts.indent,
            align: opts.align,
            max_blank_lines: opts.max_blank_lines,
            output: String::new(),
        }
    }

    /// Column where the next character will land on the current output line.
    fn current_col(&self) -> usize {
        match self.output.rfind('\n') {
            Some(pos) => self.output.len() - pos - 1,
            None => self.output.len(),
        }
    }

    /// Emit up to `max_blank_lines` blank lines for a run of `newlines`
    /// consecutive newline tokens (N newlines = N-1 blank lines). Assumes the
    /// current line is already terminated.
    fn emit_blank_lines(&mut self, newlines: usize) {
        let blanks = newlines.saturating_sub(1).min(self.max_blank_lines);
        self.output.extend(std::iter::repeat_n('\n', blanks));
    }

    /// Format a sequence of top-level nodes: one form per line, blank-line
    /// runs collapsed to a single blank line, trailing comments kept on the
    /// same line as their form, and (with `align`) consecutive one-liner
    /// defines column-aligned as a group.
    fn format_top_level(&mut self, nodes: &[Node]) {
        let mut i = 0;
        let len = nodes.len();
        // Track whether we've emitted any content yet
        let mut first_content = true;
        // Track consecutive newline count for blank line collapsing
        let mut pending_blank_lines: usize = 0;

        while i < len {
            match &nodes[i] {
                Node::Newline => {
                    pending_blank_lines += 1;
                    i += 1;
                }
                Node::Comment(text) => {
                    if !first_content {
                        // Terminate the current line first, THEN emit the
                        // blank — the other order collapses the blank into
                        // the line terminator.
                        if !self.output.ends_with('\n') {
                            self.output.push('\n');
                        }
                        self.emit_blank_lines(pending_blank_lines);
                    }
                    pending_blank_lines = 0;
                    self.output.push_str(text);
                    self.output.push('\n');
                    first_content = false;
                    i += 1;
                }
                _ => {
                    if !first_content {
                        // Line terminator before the blank (see Comment arm).
                        if !self.output.ends_with('\n') {
                            self.output.push('\n');
                        }
                        self.emit_blank_lines(pending_blank_lines);
                    }
                    pending_blank_lines = 0;

                    // Try to collect a group of consecutive alignable defines
                    if let Some(next_i) = self.try_format_define_run(nodes, i, 0) {
                        if !self.output.ends_with('\n') {
                            self.output.push('\n');
                        }
                        i = next_i;
                        first_content = false;
                        continue;
                    }

                    // Normal (non-aligned) formatting
                    let trailing_comment = self.find_trailing_comment(nodes, i + 1);

                    self.format_node(&nodes[i], 0);
                    if let Some((comment_text, skip_to)) = trailing_comment {
                        self.output.push(' ');
                        self.output.push_str(&comment_text);
                        i = skip_to;
                    } else {
                        i += 1;
                    }
                    if !self.output.ends_with('\n') {
                        self.output.push('\n');
                    }
                    first_content = false;
                }
            }
        }
    }

    /// Look ahead from `start` to see if there is a trailing comment
    /// (a comment that was on the same line as the preceding form).
    /// Returns Some((comment_text, next_pos_after_comment)) if found.
    fn find_trailing_comment(&self, nodes: &[Node], start: usize) -> Option<(String, usize)> {
        // A trailing comment is: possibly nothing, then a Comment, with no
        // Newline in between.
        if start < nodes.len() {
            if let Node::Comment(text) = &nodes[start] {
                return Some((text.clone(), start + 1));
            }
        }
        None
    }

    /// Format a single node at the given indentation level.
    /// Try to render `children` as a single flat line: no inner comments, not
    /// originally multi-line anywhere in the tree (respect layout intent), an
    /// additional caller-supplied guard (e.g. `format_list`'s
    /// `!should_force_multiline`), and the result fits `self.width`. On success,
    /// writes the flat form and returns `true`; on failure, writes nothing and
    /// returns `false` so the caller falls through to its multi-line layout.
    fn try_flat(
        &mut self,
        children: &[Node],
        indent: usize,
        open: &str,
        close: &str,
        extra_ok: bool,
    ) -> bool {
        let has_comments = children.iter().any(has_any_comments);
        let originally_multiline = children.iter().any(has_any_newlines);
        if has_comments || originally_multiline || !extra_ok {
            return false;
        }
        let one_line = flat_string(children, open, close);
        if indent + one_line.len() > self.width {
            return false;
        }
        self.output.push_str(&one_line);
        true
    }

    fn format_node(&mut self, node: &Node, indent: usize) {
        match node {
            Node::Atom(tok) => {
                self.output.push_str(&token_text(tok));
            }
            Node::StringAtom(raw) => {
                self.output.push_str(raw);
            }
            Node::Comment(text) => {
                self.output.push_str(text);
            }
            Node::Newline => {
                // At the formatting level, newlines are handled by the parent logic
            }
            Node::List(children) => {
                self.format_list(children, indent, "(", ")");
            }
            Node::Vector(children) => {
                self.format_collection(children, indent, "[", "]");
            }
            Node::Map(children) => {
                self.format_map(children, indent, "{", "}");
            }
            Node::ShortLambda(children) => {
                self.format_list(children, indent, "#(", ")");
            }
            Node::ByteVector(children) => {
                self.format_collection(children, indent, "#u8(", ")");
            }
            Node::Prefix(tok, inner) => {
                self.output.push_str(prefix_text(tok));
                // A comment directly after a prefix must not fuse with it
                // (`';; c` re-parses the same but flip-flops the spacing).
                if matches!(**inner, Node::Comment(_)) {
                    self.output.push(' ');
                }
                self.format_node(inner, indent);
            }
        }
    }

    /// Format a list form with Lisp-aware indentation.
    fn format_list(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);

        // Empty form
        if semantic.is_empty() {
            return self.format_empty_with_comments(children, indent, open, close);
        }

        // A comment before the head can't survive any specialized first-line
        // layout — use the generic head+body layout, which preserves it.
        if Self::semantics_before_first_comment(children) == 0 {
            return self.format_head_body(children, indent, open, close);
        }

        let kind = classify_form(children);

        // Try one-line format (no structural reason to force multi-line, e.g. 2+
        // body exprs, on top of try_flat's own no-comments/not-originally-multiline/
        // fits-width checks).
        if self.try_flat(
            children,
            indent,
            open,
            close,
            !should_force_multiline(kind, &semantic),
        ) {
            return;
        }

        // Multi-line: dispatch based on form kind
        match kind {
            FormKind::Body => self.format_body(children, indent, open, close),
            FormKind::Binding => self.format_binding(children, indent, open, close),
            FormKind::Clause => self.format_clause(children, indent, open, close),
            FormKind::Threading => self.format_threading(children, indent, open, close),
            FormKind::TryCatch => self.format_body(children, indent, open, close),
            FormKind::Cond => self.format_conditional(children, indent, open, close),
            FormKind::Import => self.format_import(children, indent, open, close),
            FormKind::Call => self.format_call(children, indent, open, close),
        }
    }

    /// Emit a form that has no semantic children but may contain comments:
    /// `open`, each comment on the head line / its own line, then the close
    /// delimiter on a fresh line so it isn't absorbed into a comment.
    /// Empty-form early returns must use this instead of `open`+`close`, or
    /// a comment-only form like `(f ;; c\n)`'s sibling `( ;; c\n)` silently
    /// loses its comment.
    fn format_empty_with_comments(
        &mut self,
        children: &[Node],
        indent: usize,
        open: &str,
        close: &str,
    ) {
        self.output.push_str(open);
        if children.iter().any(|c| matches!(c, Node::Comment(_))) {
            let elem_indent = indent + open.len();
            self.emit_leading_comments(children, elem_indent);
            self.output.push('\n');
            self.push_indent(elem_indent);
        }
        self.output.push_str(close);
    }

    /// Count the semantic nodes that precede the first direct comment child
    /// (`usize::MAX` when there is no comment). Specialized layouts flatten
    /// their first N semantic nodes onto one line, silently deleting any
    /// comment in that region — they must cap N at this value.
    fn semantics_before_first_comment(children: &[Node]) -> usize {
        let mut count = 0;
        for child in children {
            match child {
                Node::Comment(_) => return count,
                _ if is_trivia(child) => {}
                _ => count += 1,
            }
        }
        usize::MAX
    }

    /// Generic comment-safe layout: head on the first line, everything else
    /// (including comments) on its own line at one indent level. The fallback
    /// for comment placements that a specialized layout would delete.
    fn format_head_body(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);
        self.output.push_str(open);
        let elem_indent = indent + open.len();
        if self.emit_leading_comments(children, elem_indent) {
            self.output.push('\n');
            self.push_indent(elem_indent);
        }
        self.format_node(semantic[0], elem_indent);
        let rest_start = Self::index_after_nth_semantic(children, 1);
        self.emit_body_with_comments(children, rest_start, indent + self.indent_size);
        self.output.push_str(close);
    }

    // -----------------------------------------------------------------------
    // Body forms: (define name ...\n  body...)
    // -----------------------------------------------------------------------

    /// Body layout: distinguished args on the first line (how many depends on
    /// the head — see [`body_first_line_count`]), then each body form on its
    /// own line at one indent level:
    ///
    /// ```text
    /// (define (f x)
    ///   body1
    ///   body2)
    /// ```
    fn format_body(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic: Vec<(usize, &Node)> = children
            .iter()
            .enumerate()
            .filter(|(_, n)| !is_trivia(n))
            .collect();

        if semantic.is_empty() {
            return self.format_empty_with_comments(children, indent, open, close);
        }

        let head_name = match &semantic[0].1 {
            Node::Atom(Token::Symbol(s)) => s.as_str(),
            _ => "",
        };
        let semantic_refs: Vec<&Node> = semantic.iter().map(|(_, n)| *n).collect();
        let first_line_count = body_first_line_count(head_name, &semantic_refs);

        // Never flatten past a comment — it would be deleted.
        let first_count = first_line_count
            .min(semantic.len())
            .min(Self::semantics_before_first_comment(children));

        self.output.push_str(open);

        // Always emit head
        self.format_node(semantic[0].1, indent + open.len());
        let mut emitted = 1;

        // Try to put subsequent first-line args on the same line
        let body_indent = indent + self.indent_size;
        for (j, (_orig_idx, node)) in semantic.iter().enumerate().skip(1).take(first_count - 1) {
            let w = flat_width(node);
            let current_col = self.current_col();

            // Check if it fits flat on this line
            if current_col + 1 + w > self.width {
                break;
            }

            let checkpoint = self.output.len();
            self.output.push(' ');
            self.format_node(node, body_indent);

            // If it went multi-line, undo and break
            if self.output[checkpoint..].contains('\n') {
                self.output.truncate(checkpoint);
                break;
            }
            emitted = j + 1;
        }

        // Remaining args as body at indent+2. A module body is otherwise
        // just like top level for `--align` purposes (see
        // emit_body_with_comments_impl's doc comment), so it alone opts into
        // define-run grouping among FormKind::Body's heads.
        let body_start = Self::index_after_nth_semantic(children, emitted);
        self.emit_body_with_comments_impl(children, body_start, body_indent, head_name == "module");

        self.output.push_str(close);
    }

    // -----------------------------------------------------------------------
    // Binding forms: (let ([x 1] [y 2])\n  body...)
    // -----------------------------------------------------------------------

    /// Binding layout: the bindings collection hangs after the head (extra
    /// bindings align under the first), body at one indent level. Handles
    /// named let (`(let loop ([x 1]) ...)`):
    ///
    /// ```text
    /// (let ([x 1]
    ///       [y 2])
    ///   body)
    /// ```
    fn format_binding(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);

        if semantic.len() < 2 {
            // Degenerate, just format as call
            return self.format_call(children, indent, open, close);
        }

        // The first line carries head [+ name] + bindings; a comment in that
        // region would be deleted — fall back to the comment-safe layout.
        let first_line_len = if is_named_let(&semantic) { 3 } else { 2 };
        if Self::semantics_before_first_comment(children) < first_line_len {
            return self.format_head_body(children, indent, open, close);
        }

        self.output.push_str(open);

        // head (let/let*/letrec)
        self.format_node(semantic[0], indent + open.len());
        self.output.push(' ');

        // Check for named let: (let name bindings body...)
        let (bindings_idx, bindings_indent) = if is_named_let(&semantic) {
            let name_col = indent + open.len() + flat_width(semantic[0]) + 1;
            self.format_node(semantic[1], name_col);
            self.output.push(' ');
            let bi = name_col + flat_width(semantic[1]) + 1;
            (2, bi)
        } else {
            let bi = indent + open.len() + flat_width(semantic[0]) + 1;
            (1, bi)
        };

        // Format bindings as a collection (aligns elements under first element)
        match semantic[bindings_idx] {
            Node::List(inner) => {
                self.format_collection(inner, bindings_indent, "(", ")");
            }
            Node::Vector(inner) => {
                self.format_collection(inner, bindings_indent, "[", "]");
            }
            other => self.format_node(other, bindings_indent),
        }

        // body forms with interleaved comments preserved
        let body_indent = indent + self.indent_size;
        let body_start = Self::index_after_nth_semantic(children, bindings_idx + 1);
        self.emit_body_with_comments(children, body_start, body_indent);

        self.output.push_str(close);
    }

    // -----------------------------------------------------------------------
    // Clause forms: (cond\n  (test1 expr1)\n  (test2 expr2))
    // -----------------------------------------------------------------------

    /// Clause layout for cond/case/match: head alone on the first line, each
    /// clause on its own line at one indent level. With `align`, clause
    /// results are column-aligned when they all fit:
    ///
    /// ```text
    /// (cond
    ///   ((= x 1) "one")
    ///   (else    "other"))
    /// ```
    fn format_clause(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);

        if semantic.is_empty() {
            return self.format_empty_with_comments(children, indent, open, close);
        }

        // case/match/match* scrutinize a subject that belongs beside the head
        let head_name = match semantic[0] {
            Node::Atom(Token::Symbol(s)) => s.as_str(),
            _ => "",
        };
        let subjects = clause_subject_count(head_name).min(semantic.len() - 1);

        // A comment between the head and the subject would be deleted by
        // flattening them onto one line.
        if Self::semantics_before_first_comment(children) < 1 + subjects {
            return self.format_head_body(children, indent, open, close);
        }

        self.output.push_str(open);
        // head
        self.format_node(semantic[0], indent + open.len());
        for subject in &semantic[1..1 + subjects] {
            self.output.push(' ');
            self.format_node(subject, indent + self.indent_size);
        }

        let clause_indent = indent + self.indent_size;
        let clause_start = Self::index_after_nth_semantic(children, 1 + subjects);

        // Try aligned clause formatting: collect consecutive clause forms
        // (skipping comments/newlines) and try to align their test/result columns
        let clauses = semantic_children(&children[clause_start..]);

        // Check recursively: a comment INSIDE a clause would be silently
        // deleted by the flat rendering the aligned path uses.
        let has_comments = children[clause_start..].iter().any(has_any_comments);

        if self.align
            && !has_comments
            && clauses.len() >= 2
            && self.try_format_clause_aligned(&clauses, clause_indent)
        {
            self.output.push_str(close);
            return;
        }

        // Fall back to normal body-with-comments
        self.emit_body_with_comments(children, clause_start, clause_indent);

        self.output.push_str(close);
    }

    /// Try to format cond/case/match clauses with aligned result columns.
    fn try_format_clause_aligned(&mut self, clauses: &[&Node], indent: usize) -> bool {
        // All clauses must be flat-renderable 2-element lists/vectors
        let mut splits: Vec<(String, String)> = Vec::new();
        for clause in clauses {
            match Self::split_clause(clause) {
                Some(pair) => {
                    // A raw newline inside a string literal would break the
                    // aligned column — bail to normal formatting.
                    if pair.0.contains('\n') || pair.1.contains('\n') {
                        return false;
                    }
                    splits.push(pair)
                }
                None => return false,
            }
        }

        let max_left = splits
            .iter()
            .map(|(l, _)| display_width(l))
            .max()
            .unwrap_or(0);
        let min_left = splits
            .iter()
            .map(|(l, _)| display_width(l))
            .min()
            .unwrap_or(0);

        // If all lefts are the same width, use normal spacing (no alignment needed)
        let min_gap = if max_left == min_left { 1 } else { ALIGN_GAP };

        // Check all lines fit
        for (_left, right) in &splits {
            let line_width = indent + max_left + min_gap + display_width(right);
            if line_width > self.width {
                return false;
            }
        }

        // Emit aligned clauses
        for (left, right) in &splits {
            self.output.push('\n');
            self.push_indent(indent);
            self.output.push_str(left);
            let pad = max_left - display_width(left) + min_gap;
            for _ in 0..pad {
                self.output.push(' ');
            }
            self.output.push_str(right);
        }
        true
    }

    // -----------------------------------------------------------------------
    // Threading macros: (-> val\n  step1\n  step2)
    // -----------------------------------------------------------------------

    /// Threading layout: head and initial value on the first line, each step
    /// on its own line at one indent level:
    ///
    /// ```text
    /// (-> value
    ///   (step1)
    ///   (step2))
    /// ```
    fn format_threading(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);

        if semantic.len() < 2 {
            return self.format_call(children, indent, open, close);
        }

        // A comment between the head and the initial value would be deleted
        // by flattening them onto one line.
        if Self::semantics_before_first_comment(children) < 2 {
            return self.format_head_body(children, indent, open, close);
        }

        self.output.push_str(open);
        // head (->)
        self.format_node(semantic[0], indent + open.len());
        self.output.push(' ');
        // first value
        self.format_node(semantic[1], indent + self.indent_size);

        // steps with interleaved comments preserved
        let step_indent = indent + self.indent_size;
        let step_start = Self::index_after_nth_semantic(children, 2);
        self.emit_body_with_comments(children, step_start, step_indent);

        self.output.push_str(close);
    }

    // -----------------------------------------------------------------------
    // Conditional: (if test then else)
    // -----------------------------------------------------------------------

    /// Conditional layout for `if`: head and test on the first line, then/else
    /// branches each on their own line at one indent level:
    ///
    /// ```text
    /// (if test
    ///   then-branch
    ///   else-branch)
    /// ```
    fn format_conditional(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);

        // A comment between the head and the test would be deleted by
        // flattening them onto one line.
        if Self::semantics_before_first_comment(children) < 2 {
            return self.format_head_body(children, indent, open, close);
        }

        // Try: head + test on first line, then/else indented
        self.output.push_str(open);
        // head (if)
        self.format_node(semantic[0], indent + open.len());

        if semantic.len() > 1 {
            self.output.push(' ');
            // test
            self.format_node(semantic[1], indent + self.indent_size);
        }

        // then/else branches with interleaved comments preserved
        let body_indent = indent + self.indent_size;
        let body_start = Self::index_after_nth_semantic(children, 2);
        self.emit_body_with_comments(children, body_start, body_indent);

        self.output.push_str(close);
    }

    // -----------------------------------------------------------------------
    // Import: (import "module") or (import\n  "mod1"\n  "mod2")
    // -----------------------------------------------------------------------

    /// Import layout: one line when it fits, otherwise head alone and one
    /// module per line at one indent level.
    fn format_import(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        // Same as body with first_count = 1
        let semantic = semantic_children(children);

        if semantic.is_empty() {
            return self.format_empty_with_comments(children, indent, open, close);
        }

        // Try one-line first (only if no inner comments and not originally multi-line)
        if self.try_flat(children, indent, open, close, true) {
            return;
        }

        self.output.push_str(open);
        self.format_node(semantic[0], indent + open.len());

        // args with interleaved comments preserved
        let arg_indent = indent + self.indent_size;
        let arg_start = Self::index_after_nth_semantic(children, 1);
        self.emit_body_with_comments(children, arg_start, arg_indent);

        self.output.push_str(close);
    }

    // -----------------------------------------------------------------------
    // Default call: (f arg1 arg2 ...) — align args with first arg
    // -----------------------------------------------------------------------

    /// Default call layout: keep the first argument beside the head when it
    /// fits flat, remaining args one per line at one indent level. `hash-map`
    /// and `assoc` divert to [`Self::format_kv_call`] for pairwise layout.
    fn format_call(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);

        if semantic.is_empty() {
            return self.format_empty_with_comments(children, indent, open, close);
        }

        // Detect hash-map/assoc for key-value pair grouping
        let head_name = match semantic[0] {
            Node::Atom(Token::Symbol(s)) => Some(s.as_str()),
            _ => None,
        };
        if matches!(head_name, Some("hash-map" | "assoc")) {
            return self.format_kv_call(children, indent, open, close);
        }

        self.output.push_str(open);
        // head
        self.format_node(semantic[0], indent + open.len());

        if semantic.len() == 1 {
            // A trailing comment after the lone head must survive
            // (`(f ;; c\n)` used to format as `(f)`).
            let rest = Self::index_after_nth_semantic(children, 1);
            self.emit_body_with_comments(children, rest, indent + self.indent_size);
            self.output.push_str(close);
            return;
        }

        // Try: head + first arg on same line
        let head_width = flat_width(semantic[0]);
        let first_arg_col = indent + open.len() + head_width + 1;
        let arg_indent = indent + self.indent_size;

        // Check if head + first arg fits on one line (flat). Never pull the
        // first arg past a comment — it would be deleted.
        if first_arg_col + flat_width(semantic[1]) <= self.width
            && Self::semantics_before_first_comment(children) >= 2
        {
            // Try first arg on same line
            let checkpoint = self.output.len();
            self.output.push(' ');
            self.format_node(semantic[1], arg_indent);

            // If the first arg went multi-line, undo and put everything on new lines
            if self.output[checkpoint..].contains('\n') {
                self.output.truncate(checkpoint);
                // Fall through to all-on-new-lines path below
            } else {
                // First arg stayed single-line, emit rest at indent+2
                let rest_start = Self::index_after_nth_semantic(children, 2);
                self.emit_body_with_comments(children, rest_start, arg_indent);
                self.output.push_str(close);
                return;
            }
        }
        // All args on new lines at indent+2
        let rest_start = Self::index_after_nth_semantic(children, 1);
        self.emit_body_with_comments(children, rest_start, arg_indent);

        self.output.push_str(close);
    }

    // -----------------------------------------------------------------------
    // Key-value call: (hash-map k1 v1 k2 v2) / (assoc m k1 v1 k2 v2)
    // -----------------------------------------------------------------------

    /// Key-value call layout for `(hash-map k v ...)` / `(assoc m k v ...)`:
    /// each key-value pair on its own line, value dropped to a further-indented
    /// line if it can't sit beside its key.
    fn format_kv_call(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);
        let head_name = match semantic[0] {
            Node::Atom(Token::Symbol(s)) => s.as_str(),
            _ => "",
        };

        // For assoc, first arg is the map; for hash-map, all args are kv pairs
        let kv_start = if head_name == "assoc" { 2 } else { 1 };

        // Try one-line first
        if self.try_flat(children, indent, open, close, true) {
            return;
        }

        let pair_indent = indent + self.indent_size;

        // The first line carries the head (and, for assoc, the map arg); a
        // comment in that region would be deleted.
        if Self::semantics_before_first_comment(children) < kv_start {
            return self.format_head_body(children, indent, open, close);
        }

        self.output.push_str(open);
        // head
        self.format_node(semantic[0], indent + open.len());

        // For assoc, emit the map arg first
        if head_name == "assoc" && semantic.len() > 1 {
            self.output.push(' ');
            self.format_node(semantic[1], pair_indent);
        }

        // Emit key-value pairs, each pair on its own line at indent+2,
        // walking the raw children so comments are preserved: a trailing
        // comment stays on its pair's line, a standalone one keeps its line.
        let mut semantic_count = 0;
        let mut saw_newline = false;
        let mut after_comment = false;
        for child in children.iter() {
            match child {
                Node::Newline => saw_newline = true,
                Node::Comment(text) => {
                    if saw_newline || semantic_count < kv_start {
                        self.output.push('\n');
                        self.push_indent(pair_indent);
                    } else {
                        self.output.push(' ');
                    }
                    self.output.push_str(text);
                    after_comment = true;
                    saw_newline = false;
                }
                _ if is_trivia(child) => {}
                _ => {
                    if semantic_count >= kv_start {
                        let is_key = (semantic_count - kv_start) % 2 == 0;
                        if is_key || after_comment {
                            // Keys start a new line; a value after a comment
                            // can't share the comment's line.
                            let node_indent = if is_key {
                                pair_indent
                            } else {
                                pair_indent + self.indent_size
                            };
                            self.output.push('\n');
                            self.push_indent(node_indent);
                            self.format_node(child, node_indent);
                        } else {
                            // Try key + value on one line
                            let key_col = self.current_col();
                            if key_col + 1 + flat_width(child) <= self.width {
                                let checkpoint = self.output.len();
                                self.output.push(' ');
                                self.format_node(child, pair_indent);
                                // If value went multi-line, undo and put on next line
                                if self.output[checkpoint..].contains('\n') {
                                    self.output.truncate(checkpoint);
                                    self.output.push('\n');
                                    self.push_indent(pair_indent + self.indent_size);
                                    self.format_node(child, pair_indent + self.indent_size);
                                }
                            } else {
                                // Value on next line indented further
                                self.output.push('\n');
                                self.push_indent(pair_indent + self.indent_size);
                                self.format_node(child, pair_indent + self.indent_size);
                            }
                        }
                    }
                    semantic_count += 1;
                    after_comment = false;
                    saw_newline = false;
                }
            }
        }

        // The close delimiter must not land inside a trailing comment.
        if after_comment {
            self.output.push('\n');
            self.push_indent(pair_indent);
        }
        self.output.push_str(close);
    }

    // -----------------------------------------------------------------------
    // Collection (vector): [a b c] — one-line or one-per-line
    // -----------------------------------------------------------------------

    /// Collection layout for vectors/bytevectors (also used for let-binding
    /// lists): one line when it fits, otherwise one element per line aligned
    /// under the first. With `align`, 2-element pairs get column alignment.
    fn format_collection(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);

        if semantic.is_empty() {
            return self.format_empty_with_comments(children, indent, open, close);
        }

        // Try one-line (only if no inner comments and not originally multi-line)
        if self.try_flat(children, indent, open, close, true) {
            return;
        }

        // If children contain comments, force multi-line to preserve them
        let has_comments = children.iter().any(has_any_comments);
        let originally_multiline = children.iter().any(has_any_newlines);

        // Multi-line bytevector: preserve the user's own row breaks (a
        // hand-arranged grid is often meaningful); wrap at the width only
        // when the formatter itself has to break a single-line literal.
        let elem_indent = indent + open.len();
        if open == "#u8("
            && !has_comments
            && semantic
                .iter()
                .all(|n| matches!(n, Node::Atom(_) | Node::StringAtom(_)))
        {
            self.format_byte_rows(
                children,
                &semantic,
                elem_indent,
                open,
                close,
                originally_multiline,
            );
            return;
        }

        // Multi-line: try aligned binding pairs if all children are 2-element lists/vectors
        if self.align && !has_comments && semantic.len() >= 2 {
            let all_binding_pairs = semantic
                .iter()
                .all(|n| matches!(n, Node::List(_) | Node::Vector(_)));
            if all_binding_pairs {
                self.output.push_str(open);
                if self.try_format_aligned_group(&semantic, elem_indent, Self::split_binding) {
                    self.output.push_str(close);
                    return;
                }
                // Undo the open we just pushed — fall through to normal
                let open_len = open.len();
                self.output.truncate(self.output.len() - open_len);
            }
        }

        // Normal one per line, with comments preserved
        self.output.push_str(open);
        // Emit any comments before the first semantic element
        let had_leading_comments = self.emit_leading_comments(children, elem_indent);
        if had_leading_comments {
            self.output.push('\n');
            self.push_indent(elem_indent);
        }
        self.format_node(semantic[0], elem_indent);

        let rest_start = Self::index_after_nth_semantic(children, 1);
        self.emit_body_with_comments(children, rest_start, elem_indent);

        self.output.push_str(close);
    }

    /// Multi-line `#u8(...)` layout. Rows the user wrote are preserved with
    /// spacing normalized (single spaces between bytes) — a hand-arranged
    /// grid, e.g. 4×4, keeps its shape. A literal the formatter itself must
    /// break (single-line but over the width) wraps greedily at the width.
    fn format_byte_rows(
        &mut self,
        children: &[Node],
        semantic: &[&Node],
        indent: usize,
        open: &str,
        close: &str,
        originally_multiline: bool,
    ) {
        let mut rows: Vec<Vec<String>> = vec![Vec::new()];
        if originally_multiline {
            // Preserve the user's row breaks
            for child in children {
                match child {
                    Node::Newline => {
                        if !rows.last().is_some_and(|r| r.is_empty()) {
                            rows.push(Vec::new());
                        }
                    }
                    _ if is_trivia(child) => {}
                    n => rows.last_mut().unwrap().push(node_to_flat_string(n)),
                }
            }
        } else {
            // Greedy wrap: as many bytes per line as fit the width
            let mut col = indent;
            for n in semantic {
                let cell = node_to_flat_string(n);
                let w = display_width(&cell);
                if !rows.last().is_some_and(|r| r.is_empty()) && col + 1 + w > self.width {
                    rows.push(Vec::new());
                    col = indent;
                }
                let row = rows.last_mut().unwrap();
                if !row.is_empty() {
                    col += 1;
                }
                col += w;
                row.push(cell);
            }
        }
        rows.retain(|r| !r.is_empty());

        self.output.push_str(open);
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                self.output.push('\n');
                self.push_indent(indent);
            }
            self.output.push_str(&row.join(" "));
        }
        self.output.push_str(close);
    }

    // -----------------------------------------------------------------------
    // Map: {:a 1 :b 2} — key-value pairs, one per line if doesn't fit
    // -----------------------------------------------------------------------

    /// Map literal layout: one line when it fits, otherwise one key-value
    /// pair per line aligned under the opening brace.
    fn format_map(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);

        if semantic.is_empty() {
            return self.format_empty_with_comments(children, indent, open, close);
        }

        // Try one-line (only if no inner comments and not originally multi-line)
        if self.try_flat(children, indent, open, close, true) {
            return;
        }

        // If children contain comments, force multi-line to preserve them
        let has_comments = children.iter().any(has_any_comments);

        // Multi-line: align map values when possible, otherwise preserve comments.
        let pair_indent = indent + open.len();
        if self.align
            && !has_comments
            && self.try_format_aligned_map_pairs(&semantic, pair_indent, open, close)
        {
            return;
        }

        self.output.push_str(open);

        // Iterate through all children, tracking pair state
        // semantic_count: 0 = expecting key (start of pair), 1 = expecting value
        let mut semantic_count = 0;
        let mut first_pair = true;
        // Newline seen since the last key/value/comment — distinguishes a
        // trailing comment (stays on its pair's line) from a standalone one.
        let mut saw_newline = false;
        // A comment runs to end of line, so nothing may follow it inline.
        let mut after_comment = false;
        for child in children.iter() {
            match child {
                Node::Newline => saw_newline = true,
                Node::Comment(text) => {
                    if first_pair || saw_newline {
                        // Standalone comment: own line
                        self.output.push('\n');
                        self.push_indent(pair_indent);
                    } else {
                        // Trailing comment: keep it on the line it annotates
                        self.output.push(' ');
                    }
                    self.output.push_str(text);
                    first_pair = false; // ensure next key gets a newline
                    after_comment = true;
                    saw_newline = false;
                }
                _ if is_trivia(child) => {}
                _ => {
                    if semantic_count % 2 == 0 {
                        // Key position — start a new pair
                        if !first_pair {
                            self.output.push('\n');
                            self.push_indent(pair_indent);
                        }
                        self.format_node(child, pair_indent);
                        first_pair = false;
                    } else if after_comment {
                        // Value position, but a comment owns the rest of the
                        // key's line — the value gets its own indented line.
                        let value_indent = pair_indent + self.indent_size;
                        self.output.push('\n');
                        self.push_indent(value_indent);
                        self.format_node(child, value_indent);
                    } else {
                        // Value position — on same line as key
                        self.output.push(' ');
                        self.format_node(child, pair_indent);
                    }
                    semantic_count += 1;
                    after_comment = false;
                    saw_newline = false;
                }
            }
        }

        // The close delimiter must not land inside a trailing comment.
        if after_comment {
            self.output.push('\n');
            self.push_indent(pair_indent);
        }
        self.output.push_str(close);
    }

    // -----------------------------------------------------------------------
    // Helper: emit body children with interleaved comments preserved
    // -----------------------------------------------------------------------

    /// Emit any comments that appear before the first semantic element.
    /// Returns true if any comments were emitted.
    fn emit_leading_comments(&mut self, all_children: &[Node], indent: usize) -> bool {
        let mut emitted = false;
        for child in all_children {
            match child {
                Node::Comment(text) => {
                    self.output.push('\n');
                    self.push_indent(indent);
                    self.output.push_str(text);
                    emitted = true;
                }
                Node::Newline => {}
                _ if is_trivia(child) => {}
                _ => break, // Hit first semantic element
            }
        }
        emitted
    }

    /// Find the index in `all_children` just past the `n`th semantic (non-trivia) node.
    /// Returns `all_children.len()` if fewer than `n` semantic nodes exist.
    fn index_after_nth_semantic(all_children: &[Node], n: usize) -> usize {
        let mut count = 0;
        for (i, child) in all_children.iter().enumerate() {
            if !is_trivia(child) {
                count += 1;
                if count == n {
                    return i + 1;
                }
            }
        }
        all_children.len()
    }

    /// Emit all children starting from `start_idx`, preserving comments inline.
    /// Semantic nodes are formatted on their own lines at `body_indent`.
    /// A comment with no newline before it (a trailing comment) stays on the
    /// line of the form it annotates; other comments get their own line at
    /// `body_indent`. Blank lines (2+ consecutive Newlines) are preserved as a
    /// single blank line. If the body ends with a comment, a final newline +
    /// indent is emitted so the caller's closing delimiter isn't absorbed into
    /// the comment.
    fn emit_body_with_comments(
        &mut self,
        all_children: &[Node],
        start_idx: usize,
        body_indent: usize,
    ) {
        self.emit_body_with_comments_impl(all_children, start_idx, body_indent, false)
    }

    /// `emit_body_with_comments`, with `align_defines` additionally grouping
    /// and column-aligning consecutive one-liner defines under `--align` —
    /// used for `module` bodies (see `format_body`), which are otherwise
    /// just like top-level for grouping purposes: their defines are
    /// exported/visible the same way a true top-level define is.
    fn emit_body_with_comments_impl(
        &mut self,
        all_children: &[Node],
        start_idx: usize,
        body_indent: usize,
        align_defines: bool,
    ) {
        let mut consecutive_newlines: usize = 0;
        let mut ends_with_comment = false;
        let mut idx = start_idx;
        while idx < all_children.len() {
            let child = &all_children[idx];
            match child {
                Node::Newline => {
                    consecutive_newlines += 1;
                    idx += 1;
                }
                Node::Comment(text) => {
                    if consecutive_newlines == 0 && !self.output.ends_with('\n') {
                        // Trailing comment: keep it on its form's line.
                        self.output.push(' ');
                    } else {
                        self.output.push('\n');
                        self.emit_blank_lines(consecutive_newlines);
                        self.push_indent(body_indent);
                    }
                    self.output.push_str(text);
                    consecutive_newlines = 0;
                    ends_with_comment = true;
                    idx += 1;
                }
                _ if is_trivia(child) => {
                    idx += 1;
                }
                _ => {
                    if !self.output.ends_with('\n') {
                        self.output.push('\n');
                    }
                    self.emit_blank_lines(consecutive_newlines);
                    consecutive_newlines = 0;
                    ends_with_comment = false;

                    if align_defines {
                        if let Some(next_idx) =
                            self.try_format_define_run(all_children, idx, body_indent)
                        {
                            idx = next_idx;
                            continue;
                        }
                    }
                    self.push_indent(body_indent);
                    self.format_node(child, body_indent);
                    idx += 1;
                }
            }
        }
        // A comment runs to end of line — the caller's close delimiter must
        // not land inside it.
        if ends_with_comment {
            self.output.push('\n');
            self.push_indent(body_indent);
        }
    }

    // -----------------------------------------------------------------------
    // Decorative alignment
    // -----------------------------------------------------------------------

    /// Try to format a group of sibling forms with aligned columns.
    /// Each form is split at `split_fn` into left and right parts.
    /// Returns true if alignment was applied, false if it fell back.
    ///
    /// `split_fn(semantic_children) -> Option<(left_parts, right_parts)>`
    /// where both are rendered flat and padded to align.
    fn try_format_aligned_group<F>(&mut self, forms: &[&Node], indent: usize, split_fn: F) -> bool
    where
        F: Fn(&Node) -> Option<(String, String)>,
    {
        if forms.len() < 2 {
            return false;
        }

        // Compute left/right splits for each form
        let mut splits: Vec<(String, String)> = Vec::new();
        for form in forms {
            match split_fn(form) {
                Some(pair) => {
                    // A string literal can carry a raw newline that would break
                    // the aligned column — bail to normal formatting.
                    if pair.0.contains('\n') || pair.1.contains('\n') {
                        return false;
                    }
                    splits.push(pair)
                }
                None => return false,
            }
        }

        // Find the max left width to determine the alignment column
        let max_left = splits
            .iter()
            .map(|(l, _)| display_width(l))
            .max()
            .unwrap_or(0);

        // Check that all aligned lines fit within width
        let min_gap = ALIGN_GAP;
        for (_left, right) in &splits {
            if indent + max_left + min_gap + display_width(right) > self.width {
                return false;
            }
        }

        // Also verify that the alignment actually matters — if all lefts are the
        // same width, there's nothing to align (just normal spacing)
        let min_left = splits
            .iter()
            .map(|(l, _)| display_width(l))
            .min()
            .unwrap_or(0);
        if max_left == min_left {
            return false;
        }

        // Emit aligned lines
        for (idx, (left, right)) in splits.iter().enumerate() {
            if idx > 0 {
                self.output.push('\n');
                self.push_indent(indent);
            }
            self.output.push_str(left);
            // Pad to align
            let pad = max_left - display_width(left) + min_gap;
            for _ in 0..pad {
                self.output.push(' ');
            }
            self.output.push_str(right);
        }
        true
    }

    /// Render `node` at `indent` and return its text if it came out on a
    /// single line; `None` otherwise. The output buffer is left unchanged.
    fn render_if_single_line(&mut self, node: &Node, indent: usize) -> Option<String> {
        let checkpoint = self.output.len();
        self.format_node(node, indent);
        let rendered = self.output[checkpoint..].to_string();
        self.output.truncate(checkpoint);
        if rendered.contains('\n') {
            None
        } else {
            Some(rendered)
        }
    }

    /// Format map entries with their values aligned to the widest key.
    fn try_format_aligned_map_pairs(
        &mut self,
        semantic: &[&Node],
        indent: usize,
        open: &str,
        close: &str,
    ) -> bool {
        if semantic.len() < 4 || !semantic.len().is_multiple_of(2) {
            return false;
        }

        let mut pairs = Vec::with_capacity(semantic.len() / 2);
        for pair in semantic.as_chunks::<2>().0 {
            if has_any_comments(pair[0]) || has_any_comments(pair[1]) {
                return false;
            }
            // Judge eligibility by the node's REAL rendering, not its
            // original newline structure: a node with internal newlines can
            // still render on one line (a single-pair nested map does), and
            // if eligibility disagrees with what the fallback layout emits,
            // the second pass sees different structure and aligns what the
            // first pass wouldn't — not idempotent. A rendering with a
            // newline (multi-line value, raw newline in a string) bails.
            let Some(key) = self.render_if_single_line(pair[0], indent) else {
                return false;
            };
            let Some(value) = self.render_if_single_line(pair[1], indent) else {
                return false;
            };
            pairs.push((key, value));
        }

        let max_key = pairs
            .iter()
            .map(|(key, _)| display_width(key))
            .max()
            .unwrap_or(0);
        let min_key = pairs
            .iter()
            .map(|(key, _)| display_width(key))
            .min()
            .unwrap_or(0);
        let last = pairs.len() - 1;
        if max_key == min_key
            || pairs.iter().enumerate().any(|(index, (_, value))| {
                let closer = if index == last { close.len() } else { 0 };
                indent + max_key + ALIGN_GAP + display_width(value) + closer > self.width
            })
        {
            return false;
        }

        self.output.push_str(open);
        for (index, (key, value)) in pairs.iter().enumerate() {
            if index > 0 {
                self.output.push('\n');
                self.push_indent(indent);
            }
            self.output.push_str(key);
            self.output.extend(std::iter::repeat_n(
                ' ',
                max_key - display_width(key) + ALIGN_GAP,
            ));
            self.output.push_str(value);
        }
        self.output.push_str(close);
        true
    }

    /// If `--align` is on and `nodes[i]` starts a run of 2+ consecutive
    /// alignable one-liner defines (same rules as `format_top_level`: a
    /// single newline between members keeps the run going, a blank line or a
    /// non-define breaks it), emit the whole run column-aligned at `indent`
    /// and return the index to resume scanning from. Returns `None` when
    /// `nodes[i]` isn't alignable or doesn't form a 2+ run — the caller falls
    /// through to normal per-node formatting for `nodes[i]`.
    ///
    /// Shared by `format_top_level` (`indent` 0) and `format_body`'s
    /// `module`-body path (`indent` = the module body's indent) — a
    /// `(module ...)` body is otherwise just like top level for grouping
    /// purposes, since its defines are exported/visible the same way.
    fn try_format_define_run(&mut self, nodes: &[Node], i: usize, indent: usize) -> Option<usize> {
        if !(self.align && Self::is_alignable_define(&nodes[i])) {
            return None;
        }
        let len = nodes.len();
        let group_start = i;
        let mut group_end = i + 1;
        // Look ahead for more consecutive defines (skip newlines but not blank lines)
        while group_end < len {
            match &nodes[group_end] {
                Node::Newline => {
                    // Check if this is a blank line (2+ consecutive newlines)
                    let mut peek = group_end;
                    let mut nl_count = 0;
                    while peek < len && matches!(&nodes[peek], Node::Newline) {
                        nl_count += 1;
                        peek += 1;
                    }
                    if nl_count > 1 {
                        break; // blank line breaks the group
                    }
                    // Single newline — check if next semantic node is also an alignable define
                    if peek < len && Self::is_alignable_define(&nodes[peek]) {
                        group_end = peek + 1;
                    } else {
                        break;
                    }
                }
                // A comment here directly follows a define on the same line
                // (standalone comments sit after a Newline, which the arm
                // above intercepts): it's the define's trailing comment —
                // keep it in the group instead of orphaning it below.
                Node::Comment(_) => {
                    group_end += 1;
                }
                _ if Self::is_alignable_define(&nodes[group_end]) => {
                    group_end += 1;
                }
                _ => break,
            }
        }

        // Collect the define nodes in this group, each with its
        // trailing comment (if any).
        let mut group: Vec<&Node> = Vec::new();
        let mut trailing: Vec<Option<String>> = Vec::new();
        for node in &nodes[group_start..group_end] {
            match node {
                Node::Newline => {}
                Node::Comment(text) => {
                    if let Some(last) = trailing.last_mut() {
                        *last = Some(text.clone());
                    }
                }
                _ => {
                    group.push(node);
                    trailing.push(None);
                }
            }
        }

        if group.len() < 2 {
            // Single define — fall through to normal formatting.
            return None;
        }
        self.format_define_group(&group, &trailing, indent);
        // Deliberately no trailing '\n' here — same invariant format_node
        // leaves for a single form. `format_top_level` terminates the line
        // itself; `emit_body_with_comments_impl` needs it left open so a
        // group ending the body attaches directly to the closing delimiter.
        Some(group_end)
    }

    /// Emit a group of consecutive one-liner defines, column-aligning maximal
    /// sub-runs of members whose aligned line fits the width. A member too
    /// wide to participate is formatted normally and splits the run — which
    /// is exactly how a reformat of the output would group things, keeping
    /// `--align` idempotent (an all-or-nothing group is not: the too-wide
    /// member reflows to two lines and the SECOND pass aligns the survivors).
    /// `trailing[i]` is member `i`'s trailing comment, if any; within an
    /// aligned run comments share a column past the widest value. `indent` is
    /// each line's leading indent (0 at true top level, the body indent when
    /// called for a `module` body).
    fn format_define_group(&mut self, group: &[&Node], trailing: &[Option<String>], indent: usize) {
        let min_gap = ALIGN_GAP;

        // Split each define; None marks a member that can't be aligned
        // (unsplittable, embedded raw newline, or too wide at any column).
        let splits: Vec<Option<(String, String)>> = group
            .iter()
            .map(|form| {
                let children = match form {
                    Node::List(c) => c,
                    _ => return None,
                };
                let semantic = semantic_children(children);
                let (left, right) = Self::split_define(&semantic)?;
                if left.contains('\n')
                    || right.contains('\n')
                    || indent + display_width(&left) + min_gap + display_width(&right) > self.width
                {
                    return None;
                }
                Some((left, right))
            })
            .collect();

        // Mark maximal runs of consecutive alignable members. A run aligns
        // only when it has 2+ members, unequal left widths (otherwise there
        // is nothing to align), and its shared column keeps every line
        // within the width.
        let mut run_cols: Vec<Option<(usize, usize)>> = vec![None; group.len()];
        let mut run_start = 0;
        while run_start < group.len() {
            if splits[run_start].is_none() {
                run_start += 1;
                continue;
            }
            let mut run_end = run_start;
            while run_end < group.len() && splits[run_end].is_some() {
                run_end += 1;
            }
            let run: Vec<&(String, String)> = splits[run_start..run_end]
                .iter()
                .map(|s| s.as_ref().unwrap())
                .collect();
            let max_left = run.iter().map(|(l, _)| display_width(l)).max().unwrap();
            let min_left = run.iter().map(|(l, _)| display_width(l)).min().unwrap();
            let max_right = run.iter().map(|(_, r)| display_width(r)).max().unwrap();
            let fits = run
                .iter()
                .all(|(_, r)| indent + max_left + min_gap + display_width(r) <= self.width);
            if run.len() >= 2 && max_left > min_left && fits {
                for slot in &mut run_cols[run_start..run_end] {
                    *slot = Some((max_left, max_right));
                }
            }
            run_start = run_end;
        }

        for (idx, form) in group.iter().enumerate() {
            if idx > 0 {
                self.output.push('\n');
            }
            self.push_indent(indent);
            if let (Some((max_left, max_right)), Some((left, right))) =
                (&run_cols[idx], &splits[idx])
            {
                self.output.push_str(left);
                self.output.extend(std::iter::repeat_n(
                    ' ',
                    max_left - display_width(left) + min_gap,
                ));
                self.output.push_str(right);
                if let Some(comment) = trailing[idx].as_ref() {
                    self.output.extend(std::iter::repeat_n(
                        ' ',
                        max_right - display_width(right) + min_gap,
                    ));
                    self.output.push_str(comment);
                }
            } else {
                self.format_node(form, indent);
                if let Some(comment) = trailing[idx].as_ref() {
                    self.output.push(' ');
                    self.output.push_str(comment);
                }
            }
        }
    }

    /// Check if a top-level form is a simple one-liner define (define name value)
    /// or (define (name args...) single-body).
    fn is_alignable_define(node: &Node) -> bool {
        let children = match node {
            Node::List(c) => c,
            _ => return false,
        };
        // A comment inside the define would be deleted by the flat rendering
        // alignment uses.
        if has_any_comments(node) {
            return false;
        }
        let semantic = semantic_children(children);
        let head_name = match semantic.first() {
            Some(Node::Atom(Token::Symbol(s))) if is_define_head(s) => s.as_str(),
            _ => return false,
        };
        if semantic.len() != define_signature_len(head_name) + 1 {
            return false;
        }
        // Alignment renders the define on ONE line, so eligibility must mirror
        // what normal formatting would produce — otherwise a define that the
        // first pass joins becomes alignable only on the second pass (not
        // idempotent), or alignment collapses layout the user chose to keep:
        // - (define (f x) body): format_body keeps the body on its own line,
        //   so only an already-one-line define may be aligned.
        // - (define name value): format_body packs it onto one line whenever
        //   the value renders flat, so only newlines INSIDE the name/value
        //   (e.g. a multi-line map literal, which stays multi-line) block it.
        let is_fn_define = define_signature_len(head_name) == 3
            || matches!(
                semantic[1],
                Node::List(_) | Node::Vector(_) | Node::ShortLambda(_)
            );
        if is_fn_define {
            !has_any_newlines(node)
        } else {
            !semantic.iter().skip(1).any(|n| has_any_newlines(n))
        }
    }

    /// Split a define form into left="(define sig" and right="body)" for alignment.
    fn split_define(semantic: &[&Node]) -> Option<(String, String)> {
        let head_name = match semantic.first() {
            Some(Node::Atom(Token::Symbol(s))) if is_define_head(s) => s.as_str(),
            _ => return None,
        };
        let sig_len = define_signature_len(head_name);
        if semantic.len() != sig_len + 1 {
            return None;
        }
        // Neither the signature nor the body may contain newlines or comments
        if semantic
            .iter()
            .skip(1)
            .any(|n| has_any_newlines(n) || has_any_comments(n))
        {
            return None;
        }
        let left_parts: Vec<String> = semantic[..sig_len]
            .iter()
            .map(|n| node_to_flat_string(n))
            .collect();
        let left = format!("({}", left_parts.join(" "));
        let right = format!("{})", node_to_flat_string(semantic[sig_len]));
        Some((left, right))
    }

    /// Split a cond/case clause into left="(test" and right="result)" for alignment.
    fn split_clause(form: &Node) -> Option<(String, String)> {
        let (children, open, close) = Self::pair_delims(form)?;
        let semantic = semantic_children(children);
        if semantic.len() != 2 {
            return None;
        }
        if semantic
            .iter()
            .any(|n| has_any_newlines(n) || has_any_comments(n))
        {
            return None;
        }
        let left = format!("{open}{}", node_to_flat_string(semantic[0]));
        let right = format!("{}{close}", node_to_flat_string(semantic[1]));
        Some((left, right))
    }

    /// Split a let binding pair into left="(name" and right="value)" for alignment.
    /// A pair form's children and its OWN delimiters — a `[…]` pair must be
    /// re-emitted with brackets; rewriting it as `(…)` would turn a vector
    /// literal into a call form and change the program.
    fn pair_delims(form: &Node) -> Option<(&[Node], &'static str, &'static str)> {
        match form {
            Node::List(c) => Some((c.as_slice(), "(", ")")),
            Node::Vector(c) => Some((c.as_slice(), "[", "]")),
            _ => None,
        }
    }

    fn split_binding(form: &Node) -> Option<(String, String)> {
        let (children, open, close) = Self::pair_delims(form)?;
        let semantic = semantic_children(children);
        if semantic.len() != 2 {
            return None;
        }
        // Only align if the name is a simple atom (not a destructuring pattern)
        match semantic[0] {
            Node::Atom(_) | Node::StringAtom(_) => {}
            _ => return None,
        }
        if semantic
            .iter()
            .any(|n| has_any_newlines(n) || has_any_comments(n))
        {
            return None;
        }
        let left = format!("{open}{}", node_to_flat_string(semantic[0]));
        let right = format!("{}{close}", node_to_flat_string(semantic[1]));
        Some((left, right))
    }

    // -----------------------------------------------------------------------
    // Utilities
    // -----------------------------------------------------------------------

    fn push_indent(&mut self, n: usize) {
        self.output.extend(std::iter::repeat_n(' ', n));
    }
}

/// Column width of a rendered fragment: characters, not bytes, so non-ASCII
/// symbols and strings don't skew alignment padding. (An approximation —
/// combining marks and wide CJK glyphs still count as one column each.)
fn display_width(s: &str) -> usize {
    s.chars().count()
}

/// Render a single node as a flat (single-line) string.
fn node_to_flat_string(node: &Node) -> String {
    match node {
        Node::Atom(tok) => token_text(tok).into_owned(),
        Node::StringAtom(raw) => raw.clone(),
        Node::Comment(text) => text.clone(),
        Node::Newline => String::new(),
        Node::List(children) => flat_string(children, "(", ")"),
        Node::Vector(children) => flat_string(children, "[", "]"),
        Node::Map(children) => flat_string(children, "{", "}"),
        Node::ShortLambda(children) => flat_string(children, "#(", ")"),
        Node::ByteVector(children) => flat_string(children, "#u8(", ")"),
        Node::Prefix(tok, inner) => {
            format!("{}{}", prefix_text(tok), node_to_flat_string(inner))
        }
    }
}

/// Render children flat (single line) between delimiters, skipping trivia.
fn flat_string(children: &[Node], open: &str, close: &str) -> String {
    let mut out = String::new();
    out.push_str(open);
    let mut first = true;
    for child in children {
        if is_trivia(child) {
            continue;
        }
        if !first {
            out.push(' ');
        }
        out.push_str(&node_to_flat_string(child));
        first = false;
    }
    out.push_str(close);
    out
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Options controlling [`format_source`].
///
/// [`FormatOptions::default()`] is the canonical set of formatter defaults
/// (width 80, indent 2, align off) shared by the `sema fmt` CLI, the LSP
/// server, and the playground.
///
/// # Examples
///
/// ```
/// use sema_fmt::FormatOptions;
///
/// let narrow = FormatOptions { width: 40, ..Default::default() };
/// assert_eq!(narrow.indent, 2);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOptions {
    /// Target maximum line width in columns.
    pub width: usize,
    /// Spaces per indentation level for body forms.
    pub indent: usize,
    /// Column-align consecutive similar forms (defines, cond clauses,
    /// let bindings) for readability.
    pub align: bool,
    /// Maximum number of consecutive blank lines to preserve between forms;
    /// longer runs are collapsed to this many.
    pub max_blank_lines: usize,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            width: 80,
            indent: 2,
            align: false,
            max_blank_lines: 1,
        }
    }
}

/// Format Sema source code.
///
/// The formatter preserves all comments, handles shebang lines, and produces
/// idempotent output. Returns an error if the input fails to tokenize or has
/// unbalanced delimiters; the input is never evaluated.
///
/// # Examples
///
/// ```
/// use sema_fmt::{format_source, FormatOptions};
///
/// let out = format_source("(+   1  2)", &FormatOptions::default()).unwrap();
/// assert_eq!(out, "(+ 1 2)\n");
/// ```
pub fn format_source(input: &str, opts: &FormatOptions) -> Result<String, SemaError> {
    if input.is_empty() {
        return Ok(String::new());
    }

    // 1. Handle shebang: if input starts with "#!", extract the first line
    let (shebang, rest) = if input.starts_with("#!") {
        match input.find('\n') {
            Some(pos) => (Some(&input[..pos]), &input[pos + 1..]),
            None => (Some(input), ""),
        }
    } else {
        (None, input)
    };

    if rest.trim().is_empty() {
        let mut result = String::new();
        if let Some(shebang_line) = shebang {
            result.push_str(shebang_line);
            result.push('\n');
        }
        return Ok(result);
    }

    // 2. Format the body — through the fence splitter when `@formatter:off`
    // regions are present, directly otherwise (the common, zero-cost path).
    let body = if rest.contains(FENCE_OFF) {
        format_with_fences(rest, opts)?
    } else {
        format_segment(rest, opts)?
    };

    // 3. Assemble result
    let mut final_result = String::new();
    if let Some(shebang_line) = shebang {
        final_result.push_str(shebang_line);
        final_result.push('\n');
    }
    final_result.push_str(&body);

    // 4. Ensure exactly one trailing newline
    while final_result.ends_with('\n') {
        final_result.pop();
    }
    if !final_result.is_empty() {
        final_result.push('\n');
    }

    Ok(final_result)
}

/// Format one fence-free stretch of source: tokenize, build the node tree,
/// format, and strip trailing blanks.
fn format_segment(src: &str, opts: &FormatOptions) -> Result<String, SemaError> {
    if src.trim().is_empty() {
        return Ok(String::new());
    }
    let tokens = tokenize(src)?;
    let nodes = build_nodes(&tokens, src)?;
    let mut fmt = Formatter::new(opts);
    fmt.format_top_level(&nodes);

    // Remove trailing whitespace on each line.
    //
    // We must NOT use `str::lines()`/`trim_end()` here: those treat `\r` as a
    // line separator (and `trim_end` strips a trailing `\r`), which would
    // silently mangle a CR that lives inside a preserved string/f-string/regex
    // literal — e.g. `"foo\r\nbar"` would lose its `\r`, changing the program's
    // string contents. Instead, strip only spaces/tabs that directly precede a
    // real `\n` (or the end of input), leaving `\r` untouched in every context.
    Ok(strip_trailing_blanks(&fmt.output))
}

/// Comment text (after `;`s and whitespace) that disables/re-enables the
/// formatter for the region between the two markers.
const FENCE_OFF: &str = "@formatter:off";
const FENCE_ON: &str = "@formatter:on";

/// Split the source at top-level `; @formatter:off` / `; @formatter:on`
/// comments: the fenced region — from the start of the OFF-comment's line
/// through the end of the ON-comment's line (or EOF when unmatched) — is
/// emitted byte-for-byte; everything else formats normally. Fences nested
/// inside a form are ignored (they are ordinary comments there).
fn format_with_fences(src: &str, opts: &FormatOptions) -> Result<String, SemaError> {
    let tokens = tokenize(src)?;

    // Locate depth-0 fence comments
    let mut fences: Vec<(bool, usize, usize)> = Vec::new(); // (is_off, start, end)
    let mut depth = 0i64;
    for st in &tokens {
        match &st.token {
            Token::LParen
            | Token::LBracket
            | Token::LBrace
            | Token::ShortLambdaStart
            | Token::BytevectorStart => depth += 1,
            Token::RParen | Token::RBracket | Token::RBrace => depth -= 1,
            Token::Comment(text) if depth == 0 => {
                let t = text.trim_start_matches(';').trim();
                if t == FENCE_OFF {
                    fences.push((true, st.byte_start, st.byte_end));
                } else if t == FENCE_ON {
                    fences.push((false, st.byte_start, st.byte_end));
                }
            }
            _ => {}
        }
    }

    // Append `part`, separating it from previous content with a newline and
    // (when the source had a blank line at the seam) one blank line.
    fn push_part(out: &mut String, part: &str, blank_before: bool) {
        if part.is_empty() {
            return;
        }
        if !out.is_empty() {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            if blank_before {
                out.push('\n');
            }
        }
        out.push_str(part);
    }
    // Whether a source stretch ends at a blank line (ignoring spaces/tabs/CR).
    fn ends_with_blank_line(s: &str) -> bool {
        let significant: Vec<char> = s
            .chars()
            .rev()
            .filter(|c| !matches!(c, ' ' | '\t' | '\r'))
            .take(2)
            .collect();
        significant == ['\n', '\n']
    }
    // Whether a source stretch starts with a blank line.
    fn starts_with_blank_line(s: &str) -> bool {
        s.trim_start_matches([' ', '\t', '\r']).starts_with('\n')
    }

    let mut out = String::new();
    let mut cursor = 0usize;
    let mut i = 0;
    while i < fences.len() {
        let (is_off, fence_start, _) = fences[i];
        if !is_off {
            // A stray `@formatter:on` with no active OFF region is an
            // ordinary comment — leave it to normal formatting.
            i += 1;
            continue;
        }
        // Formatted stretch before the OFF-comment's line
        let line_start = src[..fence_start].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let before = &src[cursor..line_start];
        push_part(
            &mut out,
            &format_segment(before, opts)?,
            starts_with_blank_line(before),
        );
        // Verbatim through the matching ON-comment's line (or EOF)
        let mut j = i + 1;
        while j < fences.len() && fences[j].0 {
            j += 1;
        }
        let verbatim_end = if j < fences.len() {
            let on_end = fences[j].2;
            src[on_end..]
                .find('\n')
                .map(|p| on_end + p + 1)
                .unwrap_or(src.len())
        } else {
            src.len()
        };
        let verbatim = src[line_start..verbatim_end].trim_end_matches('\n');
        push_part(&mut out, verbatim, ends_with_blank_line(before));
        cursor = verbatim_end;
        i = j + 1;
    }
    let after = &src[cursor..];
    push_part(
        &mut out,
        &format_segment(after, opts)?,
        starts_with_blank_line(after),
    );
    Ok(out)
}

/// Strip trailing spaces/tabs that immediately precede a `\n` (or the end of
/// input), without treating `\r` as a line separator.
///
/// Unlike `str::lines()` + `trim_end()`, this preserves any `\r` byte — in
/// particular a `\r\n` (or bare `\r`) embedded inside a preserved string /
/// f-string / regex literal — so the formatter never alters a program's string
/// contents while cleaning up emitted layout whitespace.
fn strip_trailing_blanks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    // Index in `out` of the start of the current run of trailing spaces/tabs.
    // Bytes from this index onward are stripped when we hit a `\n` or EOF.
    let mut trailing_start = out.len();
    for c in s.chars() {
        match c {
            ' ' | '\t' => out.push(c),
            '\n' => {
                out.truncate(trailing_start);
                out.push('\n');
                trailing_start = out.len();
            }
            _ => {
                // Any other char (including `\r`) is significant: it ends the
                // current trailing-whitespace run.
                out.push(c);
                trailing_start = out.len();
            }
        }
    }
    // Strip trailing spaces/tabs at end of input (no final newline).
    out.truncate(trailing_start);
    out
}
