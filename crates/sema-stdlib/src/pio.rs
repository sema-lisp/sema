use std::collections::{BTreeMap, HashMap};

use sema_core::{check_arity, ArgsExt, SemaError, Value};

use crate::register_fn;

// ── Encoding lookup tables ───────────────────────────────────────

fn jmp_cond(s: &str) -> Result<u8, SemaError> {
    match s {
        "always" => Ok(0),
        "!x" => Ok(1),
        "x--" => Ok(2),
        "!y" => Ok(3),
        "y--" => Ok(4),
        "x!=y" => Ok(5),
        "pin" => Ok(6),
        "!osre" => Ok(7),
        _ => Err(SemaError::eval(format!("pio/jmp: unknown condition :{s}"))
            .with_hint("valid: :always :!x :x-- :!y :y-- :x!=y :pin :!osre")),
    }
}

fn in_source(s: &str) -> Result<u8, SemaError> {
    match s {
        "pins" => Ok(0),
        "x" => Ok(1),
        "y" => Ok(2),
        "null" => Ok(3),
        "isr" => Ok(6),
        "osr" => Ok(7),
        _ => Err(SemaError::eval(format!("pio/in: unknown source :{s}"))
            .with_hint("valid: :pins :x :y :null :isr :osr")),
    }
}

fn out_dest(s: &str) -> Result<u8, SemaError> {
    match s {
        "pins" => Ok(0),
        "x" => Ok(1),
        "y" => Ok(2),
        "null" => Ok(3),
        "pindirs" => Ok(4),
        "pc" => Ok(5),
        "isr" => Ok(6),
        "exec" => Ok(7),
        _ => Err(
            SemaError::eval(format!("pio/out: unknown destination :{s}"))
                .with_hint("valid: :pins :x :y :null :pindirs :pc :isr :exec"),
        ),
    }
}

fn mov_dest(s: &str) -> Result<u8, SemaError> {
    match s {
        "pins" => Ok(0),
        "x" => Ok(1),
        "y" => Ok(2),
        "exec" => Ok(4),
        "pc" => Ok(5),
        "isr" => Ok(6),
        "osr" => Ok(7),
        _ => Err(
            SemaError::eval(format!("pio/mov: unknown destination :{s}"))
                .with_hint("valid: :pins :x :y :exec :pc :isr :osr"),
        ),
    }
}

fn mov_source(s: &str) -> Result<u8, SemaError> {
    match s {
        "pins" => Ok(0),
        "x" => Ok(1),
        "y" => Ok(2),
        "null" => Ok(3),
        "status" => Ok(5),
        "isr" => Ok(6),
        "osr" => Ok(7),
        _ => Err(SemaError::eval(format!("pio/mov: unknown source :{s}"))
            .with_hint("valid: :pins :x :y :null :status :isr :osr")),
    }
}

fn set_dest(s: &str) -> Result<u8, SemaError> {
    match s {
        "pins" => Ok(0),
        "x" => Ok(1),
        "y" => Ok(2),
        "pindirs" => Ok(4),
        _ => Err(
            SemaError::eval(format!("pio/set: unknown destination :{s}"))
                .with_hint("valid: :pins :x :y :pindirs"),
        ),
    }
}

fn wait_source(s: &str) -> Result<u8, SemaError> {
    match s {
        "gpio" => Ok(0),
        "pin" => Ok(1),
        "irq" => Ok(2),
        _ => Err(SemaError::eval(format!("pio/wait: unknown source :{s}"))
            .with_hint("valid: :gpio :pin :irq")),
    }
}

/// Parse a MOV source keyword, handling the `!` prefix for invert.
/// Returns (source_code, operation_code).
fn parse_mov_source(s: &str) -> Result<(u8, u8), SemaError> {
    if let Some(base) = s.strip_prefix('!') {
        let src = mov_source(base)?;
        Ok((src, 1)) // op = invert
    } else {
        let src = mov_source(s)?;
        Ok((src, 0)) // op = none
    }
}

/// Encode bit count: 32 → 0, 1-31 → literal. 0 and >32 are errors.
fn encode_bit_count(bits: i64, fname: &str) -> Result<u8, SemaError> {
    if bits == 32 {
        Ok(0)
    } else if (1..=31).contains(&bits) {
        Ok(bits as u8)
    } else {
        Err(SemaError::eval(format!(
            "{fname}: bit count {bits} out of range 1..32"
        )))
    }
}

// ── Map field helpers ────────────────────────────────────────────

fn get_keyword_field(map: &BTreeMap<Value, Value>, key: &str) -> Result<String, SemaError> {
    map.get(&Value::keyword(key))
        .and_then(|v| v.as_keyword())
        .ok_or_else(|| SemaError::eval(format!("pio/assemble: missing or invalid :{key} field")))
}

fn get_int_field(map: &BTreeMap<Value, Value>, key: &str) -> Result<i64, SemaError> {
    map.get(&Value::keyword(key))
        .and_then(|v| v.as_int())
        .ok_or_else(|| SemaError::eval(format!("pio/assemble: missing or invalid :{key} field")))
}

fn get_optional_int_field(
    map: &BTreeMap<Value, Value>,
    key: &str,
) -> Result<Option<i64>, SemaError> {
    match map.get(&Value::keyword(key)) {
        None => Ok(None),
        Some(v) => v
            .as_int()
            .map(Some)
            .ok_or_else(|| SemaError::eval(format!("pio/assemble: :{key} must be an integer"))),
    }
}

fn get_optional_keyword_field(
    map: &BTreeMap<Value, Value>,
    key: &str,
) -> Result<Option<String>, SemaError> {
    match map.get(&Value::keyword(key)) {
        None => Ok(None),
        Some(v) => v
            .as_keyword()
            .map(Some)
            .ok_or_else(|| SemaError::eval(format!("pio/assemble: :{key} must be a keyword"))),
    }
}

fn get_symbol_field(map: &BTreeMap<Value, Value>, key: &str) -> Result<String, SemaError> {
    map.get(&Value::keyword(key))
        .and_then(|v| v.as_symbol())
        .ok_or_else(|| SemaError::eval(format!("pio/assemble: missing or invalid :{key} field")))
}

fn get_optional_bool_field(
    map: &BTreeMap<Value, Value>,
    key: &str,
) -> Result<Option<bool>, SemaError> {
    match map.get(&Value::keyword(key)) {
        None => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| SemaError::eval(format!("pio/assemble: :{key} must be a boolean"))),
    }
}

// ── Instruction map builder ──────────────────────────────────────

fn make_instr(op: &str, fields: &[(&str, Value)]) -> Value {
    let mut map = BTreeMap::new();
    map.insert(Value::keyword("op"), Value::keyword(op));
    for (k, v) in fields {
        map.insert(Value::keyword(k), v.clone());
    }
    Value::map(map)
}

// ── Opcode encoders (instruction map → 8-bit arg field) ─────────

fn encode_jmp(map: &BTreeMap<Value, Value>, labels: &HashMap<String, u8>) -> Result<u8, SemaError> {
    let cond_kw = get_keyword_field(map, "cond")?;
    let cond = jmp_cond(&cond_kw)?;
    let target = get_symbol_field(map, "target")?;
    let addr = labels.get(&target).ok_or_else(|| {
        SemaError::eval(format!("pio/assemble: undefined label '{target}'"))
            .with_hint("labels are symbols in the program list: 'my-label")
    })?;
    Ok((cond << 5) | (*addr & 0x1F))
}

fn encode_wait(map: &BTreeMap<Value, Value>) -> Result<u8, SemaError> {
    let polarity = get_int_field(map, "polarity")? as u8;
    let source_kw = get_keyword_field(map, "source")?;
    let source = wait_source(&source_kw)?;
    let index = get_int_field(map, "index")? as u8;
    let rel = get_optional_bool_field(map, "rel")?.unwrap_or(false);
    let idx = if rel { index | 0x10 } else { index & 0x1F };
    Ok((polarity << 7) | (source << 5) | idx)
}

fn encode_in(map: &BTreeMap<Value, Value>) -> Result<u8, SemaError> {
    let src_kw = get_keyword_field(map, "source")?;
    let src = in_source(&src_kw)?;
    let bits = get_int_field(map, "bits")?;
    let enc = encode_bit_count(bits, "pio/in")?;
    Ok((src << 5) | enc)
}

fn encode_out(map: &BTreeMap<Value, Value>) -> Result<u8, SemaError> {
    let dest_kw = get_keyword_field(map, "dest")?;
    let dest = out_dest(&dest_kw)?;
    let bits = get_int_field(map, "bits")?;
    let enc = encode_bit_count(bits, "pio/out")?;
    Ok((dest << 5) | enc)
}

fn encode_push(map: &BTreeMap<Value, Value>) -> Result<u8, SemaError> {
    let iffull = get_optional_bool_field(map, "iffull")?.unwrap_or(false);
    let block = get_optional_bool_field(map, "block")?.unwrap_or(true);
    // RP2040 datasheet: bit 7=0 (PUSH), bit 6=IfFull, bit 5=Block (1=stall)
    Ok(((iffull as u8) << 6) | ((block as u8) << 5))
}

fn encode_pull(map: &BTreeMap<Value, Value>) -> Result<u8, SemaError> {
    let ifempty = get_optional_bool_field(map, "ifempty")?.unwrap_or(false);
    let block = get_optional_bool_field(map, "block")?.unwrap_or(true);
    // RP2040 datasheet: bit 7=1 (PULL), bit 6=IfEmpty, bit 5=Block (1=stall)
    Ok(0x80 | ((ifempty as u8) << 6) | ((block as u8) << 5))
}

fn encode_mov(map: &BTreeMap<Value, Value>) -> Result<u8, SemaError> {
    let dest_kw = get_keyword_field(map, "dest")?;
    let dest = mov_dest(&dest_kw)?;
    let src_kw = get_keyword_field(map, "source")?;
    let (src, op_from_prefix) = parse_mov_source(&src_kw)?;
    let op = if let Some(mov_op_kw) = get_optional_keyword_field(map, "mov-op")? {
        match mov_op_kw.as_str() {
            "invert" => 1,
            "reverse" => 2,
            _ => {
                return Err(
                    SemaError::eval(format!("pio/mov: unknown operation :{mov_op_kw}"))
                        .with_hint("valid: :invert :reverse"),
                )
            }
        }
    } else {
        op_from_prefix
    };
    Ok((dest << 5) | (op << 3) | src)
}

fn encode_irq(map: &BTreeMap<Value, Value>) -> Result<u8, SemaError> {
    let mode_kw = get_keyword_field(map, "mode")?;
    let mode: u8 = match mode_kw.as_str() {
        "set" => 0b00,
        "wait" => 0b01,
        "clear" => 0b10,
        _ => {
            return Err(SemaError::eval(format!("pio/irq: unknown mode :{mode_kw}"))
                .with_hint("valid: :set :wait :clear"))
        }
    };
    let index = get_int_field(map, "index")?;
    if !(0..=7).contains(&index) {
        return Err(SemaError::eval(format!(
            "pio/irq: index {index} out of range 0..7"
        )));
    }
    let rel = get_optional_bool_field(map, "rel")?.unwrap_or(false);
    Ok((mode << 5) | ((rel as u8) << 4) | (index as u8))
}

fn encode_set(map: &BTreeMap<Value, Value>) -> Result<u8, SemaError> {
    let dest_kw = get_keyword_field(map, "dest")?;
    let dest = set_dest(&dest_kw)?;
    let value = get_int_field(map, "value")?;
    if !(0..=31).contains(&value) {
        return Err(SemaError::eval(format!(
            "pio/assemble: set value {value} out of range 0..31"
        )));
    }
    Ok((dest << 5) | (value as u8))
}

// ── Assembler ────────────────────────────────────────────────────

fn encode_instruction(
    map: &BTreeMap<Value, Value>,
    labels: &HashMap<String, u8>,
    delay_bits: u8,
    side_set_bits: u8,
    side_set_opt: bool,
) -> Result<u16, SemaError> {
    let op = get_keyword_field(map, "op")?;

    let delay = get_optional_int_field(map, "delay")?.unwrap_or(0);
    let side_set_val = get_optional_int_field(map, "side-set")?;
    let side_set = side_set_val.unwrap_or(0);

    let max_delay = (1u16 << delay_bits) - 1;
    if delay < 0 || delay > max_delay as i64 {
        return Err(SemaError::eval(format!(
            "pio/assemble: delay {delay} exceeds maximum {max_delay} ({delay_bits} delay bits)"
        )));
    }
    if side_set_bits > 0 {
        let max_side = (1i64 << side_set_bits) - 1;
        if side_set < 0 || side_set > max_side {
            return Err(SemaError::eval(format!(
                "pio/assemble: side-set value {side_set} out of range for {side_set_bits} bits"
            )));
        }
    }

    // Build the 5-bit delay/side-set field:
    // Without opt: [side_set_value (N bits)] [delay (5-N bits)]
    // With opt:    [enable (1 bit)] [side_set_value (N bits)] [delay (5-N-1 bits)]
    let delay_sideset_field = if side_set_opt {
        let enable = if side_set_val.is_some() { 1u8 } else { 0u8 };
        (enable << 4) | ((side_set as u8) << delay_bits) | (delay as u8 & ((1u8 << delay_bits) - 1))
    } else {
        ((side_set as u8) << delay_bits) | (delay as u8 & ((1u8 << delay_bits) - 1))
    };

    let opcode: u16 = match op.as_str() {
        "jmp" => 0b000,
        "wait" => 0b001,
        "in" => 0b010,
        "out" => 0b011,
        "push" | "pull" => 0b100,
        "mov" => 0b101,
        "irq" => 0b110,
        "set" => 0b111,
        _ => {
            return Err(
                SemaError::eval(format!("pio/assemble: unknown opcode :{op}"))
                    .with_hint("valid: :jmp :wait :in :out :push :pull :mov :irq :set"),
            )
        }
    };

    let arg: u8 = match op.as_str() {
        "jmp" => encode_jmp(map, labels)?,
        "wait" => encode_wait(map)?,
        "in" => encode_in(map)?,
        "out" => encode_out(map)?,
        "push" => encode_push(map)?,
        "pull" => encode_pull(map)?,
        "mov" => encode_mov(map)?,
        "irq" => encode_irq(map)?,
        "set" => encode_set(map)?,
        _ => unreachable!(),
    };

    Ok((opcode << 13) | ((delay_sideset_field as u16) << 8) | (arg as u16))
}

fn assemble(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "pio/assemble", 1..=2);
    let program = args.list_at(0, "pio/assemble")?;

    // Parse config
    let (side_set_bits, side_set_opt) = if args.len() == 2 {
        let cfg = args[1]
            .as_map_ref()
            .ok_or_else(|| SemaError::type_error("map", args[1].type_name()))?;
        let ssb = get_optional_int_field(cfg, "side-set-bits")?.unwrap_or(0);
        if !(0..=5).contains(&ssb) {
            return Err(SemaError::eval(format!(
                "pio/assemble: side-set-bits {ssb} out of range 0..5"
            )));
        }
        let sso = get_optional_bool_field(cfg, "side-set-opt")?.unwrap_or(false);
        let total = ssb as u8 + sso as u8;
        if total > 5 {
            return Err(SemaError::eval(format!(
                "pio/assemble: side-set-bits ({ssb}) + opt bit exceeds 5"
            )));
        }
        (ssb as u8, sso)
    } else {
        (0u8, false)
    };

    // side-set-opt steals one bit for the enable flag
    let side_set_total = side_set_bits + side_set_opt as u8;
    let delay_bits = 5 - side_set_total;

    // Pass 1: collect labels and wrap points
    let mut labels = HashMap::new();
    let mut wrap_target: Option<u8> = None;
    let mut wrap: Option<u8> = None;
    let mut addr: u8 = 0;

    for item in program.iter() {
        if let Some(sym) = item.as_symbol() {
            if labels.contains_key(&sym) {
                return Err(SemaError::eval(format!(
                    "pio/assemble: duplicate label '{sym}'"
                )));
            }
            labels.insert(sym, addr);
        } else if let Some(kw) = item.as_keyword() {
            match kw.as_str() {
                "wrap-target" => {
                    if wrap_target.is_some() {
                        return Err(SemaError::eval(
                            "pio/assemble: duplicate :wrap-target".to_string(),
                        ));
                    }
                    wrap_target = Some(addr);
                }
                "wrap" => {
                    if wrap.is_some() {
                        return Err(SemaError::eval("pio/assemble: duplicate :wrap".to_string()));
                    }
                    if addr == 0 {
                        return Err(SemaError::eval(
                            "pio/assemble: :wrap before any instruction".to_string(),
                        ));
                    }
                    wrap = Some(addr - 1);
                }
                _ => {
                    return Err(SemaError::eval(format!(
                        "pio/assemble: unexpected keyword :{kw} in program"
                    )))
                }
            }
        } else if item.as_map_ref().is_some() {
            addr += 1;
            if addr > 32 {
                return Err(SemaError::eval(
                    "pio/assemble: program exceeds 32 instructions".to_string(),
                ));
            }
        } else {
            return Err(SemaError::eval(format!(
                "pio/assemble: unexpected item in program: {}",
                item
            )));
        }
    }

    // Pass 2: encode
    let mut words: Vec<u16> = Vec::new();
    for item in program.iter() {
        if item.as_symbol().is_some() || item.as_keyword().is_some() {
            continue;
        }
        let map = item
            .as_map_ref()
            .ok_or_else(|| SemaError::eval("pio/assemble: internal: instruction must be a map"))?;
        let word = encode_instruction(map, &labels, delay_bits, side_set_bits, side_set_opt)?;
        words.push(word);
    }

    // Build result
    let mut bytes = Vec::with_capacity(words.len() * 2);
    for w in &words {
        bytes.push((w & 0xFF) as u8);
        bytes.push((w >> 8) as u8);
    }

    let wt = wrap_target.unwrap_or(0) as i64;
    let wr = wrap.unwrap_or(words.len().saturating_sub(1) as u8) as i64;

    let mut result = BTreeMap::new();
    result.insert(Value::keyword("instructions"), Value::bytevector(bytes));
    result.insert(Value::keyword("length"), Value::int(words.len() as i64));
    result.insert(Value::keyword("wrap-target"), Value::int(wt));
    result.insert(Value::keyword("wrap"), Value::int(wr));

    Ok(Value::map(result))
}

// ── Registration ─────────────────────────────────────────────────

pub fn register(env: &sema_core::Env) {
    // --- instruction builders ---

    register_fn(env, "pio/nop", |args| {
        check_arity!(args, "pio/nop", 0);
        Ok(make_instr(
            "mov",
            &[
                ("dest", Value::keyword("y")),
                ("source", Value::keyword("y")),
            ],
        ))
    });

    register_fn(env, "pio/jmp", |args| {
        check_arity!(args, "pio/jmp", 1..=2);
        if args.len() == 1 {
            let target = args.symbol_at(0, "pio/jmp")?;
            Ok(make_instr(
                "jmp",
                &[
                    ("cond", Value::keyword("always")),
                    ("target", Value::symbol(&target)),
                ],
            ))
        } else {
            let cond = args.keyword_at(0, "pio/jmp")?;
            jmp_cond(&cond)?; // validate
            let target = args.symbol_at(1, "pio/jmp")?;
            Ok(make_instr(
                "jmp",
                &[
                    ("cond", Value::keyword(&cond)),
                    ("target", Value::symbol(&target)),
                ],
            ))
        }
    });

    register_fn(env, "pio/wait", |args| {
        check_arity!(args, "pio/wait", 3..=4);
        let polarity = args.int_at(0, "pio/wait")?;
        if polarity != 0 && polarity != 1 {
            return Err(SemaError::eval(format!(
                "pio/wait: polarity must be 0 or 1, got {polarity}"
            )));
        }
        let source = args.keyword_at(1, "pio/wait")?;
        wait_source(&source)?;
        let index = args.int_at(2, "pio/wait")?;
        if !(0..=31).contains(&index) {
            return Err(SemaError::eval(format!(
                "pio/wait: index {index} out of range 0..31"
            )));
        }
        let mut fields: Vec<(&str, Value)> = vec![
            ("polarity", Value::int(polarity)),
            ("source", Value::keyword(&source)),
            ("index", Value::int(index)),
        ];
        if args.len() == 4 {
            let rel = args.keyword_at(3, "pio/wait")?;
            if rel != "rel" {
                return Err(SemaError::eval(format!(
                    "pio/wait: expected :rel, got :{rel}"
                )));
            }
            fields.push(("rel", Value::bool(true)));
        }
        Ok(make_instr("wait", &fields))
    });

    register_fn(env, "pio/in", |args| {
        check_arity!(args, "pio/in", 2);
        let source = args.keyword_at(0, "pio/in")?;
        in_source(&source)?;
        let bits = args.int_at(1, "pio/in")?;
        encode_bit_count(bits, "pio/in")?;
        Ok(make_instr(
            "in",
            &[
                ("source", Value::keyword(&source)),
                ("bits", Value::int(bits)),
            ],
        ))
    });

    register_fn(env, "pio/out", |args| {
        check_arity!(args, "pio/out", 2);
        let dest = args.keyword_at(0, "pio/out")?;
        out_dest(&dest)?;
        let bits = args.int_at(1, "pio/out")?;
        encode_bit_count(bits, "pio/out")?;
        Ok(make_instr(
            "out",
            &[("dest", Value::keyword(&dest)), ("bits", Value::int(bits))],
        ))
    });

    register_fn(env, "pio/push", |args| {
        check_arity!(args, "pio/push", 0..=2);
        let mut block = true;
        let mut iffull = false;
        for arg in args {
            let kw = arg
                .as_keyword()
                .ok_or_else(|| SemaError::type_error("keyword", arg.type_name()))?;
            match kw.as_str() {
                "block" => block = true,
                "no-block" => block = false,
                "iffull" => iffull = true,
                _ => {
                    return Err(
                        SemaError::eval(format!("pio/push: unexpected option :{kw}"))
                            .with_hint("valid: :block :no-block :iffull"),
                    )
                }
            }
        }
        Ok(make_instr(
            "push",
            &[
                ("block", Value::bool(block)),
                ("iffull", Value::bool(iffull)),
            ],
        ))
    });

    register_fn(env, "pio/pull", |args| {
        check_arity!(args, "pio/pull", 0..=2);
        let mut block = true;
        let mut ifempty = false;
        for arg in args {
            let kw = arg
                .as_keyword()
                .ok_or_else(|| SemaError::type_error("keyword", arg.type_name()))?;
            match kw.as_str() {
                "block" => block = true,
                "no-block" => block = false,
                "ifempty" => ifempty = true,
                _ => {
                    return Err(
                        SemaError::eval(format!("pio/pull: unexpected option :{kw}"))
                            .with_hint("valid: :block :no-block :ifempty"),
                    )
                }
            }
        }
        Ok(make_instr(
            "pull",
            &[
                ("block", Value::bool(block)),
                ("ifempty", Value::bool(ifempty)),
            ],
        ))
    });

    register_fn(env, "pio/mov", |args| {
        check_arity!(args, "pio/mov", 2..=3);
        let dest = args.keyword_at(0, "pio/mov")?;
        mov_dest(&dest)?;
        let source = args.keyword_at(1, "pio/mov")?;
        parse_mov_source(&source)?; // validate
        let mut fields: Vec<(&str, Value)> = vec![
            ("dest", Value::keyword(&dest)),
            ("source", Value::keyword(&source)),
        ];
        if args.len() == 3 {
            let op = args.keyword_at(2, "pio/mov")?;
            if op != "invert" && op != "reverse" {
                return Err(SemaError::eval(format!("pio/mov: unknown operation :{op}"))
                    .with_hint("valid: :invert :reverse"));
            }
            fields.push(("mov-op", Value::keyword(&op)));
        }
        Ok(make_instr("mov", &fields))
    });

    register_fn(env, "pio/irq", |args| {
        check_arity!(args, "pio/irq", 2..=3);
        let mode = args.keyword_at(0, "pio/irq")?;
        if mode != "set" && mode != "wait" && mode != "clear" {
            return Err(SemaError::eval(format!("pio/irq: unknown mode :{mode}"))
                .with_hint("valid: :set :wait :clear"));
        }
        let index = args.int_at(1, "pio/irq")?;
        if !(0..=7).contains(&index) {
            return Err(SemaError::eval(format!(
                "pio/irq: index {index} out of range 0..7"
            )));
        }
        let mut fields: Vec<(&str, Value)> = vec![
            ("mode", Value::keyword(&mode)),
            ("index", Value::int(index)),
        ];
        if args.len() == 3 {
            let rel = args.keyword_at(2, "pio/irq")?;
            if rel != "rel" {
                return Err(SemaError::eval(format!(
                    "pio/irq: expected :rel, got :{rel}"
                )));
            }
            fields.push(("rel", Value::bool(true)));
        }
        Ok(make_instr("irq", &fields))
    });

    register_fn(env, "pio/set", |args| {
        check_arity!(args, "pio/set", 2);
        let dest = args.keyword_at(0, "pio/set")?;
        set_dest(&dest)?;
        let value = args.int_at(1, "pio/set")?;
        if !(0..=31).contains(&value) {
            return Err(SemaError::eval(format!(
                "pio/set: value {value} out of range 0..31"
            )));
        }
        Ok(make_instr(
            "set",
            &[
                ("dest", Value::keyword(&dest)),
                ("value", Value::int(value)),
            ],
        ))
    });

    // --- composable wrappers ---

    register_fn(env, "pio/side", |args| {
        check_arity!(args, "pio/side", 2);
        let value = args.int_at(0, "pio/side")?;
        if !(0..=31).contains(&value) {
            return Err(SemaError::eval(format!(
                "pio/side: value {value} out of range 0..31"
            )));
        }
        let instr = args[1]
            .as_map_ref()
            .ok_or_else(|| SemaError::type_error("map (instruction)", args[1].type_name()))?;
        let mut new_map = instr.clone();
        new_map.insert(Value::keyword("side-set"), Value::int(value));
        Ok(Value::map(new_map))
    });

    register_fn(env, "pio/delay", |args| {
        check_arity!(args, "pio/delay", 2);
        let cycles = args.int_at(0, "pio/delay")?;
        if !(0..=31).contains(&cycles) {
            return Err(SemaError::eval(format!(
                "pio/delay: cycles {cycles} out of range 0..31"
            )));
        }
        let instr = args[1]
            .as_map_ref()
            .ok_or_else(|| SemaError::type_error("map (instruction)", args[1].type_name()))?;
        let mut new_map = instr.clone();
        new_map.insert(Value::keyword("delay"), Value::int(cycles));
        Ok(Value::map(new_map))
    });

    // --- assembler ---

    register_fn(env, "pio/assemble", assemble);
}
