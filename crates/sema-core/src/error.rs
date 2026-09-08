use std::collections::{BTreeMap, HashMap};
use std::fmt;

use crate::runtime::{CancelReason, OperationId, RootId, ScopeId};
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

/// Return a SpanMap key for a top-level immediate expression by its source order.
///
/// Immediate values do not carry an allocation identity, so this reserves the
/// unreachable upper address range for parser-to-compiler bookkeeping. Source
/// order keeps repeated equal values on distinct lines distinct.
pub fn top_level_span_key(index: usize) -> usize {
    usize::MAX - index
}

/// Structured details for a policy denial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDenial {
    pub policy: Option<String>,
    pub boundary: String,
    pub subject: String,
    pub rule: String,
    pub reason: String,
    pub action: String,
    pub source: String,
}

impl fmt::Display for PolicyDenial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(policy) = &self.policy {
            write!(
                f,
                "Policy '{policy}' denied {} '{}': {}",
                self.boundary, self.subject, self.reason
            )
        } else {
            write!(
                f,
                "Policy denied {} '{}': {}",
                self.boundary, self.subject, self.reason
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeContext {
    pub function: String,
    pub argument: Option<usize>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SemaError {
    #[error("Reader error at {span}: {message}")]
    Reader { message: String, span: Span },

    #[error("Eval error: {0}")]
    Eval(String),

    #[error("Type error: {}expected {expected}, got {got}{}", type_context(context.as_deref()), got_value.as_ref().map(|v| format!(" ({v})")).unwrap_or_default())]
    Type {
        context: Option<Box<TypeContext>>,
        expected: String,
        got: String,
        got_value: Option<String>,
    },

    #[error(
        "Arity error: {name} expects {}, got {got}",
        format_expected_arity(expected)
    )]
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

    #[error("{0}")]
    PolicyDenied(Box<PolicyDenial>),

    /// Internal workflow control transfer emitted after a durable approval request has
    /// been created. This is deliberately not catchable by Sema `try`/`catch`; only the
    /// enclosing `workflow/run` consumes it and returns a `:needs-approval` envelope.
    #[error("workflow approval required: {approval_id}")]
    WorkflowApprovalRequired { approval_id: String },

    /// Internal workflow control transfer emitted when a durable rejection is observed.
    /// Like [`Self::WorkflowApprovalRequired`], user code cannot catch it and continue
    /// past the protected action.
    #[error("workflow approval rejected: {approval_id}")]
    WorkflowApprovalRejected {
        approval_id: String,
        reason: Option<String>,
    },

    /// Fail-closed approval infrastructure or placement error. It is host-owned and
    /// uncatchable for the same reason as pending/rejected controls: user code must not
    /// continue to the protected action after authority validation fails.
    #[error("workflow approval failed: {message}")]
    WorkflowApprovalFailed { message: String },

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("User exception: {0}")]
    UserException(Value),

    /// A re-raised condition map — the `{:type ... :message ...}` value a
    /// `catch`/`guard` handler was bound to, thrown again. Kept as that map
    /// verbatim so catching and re-throwing is idempotent: N nested
    /// `(catch e ... (throw e))` guards surface the same condition as one.
    /// Displays as the condition's `:message` so the top-level report reads
    /// like the original error, not a stringified map.
    #[error("{}", condition_message(.0))]
    Condition(Value),

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

fn type_context(context: Option<&TypeContext>) -> String {
    match context {
        Some(TypeContext {
            function,
            argument: Some(argument),
        }) => format!("{function} argument {argument} "),
        Some(TypeContext {
            function,
            argument: None,
        }) => format!("{function} "),
        None => String::new(),
    }
}

fn format_expected_arity(expected: &str) -> String {
    if let Some(minimum) = expected.strip_suffix('+') {
        return format!("{minimum} or more arguments");
    }
    if let Some((minimum, maximum)) = expected.split_once('-') {
        return format!("{minimum} to {maximum} arguments");
    }
    if expected.contains(" or ") {
        return format!("{expected} arguments");
    }
    match expected {
        "0" => "no arguments".to_string(),
        "1" => "1 argument".to_string(),
        _ => format!("{expected} arguments"),
    }
}

fn type_message(
    context: Option<&TypeContext>,
    expected: &str,
    got: &str,
    got_value: Option<&str>,
) -> String {
    let value = got_value.map_or_else(String::new, |value| format!(" ({value})"));
    format!(
        "{}expected {expected}, got {got}{value}",
        type_context(context)
    )
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
///
/// Only for names Sema does *not* have. Names accepted as aliases (`defn`,
/// `progn`, `def`, `fn`) belong in the special-form table, not here — a hint
/// redirecting away from a working form is worse than no hint.
pub fn veteran_hint(name: &str) -> Option<&'static str> {
    match name {
        // Common Lisp / Emacs Lisp
        "setq" | "setf" => Some("Sema uses 'set!' for variable assignment"),
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
        "syntax-case" => {
            Some("Sema supports 'define-syntax' with 'syntax-rules', or use 'defmacro'")
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
        "with-exception-handler" => {
            Some("Sema uses 'try'/'catch', 'throw'/'raise', and 'guard' for exception handling")
        }

        _ => None,
    }
}

/// The `:type` keywords `error_to_value` puts on condition maps. A thrown map
/// is only treated as a re-raised condition when its `:type` is one of these
/// (and `:message` is a string) — user data maps that merely resemble a
/// condition keep the wrap-as-user-exception behavior.
const CONDITION_TYPES: &[&str] = &[
    "eval",
    "type-error",
    "arity",
    "unbound",
    "user",
    "io",
    "llm",
    "reader",
    "permission-denied",
    "policy-denied",
    "internal",
    "cancelled",
    "timeout",
];

/// The `:message` of a condition map, for `Display` of `SemaError::Condition`.
/// Falls back to the whole map's printed form if the shape is unexpected.
fn condition_message(condition: &Value) -> String {
    condition
        .as_map_ref()
        .and_then(|m| m.get(&Value::keyword("message")))
        .and_then(|msg| msg.as_str().map(str::to_string))
        .unwrap_or_else(|| condition.to_string())
}

/// True when `value` has the exact shape of a caught condition map:
/// a map with a known keyword `:type` and a string `:message`.
fn is_condition_map(value: &Value) -> bool {
    let Some(map) = value.as_map_ref() else {
        return false;
    };
    let known_type = map
        .get(&Value::keyword("type"))
        .and_then(|t| t.as_keyword())
        .is_some_and(|kw| CONDITION_TYPES.contains(&kw.as_str()));
    known_type
        && map
            .get(&Value::keyword("message"))
            .is_some_and(|m| m.as_str().is_some())
}

fn cancel_reason_keyword(reason: CancelReason) -> &'static str {
    match reason {
        CancelReason::Root => "root",
        CancelReason::Owner => "owner",
        CancelReason::Explicit => "explicit",
        CancelReason::Timeout => "timeout",
        CancelReason::HostStop => "host-stop",
        CancelReason::ResourceDisconnect => "resource-disconnect",
        CancelReason::InterpreterShutdown => "interpreter-shutdown",
    }
}

fn insert_decimal(condition: &mut BTreeMap<Value, Value>, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        condition.insert(Value::keyword(key), Value::int(value as i64));
    }
}

fn insert_optional_string(condition: &mut BTreeMap<Value, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        condition.insert(Value::keyword(key), Value::string(value));
    }
}

impl SemaError {
    pub fn eval(msg: impl Into<String>) -> Self {
        SemaError::Eval(msg.into())
    }

    pub fn policy_denied(denial: PolicyDenial) -> Self {
        let rule = denial.rule.clone();
        SemaError::PolicyDenied(Box::new(denial)).with_note(format!("policy rule: {rule}"))
    }

    /// Whether this error is a host-owned control transfer that language-level exception
    /// handlers must not intercept.
    pub fn is_uncatchable(&self) -> bool {
        matches!(
            self.inner(),
            SemaError::WorkflowApprovalRequired { .. }
                | SemaError::WorkflowApprovalRejected { .. }
                | SemaError::WorkflowApprovalFailed { .. }
        )
    }

    pub fn internal(message: impl Into<String>) -> Self {
        SemaError::Internal(message.into())
            .with_hint("report this as a Sema bug and include the stack trace")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn cancelled_condition(
        message: &str,
        reason: CancelReason,
        root_id: Option<RootId>,
        scope_id: Option<ScopeId>,
        operation_id: Option<OperationId>,
        operation: Option<&str>,
        duration_ms: Option<u64>,
        resource_kind: Option<&str>,
    ) -> Self {
        let mut condition = BTreeMap::from([
            (Value::keyword("type"), Value::keyword("cancelled")),
            (Value::keyword("message"), Value::string(message)),
            (
                Value::keyword("reason"),
                Value::keyword(cancel_reason_keyword(reason)),
            ),
        ]);
        insert_decimal(&mut condition, "root-id", root_id.map(RootId::get));
        insert_decimal(&mut condition, "scope-id", scope_id.map(ScopeId::get));
        insert_decimal(
            &mut condition,
            "operation-id",
            operation_id.map(OperationId::get),
        );
        insert_optional_string(&mut condition, "operation", operation);
        insert_decimal(&mut condition, "duration-ms", duration_ms);
        insert_optional_string(&mut condition, "resource-kind", resource_kind);
        SemaError::Condition(Value::map(condition))
    }

    pub fn timeout_condition(
        message: &str,
        operation: &str,
        duration_ms: u64,
        operation_id: Option<OperationId>,
    ) -> Self {
        let mut condition = BTreeMap::from([
            (Value::keyword("type"), Value::keyword("timeout")),
            (Value::keyword("message"), Value::string(message)),
            (Value::keyword("operation"), Value::string(operation)),
            (
                Value::keyword("duration-ms"),
                Value::int(duration_ms as i64),
            ),
        ]);
        insert_decimal(
            &mut condition,
            "operation-id",
            operation_id.map(OperationId::get),
        );
        SemaError::Condition(Value::map(condition))
    }

    /// The error a `throw`/`raise` of `value` raises: a caught condition map
    /// re-raises as itself (`Condition`) so nested catch/re-throw guards don't
    /// wrap it again per layer; anything else is a fresh `UserException`.
    pub fn from_thrown(value: Value) -> Self {
        if is_condition_map(&value) {
            SemaError::Condition(value)
        } else {
            SemaError::UserException(value)
        }
    }

    pub fn type_error(expected: impl Into<String>, got: impl Into<String>) -> Self {
        SemaError::Type {
            context: None,
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
        SemaError::Type {
            context: None,
            expected: expected.into(),
            got: got.into(),
            got_value: Some(Self::value_preview(value)),
        }
    }

    pub fn argument_type(
        function: impl Into<String>,
        argument: usize,
        expected: impl Into<String>,
        value: &Value,
    ) -> Self {
        SemaError::Type {
            context: Some(Box::new(TypeContext {
                function: function.into(),
                argument: Some(argument),
            })),
            expected: expected.into(),
            got: value.type_name().to_string(),
            got_value: None,
        }
    }

    pub fn argument_type_with_value(
        function: impl Into<String>,
        argument: usize,
        expected: impl Into<String>,
        value: &Value,
    ) -> Self {
        let mut error = Self::argument_type(function, argument, expected, value);
        if let SemaError::Type { got_value, .. } = &mut error {
            *got_value = Some(Self::value_preview(value));
        }
        error
    }

    fn value_preview(value: &Value) -> String {
        let display = format!("{value}");
        if display.len() > 40 {
            format!("{}…", crate::text_util::truncate_chars(&display, 39))
        } else {
            display
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

    /// Append caller frames to an existing stack trace.
    ///
    /// A VM-backed closure can run in a separate VM when invoked by a native
    /// callback. Its error already has the callee frames when it returns to the
    /// caller VM, so replacing that trace would lose the failing location.
    pub fn append_stack_trace(self, trace: StackTrace) -> Self {
        if trace.0.is_empty() {
            return self;
        }
        match self {
            SemaError::WithTrace {
                inner,
                trace: mut existing,
            } => {
                existing.0.extend(trace.0);
                SemaError::WithTrace {
                    inner,
                    trace: existing,
                }
            }
            SemaError::WithContext { inner, hint, note } => SemaError::WithContext {
                inner: Box::new(inner.append_stack_trace(trace)),
                hint,
                note,
            },
            other => other.with_stack_trace(trace),
        }
    }

    /// Fill in `file` on any trace frame that lacks one (no-op without a trace).
    ///
    /// Lowering errors synthesize frames with `file: None` because the lowering
    /// pass doesn't know the source path; the compile entry points (which do)
    /// stamp it here so traces render the real filename instead of `<input>`.
    /// Frames that already carry a file are left untouched.
    pub fn fill_trace_file(self, file: &std::path::Path) -> Self {
        match self {
            SemaError::WithTrace { inner, mut trace } => {
                for frame in &mut trace.0 {
                    if frame.file.is_none() {
                        frame.file = Some(file.to_path_buf());
                    }
                }
                SemaError::WithTrace { inner, trace }
            }
            SemaError::WithContext { inner, hint, note } => SemaError::WithContext {
                inner: Box::new(inner.fill_trace_file(file)),
                hint,
                note,
            },
            other => other,
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

    /// Return the primary user-facing message without wrapper prefixes.
    pub fn user_message(&self) -> String {
        match self.inner() {
            SemaError::Reader { message, .. } | SemaError::Eval(message) => message.clone(),
            SemaError::Type {
                context,
                expected,
                got,
                got_value,
            } => type_message(context.as_deref(), expected, got, got_value.as_deref()),
            SemaError::Arity {
                name,
                expected,
                got,
            } => format!(
                "{name} expects {}, got {got}",
                format_expected_arity(expected)
            ),
            SemaError::Unbound(name) => format!("Unbound variable: {name}"),
            SemaError::Llm(message) => format!("LLM error: {message}"),
            SemaError::Io(message) => format!("I/O error: {message}"),
            SemaError::PermissionDenied {
                function,
                capability,
            } => format!("Permission denied: {function} requires '{capability}' capability"),
            SemaError::PathDenied { function, path } => format!(
                "Permission denied: {function} — path '{path}' is outside allowed directories"
            ),
            SemaError::PolicyDenied(denial) => denial.to_string(),
            SemaError::WorkflowApprovalRequired { approval_id } => {
                format!("workflow approval required: {approval_id}")
            }
            SemaError::WorkflowApprovalRejected {
                approval_id,
                reason,
            } => reason.as_ref().map_or_else(
                || format!("workflow approval rejected: {approval_id}"),
                |reason| format!("workflow approval rejected: {approval_id}: {reason}"),
            ),
            SemaError::WorkflowApprovalFailed { message } => {
                format!("workflow approval failed: {message}")
            }
            SemaError::Internal(message) => format!("Internal error: {message}"),
            SemaError::UserException(value) => format!("User exception: {value}"),
            SemaError::Condition(condition) => condition_message(condition),
            SemaError::WithTrace { .. } | SemaError::WithContext { .. } => {
                unreachable!("inner() already unwraps wrappers")
            }
        }
    }

    /// Format a diagnostic message without source location or stack frames.
    pub fn format_diagnostic(&self) -> String {
        let mut message = self.user_message();
        if let Some(hint) = self.hint() {
            message.push_str("\n  hint: ");
            message.push_str(hint);
        }
        if let Some(note) = self.note() {
            message.push_str("\n  note: ");
            message.push_str(note);
        }
        message
    }

    /// Format an error for a plain-text channel.
    pub fn format_plain(&self) -> String {
        let mut message = self.user_message();
        if let SemaError::Reader { span, .. } = self.inner() {
            message.push_str(&format!("\n  at <input>:{span}"));
        }
        if let Some(trace) = self.stack_trace() {
            message.push('\n');
            message.push_str(trace.to_string().trim_end());
        }
        if let Some(hint) = self.hint() {
            message.push_str("\n  hint: ");
            message.push_str(hint);
        }
        if let Some(note) = self.note() {
            message.push_str("\n  note: ");
            message.push_str(note);
        }
        message
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
                SemaError::Type { expected, got, got_value, .. }
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
        assert_eq!(
            e.to_string(),
            "Arity error: my-fn expects 2 arguments, got 5"
        );
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

    #[test]
    fn append_stack_trace_keeps_callee_and_caller_frames() {
        let e = SemaError::eval("err")
            .with_stack_trace(StackTrace(vec![CallFrame {
                name: "callee".into(),
                file: None,
                span: None,
            }]))
            .append_stack_trace(StackTrace(vec![CallFrame {
                name: "caller".into(),
                file: None,
                span: None,
            }]));
        let names: Vec<_> = e
            .stack_trace()
            .unwrap()
            .0
            .iter()
            .map(|frame| frame.name.as_str())
            .collect();
        assert_eq!(names, ["callee", "caller"]);
    }

    // fill_trace_file fills only `file: None` frames, recurses through
    // WithContext, and is a no-op without a trace.
    #[test]
    fn fill_trace_file_fills_missing_files() {
        let e = SemaError::eval("err")
            .with_stack_trace(StackTrace(vec![
                CallFrame {
                    name: "bare".into(),
                    file: None,
                    span: None,
                },
                CallFrame {
                    name: "stamped".into(),
                    file: Some("already.sema".into()),
                    span: None,
                },
            ]))
            .with_hint("h");
        let e = e.fill_trace_file(std::path::Path::new("main.sema"));
        assert_eq!(e.hint(), Some("h"));
        let st = e.stack_trace().unwrap();
        assert_eq!(
            st.0[0].file.as_deref(),
            Some(std::path::Path::new("main.sema"))
        );
        assert_eq!(
            st.0[1].file.as_deref(),
            Some(std::path::Path::new("already.sema"))
        );
    }

    #[test]
    fn fill_trace_file_noop_without_trace() {
        let e = SemaError::eval("err").fill_trace_file(std::path::Path::new("main.sema"));
        assert!(e.stack_trace().is_none());
        assert!(matches!(e, SemaError::Eval(_)));
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
            veteran_hint("setq"),
            Some("Sema uses 'set!' for variable assignment")
        );
        assert_eq!(
            veteran_hint("mapcar"),
            Some("Sema uses 'map' for mapping over lists")
        );
        assert_eq!(
            veteran_hint("funcall"),
            Some("In Sema, functions are called directly: (f arg ...)")
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
        // Accepted aliases — hinting away from a working form would mislead
        assert!(veteran_hint("defn").is_none());
        assert!(veteran_hint("progn").is_none());
        assert!(veteran_hint("def").is_none());
    }

    // type_error_with_value constructor — verify variant/fields AND display
    #[test]
    fn type_error_with_value_display() {
        let e = SemaError::type_error_with_value("string", "integer", &Value::int(42));
        // Structural check: correct variant with got_value populated
        assert!(
            matches!(
                &e,
                SemaError::Type { expected, got, got_value, .. }
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

    #[test]
    fn argument_type_includes_call_context() {
        let e = SemaError::argument_type_with_value("string/split", 1, "string", &Value::int(42));
        assert_eq!(
            e.user_message(),
            "string/split argument 1 expected string, got int (42)"
        );
    }

    #[test]
    fn arity_expectations_use_readable_grammar() {
        let cases = [
            ("0", "f expects no arguments, got 9"),
            ("1", "f expects 1 argument, got 9"),
            ("2", "f expects 2 arguments, got 9"),
            ("1+", "f expects 1 or more arguments, got 9"),
            ("2-4", "f expects 2 to 4 arguments, got 9"),
            ("2 or 3", "f expects 2 or 3 arguments, got 9"),
        ];
        for (expected, message) in cases {
            assert_eq!(SemaError::arity("f", expected, 9).user_message(), message);
        }
    }

    #[test]
    fn policy_denial_preserves_details_and_renders_context() {
        let e = SemaError::policy_denied(PolicyDenial {
            policy: Some("safe-agent".to_string()),
            boundary: "tool".to_string(),
            subject: "shell/run".to_string(),
            rule: "tools.shell.deny".to_string(),
            reason: "command execution is not allowed".to_string(),
            action: "fail".to_string(),
            source: "request".to_string(),
        });
        assert_eq!(
            e.user_message(),
            "Policy 'safe-agent' denied tool 'shell/run': command execution is not allowed"
        );
        assert_eq!(e.note(), Some("policy rule: tools.shell.deny"));
    }

    #[test]
    fn plain_format_orders_trace_hint_and_note() {
        let e = SemaError::eval("failed")
            .with_stack_trace(StackTrace(vec![CallFrame {
                name: "main".to_string(),
                file: None,
                span: Some(Span::point(2, 3)),
            }]))
            .with_hint("try again")
            .with_note("extra context");
        assert_eq!(
            e.format_plain(),
            "failed\n  at main (<input>:2:3)\n  hint: try again\n  note: extra context"
        );
    }
}
