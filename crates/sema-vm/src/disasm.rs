use std::fmt::Write;

use sema_core::{bits_to_spur, resolve};

use crate::chunk::Chunk;
use crate::opcodes::Op;

fn read_u16(code: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([code[offset], code[offset + 1]])
}

fn read_u32(code: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        code[offset],
        code[offset + 1],
        code[offset + 2],
        code[offset + 3],
    ])
}

fn read_i32(code: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes([
        code[offset],
        code[offset + 1],
        code[offset + 2],
        code[offset + 3],
    ])
}

fn op_name(op: Op) -> &'static str {
    match op {
        Op::Const => "CONST",
        Op::Nil => "NIL",
        Op::True => "TRUE",
        Op::False => "FALSE",
        Op::Pop => "POP",
        Op::Dup => "DUP",
        Op::LoadLocal => "LOAD_LOCAL",
        Op::StoreLocal => "STORE_LOCAL",
        Op::LoadUpvalue => "LOAD_UPVALUE",
        Op::StoreUpvalue => "STORE_UPVALUE",
        Op::LoadGlobal => "LOAD_GLOBAL",
        Op::StoreGlobal => "STORE_GLOBAL",
        Op::DefineGlobal => "DEFINE_GLOBAL",
        Op::Jump => "JUMP",
        Op::JumpIfFalse => "JUMP_IF_FALSE",
        Op::JumpIfTrue => "JUMP_IF_TRUE",
        Op::Call => "CALL",
        Op::TailCall => "TAIL_CALL",
        Op::Return => "RETURN",
        Op::MakeClosure => "MAKE_CLOSURE",
        Op::CallNative => "CALL_NATIVE",
        Op::MakeList => "MAKE_LIST",
        Op::MakeVector => "MAKE_VECTOR",
        Op::MakeMap => "MAKE_MAP",
        Op::MakeHashMap => "MAKE_HASHMAP",
        Op::Throw => "THROW",
        Op::Add => "ADD",
        Op::Sub => "SUB",
        Op::Mul => "MUL",
        Op::Div => "DIV",
        Op::Negate => "NEGATE",
        Op::Not => "NOT",
        Op::Eq => "EQ",
        Op::Lt => "LT",
        Op::Gt => "GT",
        Op::Le => "LE",
        Op::Ge => "GE",
        Op::AddInt => "ADD_INT",
        Op::SubInt => "SUB_INT",
        Op::MulInt => "MUL_INT",
        Op::LtInt => "LT_INT",
        Op::EqInt => "EQ_INT",
        Op::LoadLocal0 => "LOAD_LOCAL_0",
        Op::LoadLocal1 => "LOAD_LOCAL_1",
        Op::LoadLocal2 => "LOAD_LOCAL_2",
        Op::LoadLocal3 => "LOAD_LOCAL_3",
        Op::StoreLocal0 => "STORE_LOCAL_0",
        Op::StoreLocal1 => "STORE_LOCAL_1",
        Op::StoreLocal2 => "STORE_LOCAL_2",
        Op::StoreLocal3 => "STORE_LOCAL_3",
        Op::CallGlobal => "CALL_GLOBAL",
        Op::Car => "CAR",
        Op::Cdr => "CDR",
        Op::Cons => "CONS",
        Op::IsNull => "IS_NULL",
        Op::IsPair => "IS_PAIR",
        Op::IsList => "IS_LIST",
        Op::IsNumber => "IS_NUMBER",
        Op::IsString => "IS_STRING",
        Op::IsSymbol => "IS_SYMBOL",
        Op::Length => "LENGTH",
        Op::Append => "APPEND",
        Op::Get => "GET",
        Op::ContainsQ => "CONTAINS_Q",
        Op::Mod => "MOD",
        Op::Nth => "NTH",
        Op::StringLength => "STRING_LENGTH",
        Op::StringRef => "STRING_REF",
        Op::StringAppend => "STRING_APPEND",
        Op::SelfTailCall => "SELF_TAIL_CALL",
        Op::CallSelf => "CALL_SELF",
        Op::TakeLocal => "TAKE_LOCAL",
        Op::MutArrGet => "MUT_ARR_GET",
        Op::MutArrSet => "MUT_ARR_SET",
    }
}

/// Produce a human-readable disassembly of a Chunk.
pub fn disassemble(chunk: &Chunk, name: Option<&str>) -> String {
    let mut out = String::new();
    let label = name.unwrap_or("<script>");
    writeln!(out, "== {label} ==").unwrap();

    let code = &chunk.code;
    let mut pc = 0usize;

    while pc < code.len() {
        let op_byte = code[pc];
        let op = match Op::from_u8(op_byte) {
            Some(op) => op,
            None => {
                writeln!(out, "{pc:04}  UNKNOWN({op_byte:#04x})").unwrap();
                pc += 1;
                continue;
            }
        };

        match op {
            Op::Const => {
                let idx = read_u16(code, pc + 1);
                let val = &chunk.consts[idx as usize];
                writeln!(out, "{pc:04}  {:<16} {idx:<4} ; {val}", op_name(op)).unwrap();
                pc += 3;
            }

            Op::LoadLocal | Op::TakeLocal | Op::StoreLocal | Op::LoadUpvalue | Op::StoreUpvalue => {
                let slot = read_u16(code, pc + 1);
                writeln!(out, "{pc:04}  {:<16} {slot}", op_name(op)).unwrap();
                pc += 3;
            }

            Op::LoadGlobal => {
                let spur_bits = read_u32(code, pc + 1);
                let cache_slot = read_u16(code, pc + 5);
                let spur = bits_to_spur(spur_bits);
                let name_str = resolve(spur);
                writeln!(
                    out,
                    "{pc:04}  {:<16} {spur_bits:<4} cache={cache_slot} ; {name_str}",
                    op_name(op)
                )
                .unwrap();
                pc += 7;
            }

            Op::StoreGlobal | Op::DefineGlobal => {
                let spur_bits = read_u32(code, pc + 1);
                let spur = bits_to_spur(spur_bits);
                let name_str = resolve(spur);
                writeln!(
                    out,
                    "{pc:04}  {:<16} {spur_bits:<4} ; {name_str}",
                    op_name(op)
                )
                .unwrap();
                pc += 5;
            }

            Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue => {
                let offset = read_i32(code, pc + 1);
                let target = (pc as i32 + 5 + offset) as u32;
                writeln!(
                    out,
                    "{pc:04}  {:<16} {offset:<4} ; -> {target:04}",
                    op_name(op)
                )
                .unwrap();
                pc += 5;
            }

            Op::Call | Op::TailCall | Op::SelfTailCall | Op::CallSelf => {
                let argc = read_u16(code, pc + 1);
                writeln!(out, "{pc:04}  {:<16} {argc}", op_name(op)).unwrap();
                pc += 3;
            }

            Op::CallNative => {
                let native_id = read_u16(code, pc + 1);
                let argc = read_u16(code, pc + 3);
                writeln!(
                    out,
                    "{pc:04}  {:<16} native={native_id} argc={argc}",
                    op_name(op)
                )
                .unwrap();
                pc += 5;
            }

            Op::MakeClosure => {
                let func_id = read_u16(code, pc + 1);
                let n_upvalues = read_u16(code, pc + 3);
                writeln!(
                    out,
                    "{pc:04}  {:<16} func={func_id} upvalues={n_upvalues}",
                    op_name(op)
                )
                .unwrap();
                pc += 5;
                for _ in 0..n_upvalues {
                    let is_local = read_u16(code, pc);
                    let idx = read_u16(code, pc + 2);
                    let kind = if is_local != 0 { "local" } else { "upvalue" };
                    writeln!(out, "        | {kind} {idx}").unwrap();
                    pc += 4;
                }
            }

            Op::CallGlobal => {
                let spur_bits = read_u32(code, pc + 1);
                let argc = read_u16(code, pc + 5);
                let cache_slot = read_u16(code, pc + 7);
                let spur = bits_to_spur(spur_bits);
                let name_str = resolve(spur);
                writeln!(
                    out,
                    "{pc:04}  {:<16} {spur_bits:<4} argc={argc} cache={cache_slot} ; {name_str}",
                    op_name(op)
                )
                .unwrap();
                pc += 9;
            }

            Op::MakeList | Op::MakeVector | Op::MakeMap | Op::MakeHashMap => {
                let count = read_u16(code, pc + 1);
                writeln!(out, "{pc:04}  {:<16} {count}", op_name(op)).unwrap();
                pc += 3;
            }

            // All zero-operand opcodes
            _ => {
                writeln!(out, "{pc:04}  {}", op_name(op)).unwrap();
                pc += 1;
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use sema_core::{intern, spur_to_bits, Value};

    use super::*;
    use crate::emit::Emitter;

    #[test]
    fn test_disassemble_simple() {
        let mut e = Emitter::new();
        e.emit_const(Value::int(1)).unwrap();
        e.emit_const(Value::int(2)).unwrap();
        e.emit_op(Op::AddInt);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("test"));
        assert!(output.contains("== test =="));
        assert!(output.contains("CONST"));
        assert!(output.contains("ADD_INT"));
        assert!(output.contains("RETURN"));
        assert!(output.contains("1"));
        assert!(output.contains("2"));
    }

    #[test]
    fn test_disassemble_self_tail_call() {
        // Regression: disasm must decode every real opcode (it delegates to the
        // canonical `Op::from_u8`). A missed opcode would decode as UNKNOWN and
        // desynchronize the pc walk (a Const after it reads a garbage index), so
        // a following CONST must still disassemble at the right offset.
        let mut e = Emitter::new();
        e.emit_const(Value::int(7)).unwrap();
        e.emit_op(Op::SelfTailCall);
        e.emit_u16(1);
        e.emit_const(Value::int(9)).unwrap();
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("stc"));
        assert!(output.contains("SELF_TAIL_CALL"), "got: {output}");
        assert!(!output.contains("UNKNOWN"), "misaligned decode: {output}");
        // The Const after SelfTailCall must decode at the right offset (value 9).
        assert!(output.contains("; 9"), "post-op Const misaligned: {output}");
    }

    #[test]
    fn test_disassemble_jump() {
        let mut e = Emitter::new();
        let patch = e.emit_jump(Op::JumpIfFalse);
        e.emit_op(Op::Nil);
        e.patch_jump(patch);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("jump_test"));
        assert!(output.contains("JUMP_IF_FALSE"));
        assert!(output.contains("->"));
    }

    #[test]
    fn test_disassemble_no_name() {
        let mut e = Emitter::new();
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, None);
        assert!(output.contains("== <script> =="));
    }

    #[test]
    fn test_disassemble_locals() {
        let mut e = Emitter::new();
        e.emit_op(Op::LoadLocal);
        e.emit_u16(3);
        e.emit_op(Op::StoreLocal);
        e.emit_u16(0);
        e.emit_op(Op::LoadUpvalue);
        e.emit_u16(1);
        e.emit_op(Op::StoreUpvalue);
        e.emit_u16(2);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("locals"));
        assert!(output.contains("LOAD_LOCAL"));
        assert!(output.contains("STORE_LOCAL"));
        assert!(output.contains("LOAD_UPVALUE"));
        assert!(output.contains("STORE_UPVALUE"));
    }

    #[test]
    fn test_disassemble_globals() {
        let spur = intern("my-var");
        let bits = spur_to_bits(spur);

        let mut e = Emitter::new();
        e.emit_op(Op::LoadGlobal);
        e.emit_u32(bits);
        e.emit_u16(0); // cache_slot
        e.emit_op(Op::DefineGlobal);
        e.emit_u32(bits);
        e.emit_op(Op::StoreGlobal);
        e.emit_u32(bits);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("globals"));
        assert!(output.contains("LOAD_GLOBAL"));
        assert!(output.contains("DEFINE_GLOBAL"));
        assert!(output.contains("STORE_GLOBAL"));
        assert!(output.contains("my-var"));
    }

    #[test]
    fn test_disassemble_call() {
        let mut e = Emitter::new();
        e.emit_op(Op::Call);
        e.emit_u16(2);
        e.emit_op(Op::TailCall);
        e.emit_u16(1);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("call"));
        assert!(output.contains("CALL"));
        assert!(output.contains("TAIL_CALL"));
    }

    #[test]
    fn test_disassemble_call_native() {
        let mut e = Emitter::new();
        e.emit_op(Op::CallNative);
        e.emit_u16(5); // native_id
        e.emit_u16(3); // argc
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("native"));
        assert!(output.contains("CALL_NATIVE"));
        assert!(output.contains("native=5"));
        assert!(output.contains("argc=3"));
    }

    #[test]
    fn test_disassemble_call_global() {
        let spur = intern("println");
        let bits = spur_to_bits(spur);

        let mut e = Emitter::new();
        e.emit_op(Op::CallGlobal);
        e.emit_u32(bits);
        e.emit_u16(1); // argc
        e.emit_u16(0); // cache_slot
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("call_global"));
        assert!(output.contains("CALL_GLOBAL"));
        assert!(output.contains("println"));
        assert!(output.contains("argc=1"));
    }

    #[test]
    fn test_disassemble_make_closure() {
        let mut e = Emitter::new();
        e.emit_op(Op::MakeClosure);
        e.emit_u16(0); // func_id
        e.emit_u16(2); // n_upvalues
                       // upvalue descriptors
        e.emit_u16(1); // is_local = true
        e.emit_u16(0); // idx = 0
        e.emit_u16(0); // is_local = false
        e.emit_u16(1); // idx = 1
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("closure"));
        assert!(output.contains("MAKE_CLOSURE"));
        assert!(output.contains("func=0"));
        assert!(output.contains("upvalues=2"));
        assert!(output.contains("| local 0"));
        assert!(output.contains("| upvalue 1"));
    }

    #[test]
    fn test_disassemble_collections() {
        let mut e = Emitter::new();
        e.emit_op(Op::MakeList);
        e.emit_u16(3);
        e.emit_op(Op::MakeVector);
        e.emit_u16(2);
        e.emit_op(Op::MakeMap);
        e.emit_u16(4);
        e.emit_op(Op::MakeHashMap);
        e.emit_u16(6);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("collections"));
        assert!(output.contains("MAKE_LIST"));
        assert!(output.contains("MAKE_VECTOR"));
        assert!(output.contains("MAKE_MAP"));
        assert!(output.contains("MAKE_HASHMAP"));
    }

    #[test]
    fn test_disassemble_zero_operand_ops() {
        let mut e = Emitter::new();
        e.emit_op(Op::Nil);
        e.emit_op(Op::True);
        e.emit_op(Op::False);
        e.emit_op(Op::Pop);
        e.emit_op(Op::Dup);
        e.emit_op(Op::Not);
        e.emit_op(Op::Throw);
        e.emit_op(Op::Car);
        e.emit_op(Op::Cdr);
        e.emit_op(Op::Cons);
        e.emit_op(Op::IsNull);
        e.emit_op(Op::Length);
        e.emit_op(Op::Get);
        e.emit_op(Op::ContainsQ);
        e.emit_op(Op::StringLength);
        e.emit_op(Op::StringRef);
        e.emit_op(Op::StringAppend);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("zero_ops"));
        assert!(output.contains("NIL"));
        assert!(output.contains("TRUE"));
        assert!(output.contains("FALSE"));
        assert!(output.contains("POP"));
        assert!(output.contains("DUP"));
        assert!(output.contains("NOT"));
        assert!(output.contains("THROW"));
        assert!(output.contains("CAR"));
        assert!(output.contains("CDR"));
        assert!(output.contains("CONS"));
        assert!(output.contains("IS_NULL"));
        assert!(output.contains("LENGTH"));
        assert!(output.contains("GET"));
        assert!(output.contains("CONTAINS_Q"));
        assert!(output.contains("STRING_LENGTH"));
        assert!(output.contains("STRING_REF"));
        assert!(output.contains("STRING_APPEND"));
    }

    #[test]
    fn test_disassemble_unknown_opcode() {
        let e = Emitter::new();
        let mut chunk = e.into_chunk();
        // Manually inject an invalid opcode byte
        chunk.code.push(0xFF);
        let output = disassemble(&chunk, Some("unknown"));
        assert!(output.contains("UNKNOWN(0xff)"));
    }

    #[test]
    fn test_disassemble_specialized_locals() {
        let mut e = Emitter::new();
        e.emit_op(Op::LoadLocal0);
        e.emit_op(Op::LoadLocal1);
        e.emit_op(Op::LoadLocal2);
        e.emit_op(Op::LoadLocal3);
        e.emit_op(Op::StoreLocal0);
        e.emit_op(Op::StoreLocal1);
        e.emit_op(Op::StoreLocal2);
        e.emit_op(Op::StoreLocal3);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("spec_locals"));
        assert!(output.contains("LOAD_LOCAL_0"));
        assert!(output.contains("LOAD_LOCAL_1"));
        assert!(output.contains("LOAD_LOCAL_2"));
        assert!(output.contains("LOAD_LOCAL_3"));
        assert!(output.contains("STORE_LOCAL_0"));
        assert!(output.contains("STORE_LOCAL_1"));
        assert!(output.contains("STORE_LOCAL_2"));
        assert!(output.contains("STORE_LOCAL_3"));
    }

    #[test]
    fn test_disassemble_all_jumps() {
        let mut e = Emitter::new();
        let j1 = e.emit_jump(Op::Jump);
        e.patch_jump(j1);
        let j2 = e.emit_jump(Op::JumpIfTrue);
        e.patch_jump(j2);
        let j3 = e.emit_jump(Op::JumpIfFalse);
        e.patch_jump(j3);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("jumps"));
        assert!(output.contains("JUMP "));
        assert!(output.contains("JUMP_IF_TRUE"));
        assert!(output.contains("JUMP_IF_FALSE"));
    }

    #[test]
    fn test_disassemble_arithmetic() {
        let mut e = Emitter::new();
        e.emit_op(Op::Add);
        e.emit_op(Op::Sub);
        e.emit_op(Op::Mul);
        e.emit_op(Op::Div);
        e.emit_op(Op::Negate);
        e.emit_op(Op::Eq);
        e.emit_op(Op::Lt);
        e.emit_op(Op::Gt);
        e.emit_op(Op::Le);
        e.emit_op(Op::Ge);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("arith"));
        assert!(output.contains("ADD\n") || output.contains("ADD\r"));
        assert!(output.contains("SUB\n") || output.contains("SUB\r"));
        assert!(output.contains("MUL\n") || output.contains("MUL\r"));
        assert!(output.contains("DIV"));
        assert!(output.contains("NEGATE"));
        assert!(output.contains("EQ\n") || output.contains("EQ\r"));
        assert!(output.contains("LT\n") || output.contains("LT\r"));
        assert!(output.contains("GT\n") || output.contains("GT\r"));
        assert!(output.contains("LE\n") || output.contains("LE\r"));
        assert!(output.contains("GE\n") || output.contains("GE\r"));
    }

    #[test]
    fn test_disassemble_intrinsic_predicates() {
        let mut e = Emitter::new();
        e.emit_op(Op::IsPair);
        e.emit_op(Op::IsList);
        e.emit_op(Op::IsNumber);
        e.emit_op(Op::IsString);
        e.emit_op(Op::IsSymbol);
        e.emit_op(Op::Append);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let output = disassemble(&chunk, Some("predicates"));
        assert!(output.contains("IS_PAIR"));
        assert!(output.contains("IS_LIST"));
        assert!(output.contains("IS_NUMBER"));
        assert!(output.contains("IS_STRING"));
        assert!(output.contains("IS_SYMBOL"));
        assert!(output.contains("APPEND"));
    }
}
