//! Bytecode decoding and the compilable-subset check.
//!
//! This pass answers one question: can this `Function`'s chunk be turned into
//! machine code that never touches a refcount and never has a side effect? It
//! decodes the chunk into [`Instr`]s, rejects any opcode outside the whitelist,
//! and computes the operand-stack depth at every instruction so the translator
//! can build Cranelift blocks without re-deriving control flow.
//!
//! Rejecting is always safe: the VM runs the function instead.

use std::collections::{BTreeMap, BTreeSet};

use sema_core::Value;
use sema_vm::{Function, Op};

/// One decoded instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instr {
    /// Byte offset of the opcode in the chunk.
    pub pc: u32,
    pub op: Op,
    /// First operand, widened. Slot / const index / argc, or a jump target pc.
    pub arg: u32,
    /// Byte offset of the following instruction.
    pub next_pc: u32,
}

/// Why a function cannot be compiled. Carried for `sema jit --stats` and tests;
/// nothing user-facing depends on the exact wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reject {
    /// An opcode outside the whitelist — a global load, a call, an allocation,
    /// anything that could touch the heap or have an effect.
    Opcode(Op),
    /// A `Const` operand that is not an immediate value (a string, a list, a
    /// bignum). Baking its bits into code would copy an `Rc` pointer.
    NonImmediateConst(u32),
    /// A jump whose target is not the start of a decoded instruction.
    UnalignedJump(u32),
    /// The chunk ends without a `Return`, or an operand runs past the end.
    Truncated(u32),
    /// Two paths reach the same instruction with different operand-stack depths.
    /// The whitelisted control flow never produces this; it is a guard against a
    /// future compiler change silently breaking the translator.
    StackMismatch { pc: u32, expected: u32, got: u32 },
    /// Operand-stack depth went negative, or exceeded [`MAX_STACK`].
    BadStackDepth(u32),
    /// The function reads a local slot the frame does not have.
    SlotOutOfRange(u32),
    /// More local slots than [`MAX_LOCALS`].
    TooManyLocals(u16),
    /// A self-call whose argument count differs from the function's arity. The
    /// VM raises an arity error for this; compiled code does not raise.
    SelfCallArity { argc: u32, arity: u16 },
}

/// Ceiling on the modelled operand stack. Compiled bodies are small; anything
/// larger is not worth the compile time.
pub const MAX_STACK: u32 = 64;

/// Ceiling on local slots, which become Cranelift variables.
pub const MAX_LOCALS: u16 = 64;

/// A chunk that passed the subset check, with everything the translator needs.
#[derive(Debug, Clone)]
pub struct Program {
    pub instrs: Vec<Instr>,
    /// Index into `instrs` for each instruction pc.
    pub index_of_pc: BTreeMap<u32, usize>,
    /// Instruction pcs that begin a basic block (jump targets).
    pub block_starts: BTreeSet<u32>,
    /// Operand-stack depth on entry to each instruction, by index.
    pub depth_at: Vec<u32>,
    /// Whether each instruction is reachable from the entry point. The
    /// translator skips the rest — Cranelift rejects a block that is created
    /// but never filled.
    pub reachable: Vec<bool>,
    /// Constant pool, already checked to be all-immediate at the indices used.
    pub consts: Vec<Value>,
    pub arity: usize,
    pub n_locals: u16,
    /// True when the body contains a non-tail self-call, which recurses on the
    /// machine stack and therefore needs the depth guard.
    pub has_self_call: bool,
}

/// Opcodes the translator implements. Everything else rejects.
///
/// The whitelist is the safety property, not an optimization: it admits no
/// opcode that can allocate, mutate a global, capture an upvalue, call anything
/// but the function itself, throw, or perform I/O. That is what makes a
/// half-finished compiled call safe to abandon and re-run in the VM.
fn is_whitelisted(op: Op) -> bool {
    use Op::*;
    matches!(
        op,
        Const
            | Nil
            | True
            | False
            | Pop
            | Dup
            | LoadLocal
            | StoreLocal
            | LoadLocal0
            | LoadLocal1
            | LoadLocal2
            | LoadLocal3
            | StoreLocal0
            | StoreLocal1
            | StoreLocal2
            | StoreLocal3
            | TakeLocal
            | Jump
            | JumpIfFalse
            | JumpIfTrue
            | Return
            | Add
            | Sub
            | Mul
            | Div
            | Negate
            | Not
            | Eq
            | Lt
            | Gt
            | Le
            | Ge
            | AddInt
            | SubInt
            | MulInt
            | LtInt
            | EqInt
            | Mod
            | CallSelf
            | SelfTailCall
    )
}

/// Operand width of a whitelisted opcode, in bytes after the opcode byte.
fn operand_width(op: Op) -> usize {
    use Op::*;
    match op {
        Const | LoadLocal | StoreLocal | TakeLocal | CallSelf | SelfTailCall => 2,
        Jump | JumpIfFalse | JumpIfTrue => 4,
        _ => 0,
    }
}

fn read_u16(code: &[u8], at: usize) -> Option<u16> {
    let bytes = code.get(at..at + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_i32(code: &[u8], at: usize) -> Option<i32> {
    let bytes = code.get(at..at + 4)?;
    Some(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Decode `func`'s chunk and check it against the compilable subset.
pub fn analyze(func: &Function) -> Result<Program, Reject> {
    let chunk = &func.chunk;
    let code = &chunk.code;

    if chunk.n_locals > MAX_LOCALS {
        return Err(Reject::TooManyLocals(chunk.n_locals));
    }
    // An exception table means a `try` in the body, whose opcodes are not
    // whitelisted anyway; reject early so the reason is the honest one.
    if !chunk.exception_table.is_empty() {
        return Err(Reject::Opcode(Op::Throw));
    }

    let mut instrs: Vec<Instr> = Vec::new();
    let mut index_of_pc: BTreeMap<u32, usize> = BTreeMap::new();
    let mut block_starts: BTreeSet<u32> = BTreeSet::new();
    let mut has_self_call = false;

    let mut pc = 0usize;
    while pc < code.len() {
        let op = Op::from_u8(code[pc]).ok_or(Reject::Truncated(pc as u32))?;
        if !is_whitelisted(op) {
            return Err(Reject::Opcode(op));
        }
        let width = operand_width(op);
        let next_pc = pc + 1 + width;
        if next_pc > code.len() {
            return Err(Reject::Truncated(pc as u32));
        }

        let arg = match op {
            Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue => {
                // Relative offsets are measured from the end of the instruction.
                let rel = read_i32(code, pc + 1).ok_or(Reject::Truncated(pc as u32))?;
                let target = next_pc as i64 + rel as i64;
                if target < 0 || target > code.len() as i64 {
                    return Err(Reject::UnalignedJump(pc as u32));
                }
                let target = target as u32;
                block_starts.insert(target);
                target
            }
            _ if width == 2 => read_u16(code, pc + 1).ok_or(Reject::Truncated(pc as u32))? as u32,
            _ => 0,
        };

        // A self-call with the wrong argument count is an arity error the VM
        // raises at runtime. Compiled code has no way to raise, so hand the
        // whole function back.
        if matches!(op, Op::CallSelf | Op::SelfTailCall) && arg != func.arity as u32 {
            return Err(Reject::SelfCallArity {
                argc: arg,
                arity: func.arity,
            });
        }
        if op == Op::CallSelf {
            has_self_call = true;
        }

        // A local slot must exist in the frame. `n_locals` covers parameters
        // and body locals alike.
        if matches!(op, Op::LoadLocal | Op::StoreLocal | Op::TakeLocal)
            && arg >= chunk.n_locals as u32
        {
            return Err(Reject::SlotOutOfRange(arg));
        }
        if let Some(fixed) = fixed_slot(op) {
            if fixed >= chunk.n_locals as u32 {
                return Err(Reject::SlotOutOfRange(fixed));
            }
        }

        // Constants are baked into the generated code as raw bits, so only
        // immediates may be referenced — an `Rc` pointer copied that way would
        // have no owner.
        if op == Op::Const {
            match chunk.consts.get(arg as usize) {
                Some(v) if v.is_immediate() => {}
                _ => return Err(Reject::NonImmediateConst(arg)),
            }
        }

        index_of_pc.insert(pc as u32, instrs.len());
        instrs.push(Instr {
            pc: pc as u32,
            op,
            arg,
            next_pc: next_pc as u32,
        });
        pc = next_pc;
    }

    if instrs.is_empty() {
        return Err(Reject::Truncated(0));
    }
    // Every jump target must land on an instruction boundary. A target one past
    // the end is never emitted by the compiler, but check rather than trust.
    for target in &block_starts {
        if !index_of_pc.contains_key(target) {
            return Err(Reject::UnalignedJump(*target));
        }
    }

    let (depth_at, reachable) = compute_depths(&instrs, &index_of_pc)?;

    // A self-tail-call re-enters at pc 0, so the entry instruction begins a
    // basic block whenever the body loops.
    if instrs.iter().any(|i| i.op == Op::SelfTailCall) {
        block_starts.insert(0);
    }
    // Drop targets the entry walk never reached; the translator only builds
    // blocks it will fill.
    block_starts.retain(|pc| index_of_pc.get(pc).is_some_and(|i| reachable[*i]));

    Ok(Program {
        instrs,
        index_of_pc,
        block_starts,
        depth_at,
        reachable,
        consts: chunk.consts.clone(),
        arity: func.arity as usize,
        n_locals: chunk.n_locals,
        has_self_call,
    })
}

/// The implicit slot of a zero-operand local opcode.
pub fn fixed_slot(op: Op) -> Option<u32> {
    use Op::*;
    match op {
        LoadLocal0 | StoreLocal0 => Some(0),
        LoadLocal1 | StoreLocal1 => Some(1),
        LoadLocal2 | StoreLocal2 => Some(2),
        LoadLocal3 | StoreLocal3 => Some(3),
        _ => None,
    }
}

/// Number of operand-stack entries an instruction pops and pushes.
///
/// Deliberately spelled out here rather than reused from `Op::stack_effect`:
/// the translator has to agree with *these* numbers, and the whitelist is small
/// enough that an explicit table is checkable by eye. A disagreement with the
/// VM would show up as a `StackMismatch` or a translator panic, both caught by
/// the equivalence tests.
fn effect(op: Op, arg: u32) -> (u32, u32) {
    use Op::*;
    match op {
        Const | Nil | True | False | Dup | LoadLocal | TakeLocal | LoadLocal0 | LoadLocal1
        | LoadLocal2 | LoadLocal3 => (0, 1),
        Pop | StoreLocal | StoreLocal0 | StoreLocal1 | StoreLocal2 | StoreLocal3 | JumpIfFalse
        | JumpIfTrue => (1, 0),
        Jump => (0, 0),
        Negate | Not => (1, 1),
        Add | Sub | Mul | Div | Eq | Lt | Gt | Le | Ge | AddInt | SubInt | MulInt | LtInt
        | EqInt | Mod => (2, 1),
        CallSelf => (arg, 1),
        // Both leave the frame: nothing flows past them.
        Return => (1, 0),
        SelfTailCall => (arg, 0),
        other => unreachable!("effect() called on non-whitelisted opcode {other:?}"),
    }
}

/// True when control does not fall through to the next instruction.
fn terminates(op: Op) -> bool {
    matches!(op, Op::Return | Op::Jump | Op::SelfTailCall)
}

/// Propagate operand-stack depth from the entry point through every reachable
/// instruction, and prove that all paths into an instruction agree.
fn compute_depths(
    instrs: &[Instr],
    index_of_pc: &BTreeMap<u32, usize>,
) -> Result<(Vec<u32>, Vec<bool>), Reject> {
    const UNVISITED: u32 = u32::MAX;
    let mut depth_at = vec![UNVISITED; instrs.len()];
    let mut work: Vec<(usize, u32)> = vec![(0, 0)];

    while let Some((idx, depth)) = work.pop() {
        if depth > MAX_STACK {
            return Err(Reject::BadStackDepth(instrs[idx].pc));
        }
        match depth_at[idx] {
            UNVISITED => depth_at[idx] = depth,
            seen if seen == depth => continue,
            seen => {
                return Err(Reject::StackMismatch {
                    pc: instrs[idx].pc,
                    expected: seen,
                    got: depth,
                })
            }
        }

        let instr = instrs[idx];
        let (pops, pushes) = effect(instr.op, instr.arg);
        if depth < pops {
            return Err(Reject::BadStackDepth(instr.pc));
        }
        let after = depth - pops + pushes;

        if matches!(instr.op, Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue) {
            let target = *index_of_pc
                .get(&instr.arg)
                .ok_or(Reject::UnalignedJump(instr.arg))?;
            work.push((target, after));
        }
        // A self-tail-call rebinds the parameters and re-enters at pc 0 with an
        // empty operand stack.
        if instr.op == Op::SelfTailCall {
            work.push((0, 0));
        }
        if !terminates(instr.op) {
            let fallthrough = *index_of_pc
                .get(&instr.next_pc)
                .ok_or(Reject::Truncated(instr.pc))?;
            work.push((fallthrough, after));
        }
    }

    let reachable: Vec<bool> = depth_at.iter().map(|d| *d != UNVISITED).collect();
    for d in depth_at.iter_mut() {
        if *d == UNVISITED {
            *d = 0;
        }
    }
    Ok((depth_at, reachable))
}
