//! R7RS `syntax-rules` matcher and template instantiator.
//!
//! A `(define-syntax name (syntax-rules (literals...) (pattern template)...))`
//! macro is registered (by `eval::register_define_syntax`) as a
//! [`sema_core::Macro`] carrying a [`sema_core::SyntaxRules`]. When the macro
//! pre-expansion pass (`eval::expand_macros_in`) sees a call to such a macro it
//! calls [`expand`], which:
//!
//! 1. tries each rule's pattern against the call arguments in order (first full
//!    match wins), producing a [`Bindings`] map from pattern variables to
//!    matched input (a [`MatchTree`] that mirrors ellipsis nesting), then
//! 2. instantiates the winning rule's template, substituting pattern variables
//!    and driving ellipsis (`...`) expansion, while
//! 3. applying **binder-directed hygiene**: before instantiating, a pass over
//!    the winning rule's template collects the set of identifiers the template
//!    itself introduces *as binders* (the vars a template-introduced `let` /
//!    `let*` / `letrec[*]` / `lambda` / `fn` / `define` / `do` / named-let
//!    binds — e.g. the `tmp` in `(let ((tmp a)) …)`). Only those binder
//!    identifiers are consistently alpha-renamed to a fresh gensym per
//!    expansion; every *other* template identifier — including free references
//!    to user globals, builtins, special forms, and the macro's own name for
//!    recursion — is kept verbatim and resolves at the use site / runtime.
//!    Pattern variables are substituted before this decision, so they are never
//!    renamed. Renaming reuses the same `next_gensym` engine as `foo#`
//!    auto-gensym, so a macro-introduced binder and its in-template references
//!    stay linked but cannot capture (or be captured by) user identifiers of
//!    the same name.
//!
//! This is binder-directed rather than reference-directed: the decision keys off
//! whether the template introduces the identifier as a binder, NOT off the
//! use-site environment. That is what lets a macro template freely reference a
//! user-defined global that is not yet bound when the macro is pre-expanded
//! (whole-program mode). It is still an approximation of full R7RS (no
//! per-identifier definition environments). See `docs/limitations.md`.

use std::collections::{HashMap, HashSet};

use sema_core::{intern, next_gensym, resolve, Env, Macro, SemaError, Spur, Value, ValueView};

/// The matched value of a pattern variable. A variable bound directly is a
/// `Leaf`; a variable under one ellipsis level is a `Seq` of the per-repetition
/// matches (which are themselves `Leaf`s, or `Seq`s for deeper nesting).
#[derive(Debug, Clone)]
enum MatchTree {
    Leaf(Value),
    Seq(Vec<MatchTree>),
}

type Bindings = HashMap<Spur, MatchTree>;

/// Expand one call to a `syntax-rules` macro. `args` are the (unevaluated) call
/// arguments — i.e. the macro-call form minus its head. `_env` is the use-site
/// environment; it is intentionally NOT consulted for hygiene (binder-directed
/// hygiene keys off the template, not the environment), and is kept only so the
/// call sites need not change.
pub fn expand(mac: &Macro, args: &[Value], _env: &Env) -> Result<Value, SemaError> {
    let sr = mac
        .syntax_rules
        .as_ref()
        .expect("expand called on a non-syntax-rules macro");
    let macro_name = resolve(mac.name);

    for (pattern, template) in &sr.rules {
        let pat_elems = pattern.as_list().ok_or_else(|| {
            SemaError::eval(format!(
                "syntax-rules: pattern in '{macro_name}' must be a list"
            ))
        })?;
        // pat_elems[0] is the macro-keyword slot (conventionally `_` or the
        // macro name); it is ignored. Match the rest against the call args.
        let mut bindings = Bindings::new();
        if match_seq(&pat_elems[1..], args, sr, &mut bindings)? {
            // Binder-directed hygiene: only identifiers the template introduces
            // as binders get alpha-renamed; everything else stays verbatim.
            let mut binders: HashSet<Spur> = HashSet::new();
            collect_template_binders(template, &bindings, sr, &mut binders);
            let mut rename: HashMap<Spur, Spur> = HashMap::new();
            return instantiate(template, &bindings, sr, &binders, &mut rename);
        }
    }

    Err(SemaError::eval(format!(
        "no matching syntax-rules clause for macro '{macro_name}'"
    )))
}

/// Is `v` the ellipsis symbol for this transformer?
fn is_ellipsis(v: &Value, sr: &sema_core::SyntaxRules) -> bool {
    v.as_symbol_spur() == Some(sr.ellipsis)
}

/// Collect the pattern variables appearing in `pat` (symbols that are not
/// literals, wildcards, or the ellipsis).
fn collect_pattern_vars(pat: &Value, sr: &sema_core::SyntaxRules, out: &mut Vec<Spur>) {
    if let Some(s) = pat.as_symbol_spur() {
        if s == sr.ellipsis || sr.literals.contains(&s) {
            return;
        }
        if resolve(s) == "_" {
            return;
        }
        if !out.contains(&s) {
            out.push(s);
        }
        return;
    }
    match pat.view() {
        ValueView::List(items) | ValueView::Vector(items) => {
            for item in items.iter() {
                collect_pattern_vars(item, sr, out);
            }
        }
        _ => {}
    }
}

/// Match a single pattern element against a single input value.
fn match_pattern(
    pat: &Value,
    input: &Value,
    sr: &sema_core::SyntaxRules,
    bindings: &mut Bindings,
) -> Result<bool, SemaError> {
    if let Some(s) = pat.as_symbol_spur() {
        if s == sr.ellipsis {
            // A bare ellipsis as a pattern element is malformed; treat as no
            // match rather than panicking.
            return Ok(false);
        }
        if resolve(s) == "_" {
            return Ok(true); // wildcard: matches anything, binds nothing
        }
        if sr.literals.contains(&s) {
            // Literal identifier: matches iff the input is the same identifier
            // (name-equality approximation of R7RS same-binding).
            return Ok(input.as_symbol_spur() == Some(s));
        }
        // Pattern variable: bind it.
        bindings.insert(s, MatchTree::Leaf(input.clone()));
        return Ok(true);
    }

    match (pat.view(), input.view()) {
        (ValueView::List(pl), _) => match input.as_list() {
            Some(il) => match_seq(&pl, il, sr, bindings),
            None => Ok(false),
        },
        (ValueView::Vector(pl), ValueView::Vector(il)) => match_seq(&pl, &il, sr, bindings),
        _ => {
            // Self-evaluating datum: match by structural equality.
            Ok(pat == input)
        }
    }
}

/// Match a sequence of pattern elements against a sequence of inputs, honoring
/// at most one ellipsis at this level.
fn match_seq(
    pats: &[Value],
    inputs: &[Value],
    sr: &sema_core::SyntaxRules,
    bindings: &mut Bindings,
) -> Result<bool, SemaError> {
    let ell_pos = pats.iter().position(|p| is_ellipsis(p, sr));
    match ell_pos {
        None => {
            if pats.len() != inputs.len() {
                return Ok(false);
            }
            for (p, i) in pats.iter().zip(inputs.iter()) {
                if !match_pattern(p, i, sr, bindings)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Some(pos) => {
            if pos == 0 {
                return Err(SemaError::eval(
                    "syntax-rules: ellipsis with no preceding subpattern",
                ));
            }
            let sub = &pats[pos - 1];
            let prefix = &pats[..pos - 1];
            let suffix = &pats[pos + 1..];
            if inputs.len() < prefix.len() + suffix.len() {
                return Ok(false);
            }
            // Fixed prefix.
            for (p, i) in prefix.iter().zip(inputs[..prefix.len()].iter()) {
                if !match_pattern(p, i, sr, bindings)? {
                    return Ok(false);
                }
            }
            let mid_end = inputs.len() - suffix.len();
            let mid = &inputs[prefix.len()..mid_end];
            // Repeated subpattern: collect each subpattern var into a Seq.
            let mut sub_vars = Vec::new();
            collect_pattern_vars(sub, sr, &mut sub_vars);
            let mut seqs: HashMap<Spur, Vec<MatchTree>> =
                sub_vars.iter().map(|v| (*v, Vec::new())).collect();
            for elem in mid.iter() {
                let mut sub_b = Bindings::new();
                if !match_pattern(sub, elem, sr, &mut sub_b)? {
                    return Ok(false);
                }
                for v in &sub_vars {
                    let mt = sub_b.remove(v).unwrap_or(MatchTree::Leaf(Value::nil()));
                    seqs.get_mut(v).unwrap().push(mt);
                }
            }
            for (v, seq) in seqs {
                bindings.insert(v, MatchTree::Seq(seq));
            }
            // Fixed suffix.
            for (p, i) in suffix.iter().zip(inputs[mid_end..].iter()) {
                if !match_pattern(p, i, sr, bindings)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}

/// Record `s` as a template-introduced binder, unless it is something that must
/// never be renamed: a pattern variable (already in `bindings`), the ellipsis, a
/// literal, or a structural marker (`.`, `&`, `_`).
fn note_binder(s: Spur, bindings: &Bindings, sr: &sema_core::SyntaxRules, out: &mut HashSet<Spur>) {
    if bindings.contains_key(&s) || s == sr.ellipsis || sr.literals.contains(&s) {
        return;
    }
    match resolve(s).as_str() {
        "." | "&" | "_" => {}
        _ => {
            out.insert(s);
        }
    }
}

/// Collect every symbol in a lambda/`fn`/define parameter list as a binder,
/// including a rest parameter (`. rest` / `& rest`). Params are a flat list (or
/// vector) of symbols with `.`/`&` markers; nested destructuring patterns are
/// walked so their vars are captured too.
fn note_param_binders(
    params: &Value,
    bindings: &Bindings,
    sr: &sema_core::SyntaxRules,
    out: &mut HashSet<Spur>,
) {
    if let Some(s) = params.as_symbol_spur() {
        note_binder(s, bindings, sr, out);
        return;
    }
    match params.view() {
        ValueView::List(items) | ValueView::Vector(items) => {
            for item in items.iter() {
                note_param_binders(item, bindings, sr, out);
            }
        }
        ValueView::Map(map) => {
            for (k, v) in map.iter() {
                note_param_binders(k, bindings, sr, out);
                note_param_binders(v, bindings, sr, out);
            }
        }
        _ => {}
    }
}

/// Collect the identifiers the template introduces as binders. These — and only
/// these — are the non-pattern-var identifiers that hygiene alpha-renames. The
/// binding forms are recognized by resolving the head symbol's name; every
/// sub-form is recursed into so nested binders are found.
fn collect_template_binders(
    tmpl: &Value,
    bindings: &Bindings,
    sr: &sema_core::SyntaxRules,
    out: &mut HashSet<Spur>,
) {
    let items = match tmpl.view() {
        ValueView::List(items) => items,
        ValueView::Vector(items) => {
            for item in items.iter() {
                collect_template_binders(item, bindings, sr, out);
            }
            return;
        }
        ValueView::Map(map) => {
            for (k, v) in map.iter() {
                collect_template_binders(k, bindings, sr, out);
                collect_template_binders(v, bindings, sr, out);
            }
            return;
        }
        _ => return,
    };

    if let Some(head) = items.first().and_then(|h| h.as_symbol_spur()) {
        match resolve(head).as_str() {
            "let" | "let*" | "letrec" | "letrec*" => {
                // Named let `(let name ((v e) ...) body ...)` also binds `name`;
                // distinguish it from `(let ((v e) ...) ...)` by a symbol in the
                // 2nd slot (a plain let has a binding *list* there).
                let mut idx = 1;
                if resolve(head) == "let" {
                    if let Some(n) = items.get(1).and_then(|x| x.as_symbol_spur()) {
                        note_binder(n, bindings, sr, out);
                        idx = 2;
                    }
                }
                if let Some(binds) = items.get(idx).and_then(|b| b.as_list()) {
                    for pair in binds {
                        if let Some(v) = pair
                            .as_list()
                            .and_then(|pl| pl.first())
                            .and_then(|x| x.as_symbol_spur())
                        {
                            note_binder(v, bindings, sr, out);
                        }
                    }
                }
            }
            "lambda" | "fn" => {
                if let Some(params) = items.get(1) {
                    note_param_binders(params, bindings, sr, out);
                }
            }
            "define" => match items.get(1) {
                Some(target) if target.as_symbol_spur().is_some() => {
                    note_binder(target.as_symbol_spur().unwrap(), bindings, sr, out);
                }
                // (define (f a b ...) body ...) → f and each arg symbol.
                Some(target) => note_param_binders(target, bindings, sr, out),
                None => {}
            },
            "do" => {
                // (do ((v init step) ...) ...) → each v.
                if let Some(specs) = items.get(1).and_then(|b| b.as_list()) {
                    for spec in specs {
                        if let Some(v) = spec
                            .as_list()
                            .and_then(|sl| sl.first())
                            .and_then(|x| x.as_symbol_spur())
                        {
                            note_binder(v, bindings, sr, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Recurse into every sub-form so nested binding forms are found.
    for item in items.iter() {
        collect_template_binders(item, bindings, sr, out);
    }
}

/// Instantiate a template into a form, substituting pattern variables and
/// applying binder-directed hygiene. `binders` is the set of identifiers the
/// template introduces as binders (see [`collect_template_binders`]); only those
/// non-pattern-var identifiers are alpha-renamed.
fn instantiate(
    tmpl: &Value,
    bindings: &Bindings,
    sr: &sema_core::SyntaxRules,
    binders: &HashSet<Spur>,
    rename: &mut HashMap<Spur, Spur>,
) -> Result<Value, SemaError> {
    if let Some(s) = tmpl.as_symbol_spur() {
        if let Some(mt) = bindings.get(&s) {
            return match mt {
                MatchTree::Leaf(v) => Ok(v.clone()),
                MatchTree::Seq(_) => Err(SemaError::eval(format!(
                    "syntax-rules: pattern variable '{}' used without ellipsis",
                    resolve(s)
                ))),
            };
        }
        if s == sr.ellipsis {
            return Ok(tmpl.clone());
        }
        // Keep every non-pattern-var identifier verbatim UNLESS the template
        // introduces it as a binder; a binder (and its in-template references)
        // is alpha-renamed to a fresh gensym, one per identifier per expansion.
        if !binders.contains(&s) {
            return Ok(tmpl.clone());
        }
        let renamed = *rename
            .entry(s)
            .or_insert_with(|| intern(&next_gensym(&resolve(s))));
        return Ok(Value::symbol_from_spur(renamed));
    }

    match tmpl.view() {
        ValueView::List(elems) => {
            let out = instantiate_seq(&elems, bindings, sr, binders, rename)?;
            Ok(Value::list(out))
        }
        ValueView::Vector(elems) => {
            let out = instantiate_seq(&elems, bindings, sr, binders, rename)?;
            Ok(Value::vector(out))
        }
        ValueView::Map(map) => {
            let mut out = std::collections::BTreeMap::new();
            for (k, v) in map.iter() {
                let nk = instantiate(k, bindings, sr, binders, rename)?;
                let nv = instantiate(v, bindings, sr, binders, rename)?;
                out.insert(nk, nv);
            }
            Ok(Value::map(out))
        }
        _ => Ok(tmpl.clone()),
    }
}

/// Instantiate the elements of a list/vector template, handling `elem ...`
/// ellipsis expansion (splicing the repeated results into the output).
fn instantiate_seq(
    elems: &[Value],
    bindings: &Bindings,
    sr: &sema_core::SyntaxRules,
    binders: &HashSet<Spur>,
    rename: &mut HashMap<Spur, Spur>,
) -> Result<Vec<Value>, SemaError> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < elems.len() {
        // Count following ellipses.
        let mut ell = 0;
        let mut j = i + 1;
        while j < elems.len() && is_ellipsis(&elems[j], sr) {
            ell += 1;
            j += 1;
        }
        if ell == 0 {
            out.push(instantiate(&elems[i], bindings, sr, binders, rename)?);
            i += 1;
        } else if ell == 1 {
            expand_ellipsis(&elems[i], bindings, sr, binders, rename, &mut out)?;
            i = j;
        } else {
            return Err(SemaError::eval(
                "syntax-rules: unsupported nested ellipsis depth in template",
            ));
        }
    }
    Ok(out)
}

/// Expand `sub ...`: iterate the ellipsis pattern variables occurring in `sub`
/// in lockstep, instantiating `sub` once per repetition and appending to `out`.
fn expand_ellipsis(
    sub: &Value,
    bindings: &Bindings,
    sr: &sema_core::SyntaxRules,
    binders: &HashSet<Spur>,
    rename: &mut HashMap<Spur, Spur>,
    out: &mut Vec<Value>,
) -> Result<(), SemaError> {
    // The driver variables are the symbols in `sub` bound to a Seq.
    let mut vars = Vec::new();
    collect_template_seq_vars(sub, bindings, &mut vars);
    if vars.is_empty() {
        return Err(SemaError::eval(
            "syntax-rules: ellipsis in template with no matching pattern variable",
        ));
    }
    // All driver Seqs must have equal length.
    let mut len: Option<usize> = None;
    for v in &vars {
        if let Some(MatchTree::Seq(items)) = bindings.get(v) {
            match len {
                None => len = Some(items.len()),
                Some(l) if l != items.len() => {
                    return Err(SemaError::eval(
                        "syntax-rules: mismatched ellipsis lengths for pattern variables",
                    ));
                }
                _ => {}
            }
        }
    }
    let len = len.unwrap_or(0);
    for idx in 0..len {
        let mut child = bindings.clone();
        for v in &vars {
            if let Some(MatchTree::Seq(items)) = bindings.get(v) {
                child.insert(*v, items[idx].clone());
            }
        }
        out.push(instantiate(sub, &child, sr, binders, rename)?);
    }
    Ok(())
}

/// Collect the symbols in `sub` that are pattern variables currently bound to a
/// `Seq` (i.e. that drive an ellipsis at this level).
fn collect_template_seq_vars(sub: &Value, bindings: &Bindings, out: &mut Vec<Spur>) {
    if let Some(s) = sub.as_symbol_spur() {
        if matches!(bindings.get(&s), Some(MatchTree::Seq(_))) && !out.contains(&s) {
            out.push(s);
        }
        return;
    }
    match sub.view() {
        ValueView::List(items) | ValueView::Vector(items) => {
            for item in items.iter() {
                collect_template_seq_vars(item, bindings, out);
            }
        }
        ValueView::Map(map) => {
            for (k, v) in map.iter() {
                collect_template_seq_vars(k, bindings, out);
                collect_template_seq_vars(v, bindings, out);
            }
        }
        _ => {}
    }
}
