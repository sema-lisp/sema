use serde_json::{json, Value as JsonValue};
use std::path::Path;
use std::sync::OnceLock;

use sema_core::{SemaError, Value, ValueView};
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

/// Convert JSON arguments into a list of Sema values based on the parameter schema order.
pub fn json_args_to_sema(
    params: &Value,
    arguments: &serde_json::Value,
    handler: &Value,
) -> Vec<Value> {
    if let serde_json::Value::Object(json_obj) = arguments {
        if let Some(lambda) = handler.as_lambda_rc() {
            return lambda
                .params
                .iter()
                .map(|name| {
                    json_obj
                        .get(&sema_core::resolve(*name))
                        .map(sema_core::json_to_value)
                        .unwrap_or(Value::nil())
                })
                .collect();
        }
        if let Some((closure, _)) = sema_vm::extract_vm_closure(handler) {
            let mut params_ordered = vec![sema_core::intern(""); closure.func.arity as usize];
            for &(slot, name) in &closure.func.local_names {
                if (slot as usize) < params_ordered.len() {
                    params_ordered[slot as usize] = name;
                }
            }
            return params_ordered
                .iter()
                .map(|name| {
                    let name_str = sema_core::resolve(*name);
                    if name_str.is_empty() {
                        Value::nil()
                    } else {
                        json_obj
                            .get(&name_str)
                            .map(sema_core::json_to_value)
                            .unwrap_or(Value::nil())
                    }
                })
                .collect();
        }
        if let Some(param_map) = params.as_map_rc() {
            return param_map
                .keys()
                .map(|k| {
                    let key_str = k
                        .as_keyword()
                        .or_else(|| k.as_str().map(|s| s.to_string()))
                        .unwrap_or_else(|| k.to_string());
                    json_obj
                        .get(&key_str)
                        .map(sema_core::json_to_value)
                        .unwrap_or(Value::nil())
                })
                .collect();
        }
    }
    vec![sema_core::json_to_value(arguments)]
}

/// Evaluate a Lisp operation with stdout and stderr capture
pub fn eval_with_capture<F, T>(f: F) -> (Result<T, String>, String)
where
    F: FnOnce() -> Result<T, SemaError>,
{
    let mut captured = String::new();
    let res = {
        let redirect_out = gag::BufferRedirect::stdout();
        let redirect_err = gag::BufferRedirect::stderr();
        let r = f();
        if let Ok(mut red) = redirect_out {
            use std::io::Read;
            let _ = red.read_to_string(&mut captured);
        }
        if let Ok(mut red) = redirect_err {
            use std::io::Read;
            let mut captured_err = String::new();
            if red.read_to_string(&mut captured_err).is_ok() && !captured_err.is_empty() {
                if !captured.is_empty() {
                    captured.push('\n');
                }
                captured.push_str("Stderr:\n");
                captured.push_str(&captured_err);
            }
        }
        r
    };
    (res.map_err(|e| format!("{e}")), captured)
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
            arity: 0,
            has_rest: false,
            local_names: Vec::new(),
            source_file: None,
            cache_offset: 0,
        }),
        upvalues: Vec::new(),
    });

    let mut vm = sema_vm::VM::new(
        interpreter.global_env.clone(),
        functions,
        &[],
        main_cache_slots,
    )?;
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
                "arguments": { "type": "array", "items": { "type": "string" }, "description": "Optional positional arguments to pass to the script." },
                "sandbox": { "type": "string", "enum": ["strict", "no-shell", "no-network", "allow-all"], "description": "Optional sandbox override mode." }
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
                "code": { "type": "string", "description": "The Sema expression to evaluate (e.g., '(+ 1 2)')." },
                "sandbox": { "type": "string", "enum": ["strict", "no-shell", "no-network", "allow-all"], "description": "Optional sandbox override mode." }
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
                "title": { "type": "string", "description": "Optional title for the notebook (defaults to 'Untitled')." }
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
        if let ValueView::ToolDef(td) = val.view() {
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
pub fn call_mcp_tool(
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
            let _sandbox = arguments.get("sandbox").and_then(|v| v.as_str()); // sandbox configs can be supported if desired

            let path = Path::new(file_path);
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => return error_result(format!("Failed to read {file_path}: {e}")),
            };

            // Setup sys/args override if parameters provided
            let prev_args = interpreter.global_env.get(sema_core::intern("sys/args"));
            if let Some(arg_list) = args {
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
            }

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

            // Restore sys/args
            if let Some(prev) = prev_args {
                interpreter
                    .global_env
                    .set(sema_core::intern("sys/args"), prev);
            } else {
                interpreter.global_env.take(sema_core::intern("sys/args"));
            }

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
                if let ValueView::ToolDef(td) = v.view() {
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
        "fmt" => {
            let file_path = arguments.get("file_path").and_then(|v| v.as_str());
            let code = arguments.get("code").and_then(|v| v.as_str());

            if let Some(file) = file_path {
                let path = Path::new(file);
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(e) => return error_result(format!("Failed to read {file}: {e}")),
                };
                let formatted = match sema_fmt::format_source_opts(&content, 80, 2, false) {
                    Ok(f) => f,
                    Err(e) => return error_result(format!("Format error: {e}")),
                };
                if let Err(e) = std::fs::write(path, formatted) {
                    return error_result(format!("Failed to write {file}: {e}"));
                }
                success_result(format!("Formatted file {file} in-place successfully."))
            } else if let Some(src) = code {
                match sema_fmt::format_source_opts(src, 80, 2, false) {
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
                "Sema MCP Server v{}\nRust version: {}\nTarget Platform: {}\nEnvironment Context: standard",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_VERSION"), // standard workspace version
                std::env::consts::OS
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

            match crate::notebook::create_notebook(notebook_cache, path_str, title) {
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
                    match add_cell(&engine_rc, &canonical, cell_type, source, after_id) {
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
                    match update_cell(&engine_rc, &canonical, cell_id, source, cell_type) {
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
                Ok((canonical, engine_rc)) => match delete_cell(&engine_rc, &canonical, cell_id) {
                    Ok(_) => success_result("Cell deleted successfully."),
                    Err(e) => error_result(format!("Failed to delete cell: {e}")),
                },
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
                            success_result(response_json.to_string())
                        }
                        Err(e) => error_result(format!("Cell evaluation failed: {e}")),
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
                    let mut engine = engine_rc.borrow_mut();
                    let results = engine.eval_all();
                    let _ = engine.notebook.save(&canonical);

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
                if let ValueView::ToolDef(td) = v.view() {
                    let sema_args = json_args_to_sema(&td.parameters, arguments, &td.handler);
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
