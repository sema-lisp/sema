use serde_json::{json, Value as JsonValue};
use std::path::Path;
use std::sync::OnceLock;

use sema_core::{SemaError, Value, ValueViewRef};
use sema_eval::{call_value, Interpreter};

use crate::notebook::{
    add_cell, delete_cell, export_notebook, get_or_create_engine, update_cell, NotebookCache,
};
use crate::protocol::{CallToolResult, Tool, ToolContent};

static BUILTIN_DOCS: OnceLock<std::collections::HashMap<String, String>> = OnceLock::new();

pub fn get_builtin_doc(symbol: &str) -> Option<&'static String> {
    let map = BUILTIN_DOCS.get_or_init(crate::builtin_docs::build_builtin_docs);
    map.get(symbol)
}

/// Helper to parse a Sema parameters map into JSON-Schema.
pub fn sema_value_to_json_schema(val: &Value) -> serde_json::Value {
    if let Some(map) = val.as_map_rc() {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for (k, v) in map.iter() {
            let key = k
                .as_keyword()
                .or_else(|| k.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| k.to_string());

            // Skip metadata parameters
            if key == "mcp/expose" || key == "private" || key.starts_with("mcp/") {
                continue;
            }

            let prop = if let Some(inner) = v.as_map_rc() {
                let mut prop_obj = serde_json::Map::new();
                if let Some(t) = inner.get(&Value::keyword("type")) {
                    let type_str = t
                        .as_keyword()
                        .or_else(|| t.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| "string".to_string());
                    prop_obj.insert("type".to_string(), serde_json::Value::String(type_str));
                } else {
                    prop_obj.insert(
                        "type".to_string(),
                        serde_json::Value::String("string".to_string()),
                    );
                }
                if let Some(d) = inner.get(&Value::keyword("description")) {
                    let desc = d
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| d.to_string());
                    prop_obj.insert("description".to_string(), serde_json::Value::String(desc));
                }
                if let Some(e) = inner.get(&Value::keyword("enum")) {
                    if let Some(items) = e.as_seq() {
                        let vals: Vec<serde_json::Value> = items
                            .iter()
                            .map(|v| {
                                serde_json::Value::String(
                                    v.as_str()
                                        .map(|s| s.to_string())
                                        .or_else(|| v.as_keyword())
                                        .unwrap_or_else(|| v.to_string()),
                                )
                            })
                            .collect();
                        prop_obj.insert("enum".to_string(), serde_json::Value::Array(vals));
                    }
                }
                // Mark as required unless :optional #t
                let optional = inner
                    .get(&Value::keyword("optional"))
                    .map(|v| v.is_truthy())
                    .unwrap_or(false);
                if !optional {
                    required.push(serde_json::Value::String(key.clone()));
                }
                serde_json::Value::Object(prop_obj)
            } else {
                required.push(serde_json::Value::String(key.clone()));
                serde_json::json!({"type": "string"})
            };
            properties.insert(key, prop);
        }
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required
        })
    } else {
        serde_json::json!({"type": "object", "properties": {}})
    }
}

/// One declared parameter of a `deftool` handler, in positional order.
struct ParamSpec {
    /// The parameter's name (as it appears in the JSON arguments object).
    name: String,
    /// Declared type from the schema (`:type`), lower-cased; `None` if the param
    /// was declared as a bare value (no map) or has no `:type` key.
    declared_type: Option<String>,
    /// Whether the param is required (not `:optional #t`).
    required: bool,
}

/// Whether the declared parameter list ends in a rest/variadic parameter that
/// should collect positional overflow.
struct HandlerShape {
    params: Vec<ParamSpec>,
    /// Name of the rest parameter if the handler is variadic, else `None`.
    rest: Option<String>,
}

/// Look up a parameter's schema (declared type + required-ness) from the
/// `deftool` `:param` map by name. Returns `(declared_type, required)`.
///
/// A param may be declared as a map (`{:type :int :optional #t ...}`) or as a
/// bare value (treated as a required string, mirroring `sema_value_to_json_schema`).
fn lookup_param_schema(params: &Value, name: &str) -> (Option<String>, bool) {
    let Some(map) = params.as_map_rc() else {
        // No schema map at all: treat as an untyped, required positional arg.
        return (None, true);
    };
    for (k, v) in map.iter() {
        let key = k
            .as_keyword()
            .or_else(|| k.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| k.to_string());
        if key != name {
            continue;
        }
        if let Some(inner) = v.as_map_rc() {
            let declared_type = inner
                .get(&Value::keyword("type"))
                .and_then(|t| t.as_keyword().or_else(|| t.as_str().map(|s| s.to_string())))
                .map(|s| s.to_lowercase());
            let optional = inner
                .get(&Value::keyword("optional"))
                .map(|o| o.is_truthy())
                .unwrap_or(false);
            return (declared_type, !optional);
        }
        // Bare-value param: required string (matches schema generation).
        return (None, true);
    }
    // Param not described in the schema: untyped and (conservatively) required.
    (None, true)
}

/// Derive the positional parameter shape (names, types, required-ness, rest) of
/// a `deftool` handler from the closure itself, looking up per-param type and
/// required-ness from the declared schema. Returns `None` if the handler is not
/// a recognized callable (lambda or VM closure), in which case the caller falls
/// back to schema-order mapping.
fn handler_shape(params: &Value, handler: &Value) -> Option<HandlerShape> {
    if let Some(lambda) = handler.as_lambda_rc() {
        let specs = lambda
            .params
            .iter()
            .map(|spur| {
                let name = sema_core::resolve(*spur);
                let (declared_type, required) = lookup_param_schema(params, &name);
                ParamSpec {
                    name,
                    declared_type,
                    required,
                }
            })
            .collect();
        let rest = lambda.rest_param.map(sema_core::resolve);
        return Some(HandlerShape {
            params: specs,
            rest,
        });
    }

    if let Some((closure, _)) = sema_vm::extract_vm_closure(handler) {
        let arity = closure.func.arity as usize;
        // Reconstruct positional names from (slot, name) pairs.
        let mut names: Vec<Option<String>> = vec![None; arity];
        let mut rest_name: Option<String> = None;
        for &(slot, name_spur) in &closure.func.local_names {
            let s = slot as usize;
            if s < arity {
                names[s] = Some(sema_core::resolve(name_spur));
            } else if closure.func.has_rest && s == arity {
                rest_name = Some(sema_core::resolve(name_spur));
            }
        }
        let specs = names
            .into_iter()
            .map(|maybe_name| {
                let name = maybe_name.unwrap_or_default();
                let (declared_type, required) = if name.is_empty() {
                    (None, false)
                } else {
                    lookup_param_schema(params, &name)
                };
                ParamSpec {
                    name,
                    declared_type,
                    required,
                }
            })
            .collect();
        let rest = if closure.func.has_rest {
            // Fall back to a synthetic name so overflow can still be collected
            // even if the rest local's name wasn't recorded.
            Some(rest_name.unwrap_or_else(|| "rest".to_string()))
        } else {
            None
        };
        return Some(HandlerShape {
            params: specs,
            rest,
        });
    }

    None
}

/// Coerce a single JSON value to a Sema value for a declared parameter type.
///
/// Coercion rules (deliberately strict to surface client mistakes):
/// - `:string` ← JSON string only.
/// - `:int`/`:integer` ← JSON integer, or a float with no fractional part. A
///   JSON string is rejected (no implicit parse).
/// - `:number` ← JSON number, preserving kind (integer→int, fractional→float).
/// - `:float`/`:double` ← JSON number, always a float.
/// - `:bool`/`:boolean` ← JSON bool only.
/// - `:keyword` ← JSON string (interned as a keyword).
/// - `:list`/`:vector` ← JSON array.
/// - `:map`/`:object` ← JSON object.
/// - any other / no type ← structural conversion via `json_to_value` (no checks).
///
/// On a mismatch that cannot be coerced, returns `Err(expected_description)`.
fn coerce_typed(value: &serde_json::Value, declared_type: Option<&str>) -> Result<Value, String> {
    let Some(ty) = declared_type else {
        return Ok(sema_core::json_to_value(value));
    };
    match ty {
        "string" | "str" => value
            .as_str()
            .map(Value::string)
            .ok_or_else(|| "string".to_string()),
        "int" | "integer" => {
            if let Some(i) = value.as_i64() {
                Ok(Value::int(i))
            } else if let Some(f) = value.as_f64() {
                if f.fract() == 0.0 && f.is_finite() && f >= i64::MIN as f64 && f <= i64::MAX as f64
                {
                    Ok(Value::int(f as i64))
                } else {
                    Err("integer".to_string())
                }
            } else {
                Err("integer".to_string())
            }
        }
        "number" => {
            // Preserve the JSON numeric kind: an integer argument stays a Sema
            // int (so `15` does not silently become `15.0`), a fractional value
            // becomes a float. `:float`/`:double` below force a float.
            if let Some(i) = value.as_i64() {
                Ok(Value::int(i))
            } else if let Some(f) = value.as_f64() {
                Ok(Value::float(f))
            } else {
                Err("number".to_string())
            }
        }
        "float" | "double" => value
            .as_f64()
            .map(Value::float)
            .ok_or_else(|| "number".to_string()),
        "bool" | "boolean" => value
            .as_bool()
            .map(Value::bool)
            .ok_or_else(|| "boolean".to_string()),
        "keyword" => value
            .as_str()
            .map(Value::keyword)
            .ok_or_else(|| "string (for keyword)".to_string()),
        "list" | "vector" | "array" => {
            if let Some(arr) = value.as_array() {
                Ok(Value::list(
                    arr.iter().map(sema_core::json_to_value).collect(),
                ))
            } else {
                Err("array".to_string())
            }
        }
        "map" | "object" | "dict" => {
            if value.is_object() {
                Ok(sema_core::json_to_value(value))
            } else {
                Err("object".to_string())
            }
        }
        // Unknown declared type: accept structurally rather than rejecting valid
        // input over an unrecognized type annotation.
        _ => Ok(sema_core::json_to_value(value)),
    }
}

/// Convert a JSON arguments object into the positional Sema argument list for a
/// `deftool` handler, validating against the tool's declared schema.
///
/// Behaviour:
/// 1. A missing **required** param yields a clear error naming the field.
/// 2. Each declared type is coerced/validated; an uncoercible value yields a
///    clear error naming the field and the expected type.
/// 3. Named JSON args are placed into the handler's positional slots in declared
///    order via the closure's parameter names.
/// 4. A rest/variadic param collects any extra arguments: a JSON array under the
///    rest param's name is spread flat; surplus is appended so the evaluator's
///    own rest-collection produces a proper Sema list.
/// 5. An absent param is distinguished from an explicit `null` in error text.
pub fn json_args_to_sema(
    params: &Value,
    arguments: &serde_json::Value,
    handler: &Value,
) -> Result<Vec<Value>, String> {
    // Non-object arguments can't be mapped to named params. The only sensible
    // interpretation is a single positional argument for a 1-arg handler.
    let serde_json::Value::Object(json_obj) = arguments else {
        return Ok(vec![sema_core::json_to_value(arguments)]);
    };

    let Some(shape) = handler_shape(params, handler) else {
        // Unknown handler kind: fall back to schema-order mapping with the same
        // validation as the positional path.
        return schema_order_args(params, json_obj);
    };

    let mut out: Vec<Value> = Vec::with_capacity(shape.params.len() + 1);

    for spec in &shape.params {
        if spec.name.is_empty() {
            // Unnamed positional slot (e.g. compiler-introduced): pass nil.
            out.push(Value::nil());
            continue;
        }
        match json_obj.get(&spec.name) {
            None => {
                if spec.required {
                    return Err(format!("missing required parameter '{}'", spec.name));
                }
                out.push(Value::nil());
            }
            Some(serde_json::Value::Null) => {
                // Explicit null: allowed for optional params, rejected for
                // required ones (distinct message from an absent param).
                if spec.required {
                    return Err(format!(
                        "parameter '{}' is required but was explicitly null",
                        spec.name
                    ));
                }
                out.push(Value::nil());
            }
            Some(v) => match coerce_typed(v, spec.declared_type.as_deref()) {
                Ok(coerced) => out.push(coerced),
                Err(expected) => {
                    return Err(format!(
                        "parameter '{}' expected {}, got JSON {}",
                        spec.name,
                        expected,
                        json_type_name(v)
                    ));
                }
            },
        }
    }

    // Variadic handler: collect overflow into the rest parameter. The evaluator
    // (both tree-walker and VM) collects trailing args beyond the fixed arity
    // into a list, so we append the rest values flat here.
    if let Some(rest_name) = &shape.rest {
        if let Some(rest_val) = json_obj.get(rest_name) {
            match rest_val {
                serde_json::Value::Array(items) => {
                    for item in items {
                        out.push(sema_core::json_to_value(item));
                    }
                }
                serde_json::Value::Null => {}
                other => out.push(sema_core::json_to_value(other)),
            }
        }
    }

    Ok(out)
}

/// Fallback mapping when the handler isn't a recognized closure: drive the
/// positional order from the schema map's keys and apply the same
/// required/type validation.
fn schema_order_args(
    params: &Value,
    json_obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<Value>, String> {
    let Some(param_map) = params.as_map_rc() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for k in param_map.keys() {
        let name = k
            .as_keyword()
            .or_else(|| k.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| k.to_string());
        if name == "mcp/expose" || name == "private" || name.starts_with("mcp/") {
            continue;
        }
        let (declared_type, required) = lookup_param_schema(params, &name);
        match json_obj.get(&name) {
            None => {
                if required {
                    return Err(format!("missing required parameter '{name}'"));
                }
                out.push(Value::nil());
            }
            Some(serde_json::Value::Null) => {
                if required {
                    return Err(format!(
                        "parameter '{name}' is required but was explicitly null"
                    ));
                }
                out.push(Value::nil());
            }
            Some(v) => match coerce_typed(v, declared_type.as_deref()) {
                Ok(coerced) => out.push(coerced),
                Err(expected) => {
                    return Err(format!(
                        "parameter '{name}' expected {expected}, got JSON {}",
                        json_type_name(v)
                    ));
                }
            },
        }
    }
    Ok(out)
}

/// Human-readable JSON type name for error messages.
fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Evaluate a Lisp operation, capturing program stdout/stderr.
///
/// Capture goes through sema-core's thread-local output hooks rather than
/// redirecting the process stdout file descriptor. The MCP transport writes
/// JSON-RPC frames to the real stdout, so fd-level redirection risks
/// interleaving user `print` output into the protocol stream (and silently
/// corrupting it if the redirect fails to install). The hook keeps program
/// output and the protocol stream completely separate.
pub fn eval_with_capture<F, T>(f: F) -> (Result<T, String>, String)
where
    F: FnOnce() -> Result<T, SemaError>,
{
    use std::sync::{Arc, Mutex};

    let buf = Arc::new(Mutex::new(String::new()));
    let out = buf.clone();
    sema_core::set_stdout_hook(Some(Box::new(move |s: &str| {
        if let Ok(mut b) = out.lock() {
            b.push_str(s);
        }
    })));
    let err = buf.clone();
    sema_core::set_stderr_hook(Some(Box::new(move |s: &str| {
        if let Ok(mut b) = err.lock() {
            b.push_str(s);
        }
    })));

    // Run the operation, clearing the hooks even if it panics — otherwise a
    // leaked hook would capture into a dropped buffer and silence all later
    // output. The panic is re-raised for the dispatch-level handler to convert
    // into an error result.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

    sema_core::set_stdout_hook(None);
    sema_core::set_stderr_hook(None);

    let captured = buf.lock().map(|b| b.clone()).unwrap_or_default();

    match result {
        Ok(r) => (r.map_err(|e| format!("{e}")), captured),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// Run compiled bytecode on the VM
pub fn run_bytecode_bytes(
    interpreter: &Interpreter,
    bytes: &[u8],
) -> Result<sema_core::Value, SemaError> {
    let result = sema_vm::deserialize_from_bytes(bytes)?;

    let functions: Vec<std::rc::Rc<sema_vm::Function>> =
        result.functions.into_iter().map(std::rc::Rc::new).collect();
    let main_cache_slots = result.chunk.n_global_cache_slots;
    let closure = std::rc::Rc::new(sema_vm::Closure {
        func: std::rc::Rc::new(sema_vm::Function {
            name: None,
            chunk: result.chunk,
            upvalue_descs: Vec::new(),
            upvalue_names: Vec::new(),
            arity: 0,
            has_rest: false,
            local_names: Vec::new(),
            local_scopes: Vec::new(),
            source_file: None,
            cache_offset: 0,
        }),
        upvalues: Vec::new(),
        // Top-level main closure: uses the VM's own globals and function table.
        globals: None,
        functions: None,
    });

    let mut vm = sema_vm::VM::new(
        interpreter.global_env.clone(),
        functions,
        &[],
        main_cache_slots,
    )?;
    // Initialize the async scheduler so async/await and channels work when an
    // MCP `run_file` executes a `.semac` program. A `.semac` carries no native
    // table (the format is process-local), and bytecode compiled with
    // `known_natives=None` uses CallGlobal rather than CallNative, so task VMs
    // resolve natives via the shared global env — an empty native table is
    // correct here.
    sema_vm::init_scheduler(interpreter.global_env.clone(), Vec::new());
    vm.execute(closure, &interpreter.ctx)
}

/// Lists all default, notebook, and user-defined tools matching CLI filters
pub fn list_mcp_tools(
    interpreter: &Interpreter,
    include_tools: Option<&[String]>,
    exclude_tools: Option<&[String]>,
) -> Vec<Tool> {
    let mut tools = Vec::new();

    // 1. Register Default tools
    let default_tool_list = vec![
        ("run_file", "Run a Sema source (.sema) or compiled bytecode (.semac) file.", json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Path to the Sema file (relative to CWD or absolute)." },
                "arguments": { "type": "array", "items": { "type": "string" }, "description": "Optional positional arguments to pass to the script." }
            },
            "required": ["file_path"]
        })),
        ("compile", "Compile a .sema file to .semac bytecode.", json!({
            "type": "object",
            "properties": {
                "source_path": { "type": "string", "description": "Path to the source .sema file." },
                "output_path": { "type": "string", "description": "Destination path for the compiled .semac bytecode file (optional)." }
            },
            "required": ["source_path"]
        })),
        ("eval", "Evaluate a single Sema expression string and return the result and captured stdout/stderr.", json!({
            "type": "object",
            "properties": {
                "code": { "type": "string", "description": "The Sema expression to evaluate (e.g., '(+ 1 2)')." }
            },
            "required": ["code"]
        })),
        ("docs", "Get documentation and signatures for a Sema symbol or standard library function.", json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "The symbol or function name (e.g., 'string/split', 'llm/prompt')." }
            },
            "required": ["symbol"]
        })),
        ("fmt", "Format Sema code string or a .sema file in-place.", json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Path to format a .sema file in-place (optional)." },
                "code": { "type": "string", "description": "Raw Sema code string to format and return (optional)." }
            }
        })),
        ("disasm", "Disassemble a source .sema or bytecode .semac file into VM bytecode instructions.", json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "Path to the .sema or .semac file." }
            },
            "required": ["file_path"]
        })),
        ("build", "Build a standalone executable from a .sema file.", json!({
            "type": "object",
            "properties": {
                "source_path": { "type": "string", "description": "Path to the source .sema file." },
                "output_path": { "type": "string", "description": "Destination path for the compiled standalone binary executable." }
            },
            "required": ["source_path", "output_path"]
        })),
        ("info", "Get version and environment info about the running Sema MCP server.", json!({
            "type": "object",
            "properties": {}
        })),
        ("docs_search", "Search Sema documentation by natural-language query (e.g. 'reverse a list', 'read file lines'). Returns the most relevant builtins/special forms ranked by relevance, as a JSON array of {name, module, summary, score}. Use this to discover the right function or syntax when you don't already know its name; use `docs` for the full docs of a known symbol.", json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "What you're looking for, in plain words (e.g. 'parse json string', 'spawn an async task')." },
                "limit": { "type": "integer", "description": "Maximum number of results to return (default 5, max 25)." }
            },
            "required": ["query"]
        })),
    ];

    for (name, desc, schema) in default_tool_list {
        if is_tool_allowed(name, include_tools, exclude_tools) {
            tools.push(Tool {
                name: name.to_string(),
                description: desc.to_string(),
                input_schema: schema,
            });
        }
    }

    // 2. Register Notebook tools
    let notebook_tool_list = vec![
        ("notebook/new", "Create a new empty Sema notebook (.sema-nb) file.", json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Destination path for the new notebook file (e.g. 'notes.sema-nb')." },
                "title": { "type": "string", "description": "Optional title for the notebook (defaults to 'Untitled')." },
                "overwrite": { "type": "boolean", "description": "Replace an existing notebook at this path. Defaults to false; creation fails if the file already exists." }
            },
            "required": ["path"]
        })),
        ("notebook/read", "Read a Sema notebook (.sema-nb) structure, cell types, source code, and outputs.", json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the .sema-nb notebook file." }
            },
            "required": ["path"]
        })),
        ("notebook/add_cell", "Add a new cell (code or markdown) to a notebook.", json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the .sema-nb notebook file." },
                "type": { "type": "string", "enum": ["code", "markdown"], "description": "The type of the cell." },
                "source": { "type": "string", "description": "The source code or markdown content for the cell." },
                "after_id": { "type": "string", "description": "Optional cell ID to insert after. If omitted, appends to the end." }
            },
            "required": ["path", "type", "source"]
        })),
        ("notebook/update_cell", "Update the source code or cell type of an existing notebook cell.", json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the .sema-nb notebook file." },
                "id": { "type": "string", "description": "The unique cell ID." },
                "source": { "type": "string", "description": "New source content (optional)." },
                "type": { "type": "string", "enum": ["code", "markdown"], "description": "New cell type (optional)." }
            },
            "required": ["path", "id"]
        })),
        ("notebook/delete_cell", "Delete a cell from a notebook.", json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the .sema-nb notebook file." },
                "id": { "type": "string", "description": "The cell ID to remove." }
            },
            "required": ["path", "id"]
        })),
        ("notebook/eval_cell", "Evaluate a single code cell, saving outputs back to the file and returning results.", json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the .sema-nb notebook file." },
                "id": { "type": "string", "description": "The ID of the cell to evaluate." }
            },
            "required": ["path", "id"]
        })),
        ("notebook/eval_all", "Evaluate all code cells in order, saving outputs back to the file and returning results.", json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the .sema-nb notebook file." }
            },
            "required": ["path"]
        })),
        ("notebook/export", "Export a notebook to Markdown or a clean .sema source file.", json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the .sema-nb notebook file." },
                "format": { "type": "string", "enum": ["markdown", "source"], "description": "Target export format." },
                "output_path": { "type": "string", "description": "Optional destination path on disk. If omitted, returns the exported text directly." }
            },
            "required": ["path", "format"]
        })),
    ];

    for (name, desc, schema) in notebook_tool_list {
        if is_tool_allowed(name, include_tools, exclude_tools) {
            tools.push(Tool {
                name: name.to_string(),
                description: desc.to_string(),
                input_schema: schema,
            });
        }
    }

    // 3. Discover User Defined deftool definitions in Interpreter
    interpreter.global_env.iter_bindings(|_spur, val| {
        if let ValueViewRef::ToolDef(td) = val.view_ref() {
            if is_tool_allowed(&td.name, include_tools, exclude_tools) {
                // Ignore hidden prefix "_"
                if !td.name.starts_with('_') {
                    // Check metadata tags
                    let mut expose = true;
                    if let Some(map) = td.parameters.as_map_rc() {
                        if let Some(v) = map.get(&Value::keyword("mcp/expose")) {
                            if !v.is_truthy() {
                                expose = false;
                            }
                        }
                        if let Some(v) = map.get(&Value::keyword("private")) {
                            if v.is_truthy() {
                                expose = false;
                            }
                        }
                    }

                    if expose {
                        tools.push(Tool {
                            name: td.name.clone(),
                            description: td.description.clone(),
                            input_schema: sema_value_to_json_schema(&td.parameters),
                        });
                    }
                }
            }
        }
    });

    tools
}

fn is_tool_allowed(
    name: &str,
    include_tools: Option<&[String]>,
    exclude_tools: Option<&[String]>,
) -> bool {
    if let Some(inc) = include_tools {
        if !inc.iter().any(|t| t == name) {
            return false;
        }
    }
    if let Some(exc) = exclude_tools {
        if exc.iter().any(|t| t == name) {
            return false;
        }
    }
    true
}

/// Invokes standard dev tools, notebook tools, or user-defined deftools
/// Restores the previous `sys/args` binding on the shared interpreter env when
/// dropped, so a `run_file` call's argument override cannot leak into a later
/// call — even if evaluation panics and unwinds.
struct SysArgsGuard {
    env: std::rc::Rc<sema_core::Env>,
    prev: Option<Value>,
}

impl Drop for SysArgsGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(prev) => self.env.set(sema_core::intern("sys/args"), prev),
            None => {
                self.env.take(sema_core::intern("sys/args"));
            }
        }
    }
}

/// Extract a human-readable message from a caught panic payload.
fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Dispatch a tool call, converting any panic during evaluation into an
/// `isError` result instead of letting it unwind out and terminate the
/// (single-threaded) MCP server loop.
pub fn call_mcp_tool(
    name: &str,
    arguments: &JsonValue,
    interpreter: &Interpreter,
    notebook_cache: &NotebookCache,
    include_tools: Option<&[String]>,
    exclude_tools: Option<&[String]>,
) -> CallToolResult {
    let dispatch = std::panic::AssertUnwindSafe(|| {
        call_mcp_tool_inner(
            name,
            arguments,
            interpreter,
            notebook_cache,
            include_tools,
            exclude_tools,
        )
    });
    match std::panic::catch_unwind(dispatch) {
        Ok(result) => result,
        Err(panic) => error_result(format!(
            "Tool '{name}' panicked during evaluation: {}",
            panic_message(panic.as_ref())
        )),
    }
}

fn call_mcp_tool_inner(
    name: &str,
    arguments: &JsonValue,
    interpreter: &Interpreter,
    notebook_cache: &NotebookCache,
    include_tools: Option<&[String]>,
    exclude_tools: Option<&[String]>,
) -> CallToolResult {
    if !is_tool_allowed(name, include_tools, exclude_tools) {
        return CallToolResult {
            content: vec![ToolContent::Text {
                text: format!("Tool '{name}' is not allowed or excluded."),
            }],
            is_error: true,
        };
    }

    match name {
        "run_file" => {
            let file_path = match arguments.get("file_path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return error_result("Missing required parameter: file_path"),
            };
            let args = arguments.get("arguments").and_then(|v| v.as_array());

            let path = Path::new(file_path);
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => return error_result(format!("Failed to read {file_path}: {e}")),
            };

            // Setup sys/args override if parameters provided. The guard restores
            // the previous binding on scope exit — including a panic unwind — so
            // one tool call can't leak its args into the next on the shared env.
            let _args_guard = args.map(|arg_list| {
                let prev = interpreter.global_env.get(sema_core::intern("sys/args"));
                let lisp_args: Vec<Value> = arg_list
                    .iter()
                    .map(|v| Value::string(v.as_str().unwrap_or("")))
                    .collect();
                let list_val = Value::list(lisp_args);
                interpreter.global_env.set(
                    sema_core::intern("sys/args"),
                    Value::native_fn(sema_core::NativeFn::simple("sys/args", move |_| {
                        Ok(list_val.clone())
                    })),
                );
                SysArgsGuard {
                    env: interpreter.global_env.clone(),
                    prev,
                }
            });

            let (res, stdout) = eval_with_capture(|| {
                if sema_vm::is_bytecode_file(&bytes) {
                    run_bytecode_bytes(interpreter, &bytes)
                } else {
                    let source =
                        std::str::from_utf8(&bytes).map_err(|e| SemaError::eval(e.to_string()))?;
                    if let Ok(canonical) = path.canonicalize() {
                        interpreter.ctx.push_file_path(canonical);
                    }
                    let r = interpreter.eval_str_compiled(source);
                    interpreter.ctx.pop_file_path();
                    r
                }
            });

            drop(_args_guard);

            match res {
                Ok(val) => {
                    let mut text = stdout;
                    if !val.is_nil() {
                        text.push_str(&format!("\nResult: {val}"));
                    }
                    success_result(text)
                }
                Err(e) => {
                    let mut text = stdout;
                    text.push_str(&format!("\nError: {e}"));
                    CallToolResult {
                        content: vec![ToolContent::Text { text }],
                        is_error: true,
                    }
                }
            }
        }
        "compile" => {
            let source_path = match arguments.get("source_path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return error_result("Missing required parameter: source_path"),
            };
            let output_path = arguments.get("output_path").and_then(|v| v.as_str());

            let path = Path::new(source_path);
            let source = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => return error_result(format!("Failed to read {source_path}: {e}")),
            };

            let source_hash = crc32fast::hash(source.as_bytes());
            let compile_interpreter =
                Interpreter::new_with_sandbox(&sema_core::Sandbox::allow_all());
            let result = match compile_interpreter.compile_to_bytecode(&source) {
                Ok(r) => r,
                Err(e) => return error_result(format!("Compile error: {}", e.inner())),
            };

            let bytes = match sema_vm::serialize_to_bytes(&result, source_hash) {
                Ok(b) => b,
                Err(e) => return error_result(format!("Serialization error: {}", e.inner())),
            };

            let out_path = match output_path {
                Some(o) => std::path::PathBuf::from(o),
                None => path.with_extension("semac"),
            };

            if let Err(e) = std::fs::write(&out_path, &bytes) {
                return error_result(format!("Error writing {}: {e}", out_path.display()));
            }

            success_result(format!("Compiled successfully to {}", out_path.display()))
        }
        "eval" => {
            let code = match arguments.get("code").and_then(|v| v.as_str()) {
                Some(c) => c,
                None => return error_result("Missing required parameter: code"),
            };

            let (res, stdout) = eval_with_capture(|| interpreter.eval_str_compiled(code));

            match res {
                Ok(val) => {
                    let mut text = stdout;
                    if !val.is_nil() {
                        text.push_str(&format!("\nResult: {val}"));
                    }
                    success_result(text)
                }
                Err(e) => {
                    let mut text = stdout;
                    text.push_str(&format!("\nError: {e}"));
                    CallToolResult {
                        content: vec![ToolContent::Text { text }],
                        is_error: true,
                    }
                }
            }
        }
        "docs" => {
            let symbol = match arguments.get("symbol").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return error_result("Missing required parameter: symbol"),
            };

            // First check user-defined deftool in env
            let env_val = interpreter.global_env.get(sema_core::intern(symbol));
            if let Some(v) = env_val {
                if let ValueViewRef::ToolDef(td) = v.view_ref() {
                    let doc = format!(
                        "Tool: {}\nDescription: {}\nParameters: {}",
                        td.name,
                        td.description,
                        sema_core::pretty_print(&td.parameters, 80)
                    );
                    return success_result(doc);
                }
            }

            // Fallback to builtin docs database
            match get_builtin_doc(symbol) {
                Some(doc) => success_result(doc.clone()),
                None => error_result(format!("No documentation found for symbol: {symbol}")),
            }
        }
        "docs_search" => {
            let query = match arguments.get("query").and_then(|v| v.as_str()) {
                Some(q) => q,
                None => return error_result("Missing required parameter: query"),
            };
            let limit = arguments
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(crate::docs_search::DEFAULT_LIMIT);
            let hits = crate::docs_search::search(query, limit);
            if hits.is_empty() {
                return success_result(format!("No documentation matched query: {query}"));
            }
            match serde_json::to_string_pretty(&hits) {
                Ok(json) => success_result(json),
                Err(e) => error_result(format!("Failed to serialize search results: {e}")),
            }
        }
        "fmt" => {
            let file_path = arguments.get("file_path").and_then(|v| v.as_str());
            let code = arguments.get("code").and_then(|v| v.as_str());

            if let Some(file) = file_path {
                let path = Path::new(file);
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(e) => return error_result(format!("Failed to read {file}: {e}")),
                };
                let fmt_opts = sema_fmt::FormatOptions {
                    width: 80,
                    indent: 2,
                    align: false,
                };
                let formatted = match sema_fmt::format_source(&content, &fmt_opts) {
                    Ok(f) => f,
                    Err(e) => return error_result(format!("Format error: {e}")),
                };
                if let Err(e) = std::fs::write(path, formatted) {
                    return error_result(format!("Failed to write {file}: {e}"));
                }
                success_result(format!("Formatted file {file} in-place successfully."))
            } else if let Some(src) = code {
                let fmt_opts = sema_fmt::FormatOptions {
                    width: 80,
                    indent: 2,
                    align: false,
                };
                match sema_fmt::format_source(src, &fmt_opts) {
                    Ok(formatted) => success_result(formatted),
                    Err(e) => error_result(format!("Format error: {e}")),
                }
            } else {
                error_result("Must specify either file_path or code parameter.")
            }
        }
        "disasm" => {
            let file_path = match arguments.get("file_path").and_then(|v| v.as_str()) {
                Some(f) => f,
                None => return error_result("Missing required parameter: file_path"),
            };

            let bytes = match std::fs::read(file_path) {
                Ok(b) => b,
                Err(e) => return error_result(format!("Failed to read {file_path}: {e}")),
            };

            let compile_result = if sema_vm::is_bytecode_file(&bytes) {
                match sema_vm::deserialize_from_bytes(&bytes) {
                    Ok(r) => r,
                    Err(e) => return error_result(format!("Deserialization error: {}", e.inner())),
                }
            } else {
                let source = match std::str::from_utf8(&bytes) {
                    Ok(s) => s,
                    Err(e) => return error_result(format!("Invalid UTF-8 in source: {e}")),
                };
                let compile_interpreter =
                    Interpreter::new_with_sandbox(&sema_core::Sandbox::allow_all());
                match compile_interpreter.compile_to_bytecode(source) {
                    Ok(r) => r,
                    Err(e) => return error_result(format!("Compile error: {}", e.inner())),
                }
            };

            let mut disasm_str = sema_vm::disassemble(&compile_result.chunk, Some("<main>"));
            for (i, func) in compile_result.functions.iter().enumerate() {
                let name = func
                    .name
                    .map(sema_core::resolve)
                    .unwrap_or_else(|| format!("<fn {i}>"));
                disasm_str.push_str(&sema_vm::disassemble(&func.chunk, Some(&name)));
            }

            success_result(disasm_str)
        }
        "build" => {
            let source_path = match arguments.get("source_path").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return error_result("Missing required parameter: source_path"),
            };
            let output_path = match arguments.get("output_path").and_then(|v| v.as_str()) {
                Some(o) => o,
                None => return error_result("Missing required parameter: output_path"),
            };

            // Call standard sema command line tool using current executable
            let exe = match std::env::current_exe() {
                Ok(e) => e,
                Err(e) => return error_result(format!("Failed to get current executable: {e}")),
            };

            let output = match std::process::Command::new(exe)
                .arg("build")
                .arg(source_path)
                .arg("-o")
                .arg(output_path)
                .output()
            {
                Ok(out) => out,
                Err(e) => return error_result(format!("Failed to execute build subprocess: {e}")),
            };

            if output.status.success() {
                success_result(format!(
                    "Built standalone binary successfully at {output_path}"
                ))
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                error_result(format!("Build process failed:\n{stderr}"))
            }
        }
        "info" => {
            let info_str = format!(
                "Sema MCP Server v{}\nTarget Platform: {}/{}\nSandbox: unrestricted — tools execute with full filesystem, network, and shell access in the host environment. Only connect trusted clients.",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
            );
            success_result(info_str)
        }
        // Stateful Notebook operations
        "notebook/new" => {
            let path_str = match arguments.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return error_result("Missing required parameter: path"),
            };
            let title = arguments.get("title").and_then(|v| v.as_str());
            let overwrite = arguments
                .get("overwrite")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            match crate::notebook::create_notebook(notebook_cache, path_str, title, overwrite) {
                Ok(canonical) => success_result(format!(
                    "Created new empty notebook at {}",
                    canonical.display()
                )),
                Err(e) => error_result(format!("Failed to create notebook: {e}")),
            }
        }
        "notebook/read" => {
            let path_str = match arguments.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return error_result("Missing required parameter: path"),
            };

            match get_or_create_engine(notebook_cache, path_str) {
                Ok((_, engine_rc)) => {
                    let engine = engine_rc.borrow();
                    let json_rep = serde_json::to_value(&engine.notebook).unwrap_or(json!({}));
                    success_result(serde_json::to_string_pretty(&json_rep).unwrap())
                }
                Err(e) => error_result(format!("Failed to read notebook: {e}")),
            }
        }
        "notebook/add_cell" => {
            let path_str = match arguments.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return error_result("Missing required parameter: path"),
            };
            let cell_type = match arguments.get("type").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => return error_result("Missing required parameter: type"),
            };
            let source = match arguments.get("source").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return error_result("Missing required parameter: source"),
            };
            let after_id = arguments.get("after_id").and_then(|v| v.as_str());

            match get_or_create_engine(notebook_cache, path_str) {
                Ok((canonical, engine_rc)) => {
                    match add_cell(
                        notebook_cache,
                        &engine_rc,
                        &canonical,
                        cell_type,
                        source,
                        after_id,
                    ) {
                        Ok(cell_id) => success_result(json!({ "cell_id": cell_id }).to_string()),
                        Err(e) => error_result(format!("Failed to add cell: {e}")),
                    }
                }
                Err(e) => error_result(format!("Failed to load notebook: {e}")),
            }
        }
        "notebook/update_cell" => {
            let path_str = match arguments.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return error_result("Missing required parameter: path"),
            };
            let cell_id = match arguments.get("id").and_then(|v| v.as_str()) {
                Some(i) => i,
                None => return error_result("Missing required parameter: id"),
            };
            let source = arguments.get("source").and_then(|v| v.as_str());
            let cell_type = arguments.get("type").and_then(|v| v.as_str());

            match get_or_create_engine(notebook_cache, path_str) {
                Ok((canonical, engine_rc)) => {
                    match update_cell(
                        notebook_cache,
                        &engine_rc,
                        &canonical,
                        cell_id,
                        source,
                        cell_type,
                    ) {
                        Ok(_) => success_result("Cell updated successfully."),
                        Err(e) => error_result(format!("Failed to update cell: {e}")),
                    }
                }
                Err(e) => error_result(format!("Failed to load notebook: {e}")),
            }
        }
        "notebook/delete_cell" => {
            let path_str = match arguments.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return error_result("Missing required parameter: path"),
            };
            let cell_id = match arguments.get("id").and_then(|v| v.as_str()) {
                Some(i) => i,
                None => return error_result("Missing required parameter: id"),
            };

            match get_or_create_engine(notebook_cache, path_str) {
                Ok((canonical, engine_rc)) => {
                    match delete_cell(notebook_cache, &engine_rc, &canonical, cell_id) {
                        Ok(_) => success_result("Cell deleted successfully."),
                        Err(e) => error_result(format!("Failed to delete cell: {e}")),
                    }
                }
                Err(e) => error_result(format!("Failed to load notebook: {e}")),
            }
        }
        "notebook/eval_cell" => {
            let path_str = match arguments.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return error_result("Missing required parameter: path"),
            };
            let cell_id = match arguments.get("id").and_then(|v| v.as_str()) {
                Some(i) => i,
                None => return error_result("Missing required parameter: id"),
            };

            match get_or_create_engine(notebook_cache, path_str) {
                Ok((canonical, engine_rc)) => {
                    let result = {
                        let mut engine = engine_rc.borrow_mut();
                        match engine.eval_cell(cell_id) {
                            Ok(res) => {
                                // Save notebook state
                                let _ = engine.notebook.save(&canonical);
                                let response_json = json!({
                                    "stdout": res.stdout,
                                    "display": res.output.display,
                                    "sema_value": res.output.sema_value,
                                    "requires_reeval": res.output.requires_reeval
                                });
                                Ok(success_result(response_json.to_string()))
                            }
                            Err(e) => Err(format!("Cell evaluation failed: {e}")),
                        }
                    };
                    // Keep the cache mtime coherent with our own save above.
                    crate::notebook::note_external_save(notebook_cache, &canonical);
                    match result {
                        Ok(r) => r,
                        Err(e) => error_result(e),
                    }
                }
                Err(e) => error_result(format!("Failed to load notebook: {e}")),
            }
        }
        "notebook/eval_all" => {
            let path_str = match arguments.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return error_result("Missing required parameter: path"),
            };

            match get_or_create_engine(notebook_cache, path_str) {
                Ok((canonical, engine_rc)) => {
                    let results = {
                        let mut engine = engine_rc.borrow_mut();
                        let results = engine.eval_all();
                        let _ = engine.notebook.save(&canonical);
                        results
                    };
                    // Keep the cache mtime coherent with our own save above.
                    crate::notebook::note_external_save(notebook_cache, &canonical);

                    let formatted_results: Vec<serde_json::Value> = results
                        .into_iter()
                        .map(|(cell_id, res)| match res {
                            Ok(eval_res) => json!({
                                "cell_id": cell_id,
                                "success": true,
                                "stdout": eval_res.stdout,
                                "display": eval_res.output.display,
                                "sema_value": eval_res.output.sema_value
                            }),
                            Err(e) => json!({
                                "cell_id": cell_id,
                                "success": false,
                                "error": e
                            }),
                        })
                        .collect();

                    success_result(serde_json::Value::Array(formatted_results).to_string())
                }
                Err(e) => error_result(format!("Failed to load notebook: {e}")),
            }
        }
        "notebook/export" => {
            let path_str = match arguments.get("path").and_then(|v| v.as_str()) {
                Some(p) => p,
                None => return error_result("Missing required parameter: path"),
            };
            let format = match arguments.get("format").and_then(|v| v.as_str()) {
                Some(f) => f,
                None => return error_result("Missing required parameter: format"),
            };
            let output_path = arguments.get("output_path").and_then(|v| v.as_str());

            match get_or_create_engine(notebook_cache, path_str) {
                Ok((_, engine_rc)) => match export_notebook(&engine_rc, format) {
                    Ok(exported_text) => {
                        if let Some(out_p) = output_path {
                            if let Err(e) = std::fs::write(out_p, &exported_text) {
                                return error_result(format!(
                                    "Failed to write export to {out_p}: {e}"
                                ));
                            }
                            success_result(format!("Notebook exported successfully to {out_p}"))
                        } else {
                            success_result(exported_text)
                        }
                    }
                    Err(e) => error_result(format!("Failed to export notebook: {e}")),
                },
                Err(e) => error_result(format!("Failed to load notebook: {e}")),
            }
        }
        // User deftool definitions
        _ => {
            let bindings = interpreter.global_env.get(sema_core::intern(name));
            if let Some(v) = bindings {
                if let ValueViewRef::ToolDef(td) = v.view_ref() {
                    let sema_args = match json_args_to_sema(&td.parameters, arguments, &td.handler)
                    {
                        Ok(a) => a,
                        Err(e) => {
                            return error_result(format!(
                                "Invalid arguments for tool '{name}': {e}"
                            ));
                        }
                    };
                    let (res, stdout) =
                        eval_with_capture(|| call_value(&interpreter.ctx, &td.handler, &sema_args));

                    match res {
                        Ok(val) => {
                            let mut text = stdout;
                            if let Some(s) = val.as_str() {
                                text.push_str(s);
                            } else if val.as_map_rc().is_some() || val.as_seq().is_some() {
                                let json_lossy = sema_core::value_to_json_lossy(&val);
                                text.push_str(
                                    &serde_json::to_string(&json_lossy)
                                        .unwrap_or_else(|_| val.to_string()),
                                );
                            } else if !val.is_nil() {
                                text.push_str(&val.to_string());
                            }
                            success_result(text)
                        }
                        Err(e) => {
                            let mut text = stdout;
                            text.push_str(&format!("\nError: {e}"));
                            CallToolResult {
                                content: vec![ToolContent::Text { text }],
                                is_error: true,
                            }
                        }
                    }
                } else {
                    error_result(format!("Symbol '{name}' is defined but is not a tool."))
                }
            } else {
                error_result(format!("Unknown tool: {name}"))
            }
        }
    }
}

fn success_result(text: impl Into<String>) -> CallToolResult {
    CallToolResult {
        content: vec![ToolContent::Text { text: text.into() }],
        is_error: false,
    }
}

fn error_result(text: impl Into<String>) -> CallToolResult {
    CallToolResult {
        content: vec![ToolContent::Text { text: text.into() }],
        is_error: true,
    }
}
