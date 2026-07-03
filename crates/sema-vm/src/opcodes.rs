/// Bytecode opcodes for the Sema VM.
///
/// Stack-based: operands are pushed/popped from the value stack.
/// Variable-length encoding: opcode (1 byte) + operands (u16/u32/i32).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    // Constants & stack
    Const, // u16 const_index → push constants[i]
    Nil,   // push nil
    True,  // push #t
    False, // push #f
    Pop,   // discard TOS
    Dup,   // duplicate TOS

    // Locals (slot-addressed within call frame)
    LoadLocal,  // u16 slot → push locals[slot]
    StoreLocal, // u16 slot → locals[slot] = pop

    // Upvalues (captured variables from enclosing scopes)
    LoadUpvalue,  // u16 index → push upvalues[i].get()
    StoreUpvalue, // u16 index → upvalues[i].set(pop)

    // Globals (module-level bindings, keyed by Spur)
    LoadGlobal,   // u32 spur → push globals[spur]
    StoreGlobal,  // u32 spur → globals[spur] = pop
    DefineGlobal, // u32 spur → globals[spur] = pop (define, not set!)

    // Control flow
    Jump,        // i32 relative offset
    JumpIfFalse, // i32 relative offset (pop condition)
    JumpIfTrue,  // i32 relative offset (pop condition)

    // Function calls
    Call,     // u16 argc → call TOS-argc with argc args
    TailCall, // u16 argc → tail call (reuse frame)
    Return,   // return TOS

    // Closures
    MakeClosure, // u16 func_id, u16 n_upvalues, then n * (u16 is_local, u16 idx)

    // Native function call fast path — direct dispatch without env lookup
    CallNative, // u16 native_id, u16 argc

    // Data constructors
    MakeList,    // u16 n → pop n values, push list
    MakeVector,  // u16 n → pop n values, push vector
    MakeMap,     // u16 n_pairs → pop 2n values, push map
    MakeHashMap, // u16 n_pairs → pop 2n values, push hashmap

    // Exception handling
    Throw, // pop value, throw as exception

    // Generic arithmetic & comparison
    Add,
    Sub,
    Mul,
    Div,
    Negate,
    Not,
    Eq,
    Lt,
    Gt,
    Le,
    Ge,

    // Specialized int arithmetic (fast paths)
    AddInt,
    SubInt,
    MulInt,
    LtInt,
    EqInt,

    // Specialized zero-operand locals (most common slots)
    LoadLocal0,  // = 42
    LoadLocal1,  // = 43
    LoadLocal2,  // = 44
    LoadLocal3,  // = 45
    StoreLocal0, // = 46
    StoreLocal1, // = 47
    StoreLocal2, // = 48
    StoreLocal3, // = 49

    // Fused global call (LOAD_GLOBAL + CALL in one instruction)
    CallGlobal, // u32 spur, u16 argc → lookup global, call with argc args

    // Inline stdlib intrinsics (bypass CallGlobal overhead)
    Car,       // pop list, push first element (or nil if empty)
    Cdr,       // pop list, push rest (tail)
    Cons,      // pop head, pop tail → push new list
    IsNull,    // pop value, push #t if nil or empty list
    IsPair,    // pop value, push #t if non-empty list
    IsList,    // pop value, push #t if list
    IsNumber,  // pop value, push #t if int or float
    IsString,  // pop value, push #t if string
    IsSymbol,  // pop value, push #t if symbol
    Length,    // pop collection, push its length as int
    Append,    // pop two lists, push concatenated list (2-arg only)
    Get,       // pop map, pop key → push map[key] or nil
    ContainsQ, // pop map, pop key → push #t if key exists

    // Modulo (integer fast path)
    Mod, // pop a, pop b → push a % b

    // Vector/list indexed access (fast path for nth)
    Nth, // pop collection, pop index → push element

    // Inline string intrinsics (bypass CallGlobal overhead)
    StringLength, // pop string, push char count as int (1-arg only)
    StringRef,    // pop index, pop string → push char at index
    StringAppend, // pop two values, push concatenated string (2-arg only)

    // Self-recursive tail call — the callee is the current frame's own closure,
    // so no closure value is on the stack (contrast `TailCall`). Emitted by the
    // compiler for self-tail-only named-let / letrec loops whose self upvalue
    // was elided by the resolver (see `VarResolution::SelfFn`). Appended last to
    // keep all preceding opcode numbers stable for `.semac` compatibility.
    SelfTailCall, // u16 argc → tail call reusing the current frame's closure
}

impl Op {
    /// Convert a raw byte to an Op using a safe match.
    ///
    /// Adding a new variant to `Op` without adding it here will cause a
    /// compile-time error because of the `#[deny(unreachable_patterns)]` on
    /// the const-assertion match at the bottom of this block.
    pub fn from_u8(byte: u8) -> Option<Op> {
        match byte {
            0 => Some(Op::Const),
            1 => Some(Op::Nil),
            2 => Some(Op::True),
            3 => Some(Op::False),
            4 => Some(Op::Pop),
            5 => Some(Op::Dup),
            6 => Some(Op::LoadLocal),
            7 => Some(Op::StoreLocal),
            8 => Some(Op::LoadUpvalue),
            9 => Some(Op::StoreUpvalue),
            10 => Some(Op::LoadGlobal),
            11 => Some(Op::StoreGlobal),
            12 => Some(Op::DefineGlobal),
            13 => Some(Op::Jump),
            14 => Some(Op::JumpIfFalse),
            15 => Some(Op::JumpIfTrue),
            16 => Some(Op::Call),
            17 => Some(Op::TailCall),
            18 => Some(Op::Return),
            19 => Some(Op::MakeClosure),
            20 => Some(Op::CallNative),
            21 => Some(Op::MakeList),
            22 => Some(Op::MakeVector),
            23 => Some(Op::MakeMap),
            24 => Some(Op::MakeHashMap),
            25 => Some(Op::Throw),
            26 => Some(Op::Add),
            27 => Some(Op::Sub),
            28 => Some(Op::Mul),
            29 => Some(Op::Div),
            30 => Some(Op::Negate),
            31 => Some(Op::Not),
            32 => Some(Op::Eq),
            33 => Some(Op::Lt),
            34 => Some(Op::Gt),
            35 => Some(Op::Le),
            36 => Some(Op::Ge),
            37 => Some(Op::AddInt),
            38 => Some(Op::SubInt),
            39 => Some(Op::MulInt),
            40 => Some(Op::LtInt),
            41 => Some(Op::EqInt),
            42 => Some(Op::LoadLocal0),
            43 => Some(Op::LoadLocal1),
            44 => Some(Op::LoadLocal2),
            45 => Some(Op::LoadLocal3),
            46 => Some(Op::StoreLocal0),
            47 => Some(Op::StoreLocal1),
            48 => Some(Op::StoreLocal2),
            49 => Some(Op::StoreLocal3),
            50 => Some(Op::CallGlobal),
            51 => Some(Op::Car),
            52 => Some(Op::Cdr),
            53 => Some(Op::Cons),
            54 => Some(Op::IsNull),
            55 => Some(Op::IsPair),
            56 => Some(Op::IsList),
            57 => Some(Op::IsNumber),
            58 => Some(Op::IsString),
            59 => Some(Op::IsSymbol),
            60 => Some(Op::Length),
            61 => Some(Op::Append),
            62 => Some(Op::Get),
            63 => Some(Op::ContainsQ),
            64 => Some(Op::Mod),
            65 => Some(Op::Nth),
            66 => Some(Op::StringLength),
            67 => Some(Op::StringRef),
            68 => Some(Op::StringAppend),
            69 => Some(Op::SelfTailCall),
            _ => None,
        }
    }

    /// Static stack effect of this opcode.
    ///
    /// For variable-arity opcodes (`Call`, `TailCall`, `SelfTailCall`, `CallGlobal`,
    /// `CallNative`, `MakeList`, `MakeVector`, `MakeMap`, `MakeHashMap`) the caller
    /// must pass the decoded operand count (`argc` / `n` / `n_pairs`); for all other opcodes the
    /// `operand` argument is ignored — pass `0`.
    ///
    /// This is the single source of truth used by the bytecode verifier
    /// (`crate::serialize::validate_bytecode`) to prove stack balance before the
    /// VM is allowed to run deserialized bytecode through its unchecked
    /// `pop_unchecked` hot path. It must agree exactly with the pops/pushes the
    /// dispatch arms in `vm.rs` perform. The match is exhaustive: adding a new
    /// opcode without a case here fails to compile.
    pub fn stack_effect(self, operand: u16) -> StackEffect {
        use Op::*;
        let operand = operand as u32;
        match self {
            // 0 pops, 1 push — produce a value
            Const | Nil | True | False | Dup | LoadLocal | LoadUpvalue | LoadGlobal
            | LoadLocal0 | LoadLocal1 | LoadLocal2 | LoadLocal3 | MakeClosure => StackEffect {
                pops: 0,
                pushes: 1,
                exits_frame: false,
            },
            // 1 pop, 0 pushes — consume a value
            Pop | StoreLocal | StoreUpvalue | StoreGlobal | DefineGlobal | StoreLocal0
            | StoreLocal1 | StoreLocal2 | StoreLocal3 | JumpIfFalse | JumpIfTrue => StackEffect {
                pops: 1,
                pushes: 0,
                exits_frame: false,
            },
            // unconditional branch — no stack effect, depth flows to target
            Jump => StackEffect {
                pops: 0,
                pushes: 0,
                exits_frame: false,
            },
            // variable-arity calls
            Call => StackEffect {
                pops: operand + 1, // callee + args
                pushes: 1,
                exits_frame: false,
            },
            TailCall => StackEffect {
                pops: operand + 1, // callee + args
                pushes: 0,
                exits_frame: true,
            },
            SelfTailCall => StackEffect {
                pops: operand, // args only — the callee is the current frame's own closure
                pushes: 0,
                exits_frame: true,
            },
            CallGlobal | CallNative => StackEffect {
                pops: operand, // args only (callee resolved by id/spur)
                pushes: 1,
                exits_frame: false,
            },
            // variable-arity constructors
            MakeList | MakeVector => StackEffect {
                pops: operand,
                pushes: 1,
                exits_frame: false,
            },
            MakeMap | MakeHashMap => StackEffect {
                pops: operand * 2,
                pushes: 1,
                exits_frame: false,
            },
            // frame-exiting
            Return | Throw => StackEffect {
                pops: 1,
                pushes: 0,
                exits_frame: true,
            },
            // binary ops — 2 pops, 1 push
            Add | Sub | Mul | Div | Eq | Lt | Gt | Le | Ge | AddInt | SubInt | MulInt | LtInt
            | EqInt | Cons | Append | Get | ContainsQ | Mod | Nth | StringRef | StringAppend => {
                StackEffect {
                    pops: 2,
                    pushes: 1,
                    exits_frame: false,
                }
            }
            // unary ops — 1 pop, 1 push
            Negate | Not | Car | Cdr | Length | IsNull | IsPair | IsList | IsNumber | IsString
            | IsSymbol | StringLength => StackEffect {
                pops: 1,
                pushes: 1,
                exits_frame: false,
            },
        }
    }
}

/// Static description of an opcode's effect on the operand stack.
///
/// `pops` values are removed and `pushes` values are added (net change is
/// `pushes - pops`). `exits_frame` is true for opcodes that terminate the
/// current frame (`Return`, `TailCall`, `Throw`) — these have no fallthrough
/// successor in the control-flow graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackEffect {
    pub pops: u32,
    pub pushes: u32,
    pub exits_frame: bool,
}

// Compile-time assertion: every `Op` variant is covered by `from_u8`.
// If a new variant is added to the enum, this match will fail to compile
// because the new variant won't have a corresponding arm.
const _: () = {
    #[deny(unreachable_patterns)]
    fn _assert_all_ops_covered(op: Op) {
        match op {
            Op::Const => {}
            Op::Nil => {}
            Op::True => {}
            Op::False => {}
            Op::Pop => {}
            Op::Dup => {}
            Op::LoadLocal => {}
            Op::StoreLocal => {}
            Op::LoadUpvalue => {}
            Op::StoreUpvalue => {}
            Op::LoadGlobal => {}
            Op::StoreGlobal => {}
            Op::DefineGlobal => {}
            Op::Jump => {}
            Op::JumpIfFalse => {}
            Op::JumpIfTrue => {}
            Op::Call => {}
            Op::TailCall => {}
            Op::Return => {}
            Op::MakeClosure => {}
            Op::CallNative => {}
            Op::MakeList => {}
            Op::MakeVector => {}
            Op::MakeMap => {}
            Op::MakeHashMap => {}
            Op::Throw => {}
            Op::Add => {}
            Op::Sub => {}
            Op::Mul => {}
            Op::Div => {}
            Op::Negate => {}
            Op::Not => {}
            Op::Eq => {}
            Op::Lt => {}
            Op::Gt => {}
            Op::Le => {}
            Op::Ge => {}
            Op::AddInt => {}
            Op::SubInt => {}
            Op::MulInt => {}
            Op::LtInt => {}
            Op::EqInt => {}
            Op::LoadLocal0 => {}
            Op::LoadLocal1 => {}
            Op::LoadLocal2 => {}
            Op::LoadLocal3 => {}
            Op::StoreLocal0 => {}
            Op::StoreLocal1 => {}
            Op::StoreLocal2 => {}
            Op::StoreLocal3 => {}
            Op::CallGlobal => {}
            Op::Car => {}
            Op::Cdr => {}
            Op::Cons => {}
            Op::IsNull => {}
            Op::IsPair => {}
            Op::IsList => {}
            Op::IsNumber => {}
            Op::IsString => {}
            Op::IsSymbol => {}
            Op::Length => {}
            Op::Append => {}
            Op::Get => {}
            Op::ContainsQ => {}
            Op::Mod => {}
            Op::Nth => {}
            Op::StringLength => {}
            Op::StringRef => {}
            Op::StringAppend => {}
            Op::SelfTailCall => {}
        }
    }
};

/// Opcode constants for use in match patterns (avoids `Op::from_u8` overhead).
pub mod op {
    use super::Op;
    pub const CONST: u8 = Op::Const as u8;
    pub const NIL: u8 = Op::Nil as u8;
    pub const TRUE: u8 = Op::True as u8;
    pub const FALSE: u8 = Op::False as u8;
    pub const POP: u8 = Op::Pop as u8;
    pub const DUP: u8 = Op::Dup as u8;
    pub const LOAD_LOCAL: u8 = Op::LoadLocal as u8;
    pub const STORE_LOCAL: u8 = Op::StoreLocal as u8;
    pub const LOAD_UPVALUE: u8 = Op::LoadUpvalue as u8;
    pub const STORE_UPVALUE: u8 = Op::StoreUpvalue as u8;
    pub const LOAD_GLOBAL: u8 = Op::LoadGlobal as u8;
    pub const STORE_GLOBAL: u8 = Op::StoreGlobal as u8;
    pub const DEFINE_GLOBAL: u8 = Op::DefineGlobal as u8;
    pub const JUMP: u8 = Op::Jump as u8;
    pub const JUMP_IF_FALSE: u8 = Op::JumpIfFalse as u8;
    pub const JUMP_IF_TRUE: u8 = Op::JumpIfTrue as u8;
    pub const CALL: u8 = Op::Call as u8;
    pub const TAIL_CALL: u8 = Op::TailCall as u8;
    pub const RETURN: u8 = Op::Return as u8;
    pub const MAKE_CLOSURE: u8 = Op::MakeClosure as u8;
    pub const CALL_NATIVE: u8 = Op::CallNative as u8;
    pub const MAKE_LIST: u8 = Op::MakeList as u8;
    pub const MAKE_VECTOR: u8 = Op::MakeVector as u8;
    pub const MAKE_MAP: u8 = Op::MakeMap as u8;
    pub const MAKE_HASH_MAP: u8 = Op::MakeHashMap as u8;
    pub const THROW: u8 = Op::Throw as u8;
    pub const ADD: u8 = Op::Add as u8;
    pub const SUB: u8 = Op::Sub as u8;
    pub const MUL: u8 = Op::Mul as u8;
    pub const DIV: u8 = Op::Div as u8;
    pub const NEGATE: u8 = Op::Negate as u8;
    pub const NOT: u8 = Op::Not as u8;
    pub const EQ: u8 = Op::Eq as u8;
    pub const LT: u8 = Op::Lt as u8;
    pub const GT: u8 = Op::Gt as u8;
    pub const LE: u8 = Op::Le as u8;
    pub const GE: u8 = Op::Ge as u8;
    pub const ADD_INT: u8 = Op::AddInt as u8;
    pub const SUB_INT: u8 = Op::SubInt as u8;
    pub const MUL_INT: u8 = Op::MulInt as u8;
    pub const LT_INT: u8 = Op::LtInt as u8;
    pub const EQ_INT: u8 = Op::EqInt as u8;
    pub const LOAD_LOCAL0: u8 = Op::LoadLocal0 as u8;
    pub const LOAD_LOCAL1: u8 = Op::LoadLocal1 as u8;
    pub const LOAD_LOCAL2: u8 = Op::LoadLocal2 as u8;
    pub const LOAD_LOCAL3: u8 = Op::LoadLocal3 as u8;
    pub const STORE_LOCAL0: u8 = Op::StoreLocal0 as u8;
    pub const STORE_LOCAL1: u8 = Op::StoreLocal1 as u8;
    pub const STORE_LOCAL2: u8 = Op::StoreLocal2 as u8;
    pub const STORE_LOCAL3: u8 = Op::StoreLocal3 as u8;
    pub const CALL_GLOBAL: u8 = Op::CallGlobal as u8;
    pub const CAR: u8 = Op::Car as u8;
    pub const CDR: u8 = Op::Cdr as u8;
    pub const CONS: u8 = Op::Cons as u8;
    pub const IS_NULL: u8 = Op::IsNull as u8;
    pub const IS_PAIR: u8 = Op::IsPair as u8;
    pub const IS_LIST: u8 = Op::IsList as u8;
    pub const IS_NUMBER: u8 = Op::IsNumber as u8;
    pub const IS_STRING: u8 = Op::IsString as u8;
    pub const IS_SYMBOL: u8 = Op::IsSymbol as u8;
    pub const LENGTH: u8 = Op::Length as u8;
    pub const APPEND: u8 = Op::Append as u8;
    pub const GET: u8 = Op::Get as u8;
    pub const CONTAINS_Q: u8 = Op::ContainsQ as u8;
    pub const MOD: u8 = Op::Mod as u8;
    pub const NTH: u8 = Op::Nth as u8;
    pub const STRING_LENGTH: u8 = Op::StringLength as u8;
    pub const STRING_REF: u8 = Op::StringRef as u8;
    pub const STRING_APPEND: u8 = Op::StringAppend as u8;
    pub const SELF_TAIL_CALL: u8 = Op::SelfTailCall as u8;

    // Instruction sizes (opcode byte + operand bytes)
    /// Size of a bare opcode with no operands: 1
    pub const SIZE_OP: usize = 1;
    /// Size of an instruction with a u16 operand (e.g., CALL, LOAD_LOCAL): 1 + 2 = 3
    pub const SIZE_OP_U16: usize = 3;
    /// Size of an instruction with a u32 operand (e.g., STORE_GLOBAL, DEFINE_GLOBAL): 1 + 4 = 5
    #[allow(dead_code)]
    pub const SIZE_OP_U32: usize = 5;
    /// Size of LOAD_GLOBAL: 1 + u32 spur + u16 cache_slot = 7
    pub const SIZE_LOAD_GLOBAL: usize = 7;
    /// Size of CALL_GLOBAL: 1 + u32 spur + u16 argc + u16 cache_slot = 9
    pub const SIZE_CALL_GLOBAL: usize = 9;
    /// Size of CALL_NATIVE: 1 + u16 native_id + u16 argc = 5
    pub const SIZE_CALL_NATIVE: usize = 5;
}
