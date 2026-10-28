//! Shared MCP arguments for the CLI and standalone MCP binaries.
use clap::{Args, Parser, ValueEnum};
use sema_mcp::session::{LogEvent, McpServerOptions, McpSession};

#[derive(Clone, Copy, Default, ValueEnum)]
pub(crate) enum Preset {
    #[default]
    Cli,
    Embedded,
}

#[derive(Args, Default)]
pub(crate) struct McpServeArgs {
    /// Comma-separated tool allowlist, applied after the preset
    #[arg(long, value_name = "TOOLS")]
    pub include: Option<String>,
    /// Comma-separated tool denylist; exclusions take precedence
    #[arg(long, value_name = "TOOLS")]
    pub exclude: Option<String>,
    /// Sandbox mode, such as strict or no-fs-write,no-shell
    #[arg(long, value_name = "MODE")]
    pub sandbox: Option<String>,
    /// Per-call time budget in milliseconds. Calls may shorten it. 0 disables the session limit
    #[arg(long, value_name = "MS")]
    pub timeout_ms: Option<u64>,
    /// Tool preset; embedded exposes visible deftools, docs, docs_search, and info
    #[arg(long, value_enum, default_value = "cli")]
    pub preset: Preset,
    /// Evaluate the tool files, print the selected tool descriptions as JSON, then exit
    #[arg(long)]
    pub check: bool,
}

#[derive(Parser)]
#[command(name = "embedded-sema")]
struct EmbeddedArgs {
    #[arg(long,num_args=0..=1,default_missing_value="",require_equals=true)]
    mcp: Option<String>,
    #[command(flatten)]
    serve: McpServeArgs,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    _script_args: Vec<String>,
}

pub(crate) fn parse_embedded(args: Vec<String>) -> Result<McpServeArgs, clap::Error> {
    EmbeddedArgs::try_parse_from(args).map(|args| args.serve)
}

pub(crate) fn run(
    interpreter: &sema_eval::Interpreter,
    args: McpServeArgs,
    with_build: bool,
) -> Result<(), String> {
    let mut options = match args.preset {
        Preset::Cli => McpServerOptions::cli(),
        Preset::Embedded => McpServerOptions::embedded(),
    }
    .timeout(crate::resolve_mcp_tool_timeout(args.timeout_ms));
    if let Some(names) = args.include {
        options = options.only_tools(names.split(',').map(str::trim));
    }
    if let Some(names) = args.exclude {
        options = options.exclude_tools(names.split(',').map(str::trim));
    }
    if with_build {
        options = options.with_build_hook(crate::build::mcp_build);
    }
    options = options.on_log(|event| match event {
        LogEvent::Started => eprintln!("Sema MCP server starting stdio loop..."),
        LogEvent::Stopped => eprintln!("Sema MCP server stdio loop exited."),
        LogEvent::WriteFailed => eprintln!("MCP response write failed"),
        LogEvent::ToolShadowed(name) => {
            eprintln!("MCP tool '{name}' uses a reserved name and is hidden")
        }
        LogEvent::ToolExcluded(_) => {}
        _ => {}
    });
    let session = McpSession::new(interpreter, options).map_err(|e| e.to_string())?;
    if args.check {
        serde_json::to_writer_pretty(std::io::stdout().lock(), &session.list_tools(interpreter))
            .map_err(|e| e.to_string())?;
        println!();
        Ok(())
    } else {
        session.serve_stdio(interpreter).map_err(|e| e.to_string())
    }
}
