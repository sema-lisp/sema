use std::collections::HashSet;

use sema_core::{intern, resolve as resolve_spur, SemaError, Spur, Value};

use crate::chunk::{Chunk, ExceptionEntry, Function, UpvalueDesc};
use crate::core_expr::{DoLoop, LambdaDef, PromptEntry, ResolvedExpr, VarRef, VarResolution};
use crate::emit::Emitter;
use crate::opcodes::Op;

/// Result of compiling a top-level expression.
pub struct CompileResult {
    /// The top-level chunk to execute.
    pub chunk: Chunk,
    /// All compiled function templates (referenced by MakeClosure func_id).
    pub functions: Vec<Function>,
    /// Native function table: maps native_id (index) to global name Spur.
    /// Used by CallNative opcode for direct dispatch without env lookup.
    /// Empty when no known_natives were provided to the compiler.
    pub native_table: Vec<Spur>,
}

impl CompileResult {
    pub fn new(chunk: Chunk, functions: Vec<Function>) -> Self {
        CompileResult {
            chunk,
            functions,
            native_table: Vec::new(),
        }
    }
}

/// Maximum recursion depth for the compiler.
/// This prevents native stack overflow from deeply nested expressions.
const MAX_COMPILE_DEPTH: usize = 256;

/// Compile resolved expressions into bytecode.
///
/// - `n_locals`: pre-allocated top-level local slots (from resolver)
/// - `known_natives`: if provided, global calls to these names emit CallNative
///   for direct dispatch without env lookup at runtime
pub fn compile(
    exprs: &[ResolvedExpr],
    n_locals: u16,
    known_natives: Option<HashSet<Spur>>,
) -> Result<CompileResult, SemaError> {
    let mut compiler = match known_natives {
        Some(mut natives) => {
            // Remove any names that are (re)defined in this program —
            // user defines shadow the native, so CallNative would dispatch wrong.
            for expr in exprs {
                collect_defines(expr, &mut |spur| {
                    natives.remove(&spur);
                });
            }
            if natives.is_empty() {
                Compiler::new()
            } else {
                Compiler::with_known_natives(natives)
            }
        }
        None => Compiler::new(),
    };
    // Collect all names defined in this program — intrinsics must not fire for these.
    for expr in exprs {
        collect_defines(expr, &mut |spur| {
            compiler.redefined_globals.insert(spur);
        });
    }
    // Self-call fast-path eligibility: a top-level `(define (f ...))` may call
    // itself directly (CallSelf / SelfTailCall — no global lookup) only when
    // nothing else in the program can rebind `f`. A second `define` of the same
    // name or a global `set!` at any depth opts the name out — the same
    // program-wide redefinition rule the intrinsics use, extended to `set!`.
    {
        let mut seen: HashSet<Spur> = HashSet::new();
        for expr in exprs {
            scan_global_rebinds(expr, &mut |spur, is_set| {
                if is_set || !seen.insert(spur) {
                    compiler.self_rebound_globals.insert(spur);
                }
            });
        }
    }
    compiler.n_locals = n_locals;
    for (i, expr) in exprs.iter().enumerate() {
        compiler.compile_expr(expr)?;
        if i < exprs.len() - 1 {
            compiler.emit.emit_op(Op::Pop);
        }
    }
    if exprs.is_empty() {
        compiler.emit.emit_op(Op::Nil);
    }
    compiler.emit.emit_op(Op::Return);
    let (chunk, functions, native_table, _local_names, _local_scopes) = compiler.finish();
    Ok(CompileResult {
        chunk,
        functions,
        native_table,
    })
}

/// Walk resolved expressions and call `f` for every globally-defined name.
/// Used to exclude user-defined names from the CallNative optimization.
fn collect_defines(expr: &ResolvedExpr, f: &mut impl FnMut(Spur)) {
    match expr {
        ResolvedExpr::Define(spur, _) => f(*spur),
        ResolvedExpr::Begin(exprs) => {
            for e in exprs {
                collect_defines(e, f);
            }
        }
        ResolvedExpr::Spanned(_, inner) => collect_defines(inner, f),
        _ => {}
    }
}

/// Walk a resolved tree and report every site that (re)binds a global name:
/// `Define` nodes (`is_set = false`) and `set!` targets that resolved as Global
/// (`is_set = true`). Unlike `collect_defines` (the intrinsics' shallow
/// top-level walk), this descends into lambda, module, and binding bodies so
/// the self-call fast path is disabled by a rebind at any depth.
fn scan_global_rebinds(expr: &ResolvedExpr, f: &mut impl FnMut(Spur, bool)) {
    use ResolvedExpr as E;
    match expr {
        E::Const(_) | E::Quote(_) | E::Var(_) | E::DefineRecordType { .. } => {}
        E::Define(spur, val) => {
            f(*spur, false);
            scan_global_rebinds(val, f);
        }
        E::Set(vr, val) => {
            if let VarResolution::Global { spur } = vr.resolution {
                f(spur, true);
            }
            scan_global_rebinds(val, f);
        }
        E::Lambda(def) => {
            for e in &def.body {
                scan_global_rebinds(e, f);
            }
        }
        E::If { test, then, else_ } => {
            scan_global_rebinds(test, f);
            scan_global_rebinds(then, f);
            scan_global_rebinds(else_, f);
        }
        E::Begin(v) | E::And(v) | E::Or(v) | E::MakeList(v) | E::MakeVector(v) => {
            for e in v {
                scan_global_rebinds(e, f);
            }
        }
        E::Call { func, args, .. } => {
            scan_global_rebinds(func, f);
            for a in args {
                scan_global_rebinds(a, f);
            }
        }
        E::Let { bindings, body }
        | E::LetStar { bindings, body }
        | E::Letrec { bindings, body } => {
            for (_, init) in bindings {
                scan_global_rebinds(init, f);
            }
            for e in body {
                scan_global_rebinds(e, f);
            }
        }
        E::Do(do_loop) => {
            for v in &do_loop.vars {
                scan_global_rebinds(&v.init, f);
                if let Some(s) = &v.step {
                    scan_global_rebinds(s, f);
                }
            }
            scan_global_rebinds(&do_loop.test, f);
            for e in &do_loop.result {
                scan_global_rebinds(e, f);
            }
            for e in &do_loop.body {
                scan_global_rebinds(e, f);
            }
        }
        E::Try { body, handler, .. } => {
            for e in body {
                scan_global_rebinds(e, f);
            }
            for e in handler {
                scan_global_rebinds(e, f);
            }
        }
        E::Throw(val)
        | E::Load(val)
        | E::Eval(val)
        | E::Delay(val)
        | E::Force(val)
        | E::Macroexpand(val)
        | E::Spanned(_, val) => scan_global_rebinds(val, f),
        E::MakeMap(pairs) => {
            for (k, v) in pairs {
                scan_global_rebinds(k, f);
                scan_global_rebinds(v, f);
            }
        }
        E::Defmacro { body, .. } | E::Module { body, .. } => {
            for e in body {
                scan_global_rebinds(e, f);
            }
        }
        E::Import { path, .. } => scan_global_rebinds(path, f),
        E::Prompt(entries) => {
            for entry in entries {
                match entry {
                    PromptEntry::RoleContent { parts, .. } => {
                        for e in parts {
                            scan_global_rebinds(e, f);
                        }
                    }
                    PromptEntry::Expr(e) => scan_global_rebinds(e, f),
                }
            }
        }
        E::Message { role, parts } => {
            scan_global_rebinds(role, f);
            for e in parts {
                scan_global_rebinds(e, f);
            }
        }
        E::Deftool {
            description,
            parameters,
            handler,
            ..
        } => {
            scan_global_rebinds(description, f);
            scan_global_rebinds(parameters, f);
            scan_global_rebinds(handler, f);
        }
        E::Defagent { options, .. } => scan_global_rebinds(options, f),
    }
}

/// True iff `expr` is a lambda, peeking through span wrappers.
fn is_lambda_shaped(expr: &ResolvedExpr) -> bool {
    match expr {
        ResolvedExpr::Lambda(_) => true,
        ResolvedExpr::Spanned(_, inner) => is_lambda_shaped(inner),
        _ => false,
    }
}

/// Walk bytecode and add `offset` to all MakeClosure func_id operands.
fn patch_closure_func_ids(chunk: &mut Chunk, offset: u16) {
    let code = &mut chunk.code;
    let mut pc = 0;
    while pc < code.len() {
        let Some(op) = Op::from_u8(code[pc]) else {
            break;
        };
        match op {
            Op::MakeClosure => {
                // func_id is at pc+1..pc+3 (u16 LE)
                let old = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                let new = old + offset;
                let bytes = new.to_le_bytes();
                code[pc + 1] = bytes[0];
                code[pc + 2] = bytes[1];
                // n_upvalues at pc+3..pc+5
                let n_upvalues = u16::from_le_bytes([code[pc + 3], code[pc + 4]]) as usize;
                // Skip: op(1) + func_id(2) + n_upvalues(2) + n_upvalues * (is_local(2) + idx(2))
                pc += 1 + 2 + 2 + n_upvalues * 4;
            }
            // Variable-length instructions: skip op + operands
            Op::Const
            | Op::LoadLocal
            | Op::TakeLocal
            | Op::StoreLocal
            | Op::LoadUpvalue
            | Op::StoreUpvalue
            | Op::Call
            | Op::TailCall
            | Op::SelfTailCall
            | Op::CallSelf
            | Op::MakeList
            | Op::MakeVector
            | Op::MakeMap
            | Op::MakeHashMap => {
                pc += 1 + 2; // op + u16
            }
            Op::CallNative => {
                pc += 1 + 2 + 2; // op + u16 native_id + u16 argc
            }
            Op::StoreGlobal | Op::DefineGlobal => {
                pc += 1 + 4; // op + u32
            }
            Op::LoadGlobal => {
                pc += 1 + 4 + 2; // op + u32 spur + u16 cache_slot
            }
            Op::CallGlobal => {
                pc += 1 + 4 + 2 + 2; // op + u32 spur + u16 argc + u16 cache_slot
            }
            Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue => {
                pc += 1 + 4; // op + i32
            }
            // Single-byte instructions
            _ => {
                pc += 1;
            }
        }
    }
}

struct Compiler {
    emit: Emitter,
    functions: Vec<Function>,
    exception_entries: Vec<ExceptionEntry>,
    n_locals: u16,
    /// Current operand stack depth above locals (for exception handler stack restore).
    stack_height: u16,
    depth: usize,
    /// Set of global names known to be native functions (for CallNative optimization).
    known_natives: Option<HashSet<Spur>>,
    /// Native function table: maps native_id (index) → Spur. Built during compilation.
    native_table: Vec<Spur>,
    /// Reverse lookup: Spur → native_id for deduplication.
    native_id_map: hashbrown::HashMap<Spur, u16>,
    /// Next inline cache slot to allocate for LoadGlobal/CallGlobal instructions.
    next_cache_slot: u16,
    /// Global names that are (re)defined in this program — intrinsics must not
    /// be emitted for these since the user may have changed the binding.
    redefined_globals: HashSet<Spur>,
    /// Global names bound more than once (second `define`) or `set!` anywhere
    /// in this program — the self-call fast path must not fire for these.
    self_rebound_globals: HashSet<Spur>,
    /// The global name of the top-level define whose own lambda body this
    /// compiler is compiling, when that name is eligible for the self-call fast
    /// path. Calls to it compile to CallSelf / SelfTailCall (the running
    /// frame's closure IS the defined function). `None` for the top-level
    /// chunk and for nested lambdas, whose frames are not the defined function.
    self_global: Option<Spur>,
    /// Handoff slot from `compile_define` to `compile_lambda`: arms
    /// `self_global` on the inner compiler for the define's own lambda (the
    /// next lambda compiled, reached only through span wrappers).
    pending_self_global: Option<Spur>,
    /// `Var` load sites (addresses of the `VarRef` inside `ResolvedExpr::Var`
    /// nodes of the lambda body being compiled) proven by `takelocal.rs` to be
    /// the statically-last use of their never-captured local slot. These
    /// compile to `TakeLocal` (move) instead of `LoadLocal` (clone). Always
    /// empty for the top-level chunk, whose slots may outlive the chunk (REPL).
    take_loads: HashSet<crate::takelocal::LoadSite>,
    /// Local slot names for debugger scope inspection.
    local_names: Vec<(u16, Spur)>,
    /// Block scope `(slot, start_pc, end_pc)` of each block-introduced local, so
    /// the debugger can hide locals that are out of scope at the current pc.
    local_scopes: Vec<(u16, u32, u32)>,
}

type CompilerFinish = (
    Chunk,
    Vec<Function>,
    Vec<Spur>,
    Vec<(u16, Spur)>,
    Vec<(u16, u32, u32)>,
);

impl Compiler {
    fn new() -> Self {
        Compiler {
            emit: Emitter::new(),
            functions: Vec::new(),
            exception_entries: Vec::new(),
            n_locals: 0,
            stack_height: 0,
            depth: 0,
            known_natives: None,
            native_table: Vec::new(),
            native_id_map: hashbrown::HashMap::new(),
            next_cache_slot: 0,
            redefined_globals: HashSet::new(),
            self_rebound_globals: HashSet::new(),
            self_global: None,
            pending_self_global: None,
            take_loads: HashSet::new(),
            local_names: Vec::new(),
            local_scopes: Vec::new(),
        }
    }

    fn with_known_natives(known_natives: HashSet<Spur>) -> Self {
        let mut c = Self::new();
        c.known_natives = Some(known_natives);
        c
    }

    fn finish(self) -> CompilerFinish {
        let mut chunk = self.emit.into_chunk();
        chunk.n_locals = self.n_locals;
        chunk.exception_table = self.exception_entries;
        chunk.n_global_cache_slots = self.next_cache_slot;
        (
            chunk,
            self.functions,
            self.native_table,
            self.local_names,
            self.local_scopes,
        )
    }

    fn record_local_name(&mut self, vr: &VarRef) {
        if let VarResolution::Local { slot } = vr.resolution {
            if !self.local_names.iter().any(|(s, _)| *s == slot) {
                self.local_names.push((slot, vr.name));
            }
        }
    }

    /// Record the block scope (`[start_pc, end_pc)`) of each local introduced by
    /// a binding form, so the debugger only shows it while pc is in range.
    fn record_local_scopes(
        &mut self,
        bindings: &[(VarRef, ResolvedExpr)],
        start_pc: u32,
        end_pc: u32,
    ) {
        for (vr, _) in bindings {
            if let VarResolution::Local { slot } = vr.resolution {
                self.local_scopes.push((slot, start_pc, end_pc));
            }
        }
    }

    /// Allocate a cache slot and return its index.
    ///
    /// Cache slots are u16-indexed in the bytecode; errors on overflow rather
    /// than wrapping, which would alias two globals onto the same inline-cache
    /// slot and produce wrong cached dispatch (VM-7).
    fn alloc_cache_slot(&mut self) -> Result<u16, SemaError> {
        let slot = self.next_cache_slot;
        self.next_cache_slot = self.next_cache_slot.checked_add(1).ok_or_else(|| {
            SemaError::eval("inline-cache slot overflow: a single compilation unit cannot reference more than 65536 cached global sites")
        })?;
        Ok(slot)
    }

    /// Emit a LoadGlobal instruction with an inline cache slot.
    fn emit_load_global(&mut self, spur: Spur) -> Result<(), SemaError> {
        let cache_slot = self.alloc_cache_slot()?;
        self.emit.emit_op(Op::LoadGlobal);
        self.emit.emit_u32(spur_to_u32(spur));
        self.emit.emit_u16(cache_slot);
        Ok(())
    }

    /// Emit a CallGlobal instruction with an inline cache slot.
    fn emit_call_global(&mut self, spur: Spur, argc: u16) -> Result<(), SemaError> {
        let cache_slot = self.alloc_cache_slot()?;
        self.emit.emit_op(Op::CallGlobal);
        self.emit.emit_u32(spur_to_u32(spur));
        self.emit.emit_u16(argc);
        self.emit.emit_u16(cache_slot);
        Ok(())
    }

    /// Get or allocate a native_id for a given Spur.
    fn get_native_id(&mut self, spur: Spur) -> u16 {
        if let Some(&id) = self.native_id_map.get(&spur) {
            return id;
        }
        let id = self.native_table.len() as u16;
        self.native_table.push(spur);
        self.native_id_map.insert(spur, id);
        id
    }

    fn compile_expr(&mut self, expr: &ResolvedExpr) -> Result<(), SemaError> {
        self.depth += 1;
        if self.depth > MAX_COMPILE_DEPTH {
            self.depth -= 1;
            return Err(SemaError::eval("maximum compilation depth exceeded"));
        }
        // Track operand stack: every expression has a net effect of +1
        // (pushes exactly one result value). We save before and restore
        // the +1 after, so inner recursive calls accumulate correctly
        // for compile_try's stack_depth calculation.
        let result = self.compile_expr_inner(expr);
        self.depth -= 1;
        result
    }

    fn compile_expr_inner(&mut self, expr: &ResolvedExpr) -> Result<(), SemaError> {
        match expr {
            ResolvedExpr::Const(val) => self.compile_const(val),
            ResolvedExpr::Var(vr) => self.compile_var_load(vr),
            ResolvedExpr::If { test, then, else_ } => self.compile_if(test, then, else_),
            ResolvedExpr::Begin(exprs) => self.compile_begin(exprs),
            ResolvedExpr::Set(vr, val) => self.compile_set(vr, val),
            ResolvedExpr::Lambda(def) => self.compile_lambda(def),
            ResolvedExpr::Call { func, args, tail } => self.compile_call(func, args, *tail),
            ResolvedExpr::Define(spur, val) => self.compile_define(*spur, val),
            ResolvedExpr::Let { bindings, body } => self.compile_let(bindings, body),
            ResolvedExpr::LetStar { bindings, body } => self.compile_let_star(bindings, body),
            ResolvedExpr::Letrec { bindings, body } => self.compile_letrec(bindings, body),
            // ResolvedExpr::NamedLet removed — desugared to Letrec+Lambda in lowering
            ResolvedExpr::Do(do_loop) => self.compile_do(do_loop),
            ResolvedExpr::Try {
                body,
                catch_var,
                handler,
            } => self.compile_try(body, catch_var, handler),
            ResolvedExpr::Throw(val) => self.compile_throw(val),
            ResolvedExpr::And(exprs) => self.compile_and(exprs),
            ResolvedExpr::Or(exprs) => self.compile_or(exprs),
            ResolvedExpr::Quote(val) => self.compile_const(val),
            ResolvedExpr::MakeList(exprs) => self.compile_make_list(exprs),
            ResolvedExpr::MakeVector(exprs) => self.compile_make_vector(exprs),
            ResolvedExpr::MakeMap(pairs) => self.compile_make_map(pairs),
            ResolvedExpr::Defmacro {
                name,
                params,
                rest,
                body,
            } => self.compile_defmacro(*name, params, rest, body),
            ResolvedExpr::DefineRecordType {
                type_name,
                ctor_name,
                pred_name,
                field_names,
                field_specs,
            } => self.compile_define_record_type(
                *type_name,
                *ctor_name,
                *pred_name,
                field_names,
                field_specs,
            ),
            ResolvedExpr::Module {
                name,
                exports,
                body,
            } => self.compile_module(*name, exports, body),
            ResolvedExpr::Import { path, selective } => self.compile_import(path, selective),
            ResolvedExpr::Load(path) => self.compile_load(path),
            ResolvedExpr::Eval(expr) => self.compile_eval(expr),
            ResolvedExpr::Prompt(entries) => self.compile_prompt(entries),
            ResolvedExpr::Message { role, parts } => self.compile_message(role, parts),
            ResolvedExpr::Deftool {
                name,
                description,
                parameters,
                handler,
            } => self.compile_deftool(*name, description, parameters, handler),
            ResolvedExpr::Defagent { name, options } => self.compile_defagent(*name, options),
            ResolvedExpr::Delay(expr) => self.compile_delay(expr),
            ResolvedExpr::Force(expr) => self.compile_force(expr),
            ResolvedExpr::Macroexpand(expr) => self.compile_macroexpand(expr),
            ResolvedExpr::Spanned(span, inner) => {
                self.emit.emit_span(*span);
                self.compile_expr(inner)
            }
        }
    }

    // --- Constants ---

    fn compile_const(&mut self, val: &Value) -> Result<(), SemaError> {
        if val.is_nil() {
            self.emit.emit_op(Op::Nil);
        } else if val.as_bool() == Some(true) {
            self.emit.emit_op(Op::True);
        } else if val.as_bool() == Some(false) {
            self.emit.emit_op(Op::False);
        } else {
            self.emit.emit_const(val.clone())?;
        }
        Ok(())
    }

    // --- Variable access ---

    fn compile_var_load(&mut self, vr: &VarRef) -> Result<(), SemaError> {
        match vr.resolution {
            VarResolution::Local { slot } => {
                // Statically-last use of a never-captured slot: move the value
                // (TakeLocal) instead of cloning it, so a uniquely-owned value
                // can hit the stdlib's in-place COW fast paths. The slot reads
                // nil afterwards, which nothing can observe except the debug
                // inspector (documented in the bytecode format spec).
                if self.take_loads.contains(&crate::takelocal::load_site(vr)) {
                    self.emit.emit_op(Op::TakeLocal);
                    self.emit.emit_u16(slot);
                    return Ok(());
                }
                match slot {
                    0 => self.emit.emit_op(Op::LoadLocal0),
                    1 => self.emit.emit_op(Op::LoadLocal1),
                    2 => self.emit.emit_op(Op::LoadLocal2),
                    3 => self.emit.emit_op(Op::LoadLocal3),
                    _ => {
                        self.emit.emit_op(Op::LoadLocal);
                        self.emit.emit_u16(slot);
                    }
                }
            }
            VarResolution::Upvalue { index } => {
                self.emit.emit_op(Op::LoadUpvalue);
                self.emit.emit_u16(index);
            }
            VarResolution::Global { spur } => {
                self.emit_load_global(spur)?;
            }
            // SelfFn only ever appears as the operator of a tail `Call`, which
            // `compile_call` intercepts before reaching here. Loading it as a
            // value would mean the resolver mis-fired the self-tail-call opt.
            VarResolution::SelfFn => {
                return Err(SemaError::eval(
                    "internal: self-recursive reference used outside tail-call position",
                ));
            }
        }
        Ok(())
    }

    fn compile_var_store(&mut self, vr: &VarRef) {
        self.record_local_name(vr);
        match vr.resolution {
            VarResolution::Local { slot } => match slot {
                0 => self.emit.emit_op(Op::StoreLocal0),
                1 => self.emit.emit_op(Op::StoreLocal1),
                2 => self.emit.emit_op(Op::StoreLocal2),
                3 => self.emit.emit_op(Op::StoreLocal3),
                _ => {
                    self.emit.emit_op(Op::StoreLocal);
                    self.emit.emit_u16(slot);
                }
            },
            VarResolution::Upvalue { index } => {
                self.emit.emit_op(Op::StoreUpvalue);
                self.emit.emit_u16(index);
            }
            VarResolution::Global { spur } => {
                self.emit.emit_op(Op::StoreGlobal);
                self.emit.emit_u32(spur_to_u32(spur));
            }
            // The resolver disqualifies the self-tail-call opt when the loop name
            // is a `set!` target, so a SelfFn store is unreachable by construction.
            VarResolution::SelfFn => {
                unreachable!("self-recursive reference cannot be a set! target")
            }
        }
    }

    // --- Control flow ---

    fn compile_if(
        &mut self,
        test: &ResolvedExpr,
        then: &ResolvedExpr,
        else_: &ResolvedExpr,
    ) -> Result<(), SemaError> {
        // Peephole: (if (not X) then else) → compile X, JumpIfTrue to else
        // Avoids emitting NOT + JumpIfFalse, saving one opcode dispatch.
        // Fires exactly when the Not intrinsic would: a global `not` not
        // (re)defined in this program — a redefined `not` must dispatch to
        // the user's definition. Peeks through a span wrapper on the test
        // (the Not that would carry the span is never emitted).
        let bare_test = match test {
            ResolvedExpr::Spanned(_, inner) => inner.as_ref(),
            other => other,
        };
        if let ResolvedExpr::Call { func, args, .. } = bare_test {
            if args.len() == 1 {
                if let ResolvedExpr::Var(vr) = func.as_ref() {
                    if let VarResolution::Global { spur } = vr.resolution {
                        if resolve_spur(spur) == "not" && !self.redefined_globals.contains(&spur) {
                            self.compile_expr(&args[0])?;
                            let else_jump = self.emit.emit_jump(Op::JumpIfTrue);
                            self.compile_expr(then)?;
                            let end_jump = self.emit.emit_jump(Op::Jump);
                            self.emit.patch_jump(else_jump);
                            self.compile_expr(else_)?;
                            self.emit.patch_jump(end_jump);
                            return Ok(());
                        }
                    }
                }
            }
        }

        self.compile_expr(test)?;
        let else_jump = self.emit.emit_jump(Op::JumpIfFalse);
        self.compile_expr(then)?;
        let end_jump = self.emit.emit_jump(Op::Jump);
        self.emit.patch_jump(else_jump);
        self.compile_expr(else_)?;
        self.emit.patch_jump(end_jump);
        Ok(())
    }

    fn compile_begin(&mut self, exprs: &[ResolvedExpr]) -> Result<(), SemaError> {
        if exprs.is_empty() {
            self.emit.emit_op(Op::Nil);
            return Ok(());
        }
        for (i, expr) in exprs.iter().enumerate() {
            self.compile_expr(expr)?;
            if i < exprs.len() - 1 {
                self.emit.emit_op(Op::Pop);
                // compile_expr pushed +1, Pop removes it
            }
            // Last expr's value stays on stack (net +1 for the whole begin)
        }
        Ok(())
    }

    // --- Assignment ---

    fn compile_set(&mut self, vr: &VarRef, val: &ResolvedExpr) -> Result<(), SemaError> {
        self.compile_expr(val)?;
        self.emit.emit_op(Op::Dup); // set! returns the value
        self.compile_var_store(vr);
        Ok(())
    }

    fn compile_define(&mut self, spur: Spur, val: &ResolvedExpr) -> Result<(), SemaError> {
        // A define's own lambda may call itself directly — the running frame's
        // closure IS the defined function — when nothing in the program rebinds
        // the name. Hand the name to compile_lambda, which arms the self-call
        // fast path (CallSelf / SelfTailCall) on the lambda's own compiler.
        if is_lambda_shaped(val) && !self.self_rebound_globals.contains(&spur) {
            self.pending_self_global = Some(spur);
        }
        let result = self.compile_expr(val);
        self.pending_self_global = None;
        result?;
        self.emit.emit_op(Op::DefineGlobal);
        self.emit.emit_u32(spur_to_u32(spur));
        self.emit.emit_op(Op::Nil); // define returns nil
        Ok(())
    }

    // --- Lambda ---

    fn compile_lambda(&mut self, def: &LambdaDef<VarRef>) -> Result<(), SemaError> {
        // Compile the lambda body into a separate function. The redefinition
        // guard travels with it: intrinsics and the if-not peephole must
        // dispatch generically to a (re)defined global at any nesting depth,
        // not just in the top-level chunk.
        let mut inner = Compiler::new();
        inner.redefined_globals = self.redefined_globals.clone();
        inner.self_rebound_globals = self.self_rebound_globals.clone();
        // Arm the self-call fast path iff this lambda is the value of an
        // eligible top-level define (set by compile_define). `take()` keeps it
        // off nested lambdas — their running frames are not the defined
        // function, so a self-call there must stay a global call.
        inner.self_global = self.pending_self_global.take();
        // Last-use moves: a *tail* self-call under an armed self_global reuses
        // this frame in place (SelfTailCall), which the straight-line liveness
        // walk cannot see — skip the analysis for those functions (takelocal.rs
        // handles the resolver-elided SelfFn loops itself). Non-tail self-calls
        // (CallSelf) push a fresh frame and need no special treatment.
        let reuses_own_frame = match inner.self_global {
            Some(sg) => crate::takelocal::has_tail_self_call(&def.body, sg),
            None => false,
        };
        if !reuses_own_frame {
            inner.take_loads = crate::takelocal::takeable_loads(def);
        }
        inner.n_locals = def.n_locals;
        for (slot, &name) in def.params.iter().enumerate() {
            inner.local_names.push((slot as u16, name));
        }
        if let Some(rest) = def.rest {
            inner.local_names.push((def.params.len() as u16, rest));
        }

        // Compile body
        if def.body.is_empty() {
            inner.emit.emit_op(Op::Nil);
        } else {
            for (i, expr) in def.body.iter().enumerate() {
                inner.compile_expr(expr)?;
                if i < def.body.len() - 1 {
                    inner.emit.emit_op(Op::Pop);
                }
            }
        }
        inner.emit.emit_op(Op::Return);

        let func_id = self.functions.len() as u16;
        let (mut chunk, mut child_functions, _inner_natives, local_names, local_scopes) =
            inner.finish();

        // The inner compiler assigned func_ids starting from 0, but child functions
        // will be placed starting at func_id + 1 in our functions vec.
        // Patch all MakeClosure func_id operands in the inner chunk and child functions.
        let offset = func_id + 1;
        if offset > 0 && !child_functions.is_empty() {
            patch_closure_func_ids(&mut chunk, offset);
            for f in &mut child_functions {
                patch_closure_func_ids(&mut f.chunk, offset);
            }
        }

        let func = Function {
            name: def.name,
            chunk,
            upvalue_descs: def.upvalues.clone(),
            upvalue_names: def.upvalue_names.clone(),
            arity: def.params.len() as u16,
            has_rest: def.rest.is_some(),
            local_names,
            local_scopes,
            source_file: None,
            cache_offset: 0,
        };
        self.functions.push(func);
        self.functions.extend(child_functions);

        // Emit MakeClosure instruction
        let n_upvalues = def.upvalues.len() as u16;
        self.emit.emit_op(Op::MakeClosure);
        self.emit.emit_u16(func_id);
        self.emit.emit_u16(n_upvalues);

        // Emit upvalue descriptors inline
        for uv in &def.upvalues {
            match uv {
                UpvalueDesc::ParentLocal(slot) => {
                    self.emit.emit_u16(1); // is_local = true (using u16 for alignment)
                    self.emit.emit_u16(*slot);
                }
                UpvalueDesc::ParentUpvalue(idx) => {
                    self.emit.emit_u16(0); // is_local = false
                    self.emit.emit_u16(*idx);
                }
            }
        }

        Ok(())
    }

    // --- Function calls ---

    /// Try to compile a call to a known global as an inline opcode.
    /// Returns `true` if the intrinsic was emitted, `false` if not recognized.
    fn try_compile_intrinsic(
        &mut self,
        spur: Spur,
        args: &[ResolvedExpr],
    ) -> Result<bool, SemaError> {
        // Don't emit intrinsic opcodes for names that are (re)defined in this program
        if self.redefined_globals.contains(&spur) {
            return Ok(false);
        }
        let name = resolve_spur(spur);
        let argc = args.len();

        let op = match (name.as_str(), argc) {
            // Unary
            ("not", 1) => Op::Not,
            ("-", 1) => Op::Negate,
            // Binary arithmetic
            ("+", 2) => Op::AddInt,
            ("-", 2) => Op::SubInt,
            ("*", 2) => Op::MulInt,
            ("/", 2) => Op::Div,
            // Binary comparison
            ("<", 2) => Op::LtInt,
            (">", 2) => Op::Gt,
            ("<=", 2) => Op::Le,
            (">=", 2) => Op::Ge,
            ("=", 2) => Op::EqInt,
            // List operations
            ("car", 1) | ("first", 1) => Op::Car,
            ("cdr", 1) | ("rest", 1) => Op::Cdr,
            ("cons", 2) => Op::Cons,
            // Type predicates
            ("null?", 1) => Op::IsNull,
            ("pair?", 1) => Op::IsPair,
            ("list?", 1) => Op::IsList,
            ("number?", 1) => Op::IsNumber,
            ("string?", 1) => Op::IsString,
            ("symbol?", 1) => Op::IsSymbol,
            // Collection
            ("length", 1) => Op::Length,
            ("append", 2) => Op::Append,
            ("get", 2) => Op::Get,
            ("contains?", 2) => Op::ContainsQ,
            // Modulo
            ("mod", 2) | ("modulo", 2) => Op::Mod,
            // Indexed access
            ("nth", 2) => Op::Nth,
            // Mutable-array accessors. Only the exact arities are intrinsified:
            // the 3-arg (default) form of get and any wrong-arity call fall
            // through to the native, which owns the default logic and the
            // arity errors.
            ("mutable-array/get", 2) => Op::MutArrGet,
            ("mutable-array/set!", 3) => Op::MutArrSet,
            // String operations (legacy Scheme names, Decision #24).
            // string-append is N-ary in stdlib; only the 2-arg case is
            // intrinsified (mirrors the Append precedent) — N-ary stays generic.
            ("string-length", 1) => Op::StringLength,
            ("string-ref", 2) => Op::StringRef,
            ("string-append", 2) => Op::StringAppend,
            _ => return Ok(false),
        };

        // Compile all arguments, tracking stack height for exception handlers.
        for arg in args {
            self.compile_expr(arg)?;
            self.stack_height += 1;
        }
        self.emit.emit_op(op);
        // Opcode consumes all args and produces 1 result.
        // stack_height tracks intermediate operands (for exception handler restore),
        // not including the final result — same convention as compile_call.
        self.stack_height -= argc as u16;
        Ok(true)
    }

    fn compile_call(
        &mut self,
        func: &ResolvedExpr,
        args: &[ResolvedExpr],
        tail: bool,
    ) -> Result<(), SemaError> {
        // Self-tail-call: the resolver elided the self upvalue and marked this
        // operator as the running closure (VarResolution::SelfFn). Emit
        // SelfTailCall, which reuses the current frame's own closure — no callee
        // is pushed onto the stack.
        if tail {
            if let ResolvedExpr::Var(vr) = func {
                if matches!(vr.resolution, VarResolution::SelfFn) {
                    for arg in args {
                        self.compile_expr(arg)?;
                        self.stack_height += 1;
                    }
                    let argc = args.len() as u16;
                    self.emit.emit_op(Op::SelfTailCall);
                    self.emit.emit_u16(argc);
                    self.stack_height -= argc;
                    return Ok(());
                }
            }
        }

        // Direct self-call: inside an eligible top-level define's own lambda, a
        // call to the defining name targets the running frame's own closure —
        // no global lookup, no callable dispatch. Tail calls reuse the frame
        // (SelfTailCall); non-tail calls push a frame directly (CallSelf).
        // Value (non-operator) references to the name stay LoadGlobal, so
        // identity and escape semantics are unchanged.
        if let ResolvedExpr::Var(vr) = func {
            if let VarResolution::Global { spur } = vr.resolution {
                if self.self_global == Some(spur) {
                    for arg in args {
                        self.compile_expr(arg)?;
                        self.stack_height += 1;
                    }
                    let argc = args.len() as u16;
                    self.emit
                        .emit_op(if tail { Op::SelfTailCall } else { Op::CallSelf });
                    self.emit.emit_u16(argc);
                    self.stack_height -= argc;
                    return Ok(());
                }
            }
        }

        // Intrinsic recognition: emit inline opcodes for known builtins.
        // This applies regardless of tail position since intrinsics don't create frames.
        if let ResolvedExpr::Var(vr) = func {
            if let VarResolution::Global { spur } = vr.resolution {
                if self.try_compile_intrinsic(spur, args)? {
                    return Ok(());
                }
            }
        }

        // Fused CALL_GLOBAL / CALL_NATIVE for non-tail calls to global functions.
        // Tail calls can't use this because these opcodes push a new frame
        // (tail calls need to reuse the current frame).
        if !tail {
            if let ResolvedExpr::Var(vr) = func {
                if let VarResolution::Global { spur } = vr.resolution {
                    // Compile arguments (each pushes 1 value)
                    for arg in args {
                        self.compile_expr(arg)?;
                        self.stack_height += 1;
                    }
                    let argc = args.len() as u16;

                    // If this global is a known native function, emit CallNative
                    // for direct dispatch (no env lookup at runtime).
                    if self
                        .known_natives
                        .as_ref()
                        .is_some_and(|s| s.contains(&spur))
                    {
                        let native_id = self.get_native_id(spur);
                        self.emit.emit_op(Op::CallNative);
                        self.emit.emit_u16(native_id);
                        self.emit.emit_u16(argc);
                    } else {
                        self.emit_call_global(spur, argc)?;
                    }
                    self.stack_height -= argc;
                    return Ok(());
                }
            }
        }

        // General path: compile function expression (pushes 1 value onto operand stack)
        self.compile_expr(func)?;
        self.stack_height += 1;
        // Compile arguments (each pushes 1 value)
        for arg in args {
            self.compile_expr(arg)?;
            self.stack_height += 1;
        }
        let argc = args.len() as u16;
        if tail {
            self.emit.emit_op(Op::TailCall);
        } else {
            self.emit.emit_op(Op::Call);
        }
        self.emit.emit_u16(argc);
        // CALL pops func + args, pushes 1 result. Net from our perspective:
        // we pushed (1 + argc) above, result is handled by our caller.
        self.stack_height -= 1 + argc;
        Ok(())
    }

    // --- Let forms ---

    fn compile_let(
        &mut self,
        bindings: &[(VarRef, ResolvedExpr)],
        body: &[ResolvedExpr],
    ) -> Result<(), SemaError> {
        // Compile all init expressions first. Each leaves its value on the
        // operand stack, so track stack_height: if a later init throws (e.g. a
        // `try` binding), the exception handler restores the stack to
        // `n_locals + stack_height` and must not discard the earlier inits
        // already pushed here. (Call-argument compilation tracks this the same
        // way; omitting it here corrupted the stack — see the dual-eval
        // `let_binding_throwing_try_*` regression tests.)
        for (_, init) in bindings {
            self.compile_expr(init)?;
            self.stack_height += 1;
        }
        // Store into local slots (in reverse to match stack order)
        for (vr, _) in bindings.iter().rev() {
            self.compile_var_store(vr);
            self.stack_height -= 1;
        }
        // Compile body; the bindings are in scope for its full pc range.
        let body_start = self.emit.current_pc();
        self.compile_begin(body)?;
        let body_end = self.emit.current_pc();
        self.record_local_scopes(bindings, body_start, body_end);
        Ok(())
    }

    fn compile_let_star(
        &mut self,
        bindings: &[(VarRef, ResolvedExpr)],
        body: &[ResolvedExpr],
    ) -> Result<(), SemaError> {
        // Sequential: compile init, store, next binding
        for (vr, init) in bindings {
            self.compile_expr(init)?;
            self.compile_var_store(vr);
        }
        let body_start = self.emit.current_pc();
        self.compile_begin(body)?;
        let body_end = self.emit.current_pc();
        self.record_local_scopes(bindings, body_start, body_end);
        Ok(())
    }

    fn compile_letrec(
        &mut self,
        bindings: &[(VarRef, ResolvedExpr)],
        body: &[ResolvedExpr],
    ) -> Result<(), SemaError> {
        // Initialize all slots to nil first
        for (vr, _) in bindings {
            self.emit.emit_op(Op::Nil);
            self.compile_var_store(vr);
        }
        // Then compile and assign each init
        for (vr, init) in bindings {
            self.compile_expr(init)?;
            self.compile_var_store(vr);
        }
        let body_start = self.emit.current_pc();
        self.compile_begin(body)?;
        let body_end = self.emit.current_pc();
        self.record_local_scopes(bindings, body_start, body_end);
        Ok(())
    }

    // compile_named_let removed — named-let is desugared to letrec+lambda in lowering (Decision #52).

    // --- Do loop ---

    fn compile_do(&mut self, do_loop: &DoLoop<VarRef>) -> Result<(), SemaError> {
        // 1. Compile init expressions and store to vars
        for var in &do_loop.vars {
            self.compile_expr(&var.init)?;
            self.compile_var_store(&var.name);
        }

        // 2. Loop top
        let loop_top = self.emit.current_pc();

        // 3. Compile test
        self.compile_expr(&do_loop.test)?;
        let exit_jump = self.emit.emit_jump(Op::JumpIfTrue);

        // 4. Compile loop body
        for expr in &do_loop.body {
            self.compile_expr(expr)?;
            self.emit.emit_op(Op::Pop);
        }

        // 5. Compile step expressions and update vars
        // First compile all step values, then store (to avoid using partially-updated vars)
        let mut step_vars = Vec::new();
        for var in &do_loop.vars {
            if let Some(step) = &var.step {
                self.compile_expr(step)?;
                step_vars.push(&var.name);
            }
        }
        // Store in reverse order (stack is LIFO)
        for vr in step_vars.iter().rev() {
            self.compile_var_store(vr);
        }

        // 6. Jump back to loop top
        self.emit.emit_op(Op::Jump);
        let jump_end_pc = self.emit.current_pc();
        let offset = loop_top as i32 - (jump_end_pc as i32 + 4);
        self.emit.emit_i32(offset);

        // 7. Exit: compile result expressions
        self.emit.patch_jump(exit_jump);
        if do_loop.result.is_empty() {
            self.emit.emit_op(Op::Nil);
        } else {
            for (i, expr) in do_loop.result.iter().enumerate() {
                self.compile_expr(expr)?;
                if i < do_loop.result.len() - 1 {
                    self.emit.emit_op(Op::Pop);
                }
            }
        }

        // The loop vars are in scope from the loop top through the result exprs.
        let do_end = self.emit.current_pc();
        for var in &do_loop.vars {
            if let VarResolution::Local { slot } = var.name.resolution {
                self.local_scopes.push((slot, loop_top, do_end));
            }
        }

        Ok(())
    }

    // --- Exception handling ---

    fn compile_try(
        &mut self,
        body: &[ResolvedExpr],
        catch_var: &VarRef,
        handler: &[ResolvedExpr],
    ) -> Result<(), SemaError> {
        let try_start = self.emit.current_pc();

        // Compile body
        self.compile_begin(body)?;
        let try_end = self.emit.current_pc();

        // Jump over handler on success
        let success_jump = self.emit.emit_jump(Op::Jump);

        let handler_pc = self.emit.current_pc();

        // The VM will push the caught error value onto the stack
        // Store it in the catch variable slot
        let catch_slot = match catch_var.resolution {
            VarResolution::Local { slot } => slot,
            _ => 0,
        };
        self.emit.emit_op(Op::StoreLocal);
        self.emit.emit_u16(catch_slot);

        // Compile handler body
        self.compile_begin(handler)?;

        self.emit.patch_jump(success_jump);

        // Add exception table entry
        // We need to modify the emitter's chunk directly — use a deferred approach
        // Store the exception entry data and apply after finish
        // Actually, the Emitter gives us into_chunk which we can modify.
        // Let's store exception entries separately and merge at finish.
        // For now, store in the compiler and merge.
        // We'll need to access the chunk... let's extend Emitter slightly or use a side vec.
        self.add_exception_entry(ExceptionEntry {
            try_start,
            try_end,
            handler_pc,
            // Restore stack to locals + any operand values pushed before the try.
            // Without this, unwinding from a callee frame would discard operand
            // values belonging to a surrounding expression (e.g., a function being
            // called with the try result as an argument).
            stack_depth: self.n_locals + self.stack_height,
            catch_slot,
        });

        Ok(())
    }

    fn add_exception_entry(&mut self, entry: ExceptionEntry) {
        // We'll store these and apply when finishing the chunk
        // For now, emit directly into the emitter's chunk
        // Since Emitter doesn't expose this, we use a workaround:
        // Store entries in Compiler and merge in finish_chunk
        self.exception_entries.push(entry);
    }

    fn compile_throw(&mut self, val: &ResolvedExpr) -> Result<(), SemaError> {
        self.compile_expr(val)?;
        self.emit.emit_op(Op::Throw);
        Ok(())
    }

    // --- Short-circuit boolean ---

    fn compile_and(&mut self, exprs: &[ResolvedExpr]) -> Result<(), SemaError> {
        if exprs.is_empty() {
            self.emit.emit_op(Op::True);
            return Ok(());
        }

        let mut jumps = Vec::new();
        for (i, expr) in exprs.iter().enumerate() {
            self.compile_expr(expr)?;
            if i < exprs.len() - 1 {
                // Dup so the value is preserved if we short-circuit
                self.emit.emit_op(Op::Dup);
                let jump = self.emit.emit_jump(Op::JumpIfFalse);
                jumps.push(jump);
                self.emit.emit_op(Op::Pop); // discard the dup'd value (continuing)
            }
        }
        let end_jump = self.emit.emit_jump(Op::Jump);
        // Short-circuit target: the dup'd falsy value is on the stack
        for jump in jumps {
            self.emit.patch_jump(jump);
        }
        self.emit.patch_jump(end_jump);
        Ok(())
    }

    fn compile_or(&mut self, exprs: &[ResolvedExpr]) -> Result<(), SemaError> {
        if exprs.is_empty() {
            self.emit.emit_op(Op::False);
            return Ok(());
        }

        let mut jumps = Vec::new();
        for (i, expr) in exprs.iter().enumerate() {
            self.compile_expr(expr)?;
            if i < exprs.len() - 1 {
                self.emit.emit_op(Op::Dup);
                let jump = self.emit.emit_jump(Op::JumpIfTrue);
                jumps.push(jump);
                self.emit.emit_op(Op::Pop);
            }
        }
        let end_jump = self.emit.emit_jump(Op::Jump);
        for jump in jumps {
            self.emit.patch_jump(jump);
        }
        self.emit.patch_jump(end_jump);
        Ok(())
    }

    // --- Data constructors ---

    fn compile_make_list(&mut self, exprs: &[ResolvedExpr]) -> Result<(), SemaError> {
        for expr in exprs {
            self.compile_expr(expr)?;
        }
        self.emit.emit_op(Op::MakeList);
        self.emit.emit_u16(exprs.len() as u16);
        Ok(())
    }

    fn compile_make_vector(&mut self, exprs: &[ResolvedExpr]) -> Result<(), SemaError> {
        for expr in exprs {
            self.compile_expr(expr)?;
        }
        self.emit.emit_op(Op::MakeVector);
        self.emit.emit_u16(exprs.len() as u16);
        Ok(())
    }

    fn compile_make_map(
        &mut self,
        pairs: &[(ResolvedExpr, ResolvedExpr)],
    ) -> Result<(), SemaError> {
        for (key, val) in pairs {
            self.compile_expr(key)?;
            self.compile_expr(val)?;
        }
        self.emit.emit_op(Op::MakeMap);
        self.emit.emit_u16(pairs.len() as u16);
        Ok(())
    }

    // --- Forms that delegate to runtime native calls ---
    // These forms cannot be fully compiled to bytecode because they need
    // access to the tree-walker (eval, macros, modules) or have complex
    // runtime semantics. They are compiled as calls to well-known global
    // functions that the VM/interpreter provides.

    fn compile_eval(&mut self, expr: &ResolvedExpr) -> Result<(), SemaError> {
        self.emit_runtime_call("__vm-eval", &[expr])
    }

    fn compile_load(&mut self, path: &ResolvedExpr) -> Result<(), SemaError> {
        self.emit_runtime_call("__vm-load", &[path])
    }

    fn compile_import(&mut self, path: &ResolvedExpr, selective: &[Spur]) -> Result<(), SemaError> {
        let sel_list: Vec<Value> = selective
            .iter()
            .map(|s| Value::symbol_from_spur(*s))
            .collect();
        self.emit_runtime_call_with_const("__vm-import", path, &Value::list(sel_list))
    }

    fn compile_module(
        &mut self,
        _name: Spur,
        exports: &[Spur],
        body: &[ResolvedExpr],
    ) -> Result<(), SemaError> {
        // Register the declared export list with the runtime so `import`
        // restricts the copied bindings to exactly these names (a bare define-
        // only module — no `module` form — exports everything). Emitted even for
        // an empty export list, which means "export nothing".
        let export_vals: Vec<Value> = exports
            .iter()
            .map(|s| Value::symbol_from_spur(*s))
            .collect();
        self.emit_load_global(intern("__vm-module-exports"))?;
        self.emit.emit_const(Value::list(export_vals))?;
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(1);
        self.emit.emit_op(Op::Pop);

        // Compile module body sequentially
        for (i, expr) in body.iter().enumerate() {
            self.compile_expr(expr)?;
            if i < body.len() - 1 {
                self.emit.emit_op(Op::Pop);
            }
        }
        if body.is_empty() {
            self.emit.emit_op(Op::Nil);
        }
        // Module result is the last body expression
        // Module registration is handled by the VM when it sees this was a module
        Ok(())
    }

    fn compile_defmacro(
        &mut self,
        name: Spur,
        params: &[Spur],
        rest: &Option<Spur>,
        body: &[ResolvedExpr],
    ) -> Result<(), SemaError> {
        // Defmacro at compile time — emit as a call to __vm-defmacro
        // For now, compile the body as a lambda and register it
        let param_vals: Vec<Value> = params.iter().map(|s| Value::symbol_from_spur(*s)).collect();
        self.emit_load_global(intern("__vm-defmacro"))?;
        self.emit.emit_const(Value::symbol_from_spur(name))?;
        self.emit.emit_const(Value::list(param_vals))?;
        if let Some(r) = rest {
            self.emit.emit_const(Value::symbol_from_spur(*r))?;
        } else {
            self.emit.emit_op(Op::Nil);
        }
        // Compile body as a begin
        self.compile_begin(body)?;
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(4);
        Ok(())
    }

    fn compile_define_record_type(
        &mut self,
        type_name: Spur,
        ctor_name: Spur,
        pred_name: Spur,
        field_names: &[Spur],
        field_specs: &[(Spur, Spur)],
    ) -> Result<(), SemaError> {
        // Emit as a call to __vm-define-record-type with all info as constants
        // Function must be pushed first (before args) to match VM calling convention
        self.emit_load_global(intern("__vm-define-record-type"))?;
        self.emit.emit_const(Value::symbol_from_spur(type_name))?;
        self.emit.emit_const(Value::symbol_from_spur(ctor_name))?;
        self.emit.emit_const(Value::symbol_from_spur(pred_name))?;
        let fields: Vec<Value> = field_names
            .iter()
            .map(|s| Value::symbol_from_spur(*s))
            .collect();
        self.emit.emit_const(Value::list(fields))?;
        let specs: Vec<Value> = field_specs
            .iter()
            .map(|(f, a)| {
                Value::list(vec![
                    Value::symbol_from_spur(*f),
                    Value::symbol_from_spur(*a),
                ])
            })
            .collect();
        self.emit.emit_const(Value::list(specs))?;
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(5);
        Ok(())
    }

    fn compile_prompt(&mut self, entries: &[PromptEntry<VarRef>]) -> Result<(), SemaError> {
        // Function must be pushed first (before args) to match VM calling convention
        self.emit_load_global(intern("__vm-prompt"))?;
        // Compile each prompt entry and build a list
        for entry in entries {
            match entry {
                PromptEntry::RoleContent { role, parts } => {
                    self.emit.emit_const(Value::string(role))?;
                    for part in parts {
                        self.compile_expr(part)?;
                    }
                    self.emit.emit_op(Op::MakeList);
                    self.emit.emit_u16(parts.len() as u16);
                    // Make a (role parts-list) pair
                    self.emit.emit_op(Op::MakeList);
                    self.emit.emit_u16(2);
                }
                PromptEntry::Expr(expr) => {
                    self.compile_expr(expr)?;
                }
            }
        }
        self.emit.emit_op(Op::MakeList);
        self.emit.emit_u16(entries.len() as u16);
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(1);
        Ok(())
    }

    fn compile_message(
        &mut self,
        role: &ResolvedExpr,
        parts: &[ResolvedExpr],
    ) -> Result<(), SemaError> {
        self.emit_load_global(intern("__vm-message"))?;
        self.compile_expr(role)?;
        for part in parts {
            self.compile_expr(part)?;
        }
        self.emit.emit_op(Op::MakeList);
        self.emit.emit_u16(parts.len() as u16);
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(2);
        Ok(())
    }

    fn compile_deftool(
        &mut self,
        name: Spur,
        description: &ResolvedExpr,
        parameters: &ResolvedExpr,
        handler: &ResolvedExpr,
    ) -> Result<(), SemaError> {
        self.emit_load_global(intern("__vm-deftool"))?;
        self.emit.emit_const(Value::symbol_from_spur(name))?;
        self.compile_expr(description)?;
        self.compile_expr(parameters)?;
        self.compile_expr(handler)?;
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(4);
        Ok(())
    }

    fn compile_defagent(&mut self, name: Spur, options: &ResolvedExpr) -> Result<(), SemaError> {
        self.emit_load_global(intern("__vm-defagent"))?;
        self.emit.emit_const(Value::symbol_from_spur(name))?;
        self.compile_expr(options)?;
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(2);
        Ok(())
    }

    fn compile_delay(&mut self, expr: &ResolvedExpr) -> Result<(), SemaError> {
        // Delay wraps expr in a zero-arg lambda (thunk)
        // The resolver already handles this if lowered as a lambda,
        // but if it comes through as Delay, compile as a call to __vm-delay
        self.emit_load_global(intern("__vm-delay"))?;
        self.compile_expr(expr)?;
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(1);
        Ok(())
    }

    fn compile_force(&mut self, expr: &ResolvedExpr) -> Result<(), SemaError> {
        self.emit_load_global(intern("__vm-force"))?;
        self.compile_expr(expr)?;
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(1);
        Ok(())
    }

    fn compile_macroexpand(&mut self, expr: &ResolvedExpr) -> Result<(), SemaError> {
        self.emit_load_global(intern("__vm-macroexpand"))?;
        self.compile_expr(expr)?;
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(1);
        Ok(())
    }

    // --- Helper: emit a call to a well-known runtime function ---

    fn emit_runtime_call(&mut self, name: &str, args: &[&ResolvedExpr]) -> Result<(), SemaError> {
        self.emit_load_global(intern(name))?;
        for arg in args {
            self.compile_expr(arg)?;
        }
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(args.len() as u16);
        Ok(())
    }

    fn emit_runtime_call_with_const(
        &mut self,
        name: &str,
        arg1: &ResolvedExpr,
        arg2: &Value,
    ) -> Result<(), SemaError> {
        self.emit_load_global(intern(name))?;
        self.compile_expr(arg1)?;
        self.emit.emit_const(arg2.clone())?;
        self.emit.emit_op(Op::Call);
        self.emit.emit_u16(2);
        Ok(())
    }
}

fn spur_to_u32(spur: Spur) -> u32 {
    spur.into_inner().get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::lower;
    use crate::resolve::resolve_with_locals;

    fn compile_str(input: &str) -> CompileResult {
        let val = sema_reader::read(input).unwrap();
        let core = lower(&val, None).unwrap();
        let (resolved, _) = resolve_with_locals(&core).unwrap();
        compile(&[resolved], 0, None).unwrap()
    }

    fn compile_many_str(input: &str) -> CompileResult {
        let vals = sema_reader::read_many(input).unwrap();
        let mut resolved = Vec::new();
        for val in &vals {
            let core = lower(val, None).unwrap();
            let (res, _) = resolve_with_locals(&core).unwrap();
            resolved.push(res);
        }
        compile(&resolved, 0, None).unwrap()
    }

    /// Extract just the opcode bytes from a chunk, skipping operands.
    fn extract_ops(chunk: &Chunk) -> Vec<Op> {
        let code = &chunk.code;
        let mut ops = Vec::new();
        let mut pc = 0;
        while pc < code.len() {
            let op = Op::from_u8(code[pc])
                .unwrap_or_else(|| panic!("invalid opcode byte {} at pc={}", code[pc], pc));
            ops.push(op);
            pc += 1;
            // Skip operands based on opcode
            match op {
                Op::Const
                | Op::LoadLocal
                | Op::TakeLocal
                | Op::StoreLocal
                | Op::LoadUpvalue
                | Op::StoreUpvalue
                | Op::Call
                | Op::TailCall
                | Op::SelfTailCall
                | Op::CallSelf
                | Op::MakeList
                | Op::MakeVector
                | Op::MakeMap
                | Op::MakeHashMap => pc += 2,
                Op::StoreGlobal | Op::DefineGlobal => pc += 4,
                Op::LoadGlobal => pc += 6, // u32 spur + u16 cache_slot
                Op::CallGlobal => pc += 8, // u32 spur + u16 argc + u16 cache_slot
                Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue => pc += 4,
                Op::CallNative => pc += 4,
                Op::MakeClosure => {
                    let func_id = u16::from_le_bytes([code[pc], code[pc + 1]]);
                    let n_upvalues = u16::from_le_bytes([code[pc + 2], code[pc + 3]]);
                    pc += 4;
                    pc += n_upvalues as usize * 4; // each upvalue is u16 + u16
                    let _ = func_id;
                }
                _ => {} // zero-operand opcodes
            }
        }
        ops
    }

    /// Read the i32 operand of a Jump/JumpIfFalse/JumpIfTrue at the given opcode PC.
    fn read_jump_offset(chunk: &Chunk, op_pc: usize) -> i32 {
        i32::from_le_bytes([
            chunk.code[op_pc + 1],
            chunk.code[op_pc + 2],
            chunk.code[op_pc + 3],
            chunk.code[op_pc + 4],
        ])
    }

    // --- Literal compilation ---

    #[test]
    fn test_compile_int_literal() {
        let result = compile_str("42");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::Const, Op::Return]);
        assert_eq!(result.chunk.consts[0], Value::int(42));
    }

    #[test]
    fn test_compile_nil() {
        let result = compile_str("()");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::Nil, Op::Return]);
    }

    #[test]
    fn test_compile_true_false() {
        let t = compile_str("#t");
        assert_eq!(extract_ops(&t.chunk), vec![Op::True, Op::Return]);

        let f = compile_str("#f");
        assert_eq!(extract_ops(&f.chunk), vec![Op::False, Op::Return]);
    }

    #[test]
    fn test_compile_string_literal() {
        let result = compile_str("\"hello\"");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::Const, Op::Return]);
        assert_eq!(result.chunk.consts[0].as_str(), Some("hello"));
    }

    // --- Variable access ---

    #[test]
    fn test_compile_global_var() {
        let result = compile_str("x");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::LoadGlobal, Op::Return]);
    }

    // --- Control flow ---

    #[test]
    fn test_compile_if() {
        let result = compile_str("(if #t 1 2)");
        let ops = extract_ops(&result.chunk);
        // TRUE, JumpIfFalse, CONST(1), Jump, CONST(2), RETURN
        assert_eq!(
            ops,
            vec![
                Op::True,
                Op::JumpIfFalse,
                Op::Const,
                Op::Jump,
                Op::Const,
                Op::Return
            ]
        );
    }

    #[test]
    fn test_compile_nested_if() {
        let result = compile_str("(if #t (if #f 1 2) 3)");
        let ops = extract_ops(&result.chunk);
        let jif_count = ops.iter().filter(|&&op| op == Op::JumpIfFalse).count();
        assert_eq!(jif_count, 2);
    }

    #[test]
    fn test_compile_begin() {
        let result = compile_str("(begin 1 2 3)");
        let ops = extract_ops(&result.chunk);
        // CONST(1), POP, CONST(2), POP, CONST(3), RETURN
        assert_eq!(
            ops,
            vec![
                Op::Const,
                Op::Pop,
                Op::Const,
                Op::Pop,
                Op::Const,
                Op::Return
            ]
        );
    }

    // --- Define ---

    #[test]
    fn test_compile_define() {
        let result = compile_str("(define x 42)");
        let ops = extract_ops(&result.chunk);
        // CONST(42), DefineGlobal, Nil, Return
        assert_eq!(ops, vec![Op::Const, Op::DefineGlobal, Op::Nil, Op::Return]);
    }

    // --- Lambda ---

    #[test]
    fn test_compile_lambda() {
        let result = compile_str("(lambda (x) x)");
        assert_eq!(result.functions.len(), 1);
        let func = &result.functions[0];
        assert_eq!(func.arity, 1);
        assert!(!func.has_rest);

        // Inner function: TakeLocal(0), Return — the identity read is x's
        // last use, so it moves the slot value instead of cloning it.
        let inner_ops = extract_ops(&func.chunk);
        assert_eq!(inner_ops, vec![Op::TakeLocal, Op::Return]);

        // Top-level: MakeClosure, Return
        let top_ops = extract_ops(&result.chunk);
        assert_eq!(top_ops, vec![Op::MakeClosure, Op::Return]);
    }

    #[test]
    fn test_compile_lambda_rest_param() {
        let result = compile_str("(lambda (x . rest) x)");
        assert_eq!(result.functions.len(), 1);
        let func = &result.functions[0];
        assert_eq!(func.arity, 1);
        assert!(func.has_rest);
    }

    // --- Calls: non-tail vs tail ---

    #[test]
    fn test_compile_non_tail_call() {
        // Top-level call is NOT in tail position of a lambda
        // Intrinsic builtins like + compile to inline opcodes
        let result = compile_str("(+ 1 2)");
        let ops = extract_ops(&result.chunk);
        // Const(1), Const(2), AddInt, Return
        assert_eq!(ops, vec![Op::Const, Op::Const, Op::AddInt, Op::Return]);
    }

    #[test]
    fn test_compile_tail_call() {
        let result = compile_str("(lambda () (f 1))");
        assert_eq!(result.functions.len(), 1);
        let inner_ops = extract_ops(&result.functions[0].chunk);
        // LoadGlobal(f), Const(1), TailCall(1), Return
        assert_eq!(
            inner_ops,
            vec![Op::LoadGlobal, Op::Const, Op::TailCall, Op::Return]
        );
        // Verify it's TailCall, NOT Call
        assert!(!inner_ops.contains(&Op::Call));
    }

    #[test]
    fn test_compile_non_tail_in_begin() {
        // (lambda () (f 1) (g 2)) — first call is NOT tail, second IS tail
        let result = compile_str("(lambda () (f 1) (g 2))");
        let inner_ops = extract_ops(&result.functions[0].chunk);
        // f call: Const, CallGlobal(f, 1), Pop  (non-tail uses fused CallGlobal)
        // g call: LoadGlobal, Const, TailCall, Return  (tail call can't use CallGlobal)
        assert_eq!(
            inner_ops,
            vec![
                Op::Const,
                Op::CallGlobal,
                Op::Pop,
                Op::LoadGlobal,
                Op::Const,
                Op::TailCall,
                Op::Return
            ]
        );
    }

    // --- Intrinsic recognition ---

    #[test]
    fn test_compile_intrinsic_sub() {
        let result = compile_str("(- 5 3)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::Const, Op::Const, Op::SubInt, Op::Return]);
    }

    #[test]
    fn test_compile_intrinsic_lt() {
        let result = compile_str("(< 1 2)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::Const, Op::Const, Op::LtInt, Op::Return]);
    }

    #[test]
    fn test_compile_intrinsic_not() {
        let result = compile_str("(not #t)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::True, Op::Not, Op::Return]);
    }

    #[test]
    fn test_compile_intrinsic_negate() {
        let result = compile_str("(- 5)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::Const, Op::Negate, Op::Return]);
    }

    #[test]
    fn test_compile_intrinsic_in_tail_position() {
        // Intrinsics work in tail position too (they don't create frames).
        // Both reads are last uses, so both move (TakeLocal).
        let result = compile_str("(lambda (x y) (+ x y))");
        let inner_ops = extract_ops(&result.functions[0].chunk);
        assert_eq!(
            inner_ops,
            vec![Op::TakeLocal, Op::TakeLocal, Op::AddInt, Op::Return]
        );
    }

    #[test]
    fn test_compile_non_intrinsic_still_uses_call_global() {
        // Non-intrinsic globals still use CallGlobal
        let result = compile_str("(foo 1 2)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::Const, Op::Const, Op::CallGlobal, Op::Return]);
    }

    #[test]
    fn test_compile_intrinsic_mut_arr_get() {
        let result = compile_str("(mutable-array/get a 0)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(
            ops,
            vec![Op::LoadGlobal, Op::Const, Op::MutArrGet, Op::Return]
        );
    }

    #[test]
    fn test_compile_intrinsic_mut_arr_set() {
        let result = compile_str("(mutable-array/set! a 0 1)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(
            ops,
            vec![
                Op::LoadGlobal,
                Op::Const,
                Op::Const,
                Op::MutArrSet,
                Op::Return
            ]
        );
    }

    #[test]
    fn test_compile_mut_arr_get_default_form_stays_call_global() {
        // The 3-arg (default) form is not intrinsified — the native owns the
        // default logic.
        let result = compile_str("(mutable-array/get a 0 :missing)");
        let ops = extract_ops(&result.chunk);
        assert!(
            !ops.contains(&Op::MutArrGet),
            "3-arg get must not intrinsify: {ops:?}"
        );
        assert!(
            ops.contains(&Op::CallGlobal),
            "3-arg get dispatches to the native: {ops:?}"
        );
    }

    #[test]
    fn test_compile_mut_arr_set_wrong_arity_stays_call_global() {
        // Wrong-arity calls fall through to the native so its arity error fires.
        let result = compile_str("(mutable-array/set! a 0)");
        let ops = extract_ops(&result.chunk);
        assert!(
            !ops.contains(&Op::MutArrSet),
            "2-arg set! must not intrinsify: {ops:?}"
        );
        assert!(
            ops.contains(&Op::CallGlobal),
            "2-arg set! dispatches to the native: {ops:?}"
        );
    }

    #[test]
    fn test_compile_mut_arr_intrinsics_guarded_by_redefinition() {
        // A program-level (re)define of the accessor disables the intrinsic —
        // calls dispatch to the user's definition.
        let result =
            compile_many_str("(define (mutable-array/get a i) :mine) (mutable-array/get x 0)");
        let ops = extract_ops(&result.chunk);
        assert!(
            !ops.contains(&Op::MutArrGet),
            "redefined mutable-array/get must not intrinsify: {ops:?}"
        );
        assert!(
            ops.contains(&Op::CallGlobal),
            "must dispatch to the user fn: {ops:?}"
        );
    }

    #[test]
    fn test_compile_if_not_peephole() {
        // (if (not x) then else) → compile x, JumpIfTrue
        let result = compile_str("(lambda (x) (if (not x) 1 2))");
        let inner_ops = extract_ops(&result.functions[0].chunk);
        // TakeLocal(x) — the test read is x's last use — JumpIfTrue,
        // Const(1), Jump, Const(2), Return
        assert_eq!(
            inner_ops,
            vec![
                Op::TakeLocal,
                Op::JumpIfTrue,
                Op::Const,
                Op::Jump,
                Op::Const,
                Op::Return
            ]
        );
        // Should NOT contain Not opcode
        assert!(!inner_ops.contains(&Op::Not));
    }

    #[test]
    fn test_compile_if_not_peephole_guarded_by_redefinition() {
        // A `not` (re)defined anywhere in the program disables the peephole:
        // the test dispatches to the user's `not` (generic global call) and
        // the if branches with JumpIfFalse.
        let result = compile_many_str("(define not (lambda (x) x)) (lambda (x) (if (not x) 1 2))");
        let if_fn = result
            .functions
            .iter()
            .find(|f| {
                let ops = extract_ops(&f.chunk);
                ops.contains(&Op::JumpIfFalse) || ops.contains(&Op::JumpIfTrue)
            })
            .expect("the lambda containing the if");
        let ops = extract_ops(&if_fn.chunk);
        assert!(
            !ops.contains(&Op::JumpIfTrue),
            "redefined `not` must not trigger the polarity peephole: {ops:?}"
        );
        assert!(
            ops.contains(&Op::CallGlobal),
            "the test must dispatch to the user-defined `not`: {ops:?}"
        );
        assert!(!ops.contains(&Op::Not), "no Not intrinsic either: {ops:?}");
    }

    // --- Let forms ---

    #[test]
    fn test_compile_let() {
        let result = compile_str("(lambda () (let ((x 1) (y 2)) x))");
        let inner_ops = extract_ops(&result.functions[0].chunk);
        // CONST(1), CONST(2), StoreLocal1(y=1), StoreLocal0(x=0),
        // TakeLocal(x=0) — the body read is x's last use — Return
        assert_eq!(
            inner_ops,
            vec![
                Op::Const,
                Op::Const,
                Op::StoreLocal1,
                Op::StoreLocal0,
                Op::TakeLocal,
                Op::Return
            ]
        );
    }

    #[test]
    fn test_compile_let_star() {
        // let* stores sequentially so later bindings see earlier ones
        let result = compile_str("(lambda () (let* ((x 1) (y x)) y))");
        let inner_ops = extract_ops(&result.functions[0].chunk);
        // CONST(1), StoreLocal0(x), TakeLocal(x), StoreLocal1(y),
        // TakeLocal(y), Return — both reads are last uses, so both move.
        assert_eq!(
            inner_ops,
            vec![
                Op::Const,
                Op::StoreLocal0,
                Op::TakeLocal,
                Op::StoreLocal1,
                Op::TakeLocal,
                Op::Return
            ]
        );
    }

    #[test]
    fn test_compile_letrec() {
        let result = compile_str("(lambda () (letrec ((x 1)) x))");
        let inner_ops = extract_ops(&result.functions[0].chunk);
        // Nil, StoreLocal0(x), CONST(1), StoreLocal0(x),
        // TakeLocal(x) — the body read is x's last use — Return
        assert_eq!(
            inner_ops,
            vec![
                Op::Nil,
                Op::StoreLocal0,
                Op::Const,
                Op::StoreLocal0,
                Op::TakeLocal,
                Op::Return
            ]
        );
    }

    // --- Set! ---

    #[test]
    fn test_compile_set_local() {
        let result = compile_str("(lambda (x) (set! x 42))");
        let inner_ops = extract_ops(&result.functions[0].chunk);
        // CONST(42), Dup, StoreLocal0(0), Return
        assert_eq!(
            inner_ops,
            vec![Op::Const, Op::Dup, Op::StoreLocal0, Op::Return]
        );
    }

    #[test]
    fn test_compile_set_global() {
        let result = compile_str("(set! x 42)");
        let ops = extract_ops(&result.chunk);
        // CONST(42), Dup, StoreGlobal, Return
        assert_eq!(ops, vec![Op::Const, Op::Dup, Op::StoreGlobal, Op::Return]);
    }

    #[test]
    fn test_compile_set_upvalue() {
        // Inner lambda sets outer variable
        let result = compile_str("(lambda (x) (lambda () (set! x 1)))");
        assert_eq!(result.functions.len(), 2);
        let inner_ops = extract_ops(&result.functions[1].chunk);
        // CONST(1), Dup, StoreUpvalue(0), Return
        assert_eq!(
            inner_ops,
            vec![Op::Const, Op::Dup, Op::StoreUpvalue, Op::Return]
        );
    }

    // --- Short-circuit boolean ---

    #[test]
    fn test_compile_and_empty() {
        let result = compile_str("(and)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::True, Op::Return]);
    }

    #[test]
    fn test_compile_or_empty() {
        let result = compile_str("(or)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::False, Op::Return]);
    }

    #[test]
    fn test_compile_and_short_circuit() {
        let result = compile_str("(and 1 2)");
        let ops = extract_ops(&result.chunk);
        // CONST(1), Dup, JumpIfFalse, Pop, CONST(2), Jump, Return
        assert_eq!(
            ops,
            vec![
                Op::Const,
                Op::Dup,
                Op::JumpIfFalse,
                Op::Pop,
                Op::Const,
                Op::Jump,
                Op::Return
            ]
        );
    }

    #[test]
    fn test_compile_or_short_circuit() {
        let result = compile_str("(or 1 2)");
        let ops = extract_ops(&result.chunk);
        // CONST(1), Dup, JumpIfTrue, Pop, CONST(2), Jump, Return
        assert_eq!(
            ops,
            vec![
                Op::Const,
                Op::Dup,
                Op::JumpIfTrue,
                Op::Pop,
                Op::Const,
                Op::Jump,
                Op::Return
            ]
        );
    }

    // --- Data constructors ---

    #[test]
    fn test_compile_vector_literal() {
        let result = compile_str("[1 2 3]");
        let ops = extract_ops(&result.chunk);
        assert_eq!(
            ops,
            vec![Op::Const, Op::Const, Op::Const, Op::MakeVector, Op::Return]
        );
    }

    #[test]
    fn test_compile_quote() {
        let result = compile_str("'(1 2 3)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::Const, Op::Return]);
    }

    // --- Exception handling ---

    #[test]
    fn test_compile_throw() {
        let result = compile_str("(throw 42)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::Const, Op::Throw, Op::Return]);
    }

    #[test]
    fn test_compile_try_catch() {
        let result = compile_str("(lambda () (try (/ 1 0) (catch e e)))");
        let func = &result.functions[0];
        // Verify exception table
        assert_eq!(func.chunk.exception_table.len(), 1);
        let entry = &func.chunk.exception_table[0];
        assert!(entry.try_start < entry.try_end);
        assert!(entry.handler_pc > entry.try_end);
        assert!((entry.handler_pc as usize) < func.chunk.code.len());
        // Handler should store to catch slot then load it
        let ops = extract_ops(&func.chunk);
        assert!(ops.contains(&Op::StoreLocal)); // store caught error
                                                // The jump-over-handler should be present
        assert!(ops.contains(&Op::Jump));
    }

    // --- Closures ---

    #[test]
    fn test_compile_closure_with_upvalue() {
        let result = compile_str("(lambda (x) (lambda () x))");
        assert_eq!(result.functions.len(), 2);
        // Outer function: MakeClosure for inner, Return
        let outer = &result.functions[0];
        let outer_ops = extract_ops(&outer.chunk);
        assert!(outer_ops.contains(&Op::MakeClosure));
        // Inner function: LoadUpvalue(0), Return
        let inner = &result.functions[1];
        let inner_ops = extract_ops(&inner.chunk);
        assert_eq!(inner_ops, vec![Op::LoadUpvalue, Op::Return]);
        // Inner function's upvalue_descs should match
        assert_eq!(inner.upvalue_descs.len(), 1);
        assert!(matches!(
            inner.upvalue_descs[0],
            UpvalueDesc::ParentLocal(0)
        ));
    }

    #[test]
    fn test_compile_nested_lambda_func_ids() {
        // (lambda () (lambda () 1) (lambda () 2))
        // Outer is func 0, inner lambdas are func 1 and func 2
        let result = compile_str("(lambda () (lambda () 1) (lambda () 2))");
        assert_eq!(result.functions.len(), 3);
        // Verify each inner function compiles correctly
        let f1 = &result.functions[1];
        let f1_ops = extract_ops(&f1.chunk);
        assert_eq!(f1_ops, vec![Op::Const, Op::Return]);
        assert_eq!(f1.chunk.consts[0], Value::int(1));

        let f2 = &result.functions[2];
        let f2_ops = extract_ops(&f2.chunk);
        assert_eq!(f2_ops, vec![Op::Const, Op::Return]);
        assert_eq!(f2.chunk.consts[0], Value::int(2));

        // Verify the outer function has two MakeClosure instructions
        // with func_ids 1 and 2 (checking the raw bytes)
        let outer = &result.functions[0];
        let outer_ops = extract_ops(&outer.chunk);
        let mc_count = outer_ops
            .iter()
            .filter(|&&op| op == Op::MakeClosure)
            .count();
        assert_eq!(mc_count, 2);
    }

    // --- Do loop ---

    #[test]
    fn test_compile_do_loop() {
        let result = compile_str("(lambda () (do ((i 0 (+ i 1))) ((= i 10) i) (display i)))");
        let func = &result.functions[0];
        let ops = extract_ops(&func.chunk);
        // Must contain a backward Jump (negative offset) for the loop back-edge
        let jump_pcs: Vec<usize> = (0..func.chunk.code.len())
            .filter(|&pc| func.chunk.code[pc] == Op::Jump as u8)
            .collect();
        // Find the back-edge jump (should have a negative offset)
        let has_back_edge = jump_pcs
            .iter()
            .any(|&pc| read_jump_offset(&func.chunk, pc) < 0);
        assert!(has_back_edge, "do loop must have a backward jump");
        // Must have JumpIfTrue for the exit condition
        assert!(ops.contains(&Op::JumpIfTrue));
    }

    // --- Named let ---

    #[test]
    fn test_compile_named_let() {
        // Named let desugars to letrec+lambda, compiled as letrec with a closure
        let result = compile_str("(lambda () (let loop ((n 10)) (if (= n 0) n (loop (- n 1)))))");
        // Should have at least 2 functions: outer lambda + loop lambda
        assert!(result.functions.len() >= 2);
        // The outer function should contain MakeClosure (for the loop) + TailCall (initial invocation in tail position)
        let outer = &result.functions[0];
        let outer_ops = extract_ops(&outer.chunk);
        assert!(outer_ops.contains(&Op::MakeClosure));
        assert!(outer_ops.contains(&Op::TailCall) || outer_ops.contains(&Op::Call));
    }

    #[test]
    fn test_compile_named_let_emits_self_tail_call() {
        // A self-tail-only loop compiles to SelfTailCall and captures no self
        // upvalue (issue #62): the loop function must not load a self upvalue.
        let result = compile_str("(let loop ((n 5)) (if (= n 0) n (loop (- n 1))))");
        let loop_fn = result
            .functions
            .iter()
            .find(|f| extract_ops(&f.chunk).contains(&Op::SelfTailCall))
            .expect("the loop function should emit SelfTailCall");
        let ops = extract_ops(&loop_fn.chunk);
        assert!(
            !ops.contains(&Op::LoadUpvalue),
            "loop must not load a self upvalue"
        );
        assert!(
            loop_fn.upvalue_descs.is_empty(),
            "loop function should capture nothing, got {:?}",
            loop_fn.upvalue_descs
        );
    }

    #[test]
    fn test_compile_named_let_escaping_keeps_upvalue() {
        // When the loop name escapes (passed to `list`), no SelfTailCall is
        // emitted — the loop still captures itself and self-calls via LoadUpvalue.
        let result = compile_str("(let loop ((n 5)) (if (= n 0) (list loop) (loop (- n 1))))");
        for f in &result.functions {
            assert!(
                !extract_ops(&f.chunk).contains(&Op::SelfTailCall),
                "escaping loop must not self-tail-call"
            );
        }
    }

    // --- CallSelf: direct self-call fast path for top-level defines ---

    fn find_fn<'a>(result: &'a CompileResult, name: &str) -> &'a Function {
        result
            .functions
            .iter()
            .find(|f| f.name == Some(intern(name)))
            .unwrap_or_else(|| panic!("no compiled function named `{name}`"))
    }

    #[test]
    fn test_compile_define_nontail_self_call_emits_call_self() {
        let result = compile_str("(define (f n) (if (< n 2) 1 (+ (f (- n 1)) n)))");
        let ops = extract_ops(&find_fn(&result, "f").chunk);
        assert!(
            ops.contains(&Op::CallSelf),
            "non-tail self-call must emit CallSelf: {ops:?}"
        );
        assert!(
            !ops.contains(&Op::CallGlobal),
            "self-call must not go through CallGlobal: {ops:?}"
        );
    }

    #[test]
    fn test_compile_define_tail_self_call_emits_self_tail_call() {
        let result = compile_str("(define (walk n) (if (= n 0) 'done (walk (- n 1))))");
        let ops = extract_ops(&find_fn(&result, "walk").chunk);
        assert!(
            ops.contains(&Op::SelfTailCall),
            "tail self-call must emit SelfTailCall: {ops:?}"
        );
        assert!(
            !ops.contains(&Op::TailCall) && !ops.contains(&Op::LoadGlobal),
            "tail self-call must not load the global: {ops:?}"
        );
    }

    #[test]
    fn test_compile_redefined_name_disables_self_call() {
        // A second define of the same name opts it out — every call dispatches
        // through the global so redefinition behaves exactly as before.
        let result = compile_many_str("(define (f n) (+ (f (- n 1)) 1)) (define (f n) 42)");
        for func in &result.functions {
            let ops = extract_ops(&func.chunk);
            assert!(
                !ops.contains(&Op::CallSelf) && !ops.contains(&Op::SelfTailCall),
                "redefined name must not self-call: {ops:?}"
            );
        }
    }

    #[test]
    fn test_compile_global_set_disables_self_call() {
        // A global set! anywhere in the program (here inside another function)
        // opts the name out of the self-call fast path.
        let result = compile_many_str("(define (f n) (+ (f (- n 1)) 1)) (define (g) (set! f 9))");
        for func in &result.functions {
            let ops = extract_ops(&func.chunk);
            assert!(
                !ops.contains(&Op::CallSelf) && !ops.contains(&Op::SelfTailCall),
                "set! name must not self-call: {ops:?}"
            );
        }
    }

    #[test]
    fn test_compile_mutual_recursion_stays_global() {
        // Calls to a DIFFERENT global never become self-calls.
        let result = compile_many_str("(define (f n) (g n)) (define (g n) (f n))");
        for func in &result.functions {
            let ops = extract_ops(&func.chunk);
            assert!(
                !ops.contains(&Op::CallSelf) && !ops.contains(&Op::SelfTailCall),
                "mutual recursion must stay on the global path: {ops:?}"
            );
        }
    }

    #[test]
    fn test_compile_nested_lambda_self_name_stays_global() {
        // A nested lambda's running frame is not `f`, so its call to f must
        // stay a global call even though the enclosing define is eligible.
        let result = compile_str("(define (f n) (+ 1 ((lambda (x) (f x)) n)))");
        for func in &result.functions {
            let ops = extract_ops(&func.chunk);
            assert!(
                !ops.contains(&Op::CallSelf) && !ops.contains(&Op::SelfTailCall),
                "nested lambda must not self-call the outer define: {ops:?}"
            );
        }
    }

    #[test]
    fn test_compile_value_reference_stays_load_global() {
        // Operator self-calls get CallSelf; a value (non-operator) reference to
        // the name stays LoadGlobal, preserving identity/escape semantics.
        let result = compile_str("(define (f n) (if (= n 0) f (car (list (f (- n 1))))))");
        let ops = extract_ops(&find_fn(&result, "f").chunk);
        assert!(
            ops.contains(&Op::CallSelf),
            "operator self-call must emit CallSelf: {ops:?}"
        );
        assert!(
            ops.contains(&Op::LoadGlobal),
            "value self-reference must stay LoadGlobal: {ops:?}"
        );
    }

    // --- TakeLocal: moving loads for statically-last uses ---

    #[test]
    fn test_take_local_last_use_call_arg() {
        // `m`'s only use is dead after the call — compiled as a move.
        let result = compile_str("(fn (m) (assoc m :k 1))");
        let ops = extract_ops(&result.functions[0].chunk);
        assert!(
            ops.contains(&Op::TakeLocal),
            "last use must emit TakeLocal: {ops:?}"
        );
        assert!(
            !ops.contains(&Op::LoadLocal0) && !ops.contains(&Op::LoadLocal),
            "no cloning load should remain for m: {ops:?}"
        );
    }

    #[test]
    fn test_take_local_only_at_last_use() {
        // First read stays a clone (the slot is read again later); only the
        // final read moves.
        let result = compile_str("(fn (m) (list (get m :a) (assoc m :k 1)))");
        let ops = extract_ops(&result.functions[0].chunk);
        assert_eq!(
            ops.iter().filter(|&&op| op == Op::TakeLocal).count(),
            1,
            "exactly one move: {ops:?}"
        );
        assert!(
            ops.contains(&Op::LoadLocal0),
            "earlier use must stay a cloning load: {ops:?}"
        );
        // The clone must come before the move.
        let load_pos = ops.iter().position(|&op| op == Op::LoadLocal0).unwrap();
        let take_pos = ops.iter().position(|&op| op == Op::TakeLocal).unwrap();
        assert!(load_pos < take_pos, "clone before move: {ops:?}");
    }

    #[test]
    fn test_take_local_in_both_branches() {
        // Mutually exclusive branches may each take the slot; the test read
        // before the branch stays a clone (both arms still read m).
        let result = compile_str("(fn (m) (if (null? m) m (assoc m :k 1)))");
        let ops = extract_ops(&result.functions[0].chunk);
        assert_eq!(
            ops.iter().filter(|&&op| op == Op::TakeLocal).count(),
            2,
            "each branch's use is last on its path: {ops:?}"
        );
        assert!(
            ops.contains(&Op::LoadLocal0),
            "the test read must stay a cloning load: {ops:?}"
        );
    }

    #[test]
    fn test_take_local_blocked_by_read_after_branch() {
        // The use inside the `if` is NOT last — the slot is read again after
        // the join. Only the final read moves.
        let result = compile_str("(fn (m) (list (if (contains? m :a) (get m :a) 1) m))");
        let ops = extract_ops(&result.functions[0].chunk);
        assert_eq!(
            ops.iter().filter(|&&op| op == Op::TakeLocal).count(),
            1,
            "only the post-join use is last: {ops:?}"
        );
        assert!(
            ops.contains(&Op::LoadLocal0),
            "in-branch use must stay a cloning load: {ops:?}"
        );
    }

    #[test]
    fn test_no_take_for_captured_slot() {
        // `m` is captured by the inner lambda: an upvalue cell aliases the
        // slot, so it must never be moved out.
        let result = compile_str("(fn (m) (list (fn () m) (assoc m :k 1)))");
        for f in &result.functions {
            let ops = extract_ops(&f.chunk);
            assert!(
                !ops.contains(&Op::TakeLocal),
                "captured slot must never be taken: {ops:?}"
            );
        }
    }

    #[test]
    fn test_no_take_for_set_target() {
        let result = compile_str("(fn (m) (set! m {:x 1}) (assoc m :k 2))");
        let ops = extract_ops(&result.functions[0].chunk);
        assert!(
            !ops.contains(&Op::TakeLocal),
            "set! target slot must never be taken: {ops:?}"
        );
    }

    #[test]
    fn test_no_take_in_function_with_try() {
        let result = compile_str("(fn (m) (try (assoc m :k 1) (catch e m)))");
        for f in &result.functions {
            let ops = extract_ops(&f.chunk);
            assert!(
                !ops.contains(&Op::TakeLocal),
                "a function containing try opts out entirely: {ops:?}"
            );
        }
    }

    #[test]
    fn test_no_take_in_function_with_do_loop() {
        let result = compile_str("(fn (m) (do ((i 0 (+ i 1))) ((= i 3) m) (get m i)))");
        for f in &result.functions {
            let ops = extract_ops(&f.chunk);
            assert!(
                !ops.contains(&Op::TakeLocal),
                "a function containing a do loop opts out entirely: {ops:?}"
            );
        }
    }

    #[test]
    fn test_no_take_in_self_tail_recursive_define() {
        // A tail self-call reuses the frame in place — the whole function
        // opts out of moves.
        let result = compile_str("(define (walk n) (if (= n 0) 'done (walk (- n 1))))");
        let ops = extract_ops(&find_fn(&result, "walk").chunk);
        assert!(ops.contains(&Op::SelfTailCall), "sanity: {ops:?}");
        assert!(
            !ops.contains(&Op::TakeLocal),
            "frame-reusing function must not take: {ops:?}"
        );
    }

    #[test]
    fn test_take_in_nontail_self_recursive_define() {
        // Non-tail self-calls push a fresh frame; takes stay enabled.
        let result = compile_str("(define (f n) (if (< n 2) 1 (+ (f (- n 1)) n)))");
        let ops = extract_ops(&find_fn(&result, "f").chunk);
        assert!(ops.contains(&Op::CallSelf), "sanity: {ops:?}");
        assert!(
            ops.contains(&Op::TakeLocal),
            "the final read of n is a move: {ops:?}"
        );
    }

    #[test]
    fn test_no_take_in_named_let_loop() {
        let result = compile_str("(let loop ((n 5) (m {})) (if (= n 0) m (loop (- n 1) m)))");
        for f in &result.functions {
            let ops = extract_ops(&f.chunk);
            assert!(
                !ops.contains(&Op::TakeLocal),
                "SelfFn loop frame is reused — no takes: {ops:?}"
            );
        }
    }

    #[test]
    fn test_no_take_in_top_level_chunk() {
        // Top-level locals may outlive the chunk (REPL/eval continuation):
        // the main chunk never takes.
        let result = compile_str("(let ((m {})) (assoc m :k 1))");
        let ops = extract_ops(&result.chunk);
        assert!(
            !ops.contains(&Op::TakeLocal),
            "top-level chunk must not take: {ops:?}"
        );
    }

    // --- compile_many ---

    #[test]
    fn test_compile_many_empty() {
        let result = compile(&[], 0, None).unwrap();
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::Nil, Op::Return]);
    }

    #[test]
    fn test_compile_many_multiple() {
        let result = compile_many_str("1 2 3");
        let ops = extract_ops(&result.chunk);
        // CONST(1), Pop, CONST(2), Pop, CONST(3), Return
        assert_eq!(
            ops,
            vec![
                Op::Const,
                Op::Pop,
                Op::Const,
                Op::Pop,
                Op::Const,
                Op::Return
            ]
        );
    }

    #[test]
    fn test_compile_many_single() {
        let result = compile_many_str("42");
        let ops = extract_ops(&result.chunk);
        // CONST(42), Return (no Pop)
        assert_eq!(ops, vec![Op::Const, Op::Return]);
    }

    // --- Calling convention: function must be below args ---

    #[test]
    fn test_calling_convention_runtime_call() {
        // (eval 42) compiles as: LoadGlobal(__vm-eval), CONST(42), Call(1)
        let result = compile_str("(eval 42)");
        let ops = extract_ops(&result.chunk);
        assert_eq!(ops, vec![Op::LoadGlobal, Op::Const, Op::Call, Op::Return]);
        // The first op must be LoadGlobal (function loaded first)
        assert_eq!(ops[0], Op::LoadGlobal);
    }

    // --- Map literal ---

    #[test]
    fn test_compile_map_literal() {
        let result = compile_str("{:a 1 :b 2}");
        let ops = extract_ops(&result.chunk);
        // key, val, key, val, MakeMap, Return
        assert_eq!(
            ops,
            vec![
                Op::Const,
                Op::Const,
                Op::Const,
                Op::Const,
                Op::MakeMap,
                Op::Return
            ]
        );
    }

    #[test]
    fn test_compile_depth_limit() {
        // Build deeply nested Begin(Begin(Begin(...Const(1)...))) bypassing lowering
        let mut expr = ResolvedExpr::Const(Value::int(1));
        for _ in 0..300 {
            expr = ResolvedExpr::Begin(vec![expr]);
        }
        let result = compile(&[expr], 0, None);
        let err = result.err().expect("expected compilation to fail");
        let msg = err.to_string();
        assert!(
            msg.contains("compilation depth"),
            "expected compilation depth error, got: {msg}"
        );
    }

    #[test]
    fn test_alloc_cache_slot_overflow_errors() {
        // VM-7: cache slots are u16; the allocator must error on overflow
        // instead of wrapping (which would alias two globals onto one slot).
        // The slot *count* (n_global_cache_slots) is also a u16, so at most
        // 65535 slots (indices 0..=65534) can be allocated; the 65536th must
        // error rather than wrap next_cache_slot back to 0.
        let mut c = Compiler::new();
        for expected in 0..u16::MAX {
            let slot = c.alloc_cache_slot().expect("first 65535 slots allocate");
            assert_eq!(slot, expected);
        }
        let err = c
            .alloc_cache_slot()
            .expect_err("65536th cache slot must overflow");
        assert!(
            err.to_string().contains("inline-cache slot overflow"),
            "unexpected error: {err}"
        );
    }
}
