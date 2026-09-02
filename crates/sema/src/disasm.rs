use crate::die;

pub(crate) fn run_disasm(file: &str, json: bool) {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            let msg = match e.kind() {
                std::io::ErrorKind::NotFound => format!("file not found: {file}"),
                std::io::ErrorKind::PermissionDenied => format!("permission denied: {file}"),
                _ => format!("reading {file}: {e}"),
            };
            die(msg);
        }
    };

    if !sema_vm::is_bytecode_file(&bytes) {
        die(format!("{file} is not a valid .semac bytecode file"));
    }

    let result = match sema_vm::deserialize_from_bytes(&bytes) {
        Ok(r) => r,
        Err(e) => {
            die(format!("deserialization failed: {}", e.format_plain()));
        }
    };

    if json {
        let json_val = disassemble_to_json(&result, &bytes);
        println!("{}", serde_json::to_string_pretty(&json_val).unwrap());
    } else {
        // Disassemble main chunk
        print!("{}", sema_vm::disassemble(&result.chunk, Some("<main>")));

        // Disassemble each function
        for (i, func) in result.functions.iter().enumerate() {
            let name = func
                .name
                .map(sema_core::resolve)
                .unwrap_or_else(|| format!("<fn {i}>"));
            print!("{}", sema_vm::disassemble(&func.chunk, Some(&name)));
        }
    }
}

fn disassemble_to_json(result: &sema_vm::CompileResult, bytes: &[u8]) -> serde_json::Value {
    let format_version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let major = u16::from_le_bytes([bytes[8], bytes[9]]);
    let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
    let patch = u16::from_le_bytes([bytes[12], bytes[13]]);

    let mut functions = Vec::new();

    // Main chunk
    functions.push(chunk_to_json(&result.chunk, "<main>"));

    // Function templates
    for (i, func) in result.functions.iter().enumerate() {
        let name = func
            .name
            .map(sema_core::resolve)
            .unwrap_or_else(|| format!("<fn {i}>"));
        let mut obj = chunk_to_json(&func.chunk, &name);
        obj["arity"] = serde_json::json!(func.arity);
        obj["has_rest"] = serde_json::json!(func.has_rest);
        obj["upvalues"] = serde_json::json!(func.upvalue_descs.len());
        functions.push(obj);
    }

    serde_json::json!({
        "format_version": format_version,
        "sema_version": format!("{major}.{minor}.{patch}"),
        "size_bytes": bytes.len(),
        "functions": functions,
    })
}

fn chunk_to_json(chunk: &sema_vm::Chunk, name: &str) -> serde_json::Value {
    let mut instructions = Vec::new();
    let code = &chunk.code;
    let mut pc = 0usize;

    while pc < code.len() {
        let op_byte = code[pc];
        let op = sema_vm::Op::from_u8(op_byte);
        let op_name = op
            .map(|o| format!("{o:?}"))
            .unwrap_or_else(|| format!("Unknown(0x{op_byte:02x})"));

        let (inst, next_pc) = match op {
            Some(sema_vm::Op::Const) => {
                let idx = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                let val_str = chunk
                    .consts
                    .get(idx as usize)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into());
                (
                    serde_json::json!({"pc": pc, "op": op_name, "index": idx, "value": val_str}),
                    pc + 3,
                )
            }
            Some(
                sema_vm::Op::LoadLocal
                | sema_vm::Op::TakeLocal
                | sema_vm::Op::StoreLocal
                | sema_vm::Op::LoadUpvalue
                | sema_vm::Op::StoreUpvalue,
            ) => {
                let slot = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "slot": slot}),
                    pc + 3,
                )
            }
            Some(sema_vm::Op::StoreGlobal | sema_vm::Op::DefineGlobal) => {
                let spur_bits =
                    u32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                // The deserialized bytecode has already remapped indices to valid Spurs.
                let name_str = if spur_bits != 0 {
                    let spur = sema_core::bits_to_spur(spur_bits);
                    sema_core::resolve(spur)
                } else {
                    format!("spur({spur_bits})")
                };
                (
                    serde_json::json!({"pc": pc, "op": op_name, "name": name_str}),
                    pc + 5,
                )
            }
            Some(sema_vm::Op::LoadGlobal) => {
                let spur_bits =
                    u32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                let name_str = if spur_bits != 0 {
                    let spur = sema_core::bits_to_spur(spur_bits);
                    sema_core::resolve(spur)
                } else {
                    format!("spur({spur_bits})")
                };
                let cache_slot = u16::from_le_bytes([code[pc + 5], code[pc + 6]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "name": name_str, "cache_slot": cache_slot}),
                    pc + 7,
                )
            }
            Some(sema_vm::Op::CallGlobal) => {
                let spur_bits =
                    u32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                let name_str = if spur_bits != 0 {
                    let spur = sema_core::bits_to_spur(spur_bits);
                    sema_core::resolve(spur)
                } else {
                    format!("spur({spur_bits})")
                };
                let argc = u16::from_le_bytes([code[pc + 5], code[pc + 6]]);
                let cache_slot = u16::from_le_bytes([code[pc + 7], code[pc + 8]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "name": name_str, "argc": argc, "cache_slot": cache_slot}),
                    pc + 9,
                )
            }
            Some(sema_vm::Op::Jump | sema_vm::Op::JumpIfFalse | sema_vm::Op::JumpIfTrue) => {
                let offset =
                    i32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                let target = (pc as i32 + 5 + offset) as u32;
                (
                    serde_json::json!({"pc": pc, "op": op_name, "offset": offset, "target": target}),
                    pc + 5,
                )
            }
            Some(
                sema_vm::Op::Call
                | sema_vm::Op::TailCall
                | sema_vm::Op::SelfTailCall
                | sema_vm::Op::CallSelf,
            ) => {
                let argc = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "argc": argc}),
                    pc + 3,
                )
            }
            Some(sema_vm::Op::CallNative) => {
                let native_id = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                let argc = u16::from_le_bytes([code[pc + 3], code[pc + 4]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "native_id": native_id, "argc": argc}),
                    pc + 5,
                )
            }
            Some(sema_vm::Op::MakeClosure) => {
                let func_id = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                let n_upvalues = u16::from_le_bytes([code[pc + 3], code[pc + 4]]);
                let mut upvals = Vec::new();
                let mut upc = pc + 5;
                for _ in 0..n_upvalues {
                    let is_local = u16::from_le_bytes([code[upc], code[upc + 1]]);
                    let idx = u16::from_le_bytes([code[upc + 2], code[upc + 3]]);
                    upvals.push(serde_json::json!({"is_local": is_local != 0, "index": idx}));
                    upc += 4;
                }
                (
                    serde_json::json!({"pc": pc, "op": op_name, "func_id": func_id, "upvalues": upvals}),
                    upc,
                )
            }
            Some(
                sema_vm::Op::MakeList
                | sema_vm::Op::MakeVector
                | sema_vm::Op::MakeMap
                | sema_vm::Op::MakeHashMap,
            ) => {
                let count = u16::from_le_bytes([code[pc + 1], code[pc + 2]]);
                (
                    serde_json::json!({"pc": pc, "op": op_name, "count": count}),
                    pc + 3,
                )
            }
            _ => (serde_json::json!({"pc": pc, "op": op_name}), pc + 1),
        };

        instructions.push(inst);
        pc = next_pc;
    }

    let constants: Vec<String> = chunk.consts.iter().map(|v| v.to_string()).collect();

    serde_json::json!({
        "name": name,
        "n_locals": chunk.n_locals,
        "max_stack": chunk.max_stack,
        "code_bytes": chunk.code.len(),
        "constants": constants,
        "instructions": instructions,
        "exception_table": chunk.exception_table.iter().map(|e| {
            serde_json::json!({
                "try_start": e.try_start,
                "try_end": e.try_end,
                "handler_pc": e.handler_pc,
                "stack_depth": e.stack_depth,
                "catch_slot": e.catch_slot,
            })
        }).collect::<Vec<_>>(),
    })
}
