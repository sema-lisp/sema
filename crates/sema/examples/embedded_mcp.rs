//! An MCP server using only the public sema-lang API.
use sema::mcp::{McpServerOptions, McpSession};
use sema::{Caps, Interpreter, Sandbox};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let interpreter = Interpreter::builder()
        .with_sandbox(Sandbox::deny(Caps::STRICT))
        .with_mcp(false)
        .with_llm(false)
        .build();
    interpreter.register_async_fn_typed("host/double", |n: i64| async move {
        Ok::<_, std::convert::Infallible>(n * 2)
    });
    interpreter.eval_str(
        r#"(deftool double "Double a number in the Rust host."
        {:n {:type :integer}} (lambda (n) (host/double n)))"#,
    )?;
    McpSession::new(
        &interpreter,
        McpServerOptions::embedded().server_info("embedded-example", env!("CARGO_PKG_VERSION")),
    )?
    .serve_stdio(&interpreter)?;
    Ok(())
}
