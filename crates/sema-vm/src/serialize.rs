use hashbrown::HashMap;
use sema_core::{intern, resolve, SemaError, Span, Spur, Value, ValueView};

use crate::chunk::{Chunk, ExceptionEntry, Function, UpvalueDesc};
use crate::compiler::CompileResult;
use crate::opcodes::Op;

/// Builds a deduplicated string table for serialization.
pub struct StringTableBuilder {
    strings: Vec<String>,
    index: HashMap<String, u32>,
}

impl Default for StringTableBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl StringTableBuilder {
    pub fn new() -> Self {
        let mut b = StringTableBuilder {
            strings: Vec::new(),
            index: HashMap::new(),
        };
        b.intern_str(""); // index 0 = empty string
        b
    }

    pub fn intern_str(&mut self, s: &str) -> u32 {
        if let Some(&idx) = self.index.get(s) {
            return idx;
        }
        let idx = self.strings.len() as u32;
        self.strings.push(s.to_string());
        self.index.insert(s.to_string(), idx);
        idx
    }

    pub fn intern_spur(&mut self, spur: Spur) -> u32 {
        let s = resolve(spur);
        self.intern_str(&s)
    }

    pub fn finish(self) -> Vec<String> {
        self.strings
    }
}

// ── Spur remap table ──────────────────────────────────────────────

/// Build a remap table: for each string table index, intern it to get a process-local Spur.
pub fn build_remap_table(table: &[String]) -> Vec<Spur> {
    table.iter().map(|s| intern(s)).collect()
}

// ── Value tag constants (bytecode format) ─────────────────────────

const VAL_NIL: u8 = 0x00;
const VAL_BOOL: u8 = 0x01;
const VAL_INT: u8 = 0x02;
const VAL_FLOAT: u8 = 0x03;
const VAL_STRING: u8 = 0x04;
const VAL_SYMBOL: u8 = 0x05;
const VAL_KEYWORD: u8 = 0x06;
const VAL_CHAR: u8 = 0x07;
const VAL_LIST: u8 = 0x08;
const VAL_VECTOR: u8 = 0x09;
const VAL_MAP: u8 = 0x0A;
const VAL_HASHMAP: u8 = 0x0B;
const VAL_BYTEVECTOR: u8 = 0x0C;

const MAX_VALUE_DEPTH: usize = 128;

// ── Checked conversions ───────────────────────────────────────────

fn checked_u16(n: usize, what: &str) -> Result<u16, SemaError> {
    u16::try_from(n).map_err(|_| SemaError::eval(format!("{what} exceeds u16::MAX ({n})")))
}

fn checked_u32(n: usize, what: &str) -> Result<u32, SemaError> {
    u32::try_from(n).map_err(|_| SemaError::eval(format!("{what} exceeds u32::MAX ({n})")))
}

// ── Value serialization ───────────────────────────────────────────

pub fn serialize_value(
    val: &Value,
    buf: &mut Vec<u8>,
    stb: &mut StringTableBuilder,
) -> Result<(), SemaError> {
    match val.view() {
        ValueView::Nil => buf.push(VAL_NIL),
        ValueView::Bool(b) => {
            buf.push(VAL_BOOL);
            buf.push(if b { 1 } else { 0 });
        }
        ValueView::Int(n) => {
            buf.push(VAL_INT);
            buf.extend_from_slice(&n.to_le_bytes());
        }
        ValueView::Float(f) => {
            buf.push(VAL_FLOAT);
            buf.extend_from_slice(&f.to_le_bytes());
        }
        ValueView::String(s) => {
            buf.push(VAL_STRING);
            let idx = stb.intern_str(&s);
            buf.extend_from_slice(&idx.to_le_bytes());
        }
        ValueView::Symbol(spur) => {
            buf.push(VAL_SYMBOL);
            let idx = stb.intern_spur(spur);
            buf.extend_from_slice(&idx.to_le_bytes());
        }
        ValueView::Keyword(spur) => {
            buf.push(VAL_KEYWORD);
            let idx = stb.intern_spur(spur);
            buf.extend_from_slice(&idx.to_le_bytes());
        }
        ValueView::Char(c) => {
            buf.push(VAL_CHAR);
            buf.extend_from_slice(&(c as u32).to_le_bytes());
        }
        ValueView::List(items) => {
            let len = checked_u16(items.len(), "list length")?;
            buf.push(VAL_LIST);
            buf.extend_from_slice(&len.to_le_bytes());
            for item in items.iter() {
                serialize_value(item, buf, stb)?;
            }
        }
        ValueView::Vector(items) => {
            let len = checked_u16(items.len(), "vector length")?;
            buf.push(VAL_VECTOR);
            buf.extend_from_slice(&len.to_le_bytes());
            for item in items.iter() {
                serialize_value(item, buf, stb)?;
            }
        }
        ValueView::Map(map) => {
            let len = checked_u16(map.len(), "map length")?;
            buf.push(VAL_MAP);
            buf.extend_from_slice(&len.to_le_bytes());
            for (k, v) in map.iter() {
                serialize_value(k, buf, stb)?;
                serialize_value(v, buf, stb)?;
            }
        }
        ValueView::HashMap(map) => {
            let len = checked_u16(map.len(), "hashmap length")?;
            buf.push(VAL_HASHMAP);
            buf.extend_from_slice(&len.to_le_bytes());
            for (k, v) in map.iter() {
                serialize_value(k, buf, stb)?;
                serialize_value(v, buf, stb)?;
            }
        }
        ValueView::Bytevector(bv) => {
            let len = checked_u32(bv.len(), "bytevector length")?;
            buf.push(VAL_BYTEVECTOR);
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(&bv);
        }
        // Runtime-only types cannot appear in bytecode constant pools
        _ => {
            return Err(SemaError::eval(format!(
                "cannot serialize {} to bytecode constant pool",
                val.type_name()
            )));
        }
    }
    Ok(())
}

// ── Value deserialization ─────────────────────────────────────────

fn read_u8(buf: &[u8], cursor: &mut usize) -> Result<u8, SemaError> {
    if *cursor >= buf.len() {
        return Err(SemaError::eval("unexpected end of bytecode data"));
    }
    let v = buf[*cursor];
    *cursor += 1;
    Ok(v)
}

fn read_u16_le(buf: &[u8], cursor: &mut usize) -> Result<u16, SemaError> {
    if *cursor + 2 > buf.len() {
        return Err(SemaError::eval("unexpected end of bytecode data"));
    }
    let v = u16::from_le_bytes([buf[*cursor], buf[*cursor + 1]]);
    *cursor += 2;
    Ok(v)
}

fn read_u32_le(buf: &[u8], cursor: &mut usize) -> Result<u32, SemaError> {
    if *cursor + 4 > buf.len() {
        return Err(SemaError::eval("unexpected end of bytecode data"));
    }
    let v = u32::from_le_bytes([
        buf[*cursor],
        buf[*cursor + 1],
        buf[*cursor + 2],
        buf[*cursor + 3],
    ]);
    *cursor += 4;
    Ok(v)
}

fn read_i64_le(buf: &[u8], cursor: &mut usize) -> Result<i64, SemaError> {
    if *cursor + 8 > buf.len() {
        return Err(SemaError::eval("unexpected end of bytecode data"));
    }
    let v = i64::from_le_bytes(buf[*cursor..*cursor + 8].try_into().unwrap());
    *cursor += 8;
    Ok(v)
}

fn read_f64_le(buf: &[u8], cursor: &mut usize) -> Result<f64, SemaError> {
    if *cursor + 8 > buf.len() {
        return Err(SemaError::eval("unexpected end of bytecode data"));
    }
    let v = f64::from_le_bytes(buf[*cursor..*cursor + 8].try_into().unwrap());
    *cursor += 8;
    Ok(v)
}

fn read_bytes(buf: &[u8], cursor: &mut usize, len: usize) -> Result<Vec<u8>, SemaError> {
    if *cursor + len > buf.len() {
        return Err(SemaError::eval("unexpected end of bytecode data"));
    }
    let v = buf[*cursor..*cursor + len].to_vec();
    *cursor += len;
    Ok(v)
}

pub fn deserialize_value(
    buf: &[u8],
    cursor: &mut usize,
    table: &[String],
    remap: &[Spur],
) -> Result<Value, SemaError> {
    deserialize_value_inner(buf, cursor, table, remap, 0)
}

fn deserialize_value_inner(
    buf: &[u8],
    cursor: &mut usize,
    table: &[String],
    remap: &[Spur],
    depth: usize,
) -> Result<Value, SemaError> {
    if depth > MAX_VALUE_DEPTH {
        return Err(SemaError::eval(format!(
            "value nesting depth exceeds maximum ({MAX_VALUE_DEPTH})"
        )));
    }
    let tag = read_u8(buf, cursor)?;
    match tag {
        VAL_NIL => Ok(Value::nil()),
        VAL_BOOL => {
            let b = read_u8(buf, cursor)?;
            match b {
                0 => Ok(Value::bool(false)),
                1 => Ok(Value::bool(true)),
                _ => Err(SemaError::eval(format!(
                    "invalid bool payload in bytecode: 0x{b:02x}"
                ))),
            }
        }
        VAL_INT => {
            let n = read_i64_le(buf, cursor)?;
            Ok(Value::int(n))
        }
        VAL_FLOAT => {
            let f = read_f64_le(buf, cursor)?;
            Ok(Value::float(f))
        }
        VAL_STRING => {
            let idx = read_u32_le(buf, cursor)? as usize;
            if idx >= table.len() {
                return Err(SemaError::eval(format!(
                    "string table index {idx} out of range (table has {} entries)",
                    table.len()
                )));
            }
            Ok(Value::string(&table[idx]))
        }
        VAL_SYMBOL => {
            let idx = read_u32_le(buf, cursor)? as usize;
            if idx >= remap.len() {
                return Err(SemaError::eval(format!(
                    "string table index {idx} out of range for symbol remap"
                )));
            }
            Ok(Value::symbol_from_spur(remap[idx]))
        }
        VAL_KEYWORD => {
            let idx = read_u32_le(buf, cursor)? as usize;
            if idx >= remap.len() {
                return Err(SemaError::eval(format!(
                    "string table index {idx} out of range for keyword remap"
                )));
            }
            Ok(Value::keyword_from_spur(remap[idx]))
        }
        VAL_CHAR => {
            let cp = read_u32_le(buf, cursor)?;
            let c = char::from_u32(cp)
                .ok_or_else(|| SemaError::eval(format!("invalid unicode code point: {cp}")))?;
            Ok(Value::char(c))
        }
        VAL_LIST => {
            let count = read_u16_le(buf, cursor)? as usize;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                items.push(deserialize_value_inner(
                    buf,
                    cursor,
                    table,
                    remap,
                    depth + 1,
                )?);
            }
            Ok(Value::list(items))
        }
        VAL_VECTOR => {
            let count = read_u16_le(buf, cursor)? as usize;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                items.push(deserialize_value_inner(
                    buf,
                    cursor,
                    table,
                    remap,
                    depth + 1,
                )?);
            }
            Ok(Value::vector(items))
        }
        VAL_MAP => {
            let n_pairs = read_u16_le(buf, cursor)? as usize;
            let mut map = std::collections::BTreeMap::new();
            for _ in 0..n_pairs {
                let k = deserialize_value_inner(buf, cursor, table, remap, depth + 1)?;
                let v = deserialize_value_inner(buf, cursor, table, remap, depth + 1)?;
                map.insert(k, v);
            }
            Ok(Value::map(map))
        }
        VAL_HASHMAP => {
            let n_pairs = read_u16_le(buf, cursor)? as usize;
            let mut entries = Vec::with_capacity(n_pairs);
            for _ in 0..n_pairs {
                let k = deserialize_value_inner(buf, cursor, table, remap, depth + 1)?;
                let v = deserialize_value_inner(buf, cursor, table, remap, depth + 1)?;
                entries.push((k, v));
            }
            Ok(Value::hashmap(entries))
        }
        VAL_BYTEVECTOR => {
            let len = read_u32_le(buf, cursor)? as usize;
            let data = read_bytes(buf, cursor, len)?;
            Ok(Value::bytevector(data))
        }
        _ => Err(SemaError::eval(format!(
            "unknown value tag in bytecode: 0x{tag:02x}"
        ))),
    }
}

// ── Chunk serialization ───────────────────────────────────────────

pub fn serialize_chunk(
    chunk: &Chunk,
    buf: &mut Vec<u8>,
    stb: &mut StringTableBuilder,
) -> Result<(), SemaError> {
    // code — remap Spur operands to string table indices before writing
    let remapped_code = remap_spurs_to_indices(&chunk.code, stb)?;
    let code_len = checked_u32(remapped_code.len(), "bytecode length")?;
    buf.extend_from_slice(&code_len.to_le_bytes());
    buf.extend_from_slice(&remapped_code);

    // constants
    let n_consts = checked_u16(chunk.consts.len(), "constant pool size")?;
    buf.extend_from_slice(&n_consts.to_le_bytes());
    for val in &chunk.consts {
        serialize_value(val, buf, stb)?;
    }

    // spans: Vec<(u32, Span)> where Span { line, col, end_line, end_col }
    let n_spans = checked_u32(chunk.spans.len(), "span count")?;
    buf.extend_from_slice(&n_spans.to_le_bytes());
    for &(pc, ref span) in &chunk.spans {
        buf.extend_from_slice(&pc.to_le_bytes());
        let line = checked_u32(span.line, "span line")?;
        let col = checked_u32(span.col, "span col")?;
        let end_line = checked_u32(span.end_line, "span end_line")?;
        let end_col = checked_u32(span.end_col, "span end_col")?;
        buf.extend_from_slice(&line.to_le_bytes());
        buf.extend_from_slice(&col.to_le_bytes());
        buf.extend_from_slice(&end_line.to_le_bytes());
        buf.extend_from_slice(&end_col.to_le_bytes());
    }

    // max_stack, n_locals, n_global_cache_slots
    buf.extend_from_slice(&chunk.max_stack.to_le_bytes());
    buf.extend_from_slice(&chunk.n_locals.to_le_bytes());
    buf.extend_from_slice(&chunk.n_global_cache_slots.to_le_bytes());

    // exception table
    let n_exceptions = checked_u16(chunk.exception_table.len(), "exception table size")?;
    buf.extend_from_slice(&n_exceptions.to_le_bytes());
    for entry in &chunk.exception_table {
        buf.extend_from_slice(&entry.try_start.to_le_bytes());
        buf.extend_from_slice(&entry.try_end.to_le_bytes());
        buf.extend_from_slice(&entry.handler_pc.to_le_bytes());
        buf.extend_from_slice(&entry.stack_depth.to_le_bytes());
        buf.extend_from_slice(&entry.catch_slot.to_le_bytes());
    }

    Ok(())
}

pub fn deserialize_chunk(
    buf: &[u8],
    cursor: &mut usize,
    table: &[String],
    remap: &[Spur],
) -> Result<Chunk, SemaError> {
    // code — remap string table indices back to process-local Spurs
    let code_len = read_u32_le(buf, cursor)? as usize;
    let remaining = buf.len().saturating_sub(*cursor);
    if code_len > remaining {
        return Err(SemaError::eval(format!(
            "bytecode code_len ({code_len}) exceeds remaining data ({remaining})"
        )));
    }
    let mut code = read_bytes(buf, cursor, code_len)?;
    remap_indices_to_spurs(&mut code, remap)?;

    // constants
    let n_consts = read_u16_le(buf, cursor)? as usize;
    let mut consts = Vec::with_capacity(n_consts);
    for _ in 0..n_consts {
        consts.push(deserialize_value(buf, cursor, table, remap)?);
    }

    // spans (each span = 20 bytes: u32 pc + u32 line + u32 col + u32 end_line + u32 end_col)
    let n_spans = read_u32_le(buf, cursor)? as usize;
    let span_remaining = buf.len().saturating_sub(*cursor);
    if n_spans
        .checked_mul(20)
        .is_none_or(|need| need > span_remaining)
    {
        return Err(SemaError::eval(format!(
            "span count ({n_spans}) exceeds remaining data ({span_remaining} bytes)"
        )));
    }
    let mut spans = Vec::with_capacity(n_spans);
    for _ in 0..n_spans {
        let pc = read_u32_le(buf, cursor)?;
        let line = read_u32_le(buf, cursor)? as usize;
        let col = read_u32_le(buf, cursor)? as usize;
        let end_line = read_u32_le(buf, cursor)? as usize;
        let end_col = read_u32_le(buf, cursor)? as usize;
        spans.push((pc, Span::new(line, col, end_line, end_col)));
    }

    // max_stack, n_locals, n_global_cache_slots
    let max_stack = read_u16_le(buf, cursor)?;
    let n_locals = read_u16_le(buf, cursor)?;
    let n_global_cache_slots = read_u16_le(buf, cursor)?;

    // exception table
    let n_exceptions = read_u16_le(buf, cursor)? as usize;
    let mut exception_table = Vec::with_capacity(n_exceptions);
    for _ in 0..n_exceptions {
        let try_start = read_u32_le(buf, cursor)?;
        let try_end = read_u32_le(buf, cursor)?;
        let handler_pc = read_u32_le(buf, cursor)?;
        let stack_depth = read_u16_le(buf, cursor)?;
        let catch_slot = read_u16_le(buf, cursor)?;
        exception_table.push(ExceptionEntry {
            try_start,
            try_end,
            handler_pc,
            stack_depth,
            catch_slot,
        });
    }

    Ok(Chunk {
        code,
        consts,
        spans,
        max_stack,
        n_locals,
        exception_table,
        n_global_cache_slots,
    })
}

// ── Function serialization ────────────────────────────────────────

const ANONYMOUS_NAME: u32 = 0xFFFF_FFFF;

pub fn serialize_function(
    func: &Function,
    buf: &mut Vec<u8>,
    stb: &mut StringTableBuilder,
) -> Result<(), SemaError> {
    // name: u32 string table index (0xFFFFFFFF = anonymous)
    match func.name {
        Some(spur) => {
            let idx = stb.intern_spur(spur);
            buf.extend_from_slice(&idx.to_le_bytes());
        }
        None => buf.extend_from_slice(&ANONYMOUS_NAME.to_le_bytes()),
    }

    // arity: u16
    buf.extend_from_slice(&func.arity.to_le_bytes());

    // has_rest: u8
    buf.push(if func.has_rest { 1 } else { 0 });

    if func.upvalue_names.len() != func.upvalue_descs.len() {
        return Err(SemaError::eval(format!(
            "function upvalue debug name count ({}) does not match upvalue descriptor count ({})",
            func.upvalue_names.len(),
            func.upvalue_descs.len()
        )));
    }

    // upvalue descriptors
    let n_upvalues = checked_u16(func.upvalue_descs.len(), "upvalue descriptor count")?;
    buf.extend_from_slice(&n_upvalues.to_le_bytes());
    for desc in &func.upvalue_descs {
        match desc {
            UpvalueDesc::ParentLocal(idx) => {
                buf.push(0);
                buf.extend_from_slice(&idx.to_le_bytes());
            }
            UpvalueDesc::ParentUpvalue(idx) => {
                buf.push(1);
                buf.extend_from_slice(&idx.to_le_bytes());
            }
        }
    }
    let n_upvalue_names = checked_u16(func.upvalue_names.len(), "upvalue name count")?;
    buf.extend_from_slice(&n_upvalue_names.to_le_bytes());
    for &spur in &func.upvalue_names {
        let idx = stb.intern_spur(spur);
        buf.extend_from_slice(&idx.to_le_bytes());
    }

    // chunk
    serialize_chunk(&func.chunk, buf, stb)?;

    // local_names: Vec<(u16, Spur)>
    let n_local_names = checked_u16(func.local_names.len(), "local name count")?;
    buf.extend_from_slice(&n_local_names.to_le_bytes());
    for &(slot, spur) in &func.local_names {
        buf.extend_from_slice(&slot.to_le_bytes());
        let idx = stb.intern_spur(spur);
        buf.extend_from_slice(&idx.to_le_bytes());
    }

    // local_scopes: Vec<(u16 slot, u32 start_pc, u32 end_pc)> — block-scope debug
    // metadata used by the debugger to hide out-of-scope locals.
    let n_local_scopes = checked_u16(func.local_scopes.len(), "local scope count")?;
    buf.extend_from_slice(&n_local_scopes.to_le_bytes());
    for &(slot, start_pc, end_pc) in &func.local_scopes {
        buf.extend_from_slice(&slot.to_le_bytes());
        buf.extend_from_slice(&start_pc.to_le_bytes());
        buf.extend_from_slice(&end_pc.to_le_bytes());
    }

    Ok(())
}

pub fn deserialize_function(
    buf: &[u8],
    cursor: &mut usize,
    table: &[String],
    remap: &[Spur],
) -> Result<Function, SemaError> {
    // name
    let name_idx = read_u32_le(buf, cursor)?;
    let name = if name_idx == ANONYMOUS_NAME {
        None
    } else {
        let idx = name_idx as usize;
        if idx >= remap.len() {
            return Err(SemaError::eval(format!(
                "function name string table index {idx} out of range"
            )));
        }
        Some(remap[idx])
    };

    // arity
    let arity = read_u16_le(buf, cursor)?;

    // has_rest
    let has_rest_byte = read_u8(buf, cursor)?;
    let has_rest = match has_rest_byte {
        0 => false,
        1 => true,
        _ => {
            return Err(SemaError::eval(format!(
                "invalid has_rest byte: 0x{has_rest_byte:02x}"
            )));
        }
    };

    // upvalue descriptors
    let n_upvalues = read_u16_le(buf, cursor)? as usize;
    let mut upvalue_descs = Vec::with_capacity(n_upvalues);
    for _ in 0..n_upvalues {
        let kind = read_u8(buf, cursor)?;
        let index = read_u16_le(buf, cursor)?;
        match kind {
            0 => upvalue_descs.push(UpvalueDesc::ParentLocal(index)),
            1 => upvalue_descs.push(UpvalueDesc::ParentUpvalue(index)),
            _ => {
                return Err(SemaError::eval(format!(
                    "invalid upvalue kind: 0x{kind:02x}"
                )));
            }
        }
    }
    let n_upvalue_names = read_u16_le(buf, cursor)? as usize;
    let mut upvalue_names = Vec::with_capacity(n_upvalue_names);
    for _ in 0..n_upvalue_names {
        let name_idx = read_u32_le(buf, cursor)? as usize;
        if name_idx >= remap.len() {
            return Err(SemaError::eval(format!(
                "upvalue name string table index {name_idx} out of range"
            )));
        }
        upvalue_names.push(remap[name_idx]);
    }
    if upvalue_names.len() != upvalue_descs.len() {
        return Err(SemaError::eval(format!(
            "upvalue name count ({}) does not match upvalue descriptor count ({})",
            upvalue_names.len(),
            upvalue_descs.len()
        )));
    }

    // chunk
    let chunk = deserialize_chunk(buf, cursor, table, remap)?;

    // local_names
    let n_local_names = read_u16_le(buf, cursor)? as usize;
    let mut local_names = Vec::with_capacity(n_local_names);
    for _ in 0..n_local_names {
        let slot = read_u16_le(buf, cursor)?;
        let name_idx = read_u32_le(buf, cursor)? as usize;
        if name_idx >= remap.len() {
            return Err(SemaError::eval(format!(
                "local name string table index {name_idx} out of range"
            )));
        }
        local_names.push((slot, remap[name_idx]));
    }

    // local_scopes: Vec<(u16 slot, u32 start_pc, u32 end_pc)>
    // Each entry is 10 bytes (u16 + u32 + u32); bounds-check the count against the
    // remaining buffer so a crafted count cannot trigger an unbounded allocation.
    let n_local_scopes = read_u16_le(buf, cursor)? as usize;
    let scopes_remaining = buf.len().saturating_sub(*cursor);
    if n_local_scopes
        .checked_mul(10)
        .is_none_or(|need| need > scopes_remaining)
    {
        return Err(SemaError::eval(format!(
            "local scope count ({n_local_scopes}) exceeds remaining data ({scopes_remaining} bytes)"
        )));
    }
    let mut local_scopes = Vec::with_capacity(n_local_scopes);
    for _ in 0..n_local_scopes {
        let slot = read_u16_le(buf, cursor)?;
        let start_pc = read_u32_le(buf, cursor)?;
        let end_pc = read_u32_le(buf, cursor)?;
        local_scopes.push((slot, start_pc, end_pc));
    }

    Ok(Function {
        name,
        chunk,
        upvalue_descs,
        upvalue_names,
        arity,
        has_rest,
        local_names,
        local_scopes,
        source_file: None,
        cache_offset: 0,
    })
}

// ── Spur remapping in bytecode ────────────────────────────────────

fn spur_to_u32(spur: Spur) -> u32 {
    spur.into_inner().get()
}

fn u32_to_spur(bits: u32) -> Spur {
    use lasso::Key;
    let idx = bits
        .checked_sub(1)
        .expect("invalid Spur bits: 0 is not valid");
    Spur::try_from_usize(idx as usize).expect("invalid Spur bits")
}

/// Compute the next PC after the instruction at `code[pc]`, validating operand bounds.
fn advance_pc(code: &[u8], pc: usize) -> Result<(Op, usize), SemaError> {
    let Some(op) = Op::from_u8(code[pc]) else {
        return Err(SemaError::eval(format!(
            "invalid opcode 0x{:02x} at pc {pc}",
            code[pc]
        )));
    };
    let next = match op {
        Op::StoreGlobal | Op::DefineGlobal => pc + 5, // op + u32
        Op::LoadGlobal => pc + 7,                     // op + u32 + u16 cache_slot
        Op::CallGlobal => pc + 9,                     // op + u32 + u16 + u16 cache_slot
        Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue => pc + 5, // op + i32
        Op::CallNative => pc + 5,                     // op + u16 + u16
        Op::MakeClosure => {
            if pc + 5 > code.len() {
                return Err(SemaError::eval(format!(
                    "truncated MakeClosure operands at pc {pc}"
                )));
            }
            let n_upvalues = u16::from_le_bytes([code[pc + 3], code[pc + 4]]) as usize;
            pc + 5 + n_upvalues * 4
        }
        Op::Const
        | Op::LoadLocal
        | Op::StoreLocal
        | Op::LoadUpvalue
        | Op::StoreUpvalue
        | Op::Call
        | Op::TailCall
        | Op::MakeList
        | Op::MakeVector
        | Op::MakeMap
        | Op::MakeHashMap => pc + 3, // op + u16
        _ => pc + 1, // single-byte
    };
    if next > code.len() {
        return Err(SemaError::eval(format!(
            "truncated operand for {:?} at pc {pc} (need {} bytes, have {})",
            op,
            next - pc,
            code.len() - pc
        )));
    }
    Ok((op, next))
}

/// Walk bytecode and rewrite global opcodes: Spur u32 → string table index.
/// Returns the rewritten code.
pub fn remap_spurs_to_indices(
    code: &[u8],
    stb: &mut StringTableBuilder,
) -> Result<Vec<u8>, SemaError> {
    let mut out = code.to_vec();
    let mut pc = 0;
    while pc < out.len() {
        let (op, next) = advance_pc(&out, pc)?;
        if matches!(
            op,
            Op::LoadGlobal | Op::StoreGlobal | Op::DefineGlobal | Op::CallGlobal
        ) {
            let spur_bits =
                u32::from_le_bytes([out[pc + 1], out[pc + 2], out[pc + 3], out[pc + 4]]);
            let spur = u32_to_spur(spur_bits);
            let s = resolve(spur);
            let idx = stb.intern_str(&s);
            let bytes = idx.to_le_bytes();
            out[pc + 1] = bytes[0];
            out[pc + 2] = bytes[1];
            out[pc + 3] = bytes[2];
            out[pc + 4] = bytes[3];
        }
        pc = next;
    }
    Ok(out)
}

/// Walk bytecode and rewrite global opcodes: string table index → process-local Spur u32.
pub fn remap_indices_to_spurs(code: &mut [u8], remap: &[Spur]) -> Result<(), SemaError> {
    let mut pc = 0;
    while pc < code.len() {
        let (op, next) = advance_pc(code, pc)?;
        if matches!(
            op,
            Op::LoadGlobal | Op::StoreGlobal | Op::DefineGlobal | Op::CallGlobal
        ) {
            let idx = u32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]])
                as usize;
            if idx >= remap.len() {
                return Err(SemaError::eval(format!(
                    "global spur remap index {idx} out of range at pc {pc}"
                )));
            }
            let spur_bits = spur_to_u32(remap[idx]);
            let bytes = spur_bits.to_le_bytes();
            code[pc + 1] = bytes[0];
            code[pc + 2] = bytes[1];
            code[pc + 3] = bytes[2];
            code[pc + 4] = bytes[3];
        }
        pc = next;
    }
    Ok(())
}

// ── File format constants ─────────────────────────────────────────

const MAGIC: [u8; 4] = [0x00, b'S', b'E', b'M'];
const FORMAT_VERSION: u16 = 4;
const SECTION_STRING_TABLE: u16 = 0x01;
const SECTION_FUNCTION_TABLE: u16 = 0x02;
const SECTION_MAIN_CHUNK: u16 = 0x03;

// ── Full file serialization ───────────────────────────────────────

/// Serialize a CompileResult to the .semac binary format.
pub fn serialize_to_bytes(result: &CompileResult, source_hash: u32) -> Result<Vec<u8>, SemaError> {
    let mut stb = StringTableBuilder::new();

    // Pre-serialize sections to get their bytes
    // We need to serialize functions and main chunk first to populate the string table,
    // then serialize the string table.

    // Function table section payload
    let mut func_payload = Vec::new();
    let n_funcs = checked_u32(result.functions.len(), "function count")?;
    func_payload.extend_from_slice(&n_funcs.to_le_bytes());
    for func in &result.functions {
        serialize_function(func, &mut func_payload, &mut stb)?;
    }

    // Main chunk section payload
    let mut chunk_payload = Vec::new();
    serialize_chunk(&result.chunk, &mut chunk_payload, &mut stb)?;

    // Now build the string table section payload
    let string_table = stb.finish();
    let mut strtab_payload = Vec::new();
    let n_strings = checked_u32(string_table.len(), "string table size")?;
    strtab_payload.extend_from_slice(&n_strings.to_le_bytes());
    for s in &string_table {
        let bytes = s.as_bytes();
        let len = checked_u32(bytes.len(), "string length")?;
        strtab_payload.extend_from_slice(&len.to_le_bytes());
        strtab_payload.extend_from_slice(bytes);
    }

    // Assemble the file
    let n_sections: u16 = 3; // string table + function table + main chunk
    let mut out = Vec::new();

    // Header (24 bytes)
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // flags
                                                // Sema version — parse from Cargo.toml version at compile time
    let (major, minor, patch) = parse_sema_version();
    out.extend_from_slice(&major.to_le_bytes());
    out.extend_from_slice(&minor.to_le_bytes());
    out.extend_from_slice(&patch.to_le_bytes());
    out.extend_from_slice(&n_sections.to_le_bytes());
    out.extend_from_slice(&source_hash.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // reserved

    // Section: String Table
    write_section(&mut out, SECTION_STRING_TABLE, &strtab_payload)?;
    // Section: Function Table
    write_section(&mut out, SECTION_FUNCTION_TABLE, &func_payload)?;
    // Section: Main Chunk
    write_section(&mut out, SECTION_MAIN_CHUNK, &chunk_payload)?;

    Ok(out)
}

fn write_section(out: &mut Vec<u8>, section_type: u16, payload: &[u8]) -> Result<(), SemaError> {
    let len = checked_u32(payload.len(), "section payload length")?;
    out.extend_from_slice(&section_type.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

fn parse_sema_version() -> (u16, u16, u16) {
    let version = env!("CARGO_PKG_VERSION");
    let parts: Vec<&str> = version.split('.').collect();
    let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

/// Validate bytecode operand bounds after deserialization.
fn validate_bytecode(result: &CompileResult) -> Result<(), SemaError> {
    // The native table is process-local and is NOT serialized in the .semac
    // format, so a deserialized `CompileResult` carries an empty `native_table`
    // (the VM resolves natives via the shared global env using CallGlobal). Any
    // CallNative opcode in loaded bytecode therefore has no valid backing entry,
    // and `native_id < n_natives` rejects it — matching the runtime invariant
    // that loaded bytecode is run with an empty native table.
    let n_natives = result.native_table.len();
    validate_chunk_bytecode(
        &result.chunk,
        result.functions.len(),
        0,
        n_natives,
        "main chunk",
    )?;
    for (i, func) in result.functions.iter().enumerate() {
        let label = format!("function {i}");
        let n_upvalues = func.upvalue_descs.len();
        validate_chunk_bytecode(
            &func.chunk,
            result.functions.len(),
            n_upvalues,
            n_natives,
            &label,
        )?;
    }
    Ok(())
}

fn validate_chunk_bytecode(
    chunk: &Chunk,
    n_functions: usize,
    n_upvalues: usize,
    n_natives: usize,
    label: &str,
) -> Result<(), SemaError> {
    let code = &chunk.code;
    let n_locals = chunk.n_locals as usize;

    // First pass: collect valid instruction boundaries and validate operand indices
    let mut valid_pcs = std::collections::HashSet::new();
    let mut jump_targets: Vec<(usize, isize)> = Vec::new(); // (source_pc, target_pc)
    let mut pc = 0;
    while pc < code.len() {
        valid_pcs.insert(pc);
        let (op, next) = advance_pc(code, pc)?;
        match op {
            Op::Const => {
                let idx = u16::from_le_bytes([code[pc + 1], code[pc + 2]]) as usize;
                if idx >= chunk.consts.len() {
                    return Err(SemaError::eval(format!(
                        "in {label}: Const index {idx} out of range (pool has {} entries) at pc {pc}",
                        chunk.consts.len()
                    )));
                }
            }
            Op::MakeClosure => {
                let func_id = u16::from_le_bytes([code[pc + 1], code[pc + 2]]) as usize;
                if func_id >= n_functions {
                    return Err(SemaError::eval(format!(
                        "in {label}: MakeClosure func_id {func_id} out of range ({n_functions} functions) at pc {pc}",
                    )));
                }
            }
            Op::LoadLocal | Op::StoreLocal => {
                let slot = u16::from_le_bytes([code[pc + 1], code[pc + 2]]) as usize;
                if slot >= n_locals {
                    return Err(SemaError::eval(format!(
                        "in {label}: local slot {slot} out of range (n_locals={n_locals}) at pc {pc}",
                    )));
                }
            }
            Op::LoadUpvalue | Op::StoreUpvalue => {
                let slot = u16::from_le_bytes([code[pc + 1], code[pc + 2]]) as usize;
                if slot >= n_upvalues {
                    return Err(SemaError::eval(format!(
                        "in {label}: upvalue slot {slot} out of range (n_upvalues={n_upvalues}) at pc {pc}",
                    )));
                }
            }
            Op::CallNative => {
                // CallNative = op + u16 native_id + u16 argc. The native_id
                // indexes the VM's resolved native table at runtime; an
                // out-of-range id would index past it (a release-build OOB
                // guarded only by a debug_assert in vm.rs). Reject it here.
                let native_id = u16::from_le_bytes([code[pc + 1], code[pc + 2]]) as usize;
                if native_id >= n_natives {
                    return Err(SemaError::eval(format!(
                        "in {label}: CallNative native_id {native_id} out of range (table has {n_natives} entries) at pc {pc}",
                    )));
                }
            }
            Op::Jump | Op::JumpIfFalse | Op::JumpIfTrue => {
                let offset =
                    i32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                // Jump offset is relative to the end of the instruction (next)
                let target = next as isize + offset as isize;
                jump_targets.push((pc, target));
            }
            _ => {}
        }
        pc = next;
    }
    // code.len() is also valid (end-of-code, reachable by forward jumps)
    valid_pcs.insert(code.len());

    // Second pass: validate all jump targets land on instruction boundaries
    for (source_pc, target) in jump_targets {
        if target < 0 || target as usize > code.len() {
            return Err(SemaError::eval(format!(
                "in {label}: jump at pc {source_pc} targets out-of-bounds pc {target}",
            )));
        }
        if !valid_pcs.contains(&(target as usize)) {
            return Err(SemaError::eval(format!(
                "in {label}: jump at pc {source_pc} targets non-instruction boundary pc {target}",
            )));
        }
    }

    // Third pass: abstract stack-depth verification. This is the precondition
    // that makes `vm.rs::pop_unchecked` sound for deserialized bytecode — it
    // proves no reachable opcode can pop from an empty operand stack.
    verify_stack_balance(chunk, n_locals, label)?;

    Ok(())
}

/// Maximum operand-stack depth the verifier will tolerate. A well-formed chunk
/// from the in-process compiler stays far below this; the bound exists purely to
/// reject crafted bytecode that would otherwise grow the abstract depth without
/// limit (e.g. a `Dup` loop) and to keep `max_stack` within `u16`.
const MAX_STACK_DEPTH: i64 = 65535;

/// Decode the variable-arity operand (argc / element count / pair count) that an
/// opcode's stack effect depends on. Fixed-arity opcodes return 0. Operand bytes
/// are already proven in-bounds by `advance_pc` during the first pass, but we
/// re-check defensively so this function is sound in isolation.
fn stack_effect_operand(code: &[u8], pc: usize, op: Op) -> Result<u16, SemaError> {
    let read_u16_at = |off: usize| -> Result<u16, SemaError> {
        if pc + off + 2 > code.len() {
            return Err(SemaError::eval(format!(
                "truncated operand for {op:?} at pc {pc}"
            )));
        }
        Ok(u16::from_le_bytes([code[pc + off], code[pc + off + 1]]))
    };
    match op {
        // u16 operand immediately after the opcode byte
        Op::Call | Op::TailCall | Op::MakeList | Op::MakeVector | Op::MakeMap | Op::MakeHashMap => {
            read_u16_at(1)
        }
        // u16 native_id + u16 argc → argc is the second u16
        Op::CallNative => read_u16_at(3),
        // u32 spur + u16 argc + u16 cache_slot → argc is the u16 after the spur
        Op::CallGlobal => read_u16_at(5),
        _ => Ok(0),
    }
}

/// Sound, conservative abstract-interpretation pass over a chunk's bytecode that
/// proves the operand stack never underflows and never exceeds `MAX_STACK_DEPTH`.
///
/// Uses a worklist over instruction boundaries, tracking the operand-stack depth
/// on entry to each pc. Join points must agree exactly (strict equality, like the
/// JVM/CLR verifiers) — a disagreement means the bytecode is malformed and is
/// rejected. Exception handlers are seeded as additional roots with their known
/// entry depth. The verifier never accepts an underflowing chunk; it may reject
/// some exotic-but-safe bytecode that a real optimizing compiler could emit, but
/// Sema's compiler only produces structured control flow that converges.
fn verify_stack_balance(chunk: &Chunk, n_locals: usize, label: &str) -> Result<(), SemaError> {
    let code = &chunk.code;
    if code.is_empty() {
        return Ok(());
    }

    // entry_depth[pc] = operand-stack depth on entry to the instruction at pc.
    let mut entry_depth: std::collections::HashMap<usize, i64> = std::collections::HashMap::new();
    let mut worklist: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    let mut max_depth: i64 = 0;

    // Root 0: normal entry with empty operand stack.
    entry_depth.insert(0, 0);
    worklist.push_back(0);

    // Exception handlers are reachable from any pc in their protected range. The
    // runtime truncates the operand stack to `stack_depth` and pushes the caught
    // error value, then the handler's first op (StoreLocal catch_slot) pops it.
    // So the handler's operand-stack entry depth is `stack_depth - n_locals + 1`.
    let n_locals_i = n_locals as i64;
    for entry in &chunk.exception_table {
        let handler_pc = entry.handler_pc as usize;
        if handler_pc >= code.len() {
            return Err(SemaError::eval(format!(
                "in {label}: exception handler_pc {handler_pc} out of range (code len {})",
                code.len()
            )));
        }
        let depth = entry.stack_depth as i64 - n_locals_i + 1;
        if depth < 0 {
            return Err(SemaError::eval(format!(
                "in {label}: exception handler at pc {handler_pc} has negative operand depth {depth}",
            )));
        }
        seed_or_join(&mut entry_depth, &mut worklist, handler_pc, depth, label)?;
    }

    while let Some(pc) = worklist.pop_front() {
        let depth = entry_depth[&pc];
        let (op, next) = advance_pc(code, pc)?;
        let operand = stack_effect_operand(code, pc, op)?;
        let effect = op.stack_effect(operand);

        let pops = effect.pops as i64;
        if depth < pops {
            return Err(SemaError::eval(format!(
                "in {label}: stack underflow at pc {pc}: {op:?} pops {pops} but operand stack depth is {depth}",
            )));
        }
        let after = depth - pops + effect.pushes as i64;
        // The peak the runtime touches at this op is the larger of the entry
        // depth and the post-effect depth (pushes can grow it above entry).
        max_depth = max_depth.max(depth).max(after);
        if max_depth > MAX_STACK_DEPTH {
            return Err(SemaError::eval(format!(
                "in {label}: operand stack depth {max_depth} exceeds maximum ({MAX_STACK_DEPTH}) at pc {pc}",
            )));
        }

        if effect.exits_frame {
            // `Return`/`TailCall`/`Throw` each pop one value (already checked via
            // `effect.pops` above, so `depth >= 1` holds here). The runtime's
            // `Return` additionally tolerates extra leftover operands and an empty
            // stack (substituting nil), so we do not require an exact depth — only
            // that the pop the opcode performs cannot underflow, which the generic
            // `depth < pops` check already guarantees. These ops have no
            // intra-frame successors.
            continue;
        }

        // Successors: fallthrough and/or branch target (at most two).
        let mut successors: Vec<usize> = Vec::with_capacity(2);
        match op {
            Op::Jump => {
                let offset =
                    i32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                successors.push((next as i64 + offset as i64) as usize);
            }
            Op::JumpIfFalse | Op::JumpIfTrue => {
                let offset =
                    i32::from_le_bytes([code[pc + 1], code[pc + 2], code[pc + 3], code[pc + 4]]);
                successors.push(next); // not-taken fallthrough
                successors.push((next as i64 + offset as i64) as usize); // taken
            }
            _ => successors.push(next),
        }

        for succ in successors {
            if succ >= code.len() {
                // The only safe way to leave a frame is via a frame-exiting op
                // (handled above). Falling off the end means missing Return.
                return Err(SemaError::eval(format!(
                    "in {label}: control falls off the end of the chunk at pc {pc}",
                )));
            }
            seed_or_join(&mut entry_depth, &mut worklist, succ, after, label)?;
        }
    }

    // Validate exception handlers against the COMPUTED operand depths, not just
    // the file-supplied `stack_depth`. On a throw the runtime does a shrink-only
    // `truncate(base + stack_depth)` then pushes the error, so the handler runs
    // with exactly `stack_depth - n_locals + 1` operands ONLY IF the operand
    // depth at the throw site was at least `stack_depth - n_locals`. A throw can
    // fire at ANY op in the protected range (not just `Throw` — type errors,
    // arity errors, etc. all raise). So every reachable pc in [try_start,try_end)
    // must hold at least `stack_depth - n_locals` operands; otherwise a crafted
    // inflated `stack_depth` would make the truncate a no-op and the handler
    // underflow `pop_unchecked`. This is what makes the handler seeds above sound
    // for untrusted bytecode.
    for entry in &chunk.exception_table {
        let needed = entry.stack_depth as i64 - n_locals_i;
        let try_start = entry.try_start as usize;
        let try_end = entry.try_end as usize;
        for (&pc, &depth) in &entry_depth {
            if pc >= try_start && pc < try_end && depth < needed {
                return Err(SemaError::eval(format!(
                    "in {label}: exception handler assumes {needed} operands (stack_depth {}, n_locals {n_locals}), but pc {pc} in protected range [{try_start},{try_end}) has operand depth {depth}",
                    entry.stack_depth
                )));
            }
        }
    }

    Ok(())
}

/// Record the operand-stack depth on entry to `pc`. If `pc` was already visited
/// with a different depth, the bytecode joins control flow at inconsistent stack
/// heights — reject it (strict-equality lattice).
fn seed_or_join(
    entry_depth: &mut std::collections::HashMap<usize, i64>,
    worklist: &mut std::collections::VecDeque<usize>,
    pc: usize,
    depth: i64,
    label: &str,
) -> Result<(), SemaError> {
    match entry_depth.get(&pc) {
        None => {
            entry_depth.insert(pc, depth);
            worklist.push_back(pc);
            Ok(())
        }
        Some(&existing) if existing == depth => Ok(()),
        Some(&existing) => Err(SemaError::eval(format!(
            "in {label}: stack depth disagreement at pc {pc}: {existing} vs {depth}",
        ))),
    }
}

/// Deserialize a .semac file from bytes into a CompileResult.
pub fn deserialize_from_bytes(bytes: &[u8]) -> Result<CompileResult, SemaError> {
    if bytes.len() < 24 {
        return Err(SemaError::eval(
            "bytecode file too short (< 24 bytes header)",
        ));
    }

    // Validate header
    if bytes[0..4] != MAGIC {
        return Err(SemaError::eval(
            "invalid bytecode magic number (expected \\x00SEM)",
        ));
    }
    let format_version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if format_version != FORMAT_VERSION {
        return Err(SemaError::eval(format!(
            "unsupported bytecode format version {format_version} (expected {FORMAT_VERSION}). Recompile from source."
        )));
    }
    let reserved = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if reserved != 0 {
        return Err(SemaError::eval(format!(
            "non-zero reserved header field (0x{reserved:08x}); file may be from a newer Sema version"
        )));
    }
    let n_sections = u16::from_le_bytes([bytes[14], bytes[15]]) as usize;

    // Read sections
    let mut cursor = 24;
    let mut string_table: Option<Vec<String>> = None;
    let mut func_table_data: Option<(usize, usize)> = None; // (start, len) in bytes
    let mut main_chunk_data: Option<(usize, usize)> = None;

    for _ in 0..n_sections {
        if cursor + 6 > bytes.len() {
            return Err(SemaError::eval(
                "unexpected end of bytecode file in section header",
            ));
        }
        let section_type = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]);
        let section_len = u32::from_le_bytes([
            bytes[cursor + 2],
            bytes[cursor + 3],
            bytes[cursor + 4],
            bytes[cursor + 5],
        ]) as usize;
        cursor += 6;

        if cursor + section_len > bytes.len() {
            return Err(SemaError::eval(format!(
                "section 0x{section_type:04x} claims {section_len} bytes but only {} remain",
                bytes.len() - cursor
            )));
        }

        match section_type {
            0x01 => {
                // String Table — slice to section boundary
                let section_data = &bytes[cursor..cursor + section_len];
                let mut sc = 0usize;
                let count = read_u32_le(section_data, &mut sc)? as usize;
                // Each string needs at least 4 bytes for its length prefix;
                // use remaining bytes after reading count
                let remaining_after_count = section_len.saturating_sub(sc);
                if count > remaining_after_count / 4 {
                    return Err(SemaError::eval(format!(
                        "string table count ({count}) exceeds section capacity"
                    )));
                }
                let mut table = Vec::with_capacity(count);
                for _ in 0..count {
                    let len = read_u32_le(section_data, &mut sc)? as usize;
                    if sc + len > section_len {
                        return Err(SemaError::eval("string table entry extends past section"));
                    }
                    let s = std::str::from_utf8(&section_data[sc..sc + len]).map_err(|e| {
                        SemaError::eval(format!("invalid UTF-8 in string table: {e}"))
                    })?;
                    table.push(s.to_string());
                    sc += len;
                }
                string_table = Some(table);
            }
            0x02 => {
                func_table_data = Some((cursor, section_len));
            }
            0x03 => {
                main_chunk_data = Some((cursor, section_len));
            }
            _ => {
                // Unknown section — skip for forward compatibility
            }
        }
        cursor += section_len;
    }

    // Validate required sections
    let table = string_table
        .ok_or_else(|| SemaError::eval("bytecode file missing string table section"))?;
    if table.is_empty() || !table[0].is_empty() {
        return Err(SemaError::eval(
            "string table index 0 must be the empty string",
        ));
    }
    let (func_start, func_len) = func_table_data
        .ok_or_else(|| SemaError::eval("bytecode file missing function table section"))?;
    let (chunk_start, chunk_len) = main_chunk_data
        .ok_or_else(|| SemaError::eval("bytecode file missing main chunk section"))?;

    let remap = build_remap_table(&table);

    // Deserialize function table (sliced to section boundary)
    let func_section = &bytes[func_start..func_start + func_len];
    let mut fc = 0;
    let n_funcs = read_u32_le(func_section, &mut fc)? as usize;
    // Each function needs at least several bytes; use 4 as minimum
    if n_funcs > func_len / 4 {
        return Err(SemaError::eval(format!(
            "function count ({n_funcs}) exceeds section capacity"
        )));
    }
    let mut functions = Vec::with_capacity(n_funcs);
    for _ in 0..n_funcs {
        functions.push(deserialize_function(func_section, &mut fc, &table, &remap)?);
    }
    if fc != func_len {
        return Err(SemaError::eval(format!(
            "function table section has {} unconsumed trailing bytes",
            func_len - fc
        )));
    }

    // Deserialize main chunk (sliced to section boundary)
    let chunk_section = &bytes[chunk_start..chunk_start + chunk_len];
    let mut cc = 0;
    let chunk = deserialize_chunk(chunk_section, &mut cc, &table, &remap)?;
    if cc != chunk_len {
        return Err(SemaError::eval(format!(
            "main chunk section has {} unconsumed trailing bytes",
            chunk_len - cc
        )));
    }

    let result = CompileResult::new(chunk, functions);
    validate_bytecode(&result)?;
    Ok(result)
}

/// Check if a byte buffer starts with the .semac magic number.
pub fn is_bytecode_file(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0..4] == MAGIC
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::intern;

    #[test]
    fn test_string_table_builder() {
        let mut builder = StringTableBuilder::new();
        // Index 0 is always ""
        assert_eq!(builder.intern_str(""), 0);
        let idx_hello = builder.intern_str("hello");
        let idx_world = builder.intern_str("world");
        let idx_hello2 = builder.intern_str("hello");
        assert_eq!(idx_hello, idx_hello2); // deduplication
        assert_ne!(idx_hello, idx_world);

        let table = builder.finish();
        assert_eq!(table.len(), 3); // "", "hello", "world"
        assert_eq!(table[0], "");
        assert_eq!(table[idx_hello as usize], "hello");
        assert_eq!(table[idx_world as usize], "world");
    }

    #[test]
    fn test_string_table_spur_interning() {
        let mut builder = StringTableBuilder::new();
        let spur = intern("my-var");
        let idx = builder.intern_spur(spur);
        assert!(idx > 0);
        let idx2 = builder.intern_spur(spur);
        assert_eq!(idx, idx2);
    }

    #[test]
    fn test_chunk_roundtrip() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        let mut e = Emitter::new();
        e.emit_const(Value::int(42)).unwrap();
        e.emit_const(Value::string("hello")).unwrap();
        e.emit_op(Op::Add);
        e.emit_op(Op::Return);
        let mut chunk = e.into_chunk();
        chunk.n_locals = 2;
        chunk.max_stack = 4;

        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        serialize_chunk(&chunk, &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let chunk2 = deserialize_chunk(&buf, &mut cursor, &table, &remap).unwrap();

        assert_eq!(chunk2.code, chunk.code);
        assert_eq!(chunk2.consts.len(), chunk.consts.len());
        assert_eq!(chunk2.consts[0], Value::int(42));
        assert_eq!(chunk2.consts[1], Value::string("hello"));
        assert_eq!(chunk2.n_locals, 2);
        assert_eq!(chunk2.max_stack, 4);
    }

    // ── Float edge cases ────────────────────────────────────────

    #[test]
    fn test_serialize_float_nan() {
        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        serialize_value(&Value::float(f64::NAN), &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let v = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert!(v.as_float().unwrap().is_nan());
    }

    #[test]
    fn test_serialize_float_neg_zero() {
        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        let neg_zero = Value::float(-0.0);
        serialize_value(&neg_zero, &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let v = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        let f = v.as_float().unwrap();
        assert!(f.is_sign_negative());
        assert_eq!(f.to_bits(), (-0.0f64).to_bits());
    }

    #[test]
    fn test_serialize_float_infinities() {
        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        serialize_value(&Value::float(f64::INFINITY), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::float(f64::NEG_INFINITY), &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let v1 = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(v1.as_float(), Some(f64::INFINITY));
        let v2 = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(v2.as_float(), Some(f64::NEG_INFINITY));
    }

    // ── Int edge cases ───────────────────────────────────────────

    #[test]
    fn test_serialize_int_extremes() {
        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        serialize_value(&Value::int(i64::MIN), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::int(i64::MAX), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::int(0), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::int(-1), &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        assert_eq!(
            deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(),
            Value::int(i64::MIN)
        );
        assert_eq!(
            deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(),
            Value::int(i64::MAX)
        );
        assert_eq!(
            deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(),
            Value::int(0)
        );
        assert_eq!(
            deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(),
            Value::int(-1)
        );
    }

    // ── Empty collections ────────────────────────────────────────

    #[test]
    fn test_serialize_empty_collections() {
        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();

        serialize_value(&Value::list(vec![]), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::vector(vec![]), &mut buf, &mut stb).unwrap();
        serialize_value(
            &Value::map(std::collections::BTreeMap::new()),
            &mut buf,
            &mut stb,
        )
        .unwrap();
        serialize_value(&Value::hashmap(vec![]), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::bytevector(vec![]), &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;

        let l = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(l.as_list().unwrap().len(), 0);
        let v = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(v.as_vector().unwrap().len(), 0);
        let m = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(m.as_map_rc().unwrap().len(), 0);
        let hm = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(hm.as_hashmap_rc().unwrap().len(), 0);
        let bv = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(bv.as_bytevector().unwrap().len(), 0);
    }

    // ── Nested collections ───────────────────────────────────────

    #[test]
    fn test_serialize_nested_collections() {
        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();

        // vector of lists
        let nested = Value::vector(vec![
            Value::list(vec![Value::int(1), Value::int(2)]),
            Value::list(vec![Value::string("a"), Value::symbol("b")]),
        ]);
        serialize_value(&nested, &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let v = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(v, nested);
    }

    // ── Char roundtrip ───────────────────────────────────────────

    #[test]
    fn test_serialize_char() {
        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        serialize_value(&Value::char('A'), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::char('🦀'), &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        assert_eq!(
            deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(),
            Value::char('A')
        );
        assert_eq!(
            deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(),
            Value::char('🦀')
        );
    }

    // ── Bytevector roundtrip ─────────────────────────────────────

    #[test]
    fn test_serialize_bytevector() {
        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        let data = vec![0u8, 1, 2, 255, 128, 64];
        serialize_value(&Value::bytevector(data.clone()), &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let v = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(v.as_bytevector().unwrap(), &data);
    }

    // ── Invalid data deserialization ─────────────────────────────

    #[test]
    fn test_deserialize_invalid_bool() {
        let buf = vec![VAL_BOOL, 0x02]; // invalid: not 0 or 1
        let table: Vec<String> = vec![];
        let remap: Vec<Spur> = vec![];
        let mut cursor = 0;
        let result = deserialize_value(&buf, &mut cursor, &table, &remap);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_invalid_char() {
        // 0xD800 is a surrogate — not a valid Unicode scalar value
        let mut buf = vec![VAL_CHAR];
        buf.extend_from_slice(&0xD800u32.to_le_bytes());
        let table: Vec<String> = vec![];
        let remap: Vec<Spur> = vec![];
        let mut cursor = 0;
        let result = deserialize_value(&buf, &mut cursor, &table, &remap);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_unknown_tag() {
        let buf = vec![0xFF];
        let table: Vec<String> = vec![];
        let remap: Vec<Spur> = vec![];
        let mut cursor = 0;
        let result = deserialize_value(&buf, &mut cursor, &table, &remap);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_truncated_data() {
        // Int tag but only 3 bytes of payload instead of 8
        let buf = vec![VAL_INT, 0x01, 0x02, 0x03];
        let table: Vec<String> = vec![];
        let remap: Vec<Spur> = vec![];
        let mut cursor = 0;
        let result = deserialize_value(&buf, &mut cursor, &table, &remap);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_string_index_out_of_range() {
        let mut buf = vec![VAL_STRING];
        buf.extend_from_slice(&99u32.to_le_bytes()); // index 99, but table is smaller
        let table = vec!["".to_string()];
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let result = deserialize_value(&buf, &mut cursor, &table, &remap);
        assert!(result.is_err());
    }

    // ── Runtime-only types rejected ──────────────────────────────

    #[test]
    fn test_serialize_runtime_only_type_rejected() {
        use sema_core::{Env, Lambda};
        let lambda = Value::lambda(Lambda {
            params: vec![],
            rest_param: None,
            body: vec![],
            env: Env::new(),
            name: None,
        });
        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        let result = serialize_value(&lambda, &mut buf, &mut stb);
        assert!(result.is_err());
    }

    // ── Chunk edge cases ─────────────────────────────────────────

    #[test]
    fn test_chunk_roundtrip_with_exceptions() {
        use crate::chunk::ExceptionEntry;
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        let mut e = Emitter::new();
        e.emit_op(Op::Nil);
        e.emit_op(Op::Return);
        let mut chunk = e.into_chunk();
        chunk.exception_table = vec![
            ExceptionEntry {
                try_start: 0,
                try_end: 10,
                handler_pc: 20,
                stack_depth: 3,
                catch_slot: 5,
            },
            ExceptionEntry {
                try_start: 100,
                try_end: 200,
                handler_pc: 300,
                stack_depth: 0,
                catch_slot: 7,
            },
        ];

        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        serialize_chunk(&chunk, &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let chunk2 = deserialize_chunk(&buf, &mut cursor, &table, &remap).unwrap();

        assert_eq!(chunk2.exception_table.len(), 2);
        assert_eq!(chunk2.exception_table[0].try_start, 0);
        assert_eq!(chunk2.exception_table[0].try_end, 10);
        assert_eq!(chunk2.exception_table[0].handler_pc, 20);
        assert_eq!(chunk2.exception_table[0].stack_depth, 3);
        assert_eq!(chunk2.exception_table[0].catch_slot, 5);
        assert_eq!(chunk2.exception_table[1].try_start, 100);
        assert_eq!(chunk2.exception_table[1].handler_pc, 300);
    }

    #[test]
    fn test_chunk_roundtrip_with_spans() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        let mut e = Emitter::new();
        e.emit_op(Op::Nil);
        e.emit_op(Op::Return);
        let mut chunk = e.into_chunk();
        chunk.spans = vec![(0, Span::point(1, 5)), (1, Span::new(2, 10, 3, 15))];

        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        serialize_chunk(&chunk, &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let chunk2 = deserialize_chunk(&buf, &mut cursor, &table, &remap).unwrap();

        assert_eq!(chunk2.spans.len(), 2);
        assert_eq!(chunk2.spans[0].0, 0);
        assert_eq!(chunk2.spans[0].1.line, 1);
        assert_eq!(chunk2.spans[0].1.col, 5);
        assert_eq!(chunk2.spans[0].1.end_line, 1);
        assert_eq!(chunk2.spans[0].1.end_col, 5);
        assert_eq!(chunk2.spans[1].0, 1);
        assert_eq!(chunk2.spans[1].1.line, 2);
        assert_eq!(chunk2.spans[1].1.col, 10);
        assert_eq!(chunk2.spans[1].1.end_line, 3);
        assert_eq!(chunk2.spans[1].1.end_col, 15);
    }

    #[test]
    fn test_chunk_deserialize_truncated() {
        // A chunk with code_len=100 but only a few bytes in the buffer
        let mut buf = Vec::new();
        buf.extend_from_slice(&100u32.to_le_bytes()); // claims 100 bytes of code
        buf.extend_from_slice(&[0u8; 4]); // only 4 bytes, not 100

        let table: Vec<String> = vec![];
        let remap: Vec<Spur> = vec![];
        let mut cursor = 0;
        let result = deserialize_chunk(&buf, &mut cursor, &table, &remap);
        assert!(result.is_err());
    }

    // ── Spur remapping ─────────────────────────────────────────

    #[test]
    fn test_spur_remapping_in_bytecode() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        let spur = intern("my-global");
        let mut e = Emitter::new();
        e.emit_op(Op::LoadGlobal);
        e.emit_u32(spur_to_u32(spur));
        e.emit_u16(0); // cache_slot
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();

        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        serialize_chunk(&chunk, &mut buf, &mut stb).unwrap();

        // Deserialize — the spur in the deserialized bytecode should resolve to "my-global"
        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let chunk2 = deserialize_chunk(&buf, &mut cursor, &table, &remap).unwrap();

        let spur2_bits = u32::from_le_bytes([
            chunk2.code[1],
            chunk2.code[2],
            chunk2.code[3],
            chunk2.code[4],
        ]);
        let spur2 = u32_to_spur(spur2_bits);
        assert_eq!(sema_core::resolve(spur2), "my-global");
    }

    #[test]
    fn test_spur_remapping_multiple_globals() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        let spur_a = intern("alpha");
        let spur_b = intern("beta");
        let mut e = Emitter::new();
        e.emit_op(Op::LoadGlobal);
        e.emit_u32(spur_to_u32(spur_a));
        e.emit_u16(0); // cache_slot
        e.emit_op(Op::DefineGlobal);
        e.emit_u32(spur_to_u32(spur_b));
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();

        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        serialize_chunk(&chunk, &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let chunk2 = deserialize_chunk(&buf, &mut cursor, &table, &remap).unwrap();

        // Check both globals resolved correctly
        let bits_a = u32::from_le_bytes([
            chunk2.code[1],
            chunk2.code[2],
            chunk2.code[3],
            chunk2.code[4],
        ]);
        assert_eq!(sema_core::resolve(u32_to_spur(bits_a)), "alpha");

        let bits_b = u32::from_le_bytes([
            chunk2.code[8],
            chunk2.code[9],
            chunk2.code[10],
            chunk2.code[11],
        ]);
        assert_eq!(sema_core::resolve(u32_to_spur(bits_b)), "beta");
    }

    // ── Function serialization ─────────────────────────────────

    #[test]
    fn test_function_roundtrip() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        let mut e = Emitter::new();
        e.emit_op(Op::LoadLocal0);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();

        let func = Function {
            name: Some(intern("my-func")),
            chunk,
            upvalue_descs: vec![UpvalueDesc::ParentLocal(0), UpvalueDesc::ParentUpvalue(1)],
            upvalue_names: vec![intern("outer-x"), intern("outer-y")],
            arity: 2,
            has_rest: true,
            local_names: vec![(0, intern("x")), (1, intern("y"))],
            source_file: None,
            local_scopes: vec![(2, 0, 7), (3, 4, 9)],
            cache_offset: 0,
        };

        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        serialize_function(&func, &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let func2 = deserialize_function(&buf, &mut cursor, &table, &remap).unwrap();

        assert_eq!(func2.arity, 2);
        assert!(func2.has_rest);
        assert_eq!(func2.upvalue_descs.len(), 2);
        assert_eq!(func2.upvalue_names.len(), 2);
        assert_eq!(func2.local_names.len(), 2);
        assert!(func2.name.is_some());
        assert_eq!(sema_core::resolve(func2.name.unwrap()), "my-func");
        assert_eq!(sema_core::resolve(func2.upvalue_names[0]), "outer-x");
        assert_eq!(sema_core::resolve(func2.upvalue_names[1]), "outer-y");
        assert_eq!(sema_core::resolve(func2.local_names[0].1), "x");
        assert_eq!(sema_core::resolve(func2.local_names[1].1), "y");
        // local_scopes (block-scope debug metadata) must round-trip (DAP-6).
        assert_eq!(func2.local_scopes, vec![(2, 0, 7), (3, 4, 9)]);
    }

    #[test]
    fn test_function_roundtrip_anonymous() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        let mut e = Emitter::new();
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();

        let func = Function {
            name: None,
            chunk,
            upvalue_descs: vec![],
            upvalue_names: vec![],
            arity: 0,
            has_rest: false,
            local_names: vec![],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        };

        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();
        serialize_function(&func, &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let func2 = deserialize_function(&buf, &mut cursor, &table, &remap).unwrap();

        assert!(func2.name.is_none());
        assert_eq!(func2.arity, 0);
        assert!(!func2.has_rest);
        assert_eq!(func2.upvalue_descs.len(), 0);
    }

    // ── Full file serialization ─────────────────────────────────

    #[test]
    fn test_full_file_roundtrip() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        let mut e = Emitter::new();
        e.emit_const(Value::int(42)).unwrap();
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let result = CompileResult::new(chunk, vec![]);

        let bytes = serialize_to_bytes(&result, 0).unwrap();
        assert_eq!(&bytes[0..4], b"\x00SEM");

        let result2 = deserialize_from_bytes(&bytes).unwrap();
        assert_eq!(result2.chunk.consts.len(), 1);
        assert_eq!(result2.functions.len(), 0);
    }

    #[test]
    fn test_full_file_with_functions() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        // Main chunk
        let mut e = Emitter::new();
        e.emit_op(Op::MakeClosure);
        e.emit_u16(0); // func_id
        e.emit_u16(0); // n_upvalues
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();

        // Function
        let mut fe = Emitter::new();
        fe.emit_op(Op::LoadLocal0);
        fe.emit_op(Op::Return);
        let func = Function {
            name: Some(intern("add-one")),
            chunk: fe.into_chunk(),
            upvalue_descs: vec![],
            upvalue_names: vec![],
            arity: 1,
            has_rest: false,
            local_names: vec![(0, intern("x"))],
            source_file: None,
            local_scopes: Vec::new(),
            cache_offset: 0,
        };

        let result = CompileResult::new(chunk, vec![func]);

        let bytes = serialize_to_bytes(&result, 0xDEAD_BEEF).unwrap();
        let result2 = deserialize_from_bytes(&bytes).unwrap();

        assert_eq!(result2.functions.len(), 1);
        assert_eq!(result2.functions[0].arity, 1);
        assert_eq!(
            sema_core::resolve(result2.functions[0].name.unwrap()),
            "add-one"
        );
    }

    #[test]
    fn test_magic_detection() {
        assert!(is_bytecode_file(b"\x00SEM\x01\x00"));
        assert!(!is_bytecode_file(b"(define x 1)"));
        assert!(!is_bytecode_file(b""));
        assert!(!is_bytecode_file(b"\x00SE")); // too short
    }

    #[test]
    fn test_deserialize_bad_magic() {
        let mut bytes = vec![0u8; 24];
        bytes[0..4].copy_from_slice(b"NOPE");
        let result = deserialize_from_bytes(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_bad_version() {
        let mut bytes = vec![0u8; 24];
        bytes[0..4].copy_from_slice(&[0x00, b'S', b'E', b'M']);
        bytes[4..6].copy_from_slice(&99u16.to_le_bytes()); // unsupported version
        let result = deserialize_from_bytes(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_rejects_nonzero_reserved() {
        let mut bytes = vec![0u8; 24];
        bytes[0..4].copy_from_slice(&MAGIC);
        bytes[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        // Set reserved field (offset 20-23) to non-zero
        bytes[20] = 0xFF;
        let result = deserialize_from_bytes(&bytes);
        assert!(result.is_err(), "should reject non-zero reserved field");
        let err = result.err().unwrap();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn test_deserialize_too_short() {
        let result = deserialize_from_bytes(&[0x00, b'S', b'E']);
        assert!(result.is_err());
    }

    #[test]
    fn test_full_file_roundtrip_with_globals() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        // Build a chunk with global opcodes and symbol/keyword constants
        let spur_x = intern("my-var");
        let spur_print = intern("println");
        let mut e = Emitter::new();
        // (define my-var 42)
        e.emit_const(Value::int(42)).unwrap();
        e.emit_op(Op::DefineGlobal);
        e.emit_u32(spur_to_u32(spur_x));
        // (println my-var) — load both globals
        e.emit_op(Op::LoadGlobal);
        e.emit_u32(spur_to_u32(spur_print));
        e.emit_u16(0); // cache_slot
        e.emit_op(Op::LoadGlobal);
        e.emit_u32(spur_to_u32(spur_x));
        e.emit_u16(1); // cache_slot
                       // symbol and keyword in constant pool
        e.emit_const(Value::symbol("test-sym")).unwrap();
        e.emit_const(Value::keyword("test-kw")).unwrap();
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();

        let result = CompileResult::new(chunk, vec![]);

        let bytes = serialize_to_bytes(&result, 0).unwrap();
        let result2 = deserialize_from_bytes(&bytes).unwrap();

        // Verify globals resolve correctly in the deserialized bytecode
        // DefineGlobal "my-var" is at code offset 3 (after CONST(3 bytes))
        let code = &result2.chunk.code;
        // Find DefineGlobal
        let mut found_define = false;
        let mut found_load_print = false;
        let mut pc = 0;
        while pc < code.len() {
            let (op, next) = advance_pc(code, pc).unwrap();
            match op {
                Op::DefineGlobal => {
                    let bits = u32::from_le_bytes([
                        code[pc + 1],
                        code[pc + 2],
                        code[pc + 3],
                        code[pc + 4],
                    ]);
                    assert_eq!(sema_core::resolve(u32_to_spur(bits)), "my-var");
                    found_define = true;
                }
                Op::LoadGlobal => {
                    let bits = u32::from_le_bytes([
                        code[pc + 1],
                        code[pc + 2],
                        code[pc + 3],
                        code[pc + 4],
                    ]);
                    let name = sema_core::resolve(u32_to_spur(bits));
                    if name == "println" {
                        found_load_print = true;
                    }
                }
                _ => {}
            }
            pc = next;
        }
        assert!(found_define, "DefineGlobal 'my-var' not found");
        assert!(found_load_print, "LoadGlobal 'println' not found");

        // Verify symbol/keyword constants survived
        assert_eq!(result2.chunk.consts.len(), 3); // 42, test-sym, test-kw
        assert!(result2.chunk.consts[1].as_symbol().is_some());
        assert!(result2.chunk.consts[2].as_keyword().is_some());
    }

    #[test]
    fn test_truncated_global_operand_errors_not_panics() {
        // A LoadGlobal at the end with missing operand bytes
        let code = vec![Op::LoadGlobal as u8, 0x01, 0x00]; // only 2 of 4 operand bytes
        let mut stb = StringTableBuilder::new();
        let result = remap_spurs_to_indices(&code, &mut stb);
        assert!(result.is_err());

        // Also test remap_indices_to_spurs
        let mut code2 = vec![Op::LoadGlobal as u8, 0x01]; // only 1 operand byte
        let remap = vec![intern("x")];
        let result2 = remap_indices_to_spurs(&mut code2, &remap);
        assert!(result2.is_err());
    }

    #[test]
    fn test_truncated_make_closure_errors_not_panics() {
        // MakeClosure with truncated operands
        let code = vec![Op::MakeClosure as u8, 0x00]; // only 1 of 4 operand bytes
        let mut stb = StringTableBuilder::new();
        let result = remap_spurs_to_indices(&code, &mut stb);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_required_section_errors() {
        // Valid header but n_sections=0 → missing all required sections
        let mut bytes = vec![0u8; 24];
        bytes[0..4].copy_from_slice(&[0x00, b'S', b'E', b'M']);
        bytes[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[14..16].copy_from_slice(&0u16.to_le_bytes()); // 0 sections
        let result = deserialize_from_bytes(&bytes);
        match &result {
            Err(e) => assert!(e.to_string().contains("missing"), "unexpected error: {e}"),
            Ok(_) => panic!("expected error for missing sections"),
        }
    }

    // ── Unicode string table ─────────────────────────────────────

    #[test]
    fn test_string_table_unicode() {
        let mut builder = StringTableBuilder::new();
        let idx1 = builder.intern_str("こんにちは");
        let idx2 = builder.intern_str("🦀");
        let idx3 = builder.intern_str("café");

        let table = builder.finish();
        assert_eq!(table[idx1 as usize], "こんにちは");
        assert_eq!(table[idx2 as usize], "🦀");
        assert_eq!(table[idx3 as usize], "café");
    }

    #[test]
    fn test_serialize_value_roundtrip_primitives() {
        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();

        serialize_value(&Value::nil(), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::bool(true), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::bool(false), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::int(42), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::float(1.25), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::string("hello"), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::symbol("foo"), &mut buf, &mut stb).unwrap();
        serialize_value(&Value::keyword("bar"), &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        assert_eq!(
            deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(),
            Value::nil()
        );
        assert_eq!(
            deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(),
            Value::bool(true)
        );
        assert_eq!(
            deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(),
            Value::bool(false)
        );
        assert_eq!(
            deserialize_value(&buf, &mut cursor, &table, &remap).unwrap(),
            Value::int(42)
        );
        let f = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(f.as_float(), Some(1.25));
        let s = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(s.as_str().unwrap(), "hello");
        let sym = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert!(sym.as_symbol().is_some());
        let kw = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert!(kw.as_keyword().is_some());
    }

    #[test]
    fn test_serialize_value_roundtrip_collections() {
        let mut buf = Vec::new();
        let mut stb = StringTableBuilder::new();

        let list = Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]);
        serialize_value(&list, &mut buf, &mut stb).unwrap();

        let vec = Value::vector(vec![Value::string("a"), Value::string("b")]);
        serialize_value(&vec, &mut buf, &mut stb).unwrap();

        let table = stb.finish();
        let remap = build_remap_table(&table);
        let mut cursor = 0;

        let list2 = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(list2, list);

        let vec2 = deserialize_value(&buf, &mut cursor, &table, &remap).unwrap();
        assert_eq!(vec2, vec);
    }

    #[test]
    fn test_spur_u32_conversion_safe() {
        let spur = intern("test-var");
        let bits = spur_to_u32(spur);
        assert_ne!(bits, 0, "Spur should never be zero (it's NonZeroU32)");
        let spur2 = u32_to_spur(bits);
        assert_eq!(spur, spur2);
        assert_eq!(sema_core::resolve(spur2), "test-var");
    }

    #[test]
    fn test_string_table_section_boundary() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        let mut e = Emitter::new();
        e.emit_const(Value::int(1)).unwrap();
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let result = CompileResult::new(chunk, vec![]);
        let bytes = serialize_to_bytes(&result, 0).unwrap();

        // Roundtrip should work on valid data
        let result2 = deserialize_from_bytes(&bytes);
        assert!(result2.is_ok());
    }

    #[test]
    fn test_deserialize_value_depth_limit() {
        // Construct a deeply nested list: (list (list (list ... ))) 200 levels deep
        let depth = 200;
        let mut buf = Vec::new();
        for _ in 0..depth {
            buf.push(0x08); // VAL_LIST
            buf.extend_from_slice(&1u16.to_le_bytes()); // 1 element
        }
        buf.push(0x00); // VAL_NIL at the bottom

        let table = vec!["".to_string()];
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let result = deserialize_value(&buf, &mut cursor, &table, &remap);
        assert!(result.is_err(), "should reject deeply nested values");
        assert!(
            result.unwrap_err().to_string().contains("depth"),
            "error should mention depth limit"
        );
    }

    #[test]
    fn test_u32_to_spur_rejects_zero() {
        let result = std::panic::catch_unwind(|| u32_to_spur(0));
        assert!(
            result.is_err(),
            "u32_to_spur(0) should panic (was UB before fix)"
        );
    }

    // ── DoS limits on allocation sizes ──────────────────────────

    #[test]
    fn test_deserialize_rejects_huge_code_len() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // code_len
        let table = vec!["".to_string()];
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let result = deserialize_chunk(&buf, &mut cursor, &table, &remap);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_rejects_huge_string_count() {
        let mut section = Vec::new();
        section.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // count

        let mut bytes = vec![0u8; 24];
        bytes[0..4].copy_from_slice(&[0x00, b'S', b'E', b'M']);
        bytes[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[14..16].copy_from_slice(&1u16.to_le_bytes()); // 1 section
                                                            // Section header
        bytes.extend_from_slice(&0x01u16.to_le_bytes()); // string table
        bytes.extend_from_slice(&(section.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&section);

        let result = deserialize_from_bytes(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_rejects_huge_bytevector() {
        let mut buf = Vec::new();
        buf.push(0x0C); // VAL_BYTEVECTOR
        buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes()); // length
        let table = vec!["".to_string()];
        let remap = build_remap_table(&table);
        let mut cursor = 0;
        let result = deserialize_value(&buf, &mut cursor, &table, &remap);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_rejects_nonempty_string_zero() {
        let mut bad_bytes = Vec::new();
        // Header
        bad_bytes.extend_from_slice(&[0x00, b'S', b'E', b'M']); // magic
        bad_bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bad_bytes.extend_from_slice(&0u16.to_le_bytes()); // flags
        bad_bytes.extend_from_slice(&0u16.to_le_bytes()); // sema_major
        bad_bytes.extend_from_slice(&0u16.to_le_bytes()); // sema_minor
        bad_bytes.extend_from_slice(&0u16.to_le_bytes()); // sema_patch
        bad_bytes.extend_from_slice(&3u16.to_le_bytes()); // n_sections = 3
        bad_bytes.extend_from_slice(&0u32.to_le_bytes()); // source_hash
        bad_bytes.extend_from_slice(&0u32.to_le_bytes()); // reserved
        assert_eq!(bad_bytes.len(), 24);

        // String table section with index 0 = "bad" instead of ""
        let mut strtab = Vec::new();
        strtab.extend_from_slice(&1u32.to_le_bytes()); // 1 string
        strtab.extend_from_slice(&3u32.to_le_bytes()); // length 3
        strtab.extend_from_slice(b"bad"); // not empty!
        bad_bytes.extend_from_slice(&0x01u16.to_le_bytes()); // section type
        bad_bytes.extend_from_slice(&(strtab.len() as u32).to_le_bytes());
        bad_bytes.extend_from_slice(&strtab);

        // Empty function table section
        let mut functab = Vec::new();
        functab.extend_from_slice(&0u32.to_le_bytes()); // 0 functions
        bad_bytes.extend_from_slice(&0x02u16.to_le_bytes());
        bad_bytes.extend_from_slice(&(functab.len() as u32).to_le_bytes());
        bad_bytes.extend_from_slice(&functab);

        // Minimal main chunk section
        let mut chunk_data = Vec::new();
        chunk_data.extend_from_slice(&1u32.to_le_bytes()); // code_len = 1
        chunk_data.push(Op::Return as u8);
        chunk_data.extend_from_slice(&0u16.to_le_bytes()); // n_consts = 0
        chunk_data.extend_from_slice(&0u32.to_le_bytes()); // n_spans = 0
        chunk_data.extend_from_slice(&0u16.to_le_bytes()); // max_stack
        chunk_data.extend_from_slice(&0u16.to_le_bytes()); // n_locals
        chunk_data.extend_from_slice(&0u16.to_le_bytes()); // n_global_cache_slots
        chunk_data.extend_from_slice(&0u16.to_le_bytes()); // n_exceptions
        bad_bytes.extend_from_slice(&0x03u16.to_le_bytes());
        bad_bytes.extend_from_slice(&(chunk_data.len() as u32).to_le_bytes());
        bad_bytes.extend_from_slice(&chunk_data);

        let result = deserialize_from_bytes(&bad_bytes);
        assert!(
            result.is_err(),
            "should reject string table with non-empty index 0"
        );
        let err = result.err().unwrap();
        assert!(err.to_string().contains("index 0 must be the empty string"));
    }

    #[test]
    fn test_deserialize_rejects_trailing_section_bytes() {
        use crate::emit::Emitter;
        use crate::opcodes::Op;

        let mut stb = StringTableBuilder::new();
        let mut func_payload = Vec::new();
        func_payload.extend_from_slice(&0u32.to_le_bytes()); // 0 functions
        func_payload.extend_from_slice(&[0xDE, 0xAD]); // trailing garbage

        let mut chunk_payload = Vec::new();
        let mut e = Emitter::new();
        e.emit_op(Op::Nil);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        serialize_chunk(&chunk, &mut chunk_payload, &mut stb).unwrap();

        let string_table = stb.finish();
        let mut strtab_payload = Vec::new();
        strtab_payload.extend_from_slice(&(string_table.len() as u32).to_le_bytes());
        for s in &string_table {
            let sb = s.as_bytes();
            strtab_payload.extend_from_slice(&(sb.len() as u32).to_le_bytes());
            strtab_payload.extend_from_slice(sb);
        }

        let mut out = Vec::new();
        out.extend_from_slice(&[0x00, b'S', b'E', b'M']);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&3u16.to_le_bytes()); // 3 sections
        out.extend_from_slice(&0u32.to_le_bytes()); // source_hash
        out.extend_from_slice(&0u32.to_le_bytes()); // reserved

        // String table section
        out.extend_from_slice(&0x01u16.to_le_bytes());
        out.extend_from_slice(&(strtab_payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&strtab_payload);
        // Function table section (with trailing bytes)
        out.extend_from_slice(&0x02u16.to_le_bytes());
        out.extend_from_slice(&(func_payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&func_payload);
        // Main chunk section
        out.extend_from_slice(&0x03u16.to_le_bytes());
        out.extend_from_slice(&(chunk_payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&chunk_payload);

        match deserialize_from_bytes(&out) {
            Ok(_) => panic!("should reject trailing bytes in function table section"),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("trailing") || msg.contains("unconsumed"),
                    "error should mention trailing/unconsumed bytes, got: {msg}"
                );
            }
        }
    }

    // ── Post-deserialization bytecode validation ─────────────────

    #[test]
    fn test_validate_rejects_bad_const_index() {
        let chunk = Chunk {
            code: vec![Op::Const as u8, 0x03, 0x00, Op::Return as u8],
            consts: vec![Value::int(1)],
            spans: vec![],
            max_stack: 1,
            n_locals: 0,
            exception_table: vec![],
            n_global_cache_slots: 0,
        };

        let result = CompileResult::new(chunk, vec![]);
        let bytes = serialize_to_bytes(&result, 0).unwrap();
        let deser = deserialize_from_bytes(&bytes);
        assert!(deser.is_err(), "should reject out-of-bounds const index");
    }

    #[test]
    fn test_validate_rejects_bad_func_id() {
        use crate::emit::Emitter;

        let mut e = Emitter::new();
        e.emit_op(Op::MakeClosure);
        e.emit_u16(5); // func_id 5, but we'll have 0 functions
        e.emit_u16(0); // 0 upvalues
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();

        let result = CompileResult::new(chunk, vec![]);
        let bytes = serialize_to_bytes(&result, 0).unwrap();
        let deser = deserialize_from_bytes(&bytes);
        assert!(
            deser.is_err(),
            "should reject out-of-bounds func_id in MakeClosure"
        );
    }

    // ── Stack-depth verifier (VM-1 / ADR #56) ───────────────────

    #[test]
    fn test_stack_effect_variable_arity() {
        // Call argc=3 pops callee+3 args = 4, pushes 1.
        assert_eq!(
            Op::Call.stack_effect(3),
            crate::opcodes::StackEffect {
                pops: 4,
                pushes: 1,
                exits_frame: false
            }
        );
        // MakeMap with 2 pairs pops 4, pushes 1.
        assert_eq!(
            Op::MakeMap.stack_effect(2),
            crate::opcodes::StackEffect {
                pops: 4,
                pushes: 1,
                exits_frame: false
            }
        );
        // TailCall exits the frame.
        assert!(Op::TailCall.stack_effect(0).exits_frame);
        // CallGlobal/CallNative pop only the args (callee resolved by id/spur).
        assert_eq!(Op::CallGlobal.stack_effect(2).pops, 2);
        assert_eq!(Op::CallNative.stack_effect(2).pops, 2);
    }

    #[test]
    fn test_verifier_rejects_leading_pop() {
        use crate::emit::Emitter;
        let mut e = Emitter::new();
        e.emit_op(Op::Pop);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let bytes = serialize_to_bytes(&CompileResult::new(chunk, vec![]), 0).unwrap();
        let err = deserialize_from_bytes(&bytes).err().unwrap();
        assert!(
            err.to_string().contains("underflow"),
            "expected underflow rejection, got: {err}"
        );
    }

    #[test]
    fn test_verifier_accepts_balanced_branch() {
        // A balanced if/else: both arms leave exactly one value before Return.
        use crate::emit::Emitter;
        let mut e = Emitter::new();
        e.emit_op(Op::True); // cond
        let jf = e.emit_jump(Op::JumpIfFalse); // pop cond
        e.emit_const(Value::int(1)).unwrap(); // then-branch value
        let j = e.emit_jump(Op::Jump);
        e.patch_jump(jf);
        e.emit_const(Value::int(2)).unwrap(); // else-branch value
        e.patch_jump(j);
        e.emit_op(Op::Return);
        let chunk = e.into_chunk();
        let bytes = serialize_to_bytes(&CompileResult::new(chunk, vec![]), 0).unwrap();
        assert!(
            deserialize_from_bytes(&bytes).is_ok(),
            "balanced branch should pass the verifier"
        );
    }

    #[test]
    fn test_verifier_rejects_dup_overflow() {
        // An unconditional self-loop of Dup grows the abstract depth without
        // bound; the verifier must reject it (via a stack-depth disagreement at
        // the loop head when the second visit arrives at a higher depth) rather
        // than spin forever.
        //   pc 0: Const 1     (3 bytes) depth 0 -> 1
        //   pc 3: Dup         (1 byte)  depth -> 2 on first pass
        //   pc 4: Jump -6     (5 bytes) target = next(9) + (-6) = 3 (the Dup)
        use crate::emit::Emitter;
        let mut e = Emitter::new();
        e.emit_const(Value::int(1)).unwrap();
        // pc 3: Dup
        e.emit_op(Op::Dup);
        // pc 4: Jump back to pc 3
        e.emit_op(Op::Jump);
        e.emit_i32(-6); // next pc is 9, 9 + (-6) = 3
        let chunk = e.into_chunk();
        let bytes = serialize_to_bytes(&CompileResult::new(chunk, vec![]), 0).unwrap();
        let err = deserialize_from_bytes(&bytes).err().unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("maximum") || msg.contains("disagreement"),
            "expected overflow/disagreement rejection, got: {msg}"
        );
    }
}
