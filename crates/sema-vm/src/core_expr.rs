use sema_core::{Span, Spur, Value};

/// How a variable reference was resolved by the resolver pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarResolution {
    /// Local variable in the current function frame.
    Local { slot: u16 },
    /// Captured variable from an enclosing function scope.
    Upvalue { index: u16 },
    /// Module-level / global binding.
    Global { spur: Spur },
    /// The currently-executing closure itself. Produced by the self-tail-call
    /// optimization for a self-recursive letrec/named-let binding whose name is
    /// referenced only as the operator of a tail call: instead of capturing the
    /// binding as an upvalue (which would form the CORE-2 self-reference cycle),
    /// the reference reads the running frame's own closure. Only ever appears as
    /// the `func` of a tail `Call`, which the compiler lowers to `SelfTailCall`.
    SelfFn,
}

/// A resolved variable reference (name preserved for debugging).
#[derive(Debug, Clone, Copy)]
pub struct VarRef {
    pub name: Spur,
    pub resolution: VarResolution,
}

// Re-export UpvalueDesc from chunk.rs — used by both LambdaDef and Function.
pub use crate::chunk::UpvalueDesc;

/// Unified expression type, generic over the variable binding representation.
///
/// - `Expr<Spur>` (aka `CoreExpr`): output of lowering, variables are interned names.
/// - `Expr<VarRef>` (aka `ResolvedExpr`): output of resolution, variables carry slot info.
#[derive(Debug, Clone)]
pub enum Expr<V> {
    /// Literal constant
    Const(Value),
    /// Variable reference
    Var(V),
    /// if-then-else
    If {
        test: Box<Expr<V>>,
        then: Box<Expr<V>>,
        else_: Box<Expr<V>>,
    },
    /// Sequence of expressions
    Begin(Vec<Expr<V>>),
    /// Variable mutation (set!)
    Set(V, Box<Expr<V>>),
    /// Closure creation
    Lambda(LambdaDef<V>),
    /// Function call (tail flag for TCO)
    Call {
        func: Box<Expr<V>>,
        args: Vec<Expr<V>>,
        tail: bool,
    },
    /// Variable definition (define) — always uses Spur (global name)
    Define(Spur, Box<Expr<V>>),
    /// Parallel binding
    Let {
        bindings: Vec<(V, Expr<V>)>,
        body: Vec<Expr<V>>,
    },
    /// Sequential binding
    LetStar {
        bindings: Vec<(V, Expr<V>)>,
        body: Vec<Expr<V>>,
    },
    /// Recursive binding
    Letrec {
        bindings: Vec<(V, Expr<V>)>,
        body: Vec<Expr<V>>,
    },
    // NamedLet removed — desugared to Letrec+Lambda in lowering (Decision #52)
    /// Do loop
    Do(DoLoop<V>),
    /// Try/catch
    Try {
        body: Vec<Expr<V>>,
        catch_var: V,
        handler: Vec<Expr<V>>,
    },
    /// Throw exception
    Throw(Box<Expr<V>>),
    /// Short-circuit and
    And(Vec<Expr<V>>),
    /// Short-circuit or
    Or(Vec<Expr<V>>),
    /// Quoted value (no evaluation)
    Quote(Value),
    /// List constructor (evaluate elements)
    MakeList(Vec<Expr<V>>),
    /// Vector constructor (evaluate elements)
    MakeVector(Vec<Expr<V>>),
    /// Map constructor (evaluate key-value pairs)
    MakeMap(Vec<(Expr<V>, Expr<V>)>),
    /// Macro definition (runtime)
    Defmacro {
        name: Spur,
        params: Vec<Spur>,
        rest: Option<Spur>,
        body: Vec<Expr<V>>,
    },
    /// Record type definition
    DefineRecordType {
        type_name: Spur,
        ctor_name: Spur,
        pred_name: Spur,
        field_names: Vec<Spur>,
        field_specs: Vec<(Spur, Spur)>,
    },
    /// Module declaration
    Module {
        name: Spur,
        exports: Vec<Spur>,
        body: Vec<Expr<V>>,
    },
    /// Import a module
    Import {
        path: Box<Expr<V>>,
        selective: Vec<Spur>,
    },
    /// Load a file in current env
    Load(Box<Expr<V>>),
    /// Dynamic eval
    Eval(Box<Expr<V>>),
    /// Prompt (LLM data constructor)
    Prompt(Vec<PromptEntry<V>>),
    /// Message (LLM data constructor)
    Message {
        role: Box<Expr<V>>,
        parts: Vec<Expr<V>>,
    },
    /// Tool definition (LLM)
    Deftool {
        name: Spur,
        description: Box<Expr<V>>,
        parameters: Box<Expr<V>>,
        handler: Box<Expr<V>>,
    },
    /// Agent definition (LLM)
    Defagent { name: Spur, options: Box<Expr<V>> },
    /// Delay (create thunk)
    Delay(Box<Expr<V>>),
    /// Force (evaluate thunk)
    Force(Box<Expr<V>>),
    /// Macroexpand
    Macroexpand(Box<Expr<V>>),
    /// Source location annotation (transparent to evaluation)
    Spanned(Span, Box<Expr<V>>),
}

/// Pre-resolution expression (variables are interned names).
pub type CoreExpr = Expr<Spur>;

/// Post-resolution expression (variables carry slot/upvalue/global info).
pub type ResolvedExpr = Expr<VarRef>;

/// A prompt entry: either a role-content form or an expression.
#[derive(Debug, Clone)]
pub enum PromptEntry<V> {
    RoleContent { role: String, parts: Vec<Expr<V>> },
    Expr(Expr<V>),
}

#[derive(Debug, Clone)]
pub struct LambdaDef<V> {
    pub name: Option<Spur>,
    pub params: Vec<Spur>,
    pub rest: Option<Spur>,
    pub body: Vec<Expr<V>>,
    pub upvalues: Vec<UpvalueDesc>,
    pub upvalue_names: Vec<Spur>,
    pub n_locals: u16,
}

#[derive(Debug, Clone)]
pub struct DoLoop<V> {
    pub vars: Vec<DoVar<V>>,
    pub test: Box<Expr<V>>,
    pub result: Vec<Expr<V>>,
    pub body: Vec<Expr<V>>,
}

#[derive(Debug, Clone)]
pub struct DoVar<V> {
    pub name: V,
    pub init: Expr<V>,
    pub step: Option<Expr<V>>,
}
