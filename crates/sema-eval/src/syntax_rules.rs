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
//! 3. applying **hygiene**: any template identifier that is not a pattern
//!    variable, literal, ellipsis, special form, auxiliary keyword, or bound in
//!    the environment is consistently alpha-renamed to a fresh gensym per
//!    expansion. This reuses the same `next_gensym` engine as `foo#`
//!    auto-gensym, so a macro-introduced binder and its references stay linked
//!    but cannot capture (or be captured by) user identifiers of the same name.
//!
//! Hygiene here is an approximation of full R7RS: renaming keys off the
//! use-site environment (== the global env for the common top-level-macro case),
//! not a per-identifier definition environment. See `docs/limitations.md`.

use std::collections::HashMap;

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

/// Auxiliary syntactic keywords that some special forms recognize *by name* as
/// bare identifiers (they are not in `sema_vm`'s special-form table and are not
/// env bindings), so hygiene must keep them verbatim rather than rename them.
const AUX_KEYWORDS: &[&str] = &["catch", "else", "=>"];

/// Expand one call to a `syntax-rules` macro. `args` are the (unevaluated) call
/// arguments — i.e. the macro-call form minus its head. `env` is the use-site
/// environment used for the hygiene keep decision.
pub fn expand(mac: &Macro, args: &[Value], env: &Env) -> Result<Value, SemaError> {
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
            let mut rename: HashMap<Spur, Spur> = HashMap::new();
            return instantiate(template, &bindings, sr, &mut rename, env);
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

/// Should this non-pattern-var template identifier be kept verbatim (rather than
/// hygienically renamed)?
fn keep_identifier(s: Spur, name: &str, sr: &sema_core::SyntaxRules, env: &Env) -> bool {
    sr.literals.contains(&s)
        || AUX_KEYWORDS.contains(&name)
        || sema_vm::is_special_form(name)
        || env.get(s).is_some()
}

/// Instantiate a template into a form, substituting pattern variables and
/// applying hygiene.
fn instantiate(
    tmpl: &Value,
    bindings: &Bindings,
    sr: &sema_core::SyntaxRules,
    rename: &mut HashMap<Spur, Spur>,
    env: &Env,
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
        let name = resolve(s);
        if keep_identifier(s, &name, sr, env) {
            return Ok(tmpl.clone());
        }
        // Macro-introduced identifier: alpha-rename to a fresh gensym, one per
        // identifier per expansion (reused for every occurrence).
        let renamed = *rename
            .entry(s)
            .or_insert_with(|| intern(&next_gensym(&name)));
        return Ok(Value::symbol_from_spur(renamed));
    }

    match tmpl.view() {
        ValueView::List(elems) => {
            let out = instantiate_seq(&elems, bindings, sr, rename, env)?;
            Ok(Value::list(out))
        }
        ValueView::Vector(elems) => {
            let out = instantiate_seq(&elems, bindings, sr, rename, env)?;
            Ok(Value::vector(out))
        }
        ValueView::Map(map) => {
            let mut out = std::collections::BTreeMap::new();
            for (k, v) in map.iter() {
                let nk = instantiate(k, bindings, sr, rename, env)?;
                let nv = instantiate(v, bindings, sr, rename, env)?;
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
    rename: &mut HashMap<Spur, Spur>,
    env: &Env,
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
            out.push(instantiate(&elems[i], bindings, sr, rename, env)?);
            i += 1;
        } else if ell == 1 {
            expand_ellipsis(&elems[i], bindings, sr, rename, env, &mut out)?;
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
    rename: &mut HashMap<Spur, Spur>,
    env: &Env,
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
        out.push(instantiate(sub, &child, sr, rename, env)?);
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
