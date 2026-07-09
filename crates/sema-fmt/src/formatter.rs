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

/// Build the [`Node`] tree for a whole token stream (one node per top-level
/// form, comment, or newline). `source` is needed to recover the original
/// text of string/number literals via token byte spans.
fn build_nodes(tokens: &[SpannedToken], source: &str) -> Result<Vec<Node>, SemaError> {
    let mut pos = 0;
    let mut nodes = Vec::new();
    while pos < tokens.len() {
        let (node, next) = build_one(tokens, pos, source)?;
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
) -> Result<(Node, usize), SemaError> {
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

        // Prefix tokens — attach to the following node
        Token::Quote | Token::Quasiquote | Token::Unquote | Token::UnquoteSplice | Token::Deref => {
            let prefix_tok = st.token.clone();
            if pos + 1 >= tokens.len() {
                return Err(SemaError::eval("prefix token at end of input"));
            }
            let (inner, next) = build_one(tokens, pos + 1, source)?;
            Ok((Node::Prefix(prefix_tok, Box::new(inner)), next))
        }

        // Grouped forms
        Token::LParen => build_group(tokens, pos + 1, Token::RParen, source, |children| {
            Node::List(children)
        }),
        Token::LBracket => build_group(tokens, pos + 1, Token::RBracket, source, |children| {
            Node::Vector(children)
        }),
        Token::LBrace => build_group(tokens, pos + 1, Token::RBrace, source, |children| {
            Node::Map(children)
        }),
        Token::ShortLambdaStart => {
            build_group(tokens, pos + 1, Token::RParen, source, |children| {
                Node::ShortLambda(children)
            })
        }
        Token::BytevectorStart => build_group(tokens, pos + 1, Token::RParen, source, |children| {
            Node::ByteVector(children)
        }),

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
        let (node, next) = build_one(tokens, pos, source)?;
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

    match head {
        Some(
            "define"
            | "defn"
            | "defun"
            | "defmacro"
            | "fn"
            | "lambda"
            | "do"
            | "begin"
            | "when"
            | "unless"
            | "module"
            | "defagent"
            | "deftool"
            | "prompt"
            | "message"
            | "export"
            | "for"
            | "for-each"
            | "while"
            | "with-open-file"
            | "with-exception-handler"
            | "define-record-type"
            | "define-syntax"
            | "syntax-rules",
        ) => FormKind::Body,
        Some("let" | "let*" | "letrec" | "let-values" | "let*-values" | "when-let" | "if-let") => {
            FormKind::Binding
        }
        Some("cond" | "case" | "match") => FormKind::Clause,
        Some("->" | "->>" | "as->" | "some->") => FormKind::Threading,
        Some("try") => FormKind::TryCatch,
        Some("if") => FormKind::Cond,
        Some("import" | "load" | "require") => FormKind::Import,
        _ => FormKind::Call,
    }
}

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
        "define" => {
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
        // fn/lambda: head + params
        "fn" | "lambda" if semantic.len() > 1 => 2,
        "fn" | "lambda" => 1,
        // when/unless/while: head + condition on first line
        "when" | "unless" | "while" if semantic.len() > 1 => 2,
        "when" | "unless" | "while" => 1,
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
    /// The accumulated formatted source.
    output: String,
}

impl Formatter {
    fn new(width: usize, indent_size: usize, align: bool) -> Self {
        Self {
            width,
            indent_size,
            align,
            output: String::new(),
        }
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
                        if pending_blank_lines > 1 {
                            // Collapse multiple blank lines to 1
                            self.output.push('\n');
                        }
                        // Always start the comment on a new line
                        if !self.output.ends_with('\n') {
                            self.output.push('\n');
                        }
                    }
                    pending_blank_lines = 0;
                    self.output.push_str(text);
                    self.output.push('\n');
                    first_content = false;
                    i += 1;
                }
                _ => {
                    if !first_content {
                        if pending_blank_lines > 1 {
                            // There was at least one blank line between forms
                            self.output.push('\n');
                        }
                        if !self.output.ends_with('\n') {
                            self.output.push('\n');
                        }
                    }
                    pending_blank_lines = 0;

                    // Try to collect a group of consecutive alignable defines
                    if self.align && Self::is_alignable_define(&nodes[i]) {
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
                                _ if Self::is_alignable_define(&nodes[group_end]) => {
                                    group_end += 1;
                                }
                                _ => break,
                            }
                        }

                        // Collect the define nodes in this group
                        let group = semantic_children(&nodes[group_start..group_end]);

                        if group.len() >= 2
                            && self.try_format_aligned_group(&group, 0, Self::split_define)
                        {
                            if !self.output.ends_with('\n') {
                                self.output.push('\n');
                            }
                            i = group_end;
                            first_content = false;
                            continue;
                        }
                        // Alignment failed or group too small — format each
                        // define in the group individually to avoid re-scanning.
                        // Only use the batch path when we actually had a group
                        // of 2+ (otherwise fall through to normal formatting).
                        if group.len() >= 2 {
                            for node in &nodes[group_start..group_end] {
                                match node {
                                    Node::Newline => {
                                        pending_blank_lines += 1;
                                    }
                                    Node::Comment(text) => {
                                        if pending_blank_lines > 1 {
                                            self.output.push('\n');
                                        }
                                        if !self.output.ends_with('\n') {
                                            self.output.push('\n');
                                        }
                                        self.output.push_str(text);
                                        self.output.push('\n');
                                        pending_blank_lines = 0;
                                        first_content = false;
                                    }
                                    _ => {
                                        if !first_content {
                                            if pending_blank_lines > 1 {
                                                self.output.push('\n');
                                            }
                                            if !self.output.ends_with('\n') {
                                                self.output.push('\n');
                                            }
                                        }
                                        pending_blank_lines = 0;
                                        self.format_node(node, 0);
                                        first_content = false;
                                    }
                                }
                            }
                            i = group_end;
                            continue;
                        }
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
                self.format_node(inner, indent);
            }
        }
    }

    /// Format a list form with Lisp-aware indentation.
    fn format_list(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);

        // Empty form
        if semantic.is_empty() {
            self.output.push_str(open);
            self.output.push_str(close);
            return;
        }

        let kind = classify_form(children);
        let has_comments = children.iter().any(has_any_comments);
        let originally_multiline = children.iter().any(has_any_newlines);

        // Try one-line format:
        // - No inner comments
        // - Not originally multi-line anywhere in tree (respect layout intent)
        // - No structural reason to force multi-line (e.g. 2+ body exprs)
        if !has_comments && !originally_multiline && !should_force_multiline(kind, &semantic) {
            let one_line = flat_string(children, open, close);
            if indent + one_line.len() <= self.width {
                self.output.push_str(&one_line);
                return;
            }
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
            self.output.push_str(open);
            self.output.push_str(close);
            return;
        }

        let head_name = match &semantic[0].1 {
            Node::Atom(Token::Symbol(s)) => s.as_str(),
            _ => "",
        };
        let semantic_refs: Vec<&Node> = semantic.iter().map(|(_, n)| *n).collect();
        let first_line_count = body_first_line_count(head_name, &semantic_refs);

        let first_count = first_line_count.min(semantic.len());

        self.output.push_str(open);

        // Always emit head
        self.format_node(semantic[0].1, indent + open.len());
        let mut emitted = 1;

        // Try to put subsequent first-line args on the same line
        let body_indent = indent + self.indent_size;
        for (j, (_orig_idx, node)) in semantic.iter().enumerate().skip(1).take(first_count - 1) {
            let w = flat_width(node);
            let current_col = match self.output.rfind('\n') {
                Some(pos) => self.output.len() - pos - 1,
                None => self.output.len(),
            };

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

        // Remaining args as body at indent+2
        let body_start = Self::index_after_nth_semantic(children, emitted);
        self.emit_body_with_comments(children, body_start, body_indent);

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
            self.output.push_str(open);
            self.output.push_str(close);
            return;
        }

        self.output.push_str(open);
        // head
        self.format_node(semantic[0], indent + open.len());

        let clause_indent = indent + self.indent_size;
        let clause_start = Self::index_after_nth_semantic(children, 1);

        // Try aligned clause formatting: collect consecutive clause forms
        // (skipping comments/newlines) and try to align their test/result columns
        let clauses = semantic_children(&children[clause_start..]);

        let has_comments = children[clause_start..]
            .iter()
            .any(|n| matches!(n, Node::Comment(_)));

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
        // All clauses must be flat-renderable 2-element lists
        let mut splits: Vec<(String, String)> = Vec::new();
        for clause in clauses {
            let children = match clause {
                Node::List(c) => c,
                _ => return false,
            };
            let semantic = semantic_children(children);
            match Self::split_clause(&semantic) {
                Some(pair) => splits.push(pair),
                None => return false,
            }
        }

        let max_left = splits.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
        let min_left = splits.iter().map(|(l, _)| l.len()).min().unwrap_or(0);

        // If all lefts are the same width, use normal spacing (no alignment needed)
        let min_gap = if max_left == min_left { 1 } else { 2 };

        // Check all lines fit
        for (_left, right) in &splits {
            let line_width = indent + max_left + min_gap + right.len();
            if line_width > self.width {
                return false;
            }
        }

        // Emit aligned clauses
        for (left, right) in &splits {
            self.output.push('\n');
            self.push_indent(indent);
            self.output.push_str(left);
            let pad = max_left - left.len() + min_gap;
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
            self.output.push_str(open);
            self.output.push_str(close);
            return;
        }

        // If children contain comments, force multi-line to preserve them
        let has_comments = children.iter().any(has_any_comments);
        let originally_multiline = children.iter().any(has_any_newlines);

        // Try one-line first (only if no inner comments and not originally multi-line)
        if !has_comments && !originally_multiline {
            let one_line = flat_string(children, open, close);
            if indent + one_line.len() <= self.width {
                self.output.push_str(&one_line);
                return;
            }
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
            self.output.push_str(open);
            self.output.push_str(close);
            return;
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
            self.output.push_str(close);
            return;
        }

        // Try: head + first arg on same line
        let head_width = flat_width(semantic[0]);
        let first_arg_col = indent + open.len() + head_width + 1;
        let arg_indent = indent + self.indent_size;

        // Check if head + first arg fits on one line (flat)
        if first_arg_col + flat_width(semantic[1]) <= self.width {
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
        let kv_args: Vec<&Node> = semantic[kv_start..].to_vec();

        // Try one-line first
        let has_comments = children.iter().any(has_any_comments);
        let originally_multiline = children.iter().any(has_any_newlines);
        if !has_comments && !originally_multiline {
            let one_line = flat_string(children, open, close);
            if indent + one_line.len() <= self.width {
                self.output.push_str(&one_line);
                return;
            }
        }

        let pair_indent = indent + self.indent_size;

        self.output.push_str(open);
        // head
        self.format_node(semantic[0], indent + open.len());

        // For assoc, emit the map arg first
        if head_name == "assoc" && semantic.len() > 1 {
            self.output.push(' ');
            self.format_node(semantic[1], pair_indent);
        }

        // Emit key-value pairs, each pair on its own line at indent+2
        let mut i = 0;
        while i < kv_args.len() {
            self.output.push('\n');
            self.push_indent(pair_indent);
            // Key
            self.format_node(kv_args[i], pair_indent);

            if i + 1 < kv_args.len() {
                // Try key + value on one line
                let key_col = match self.output.rfind('\n') {
                    Some(pos) => self.output.len() - pos - 1,
                    None => self.output.len(),
                };
                let val_width = flat_width(kv_args[i + 1]);

                if key_col + 1 + val_width <= self.width {
                    let checkpoint = self.output.len();
                    self.output.push(' ');
                    self.format_node(kv_args[i + 1], pair_indent);
                    // If value went multi-line, undo and put on next line
                    if self.output[checkpoint..].contains('\n') {
                        self.output.truncate(checkpoint);
                        self.output.push('\n');
                        self.push_indent(pair_indent + self.indent_size);
                        self.format_node(kv_args[i + 1], pair_indent + self.indent_size);
                    }
                } else {
                    // Value on next line indented further
                    self.output.push('\n');
                    self.push_indent(pair_indent + self.indent_size);
                    self.format_node(kv_args[i + 1], pair_indent + self.indent_size);
                }
                i += 2;
            } else {
                // Odd arg (shouldn't happen normally, but handle gracefully)
                i += 1;
            }
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
            self.output.push_str(open);
            self.output.push_str(close);
            return;
        }

        // If children contain comments, force multi-line to preserve them
        let has_comments = children.iter().any(has_any_comments);
        let originally_multiline = children.iter().any(has_any_newlines);

        // Try one-line (only if no inner comments and not originally multi-line)
        if !has_comments && !originally_multiline {
            let one_line = flat_string(children, open, close);
            if indent + one_line.len() <= self.width {
                self.output.push_str(&one_line);
                return;
            }
        }

        // Multi-line: try aligned binding pairs if all children are 2-element lists/vectors
        let elem_indent = indent + open.len();
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

    // -----------------------------------------------------------------------
    // Map: {:a 1 :b 2} — key-value pairs, one per line if doesn't fit
    // -----------------------------------------------------------------------

    /// Map literal layout: one line when it fits, otherwise one key-value
    /// pair per line aligned under the opening brace.
    fn format_map(&mut self, children: &[Node], indent: usize, open: &str, close: &str) {
        let semantic = semantic_children(children);

        if semantic.is_empty() {
            self.output.push_str(open);
            self.output.push_str(close);
            return;
        }

        // If children contain comments, force multi-line to preserve them
        let has_comments = children.iter().any(has_any_comments);
        let originally_multiline = children.iter().any(has_any_newlines);

        // Try one-line (only if no inner comments and not originally multi-line)
        if !has_comments && !originally_multiline {
            let one_line = flat_string(children, open, close);
            if indent + one_line.len() <= self.width {
                self.output.push_str(&one_line);
                return;
            }
        }

        // Multi-line: each key-value pair on its own line, with comments preserved
        let pair_indent = indent + open.len();
        self.output.push_str(open);

        // Iterate through all children, tracking pair state
        // semantic_count: 0 = expecting key (start of pair), 1 = expecting value
        let mut semantic_count = 0;
        let mut first_pair = true;
        for child in children.iter() {
            match child {
                Node::Newline => {}
                Node::Comment(text) => {
                    self.output.push('\n');
                    self.push_indent(pair_indent);
                    self.output.push_str(text);
                    first_pair = false; // ensure next key gets a newline
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
                    } else {
                        // Value position — on same line as key
                        self.output.push(' ');
                        self.format_node(child, pair_indent);
                    }
                    semantic_count += 1;
                }
            }
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
    /// Comments are emitted at `body_indent`. Blank lines (2+ consecutive
    /// Newlines) are preserved as a single blank line.
    fn emit_body_with_comments(
        &mut self,
        all_children: &[Node],
        start_idx: usize,
        body_indent: usize,
    ) {
        let mut consecutive_newlines: usize = 0;
        for child in &all_children[start_idx..] {
            match child {
                Node::Newline => {
                    consecutive_newlines += 1;
                }
                Node::Comment(text) => {
                    // Preserve blank line if there were 2+ consecutive newlines
                    if consecutive_newlines >= 2 {
                        self.output.push('\n');
                    }
                    self.output.push('\n');
                    self.push_indent(body_indent);
                    self.output.push_str(text);
                    consecutive_newlines = 0;
                }
                _ if is_trivia(child) => {}
                _ => {
                    // Preserve blank line if there were 2+ consecutive newlines
                    if consecutive_newlines >= 2 {
                        self.output.push('\n');
                    }
                    self.output.push('\n');
                    self.push_indent(body_indent);
                    self.format_node(child, body_indent);
                    consecutive_newlines = 0;
                }
            }
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
        F: Fn(&[&Node]) -> Option<(String, String)>,
    {
        if forms.len() < 2 {
            return false;
        }

        // Compute left/right splits for each form
        let mut splits: Vec<(String, String)> = Vec::new();
        for form in forms {
            let children = match form {
                Node::List(c) | Node::Vector(c) | Node::ShortLambda(c) => c,
                _ => return false,
            };
            let semantic = semantic_children(children);
            match split_fn(&semantic) {
                Some(pair) => splits.push(pair),
                None => return false,
            }
        }

        // Find the max left width to determine the alignment column
        let max_left = splits.iter().map(|(l, _)| l.len()).max().unwrap_or(0);

        // Check that all aligned lines fit within width
        let min_gap = 2;
        for (_left, right) in &splits {
            if indent + max_left + min_gap + right.len() > self.width {
                return false;
            }
        }

        // Also verify that the alignment actually matters — if all lefts are the
        // same width, there's nothing to align (just normal spacing)
        let min_left = splits.iter().map(|(l, _)| l.len()).min().unwrap_or(0);
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
            let pad = max_left - left.len() + min_gap;
            for _ in 0..pad {
                self.output.push(' ');
            }
            self.output.push_str(right);
        }
        true
    }

    /// Check if a top-level form is a simple one-liner define (define name value)
    /// or (define (name args...) single-body).
    fn is_alignable_define(node: &Node) -> bool {
        let children = match node {
            Node::List(c) => c,
            _ => return false,
        };
        let semantic = semantic_children(children);
        if semantic.len() != 3 {
            return false;
        }
        match semantic[0] {
            Node::Atom(Token::Symbol(s)) => {
                matches!(s.as_str(), "define" | "defn" | "defun" | "defmacro")
            }
            _ => false,
        }
    }

    /// Split a define form into left="(define sig" and right="body)" for alignment.
    fn split_define(semantic: &[&Node]) -> Option<(String, String)> {
        if semantic.len() != 3 {
            return None;
        }
        match semantic[0] {
            Node::Atom(Token::Symbol(s))
                if matches!(s.as_str(), "define" | "defn" | "defun" | "defmacro") => {}
            _ => return None,
        }
        // Check that neither the signature nor body contain newlines
        if has_any_newlines(semantic[1]) || has_any_newlines(semantic[2]) {
            return None;
        }
        if has_any_comments(semantic[1]) || has_any_comments(semantic[2]) {
            return None;
        }
        let head = node_to_flat_string(semantic[0]);
        let sig = node_to_flat_string(semantic[1]);
        let body = node_to_flat_string(semantic[2]);
        let left = format!("({head} {sig}");
        let right = format!("{body})");
        Some((left, right))
    }

    /// Split a cond/case clause into left="(test" and right="result)" for alignment.
    fn split_clause(semantic: &[&Node]) -> Option<(String, String)> {
        if semantic.len() != 2 {
            return None;
        }
        if has_any_newlines(semantic[0]) || has_any_newlines(semantic[1]) {
            return None;
        }
        if has_any_comments(semantic[0]) || has_any_comments(semantic[1]) {
            return None;
        }
        let test = node_to_flat_string(semantic[0]);
        let result = node_to_flat_string(semantic[1]);
        let left = format!("({test}");
        let right = format!("{result})");
        Some((left, right))
    }

    /// Split a let binding pair into left="(name" and right="value)" for alignment.
    fn split_binding(semantic: &[&Node]) -> Option<(String, String)> {
        if semantic.len() != 2 {
            return None;
        }
        if has_any_newlines(semantic[0]) || has_any_newlines(semantic[1]) {
            return None;
        }
        if has_any_comments(semantic[0]) || has_any_comments(semantic[1]) {
            return None;
        }
        let name = node_to_flat_string(semantic[0]);
        let value = node_to_flat_string(semantic[1]);
        // Only align if the name is a simple atom (not a destructuring pattern)
        match semantic[0] {
            Node::Atom(_) | Node::StringAtom(_) => {}
            _ => return None,
        }
        let left = format!("({name}");
        let right = format!("{value})");
        Some((left, right))
    }

    // -----------------------------------------------------------------------
    // Utilities
    // -----------------------------------------------------------------------

    fn push_indent(&mut self, n: usize) {
        self.output.extend(std::iter::repeat_n(' ', n));
    }
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
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            width: 80,
            indent: 2,
            align: false,
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

    // 2. Tokenize the remaining source
    if rest.trim().is_empty() {
        let mut result = String::new();
        if let Some(shebang_line) = shebang {
            result.push_str(shebang_line);
            result.push('\n');
        }
        return Ok(result);
    }

    let tokens = tokenize(rest)?;

    // 3. Build node tree from tokens (passing source for string round-tripping)
    let nodes = build_nodes(&tokens, rest)?;

    // 4. Format node tree to string
    let mut fmt = Formatter::new(opts.width, opts.indent, opts.align);
    fmt.format_top_level(&nodes);

    // 5. Assemble result
    let mut result = String::new();
    if let Some(shebang_line) = shebang {
        result.push_str(shebang_line);
        result.push('\n');
    }
    result.push_str(&fmt.output);

    // 6. Remove trailing whitespace on each line.
    //
    // We must NOT use `str::lines()`/`trim_end()` here: those treat `\r` as a
    // line separator (and `trim_end` strips a trailing `\r`), which would
    // silently mangle a CR that lives inside a preserved string/f-string/regex
    // literal — e.g. `"foo\r\nbar"` would lose its `\r`, changing the program's
    // string contents. Instead, strip only spaces/tabs that directly precede a
    // real `\n` (or the end of input), leaving `\r` untouched in every context.
    let mut final_result = strip_trailing_blanks(&result);

    // 7. Ensure exactly one trailing newline
    while final_result.ends_with('\n') {
        final_result.pop();
    }
    if !final_result.is_empty() {
        final_result.push('\n');
    }

    Ok(final_result)
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
