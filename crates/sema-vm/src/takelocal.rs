//! Last-use analysis for `Op::TakeLocal` (moving local loads).
//!
//! A `LoadLocal` clones the slot value, so a local that is about to die still
//! pins its heap allocation with an extra refcount. That extra count is what
//! defeats the stdlib's uniqueness-gated in-place fast paths
//! (`with_hashmap_mut_if_unique` & co.): an accumulator threaded through
//! `(assoc acc k v)` is cloned on every step even though the old map is dead.
//! Compiling the *statically last* use of a slot as `TakeLocal` (move the
//! value out, leave nil) collapses the dead ref, so a uniquely-owned
//! accumulator is mutated in place.
//!
//! The analysis is deliberately conservative — a missed take costs one clone,
//! a wrong take reads nil where a value was expected. A whole function is
//! **opted out** when any construct below appears anywhere in its body:
//!
//! - `try`/`catch`: an exception entering the handler could otherwise observe
//!   slots already taken by the partially-executed body.
//! - `do` loops: the back-edge re-reads loop-variable slots stored in a
//!   *previous* iteration, which a straight-line liveness walk cannot see.
//! - self-frame-reuse (`SelfTailCall` via `VarResolution::SelfFn`): the frame
//!   restarts at pc 0 with only the params rebound, so "after the call" is not
//!   the end of the slot's life.
//! - any form outside the core allowlist (modules, macros, eval/load, LLM
//!   constructors, ...): these lower to runtime calls whose evaluation order
//!   is not worth modeling — they never appear in hot lambdas.
//!
//! Slot-level opt-outs (the rest of the function still participates):
//!
//! - slots captured as upvalues by a nested lambda (`UpvalueDesc::ParentLocal`;
//!   transitive captures also surface as `ParentLocal` on a direct child, so
//!   scanning direct children covers all depths): an open upvalue cell aliases
//!   the stack slot, so the slot ref is never dead.
//! - `set!` targets: a store could resurrect a taken slot; not worth the
//!   dataflow to prove otherwise.
//!
//! Within an eligible function, a backward walk in reverse evaluation order
//! tracks the set of slots that may still be read ("live"). A `Var` load of a
//! slot not in the live set is the last use on every control-flow path from
//! that point, and is marked takeable. Branch joins (`if`) union both arms;
//! short-circuit forms (`and`/`or`) accumulate exactly like sequences (the
//! union of "continues" and "jumps out" is the accumulated set). Kills
//! (binding stores) are ignored — that only over-approximates liveness, which
//! is sound.

use std::collections::HashSet;

use crate::chunk::UpvalueDesc;
use crate::core_expr::{Expr, LambdaDef, ResolvedExpr, VarRef, VarResolution};

/// Identity of a `Var` load site: the address of the `VarRef` inside its
/// `ResolvedExpr::Var` node. The resolved tree is borrowed (never moved)
/// between this analysis and `Compiler::compile_var_load`, so addresses are
/// stable for the whole compilation of the lambda.
pub(crate) type LoadSite = usize;

pub(crate) fn load_site(vr: &VarRef) -> LoadSite {
    vr as *const VarRef as usize
}

/// Compute the set of `Var` load sites in `def`'s body that are the statically
/// last use of their (never-captured, never-`set!`) local slot. Returns an
/// empty set when the function opts out (see module docs).
pub(crate) fn takeable_loads(def: &LambdaDef<VarRef>) -> HashSet<LoadSite> {
    // Pre-scan: function-level bails + slot-level disqualification.
    let mut pre = PreScan {
        disqualified: HashSet::new(),
        bail: false,
    };
    for e in &def.body {
        pre.scan(e);
        if pre.bail {
            return HashSet::new();
        }
    }

    // Backward liveness walk.
    let mut a = Analysis {
        disqualified: pre.disqualified,
        live: HashSet::new(),
        takes: HashSet::new(),
        bail: false,
    };
    a.seq(&def.body);
    if a.bail {
        HashSet::new()
    } else {
        a.takes
    }
}

struct PreScan {
    disqualified: HashSet<u16>,
    bail: bool,
}

impl PreScan {
    fn scan(&mut self, e: &ResolvedExpr) {
        if self.bail {
            return;
        }
        match e {
            Expr::Const(_) | Expr::Quote(_) => {}
            Expr::Var(vr) => {
                // A self-recursive operator means the frame is reused in place
                // (SelfTailCall) — bail on the whole function.
                if matches!(vr.resolution, VarResolution::SelfFn) {
                    self.bail = true;
                }
            }
            Expr::Set(vr, val) => {
                if let VarResolution::Local { slot } = vr.resolution {
                    self.disqualified.insert(slot);
                }
                self.scan(val);
            }
            Expr::Lambda(inner) => {
                // Captured slots are aliased by upvalue cells — never takeable.
                // Transitive captures of OUR slots also appear as ParentLocal on
                // this direct child (upvalue chains are built per level), so we
                // don't descend into the child's body: its own Var nodes index
                // its own frame, not ours.
                for uv in &inner.upvalues {
                    if let UpvalueDesc::ParentLocal(slot) = uv {
                        self.disqualified.insert(*slot);
                    }
                }
            }
            Expr::If { test, then, else_ } => {
                self.scan(test);
                self.scan(then);
                self.scan(else_);
            }
            Expr::Begin(v)
            | Expr::And(v)
            | Expr::Or(v)
            | Expr::MakeList(v)
            | Expr::MakeVector(v) => {
                for e in v {
                    self.scan(e);
                }
            }
            Expr::MakeMap(pairs) => {
                for (k, v) in pairs {
                    self.scan(k);
                    self.scan(v);
                }
            }
            Expr::Call { func, args, .. } => {
                self.scan(func);
                for a in args {
                    self.scan(a);
                }
            }
            Expr::Define(_, val) | Expr::Throw(val) | Expr::Spanned(_, val) => self.scan(val),
            Expr::Let { bindings, body }
            | Expr::LetStar { bindings, body }
            | Expr::Letrec { bindings, body } => {
                for (_, init) in bindings {
                    self.scan(init);
                }
                for e in body {
                    self.scan(e);
                }
            }
            // Anything below opts the whole function out (module docs).
            Expr::Try { .. }
            | Expr::Do(_)
            | Expr::Defmacro { .. }
            | Expr::DefineRecordType { .. }
            | Expr::Module { .. }
            | Expr::Import { .. }
            | Expr::Load(_)
            | Expr::Eval(_)
            | Expr::Prompt(_)
            | Expr::Message { .. }
            | Expr::Deftool { .. }
            | Expr::Defagent { .. }
            | Expr::Delay(_)
            | Expr::Force(_)
            | Expr::Macroexpand(_) => {
                self.bail = true;
            }
        }
    }
}

struct Analysis {
    disqualified: HashSet<u16>,
    /// Slots that may still be read after the current program point.
    live: HashSet<u16>,
    takes: HashSet<LoadSite>,
    bail: bool,
}

impl Analysis {
    /// Process a sequence (evaluated left-to-right) in reverse.
    fn seq(&mut self, exprs: &[ResolvedExpr]) {
        for e in exprs.iter().rev() {
            self.expr(e);
        }
    }

    /// Process one expression in reverse evaluation order: `self.live` on
    /// entry describes the program point *after* the expression, and on exit
    /// describes the point *before* it.
    fn expr(&mut self, e: &ResolvedExpr) {
        if self.bail {
            return;
        }
        match e {
            Expr::Const(_) | Expr::Quote(_) => {}
            Expr::Var(vr) => {
                if let VarResolution::Local { slot } = vr.resolution {
                    if !self.live.contains(&slot) && !self.disqualified.contains(&slot) {
                        self.takes.insert(load_site(vr));
                    }
                    self.live.insert(slot);
                }
            }
            Expr::Spanned(_, inner) => self.expr(inner),
            Expr::If { test, then, else_ } => {
                // Both arms continue to the same join; a use inside an arm is
                // last iff the slot is dead at the join. Walk each arm from a
                // copy of the join's live set, then union.
                let join = self.live.clone();
                self.expr(then);
                let live_then = std::mem::replace(&mut self.live, join);
                self.expr(else_);
                self.live.extend(live_then);
                self.expr(test);
            }
            // `and`/`or` short-circuit to the end; backward accumulation over
            // the sequence is exactly the may-read-after set for both the
            // "continue" and "jump out" paths.
            Expr::Begin(v)
            | Expr::And(v)
            | Expr::Or(v)
            | Expr::MakeList(v)
            | Expr::MakeVector(v) => self.seq(v),
            Expr::MakeMap(pairs) => {
                for (k, v) in pairs.iter().rev() {
                    self.expr(v);
                    self.expr(k);
                }
            }
            Expr::Call { func, args, .. } => {
                // Evaluation order: func, then args left-to-right. A tail call
                // needs no special casing: the live set at this point already
                // describes what can run after the call on every path.
                self.seq(args);
                self.expr(func);
            }
            Expr::Set(_, val) => {
                // The store target is disqualified by the pre-scan; the kill is
                // deliberately ignored (over-approximates liveness — sound).
                self.expr(val);
            }
            Expr::Define(_, val) | Expr::Throw(val) => self.expr(val),
            Expr::Lambda(_) => {
                // Closure creation captures cells for its upvalues — all
                // disqualified slots — and its body indexes its own frame.
            }
            Expr::Let { bindings, body } | Expr::Letrec { bindings, body } => {
                // let: inits left-to-right, stores, then body.
                // letrec: nil-stores, then init/store pairs, then body.
                // Same backward shape for liveness (stores read nothing).
                self.seq(body);
                for (_, init) in bindings.iter().rev() {
                    self.expr(init);
                }
            }
            Expr::LetStar { bindings, body } => {
                // Sequential init/store pairs, then body.
                self.seq(body);
                for (_, init) in bindings.iter().rev() {
                    self.expr(init);
                }
            }
            // Unreachable: the pre-scan bails on all of these. Bail again
            // defensively so a future pre-scan change cannot silently leave
            // an unmodeled construct in the backward walk.
            Expr::Try { .. }
            | Expr::Do(_)
            | Expr::Defmacro { .. }
            | Expr::DefineRecordType { .. }
            | Expr::Module { .. }
            | Expr::Import { .. }
            | Expr::Load(_)
            | Expr::Eval(_)
            | Expr::Prompt(_)
            | Expr::Message { .. }
            | Expr::Deftool { .. }
            | Expr::Defagent { .. }
            | Expr::Delay(_)
            | Expr::Force(_)
            | Expr::Macroexpand(_) => {
                self.bail = true;
            }
        }
    }
}
