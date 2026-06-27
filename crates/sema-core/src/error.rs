use std::collections::HashMap;
use std::fmt;

use crate::value::Value;

/// Check arity of a native function's arguments, returning `SemaError::Arity` on mismatch.
///
/// # Forms
///
/// ```ignore
/// check_arity!(args, "fn-name", 2);        // exactly 2
/// check_arity!(args, "fn-name", 1..=3);    // 1 to 3 inclusive
/// check_arity!(args, "fn-name", 2..);      // 2 or more
/// ```
#[macro_export]
macro_rules! check_arity {
    ($args:expr, $name:expr, $exact:literal) => {
        if $args.len() != $exact {
            return Err($crate::SemaError::arity(
                $name,
                stringify!($exact),
                $args.len(),
            ));
        }
    };
    ($args:expr, $name:expr, $lo:literal ..= $hi:literal) => {
        if $args.len() < $lo || $args.len() > $hi {
            return Err($crate::SemaError::arity(
                $name,
                concat!(stringify!($lo), "-", stringify!($hi)),
                $args.len(),
            ));
        }
    };
    ($args:expr, $name:expr, $lo:literal ..) => {
        if $args.len() < $lo {
            return Err($crate::SemaError::arity(
                $name,
                concat!(stringify!($lo), "+"),
                $args.len(),
            ));
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

impl Span {
    /// Create a point span (start == end).
    pub fn point(line: usize, col: usize) -> Self {
        Span {
            line,
            col,
            end_line: line,
            end_col: col,
        }
    }

    /// Create a span with explicit start and end.
    pub fn new(line: usize, col: usize, end_line: usize, end_col: usize) -> Self {
        Span {
            line,
            col,
            end_line,
            end_col,
        }
    }

    /// Create a span from the start of `self` to the end of `other`.
    pub fn to(self, other: &Span) -> Span {
        Span {
            line: self.line,
            col: self.col,
            end_line: other.end_line,
            end_col: other.end_col,
        }
    }

    /// Create a span from the start of `self` to an explicit end position.
    pub fn with_end(self, end_line: usize, end_col: usize) -> Span {
        Span {
            line: self.line,
            col: self.col,
            end_line,
            end_col,
        }
    }

    /// Check if `self` fully contains `other` (inclusive bounds).
    pub fn contains(&self, other: &Span) -> bool {
        let inner_start = (other.line, other.col);
        let inner_end = (other.end_line, other.end_col);
        let outer_start = (self.line, self.col);
        let outer_end = (self.end_line, self.end_col);
        inner_start >= outer_start && inner_end <= outer_end
    }

    /// Check if position `(line, col)` falls within this span (inclusive).
    pub fn contains_pos(&self, line: usize, col: usize) -> bool {
        let pos = (line, col);
        let start = (self.line, self.col);
        let end = (self.end_line, self.end_col);
        pos >= start && pos <= end
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// A single frame in a call stack trace.
#[derive(Debug, Clone)]
pub struct CallFrame {
    pub name: String,
    pub file: Option<std::path::PathBuf>,
    pub span: Option<Span>,
}

/// A captured stack trace (list of call frames, innermost first).
#[derive(Debug, Clone)]
pub struct StackTrace(pub Vec<CallFrame>);

impl fmt::Display for StackTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for frame in &self.0 {
            write!(f, "  at {}", frame.name)?;
            match (&frame.file, &frame.span) {
                (Some(file), Some(span)) => writeln!(f, " ({}:{span})", file.display())?,
                (Some(file), None) => writeln!(f, " ({})", file.display())?,
                (None, Some(span)) => writeln!(f, " (<input>:{span})")?,
                (None, None) => writeln!(f)?,
            }
        }
        Ok(())
    }
}

/// Maps Rc pointer addresses to source spans for expression tracking.
pub type SpanMap = HashMap<usize, Span>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum SemaError {
    #[error("Reader error at {span}: {message}")]
    Reader { message: String, span: Span },

    #[error("Eval error: {0}")]
    Eval(String),

    #[error("Type error: expected {expected}, got {got}{}", got_value.as_ref().map(|v| format!(" ({v})")).unwrap_or_default())]
    Type {
        expected: String,
        got: String,
        got_value: Option<String>,
    },

    #[error("Arity error: {name} expects {expected} args, got {got}")]
    Arity {
        name: String,
        expected: String,
        got: usize,
    },

    #[error("Unbound variable: {0}")]
    Unbound(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Permission denied: {function} requires '{capability}' capability")]
    PermissionDenied {
        function: String,
        capability: String,
    },

    #[error("Permission denied: {function} — path '{path}' is outside allowed directories")]
    PathDenied { function: String, path: String },

    #[error("User exception: {0}")]
    UserException(Value),

    #[error("{inner}")]
    WithTrace {
        inner: Box<SemaError>,
        trace: StackTrace,
    },

    #[error("{inner}")]
    WithContext {
        inner: Box<SemaError>,
        hint: Option<String>,
        note: Option<String>,
    },
}

/// Compute the Levenshtein edit distance between two strings.
fn edit_distance(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();
    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0; b_len + 1];

    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_len]
}

/// Find the most similar name from a list of candidates.
/// Returns `None` if no candidate is close enough.
pub fn suggest_similar(name: &str, candidates: &[&str]) -> Option<String> {
    // Max distance threshold: roughly 1/3 of the name length, min 1, max 3
    let threshold = (name.len() / 3).clamp(1, 3);

    candidates
        .iter()
        .filter_map(|c| {
            let d = edit_distance(name, c);
            if d > 0 && d <= threshold {
                Some((*c, d))
            } else {
                None
            }
        })
        .min_by_key(|(_, d)| *d)
        .map(|(name, _)| name.to_string())
}

/// Provide targeted hints for common names from other Lisp dialects.
/// Checked before fuzzy matching to give more helpful, specific guidance.
pub fn veteran_hint(name: &str) -> Option<&'static str> {
    match name {
        // Common Lisp / Emacs Lisp
        "setq" | "setf" => Some("Sema uses 'set!' for variable assignment"),
        "progn" => Some("Sema uses 'begin' to sequence expressions"),
        "funcall" => Some("In Sema, functions are called directly: (f arg ...)"),
        "mapcar" => Some("Sema uses 'map' for mapping over lists"),
        "loop" => Some("Sema uses 'do' or 'while' for iteration, or tail recursion"),
        "princ" | "prin1" => Some("Sema uses 'print' or 'println' for output"),
        "format-string" => Some("Sema uses 'format' with ~a (display) and ~s (write) directives"),
        "defvar" | "defparameter" => Some("Sema uses 'define' for variable definitions"),
        "labels" | "flet" => Some("Sema uses 'letrec' for local recursive bindings"),
        "block" | "return-from" => {
            Some("Sema uses 'begin' for sequencing; use 'throw'/'try' for non-local exits")
        }
        "multiple-value-bind" => Some("Sema uses destructuring 'let' for multiple return values"),
        "typep" | "type-of" => Some("Sema uses 'type' to get the type of a value"),

        // Clojure
        "defn" => Some("Sema uses 'defun' to define named functions"),
        "atom" => Some("Sema is single-threaded; use 'define' for mutable state with 'set!'"),
        "swap!" => Some("Sema is single-threaded; use 'set!' for mutation"),
        "deref" => Some("Sema uses 'force' to evaluate delayed/promised values"),
        "into" => Some("Use type-specific conversions like 'list->vector' or 'vector->list'"),
        "conj" => Some("Sema uses 'cons' to prepend and 'append' to add to the end"),
        "some" => Some("Sema uses 'any' to test if any element matches a predicate"),
        "every?" => Some("Sema uses 'every' (without '?') to test if all elements match"),
        "any?" => Some("Sema uses 'any' (without '?') to test if any element matches"),
        "not=" => Some("Use (not (equal? a b)) for inequality in Sema"),

        // Scheme / Racket
        "define-syntax" | "syntax-rules" | "syntax-case" => {
            Some("Sema uses 'defmacro' for macro definitions")
        }
        "call-with-current-continuation" | "call/cc" => Some(
            "Sema doesn't support first-class continuations; use 'try'/'throw' for control flow",
        ),
        "string-join" => Some("Sema uses 'string/join' (slash-namespaced)"),
        "string-split" => Some("Sema uses 'string/split' (slash-namespaced)"),
        "string-trim" => Some("Sema uses 'string/trim' (slash-namespaced)"),
        "string-contains" => Some("Sema uses 'string/contains?' (slash-namespaced, with '?')"),
        "string-upcase" | "string-downcase" => Some("Sema uses 'string/upper' and 'string/lower'"),
        "make-string" => Some("Sema uses 'string/repeat' to create repeated strings"),
        "hash-ref" => Some("Sema uses 'get' to look up values in maps"),
        "hash-set!" => Some("Sema maps are immutable; use 'assoc' to create an updated copy"),
        "hash-map?" => Some("Sema uses 'map?' to check if a value is a map"),
        "with-exception-handler" | "raise" => {
            Some("Sema uses 'try'/'catch' and 'throw' for exception handling")
        }

        _ => None,
    }
}

impl SemaError {
    pub fn eval(msg: impl Into<String>) -> Self {
        SemaError::Eval(msg.into())
    }

    pub fn type_error(expected: impl Into<String>, got: impl Into<String>) -> Self {
        SemaError::Type {
            expected: expected.into(),
            got: got.into(),
            got_value: None,
        }
    }

    pub fn type_error_with_value(
        expected: impl Into<String>,
        got: impl Into<String>,
        value: &Value,
    ) -> Self {
        let display = format!("{value}");
        let truncated = if display.len() > 40 {
            format!("{}…", crate::text_util::truncate_chars(&display, 39))
        } else {
            display
        };
        SemaError::Type {
            expected: expected.into(),
            got: got.into(),
            got_value: Some(truncated),
        }
    }

    pub fn arity(name: impl Into<String>, expected: impl Into<String>, got: usize) -> Self {
        SemaError::Arity {
            name: name.into(),
            expected: expected.into(),
            got,
        }
    }

    /// Attach a hint (actionable suggestion) to this error.
    pub fn with_hint(self, hint: impl Into<String>) -> Self {
        match self {
            SemaError::WithContext { inner, note, .. } => SemaError::WithContext {
                inner,
                hint: Some(hint.into()),
                note,
            },
            other => SemaError::WithContext {
                inner: Box::new(other),
                hint: Some(hint.into()),
                note: None,
            },
        }
    }

    /// Attach a note (extra context) to this error.
    pub fn with_note(self, note: impl Into<String>) -> Self {
        match self {
            SemaError::WithContext { inner, hint, .. } => SemaError::WithContext {
                inner,
                hint,
                note: Some(note.into()),
            },
            other => SemaError::WithContext {
                inner: Box::new(other),
                hint: None,
                note: Some(note.into()),
            },
        }
    }

    /// Get the hint from this error, if any.
    pub fn hint(&self) -> Option<&str> {
        match self {
            SemaError::WithContext { hint, .. } => hint.as_deref(),
            SemaError::WithTrace { inner, .. } => inner.hint(),
            _ => None,
        }
    }

    /// Get the note from this error, if any.
    pub fn note(&self) -> Option<&str> {
        match self {
            SemaError::WithContext { note, .. } => note.as_deref(),
            SemaError::WithTrace { inner, .. } => inner.note(),
            _ => None,
        }
    }

    /// Wrap this error with a stack trace (no-op if already wrapped).
    pub fn with_stack_trace(self, trace: StackTrace) -> Self {
        if trace.0.is_empty() {
            return self;
        }
        match self {
            SemaError::WithTrace { .. } => self,
            SemaError::WithContext { inner, hint, note } => SemaError::WithContext {
                inner: Box::new(inner.with_stack_trace(trace)),
                hint,
                note,
            },
            other => SemaError::WithTrace {
                inner: Box::new(other),
                trace,
            },
        }
    }

    pub fn stack_trace(&self) -> Option<&StackTrace> {
        match self {
            SemaError::WithTrace { trace, .. } => Some(trace),
            SemaError::WithContext { inner, .. } => inner.stack_trace(),
            _ => None,
        }
    }

    pub fn inner(&self) -> &SemaError {
        match self {
            SemaError::WithTrace { inner, .. } => inner.inner(),
            SemaError::WithContext { inner, .. } => inner.inner(),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    // 1. Span Display
    #[test]
    fn span_display() {
        let span = Span::point(1, 5);
        assert_eq!(span.to_string(), "1:5");
    }

    // 2. StackTrace Display — file+span, file only, span only, neither
    //    Intentionally testing the Display format; string assertions are appropriate here.
    #[test]
    fn stack_trace_display() {
        let trace = StackTrace(vec![
            CallFrame {
                name: "foo".into(),
                file: Some("/a/b.sema".into()),
                span: Some(Span::point(3, 7)),
            },
            CallFrame {
                name: "bar".into(),
                file: Some("/c/d.sema".into()),
                span: None,
            },
            CallFrame {
                name: "baz".into(),
                file: None,
                span: Some(Span::point(10, 1)),
            },
            CallFrame {
                name: "qux".into(),
                file: None,
                span: None,
            },
        ]);
        let s = trace.to_string();
        assert!(s.contains("at foo (/a/b.sema:3:7)"));
        assert!(s.contains("at bar (/c/d.sema)"));
        assert!(s.contains("at baz (<input>:10:1)"));
        assert!(s.contains("at qux\n"));
    }

    // 3. SemaError::eval() constructor — verify variant/fields AND display
    #[test]
    fn type_error_with_value_does_not_split_multibyte_char() {
        // A value whose display is > 40 bytes with a multi-byte char straddling
        // byte 39: truncating at a raw byte index would split the char ("byte
        // index 39 is not a char boundary"), so truncation must land on a boundary.
        let value = Value::string(&format!("x{}", "λ".repeat(40)));
        let e = SemaError::type_error_with_value("map", "string", &value);
        // Must construct without panicking and carry a truncated display.
        match e {
            SemaError::Type { got_value, .. } => {
                let gv = got_value.expect("got_value should be Some");
                assert!(gv.ends_with('…'));
            }
            other => panic!("expected Type variant, got {other:?}"),
        }
    }

    #[test]
    fn eval_error() {
        let e = SemaError::eval("something broke");
        // Structural check: correct variant with expected message
        assert!(
            matches!(&e, SemaError::Eval(msg) if msg == "something broke"),
            "expected Eval variant with message 'something broke', got {e:?}"
        );
        // Display check (intentionally testing Display format)
        assert_eq!(e.to_string(), "Eval error: something broke");
    }

    // 4. SemaError::type_error() constructor — verify variant/fields AND display
    #[test]
    fn type_error() {
        let e = SemaError::type_error("string", "integer");
        // Structural check: correct variant with expected fields
        assert!(
            matches!(
                &e,
                SemaError::Type { expected, got, got_value }
                if expected == "string" && got == "integer" && got_value.is_none()
            ),
            "expected Type variant with expected='string', got='integer', got_value=None, got {e:?}"
        );
        // Display check (intentionally testing Display format)
        assert_eq!(e.to_string(), "Type error: expected string, got integer");
    }

    // 5. SemaError::arity() constructor — verify variant/fields AND display
    #[test]
    fn arity_error() {
        let e = SemaError::arity("my-fn", "2", 5);
        // Structural check: correct variant with expected fields
        assert!(
            matches!(
                &e,
                SemaError::Arity { name, expected, got }
                if name == "my-fn" && expected == "2" && *got == 5
            ),
            "expected Arity variant with name='my-fn', expected='2', got=5, got {e:?}"
        );
        // Display check (intentionally testing Display format)
        assert_eq!(e.to_string(), "Arity error: my-fn expects 2 args, got 5");
    }

    // 6. with_hint attaches hint retrievable via .hint()
    #[test]
    fn with_hint() {
        let e = SemaError::eval("oops").with_hint("try this");
        assert_eq!(e.hint(), Some("try this"));
    }

    // 7. with_note attaches note retrievable via .note()
    #[test]
    fn with_note() {
        let e = SemaError::eval("oops").with_note("extra info");
        assert_eq!(e.note(), Some("extra info"));
    }

    // 8. with_hint on already-wrapped WithContext preserves note
    #[test]
    fn with_hint_preserves_note() {
        let e = SemaError::eval("oops")
            .with_note("kept note")
            .with_hint("new hint");
        assert_eq!(e.hint(), Some("new hint"));
        assert_eq!(e.note(), Some("kept note"));
    }

    // 9. with_note on already-wrapped WithContext preserves hint
    #[test]
    fn with_note_preserves_hint() {
        let e = SemaError::eval("oops")
            .with_hint("kept hint")
            .with_note("new note");
        assert_eq!(e.hint(), Some("kept hint"));
        assert_eq!(e.note(), Some("new note"));
    }

    // 10. with_stack_trace wraps in WithTrace, retrievable via .stack_trace()
    #[test]
    fn with_stack_trace() {
        let trace = StackTrace(vec![CallFrame {
            name: "f".into(),
            file: None,
            span: None,
        }]);
        let e = SemaError::eval("err").with_stack_trace(trace);
        let st = e.stack_trace().expect("should have stack trace");
        assert_eq!(st.0.len(), 1);
        assert_eq!(st.0[0].name, "f");
    }

    // 11. with_stack_trace with empty trace is no-op
    #[test]
    fn with_stack_trace_empty_is_noop() {
        let e = SemaError::eval("err").with_stack_trace(StackTrace(vec![]));
        assert!(e.stack_trace().is_none());
        assert!(matches!(e, SemaError::Eval(_)));
    }

    // 12. with_stack_trace on already-wrapped WithTrace is no-op
    #[test]
    fn with_stack_trace_already_wrapped_is_noop() {
        let frame = || CallFrame {
            name: "first".into(),
            file: None,
            span: None,
        };
        let e = SemaError::eval("err").with_stack_trace(StackTrace(vec![frame()]));
        let e2 = e.with_stack_trace(StackTrace(vec![CallFrame {
            name: "second".into(),
            file: None,
            span: None,
        }]));
        let st = e2.stack_trace().unwrap();
        assert_eq!(st.0.len(), 1);
        assert_eq!(st.0[0].name, "first");
    }

    // 13. inner() unwraps through WithTrace and WithContext
    #[test]
    fn inner_unwraps() {
        let e = SemaError::eval("root")
            .with_hint("h")
            .with_stack_trace(StackTrace(vec![CallFrame {
                name: "x".into(),
                file: None,
                span: None,
            }]));
        let inner = e.inner();
        assert!(matches!(inner, SemaError::Eval(msg) if msg == "root"));
    }

    // 14. hint() and note() return None on plain errors
    #[test]
    fn hint_note_none_on_plain() {
        let e = SemaError::eval("plain");
        assert!(e.hint().is_none());
        assert!(e.note().is_none());
    }

    // 15. check_arity! exact match passes, mismatch returns error
    #[test]
    fn check_arity_exact() {
        fn run(args: &[Value]) -> Result<(), SemaError> {
            check_arity!(args, "test-fn", 2);
            Ok(())
        }
        assert!(run(&[Value::nil(), Value::nil()]).is_ok());
        let err = run(&[Value::nil()]).unwrap_err();
        assert!(err.to_string().contains("test-fn"));
        assert!(err.to_string().contains("2"));
    }

    // 16. check_arity! range match (1..=3) passes and fails
    #[test]
    fn check_arity_range() {
        fn run(args: &[Value]) -> Result<(), SemaError> {
            check_arity!(args, "range-fn", 1..=3);
            Ok(())
        }
        assert!(run(&[Value::nil()]).is_ok());
        assert!(run(&[Value::nil(), Value::nil()]).is_ok());
        assert!(run(&[Value::nil(), Value::nil(), Value::nil()]).is_ok());
        assert!(run(&[]).is_err());
        assert!(run(&[Value::nil(), Value::nil(), Value::nil(), Value::nil()]).is_err());
    }

    #[test]
    fn test_suggest_similar() {
        assert_eq!(
            suggest_similar(
                "strng/join",
                &["string/join", "string/split", "map", "println"]
            ),
            Some("string/join".to_string())
        );
        assert_eq!(
            suggest_similar("pritnln", &["println", "print", "map"]),
            Some("println".to_string())
        );
        assert_eq!(suggest_similar("xyzzy", &["a", "b", "c"]), None);
    }

    // 17. check_arity! open range (2..) passes and fails
    #[test]
    fn check_arity_open_range() {
        fn run(args: &[Value]) -> Result<(), SemaError> {
            check_arity!(args, "open-fn", 2..);
            Ok(())
        }
        assert!(run(&[Value::nil(), Value::nil()]).is_ok());
        assert!(run(&[Value::nil(), Value::nil(), Value::nil()]).is_ok());
        assert!(run(&[Value::nil()]).is_err());
        assert!(run(&[]).is_err());
    }

    #[test]
    fn test_veteran_hint_known() {
        assert_eq!(
            veteran_hint("defn"),
            Some("Sema uses 'defun' to define named functions")
        );
        assert_eq!(
            veteran_hint("setq"),
            Some("Sema uses 'set!' for variable assignment")
        );
        assert_eq!(
            veteran_hint("progn"),
            Some("Sema uses 'begin' to sequence expressions")
        );
        assert_eq!(
            veteran_hint("mapcar"),
            Some("Sema uses 'map' for mapping over lists")
        );
    }

    #[test]
    fn test_veteran_hint_unknown() {
        assert!(veteran_hint("xyzzy").is_none());
        assert!(veteran_hint("println").is_none());
    }

    #[test]
    fn test_veteran_hint_existing_sema_names() {
        // Names that exist in Sema should return None
        assert!(veteran_hint("do").is_none());
        assert!(veteran_hint("while").is_none());
        assert!(veteran_hint("str").is_none());
        assert!(veteran_hint("count").is_none());
    }

    // type_error_with_value constructor — verify variant/fields AND display
    #[test]
    fn type_error_with_value_display() {
        let e = SemaError::type_error_with_value("string", "integer", &Value::int(42));
        // Structural check: correct variant with got_value populated
        assert!(
            matches!(
                &e,
                SemaError::Type { expected, got, got_value }
                if expected == "string" && got == "integer" && got_value.as_deref() == Some("42")
            ),
            "expected Type variant with expected='string', got='integer', got_value=Some(\"42\"), got {e:?}"
        );
        // Display check (intentionally testing Display format)
        assert_eq!(
            e.to_string(),
            "Type error: expected string, got integer (42)"
        );
    }

    // type_error without value — verify got_value is None AND display
    #[test]
    fn type_error_without_value_display() {
        let e = SemaError::type_error("string", "integer");
        // Structural check: got_value should be None
        assert!(
            matches!(
                &e,
                SemaError::Type { got_value, .. } if got_value.is_none()
            ),
            "expected Type variant with got_value=None, got {e:?}"
        );
        // Display check (intentionally testing Display format)
        assert_eq!(e.to_string(), "Type error: expected string, got integer");
    }
}
