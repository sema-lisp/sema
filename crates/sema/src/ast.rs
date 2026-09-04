use crate::die;
use crate::read_source_file;
use sema_core::Value;
use sema_core::ValueView;

pub(crate) fn run_ast(file: Option<String>, eval: Option<String>, json: bool) {
    let source = match (&file, &eval) {
        (Some(path), None) => match read_source_file(path) {
            Ok(content) => content,
            Err(msg) => {
                die(msg);
            }
        },
        (None, Some(expr)) => expr.clone(),
        (Some(_), Some(_)) => {
            die("cannot specify both a file and --eval");
        }
        (None, None) => {
            die("provide a file or --eval expression");
        }
    };

    let exprs = match sema_reader::read_many(&source) {
        Ok(exprs) => exprs,
        Err(e) => {
            die(format!("parsing failed: {}", e.format_plain()));
        }
    };

    if json {
        let json_ast: Vec<serde_json::Value> = exprs.iter().map(value_to_ast_json).collect();
        let output = if json_ast.len() == 1 {
            serde_json::to_string_pretty(&json_ast[0]).unwrap()
        } else {
            serde_json::to_string_pretty(&json_ast).unwrap()
        };
        println!("{output}");
    } else {
        for (i, expr) in exprs.iter().enumerate() {
            if i > 0 {
                println!();
            }
            print_ast(expr, 0);
        }
    }
}

fn value_to_ast_json(val: &Value) -> serde_json::Value {
    match val.view() {
        ValueView::Nil => serde_json::Value::Object(
            [("type".to_string(), serde_json::Value::String("nil".into()))]
                .into_iter()
                .collect(),
        ),
        ValueView::Bool(b) => serde_json::Value::Object(
            [
                ("type".to_string(), serde_json::Value::String("bool".into())),
                ("value".to_string(), serde_json::Value::Bool(b)),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Int(n) => serde_json::Value::Object(
            [
                ("type".to_string(), serde_json::Value::String("int".into())),
                ("value".to_string(), serde_json::Value::Number(n.into())),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Float(f) => serde_json::Value::Object(
            [
                (
                    "type".to_string(),
                    serde_json::Value::String("float".into()),
                ),
                (
                    "value".to_string(),
                    serde_json::Number::from_f64(f)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::String(s) => serde_json::Value::Object(
            [
                (
                    "type".to_string(),
                    serde_json::Value::String("string".into()),
                ),
                (
                    "value".to_string(),
                    serde_json::Value::String(s.to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Symbol(s) => serde_json::Value::Object(
            [
                (
                    "type".to_string(),
                    serde_json::Value::String("symbol".into()),
                ),
                (
                    "value".to_string(),
                    serde_json::Value::String(sema_core::resolve(s)),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Keyword(s) => serde_json::Value::Object(
            [
                (
                    "type".to_string(),
                    serde_json::Value::String("keyword".into()),
                ),
                (
                    "value".to_string(),
                    serde_json::Value::String(sema_core::resolve(s)),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::List(items) => serde_json::Value::Object(
            [
                ("type".to_string(), serde_json::Value::String("list".into())),
                (
                    "children".to_string(),
                    serde_json::Value::Array(items.iter().map(value_to_ast_json).collect()),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Vector(items) => serde_json::Value::Object(
            [
                (
                    "type".to_string(),
                    serde_json::Value::String("vector".into()),
                ),
                (
                    "children".to_string(),
                    serde_json::Value::Array(items.iter().map(value_to_ast_json).collect()),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        ValueView::Map(map) => serde_json::Value::Object(
            [
                ("type".to_string(), serde_json::Value::String("map".into())),
                (
                    "entries".to_string(),
                    serde_json::Value::Array(
                        map.iter()
                            .map(|(k, v)| {
                                serde_json::Value::Object(
                                    [
                                        ("key".to_string(), value_to_ast_json(k)),
                                        ("value".to_string(), value_to_ast_json(v)),
                                    ]
                                    .into_iter()
                                    .collect(),
                                )
                            })
                            .collect(),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
        ),
        _ => serde_json::Value::Object(
            [(
                "type".to_string(),
                serde_json::Value::String(val.type_name().into()),
            )]
            .into_iter()
            .collect(),
        ),
    }
}

fn print_ast(val: &Value, indent: usize) {
    let pad = "  ".repeat(indent);
    match val.view() {
        ValueView::Nil => println!("{pad}Nil"),
        ValueView::Bool(b) => println!("{pad}Bool {b}"),
        ValueView::Int(n) => println!("{pad}Int {n}"),
        ValueView::Float(f) => println!("{pad}Float {f}"),
        ValueView::String(s) => println!("{pad}String {s:?}"),
        ValueView::Symbol(s) => println!("{pad}Symbol {}", sema_core::resolve(s)),
        ValueView::Keyword(s) => println!("{pad}Keyword :{}", sema_core::resolve(s)),
        ValueView::List(items) => {
            println!("{pad}List");
            for item in items.iter() {
                print_ast(item, indent + 1);
            }
        }
        ValueView::Vector(items) => {
            println!("{pad}Vector");
            for item in items.iter() {
                print_ast(item, indent + 1);
            }
        }
        ValueView::Map(map) => {
            println!("{pad}Map");
            for (k, v) in map.iter() {
                println!("{pad}  Entry");
                print_ast(k, indent + 2);
                print_ast(v, indent + 2);
            }
        }
        _ => println!("{pad}{}", val.type_name()),
    }
}
