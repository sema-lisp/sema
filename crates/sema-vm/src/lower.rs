use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use sema_core::{intern, SemaError, Span, SpanMap, Spur, Value, ValueView};

use crate::core_expr::{CoreExpr, DoLoop, DoVar, LambdaDef, PromptEntry};

/// Maximum recursion depth for the lowering pass.
/// This prevents native stack overflow from deeply nested expressions.
const MAX_LOWER_DEPTH: usize = 256;

thread_local! {
    static LOWER_DEPTH: Cell<usize> = const { Cell::new(0) };
    static SPAN_MAP: RefCell<Option<SpanMap>> = const { RefCell::new(None) };
}

/// Lower a Value AST into CoreExpr IR.
/// If `span_map` is provided, attaches source spans for debug support.
pub fn lower(expr: &Value, span_map: Option<&SpanMap>) -> Result<CoreExpr, SemaError> {
    if let Some(sm) = span_map {
        SPAN_MAP.with(|cell| {
            *cell.borrow_mut() = Some(sm.clone());
        });
        // Guard ensures SPAN_MAP is cleared even on panic
        struct SpanMapGuard;
        impl Drop for SpanMapGuard {
            fn drop(&mut self) {
                SPAN_MAP.with(|cell| {
                    *cell.borrow_mut() = None;
                });
            }
        }
        let _guard = SpanMapGuard;
        lower_expr(expr, false)
    } else {
        lower_expr(expr, false)
    }
}

/// Look up the span for a list Value using its Rc pointer identity.
fn lookup_span(val: &Value) -> Option<Span> {
    if let Some(rc) = val.as_list_rc() {
        let ptr = Rc::as_ptr(&rc) as usize;
        SPAN_MAP.with(|sm| sm.borrow().as_ref().and_then(|map| map.get(&ptr).copied()))
    } else {
        None
    }
}

/// Lower a sequence of expressions, marking the last as tail position.
pub fn lower_body(exprs: &[Value], tail: bool) -> Result<Vec<CoreExpr>, SemaError> {
    let mut result = Vec::with_capacity(exprs.len());
    for (i, expr) in exprs.iter().enumerate() {
        let is_last = i == exprs.len() - 1;
        result.push(lower_expr(expr, tail && is_last)?);
    }
    Ok(result)
}

struct LowerDepthGuard;

impl LowerDepthGuard {
    fn new() -> Result<Self, SemaError> {
        let depth = LOWER_DEPTH.with(|d| {
            let v = d.get() + 1;
            d.set(v);
            v
        });
        if depth > MAX_LOWER_DEPTH {
            LOWER_DEPTH.with(|d| d.set(d.get() - 1));
            return Err(SemaError::eval("maximum lowering depth exceeded"));
        }
        Ok(LowerDepthGuard)
    }
}

impl Drop for LowerDepthGuard {
    fn drop(&mut self) {
        LOWER_DEPTH.with(|d| d.set(d.get() - 1));
    }
}

fn lower_expr(expr: &Value, tail: bool) -> Result<CoreExpr, SemaError> {
    let _guard = LowerDepthGuard::new()?;
    lower_expr_inner(expr, tail)
}

fn lower_expr_inner(expr: &Value, tail: bool) -> Result<CoreExpr, SemaError> {
    match expr.view() {
        ValueView::Symbol(spur) => Ok(CoreExpr::Var(spur)),

        ValueView::Vector(items) => {
            let exprs = items
                .iter()
                .map(|v| lower_expr(v, false))
                .collect::<Result<_, _>>()?;
            Ok(CoreExpr::MakeVector(exprs))
        }

        ValueView::Map(map) => {
            let pairs = map
                .iter()
                .map(|(k, v)| Ok((lower_expr(k, false)?, lower_expr(v, false)?)))
                .collect::<Result<Vec<_>, SemaError>>()?;
            Ok(CoreExpr::MakeMap(pairs))
        }

        ValueView::List(items) => {
            if items.is_empty() {
                return Ok(CoreExpr::Const(Value::nil()));
            }
            let span = lookup_span(expr);
            let inner = lower_list(&items, tail)?;
            match span {
                Some(s) => Ok(CoreExpr::Spanned(s, Box::new(inner))),
                None => Ok(inner),
            }
        }

        // Nil, Bool, Int, Float, String, Char, Keyword, Bytevector,
        // NativeFn, Lambda, HashMap, Thunk, and remaining types are self-evaluating
        _ => Ok(CoreExpr::Const(expr.clone())),
    }
}

fn lower_list(items: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    let head = &items[0];
    let args = &items[1..];

    if let Some(spur) = head.as_symbol_spur() {
        if let Some(form) = special_form_for(spur) {
            return match form {
                SpecialForm::Quote => lower_quote(args),
                SpecialForm::If => lower_if(args, tail),
                SpecialForm::Cond => lower_cond(args, tail),
                SpecialForm::Define => lower_define(args),
                SpecialForm::Defun => lower_defun(args),
                SpecialForm::Set => lower_set(args),
                SpecialForm::Lambda => lower_lambda(args, None),
                SpecialForm::Let => lower_let(args, tail),
                SpecialForm::LetStar => lower_let_star(args, tail),
                SpecialForm::Letrec => lower_letrec(args, tail),
                SpecialForm::Begin => lower_begin(args, tail),
                SpecialForm::Do => lower_do(args, tail),
                SpecialForm::And => lower_and(args, tail),
                SpecialForm::Or => lower_or(args, tail),
                SpecialForm::When => lower_when(args, tail),
                SpecialForm::Unless => lower_unless(args, tail),
                SpecialForm::While => lower_while(args),
                SpecialForm::Defmacro => lower_defmacro(args),
                SpecialForm::DefineSyntax => lower_define_syntax(args),
                SpecialForm::Quasiquote => lower_quasiquote(args),
                SpecialForm::Throw => lower_throw(args),
                SpecialForm::Try => lower_try(args, tail),
                SpecialForm::Case => lower_case(args, tail),
                SpecialForm::Eval => lower_eval(args),
                SpecialForm::Macroexpand => lower_macroexpand(args),
                SpecialForm::Module => lower_module(args, tail),
                SpecialForm::Import => lower_import(args),
                SpecialForm::Load => lower_load(args),
                SpecialForm::Prompt => lower_prompt(args),
                SpecialForm::Message => lower_message(args),
                SpecialForm::Deftool => lower_deftool(args),
                SpecialForm::Defagent => lower_defagent(args),
                SpecialForm::Delay => lower_delay(args),
                SpecialForm::Force => lower_force(args),
                SpecialForm::DefineRecordType => lower_define_record_type(args),
                SpecialForm::Match => lower_match(args, tail, false),
                SpecialForm::MatchStar => lower_match(args, tail, true),
                SpecialForm::Defmulti => lower_defmulti(args),
                SpecialForm::Defmethod => lower_defmethod(args),
                SpecialForm::Async => lower_async(args),
                SpecialForm::Await => lower_await(args),
                SpecialForm::LetValues => lower_let_values(args, tail),
                SpecialForm::LetStarValues => lower_let_star_values(args, tail),
                SpecialForm::DefineValues => lower_define_values(args),
            };
        }
    }

    // Not a special form — function call
    let func = lower_expr(head, false)?;
    let call_args = args
        .iter()
        .map(|a| lower_expr(a, false))
        .collect::<Result<_, _>>()?;
    Ok(CoreExpr::Call {
        func: Box::new(func),
        args: call_args,
        tail,
    })
}

/// The set of special forms recognized by the lowerer. Each variant maps to a
/// dedicated `lower_*` handler in [`lower_list`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpecialForm {
    Quote,
    If,
    Cond,
    Define,
    Defun,
    Set,
    Lambda,
    Let,
    LetStar,
    Letrec,
    Begin,
    Do,
    And,
    Or,
    When,
    Unless,
    While,
    Defmacro,
    DefineSyntax,
    Quasiquote,
    Throw,
    Try,
    Case,
    Eval,
    Macroexpand,
    Module,
    Import,
    Load,
    Prompt,
    Message,
    Deftool,
    Defagent,
    Delay,
    Force,
    DefineRecordType,
    Match,
    MatchStar,
    Defmulti,
    Defmethod,
    Async,
    Await,
    LetValues,
    LetStarValues,
    DefineValues,
}

/// The canonical (name, form) table. Names that share a handler (e.g. `define`
/// and `def`) appear as separate rows mapping to the same [`SpecialForm`].
const SPECIAL_FORM_NAMES: &[(&str, SpecialForm)] = &[
    ("quote", SpecialForm::Quote),
    ("if", SpecialForm::If),
    ("cond", SpecialForm::Cond),
    ("define", SpecialForm::Define),
    ("def", SpecialForm::Define),
    ("defun", SpecialForm::Defun),
    ("defn", SpecialForm::Defun),
    ("set!", SpecialForm::Set),
    ("lambda", SpecialForm::Lambda),
    ("fn", SpecialForm::Lambda),
    ("let", SpecialForm::Let),
    ("let*", SpecialForm::LetStar),
    ("letrec", SpecialForm::Letrec),
    ("begin", SpecialForm::Begin),
    ("progn", SpecialForm::Begin),
    ("do", SpecialForm::Do),
    ("and", SpecialForm::And),
    ("or", SpecialForm::Or),
    ("when", SpecialForm::When),
    ("unless", SpecialForm::Unless),
    ("while", SpecialForm::While),
    ("defmacro", SpecialForm::Defmacro),
    ("define-syntax", SpecialForm::DefineSyntax),
    ("quasiquote", SpecialForm::Quasiquote),
    ("throw", SpecialForm::Throw),
    ("try", SpecialForm::Try),
    ("case", SpecialForm::Case),
    ("eval", SpecialForm::Eval),
    ("macroexpand", SpecialForm::Macroexpand),
    ("module", SpecialForm::Module),
    ("import", SpecialForm::Import),
    ("load", SpecialForm::Load),
    ("prompt", SpecialForm::Prompt),
    ("message", SpecialForm::Message),
    ("deftool", SpecialForm::Deftool),
    ("defagent", SpecialForm::Defagent),
    ("delay", SpecialForm::Delay),
    ("force", SpecialForm::Force),
    ("define-record-type", SpecialForm::DefineRecordType),
    ("match", SpecialForm::Match),
    ("match*", SpecialForm::MatchStar),
    ("defmulti", SpecialForm::Defmulti),
    ("defmethod", SpecialForm::Defmethod),
    ("async", SpecialForm::Async),
    ("await", SpecialForm::Await),
    ("let-values", SpecialForm::LetValues),
    ("let*-values", SpecialForm::LetStarValues),
    ("define-values", SpecialForm::DefineValues),
];

thread_local! {
    /// Per-thread cache of special-form name `Spur`s.
    ///
    /// `Spur` ids are only meaningful within the `thread_local!` interner that
    /// produced them (see `sema_core::INTERNER`), so this cache MUST be
    /// thread-local too — a process-global cache would resolve to garbage on
    /// any thread other than the one that populated it. It is built lazily on
    /// first use of each thread and reused for every subsequent lowering call,
    /// replacing ~40 interner round-trips per list form with one map lookup.
    static SPECIAL_FORMS: HashMap<Spur, SpecialForm> = {
        let mut map = HashMap::with_capacity(SPECIAL_FORM_NAMES.len());
        for &(name, form) in SPECIAL_FORM_NAMES {
            map.insert(intern(name), form);
        }
        map
    };
}

/// Resolve a head-position symbol's `Spur` to its [`SpecialForm`], if any.
fn special_form_for(spur: Spur) -> Option<SpecialForm> {
    SPECIAL_FORMS.with(|m| m.get(&spur).copied())
}

/// Whether `name` is a built-in special form. Exposed for syntax-rules hygiene:
/// a template identifier that names a special form must be kept verbatim (not
/// alpha-renamed) because special forms are recognized structurally, not via an
/// env binding.
pub fn is_special_form(name: &str) -> bool {
    SPECIAL_FORM_NAMES.iter().any(|&(n, _)| n == name)
}

fn require_symbol(val: &Value, context: &str) -> Result<Spur, SemaError> {
    val.as_symbol_spur()
        .ok_or_else(|| SemaError::eval(format!("{context}: expected a symbol")))
}

fn require_list<'a>(val: &'a Value, context: &str) -> Result<&'a [Value], SemaError> {
    val.as_list()
        .ok_or_else(|| SemaError::eval(format!("{context}: expected a list")))
}

/// Parse parameter list, handling rest params `(a b . rest)`.
fn parse_params(names: &[Spur]) -> (Vec<Spur>, Option<Spur>) {
    let dot = intern(".");
    if let Some(pos) = names.iter().position(|s| *s == dot) {
        let params = names[..pos].to_vec();
        let rest = if pos + 1 < names.len() {
            Some(names[pos + 1])
        } else {
            None
        };
        (params, rest)
    } else {
        (names.to_vec(), None)
    }
}

fn extract_param_spurs(param_list: &[Value], context: &str) -> Result<Vec<Spur>, SemaError> {
    param_list
        .iter()
        .map(|v| require_symbol(v, context))
        .collect()
}

/// Generate a unique temporary variable name.
fn gensym(prefix: &str) -> Spur {
    thread_local! {
        static COUNTER: Cell<usize> = const { Cell::new(0) };
    }
    let n = COUNTER.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    intern(&format!("__vm${prefix}${n}"))
}

/// Check if a Value is a destructuring pattern (vector or map).
fn is_destructuring_pattern(val: &Value) -> bool {
    val.as_vector().is_some() || val.as_map_ref().is_some()
}

/// Collect all variable names bound by a destructuring pattern.
fn collect_pattern_vars(pattern: &Value) -> Vec<Spur> {
    let underscore = intern("_");
    let amp = intern("&");
    let keys_kw = Value::keyword("keys");

    let mut vars = Vec::new();

    if let Some(spur) = pattern.as_symbol_spur() {
        if spur != underscore {
            vars.push(spur);
        }
    } else if let Some(elems) = pattern.as_vector() {
        for elem in elems {
            if let Some(s) = elem.as_symbol_spur() {
                if s == amp {
                    continue;
                }
            }
            vars.extend(collect_pattern_vars(elem));
        }
    } else if let Some(map) = pattern.as_map_ref() {
        if let Some(keys_val) = map.get(&keys_kw) {
            let key_names = if let Some(v) = keys_val.as_vector() {
                v.to_vec()
            } else if let Some(l) = keys_val.as_list() {
                l.to_vec()
            } else {
                vec![]
            };
            for k in &key_names {
                if let Some(s) = k.as_symbol_spur() {
                    vars.push(s);
                }
            }
        }
        for (k, v_pat) in map.iter() {
            if k == &keys_kw {
                continue;
            }
            vars.extend(collect_pattern_vars(v_pat));
        }
    }

    vars
}

/// Lower a destructuring binding: given a pattern and an init expression,
/// produce LetStar bindings that call `__vm-destructure` and extract vars.
fn lower_destructuring_bindings(
    pattern: &Value,
    init: CoreExpr,
) -> Result<Vec<(Spur, CoreExpr)>, SemaError> {
    let tmp = gensym("val");
    let map_tmp = gensym("map");
    let vars = collect_pattern_vars(pattern);
    let get_spur = intern("get");

    let mut bindings = Vec::new();
    // (define tmp init)
    bindings.push((tmp, init));
    // (define map_tmp (__vm-destructure 'pattern tmp))
    bindings.push((
        map_tmp,
        CoreExpr::Call {
            func: Box::new(CoreExpr::Var(intern("__vm-destructure"))),
            args: vec![CoreExpr::Quote(pattern.clone()), CoreExpr::Var(tmp)],
            tail: false,
        },
    ));
    // For each var: (define var (get map_tmp 'var))
    for var_spur in vars {
        bindings.push((
            var_spur,
            CoreExpr::Call {
                func: Box::new(CoreExpr::Var(get_spur)),
                args: vec![
                    CoreExpr::Var(map_tmp),
                    CoreExpr::Quote(Value::symbol_from_spur(var_spur)),
                ],
                tail: false,
            },
        ));
    }
    Ok(bindings)
}

/// Parse a binding list, supporting both symbols and destructuring patterns.
fn parse_bindings(bindings_val: &Value, context: &str) -> Result<Vec<(Spur, CoreExpr)>, SemaError> {
    let bindings_list = require_list(bindings_val, context)?;
    let mut bindings = Vec::new();
    for binding in bindings_list {
        let pair = require_list(binding, context)?;
        if pair.len() != 2 {
            return Err(SemaError::eval(format!(
                "{context}: each binding must have 2 elements"
            )));
        }
        let init = lower_expr(&pair[1], false)?;
        if let Some(name) = pair[0].as_symbol_spur() {
            bindings.push((name, init));
        } else if is_destructuring_pattern(&pair[0]) {
            bindings.extend(lower_destructuring_bindings(&pair[0], init)?);
        } else {
            return Err(SemaError::eval(format!(
                "{context}: binding name must be a symbol, vector, or map pattern"
            )));
        }
    }
    Ok(bindings)
}

// --- Special form lowering ---

fn lower_quote(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("quote", "1", args.len()));
    }
    Ok(CoreExpr::Quote(args[0].clone()))
}

fn lower_if(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 || args.len() > 3 {
        return Err(SemaError::arity("if", "2 or 3", args.len()));
    }
    let test = lower_expr(&args[0], false)?;
    let then = lower_expr(&args[1], tail)?;
    let else_ = if args.len() == 3 {
        lower_expr(&args[2], tail)?
    } else {
        CoreExpr::Const(Value::nil())
    };
    Ok(CoreExpr::If {
        test: Box::new(test),
        then: Box::new(then),
        else_: Box::new(else_),
    })
}

fn lower_cond(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    let else_spur = intern("else");
    lower_cond_clauses(args, 0, tail, else_spur)
}

fn lower_cond_clauses(
    clauses: &[Value],
    idx: usize,
    tail: bool,
    else_spur: Spur,
) -> Result<CoreExpr, SemaError> {
    if idx >= clauses.len() {
        return Ok(CoreExpr::Const(Value::nil()));
    }
    let clause = require_list(&clauses[idx], "cond")?;
    if clause.is_empty() {
        return Err(SemaError::eval("cond: clause must not be empty")
            .with_hint("each clause is (test body...) or (else body...)"));
    }

    let is_else = clause[0].as_symbol_spur().is_some_and(|s| s == else_spur);
    if is_else {
        let body = lower_body(&clause[1..], tail)?;
        return if body.is_empty() {
            Ok(CoreExpr::Const(Value::bool(true)))
        } else if body.len() == 1 {
            Ok(body.into_iter().next().unwrap())
        } else {
            Ok(CoreExpr::Begin(body))
        };
    }

    let test = lower_expr(&clause[0], false)?;
    let then = if clause.len() == 1 {
        CoreExpr::Const(Value::bool(true))
    } else {
        let body = lower_body(&clause[1..], tail)?;
        if body.len() == 1 {
            body.into_iter().next().unwrap()
        } else {
            CoreExpr::Begin(body)
        }
    };
    let else_ = lower_cond_clauses(clauses, idx + 1, tail, else_spur)?;

    Ok(CoreExpr::If {
        test: Box::new(test),
        then: Box::new(then),
        else_: Box::new(else_),
    })
}

fn lower_define(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.is_empty() {
        return Err(SemaError::arity("define", "2+", 0));
    }
    match args[0].view() {
        ValueView::Symbol(spur) => {
            if args.len() != 2 {
                return Err(SemaError::arity("define", "2", args.len()));
            }
            let val = lower_expr(&args[1], false)?;
            Ok(CoreExpr::Define(spur, Box::new(val)))
        }
        ValueView::List(sig) => {
            if sig.is_empty() {
                return Err(SemaError::eval("define: empty function signature"));
            }
            let name_spur = require_symbol(&sig[0], "define")?;
            let param_spurs = extract_param_spurs(&sig[1..], "define")?;
            let (params, rest) = parse_params(&param_spurs);
            let body = lower_body(&args[1..], true)?;
            if body.is_empty() {
                return Err(SemaError::eval("define: function body cannot be empty"));
            }
            Ok(CoreExpr::Define(
                name_spur,
                Box::new(CoreExpr::Lambda(LambdaDef {
                    name: Some(name_spur),
                    params,
                    rest,
                    body,
                    upvalues: vec![],
                    upvalue_names: vec![],
                    n_locals: 0,
                })),
            ))
        }
        _ if is_destructuring_pattern(&args[0]) => {
            // (define [a b] expr) or (define {:keys [x y]} expr)
            if args.len() != 2 {
                return Err(SemaError::arity("define", "2", args.len()));
            }
            let init = lower_expr(&args[1], false)?;
            let destr_bindings = lower_destructuring_bindings(&args[0], init)?;
            let mut defines: Vec<CoreExpr> = Vec::new();
            for (spur, expr) in destr_bindings {
                defines.push(CoreExpr::Define(spur, Box::new(expr)));
            }
            Ok(CoreExpr::Begin(defines))
        }
        _ => Err(SemaError::type_error(
            "symbol, list, vector, or map",
            args[0].type_name(),
        )),
    }
}

fn lower_defun(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() < 3 {
        return Err(SemaError::arity("defun", "3+", args.len()));
    }
    let name_spur = require_symbol(&args[0], "defun")?;
    let param_list = require_list(&args[1], "defun")?;
    let param_spurs = extract_param_spurs(param_list, "defun")?;
    let (params, rest) = parse_params(&param_spurs);
    let body = lower_body(&args[2..], true)?;
    Ok(CoreExpr::Define(
        name_spur,
        Box::new(CoreExpr::Lambda(LambdaDef {
            name: Some(name_spur),
            params,
            rest,
            body,
            upvalues: vec![],
            upvalue_names: vec![],
            n_locals: 0,
        })),
    ))
}

fn lower_set(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 2 {
        return Err(SemaError::arity("set!", "2", args.len()));
    }
    let spur = require_symbol(&args[0], "set!")?;
    let val = lower_expr(&args[1], false)?;
    Ok(CoreExpr::Set(spur, Box::new(val)))
}

fn lower_lambda(args: &[Value], name: Option<Spur>) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("lambda", "2+", args.len()));
    }
    let param_vals = match args[0].view() {
        ValueView::List(params) => params.as_ref().clone(),
        ValueView::Vector(params) => params.as_ref().clone(),
        _ => return Err(SemaError::type_error("list or vector", args[0].type_name())),
    };

    let dot = intern(".");
    let needs_destructuring = param_vals
        .iter()
        .any(|p| p.as_symbol_spur() != Some(dot) && is_destructuring_pattern(p));

    if needs_destructuring {
        // Desugar: generate temp param names, wrap body in let*
        let mut temp_spurs = Vec::new();
        let mut let_bindings = Vec::new();
        let mut hit_dot = false;
        let mut rest_spur = None;

        for (idx, p) in param_vals.iter().enumerate() {
            if let Some(s) = p.as_symbol_spur() {
                if s == dot {
                    hit_dot = true;
                    continue;
                }
                if hit_dot {
                    rest_spur = Some(s);
                    continue;
                }
                temp_spurs.push(s);
            } else {
                let tmp = gensym(&format!("arg{idx}"));
                temp_spurs.push(tmp);
                let destr = lower_destructuring_bindings(p, CoreExpr::Var(tmp))?;
                let_bindings.extend(destr);
            }
        }

        let orig_body = lower_body(&args[1..], true)?;
        let body = if let_bindings.is_empty() {
            orig_body
        } else {
            vec![CoreExpr::LetStar {
                bindings: let_bindings,
                body: orig_body,
            }]
        };

        Ok(CoreExpr::Lambda(LambdaDef {
            name,
            params: temp_spurs,
            rest: rest_spur,
            body,
            upvalues: vec![],
            upvalue_names: vec![],
            n_locals: 0,
        }))
    } else {
        // Fast path: all params are symbols
        let param_spurs = extract_param_spurs(&param_vals, "lambda")?;
        let (params, rest) = parse_params(&param_spurs);
        let body = lower_body(&args[1..], true)?;
        Ok(CoreExpr::Lambda(LambdaDef {
            name,
            params,
            rest,
            body,
            upvalues: vec![],
            upvalue_names: vec![],
            n_locals: 0,
        }))
    }
}

fn lower_let(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("let", "2+", args.len()));
    }

    // Named let: (let name ((var init) ...) body...)
    if let Some(loop_name) = args[0].as_symbol_spur() {
        if args.len() < 3 {
            return Err(SemaError::arity("named let", "3+", args.len()));
        }
        let bindings_list = require_list(&args[1], "named let")?;
        let mut bindings = Vec::new();
        for binding in bindings_list {
            let pair = require_list(binding, "named let")?;
            if pair.len() != 2 {
                return Err(SemaError::eval(
                    "named let: each binding must have 2 elements",
                ));
            }
            let name = require_symbol(&pair[0], "named let")?;
            let init = lower_expr(&pair[1], false)?;
            bindings.push((name, init));
        }
        // The body goes into a lambda, so it's always in tail position
        // (the last expression in a lambda body is a tail call).
        let body = lower_body(&args[2..], true)?;
        let (params, inits): (Vec<Spur>, Vec<CoreExpr>) = bindings.into_iter().unzip();
        return Ok(CoreExpr::Letrec {
            bindings: vec![(
                loop_name,
                CoreExpr::Lambda(LambdaDef {
                    name: Some(loop_name),
                    params,
                    rest: None,
                    body,
                    upvalues: vec![],
                    upvalue_names: vec![],
                    n_locals: 0,
                }),
            )],
            body: vec![CoreExpr::Call {
                func: Box::new(CoreExpr::Var(loop_name)),
                args: inits,
                tail,
            }],
        });
    }

    // Regular let — check if any binding uses destructuring
    let bindings_list = require_list(&args[0], "let")?;
    let has_destructuring = bindings_list.iter().any(|b| {
        b.as_list()
            .map(|pair| !pair.is_empty() && is_destructuring_pattern(&pair[0]))
            .unwrap_or(false)
    });

    if has_destructuring {
        // For `let` semantics: evaluate ALL inits in the outer env first (parallel),
        // then destructure sequentially. Split into two phases:
        // Phase 1: (let ((tmp1 init1) (tmp2 init2) ...) ...)  — parallel eval
        // Phase 2: (let* ((destr-bindings-from-tmp1) (destr-bindings-from-tmp2) ...) body)
        let mut parallel_bindings = Vec::new();
        let mut sequential_bindings = Vec::new();

        for binding in bindings_list {
            let pair = require_list(binding, "let")?;
            if pair.len() != 2 {
                return Err(SemaError::eval("let: each binding must have 2 elements"));
            }
            let init = lower_expr(&pair[1], false)?;
            if let Some(name) = pair[0].as_symbol_spur() {
                // Simple binding: goes into both phases (parallel eval, then visible)
                let tmp = gensym("let");
                parallel_bindings.push((tmp, init));
                sequential_bindings.push((name, CoreExpr::Var(tmp)));
            } else if is_destructuring_pattern(&pair[0]) {
                // Destructuring: eval init in parallel, destructure in sequential phase
                let tmp = gensym("let");
                parallel_bindings.push((tmp, init));
                let destr = lower_destructuring_bindings(&pair[0], CoreExpr::Var(tmp))?;
                // Skip the first binding (val tmp — redundant since we already have tmp)
                // The first binding is (val_tmp, Var(tmp)), second is (map_tmp, call destructure)
                // We need all bindings from the map_tmp onwards, but val_tmp references our tmp
                sequential_bindings.extend(destr);
            } else {
                return Err(SemaError::eval(
                    "let: binding name must be a symbol, vector, or map pattern",
                ));
            }
        }

        let body = lower_body(&args[1..], tail)?;
        Ok(CoreExpr::Let {
            bindings: parallel_bindings,
            body: vec![CoreExpr::LetStar {
                bindings: sequential_bindings,
                body,
            }],
        })
    } else {
        let bindings = parse_bindings(&args[0], "let")?;
        let body = lower_body(&args[1..], tail)?;
        Ok(CoreExpr::Let { bindings, body })
    }
}

fn lower_let_star(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("let*", "2+", args.len()));
    }
    let bindings = parse_bindings(&args[0], "let*")?;
    let body = lower_body(&args[1..], tail)?;
    Ok(CoreExpr::LetStar { bindings, body })
}

fn lower_letrec(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("letrec", "2+", args.len()));
    }
    let bindings = parse_bindings(&args[0], "letrec")?;
    let body = lower_body(&args[1..], tail)?;
    Ok(CoreExpr::Letrec { bindings, body })
}

fn lower_begin(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.is_empty() {
        return Ok(CoreExpr::Const(Value::nil()));
    }
    let body = lower_body(args, tail)?;
    if body.len() == 1 {
        Ok(body.into_iter().next().unwrap())
    } else {
        Ok(CoreExpr::Begin(body))
    }
}

fn lower_and(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.is_empty() {
        return Ok(CoreExpr::Const(Value::bool(true)));
    }
    let exprs = lower_body(args, tail)?;
    if exprs.len() == 1 {
        Ok(exprs.into_iter().next().unwrap())
    } else {
        Ok(CoreExpr::And(exprs))
    }
}

fn lower_or(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.is_empty() {
        return Ok(CoreExpr::Const(Value::bool(false)));
    }
    let exprs = lower_body(args, tail)?;
    if exprs.len() == 1 {
        Ok(exprs.into_iter().next().unwrap())
    } else {
        Ok(CoreExpr::Or(exprs))
    }
}

fn lower_when(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("when", "2+", args.len()));
    }
    let test = lower_expr(&args[0], false)?;
    let body = lower_body(&args[1..], tail)?;
    let then = if body.len() == 1 {
        body.into_iter().next().unwrap()
    } else {
        CoreExpr::Begin(body)
    };
    Ok(CoreExpr::If {
        test: Box::new(test),
        then: Box::new(then),
        else_: Box::new(CoreExpr::Const(Value::nil())),
    })
}

fn lower_unless(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("unless", "2+", args.len()));
    }
    let test = lower_expr(&args[0], false)?;
    let body = lower_body(&args[1..], tail)?;
    let else_ = if body.len() == 1 {
        body.into_iter().next().unwrap()
    } else {
        CoreExpr::Begin(body)
    };
    Ok(CoreExpr::If {
        test: Box::new(test),
        then: Box::new(CoreExpr::Const(Value::nil())),
        else_: Box::new(else_),
    })
}

fn lower_while(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("while", "2+", args.len()));
    }
    // Desugar (while test body...) into (do () ((not test)) body...)
    let test = CoreExpr::Call {
        func: Box::new(CoreExpr::Var(intern("not"))),
        args: vec![lower_expr(&args[0], false)?],
        tail: false,
    };
    let body = lower_body(&args[1..], false)?;
    Ok(CoreExpr::Do(DoLoop {
        vars: vec![],
        test: Box::new(test),
        result: vec![CoreExpr::Const(Value::nil())],
        body,
    }))
}

fn lower_defmacro(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() < 3 {
        return Err(SemaError::arity("defmacro", "3+", args.len()));
    }
    // Delegate defmacro entirely to the tree-walker: reconstruct the original
    // form and pass it quoted to __vm-defmacro-form so the body stays unevaluated.
    let mut form = vec![Value::symbol("defmacro")];
    form.extend(args.iter().cloned());
    Ok(CoreExpr::Call {
        func: Box::new(CoreExpr::Var(intern("__vm-defmacro-form"))),
        args: vec![CoreExpr::Const(Value::list(form))],
        tail: false,
    })
}

fn lower_define_syntax(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 2 {
        return Err(SemaError::arity("define-syntax", "2", args.len()));
    }
    // Delegate to the eval-side registrar: reconstruct the original form and
    // pass it quoted to __vm-define-syntax so the transformer stays unevaluated.
    let mut form = vec![Value::symbol("define-syntax")];
    form.extend(args.iter().cloned());
    Ok(CoreExpr::Call {
        func: Box::new(CoreExpr::Var(intern("__vm-define-syntax"))),
        args: vec![CoreExpr::Const(Value::list(form))],
        tail: false,
    })
}

fn lower_defmulti(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 2 {
        return Err(SemaError::arity("defmulti", "2", args.len()));
    }
    let name = require_symbol(&args[0], "defmulti")?;
    let dispatch_fn = lower_expr(&args[1], false)?;
    // (define name (__vm-make-multi name dispatch-fn))
    Ok(CoreExpr::Define(
        name,
        Box::new(CoreExpr::Call {
            func: Box::new(CoreExpr::Var(intern("__vm-make-multi"))),
            args: vec![CoreExpr::Const(Value::symbol_from_spur(name)), dispatch_fn],
            tail: false,
        }),
    ))
}

fn lower_defmethod(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 3 {
        return Err(SemaError::arity("defmethod", "3", args.len()));
    }
    let name = require_symbol(&args[0], "defmethod")?;
    let dispatch_val = lower_expr(&args[1], false)?;
    let handler = lower_expr(&args[2], false)?;
    // (__vm-defmethod multi dispatch-val handler)
    Ok(CoreExpr::Call {
        func: Box::new(CoreExpr::Var(intern("__vm-defmethod"))),
        args: vec![CoreExpr::Var(name), dispatch_val, handler],
        tail: false,
    })
}

/// (async body ...) → wrap body in zero-arg lambda, call async/spawn
fn lower_async(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.is_empty() {
        return Err(SemaError::arity("async", "1+", 0));
    }
    let body = args
        .iter()
        .map(|a| lower_expr(a, false))
        .collect::<Result<Vec<_>, _>>()?;
    let thunk = CoreExpr::Lambda(LambdaDef {
        name: None,
        params: vec![],
        rest: None,
        body,
        upvalues: vec![],
        upvalue_names: vec![],
        n_locals: 0,
    });
    Ok(CoreExpr::Call {
        func: Box::new(CoreExpr::Var(intern("async/spawn"))),
        args: vec![thunk],
        tail: false,
    })
}

/// (await expr) → call async/await with the expression
fn lower_await(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("await", "1", args.len()));
    }
    let expr = lower_expr(&args[0], false)?;
    Ok(CoreExpr::Call {
        func: Box::new(CoreExpr::Var(intern("async/await"))),
        args: vec![expr],
        tail: false,
    })
}

/// Parse a `values`-binding formals spec: either a bare symbol (binds ALL
/// produced values as a list, R7RS's `(formals producer)` with `formals` a
/// single identifier) or a `(a b . rest)` list handled by [`parse_params`].
fn parse_values_formals(
    formals: &Value,
    context: &str,
) -> Result<(Vec<Spur>, Option<Spur>), SemaError> {
    if let Some(sym) = formals.as_symbol_spur() {
        return Ok((vec![], Some(sym)));
    }
    let param_list = require_list(formals, context)?;
    let param_spurs = extract_param_spurs(param_list, context)?;
    Ok(parse_params(&param_spurs))
}

/// Build a zero-arg thunk lambda around a lowered producer expression, mirroring
/// `lower_async`'s thunk-wrapping. `call-with-values` calls this with no
/// arguments and inspects its result.
fn make_producer_thunk(producer: &Value) -> Result<CoreExpr, SemaError> {
    Ok(CoreExpr::Lambda(LambdaDef {
        name: None,
        params: vec![],
        rest: None,
        body: vec![lower_expr(producer, true)?],
        upvalues: vec![],
        upvalue_names: vec![],
        n_locals: 0,
    }))
}

/// A parsed `(formals producer)` clause shared by `let-values`/`let*-values`.
struct ValuesClause {
    params: Vec<Spur>,
    rest: Option<Spur>,
    producer: Value,
}

fn parse_values_clauses(
    bindings_val: &Value,
    context: &str,
) -> Result<Vec<ValuesClause>, SemaError> {
    let clauses = require_list(bindings_val, context)?;
    clauses
        .iter()
        .map(|clause| {
            let pair = require_list(clause, context)?;
            if pair.len() != 2 {
                return Err(SemaError::eval(format!(
                    "{context}: each clause must be (formals producer)"
                )));
            }
            let (params, rest) = parse_values_formals(&pair[0], context)?;
            Ok(ValuesClause {
                params,
                rest,
                producer: pair[1].clone(),
            })
        })
        .collect()
}

/// `(let-values (((a b) (values 1 2)) ...) body ...)` — R7RS parallel binding:
/// every producer runs in the OUTER environment before any clause's formals
/// come into scope (so clause N cannot see clause N-1's bindings).
///
/// Desugars to `(let ((tmp0 (call-with-values thunk0 list)) ...) (apply
/// (lambda formals0 (apply (lambda formals1 body...) tmp1)) tmp0))` — each
/// `tmp_i` is the full list of values clause `i` produced (list-of-length-1 for
/// an ordinary single value, since `call-with-values` spreads whatever
/// `values` returned into `list`'s args), and `apply` rebinds `formals_i`
/// against it, so arity mismatches surface as the normal lambda-arity error.
fn lower_let_values(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("let-values", "2+", args.len()));
    }
    let clauses = parse_values_clauses(&args[0], "let-values")?;
    let body = &args[1..];

    let mut outer_bindings = Vec::with_capacity(clauses.len());
    let mut temps = Vec::with_capacity(clauses.len());
    for clause in &clauses {
        let tmp = gensym("lv");
        let thunk = make_producer_thunk(&clause.producer)?;
        let call = CoreExpr::Call {
            func: Box::new(CoreExpr::Var(intern("call-with-values"))),
            args: vec![thunk, CoreExpr::Var(intern("list"))],
            tail: false,
        };
        outer_bindings.push((tmp, call));
        temps.push(tmp);
    }

    let inner_tail = if clauses.is_empty() { tail } else { true };
    let mut current = lower_body(body, inner_tail)?;
    for (i, clause) in clauses.iter().enumerate().rev() {
        let consumer = CoreExpr::Lambda(LambdaDef {
            name: None,
            params: clause.params.clone(),
            rest: clause.rest,
            body: current,
            upvalues: vec![],
            upvalue_names: vec![],
            n_locals: 0,
        });
        let is_outermost = i == 0;
        current = vec![CoreExpr::Call {
            func: Box::new(CoreExpr::Var(intern("apply"))),
            args: vec![consumer, CoreExpr::Var(temps[i])],
            tail: if is_outermost { tail } else { true },
        }];
    }

    Ok(CoreExpr::Let {
        bindings: outer_bindings,
        body: current,
    })
}

/// `(let*-values (((a b) (values 1 2)) ((c) (values (+ a b)))) c)` — R7RS
/// sequential binding: each producer sees the PRIOR clauses' bindings (unlike
/// `let-values`). Nests directly: `(call-with-values thunk0 (lambda formals0
/// (call-with-values thunk1 (lambda formals1 body...))))` — no intermediate
/// list/apply indirection needed since sequential scoping falls out of the
/// natural nesting of each consumer lambda inside the previous one.
fn lower_let_star_values(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("let*-values", "2+", args.len()));
    }
    let clauses = parse_values_clauses(&args[0], "let*-values")?;
    let body = &args[1..];

    let inner_tail = if clauses.is_empty() { tail } else { true };
    let mut current = lower_body(body, inner_tail)?;
    for (i, clause) in clauses.iter().enumerate().rev() {
        let consumer = CoreExpr::Lambda(LambdaDef {
            name: None,
            params: clause.params.clone(),
            rest: clause.rest,
            body: current,
            upvalues: vec![],
            upvalue_names: vec![],
            n_locals: 0,
        });
        let thunk = make_producer_thunk(&clause.producer)?;
        let is_outermost = i == 0;
        current = vec![CoreExpr::Call {
            func: Box::new(CoreExpr::Var(intern("call-with-values"))),
            args: vec![thunk, consumer],
            tail: if is_outermost { tail } else { true },
        }];
    }

    if current.len() == 1 {
        Ok(current.into_iter().next().unwrap())
    } else {
        Ok(CoreExpr::Begin(current))
    }
}

/// `(define-values (a b) (values 1 2))` / `(define-values (q . r) (values 1 2 3))`
/// desugars to a `begin` of plain `define`s: bundle the producer's values into a
/// gensym'd temp list via `call-with-values`/`list`, then `nth`/`drop` it apart —
/// mirroring how `(define [a b] expr)` desugars to a `Begin` of `Define`s.
///
/// Because `nth`/`drop` are lenient (they ignore surplus elements and only error
/// with a confusing out-of-bounds message when short), the raw list-splitting
/// would silently accept a wrong value count. R7RS 5.3.3 matches `formals`
/// against the produced values exactly like a lambda's parameters, so we first
/// `apply` an otherwise-inert lambda with the SAME formals to the value list:
/// that reuses the standard lambda-arity check and raises the normal
/// `expects N` error on any count mismatch (a `rest` formal makes it accept the
/// surplus, matching lambda semantics), before the (now guaranteed in-range)
/// `nth`/`drop` binds each name.
fn lower_define_values(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 2 {
        return Err(SemaError::arity("define-values", "2", args.len()));
    }
    let (params, rest) = parse_values_formals(&args[0], "define-values")?;
    let thunk = make_producer_thunk(&args[1])?;
    let tmp = gensym("dv");

    let mut defines = Vec::with_capacity(params.len() + 3);
    defines.push(CoreExpr::Define(
        tmp,
        Box::new(CoreExpr::Call {
            func: Box::new(CoreExpr::Var(intern("call-with-values"))),
            args: vec![thunk, CoreExpr::Var(intern("list"))],
            tail: false,
        }),
    ));
    // Arity check: apply a formals-shaped inert lambda to the value list so a
    // wrong value count surfaces as the normal lambda-arity error.
    defines.push(CoreExpr::Call {
        func: Box::new(CoreExpr::Var(intern("apply"))),
        args: vec![
            CoreExpr::Lambda(LambdaDef {
                name: None,
                params: params.clone(),
                rest,
                body: vec![CoreExpr::Const(Value::nil())],
                upvalues: vec![],
                upvalue_names: vec![],
                n_locals: 0,
            }),
            CoreExpr::Var(tmp),
        ],
        tail: false,
    });
    for (i, param) in params.iter().enumerate() {
        defines.push(CoreExpr::Define(
            *param,
            Box::new(CoreExpr::Call {
                func: Box::new(CoreExpr::Var(intern("nth"))),
                args: vec![CoreExpr::Var(tmp), CoreExpr::Const(Value::int(i as i64))],
                tail: false,
            }),
        ));
    }
    if let Some(rest_name) = rest {
        defines.push(CoreExpr::Define(
            rest_name,
            Box::new(CoreExpr::Call {
                func: Box::new(CoreExpr::Var(intern("drop"))),
                args: vec![
                    CoreExpr::Const(Value::int(params.len() as i64)),
                    CoreExpr::Var(tmp),
                ],
                tail: false,
            }),
        ));
    }

    Ok(CoreExpr::Begin(defines))
}

fn is_auto_gensym(sym: &str) -> bool {
    sym.len() > 1 && sym.ends_with('#') && !sym.ends_with("##")
}

fn lower_quasiquote(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("quasiquote", "1", args.len()));
    }
    let mut gensym_map: HashMap<String, String> = HashMap::new();
    expand_quasiquote(&args[0], &mut gensym_map)
}

fn expand_quasiquote(
    val: &Value,
    gensym_map: &mut HashMap<String, String>,
) -> Result<CoreExpr, SemaError> {
    // Auto-gensym: replace foo# with a consistent gensym within this quasiquote
    if let Some(sym) = val.as_symbol() {
        if is_auto_gensym(&sym) {
            let prefix = &sym[..sym.len() - 1];
            let resolved = gensym_map
                .entry(sym.to_string())
                .or_insert_with(|| sema_core::next_gensym(prefix))
                .clone();
            return Ok(CoreExpr::Quote(Value::symbol(&resolved)));
        }
    }

    match val.view() {
        ValueView::List(items) => {
            if items.is_empty() {
                return Ok(CoreExpr::Quote(val.clone()));
            }
            // Check for (unquote x)
            if let Some(sym) = items[0].as_symbol() {
                if sym == "unquote" {
                    if items.len() != 2 {
                        return Err(SemaError::arity("unquote", "1", items.len() - 1));
                    }
                    return lower_expr(&items[1], false);
                }
            }
            expand_quasiquote_seq(&items, gensym_map, false)
        }
        ValueView::Vector(items) => {
            // Same splicing semantics as lists (EVAL-1): `[1 ,@xs 2]` must splice.
            expand_quasiquote_seq(&items, gensym_map, true)
        }
        ValueView::Map(map) => {
            // Honor unquotes inside map keys/values (EVAL-2). Splicing into a map
            // isn't meaningful, so only `(unquote x)` is handled (via recursion);
            // a top-level `(unquote-splicing ...)` key/value errors clearly rather
            // than leaking literal `(unquote-splicing ...)` data.
            let entries = map
                .iter()
                .map(|(k, v)| {
                    reject_splice_in_map(k)?;
                    reject_splice_in_map(v)?;
                    Ok((
                        expand_quasiquote(k, gensym_map)?,
                        expand_quasiquote(v, gensym_map)?,
                    ))
                })
                .collect::<Result<Vec<_>, SemaError>>()?;
            Ok(CoreExpr::MakeMap(entries))
        }
        _ => Ok(CoreExpr::Quote(val.clone())),
    }
}

/// Error if `val` is a top-level `(unquote-splicing ...)` form. Used to guard
/// map keys/values inside quasiquote, where splicing has no meaning (EVAL-2).
fn reject_splice_in_map(val: &Value) -> Result<(), SemaError> {
    if let Some(items) = val.as_list() {
        if let Some(sym) = items.first().and_then(|v| v.as_symbol()) {
            if sym == "unquote-splicing" {
                return Err(SemaError::eval(
                    "unquote-splicing is not allowed in a quasiquoted map key or value",
                )
                .with_hint("splicing only makes sense inside a list or vector"));
            }
        }
    }
    Ok(())
}

/// Expand a quasiquote sequence (list or vector elements), handling
/// `unquote-splicing`. When `as_vector` is set the result is converted to a
/// vector (via `list->vector`) so vectors splice the same way lists do.
fn expand_quasiquote_seq(
    items: &[Value],
    gensym_map: &mut HashMap<String, String>,
    as_vector: bool,
) -> Result<CoreExpr, SemaError> {
    let has_splice = items.iter().any(|item| {
        item.as_list()
            .and_then(|inner| inner.first().and_then(|h| h.as_symbol()))
            .is_some_and(|sym| sym == "unquote-splicing")
    });

    if !has_splice {
        let exprs = items
            .iter()
            .map(|item| expand_quasiquote(item, gensym_map))
            .collect::<Result<_, _>>()?;
        return Ok(if as_vector {
            CoreExpr::MakeVector(exprs)
        } else {
            CoreExpr::MakeList(exprs)
        });
    }

    // Build using append: collect segments, splice where needed.
    let mut segments: Vec<CoreExpr> = Vec::new();
    let mut current_list: Vec<CoreExpr> = Vec::new();
    for item in items.iter() {
        if let Some(inner) = item.as_list() {
            if !inner.is_empty() {
                if let Some(sym) = inner[0].as_symbol() {
                    if sym == "unquote-splicing" {
                        if inner.len() != 2 {
                            return Err(SemaError::arity("unquote-splicing", "1", inner.len() - 1));
                        }
                        if !current_list.is_empty() {
                            segments.push(CoreExpr::MakeList(std::mem::take(&mut current_list)));
                        }
                        segments.push(lower_expr(&inner[1], false)?);
                        continue;
                    }
                }
            }
        }
        current_list.push(expand_quasiquote(item, gensym_map)?);
    }
    if !current_list.is_empty() {
        segments.push(CoreExpr::MakeList(current_list));
    }

    let list_expr = if segments.len() == 1 {
        segments.into_iter().next().unwrap()
    } else {
        CoreExpr::Call {
            func: Box::new(CoreExpr::Var(intern("append"))),
            args: segments,
            tail: false,
        }
    };

    if as_vector {
        Ok(CoreExpr::Call {
            func: Box::new(CoreExpr::Var(intern("list->vector"))),
            args: vec![list_expr],
            tail: false,
        })
    } else {
        Ok(list_expr)
    }
}

fn lower_throw(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("throw", "1", args.len()));
    }
    Ok(CoreExpr::Throw(Box::new(lower_expr(&args[0], false)?)))
}

fn lower_try(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.is_empty() {
        return Err(SemaError::arity("try", "1+", 0));
    }
    let catch_spur = intern("catch");
    let last = &args[args.len() - 1];
    let catch_form = require_list(last, "try")?;
    if catch_form.is_empty() {
        return Err(SemaError::eval("try: catch form is empty"));
    }
    let is_catch = catch_form[0]
        .as_symbol_spur()
        .is_some_and(|s| s == catch_spur);
    if !is_catch {
        return Err(SemaError::eval(
            "try: last argument must be (catch var handler...)",
        ));
    }
    if catch_form.len() < 3 {
        return Err(SemaError::eval("try: catch needs (catch var handler...)"));
    }
    let catch_var = require_symbol(&catch_form[1], "try catch")?;
    let handler = lower_body(&catch_form[2..], tail)?;
    // Body is NOT tail position (handler must be reachable)
    let body = lower_body(&args[..args.len() - 1], false)?;
    Ok(CoreExpr::Try {
        body,
        catch_var,
        handler,
    })
}

fn lower_case(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("case", "2+", args.len()));
    }
    let key = lower_expr(&args[0], false)?;
    // Bind the key to a temp variable so we only evaluate it once
    let tmp = intern("__case_key__");
    let body = lower_case_clauses(&args[1..], tmp, tail)?;
    Ok(CoreExpr::Let {
        bindings: vec![(tmp, key)],
        body: vec![body],
    })
}

fn lower_case_clauses(clauses: &[Value], key_var: Spur, tail: bool) -> Result<CoreExpr, SemaError> {
    let else_spur = intern("else");
    if clauses.is_empty() {
        return Ok(CoreExpr::Const(Value::nil()));
    }
    let clause = require_list(&clauses[0], "case")?;
    if clause.is_empty() {
        return Err(SemaError::eval("case: clause must not be empty"));
    }

    let is_else = clause[0].as_symbol_spur().is_some_and(|s| s == else_spur);
    if is_else {
        let body = lower_body(&clause[1..], tail)?;
        return if body.is_empty() {
            Ok(CoreExpr::Const(Value::nil()))
        } else if body.len() == 1 {
            Ok(body.into_iter().next().unwrap())
        } else {
            Ok(CoreExpr::Begin(body))
        };
    }

    // ((datum ...) body...)
    let datums = require_list(&clause[0], "case")?;
    // Build equality tests: (or (= key d1) (= key d2) ...)
    let eq_spur = intern("=");
    let tests: Vec<CoreExpr> = datums
        .iter()
        .map(|datum| {
            Ok(CoreExpr::Call {
                func: Box::new(CoreExpr::Var(eq_spur)),
                args: vec![CoreExpr::Var(key_var), CoreExpr::Quote(datum.clone())],
                tail: false,
            })
        })
        .collect::<Result<_, SemaError>>()?;

    let test = if tests.len() == 1 {
        tests.into_iter().next().unwrap()
    } else {
        CoreExpr::Or(tests)
    };

    let then_body = lower_body(&clause[1..], tail)?;
    let then = if then_body.is_empty() {
        CoreExpr::Const(Value::nil())
    } else if then_body.len() == 1 {
        then_body.into_iter().next().unwrap()
    } else {
        CoreExpr::Begin(then_body)
    };

    let else_ = lower_case_clauses(&clauses[1..], key_var, tail)?;

    Ok(CoreExpr::If {
        test: Box::new(test),
        then: Box::new(then),
        else_: Box::new(else_),
    })
}

/// Lower `(match expr [pattern body...] [pattern when guard body...] ...)`
/// into nested if/let* chains calling `__vm-try-match`.
/// `lenient` selects the no-match behavior: strict `match` (false) raises
/// `:match-failed` when no clause matches, while `match*` (true) returns nil.
fn lower_match(args: &[Value], tail: bool, lenient: bool) -> Result<CoreExpr, SemaError> {
    let form = if lenient { "match*" } else { "match" };
    if args.len() < 2 {
        return Err(SemaError::arity(form, "2+", args.len()));
    }
    let scrut = lower_expr(&args[0], false)?;
    let scrut_tmp = gensym("scrut");
    let try_match_spur = intern("__vm-try-match");
    let match_failed_spur = intern("__vm-match-failed");
    let get_spur = intern("get");
    let nil_q_spur = intern("nil?");
    let when_spur = intern("when");

    let body = lower_match_clauses(
        &args[1..],
        scrut_tmp,
        try_match_spur,
        match_failed_spur,
        get_spur,
        nil_q_spur,
        when_spur,
        tail,
        lenient,
    )?;

    Ok(CoreExpr::Let {
        bindings: vec![(scrut_tmp, scrut)],
        body: vec![body],
    })
}

#[allow(clippy::too_many_arguments)]
fn lower_match_clauses(
    clauses: &[Value],
    scrut_var: Spur,
    try_match_spur: Spur,
    match_failed_spur: Spur,
    get_spur: Spur,
    nil_q_spur: Spur,
    when_spur: Spur,
    tail: bool,
    lenient: bool,
) -> Result<CoreExpr, SemaError> {
    if clauses.is_empty() {
        // No clause matched. `match*` is lenient (nil); `match` raises via the
        // `__vm-match-failed` helper, which carries the unmatched value.
        if lenient {
            return Ok(CoreExpr::Const(Value::nil()));
        }
        return Ok(CoreExpr::Call {
            func: Box::new(CoreExpr::Var(match_failed_spur)),
            args: vec![CoreExpr::Var(scrut_var)],
            tail: false,
        });
    }

    let clause = if let Some(l) = clauses[0].as_list() {
        l
    } else if let Some(v) = clauses[0].as_vector() {
        v
    } else {
        return Err(
            SemaError::eval("match: each clause must be a list or vector")
                .with_hint("e.g. (match x (1 \"one\") (_ \"other\"))"),
        );
    };

    if clause.is_empty() {
        return Err(SemaError::eval("match: clause must not be empty"));
    }

    let pattern = &clause[0];

    // Check for guard: [pattern when guard body...]
    let (has_guard, guard_idx) = if clause.len() >= 3 {
        if let Some(s) = clause[1].as_symbol_spur() {
            if s == when_spur {
                (true, 2)
            } else {
                (false, 0)
            }
        } else {
            (false, 0)
        }
    } else {
        (false, 0)
    };

    let body_start = if has_guard { guard_idx + 1 } else { 1 };
    let map_tmp = gensym("match");
    let vars = collect_pattern_vars(pattern);

    // Build: (__vm-try-match 'pattern scrut_var)
    let try_call = CoreExpr::Call {
        func: Box::new(CoreExpr::Var(try_match_spur)),
        args: vec![CoreExpr::Quote(pattern.clone()), CoreExpr::Var(scrut_var)],
        tail: false,
    };

    // Build the var extraction bindings: (get map 'var) for each var
    let mut var_bindings = Vec::new();
    for var_spur in &vars {
        var_bindings.push((
            *var_spur,
            CoreExpr::Call {
                func: Box::new(CoreExpr::Var(get_spur)),
                args: vec![
                    CoreExpr::Var(map_tmp),
                    CoreExpr::Quote(Value::symbol_from_spur(*var_spur)),
                ],
                tail: false,
            },
        ));
    }

    // Build the body
    let clause_body = if body_start >= clause.len() {
        vec![CoreExpr::Const(Value::nil())]
    } else {
        lower_body(&clause[body_start..], tail)?
    };

    // Wrap body in let* to bind extracted vars
    let then_expr = if var_bindings.is_empty() {
        if clause_body.len() == 1 {
            clause_body.into_iter().next().unwrap()
        } else {
            CoreExpr::Begin(clause_body)
        }
    } else {
        CoreExpr::LetStar {
            bindings: var_bindings.clone(),
            body: clause_body,
        }
    };

    // If guard present, wrap in additional check
    let then_with_guard = if has_guard {
        let guard = lower_expr(&clause[guard_idx], false)?;
        // If guard fails, fall through to remaining clauses
        let else_clauses = lower_match_clauses(
            &clauses[1..],
            scrut_var,
            try_match_spur,
            match_failed_spur,
            get_spur,
            nil_q_spur,
            when_spur,
            tail,
            lenient,
        )?;
        // Need var bindings available for guard eval too
        let guard_body = CoreExpr::If {
            test: Box::new(guard),
            then: Box::new(then_expr),
            else_: Box::new(else_clauses),
        };
        if var_bindings.is_empty() {
            guard_body
        } else {
            CoreExpr::LetStar {
                bindings: var_bindings,
                body: vec![guard_body],
            }
        }
    } else {
        then_expr
    };

    // Else: try remaining clauses (always needed — pattern may fail even with guard)
    let else_expr = lower_match_clauses(
        &clauses[1..],
        scrut_var,
        try_match_spur,
        match_failed_spur,
        get_spur,
        nil_q_spur,
        when_spur,
        tail,
        lenient,
    )?;

    // Build: (let ((map_tmp (try-match ...))) (if (nil? map_tmp) else then))
    let test = CoreExpr::Call {
        func: Box::new(CoreExpr::Var(nil_q_spur)),
        args: vec![CoreExpr::Var(map_tmp)],
        tail: false,
    };

    let if_expr = CoreExpr::If {
        test: Box::new(test),
        then: Box::new(else_expr),
        else_: Box::new(then_with_guard),
    };

    Ok(CoreExpr::Let {
        bindings: vec![(map_tmp, try_call)],
        body: vec![if_expr],
    })
}

fn lower_do(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("do", "2+", args.len()));
    }
    let bindings = require_list(&args[0], "do")?;
    let test_clause = require_list(&args[1], "do")?;
    if test_clause.is_empty() {
        return Err(SemaError::eval("do: test clause must not be empty"));
    }
    let body_vals = &args[2..];

    let mut vars = Vec::new();
    for binding in bindings {
        let parts = require_list(binding, "do")?;
        if parts.len() < 2 || parts.len() > 3 {
            return Err(SemaError::eval(
                "do: binding must be (var init) or (var init step)",
            ));
        }
        let name = require_symbol(&parts[0], "do")?;
        let init = lower_expr(&parts[1], false)?;
        let step = if parts.len() == 3 {
            Some(lower_expr(&parts[2], false)?)
        } else {
            None
        };
        vars.push(DoVar { name, init, step });
    }

    let test = lower_expr(&test_clause[0], false)?;
    // Result exprs: last is tail position
    let result = lower_body(&test_clause[1..], tail)?;
    // Loop body: NOT tail position
    let body = lower_body(body_vals, false)?;

    Ok(CoreExpr::Do(DoLoop {
        vars,
        test: Box::new(test),
        result,
        body,
    }))
}

fn lower_eval(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("eval", "1", args.len()));
    }
    Ok(CoreExpr::Eval(Box::new(lower_expr(&args[0], false)?)))
}

fn lower_macroexpand(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("macroexpand", "1", args.len()));
    }
    Ok(CoreExpr::Macroexpand(Box::new(lower_expr(
        &args[0], false,
    )?)))
}

fn lower_module(args: &[Value], tail: bool) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("module", "2+", args.len()));
    }
    let name = require_symbol(&args[0], "module")?;
    let export_list = require_list(&args[1], "module")?;
    let export_spur = intern("export");
    if export_list.is_empty()
        || export_list[0]
            .as_symbol_spur()
            .is_none_or(|s| s != export_spur)
    {
        return Err(SemaError::eval(
            "module: second argument must start with 'export'",
        ));
    }
    let exports = export_list[1..]
        .iter()
        .map(|v| require_symbol(v, "module export"))
        .collect::<Result<_, _>>()?;
    let body = lower_body(&args[2..], tail)?;
    Ok(CoreExpr::Module {
        name,
        exports,
        body,
    })
}

fn lower_import(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.is_empty() {
        return Err(SemaError::arity("import", "1+", 0));
    }
    let path = lower_expr(&args[0], false)?;
    let selective = args[1..]
        .iter()
        .map(|v| require_symbol(v, "import"))
        .collect::<Result<_, _>>()?;
    Ok(CoreExpr::Import {
        path: Box::new(path),
        selective,
    })
}

fn lower_load(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("load", "1", args.len()));
    }
    Ok(CoreExpr::Load(Box::new(lower_expr(&args[0], false)?)))
}

fn lower_prompt(args: &[Value]) -> Result<CoreExpr, SemaError> {
    let role_names = ["system", "user", "assistant", "tool"];
    let mut entries = Vec::new();
    for arg in args {
        if let Some(items) = arg.as_list() {
            if !items.is_empty() {
                if let Some(sym) = items[0].as_symbol() {
                    if role_names.contains(&sym.as_str()) {
                        let parts = items[1..]
                            .iter()
                            .map(|v| lower_expr(v, false))
                            .collect::<Result<_, _>>()?;
                        entries.push(PromptEntry::RoleContent {
                            role: sym.to_string(),
                            parts,
                        });
                        continue;
                    }
                }
            }
        }
        entries.push(PromptEntry::Expr(lower_expr(arg, false)?));
    }
    Ok(CoreExpr::Prompt(entries))
}

fn lower_message(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() < 2 {
        return Err(SemaError::arity("message", "2+", args.len()));
    }
    let role = lower_expr(&args[0], false)?;
    let parts = args[1..]
        .iter()
        .map(|v| lower_expr(v, false))
        .collect::<Result<_, _>>()?;
    Ok(CoreExpr::Message {
        role: Box::new(role),
        parts,
    })
}

fn lower_deftool(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() < 4 {
        return Err(SemaError::arity("deftool", "4", args.len()));
    }
    let name = require_symbol(&args[0], "deftool")?;
    let description = lower_expr(&args[1], false)?;
    let parameters = lower_expr(&args[2], false)?;
    let handler = lower_expr(&args[3], false)?;
    Ok(CoreExpr::Deftool {
        name,
        description: Box::new(description),
        parameters: Box::new(parameters),
        handler: Box::new(handler),
    })
}

fn lower_defagent(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 2 {
        return Err(SemaError::arity("defagent", "2", args.len()));
    }
    let name = require_symbol(&args[0], "defagent")?;
    let options = lower_expr(&args[1], false)?;
    Ok(CoreExpr::Defagent {
        name,
        options: Box::new(options),
    })
}

fn lower_delay(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("delay", "1", args.len()));
    }
    // Wrap body in a zero-arg lambda thunk so it captures lexical variables.
    // __vm-delay stores the thunk; __vm-force calls it on first access.
    let body = lower_expr(&args[0], false)?;
    let thunk = CoreExpr::Lambda(LambdaDef {
        name: None,
        params: vec![],
        rest: None,
        body: vec![body],
        upvalues: vec![],
        upvalue_names: vec![],
        n_locals: 0,
    });
    Ok(CoreExpr::Call {
        func: Box::new(CoreExpr::Var(intern("__vm-delay"))),
        args: vec![thunk],
        tail: false,
    })
}

fn lower_force(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("force", "1", args.len()));
    }
    // Force evaluates its argument (to get the thunk), then calls __vm-force
    let expr = lower_expr(&args[0], false)?;
    Ok(CoreExpr::Call {
        func: Box::new(CoreExpr::Var(intern("__vm-force"))),
        args: vec![expr],
        tail: false,
    })
}

fn lower_define_record_type(args: &[Value]) -> Result<CoreExpr, SemaError> {
    if args.len() < 3 {
        return Err(SemaError::eval(
            "define-record-type: requires at least type name, constructor, and predicate",
        ));
    }
    let type_name = require_symbol(&args[0], "define-record-type")?;
    let ctor_spec = require_list(&args[1], "define-record-type")?;
    if ctor_spec.is_empty() {
        return Err(SemaError::eval(
            "define-record-type: constructor spec must have a name",
        ));
    }
    let ctor_name = require_symbol(&ctor_spec[0], "define-record-type")?;
    let field_names = extract_param_spurs(&ctor_spec[1..], "define-record-type")?;
    let pred_name = require_symbol(&args[2], "define-record-type")?;

    let mut field_specs = Vec::new();
    for spec_val in &args[3..] {
        let spec = require_list(spec_val, "define-record-type")?;
        if spec.len() < 2 {
            return Err(SemaError::eval(
                "define-record-type: field spec must have at least (field accessor)",
            ));
        }
        let field = require_symbol(&spec[0], "define-record-type")?;
        let accessor = require_symbol(&spec[1], "define-record-type")?;
        field_specs.push((field, accessor));
    }

    Ok(CoreExpr::DefineRecordType {
        type_name,
        ctor_name,
        pred_name,
        field_names,
        field_specs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Value {
        sema_reader::read(input).unwrap()
    }

    fn lower_str(input: &str) -> CoreExpr {
        lower(&parse(input), None).unwrap()
    }

    #[test]
    fn test_lower_int() {
        match lower_str("42") {
            CoreExpr::Const(v) => assert_eq!(v, Value::int(42)),
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_string() {
        match lower_str("\"hello\"") {
            CoreExpr::Const(v) => assert_eq!(v, Value::string("hello")),
            other => panic!("expected Const, got {other:?}"),
        }
    }

    /// Regression guard for the per-thread special-form cache: this list is
    /// maintained INDEPENDENTLY of `SPECIAL_FORM_NAMES`, so if a form is ever
    /// dropped from that table it would silently lower as a plain function call
    /// — and this test fails instead. Keep both in sync when adding a form.
    #[test]
    fn test_all_special_forms_recognized() {
        let names = [
            "quote",
            "if",
            "cond",
            "define",
            "def",
            "defun",
            "defn",
            "set!",
            "lambda",
            "fn",
            "let",
            "let*",
            "letrec",
            "begin",
            "progn",
            "do",
            "and",
            "or",
            "when",
            "unless",
            "while",
            "defmacro",
            "define-syntax",
            "quasiquote",
            "throw",
            "try",
            "case",
            "eval",
            "macroexpand",
            "module",
            "import",
            "load",
            "prompt",
            "message",
            "deftool",
            "defagent",
            "delay",
            "force",
            "define-record-type",
            "match",
            "match*",
            "defmulti",
            "defmethod",
            "async",
            "await",
        ];
        for name in names {
            assert!(
                special_form_for(intern(name)).is_some(),
                "special form `{name}` is not recognized — missing from SPECIAL_FORM_NAMES?"
            );
        }
    }

    #[test]
    fn test_lower_match_and_match_star_distinct() {
        // Both must lower as special forms (not function calls), and to the
        // right variant. A `match`/`match*` that lowered to a Call would mean the
        // cache table lost the entry.
        assert_eq!(special_form_for(intern("match")), Some(SpecialForm::Match));
        assert_eq!(
            special_form_for(intern("match*")),
            Some(SpecialForm::MatchStar)
        );
        // Sanity: neither lowers to a Call whose head is the literal symbol.
        for src in ["(match 1 (1 :a))", "(match* 1 (1 :a))"] {
            if let CoreExpr::Call { .. } = lower_str(src) {
                panic!("{src} wrongly lowered as a function call");
            }
        }
    }

    #[test]
    fn test_lower_bool() {
        match lower_str("#t") {
            CoreExpr::Const(v) => assert_eq!(v, Value::bool(true)),
            other => panic!("expected Const, got {other:?}"),
        }
        match lower_str("#f") {
            CoreExpr::Const(v) => assert_eq!(v, Value::bool(false)),
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_nil() {
        match lower_str("nil") {
            CoreExpr::Const(v) => assert!(v.is_nil()),
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_keyword() {
        match lower_str(":key") {
            CoreExpr::Const(v) => assert_eq!(v, Value::keyword("key")),
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_symbol() {
        assert!(matches!(lower_str("x"), CoreExpr::Var(_)));
    }

    #[test]
    fn test_lower_empty_list() {
        match lower_str("()") {
            CoreExpr::Const(v) => assert!(v.is_nil()),
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_vector() {
        match lower_str("[1 2 3]") {
            CoreExpr::MakeVector(elems) => assert_eq!(elems.len(), 3),
            other => panic!("expected MakeVector, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_map() {
        match lower_str("{:a 1 :b 2}") {
            CoreExpr::MakeMap(pairs) => assert_eq!(pairs.len(), 2),
            other => panic!("expected MakeMap, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_quote() {
        match lower_str("(quote x)") {
            CoreExpr::Quote(v) => assert!(v.as_symbol_spur().is_some()),
            other => panic!("expected Quote, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_if() {
        match lower_str("(if #t 1 2)") {
            CoreExpr::If { test, then, else_ } => {
                match *test {
                    CoreExpr::Const(v) => assert_eq!(v, Value::bool(true)),
                    _ => panic!("expected bool"),
                }
                match *then {
                    CoreExpr::Const(v) => assert_eq!(v, Value::int(1)),
                    _ => panic!("expected int"),
                }
                match *else_ {
                    CoreExpr::Const(v) => assert_eq!(v, Value::int(2)),
                    _ => panic!("expected int"),
                }
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_if_no_else() {
        match lower_str("(if #t 1)") {
            CoreExpr::If { else_, .. } => match *else_ {
                CoreExpr::Const(v) => assert!(v.is_nil()),
                _ => panic!("expected nil"),
            },
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_cond() {
        match lower_str("(cond (#t 1))") {
            CoreExpr::If { test, then, .. } => {
                assert!(matches!(*test, CoreExpr::Const(v) if v == Value::bool(true)));
                assert!(matches!(*then, CoreExpr::Const(v) if v == Value::int(1)));
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_cond_else() {
        assert!(matches!(
            lower_str("(cond (#f 1) (else 2))"),
            CoreExpr::If { .. }
        ));
    }

    #[test]
    fn test_lower_define_simple() {
        match lower_str("(define x 42)") {
            CoreExpr::Define(_, val) => match *val {
                CoreExpr::Const(v) => assert_eq!(v, Value::int(42)),
                _ => panic!("expected int"),
            },
            other => panic!("expected Define, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_define_function() {
        match lower_str("(define (f x) x)") {
            CoreExpr::Define(_, val) => {
                assert!(matches!(*val, CoreExpr::Lambda(_)));
            }
            other => panic!("expected Define with Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_defun() {
        match lower_str("(defun f (x) x)") {
            CoreExpr::Define(_, val) => {
                assert!(matches!(*val, CoreExpr::Lambda(_)));
            }
            other => panic!("expected Define with Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_set() {
        match lower_str("(set! x 42)") {
            CoreExpr::Set(_, val) => {
                assert!(matches!(*val, CoreExpr::Const(v) if v == Value::int(42)));
            }
            other => panic!("expected Set, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_lambda() {
        match lower_str("(lambda (x y) (+ x y))") {
            CoreExpr::Lambda(def) => {
                assert_eq!(def.params.len(), 2);
                assert!(def.rest.is_none());
                assert!(def.name.is_none());
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_fn() {
        match lower_str("(fn (x) x)") {
            CoreExpr::Lambda(def) => {
                assert_eq!(def.params.len(), 1);
                assert!(def.rest.is_none());
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_lambda_rest() {
        match lower_str("(lambda (x . rest) rest)") {
            CoreExpr::Lambda(def) => {
                assert_eq!(def.params.len(), 1);
                assert!(def.rest.is_some());
            }
            other => panic!("expected Lambda with rest, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_let() {
        match lower_str("(let ((x 1)) x)") {
            CoreExpr::Let { bindings, body } => {
                assert_eq!(bindings.len(), 1);
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_named_let() {
        // Named let desugars to letrec + lambda
        match lower_str("(let loop ((n 10)) (if (= n 0) 0 (loop (- n 1))))") {
            CoreExpr::Letrec { bindings, body } => {
                assert_eq!(bindings.len(), 1);
                assert!(matches!(&bindings[0].1, CoreExpr::Lambda(_)));
                assert_eq!(body.len(), 1);
                assert!(matches!(&body[0], CoreExpr::Call { .. }));
            }
            other => panic!("expected Letrec, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_let_star() {
        assert!(matches!(
            lower_str("(let* ((x 1) (y x)) y)"),
            CoreExpr::LetStar { .. }
        ));
    }

    #[test]
    fn test_lower_letrec() {
        assert!(matches!(
            lower_str("(letrec ((f (lambda () f))) f)"),
            CoreExpr::Letrec { .. }
        ));
    }

    #[test]
    fn test_lower_begin() {
        assert!(matches!(lower_str("(begin 1 2 3)"), CoreExpr::Begin(_)));
    }

    #[test]
    fn test_lower_begin_single() {
        // Single-expr begin unwraps
        match lower_str("(begin 42)") {
            CoreExpr::Const(v) => assert_eq!(v, Value::int(42)),
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_and() {
        assert!(matches!(lower_str("(and 1 2 3)"), CoreExpr::And(_)));
    }

    #[test]
    fn test_lower_and_empty() {
        match lower_str("(and)") {
            CoreExpr::Const(v) => assert_eq!(v, Value::bool(true)),
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_or() {
        assert!(matches!(lower_str("(or 1 2 3)"), CoreExpr::Or(_)));
    }

    #[test]
    fn test_lower_or_empty() {
        match lower_str("(or)") {
            CoreExpr::Const(v) => assert_eq!(v, Value::bool(false)),
            other => panic!("expected Const, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_when() {
        match lower_str("(when #t 42)") {
            CoreExpr::If { else_, .. } => match *else_ {
                CoreExpr::Const(v) => assert!(v.is_nil()),
                _ => panic!("expected nil"),
            },
            other => panic!("expected If from when, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_unless() {
        match lower_str("(unless #f 42)") {
            CoreExpr::If { then, .. } => match *then {
                CoreExpr::Const(v) => assert!(v.is_nil()),
                _ => panic!("expected nil"),
            },
            other => panic!("expected If from unless, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_throw() {
        assert!(matches!(lower_str("(throw \"error\")"), CoreExpr::Throw(_)));
    }

    #[test]
    fn test_lower_try() {
        match lower_str("(try 1 (catch e e))") {
            CoreExpr::Try { body, handler, .. } => {
                assert_eq!(body.len(), 1);
                assert_eq!(handler.len(), 1);
            }
            other => panic!("expected Try, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_do() {
        match lower_str("(do ((i 0 (+ i 1))) ((= i 10) i))") {
            CoreExpr::Do(loop_) => {
                assert_eq!(loop_.vars.len(), 1);
                assert!(loop_.vars[0].step.is_some());
            }
            other => panic!("expected Do, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_eval() {
        assert!(matches!(lower_str("(eval '(+ 1 2))"), CoreExpr::Eval(_)));
    }

    #[test]
    fn test_lower_case() {
        // (case x ((1) "one") (else "other")) → Let + If
        assert!(matches!(
            lower_str("(case x ((1) \"one\") (else \"other\"))"),
            CoreExpr::Let { .. }
        ));
    }

    #[test]
    fn test_lower_function_call() {
        match lower_str("(f 1 2)") {
            CoreExpr::Call { func, args, tail } => {
                assert!(matches!(*func, CoreExpr::Var(_)));
                assert_eq!(args.len(), 2);
                assert!(!tail);
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn test_tail_position_lambda_body() {
        // (lambda () (f x)) → last body expr should be tail call
        match lower_str("(lambda () (f x))") {
            CoreExpr::Lambda(def) => match &def.body[0] {
                CoreExpr::Call { tail, .. } => assert!(*tail),
                other => panic!("expected tail Call, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_tail_position_begin() {
        match lower_str("(lambda () (begin 1 (f x)))") {
            CoreExpr::Lambda(def) => match &def.body[0] {
                CoreExpr::Begin(exprs) => match exprs.last().unwrap() {
                    CoreExpr::Call { tail, .. } => assert!(*tail),
                    other => panic!("expected tail Call, got {other:?}"),
                },
                other => panic!("expected Begin, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_non_tail_position() {
        // (lambda () (f x) 1) → call is NOT tail
        match lower_str("(lambda () (f x) 1)") {
            CoreExpr::Lambda(def) => {
                assert_eq!(def.body.len(), 2);
                match &def.body[0] {
                    CoreExpr::Call { tail, .. } => assert!(!*tail),
                    other => panic!("expected non-tail Call, got {other:?}"),
                }
            }
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_tail_if_branches() {
        // (lambda () (if #t (f x) (g y))) → both branches are tail
        match lower_str("(lambda () (if #t (f x) (g y)))") {
            CoreExpr::Lambda(def) => match &def.body[0] {
                CoreExpr::If { then, else_, .. } => {
                    assert!(matches!(then.as_ref(), CoreExpr::Call { tail: true, .. }));
                    assert!(matches!(else_.as_ref(), CoreExpr::Call { tail: true, .. }));
                }
                other => panic!("expected If, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_try_body_not_tail() {
        // (lambda () (try (f x) (catch e e))) → try body is NOT tail
        match lower_str("(lambda () (try (f x) (catch e e)))") {
            CoreExpr::Lambda(def) => match &def.body[0] {
                CoreExpr::Try { body, .. } => match &body[0] {
                    CoreExpr::Call { tail, .. } => assert!(!*tail),
                    other => panic!("expected non-tail Call in try body, got {other:?}"),
                },
                other => panic!("expected Try, got {other:?}"),
            },
            other => panic!("expected Lambda, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_quasiquote_simple() {
        // `(1 2 3) → MakeList of quotes
        match lower_str("`(1 2 3)") {
            CoreExpr::MakeList(items) => {
                assert_eq!(items.len(), 3);
            }
            other => panic!("expected MakeList, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_quasiquote_unquote() {
        // `(1 ,x 3) → MakeList with Var for x
        match lower_str("`(1 ,x 3)") {
            CoreExpr::MakeList(items) => {
                assert_eq!(items.len(), 3);
                assert!(matches!(&items[1], CoreExpr::Var(_)));
            }
            other => panic!("expected MakeList, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_quasiquote_splice() {
        // `(1 ,@xs 3) → Call to append
        match lower_str("`(1 ,@xs 3)") {
            CoreExpr::Call { func, .. } => {
                assert!(matches!(func.as_ref(), CoreExpr::Var(_)));
            }
            other => panic!("expected Call (append), got {other:?}"),
        }
    }

    #[test]
    fn test_lower_delay() {
        // delay now lowers to a Call to __vm-delay with a zero-arg lambda thunk
        match lower_str("(delay (+ 1 2))") {
            CoreExpr::Call { func, args, .. } => {
                assert!(matches!(*func, CoreExpr::Var(_)));
                assert_eq!(args.len(), 1);
                assert!(matches!(&args[0], CoreExpr::Lambda(_)));
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_force() {
        // force now lowers to a Call to __vm-force
        assert!(matches!(lower_str("(force p)"), CoreExpr::Call { .. }));
    }

    #[test]
    fn test_lower_defmacro() {
        // defmacro now lowers to a Call to __vm-defmacro-form with the full form as a constant
        match lower_str("(defmacro my-if (test then else) (list 'if test then else))") {
            CoreExpr::Call { func, args, .. } => {
                assert!(matches!(*func, CoreExpr::Var(_)));
                assert_eq!(args.len(), 1);
                match &args[0] {
                    CoreExpr::Const(v) => assert!(v.is_list()),
                    _ => panic!("expected list const"),
                }
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_import() {
        assert!(matches!(
            lower_str("(import \"lib.sema\")"),
            CoreExpr::Import { .. }
        ));
    }

    #[test]
    fn test_lower_load() {
        assert!(matches!(
            lower_str("(load \"lib.sema\")"),
            CoreExpr::Load(_)
        ));
    }

    #[test]
    fn test_lower_define_record_type() {
        match lower_str(
            "(define-record-type point (make-point x y) point? (x point-x) (y point-y))",
        ) {
            CoreExpr::DefineRecordType {
                field_names,
                field_specs,
                ..
            } => {
                assert_eq!(field_names.len(), 2);
                assert_eq!(field_specs.len(), 2);
            }
            other => panic!("expected DefineRecordType, got {other:?}"),
        }
    }

    #[test]
    fn test_lower_depth_limit() {
        let mut expr = Value::list(vec![Value::symbol("+"), Value::int(1), Value::int(1)]);
        for _ in 0..600 {
            expr = Value::list(vec![Value::symbol("begin"), expr]);
        }
        let result = lower(&expr, None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("depth"), "expected depth error, got: {err}");
    }

    // ---- SpanMap tests (span_map=Some path) ----

    #[test]
    fn test_lower_with_span_map_attaches_spans() {
        let input = "(f 1 2)";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let core = lower(&vals[0], Some(&span_map)).unwrap();
        // With a span_map, list expressions get wrapped in Spanned
        match &core {
            CoreExpr::Spanned(span, inner) => {
                assert_eq!(span.line, 1);
                assert_eq!(span.col, 1);
                match inner.as_ref() {
                    CoreExpr::Call { func, args, .. } => {
                        assert!(matches!(**func, CoreExpr::Var(_)));
                        assert_eq!(args.len(), 2);
                    }
                    other => panic!("expected Call inside Spanned, got {other:?}"),
                }
            }
            other => panic!("expected Spanned(Call) for (f 1 2), got {other:?}"),
        }
    }

    #[test]
    fn test_lower_with_span_map_vs_without() {
        // Without span_map: bare If. With span_map: Spanned wrapping If.
        let input = "(if #t 1 2)";
        let val = parse(input);
        let without_spans = lower(&val, None).unwrap();
        assert!(
            matches!(&without_spans, CoreExpr::If { .. }),
            "without spans should be bare If"
        );

        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let with_spans = lower(&vals[0], Some(&span_map)).unwrap();
        assert!(
            matches!(&with_spans, CoreExpr::Spanned(_, inner) if matches!(inner.as_ref(), CoreExpr::If { .. })),
            "with spans should be Spanned(If)"
        );
    }

    #[test]
    fn test_lower_with_span_map_cleans_up_thread_local() {
        let input = "(+ 1 2)";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let _core = lower(&vals[0], Some(&span_map)).unwrap();

        // After lower returns, SPAN_MAP should be cleared
        SPAN_MAP.with(|cell| {
            assert!(
                cell.borrow().is_none(),
                "SPAN_MAP should be None after lower() returns"
            );
        });
    }

    #[test]
    fn test_lower_with_span_map_cleans_up_on_error() {
        // Force an error during lowering with a span_map set
        let mut expr = Value::list(vec![Value::symbol("+"), Value::int(1), Value::int(1)]);
        for _ in 0..600 {
            expr = Value::list(vec![Value::symbol("begin"), expr]);
        }
        // Build a trivial span_map (we just need Some, doesn't need real entries)
        let span_map = SpanMap::default();
        let result = lower(&expr, Some(&span_map));
        assert!(result.is_err());

        // SPAN_MAP should still be cleaned up even after error
        SPAN_MAP.with(|cell| {
            assert!(
                cell.borrow().is_none(),
                "SPAN_MAP should be None after lower() errors"
            );
        });
    }

    #[test]
    fn test_lower_with_span_map_lambda() {
        let input = "(fn (x) (+ x 1))";
        let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
        let core = lower(&vals[0], Some(&span_map)).unwrap();
        // Lambda gets wrapped in Spanned when span_map is provided
        match &core {
            CoreExpr::Spanned(_, inner) => {
                assert!(
                    matches!(inner.as_ref(), CoreExpr::Lambda(_)),
                    "expected Lambda inside Spanned, got {inner:?}"
                );
            }
            other => panic!("expected Spanned(Lambda), got {other:?}"),
        }
    }
}
