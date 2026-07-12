use std::cell::RefCell;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use sema_core::{archive, pretty_print, SemaError, Value, ValueView};
use sema_eval::Interpreter;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
struct SemaConfig {
    #[serde(default)]
    fmt: FmtConfig,
}

#[derive(Debug, Deserialize)]
struct FmtConfig {
    #[serde(default = "default_width")]
    width: usize,
    #[serde(default = "default_indent")]
    indent: usize,
    #[serde(default)]
    align: bool,
}

impl Default for FmtConfig {
    fn default() -> Self {
        Self {
            width: 80,
            indent: 2,
            align: false,
        }
    }
}

fn default_width() -> usize {
    sema_fmt::FormatOptions::default().width
}
fn default_indent() -> usize {
    sema_fmt::FormatOptions::default().indent
}

/// Walk up from cwd to find sema.toml
fn find_config() -> Option<SemaConfig> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("sema.toml");
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate).ok()?;
            return toml::from_str(&text).ok();
        }
        if !dir.pop() {
            return None;
        }
    }
}

mod colors;
mod cross_compile;
mod docs;
mod import_tracer;
mod pkg;
mod repl;
mod update;
mod web;
mod workflow_check;
// The dashboard server itself lives in the `sema` LIBRARY crate
// (`crates/sema/src/lib.rs` → `pub mod workflow_view;`), not here, so
// `crates/sema/tests/*.rs` integration tests can drive it in-process. Referenced
// below as `sema::workflow_view::…`.
use sema::workflow_view;

/// Read a source file with consistent, friendly error messages.
///
/// Standardises the wording across all subcommands so users see the same
/// phrasing for not-found / permission-denied errors regardless of which
/// command they ran.
fn read_source_file(path: impl AsRef<Path>) -> Result<String, String> {
    let p = path.as_ref();
    std::fs::read_to_string(p).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("file not found: {}", p.display()),
        std::io::ErrorKind::PermissionDenied => {
            format!("permission denied: {}", p.display())
        }
        _ => format!("reading {}: {}", p.display(), e),
    })
}

thread_local! {
    pub(crate) static LAST_SOURCE: RefCell<Option<String>> = const { RefCell::new(None) };
    pub(crate) static LAST_FILE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

// REPL completer, command set, and trait impls have moved to `src/repl/`.

#[derive(Parser)]
#[command(name = "sema", about = "Sema: A Lisp with LLM primitives", version)]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// File to execute
    #[arg(conflicts_with_all = ["eval", "print"])]
    file: Option<String>,

    /// Evaluate an expression and print result (if non-nil)
    #[arg(short, long, conflicts_with_all = ["print", "file"])]
    eval: Option<String>,

    /// Evaluate an expression and always print result
    #[arg(short, long, conflicts_with_all = ["eval", "file"])]
    print: Option<String>,

    /// Load file(s) before executing
    #[arg(short, long = "load", action = clap::ArgAction::Append)]
    load: Vec<String>,

    /// Suppress REPL banner
    #[arg(short, long)]
    quiet: bool,

    /// Enter REPL after running file or eval
    #[arg(short, long)]
    interactive: bool,

    /// Disable LLM features (skip provider auto-configuration)
    #[arg(long)]
    no_llm: bool,

    /// Sandbox mode: restrict dangerous operations.
    /// Values: "strict", "all", or comma-separated list like "no-shell,no-network,no-fs-write"
    /// Available capabilities: shell, fs-read, fs-write, network, env-read, env-write, process, llm, serial
    #[arg(long)]
    sandbox: Option<String>,

    /// Set default chat model
    #[arg(long)]
    chat_model: Option<String>,

    /// Set chat provider (anthropic, openai, gemini, groq, xai, mistral, moonshot, ollama)
    #[arg(long)]
    chat_provider: Option<String>,

    /// Set embedding model
    #[arg(long)]
    embedding_model: Option<String>,

    /// Set embedding provider (jina, voyage, cohere, openai)
    #[arg(long)]
    embedding_provider: Option<String>,

    /// Restrict file operations to these directories (comma-separated)
    #[arg(long, value_name = "DIRS")]
    allowed_paths: Option<String>,

    /// Arguments passed to the script (after --)
    #[arg(last = true)]
    script_args: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse source and print the AST
    Ast {
        /// File to parse
        file: Option<String>,

        /// Expression to parse
        #[arg(short, long)]
        eval: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,

        /// Install completions to the standard location
        #[arg(long)]
        install: bool,
    },
    /// Compile source to bytecode (.semac); imports resolve at runtime
    Compile {
        /// Source file to compile
        file: String,

        /// Output file path (default: input with .semac extension)
        #[arg(short, long)]
        output: Option<String>,

        /// Validate a .semac file without executing
        #[arg(long)]
        check: bool,
    },
    /// Disassemble a .semac bytecode file
    Disasm {
        /// Bytecode file to disassemble
        file: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Browse builtin and special-form docs
    #[command(args_conflicts_with_subcommands = true)]
    Doc {
        /// Show docs in a pager even when the output fits on one screen
        #[arg(long, conflicts_with = "no_pager")]
        pager: bool,

        /// Print directly without invoking a pager
        #[arg(long, conflicts_with = "pager")]
        no_pager: bool,

        #[command(subcommand)]
        command: Option<DocCommands>,

        /// Symbol to show documentation for (implicit `show`)
        symbol: Option<String>,
    },
    /// Sema package manager for adding, updating, and publishing dependencies
    Pkg {
        #[command(subcommand)]
        command: PkgCommands,
    },
    /// Build a standalone executable with dependencies bundled
    Build {
        /// Source file to compile and bundle
        #[arg(required_unless_present = "list_targets")]
        file: Option<String>,

        /// Output executable path (default: filename without extension)
        #[arg(short, long)]
        output: Option<String>,

        /// Additional files or directories to bundle (repeatable)
        #[arg(long = "include", action = clap::ArgAction::Append)]
        includes: Vec<String>,

        /// Sema binary to use as runtime base (default: current executable)
        #[arg(long, conflicts_with = "target")]
        runtime: Option<String>,

        /// Target platform triple or alias (e.g. linux, macos, windows, web, or a full triple).
        /// Use "all" to build for all supported targets.
        #[arg(long)]
        target: Option<String>,

        /// Show all supported target platforms
        #[arg(long)]
        list_targets: bool,

        /// Force re-download of cached runtime binaries
        #[arg(long)]
        no_cache: bool,
    },
    /// Format Sema source files
    Fmt {
        /// Files or glob patterns to format (default: **/*.sema in current directory)
        files: Vec<String>,

        /// Check formatting without writing changes (exit 1 if unformatted)
        #[arg(long)]
        check: bool,

        /// Print diff of formatting changes
        #[arg(long)]
        diff: bool,

        /// Max line width (default: 80, or value from sema.toml)
        #[arg(long)]
        width: Option<usize>,

        /// Indentation width for body forms (default: 2, or value from sema.toml)
        #[arg(long)]
        indent: Option<usize>,

        /// Align consecutive similar forms (defines, cond clauses, let bindings)
        #[arg(long)]
        align: bool,

        /// Output result as JSON (useful for editor integrations)
        #[arg(long)]
        json: bool,
    },
    /// Start the Language Server Protocol (LSP) server
    Lsp,
    /// Start the Debug Adapter Protocol (DAP) server
    Dap,
    /// Start the Model Context Protocol (MCP) server, or manage client auth (`login`/`logout`)
    #[command(args_conflicts_with_subcommands = true)]
    Mcp {
        /// Client-auth subcommand; when omitted, runs the MCP server
        #[command(subcommand)]
        auth: Option<McpAuthCommands>,
        /// Optional source files to run/load tools from (server mode)
        #[arg(value_name = "FILES")]
        files: Vec<String>,
        /// Comma-separated list of tool names to explicitly include
        #[arg(long, value_name = "TOOLS")]
        include: Option<String>,
        /// Comma-separated list of tool names to explicitly exclude
        #[arg(long, value_name = "TOOLS")]
        exclude: Option<String>,
    },
    /// Cell-based notebook with a browser UI
    Notebook {
        #[command(subcommand)]
        command: NotebookCommands,
    },
    /// Serve a sema-web app in the browser with a native LLM proxy
    Web {
        /// Path to the app's entry `.sema` file
        file: String,
        /// Host to bind. Loopback by default; a non-loopback host exposes the
        /// unauthenticated LLM proxy to the network.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to listen on (advances to the next free port if taken)
        #[arg(short, long, default_value = "3000")]
        port: u16,
        /// Don't open a browser automatically
        #[arg(long)]
        no_open: bool,
        /// Disable the built-in LLM proxy
        #[arg(long)]
        no_llm: bool,
    },
    /// Run journaled workflows and view their runs
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommands,
    },
    /// Evaluate an expression or program and print the result
    Eval {
        /// Read program from stdin instead of --expr
        #[arg(long)]
        stdin: bool,

        /// Expression to evaluate (alternative to --stdin)
        #[arg(long)]
        expr: Option<String>,

        /// Emit machine-readable JSON result envelope
        #[arg(long)]
        json: bool,

        /// Set file path for error spans and relative import resolution
        #[arg(long)]
        path: Option<String>,

        /// Kill evaluation after N milliseconds (default: 5000)
        #[arg(long, default_value = "5000")]
        timeout: u64,

        /// Sandbox mode (e.g., "strict", "all", or comma-separated capabilities)
        #[arg(long)]
        sandbox: Option<String>,

        /// Disable LLM features
        #[arg(long)]
        no_llm: bool,
    },
    /// Update sema itself to the latest released version
    Update {
        /// Check for an available update without installing it
        #[arg(long)]
        check: bool,

        /// Install a specific version instead of the latest (e.g. "1.30.0")
        #[arg(long)]
        version: Option<String>,

        /// Skip the confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum DocCommands {
    /// Show documentation for a symbol
    Show {
        /// Symbol to show documentation for
        symbol: String,
    },
    /// Search documentation by natural-language query
    Search {
        /// Query to search for
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,

        /// Maximum number of results to show
        #[arg(short = 'n', long, default_value_t = sema_mcp::docs_search::DEFAULT_LIMIT)]
        limit: usize,
    },
    /// Search symbol names by prefix, substring, and fuzzy match
    Apropos {
        /// Pattern to search for
        pattern: String,
    },
}

#[derive(Subcommand)]
enum McpAuthCommands {
    /// Log in to a remote (HTTP) MCP server and cache the OAuth token
    Login {
        /// The MCP server URL (e.g. https://mcp.example.com/mcp)
        url: String,
        /// Use the device-authorization flow instead of opening a browser
        #[arg(long, conflicts_with = "token")]
        device: bool,
        /// A pre-registered OAuth client id (when the server has no dynamic registration)
        #[arg(long = "client-id", value_name = "ID")]
        client_id: Option<String>,
        /// Store a pre-issued access token directly, skipping discovery/DCR/OAuth
        /// entirely — the headless/CI escape hatch (no browser, no device flow).
        #[arg(long, conflicts_with = "device", value_name = "TOKEN")]
        token: Option<String>,
        /// Seconds until the pre-issued --token expires (omit for a non-expiring token)
        #[arg(long, requires = "token", value_name = "SECS")]
        expires_in: Option<u64>,
    },
    /// Remove cached credentials for a remote MCP server
    Logout {
        /// The MCP server URL whose cached credentials to clear
        url: String,
    },
}

#[derive(Subcommand)]
enum PkgCommands {
    /// Add a package (from registry or git URL)
    Add {
        /// Package name or URL, optionally with @version (e.g., http-helpers@1.0.0 or github.com/user/repo@v1.0)
        spec: String,

        /// Registry URL override
        #[arg(long)]
        registry: Option<String>,
    },
    /// Install all dependencies from sema.toml
    Install {
        /// Fail if sema.lock is missing or out of sync (for CI)
        #[arg(long)]
        locked: bool,
    },
    /// Update a package (or all packages)
    Update {
        /// Package name to update (updates all if omitted)
        name: Option<String>,
    },
    /// Remove an installed package
    Remove {
        /// Package URL or name
        name: String,
    },
    /// List installed packages
    List,
    /// Initialize a new sema.toml in the current directory
    Init,
    /// Authenticate with a package registry
    Login {
        /// API token (from registry account page)
        #[arg(long)]
        token: Option<String>,

        /// Registry URL (default: https://pkg.sema-lang.com)
        #[arg(long, default_value = "https://pkg.sema-lang.com")]
        registry: String,
    },
    /// Remove stored registry credentials
    Logout,
    /// View or set package manager configuration
    Config {
        /// Config key (e.g., registry.url). Omit to show all config
        key: Option<String>,

        /// Value to set. Omit to read the current value
        value: Option<String>,
    },
    /// Publish current package to the registry
    Publish {
        /// Registry URL override
        #[arg(long)]
        registry: Option<String>,
    },
    /// Search the registry for packages
    Search {
        /// Search query
        query: String,

        /// Registry URL override
        #[arg(long)]
        registry: Option<String>,
    },
    /// Yank a published version (prevent new installs)
    Yank {
        /// Package@version to yank (e.g., my-package@0.1.0)
        spec: String,

        /// Registry URL override
        #[arg(long)]
        registry: Option<String>,
    },
    /// Show package info from the registry
    Info {
        /// Package name
        name: String,

        /// Registry URL override
        #[arg(long)]
        registry: Option<String>,
    },
}

#[derive(Subcommand)]
enum WorkflowCommands {
    /// Run a workflow file (a `.sema` program that `defworkflow`s and runs it),
    /// journaling a frozen run-directory and writing `result.json`.
    ///
    /// Exit codes: 0 success, 1 failed, 2 the run needs MCP authentication (see
    /// stderr for which server(s) and the `sema mcp login` command to run, then
    /// re-run this workflow). On an interactive terminal, a needs-auth gate logs
    /// in inline (the browser/loopback flow) instead of exiting 2; `--no-auth-prompt`
    /// forces the headless exit-2 behavior even on a TTY.
    Run {
        /// Path to the `.sema` workflow file.
        file: String,

        /// JSON object bound to the global `*workflow-args*` for the run.
        #[arg(long, default_value = "{}")]
        args: String,

        /// Base directory for run journals; the run lands in `<run-dir>/<run-id>/`.
        /// Defaults to the project-local `.sema/runs`.
        #[arg(long, default_value = ".sema/runs")]
        run_dir: String,

        /// Also start the live web viewer and keep it open after the run, so you
        /// can watch the run progress and inspect it afterwards.
        #[arg(long)]
        view: bool,

        /// Port for the `--view` viewer.
        #[arg(short, long, default_value = "8899")]
        port: u16,

        /// Resume a prior run by its run-id: reuse `<run-dir>/<run-id>/`, skip leaves
        /// already recorded in its `memo/` dir (no re-call of the model), and write a
        /// fresh `events.resume-N.jsonl` segment. A workflow edit changes the code
        /// version and re-runs everything.
        #[arg(long)]
        resume: Option<String>,

        /// Never log in inline on a needs-auth gate, even on an interactive
        /// terminal — always exit 2 with `sema mcp login` guidance instead. No
        /// effect when running headlessly (no TTY, or `CI` set): that already
        /// gets the exit-2 behavior.
        #[arg(long)]
        no_auth_prompt: bool,
    },
    /// Backfill the cross-run SQLite index (`<run-dir>/index.db`) from every run's
    /// journal — for offline/CI use; the viewer also syncs lazily on request.
    Index {
        /// Base directory holding `<run-id>/events.jsonl` run journals.
        #[arg(long, default_value = ".sema/runs")]
        run_dir: String,
    },
    /// Open the web viewer for a run directory's workflow journals
    View {
        /// Base directory holding `<run-id>/events.jsonl` run journals.
        #[arg(long, default_value = ".sema/runs")]
        run_dir: String,

        /// Host to bind. Defaults to loopback; binding elsewhere exposes the run
        /// directory to the network (the viewer has no auth).
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port to listen on.
        #[arg(short, long, default_value = "8899")]
        port: u16,
    },
    /// Statically validate a workflow `.sema` file WITHOUT evaluating it or calling any LLM
    /// — catches arity traps, bad step opts, and layout issues before a run.
    Check {
        /// Path to the `.sema` workflow file.
        file: String,

        /// Treat warnings as errors (exit non-zero if any warning fires).
        #[arg(long)]
        strict: bool,

        /// Emit machine-readable JSON diagnostics instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
#[command(subcommand_required = true, arg_required_else_help = true)]
enum NotebookCommands {
    /// Start the notebook server with browser UI
    Serve {
        /// Path to .sema-nb file (created if absent)
        file: Option<String>,

        /// Host address to bind to. Defaults to loopback (127.0.0.1); the
        /// notebook server has no auth layer, so binding to a non-loopback
        /// address exposes unauthenticated code execution to the network.
        #[arg(long, default_value = sema_notebook::server::DEFAULT_HOST)]
        host: String,

        /// Port to listen on
        #[arg(short, long, default_value = "8888")]
        port: u16,
    },
    /// Run all cells in a notebook headlessly
    Run {
        /// Path to .sema-nb file
        file: String,

        /// Only run specific cells (1-based, comma-separated)
        #[arg(long)]
        cells: Option<String>,
    },
    /// Export a notebook to Markdown
    Export {
        /// Path to .sema-nb file
        file: String,

        /// Output format
        #[arg(long, default_value = "md")]
        format: String,

        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Create a new empty notebook
    New {
        /// Path to create the .sema-nb file
        file: String,

        /// Notebook title
        #[arg(short, long)]
        title: Option<String>,
    },
}

/// Build the standard CLI interpreter: stdlib + LLM (registered inside sema-eval)
/// plus the MCP *client* builtins (`mcp/connect`, `mcp/tools`, `mcp/tools->sema`,
/// …). The MCP builtins live in `sema-mcp`, which depends on `sema-eval`, so they
/// can't be registered inside `sema-eval` itself — the binary wires them in here.
/// The real `WorkflowMcpResolver` (`sema::workflow_mcp`) is registered right
/// alongside them, so every CLI path built through this function (REPL, `sema
/// run`, `sema workflow run`, …) can resolve a workflow's declared `:mcp`
/// servers — see docs/plans/2026-06-24-workflow-mcp-auth.md §3/§9(a).
fn build_interpreter(sandbox: &sema_core::Sandbox) -> Interpreter {
    let interpreter = Interpreter::new_with_sandbox(sandbox);
    sema_mcp::register_mcp_builtins(&interpreter.global_env, sandbox);
    sema::workflow_mcp::register_real_resolver();
    interpreter
}

fn main() {
    // Check for embedded archive before parsing CLI args
    if let Some(exit_code) = try_run_embedded() {
        std::process::exit(exit_code);
    }

    // Shell-completion helper for `sema doc` symbols, handled before clap parses.
    // It is intentionally NOT a clap subcommand: a hidden subcommand makes
    // `clap_complete`'s bash generator panic (find_subcommand_with_path), which
    // would break `sema completions bash`. The generated completion scripts still
    // invoke `sema __complete-doc-symbols <prefix>`.
    {
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() == Some("__complete-doc-symbols") {
            let prefix = args.next().unwrap_or_default();
            for name in docs::completion_candidates(&prefix) {
                println!("{name}");
            }
            return;
        }
    }

    let cli = Cli::parse();

    // Opt-in OpenTelemetry: installs a provider only when SEMA_OTEL_FILE or an OTLP
    // endpoint is configured (zero-cost no-op otherwise). Held for the process
    // lifetime; its Drop does the bounded flush+shutdown on normal return. (The JSONL
    // file exporter writes synchronously, so it survives a `std::process::exit` too.)
    let _otel_guard = sema_otel::init_from_env();

    let sandbox = match &cli.sandbox {
        Some(value) => sema_core::Sandbox::parse_cli(value).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }),
        None => sema_core::Sandbox::allow_all(),
    };
    let sandbox = match &cli.allowed_paths {
        Some(value) => {
            let paths = sema_core::Sandbox::parse_allowed_paths(value);
            sandbox.with_allowed_paths(paths)
        }
        None => sandbox,
    };

    // Handle subcommands
    if let Some(command) = cli.command {
        match command {
            Commands::Ast { file, eval, json } => {
                run_ast(file, eval, json);
            }
            Commands::Completions { shell, install } => {
                if install {
                    install_completions(shell);
                } else {
                    print!("{}", generate_completions(shell));
                }
            }
            Commands::Compile {
                file,
                output,
                check,
            } => {
                if check {
                    run_check(&file);
                } else {
                    run_compile(&file, output.as_deref());
                }
            }
            Commands::Disasm { file, json } => {
                run_disasm(&file, json);
            }
            Commands::Doc {
                pager,
                no_pager,
                command,
                symbol,
            } => {
                let pager = if no_pager {
                    docs::PagerMode::Never
                } else if pager {
                    docs::PagerMode::Always
                } else {
                    docs::PagerMode::Auto
                };
                if let Err(msg) = run_doc(command, symbol, pager) {
                    eprintln!("Error: {msg}");
                    std::process::exit(1);
                }
            }
            Commands::Pkg { command } => {
                let result = match command {
                    PkgCommands::Add { spec, registry } => pkg::cmd_add(&spec, registry.as_deref()),
                    PkgCommands::Install { locked } => pkg::cmd_install(locked),
                    PkgCommands::Update { name } => pkg::cmd_update(name.as_deref()),
                    PkgCommands::Remove { name } => pkg::cmd_remove(&name),
                    PkgCommands::List => pkg::cmd_list(),
                    PkgCommands::Init => pkg::cmd_init(),
                    PkgCommands::Login { token, registry } => {
                        pkg::cmd_login(token.as_deref(), &registry)
                    }
                    PkgCommands::Logout => pkg::cmd_logout(),
                    PkgCommands::Config { key, value } => {
                        pkg::cmd_config(key.as_deref(), value.as_deref())
                    }
                    PkgCommands::Publish { registry } => pkg::cmd_publish(registry.as_deref()),
                    PkgCommands::Search { query, registry } => {
                        pkg::cmd_search(&query, registry.as_deref())
                    }
                    PkgCommands::Yank { spec, registry } => {
                        pkg::cmd_yank(&spec, registry.as_deref())
                    }
                    PkgCommands::Info { name, registry } => {
                        pkg::cmd_info(&name, registry.as_deref())
                    }
                };
                if let Err(e) = result {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
            Commands::Build {
                file,
                output,
                includes,
                runtime,
                target,
                list_targets,
                no_cache,
            } => {
                if list_targets {
                    cross_compile::list_targets();
                    return;
                }
                let file = file.expect("file is required unless --list-targets");
                if let Err(e) = run_build(
                    &file,
                    output.as_deref(),
                    &includes,
                    runtime.as_deref(),
                    target.as_deref(),
                    no_cache,
                ) {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
            Commands::Fmt {
                files,
                check,
                diff,
                width,
                indent,
                align,
                json,
            } => {
                let config = find_config().unwrap_or_default();
                let opts = sema_fmt::FormatOptions {
                    width: width.unwrap_or(config.fmt.width),
                    indent: indent.unwrap_or(config.fmt.indent),
                    align: align || config.fmt.align,
                };
                run_fmt(&files, check, diff, &opts, json);
            }
            Commands::Lsp => {
                eprintln!("Sema LSP server starting on stdio...");
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create tokio runtime")
                    .block_on(sema_lsp::run_server());
            }
            Commands::Dap => {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create tokio runtime")
                    .block_on(sema_dap::run_server());
            }
            Commands::Mcp {
                auth,
                files,
                include,
                exclude,
            } => {
                if let Some(auth) = auth {
                    let result = match auth {
                        McpAuthCommands::Login {
                            url,
                            token: Some(token),
                            expires_in,
                            ..
                        } => sema_mcp::mcp_login_token(&url, &token, expires_in),
                        McpAuthCommands::Login {
                            url,
                            device,
                            client_id,
                            token: None,
                            ..
                        } => sema_mcp::mcp_login(&url, device, client_id.as_deref()),
                        McpAuthCommands::Logout { url } => sema_mcp::mcp_logout(&url),
                    };
                    if let Err(e) = result {
                        eprintln!("mcp: {e}");
                        std::process::exit(1);
                    }
                    return;
                }
                let inc_tools = include.map(|s| {
                    s.split(',')
                        .map(|x| x.trim().to_string())
                        .collect::<Vec<String>>()
                });
                let exc_tools = exclude.map(|s| {
                    s.split(',')
                        .map(|x| x.trim().to_string())
                        .collect::<Vec<String>>()
                });

                let sandbox = sema_core::Sandbox::allow_all();
                let interpreter = build_interpreter(&sandbox);

                let _ = interpreter.eval_str("(llm/auto-configure)");

                for file in files {
                    match read_source_file(&file) {
                        Ok(content) => {
                            if let Err(e) = interpreter.eval_str_compiled(&content) {
                                eprintln!("Error loading tool file {file}: {e}");
                                std::process::exit(1);
                            }
                        }
                        Err(e) => {
                            eprintln!("Error reading tool file {file}: {e}");
                            std::process::exit(1);
                        }
                    }
                }

                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("Failed to create tokio runtime")
                    .block_on(async {
                        if let Err(e) =
                            sema_mcp::run_mcp_server(interpreter, inc_tools, exc_tools).await
                        {
                            eprintln!("MCP server error: {e}");
                            std::process::exit(1);
                        }
                    });
            }
            Commands::Notebook { command } => {
                run_notebook_command(command);
            }
            Commands::Web {
                file,
                host,
                port,
                no_open,
                no_llm,
            } => {
                if let Err(e) = web::run(&file, &host, port, !no_open, !no_llm) {
                    eprintln!("sema web: {e}");
                    std::process::exit(1);
                }
            }
            Commands::Workflow { command } => {
                run_workflow_command(command, &sandbox);
            }
            Commands::Eval {
                stdin,
                expr,
                json,
                path,
                timeout: _timeout,
                sandbox,
                no_llm,
            } => {
                run_eval(stdin, expr, json, path, sandbox, no_llm);
            }
            Commands::Update {
                check,
                version,
                yes,
            } => {
                let opts = update::UpdateOptions {
                    check_only: check,
                    target_version: version,
                    yes,
                };
                if let Err(e) = update::run(opts) {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
        return;
    }

    let interpreter = build_interpreter(&sandbox);

    // Set LLM env vars before auto-configure
    if let Some(model) = cli.chat_model.as_ref() {
        std::env::set_var("SEMA_CHAT_MODEL", model);
    }
    if let Some(provider) = cli.chat_provider.as_ref() {
        std::env::set_var("SEMA_CHAT_PROVIDER", provider);
    }
    if let Some(model) = &cli.embedding_model {
        std::env::set_var("SEMA_EMBEDDING_MODEL", model);
    }
    if let Some(provider) = &cli.embedding_provider {
        std::env::set_var("SEMA_EMBEDDING_PROVIDER", provider);
    }

    // Auto-configure LLM unless --no-llm
    if !cli.no_llm {
        if let Err(e) = interpreter.eval_str("(llm/auto-configure)") {
            if cli.chat_provider.is_some() || cli.chat_model.is_some() {
                print_error(&e);
                std::process::exit(1);
            }
        }
    }

    // Load files first (in order)
    for load_file in &cli.load {
        let path = std::path::Path::new(load_file);
        if let Ok(canonical) = path.canonicalize() {
            interpreter.ctx.push_file_path(canonical);
        }
        match read_source_file(load_file) {
            Ok(content) => {
                LAST_SOURCE.with(|s| *s.borrow_mut() = Some(content.clone()));
                LAST_FILE.with(|f| *f.borrow_mut() = Some(PathBuf::from(load_file)));
                match interpreter.eval_str_compiled(&content) {
                    Ok(_) => {
                        interpreter.ctx.pop_file_path();
                        drain_async_scheduler(&interpreter);
                    }
                    Err(e) => {
                        interpreter.ctx.pop_file_path();
                        eprint!("Error loading {load_file}: ");
                        print_error(&e);
                        std::process::exit(1);
                    }
                }
            }
            Err(msg) => {
                eprintln!("error: {msg}");
                std::process::exit(1);
            }
        }
    }

    // Handle --eval
    if let Some(expr) = &cli.eval {
        LAST_SOURCE.with(|s| *s.borrow_mut() = Some(expr.clone()));
        LAST_FILE.with(|f| *f.borrow_mut() = None);
        match interpreter.eval_str_compiled(expr) {
            Ok(val) => {
                drain_async_scheduler(&interpreter);
                if !val.is_nil() {
                    println!("{}", pretty_print(&val, 80));
                }
            }
            Err(e) => {
                print_error(&e);
                std::process::exit(1);
            }
        }
        if cli.interactive {
            repl::run(interpreter, cli.quiet, cli.sandbox.as_deref());
        }
        return;
    }

    // Handle --print
    if let Some(expr) = &cli.print {
        LAST_SOURCE.with(|s| *s.borrow_mut() = Some(expr.clone()));
        LAST_FILE.with(|f| *f.borrow_mut() = None);
        match interpreter.eval_str_compiled(expr) {
            Ok(val) => {
                drain_async_scheduler(&interpreter);
                println!("{val}");
            }
            Err(e) => {
                print_error(&e);
                std::process::exit(1);
            }
        }
        if cli.interactive {
            repl::run(interpreter, cli.quiet, cli.sandbox.as_deref());
        }
        return;
    }

    // Handle FILE
    if let Some(file) = &cli.file {
        let path = std::path::Path::new(file);

        // Auto-detect .semac bytecode files
        if let Ok(bytes) = std::fs::read(path) {
            if sema_vm::is_bytecode_file(&bytes) {
                match run_bytecode_bytes(&interpreter, &bytes) {
                    Ok(_) => {
                        drain_async_scheduler(&interpreter);
                    }
                    Err(e) => {
                        print_error(&e);
                        std::process::exit(1);
                    }
                }
                if cli.interactive {
                    repl::run(interpreter, cli.quiet, cli.sandbox.as_deref());
                }
                return;
            }
        }

        if let Ok(canonical) = path.canonicalize() {
            interpreter.ctx.push_file_path(canonical);
        }
        match read_source_file(file) {
            Ok(content) => {
                LAST_SOURCE.with(|s| *s.borrow_mut() = Some(content.clone()));
                LAST_FILE.with(|f| *f.borrow_mut() = Some(PathBuf::from(file)));
                match interpreter.eval_str_compiled(&content) {
                    Ok(_) => {
                        interpreter.ctx.pop_file_path();
                        drain_async_scheduler(&interpreter);
                    }
                    Err(e) => {
                        interpreter.ctx.pop_file_path();
                        print_error(&e);
                        std::process::exit(1);
                    }
                }
            }
            Err(msg) if msg.starts_with("file not found:") => {
                eprintln!("error: file not found: '{file}' (not a file or command)\n\nRun 'sema --help' for available commands.");
                std::process::exit(1);
            }
            Err(msg) => {
                eprintln!("error: {msg}");
                std::process::exit(1);
            }
        }
        if cli.interactive {
            repl::run(interpreter, cli.quiet, cli.sandbox.as_deref());
        }
        return;
    }

    // REPL mode
    repl::run(interpreter, cli.quiet, cli.sandbox.as_deref());
}

/// `sema workflow run <file>` — evaluate a workflow `.sema` file (which
/// `defworkflow`s and runs it) with the run-directory + args seams wired, then
/// exit non-zero if the run's `{:status …}` envelope reports failure.
fn run_workflow_command(command: WorkflowCommands, sandbox: &sema_core::Sandbox) {
    let (file, args, run_dir, view, view_port, resume, no_auth_prompt) = match command {
        WorkflowCommands::Run {
            file,
            args,
            run_dir,
            view,
            port,
            resume,
            no_auth_prompt,
        } => (file, args, run_dir, view, port, resume, no_auth_prompt),
        WorkflowCommands::View {
            run_dir,
            host,
            port,
        } => {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime")
                .block_on(workflow_view::serve(PathBuf::from(run_dir), &host, port));
            return;
        }
        WorkflowCommands::Index { run_dir } => {
            let root = PathBuf::from(&run_dir);
            match workflow_view::ingest::open(&root.join(sema_workflow::INDEX_DB)) {
                Ok(conn) => {
                    workflow_view::ingest::backfill_all(&conn, &root);
                    match workflow_view::ingest::runs_summary(&conn) {
                        Ok(rows) => println!(
                            "indexed {} run(s) → {}",
                            rows.len(),
                            root.join(sema_workflow::INDEX_DB).display()
                        ),
                        Err(e) => eprintln!("warning: index summary: {e}"),
                    }
                }
                Err(e) => {
                    eprintln!("error: cannot open index db: {e}");
                    std::process::exit(1);
                }
            }
            return;
        }
        WorkflowCommands::Check { file, strict, json } => {
            let src = match read_source_file(&file) {
                Ok(s) => s,
                Err(msg) => {
                    eprintln!("error: {msg}");
                    std::process::exit(2);
                }
            };
            let diags = workflow_check::check_source(&src);
            std::process::exit(workflow_check::report(&file, &diags, strict, json));
        }
    };

    // Interactive MCP auth (docs/plans/2026-06-24-workflow-mcp-auth.md §3): on a
    // real terminal, a needs-auth gate logs in inline instead of exiting 2. See
    // `should_enable_interactive_auth` for the exact decision and
    // `sema::workflow_mcp::set_interactive_auth` for what enabling it does.
    sema::workflow_mcp::set_interactive_auth(should_enable_interactive_auth(
        std::io::stdin().is_terminal(),
        std::io::stderr().is_terminal(),
        std::env::var("CI").ok().as_deref(),
        no_auth_prompt,
    ));

    // The workflow runtime (sema-workflow) reads this seam to choose the run-dir
    // base; the run lands in `<run-dir>/<run-id>/`.
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &run_dir);

    // `--resume <run-id>`: reuse that run's dir + memo cache. Sanitize the operator-
    // supplied id against path traversal (it joins into a filesystem path), require the
    // prior run's events.jsonl to exist, then set the seams the runtime reads.
    if let Some(run_id) = &resume {
        if run_id.is_empty()
            || run_id.contains('/')
            || run_id.contains('\\')
            || run_id.contains("..")
        {
            eprintln!("error: --resume run-id must be a bare directory name (no path separators)");
            std::process::exit(1);
        }
        let prior = PathBuf::from(&run_dir).join(run_id).join("events.jsonl");
        if !prior.exists() {
            eprintln!("error: no prior run to resume at {}", prior.display());
            std::process::exit(1);
        }
        std::env::set_var("SEMA_WORKFLOW_RUN_ID", run_id);
        std::env::set_var("SEMA_WORKFLOW_RESUME", "1");
    }
    // Recorded verbatim on the run.started event (shown in the viewer's stream/meta).
    std::env::set_var("SEMA_WORKFLOW_ARGS_JSON", &args);

    let content = match read_source_file(&file) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("error: {msg}");
            std::process::exit(1);
        }
    };

    let mut effective_sandbox = sandbox.clone();
    let permission_specs = match workflow_check::declared_permission_specs(&content) {
        Ok(specs) => specs,
        Err(e) => {
            eprintln!("error: invalid workflow permissions: {e}");
            std::process::exit(1);
        }
    };
    for spec in permission_specs {
        let declared = sema_core::Sandbox::parse_cli(&spec).unwrap_or_else(|e| {
            eprintln!("error: invalid defworkflow :permissions {spec:?}: {e}");
            std::process::exit(1);
        });
        effective_sandbox = effective_sandbox.with_more_denied(declared.denied);
    }

    // `--view`: start the live viewer on a background thread BEFORE the run, so the
    // journal (written flush-per-event) is watchable in real time, and keep it up
    // afterwards for inspection. A bind failure degrades to a warning (the run still
    // proceeds). Best-effort open the browser.
    if view {
        let vd = run_dir.clone();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime")
                .block_on(async {
                    if let Err(e) = workflow_view::serve_result(
                        PathBuf::from(vd),
                        "127.0.0.1",
                        view_port,
                        false,
                    )
                    .await
                    {
                        eprintln!("warning: --view could not start the viewer: {e}");
                    }
                });
        });
        let url = format!("http://127.0.0.1:{view_port}");
        println!("Live viewer: {url}");
        open_in_browser(&url);
        // Give the listener a moment to bind before the run starts producing events.
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    let interpreter = build_interpreter(&effective_sandbox);

    // Auto-configure an LLM provider from the environment (mirrors the default run
    // path), so a workflow whose leaves call `llm/*` works without self-configuring.
    // Best-effort: a workflow with no LLM leaves needs no provider, so ignore errors.
    let _ = interpreter.eval_str("(llm/auto-configure)");

    // Bind the parsed --args JSON object to the global `*workflow-args*` so the
    // workflow body can read its inputs.
    let args_value = match serde_json::from_str::<serde_json::Value>(&args) {
        Ok(json) => sema_core::json::json_to_value(&json),
        Err(e) => {
            eprintln!("error: --args is not valid JSON: {e}");
            std::process::exit(1);
        }
    };
    interpreter
        .global_env
        .set(sema_core::intern("*workflow-args*"), args_value);

    // Code version: a deterministic hash of the source, folded into every resume
    // content-key. Editing the workflow changes this ⇒ memos no longer match ⇒ a
    // resumed run re-executes from scratch (correct invalidation). DefaultHasher uses
    // fixed keys, so the value is stable across separate invocations of this binary.
    {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut h);
        std::env::set_var("SEMA_WORKFLOW_CODE_VERSION", format!("{:016x}", h.finish()));
    }

    // The file's last form is the `defworkflow` (which expands to `workflow/run`),
    // so eval returns the `{:status …}` envelope; journaling is its side effect.
    let exit_code = match interpreter.eval_str_compiled(&content) {
        Ok(envelope) => {
            drain_async_scheduler(&interpreter);
            let status = envelope
                .as_map_rc()
                .and_then(|m| m.get(&Value::keyword("status")).cloned())
                .and_then(|s| s.as_keyword());
            match status.as_deref() {
                Some("failed") => {
                    eprintln!("workflow failed: {}", pretty_print(&envelope, 80));
                    1
                }
                // The headless-precursor gate (docs/plans/2026-06-24-workflow-mcp-auth.md
                // §3/§5): a declared `:mcp` server had no usable session. Distinct exit
                // code so a CI/orchestrator script can branch on "needs a human to log
                // in" vs. a genuine failure.
                Some("needs-auth") => {
                    eprint!("{}", format_needs_auth_guidance(&envelope));
                    2
                }
                _ => 0,
            }
        }
        Err(e) => {
            eprint!("Error running workflow {file}: ");
            print_error(&e);
            1
        }
    };

    // With `--view`, keep the viewer up so the finished run can be inspected.
    if view {
        println!("\nRun complete — viewer live at http://127.0.0.1:{view_port}  (Ctrl-C to stop)");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    std::process::exit(exit_code);
}

/// Render the stderr guidance for a `{:status :needs-auth :auth [{:server :url
/// :persist} …]}` run envelope: terse, terminal-quiet (no banners), one `sema mcp
/// login` line per server, aliases column-aligned. A malformed/missing `:auth`
/// vector degrades to a 0-server header rather than panicking — the exit code
/// alone (2) is still meaningful to a script even if the guidance text is thin.
fn format_needs_auth_guidance(envelope: &Value) -> String {
    let entries: Vec<(String, String)> = envelope
        .as_map_rc()
        .and_then(|m| m.get(&Value::keyword("auth")).cloned())
        .and_then(|a| a.as_list_rc().or_else(|| a.as_vector_rc()))
        .map(|list| {
            list.iter()
                .filter_map(|entry| {
                    let m = entry.as_map_rc()?;
                    let server = m.get(&Value::keyword("server"))?.as_str()?.to_string();
                    let url = m.get(&Value::keyword("url"))?.as_str()?.to_string();
                    Some((server, url))
                })
                .collect()
        })
        .unwrap_or_default();

    let width = entries.iter().map(|(s, _)| s.len()).max().unwrap_or(0);
    let mut out = format!(
        "run needs authentication for {} MCP server(s):\n",
        entries.len()
    );
    for (server, url) in &entries {
        out.push_str(&format!("  {server:<width$}  sema mcp login {url}\n"));
    }
    out.push_str("then re-run this workflow. (or authenticate from `sema workflow view`)\n");
    out
}

/// Whether `run_workflow_command` should enable inline interactive MCP auth
/// (`sema::workflow_mcp::set_interactive_auth`) for this run: a needs-auth
/// gate logs in right there instead of exiting 2. Pure over its inputs — no
/// direct `IsTerminal`/`env::var` calls here — so this is unit-testable
/// without a real TTY; the caller supplies `std::io::stdin().is_terminal()`,
/// `std::io::stderr().is_terminal()`, `std::env::var("CI").ok()`, and the
/// `--no-auth-prompt` flag.
///
/// Both stdin AND stderr must be TTYs: stdin implies a human is actually at
/// the keyboard to complete a browser (or, if this run were headless, a
/// device-code) flow; stderr is where the "opening browser…" line and any
/// failure reason land, so it must be a place a human will actually see them
/// — an interactive stdin with redirected stderr (e.g. `sema workflow run x
/// 2>log.txt`) is exactly the case that should NOT pop a browser unannounced.
/// `--no-auth-prompt` and a non-empty `CI` both force the headless path
/// unconditionally, regardless of the TTY checks.
fn should_enable_interactive_auth(
    stdin_is_tty: bool,
    stderr_is_tty: bool,
    ci_env: Option<&str>,
    no_auth_prompt: bool,
) -> bool {
    if no_auth_prompt {
        return false;
    }
    if ci_env.is_some_and(|v| !v.is_empty()) {
        return false;
    }
    stdin_is_tty && stderr_is_tty
}

/// Best-effort: open `url` in the default browser via the platform opener. Silent
/// no-op if the opener isn't present (e.g. headless) — the URL is always printed.
fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Drain any pending async tasks scheduled by a top-level form.
///
/// Top-level `(async ...)` forms spawn a task but don't implicitly await it,
/// so their side effects would silently vanish on exit unless we explicitly
/// run the scheduler. This drains all pending tasks (target = `All`).
///
/// The scheduler callback is only registered once an eval has run, so we
/// silently ignore the "no async scheduler registered" error (nothing async was
/// scheduled). Other scheduler errors are reported to stderr as warnings but do
/// not fail the program — the side effects already ran.
pub(crate) fn drain_async_scheduler(interpreter: &Interpreter) {
    if let Err(e) = sema_core::call_run_scheduler(&interpreter.ctx, None) {
        let msg = e.to_string();
        if msg.contains("no async scheduler registered") {
            return;
        }
        eprintln!("warning: background task error: {msg}");
    }
}

fn run_notebook_command(command: NotebookCommands) {
    match command {
        NotebookCommands::Serve { file, host, port } => {
            let path = file.map(std::path::PathBuf::from);
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime")
                .block_on(sema_notebook::serve(path, &host, port));
        }
        NotebookCommands::Run { file, cells } => {
            let path = std::path::Path::new(&file);
            let mut engine = match sema_notebook::Engine::from_file(path) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            };

            // Collect the code cell IDs to evaluate, either specific indices
            // (--cells 1,3,5) or all code cells.
            let cell_ids: Vec<String> = if let Some(cell_spec) = &cells {
                let indices: Vec<usize> = cell_spec
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
                engine
                    .notebook
                    .cells
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| {
                        if indices.contains(&(i + 1))
                            && c.cell_type == sema_notebook::format::CellType::Code
                        {
                            Some(c.id.clone())
                        } else {
                            None
                        }
                    })
                    .collect()
            } else {
                engine
                    .notebook
                    .cells
                    .iter()
                    .filter(|c| c.cell_type == sema_notebook::format::CellType::Code)
                    .map(|c| c.id.clone())
                    .collect()
            };

            let total = cell_ids.len();
            let mut had_error = false;

            for (i, id) in cell_ids.into_iter().enumerate() {
                match engine.eval_cell(&id) {
                    Ok(r) => {
                        if !r.stdout.is_empty() {
                            print!("[{}/{}] (stdout) {}", i + 1, total, r.stdout);
                        }
                        if !r.output.display.is_empty() {
                            println!("[{}/{}] {}", i + 1, total, r.output.display);
                        }
                        if r.output.output_type == sema_notebook::format::OutputType::Error {
                            had_error = true;
                        }
                    }
                    Err(e) => {
                        eprintln!("[{}/{}] Error: {e}", i + 1, total);
                        had_error = true;
                    }
                }
            }

            // Save updated outputs back to the file
            if let Err(e) = engine.notebook.save(path) {
                eprintln!("Warning: failed to save: {e}");
            }

            if had_error {
                std::process::exit(1);
            }
        }
        NotebookCommands::Export {
            file,
            format,
            output,
        } => {
            let path = std::path::Path::new(&file);
            let notebook = match sema_notebook::Notebook::load(path) {
                Ok(nb) => nb,
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            };

            let content = match format.as_str() {
                "md" | "markdown" => sema_notebook::render::export_markdown(&notebook),
                other => {
                    eprintln!("Unknown export format: {other}. Supported: md");
                    std::process::exit(1);
                }
            };

            match output {
                Some(out_path) => {
                    if let Err(e) = std::fs::write(&out_path, &content) {
                        eprintln!("Error writing {out_path}: {e}");
                        std::process::exit(1);
                    }
                    eprintln!("Exported to {out_path}");
                }
                None => print!("{content}"),
            }
        }
        NotebookCommands::New { file, title } => {
            let path = std::path::Path::new(&file);
            let title = title.as_deref().unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Untitled")
            });
            let mut notebook = sema_notebook::Notebook::new(title);
            // Add a starter code cell
            notebook.add_code_cell("; Welcome to your Sema notebook!\n(+ 1 2)");
            if let Err(e) = notebook.save(path) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
            eprintln!("Created notebook: {file}");
        }
    }
}

fn run_eval(
    use_stdin: bool,
    expr: Option<String>,
    json: bool,
    path: Option<String>,
    sandbox_arg: Option<String>,
    no_llm: bool,
) {
    // Get the program text
    let program = if use_stdin {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .unwrap_or_else(|e| {
                if json {
                    print_eval_json(&EvalJsonResult {
                        ok: false,
                        value: None,
                        stdout: "",
                        stderr: "",
                        error_msg: None,
                        error_hint: None,
                        error_line: None,
                        error_col: None,
                        elapsed_ms: 0,
                    });
                } else {
                    eprintln!("Error reading stdin: {e}");
                }
                std::process::exit(1);
            });
        buf
    } else if let Some(e) = expr {
        e
    } else {
        if json {
            print_eval_json(&EvalJsonResult {
                ok: false,
                value: None,
                stdout: "",
                stderr: "",
                error_msg: Some("Either --stdin or --expr is required"),
                error_hint: None,
                error_line: None,
                error_col: None,
                elapsed_ms: 0,
            });
        } else {
            eprintln!("Error: either --stdin or --expr is required");
        }
        std::process::exit(1);
    };

    // Set up sandbox
    let sandbox = match &sandbox_arg {
        Some(value) => sema_core::Sandbox::parse_cli(value).unwrap_or_else(|e| {
            if json {
                print_eval_json(&EvalJsonResult {
                    ok: false,
                    value: None,
                    stdout: "",
                    stderr: "",
                    error_msg: Some(&format!("Invalid sandbox: {e}")),
                    error_hint: None,
                    error_line: None,
                    error_col: None,
                    elapsed_ms: 0,
                });
            } else {
                eprintln!("Error: {e}");
            }
            std::process::exit(1);
        }),
        None => sema_core::Sandbox::allow_all(),
    };

    let interpreter = build_interpreter(&sandbox);

    // Auto-configure LLM unless --no-llm
    if !no_llm {
        let _ = interpreter.eval_str("(llm/auto-configure)");
    }

    // Set file path for import resolution
    if let Some(ref p) = path {
        let file_path = std::path::Path::new(p);
        // Try to canonicalize; fall back to the raw path (supports unsaved/virtual buffers)
        let resolved = file_path
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(p));
        interpreter.ctx.push_file_path(resolved);
    }

    // In JSON mode, capture stdout/stderr from user code by overriding IO functions
    // (same approach as sema-wasm). This prevents print/println/display from
    // corrupting the JSON envelope on real stdout.
    let captured_stdout: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let captured_stderr: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    if json {
        install_capturing_io(&interpreter, &captured_stdout, &captured_stderr);
    }

    let start = std::time::Instant::now();
    let result = interpreter.eval_str_compiled(&program);
    if result.is_ok() {
        drain_async_scheduler(&interpreter);
    }
    let elapsed_ms = start.elapsed().as_millis() as u64;

    let stdout_text = captured_stdout.borrow();
    let stderr_text = captured_stderr.borrow();

    match result {
        Ok(val) => {
            if json {
                let val_str = if val.is_nil() {
                    None
                } else {
                    Some(pretty_print(&val, 120))
                };
                print_eval_json(&EvalJsonResult {
                    ok: true,
                    value: val_str.as_deref(),
                    stdout: &stdout_text,
                    stderr: &stderr_text,
                    error_msg: None,
                    error_hint: None,
                    error_line: None,
                    error_col: None,
                    elapsed_ms,
                });
            } else if !val.is_nil() {
                println!("{}", pretty_print(&val, 120));
            }
        }
        Err(e) => {
            let inner = e.inner();
            let msg = inner.to_string();
            let hint = e.hint().map(|s| s.to_string());
            // Extract line+col from Reader span or first stack trace frame
            let (line, col) = match inner {
                SemaError::Reader { span, .. } => (Some(span.line), Some(span.col)),
                _ => e
                    .stack_trace()
                    .and_then(|t| t.0.first())
                    .and_then(|f| f.span.as_ref())
                    .map(|s| (Some(s.line), Some(s.col)))
                    .unwrap_or((None, None)),
            };
            if json {
                print_eval_json(&EvalJsonResult {
                    ok: false,
                    value: None,
                    stdout: &stdout_text,
                    stderr: &stderr_text,
                    error_msg: Some(&msg),
                    error_hint: hint.as_deref(),
                    error_line: line,
                    error_col: col,
                    elapsed_ms,
                });
            } else {
                print_error(&e);
                std::process::exit(1);
            }
        }
    }
}

/// Override display/print/println/pprint/newline/print-error/println-error to write
/// to in-memory buffers instead of real stdout/stderr. This prevents user code output
/// from corrupting the JSON envelope in `sema eval --json` mode.
fn install_capturing_io(
    interpreter: &Interpreter,
    stdout_buf: &Rc<RefCell<String>>,
    stderr_buf: &Rc<RefCell<String>>,
) {
    use sema_core::{intern, NativeFn, Value};
    let env = &interpreter.global_env;

    // Helper: register a simple native fn that captures to a buffer
    macro_rules! capture_fn {
        ($name:expr, $buf:expr, $newline:expr, $raw:expr) => {{
            let buf = $buf.clone();
            env.set(
                intern($name),
                Value::native_fn(NativeFn::simple($name, move |args| {
                    let mut out = buf.borrow_mut();
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            out.push(' ');
                        }
                        if $raw {
                            out.push_str(&format!("{arg}"));
                        } else {
                            match arg.as_str() {
                                Some(s) => out.push_str(s),
                                None => out.push_str(&format!("{arg}")),
                            }
                        }
                    }
                    if $newline {
                        out.push('\n');
                    }
                    Ok(Value::nil())
                })),
            );
        }};
    }

    // stdout-targeting functions
    capture_fn!("display", stdout_buf, false, false);
    capture_fn!("print", stdout_buf, false, true);
    capture_fn!("println", stdout_buf, true, false);
    capture_fn!("newline", stdout_buf, true, false);

    // pprint needs special handling (uses pretty_print)
    let pprint_buf = stdout_buf.clone();
    env.set(
        intern("pprint"),
        Value::native_fn(NativeFn::simple("pprint", move |args| {
            sema_core::check_arity!(args, "pprint", 1);
            let mut out = pprint_buf.borrow_mut();
            out.push_str(&sema_core::pretty_print(&args[0], 80));
            out.push('\n');
            Ok(Value::nil())
        })),
    );

    // stderr-targeting functions
    capture_fn!("print-error", stderr_buf, false, false);
    capture_fn!("println-error", stderr_buf, true, false);
}

struct EvalJsonResult<'a> {
    ok: bool,
    value: Option<&'a str>,
    stdout: &'a str,
    stderr: &'a str,
    error_msg: Option<&'a str>,
    error_hint: Option<&'a str>,
    error_line: Option<usize>,
    error_col: Option<usize>,
    elapsed_ms: u64,
}

fn print_eval_json(r: &EvalJsonResult) {
    let result = serde_json::json!({
        "ok": r.ok,
        "value": r.value,
        "stdout": r.stdout,
        "stderr": r.stderr,
        "error": r.error_msg.map(|msg| {
            let mut err = serde_json::json!({ "message": msg });
            if let Some(hint) = r.error_hint {
                err["hint"] = serde_json::json!(hint);
            }
            if let Some(line) = r.error_line {
                err["line"] = serde_json::json!(line);
            }
            if let Some(col) = r.error_col {
                err["col"] = serde_json::json!(col);
            }
            err
        }),
        "elapsedMs": r.elapsed_ms,
    });
    println!("{}", serde_json::to_string(&result).unwrap());
}

fn run_compile(file: &str, output: Option<&str>) {
    let path = std::path::Path::new(file);
    let source = match read_source_file(path) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("error: {msg}");
            std::process::exit(1);
        }
    };

    // Compute source hash (CRC-32)
    let source_hash = crc32fast::hash(source.as_bytes());

    // Use Interpreter for macro expansion before compilation
    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = build_interpreter(&sandbox);

    let result = match interpreter.compile_to_bytecode(&source) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Compile error: {}", e.inner());
            std::process::exit(1);
        }
    };

    // Serialize
    let bytes = match sema_vm::serialize_to_bytes(&result, source_hash) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Serialization error: {}", e.inner());
            std::process::exit(1);
        }
    };

    // Write output
    let out_path = match output {
        Some(o) => std::path::PathBuf::from(o),
        None => path.with_extension("semac"),
    };
    if let Err(e) = std::fs::write(&out_path, &bytes) {
        eprintln!("Error writing {}: {e}", out_path.display());
        std::process::exit(1);
    }
}

fn try_run_embedded() -> Option<i32> {
    let exe_path = std::env::current_exe().ok()?;

    // Try named section first (macOS Mach-O / Windows PE via libsui),
    // fall back to trailer scan (Linux ELF raw append).
    let archive_data = if let Ok(Some(data)) = libsui::find_section("semaexec") {
        data.to_vec()
    } else if archive::has_embedded_archive(&exe_path).ok()? {
        match std::fs::read(&exe_path) {
            Ok(data) => {
                let len = data.len();
                let trailer = &data[len - 16..];
                let archive_size = u64::from_le_bytes(trailer[0..8].try_into().unwrap()) as usize;
                data[len - 16 - archive_size..len - 16].to_vec()
            }
            Err(_) => return None,
        }
    } else {
        return None;
    };

    let arch = match archive::deserialize_archive_from_bytes(&archive_data) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error: failed to load embedded archive: {e}");
            return Some(1);
        }
    };

    let entry_point = arch
        .metadata
        .get("entry-point")
        .and_then(|v| std::str::from_utf8(v).ok())
        .unwrap_or("__main__.semac")
        .to_string();

    let bytecode = match arch.files.get(&entry_point) {
        Some(b) => b.clone(),
        None => {
            eprintln!("Error: entry point '{entry_point}' not found in embedded archive");
            return Some(1);
        }
    };

    sema_core::vfs::init_vfs(arch.files);

    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = build_interpreter(&sandbox);

    let _ = interpreter.eval_str("(llm/auto-configure)");

    let args: Vec<String> = std::env::args().collect();
    let is_mcp = args
        .iter()
        .any(|arg| arg == "--mcp" || arg.starts_with("--mcp="));

    if is_mcp {
        let mut include = None;
        let mut exclude = None;
        for window in args.windows(2) {
            if window[0] == "--include" {
                include = Some(window[1].clone());
            } else if window[0] == "--exclude" {
                exclude = Some(window[1].clone());
            }
        }
        for arg in &args {
            if let Some(rest) = arg.strip_prefix("--include=") {
                include = Some(rest.to_string());
            } else if let Some(rest) = arg.strip_prefix("--exclude=") {
                exclude = Some(rest.to_string());
            }
        }

        let inc_tools = include.map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .collect::<Vec<String>>()
        });
        let exc_tools = exclude.map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .collect::<Vec<String>>()
        });

        if let Err(e) = run_bytecode_bytes(&interpreter, &bytecode) {
            print_error(&e);
            return Some(1);
        }

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime")
            .block_on(async {
                if let Err(e) = sema_mcp::run_mcp_server(interpreter, inc_tools, exc_tools).await {
                    eprintln!("MCP server error: {e}");
                    std::process::exit(1);
                }
            });
        Some(0)
    } else {
        match run_bytecode_bytes(&interpreter, &bytecode) {
            Ok(_) => Some(0),
            Err(e) => {
                print_error(&e);
                Some(1)
            }
        }
    }
}

fn run_build(
    file: &str,
    output: Option<&str>,
    includes: &[String],
    runtime: Option<&str>,
    target: Option<&str>,
    no_cache: bool,
) -> Result<(), String> {
    // Handle --target all (build for every supported target)
    if target == Some("all") {
        let stem = std::path::Path::new(file)
            .file_stem()
            .unwrap_or(std::ffi::OsStr::new(file))
            .to_string_lossy();
        let mut failures = Vec::new();
        for t in cross_compile::SUPPORTED_TARGETS {
            let ext = if cross_compile::is_windows_target(t) {
                ".exe"
            } else {
                ""
            };
            let target_output = format!("{stem}-{t}{ext}");
            eprintln!("\n━━━ Building for {t} ━━━");
            if let Err(e) = run_build(
                file,
                Some(&target_output),
                includes,
                None,
                Some(t),
                no_cache,
            ) {
                eprintln!("Error: {e}");
                failures.push(*t);
            }
        }
        if !failures.is_empty() {
            return Err(format!(
                "failed to build for {} target(s): {}\n  Hint: re-run a single target for details: `sema build --target <target> {}`\n  Hint: use `--runtime /path/to/sema` if downloads fail, or install a released version of sema.",
                failures.len(),
                failures.join(", "),
                file
            ));
        }
        return Ok(());
    }

    if target == Some("web") {
        return run_build_web(file, output, includes);
    }

    let path = std::path::Path::new(file);

    let source = read_source_file(path)?;

    // Pre-flight: resolve output path now so we can probe the parent directory
    // for writability before running any compilation steps. This avoids the
    // frustrating "failed at step 5 of 5" experience when the user gave an
    // unwritable -o path.
    let output_path: std::path::PathBuf = match output {
        Some(o) => std::path::PathBuf::from(o),
        None => {
            let stem = path.file_stem().unwrap_or(path.as_os_str());
            let needs_exe = target
                .and_then(|t| cross_compile::resolve_target(t).ok())
                .is_some_and(cross_compile::is_windows_target)
                || (target.is_none() && cfg!(windows));
            if needs_exe {
                std::path::PathBuf::from(format!("{}.exe", stem.to_string_lossy()))
            } else {
                std::path::PathBuf::from(stem)
            }
        }
    };
    probe_output_writable(&output_path)?;

    eprintln!("[1/5] Compiling {file}...");

    // Compute source hash and compile to bytecode
    let source_hash = crc32fast::hash(source.as_bytes());
    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = build_interpreter(&sandbox);

    let result = match interpreter.compile_to_bytecode(&source) {
        Ok(r) => r,
        Err(e) => {
            return Err(format!("compile error: {}", e.inner()));
        }
    };

    let bytecode = match sema_vm::serialize_to_bytes(&result, source_hash) {
        Ok(b) => b,
        Err(e) => {
            return Err(format!("serialization error: {}", e.inner()));
        }
    };

    eprintln!("[2/5] Tracing imports...");

    // Trace transitive imports
    let imports = match import_tracer::trace_imports(path) {
        Ok(m) => m,
        Err(e) => {
            return Err(format!("tracing imports: {e}"));
        }
    };

    eprintln!("[3/5] Collecting assets...");

    // Build VFS files map
    let mut files = std::collections::HashMap::new();

    // Entry point bytecode
    files.insert("__main__.semac".to_string(), bytecode);

    // Traced imports
    for (rel_path, contents) in &imports {
        if let Err(e) = sema_core::vfs::validate_vfs_path(rel_path) {
            eprintln!("Warning: skipping import with invalid VFS path: {e}");
            continue;
        }
        files.insert(rel_path.clone(), contents.clone());
    }

    // Additional --include assets
    for include in includes {
        let inc_path = std::path::Path::new(include);
        if inc_path.is_dir() {
            let base = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            collect_directory_files(inc_path, &base, &mut files);
        } else if inc_path.is_file() {
            let rel = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            if let Err(e) = sema_core::vfs::validate_vfs_path(&rel) {
                eprintln!("Warning: skipping {include}: {e}");
                continue;
            }
            match std::fs::read(inc_path) {
                Ok(data) => {
                    files.insert(rel, data);
                }
                Err(e) => {
                    eprintln!("Warning: cannot read {include}: {e}");
                }
            }
        } else {
            eprintln!("Warning: --include path not found: {include}");
        }
    }

    eprintln!("[4/5] Building archive ({} files)...", files.len());

    // Build metadata
    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "sema-version".to_string(),
        env!("CARGO_PKG_VERSION").as_bytes().to_vec(),
    );
    metadata.insert(
        "build-timestamp".to_string(),
        build_timestamp().into_bytes(),
    );
    metadata.insert("entry-point".to_string(), b"__main__.semac".to_vec());

    let canonical_root = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    metadata.insert(
        "build-root".to_string(),
        canonical_root.to_string_lossy().into_owned().into_bytes(),
    );

    let archive_bytes = archive::serialize_archive(&metadata, &files);

    eprintln!("[5/5] Writing executable...");

    // Check that output doesn't overwrite the source file
    let input_canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let output_canonical =
        std::fs::canonicalize(&output_path).unwrap_or_else(|_| output_path.clone());
    if input_canonical == output_canonical {
        return Err(format!(
            "Output path would overwrite the source file '{}'.\n  Hint: use `-o <output>` to specify a different output path, or rename your source file to use a .sema extension.",
            path.display()
        ));
    }

    // Resolve target triple for later use
    let resolved_target = target.and_then(|t| cross_compile::resolve_target(t).ok());

    // Determine runtime binary
    let runtime_path = if let Some(r) = runtime {
        // Validate runtime binary format against target if both are specified
        if let Some(resolved) = resolved_target {
            let runtime_bytes =
                std::fs::read(r).map_err(|e| format!("cannot read --runtime file '{}': {e}", r))?;
            let detected = cross_compile::detect_binary_format(&runtime_bytes);
            let expected = cross_compile::expected_format(resolved);
            if let Some(det) = detected {
                if det != expected {
                    return Err(format!(
                        "Runtime binary format mismatch: {resolved} expects {expected} but --runtime file is {det}\n  Hint: provide a {expected} binary built for {resolved}, or omit --runtime to download automatically."
                    ));
                }
            }
        }
        std::path::PathBuf::from(r)
    } else if let Some(t) = target {
        let resolved = match cross_compile::resolve_target(t) {
            Ok(t) => t,
            Err(e) => {
                return Err(e.to_string());
            }
        };
        if cross_compile::is_host_target(resolved) {
            eprintln!("  Target {resolved} matches host — using local runtime (no download)");
            match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => {
                    return Err(format!("cannot determine current executable path: {e}"));
                }
            }
        } else {
            match cross_compile::ensure_runtime(resolved, no_cache) {
                Ok(p) => p,
                Err(e) => {
                    return Err(e.to_string());
                }
            }
        }
    } else {
        match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                return Err(format!("cannot determine current executable path: {e}"));
            }
        }
    };

    if let Err(e) = write_executable_platform(&runtime_path, &output_path, &archive_bytes) {
        return Err(format!("writing executable: {e}"));
    }

    eprintln!(
        "Built: {} ({} bytes, {} bundled files)",
        output_path.display(),
        std::fs::metadata(&output_path)
            .map(|m| m.len())
            .unwrap_or(0),
        files.len()
    );

    if let Some(resolved) = resolved_target {
        if !cross_compile::is_host_target(resolved) {
            eprintln!(
                "  Note: this binary targets {resolved} and won't run on your current machine."
            );
        }
    }

    Ok(())
}

fn compile_source_to_bytecode(source: &str) -> Result<Vec<u8>, String> {
    let source_hash = crc32fast::hash(source.as_bytes());
    let sandbox = sema_core::Sandbox::allow_all();
    let interpreter = Interpreter::new_with_sandbox(&sandbox);
    interpreter
        .eval_str_in_global(include_str!("web_prelude.sema"))
        .map_err(|e| format!("web prelude error: {}", e.inner()))?;
    let result = interpreter
        .compile_to_bytecode(source)
        .map_err(|e| format!("compile error: {}", e.inner()))?;
    sema_vm::serialize_to_bytes(&result, source_hash)
        .map_err(|e| format!("serialization error: {}", e.inner()))
}

fn should_compile_traced_import(rel_path: &str) -> bool {
    rel_path.ends_with(".sema") || sema_core::resolve::is_package_import(rel_path)
}

fn web_output_path(input: &std::path::Path, output: Option<&str>) -> std::path::PathBuf {
    let default_name = format!(
        "{}.vfs",
        input
            .file_stem()
            .unwrap_or(input.as_os_str())
            .to_string_lossy()
    );

    match output {
        Some(raw) => {
            let path = std::path::PathBuf::from(raw);
            if path.is_dir() || raw.ends_with(std::path::MAIN_SEPARATOR) {
                path.join(default_name)
            } else if path.extension().is_none() {
                path.with_extension("vfs")
            } else {
                path
            }
        }
        None => std::path::PathBuf::from(default_name),
    }
}

/// Compile an entry `.sema` plus its traced imports (and any `includes`) into a
/// web `.vfs` archive. Returns the archive bytes and the number of traced
/// imports (0 = single-file). Shared by `sema build --target web` and the
/// `sema web` dev server, which builds an archive on the fly for multi-file apps.
pub(crate) fn build_web_archive(
    path: &std::path::Path,
    includes: &[String],
) -> Result<(Vec<u8>, usize), String> {
    let source =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let entry_bytecode = compile_source_to_bytecode(&source)?;

    let imports =
        import_tracer::trace_imports(path).map_err(|e| format!("tracing imports: {e}"))?;
    let import_count = imports.len();

    let mut files = std::collections::HashMap::new();
    files.insert("__main__.semac".to_string(), entry_bytecode);

    for (rel_path, contents) in &imports {
        if let Err(e) = sema_core::vfs::validate_vfs_path(rel_path) {
            eprintln!("Warning: skipping import with invalid VFS path: {e}");
            continue;
        }

        let bundled = if should_compile_traced_import(rel_path) {
            let import_source = String::from_utf8(contents.clone()).map_err(|e| {
                format!("compile error in {rel_path}: import is not valid UTF-8: {e}")
            })?;
            compile_source_to_bytecode(&import_source).map_err(|e| format!("{e} in {rel_path}"))?
        } else {
            contents.clone()
        };

        files.insert(rel_path.clone(), bundled);
    }

    for include in includes {
        let inc_path = std::path::Path::new(include);
        if inc_path.is_dir() {
            let base = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            collect_directory_files(inc_path, &base, &mut files);
        } else if inc_path.is_file() {
            let rel = inc_path
                .file_name()
                .unwrap_or(inc_path.as_os_str())
                .to_string_lossy()
                .to_string();
            if let Err(e) = sema_core::vfs::validate_vfs_path(&rel) {
                eprintln!("Warning: skipping {include}: {e}");
                continue;
            }
            match std::fs::read(inc_path) {
                Ok(data) => {
                    files.insert(rel, data);
                }
                Err(e) => {
                    eprintln!("Warning: cannot read {include}: {e}");
                }
            }
        } else {
            eprintln!("Warning: --include path not found: {include}");
        }
    }

    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "sema-version".to_string(),
        env!("CARGO_PKG_VERSION").as_bytes().to_vec(),
    );
    metadata.insert(
        "build-timestamp".to_string(),
        build_timestamp().into_bytes(),
    );
    metadata.insert("entry-point".to_string(), b"__main__.semac".to_vec());
    metadata.insert("build-target".to_string(), b"web".to_vec());

    let canonical_root = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    metadata.insert(
        "build-root".to_string(),
        canonical_root.to_string_lossy().into_owned().into_bytes(),
    );

    Ok((archive::serialize_archive(&metadata, &files), import_count))
}

fn run_build_web(file: &str, output: Option<&str>, includes: &[String]) -> Result<(), String> {
    let path = std::path::Path::new(file);
    if !path.exists() {
        return Err(format!("source file not found: {file}"));
    }

    eprintln!("Compiling {file} for web...");
    let (archive_bytes, import_count) = build_web_archive(path, includes)?;

    let output_path = web_output_path(path, output);
    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating output directory {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(&output_path, &archive_bytes)
        .map_err(|e| format!("writing {}: {e}", output_path.display()))?;

    eprintln!(
        "Built web archive: {} ({} bytes, {} imports bundled)",
        output_path.display(),
        archive_bytes.len(),
        import_count
    );
    eprintln!(
        "  Load with <script type=\"text/sema\" src=\"{}\"></script>",
        output_path.display()
    );

    Ok(())
}

/// Probe whether we can write to the directory that will hold `output_path`.
///
/// Creates and immediately deletes a tiny probe file in the parent directory.
/// Returns a clear error before the build commits to any work if the directory
/// doesn't exist or denies writes (e.g. /readonly/sema, /no/such/dir/sema).
fn probe_output_writable(output_path: &Path) -> Result<(), String> {
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    if !parent.exists() {
        return Err(format!(
            "output directory does not exist: {}",
            parent.display()
        ));
    }
    let probe_name = format!(
        ".sema-build-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    let probe = parent.join(probe_name);
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => match e.kind() {
            std::io::ErrorKind::PermissionDenied => Err(format!(
                "permission denied writing to {} (for output {})",
                parent.display(),
                output_path.display()
            )),
            std::io::ErrorKind::NotFound => Err(format!(
                "output directory does not exist: {}",
                parent.display()
            )),
            _ => Err(format!(
                "cannot write to {}: {}",
                parent.display(),
                strip_os_error(&e.to_string())
            )),
        },
    }
}

/// Strip trailing " (os error N)" from a system error string for nicer output.
fn strip_os_error(s: &str) -> String {
    if let Some(idx) = s.rfind(" (os error ") {
        if s.ends_with(')') {
            return s[..idx].to_string();
        }
    }
    s.to_string()
}

/// Write the executable using format-aware injection.
///
/// Detects the binary format at runtime (not compile-time) so that
/// cross-compilation works: e.g. injecting into an ELF binary from macOS.
/// Note: libsui uses pure Rust for Mach-O ad-hoc signing (sha2 + object crate),
/// so cross-injecting Mach-O from Linux works without macOS tools.
fn write_executable_platform(
    runtime_path: &std::path::Path,
    output_path: &std::path::Path,
    archive_bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = std::fs::read(runtime_path)?;

    let format = cross_compile::detect_binary_format(&runtime).ok_or_else(|| {
        if runtime.len() < 4 {
            "runtime binary too small to detect format".to_string()
        } else if runtime[..4].iter().all(|&b| b.is_ascii_graphic() || b.is_ascii_whitespace())
            || (runtime.len() >= 3 && runtime[..3] == [0xEF, 0xBB, 0xBF])
        {
            format!(
                "unrecognized binary format (magic: {:02X} {:02X} {:02X} {:02X})\n  \
                 This looks like a source file, not a compiled binary.",
                runtime[0], runtime[1], runtime[2], runtime[3]
            )
        } else if (runtime[0] == 0x50 && runtime[1] == 0x4B)  // ZIP (PK)
            || (runtime[0] == 0x1F && runtime[1] == 0x8B)      // gzip
            || (runtime[..4] == [0xFD, 0x37, 0x7A, 0x58])      // xz
        {
            format!(
                "unrecognized binary format (magic: {:02X} {:02X} {:02X} {:02X})\n  \
                 This looks like an archive. Extract it first, or omit --runtime to let sema download automatically.",
                runtime[0], runtime[1], runtime[2], runtime[3]
            )
        } else {
            format!(
                "unrecognized binary format (magic: {:02X} {:02X} {:02X} {:02X})\n  \
                 The --runtime file doesn't appear to be a valid sema executable.",
                runtime[0], runtime[1], runtime[2], runtime[3]
            )
        }
    })?;

    match format {
        cross_compile::BinaryFormat::MachO => {
            let mut out = std::fs::File::create(output_path)?;
            libsui::Macho::from(runtime)?
                .write_section("semaexec", archive_bytes.to_vec())?
                .build_and_sign(&mut out)?;
        }
        cross_compile::BinaryFormat::Pe => {
            let mut out = std::fs::File::create(output_path)?;
            libsui::PortableExecutable::from(&runtime)?
                .write_resource("semaexec", archive_bytes.to_vec())?
                .build(&mut out)?;
        }
        cross_compile::BinaryFormat::Elf => {
            archive::write_bundled_executable_from_bytes(&runtime, output_path, archive_bytes)?;
            return Ok(());
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(output_path, perms)?;
    }

    Ok(())
}

/// Recursively collect files from a directory into the VFS files map.
fn collect_directory_files(
    dir: &std::path::Path,
    base: &str,
    files: &mut std::collections::HashMap<String, Vec<u8>>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Warning: cannot read directory {}: {e}", dir.display());
            return;
        }
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let vfs_path = if base.is_empty() {
            name.clone()
        } else {
            format!("{base}/{name}")
        };

        if entry_path.is_dir() {
            collect_directory_files(&entry_path, &vfs_path, files);
        } else if entry_path.is_file() {
            if let Err(e) = sema_core::vfs::validate_vfs_path(&vfs_path) {
                eprintln!("Warning: skipping {}: {e}", entry_path.display());
                continue;
            }
            match std::fs::read(&entry_path) {
                Ok(data) => {
                    files.insert(vfs_path, data);
                }
                Err(e) => {
                    eprintln!("Warning: cannot read {}: {e}", entry_path.display());
                }
            }
        }
    }
}

/// Return current Unix timestamp as a string (seconds since epoch).
fn build_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn run_check(file: &str) {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("✗ {file}: {e}");
            std::process::exit(1);
        }
    };

    if !sema_vm::is_bytecode_file(&bytes) {
        eprintln!("✗ {file}: not a valid .semac bytecode file");
        std::process::exit(1);
    }

    // Read header info before full deserialization
    let format_version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let major = u16::from_le_bytes([bytes[8], bytes[9]]);
    let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
    let patch = u16::from_le_bytes([bytes[12], bytes[13]]);

    match sema_vm::deserialize_from_bytes(&bytes) {
        Ok(result) => {
            let n_funcs = result.functions.len();
            println!(
                "✓ {file}: valid (format v{format_version}, sema {major}.{minor}.{patch}, {n_funcs} function{}, {} bytes)",
                if n_funcs == 1 { "" } else { "s" },
                bytes.len()
            );
        }
        Err(e) => {
            eprintln!("✗ {file}: {}", e.inner());
            std::process::exit(1);
        }
    }
}

fn run_disasm(file: &str, json: bool) {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => {
            let msg = match e.kind() {
                std::io::ErrorKind::NotFound => format!("file not found: {file}"),
                std::io::ErrorKind::PermissionDenied => format!("permission denied: {file}"),
                _ => format!("reading {file}: {e}"),
            };
            eprintln!("error: {msg}");
            std::process::exit(1);
        }
    };

    if !sema_vm::is_bytecode_file(&bytes) {
        eprintln!("Error: {file} is not a valid .semac bytecode file");
        std::process::exit(1);
    }

    let result = match sema_vm::deserialize_from_bytes(&bytes) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Deserialization error: {}", e.inner());
            std::process::exit(1);
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

fn run_bytecode_bytes(
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
            param_names: Vec::new().into(),
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
    // Initialize the async scheduler so async/await and channels work in a
    // `.semac` program (top-level or inside a `(load ...)`). A `.semac` carries
    // no native table (the format is process-local), and bytecode compiled with
    // `known_natives=None` uses CallGlobal rather than CallNative, so task VMs
    // resolve natives via the shared global env — an empty native table is
    // correct here.
    sema_vm::init_scheduler(interpreter.global_env.clone(), Vec::new());
    vm.execute(closure, &interpreter.ctx)
}

fn run_fmt(
    patterns: &[String],
    check: bool,
    show_diff: bool,
    opts: &sema_fmt::FormatOptions,
    json: bool,
) {
    // Handle stdin ("-")
    if patterns.len() == 1 && patterns[0] == "-" {
        let mut source = String::new();
        if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut source) {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "formatted": false,
                        "error": format!("Error reading stdin: {e}")
                    })
                );
            } else {
                eprintln!("Error reading stdin: {e}");
            }
            std::process::exit(1);
        }
        match sema_fmt::format_source(&source, opts) {
            Ok(formatted) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "formatted": true,
                            "source": formatted
                        })
                    );
                } else {
                    print!("{formatted}");
                }
            }
            Err(e) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "formatted": false,
                            "error": format!("{e}")
                        })
                    );
                } else {
                    eprintln!("Error formatting stdin: {e}");
                }
                std::process::exit(1);
            }
        }
        return;
    }

    // Determine which files to format
    let files = if patterns.is_empty() {
        // Default: all .sema files in current directory recursively
        match glob::glob("**/*.sema") {
            Ok(paths) => paths
                .filter_map(|p| p.ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            Err(e) => {
                eprintln!("Error: invalid glob pattern: {e}");
                std::process::exit(1);
            }
        }
    } else {
        // Expand each pattern
        let mut all_files = Vec::new();
        for pattern in patterns {
            // If it contains glob characters, expand it
            if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
                match glob::glob(pattern) {
                    Ok(paths) => {
                        for path in paths.filter_map(|p| p.ok()) {
                            all_files.push(path.to_string_lossy().to_string());
                        }
                    }
                    Err(e) => {
                        eprintln!("Error: invalid glob pattern '{pattern}': {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                // Treat as literal file path
                all_files.push(pattern.clone());
            }
        }
        all_files
    };

    if files.is_empty() {
        println!("No .sema files found");
        return;
    }

    let mut checked = 0;
    let mut changed = 0;
    let mut errors = 0;

    for file in &files {
        let source = match read_source_file(file) {
            Ok(s) => s,
            Err(msg) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "file": file,
                            "formatted": false,
                            "error": msg,
                        })
                    );
                } else {
                    eprintln!("error: {msg}");
                }
                errors += 1;
                continue;
            }
        };

        let formatted = match sema_fmt::format_source(&source, opts) {
            Ok(f) => f,
            Err(e) => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "file": file,
                            "formatted": false,
                            "error": format!("Error formatting {file}: {e}")
                        })
                    );
                } else {
                    eprintln!("Error formatting {file}: {e}");
                }
                errors += 1;
                continue;
            }
        };

        if json {
            println!(
                "{}",
                serde_json::json!({
                    "file": file,
                    "formatted": true,
                    "source": formatted
                })
            );
            continue;
        }

        checked += 1;

        if source != formatted {
            changed += 1;

            if check {
                println!("Would reformat: {file}");
            } else if show_diff {
                // Simple line-by-line diff
                print_simple_diff(file, &source, &formatted);
            } else {
                // Write formatted output back
                if let Err(e) = std::fs::write(file, &formatted) {
                    eprintln!("Error writing {file}: {e}");
                    errors += 1;
                    continue;
                }
                println!("Formatted: {file}");
            }
        }
    }

    // Print summary
    if check {
        if changed > 0 {
            println!("\n{changed} file(s) would be reformatted, {checked} file(s) checked");
            std::process::exit(1);
        } else {
            println!("{checked} file(s) already formatted");
        }
    } else if show_diff {
        println!("\n{changed} file(s) would change, {checked} file(s) checked");
    } else if changed > 0 {
        println!(
            "\n{changed} file(s) formatted, {} file(s) unchanged",
            checked - changed
        );
    } else {
        println!("{checked} file(s) already formatted");
    }

    if errors > 0 {
        eprintln!("{errors} error(s)");
        std::process::exit(1);
    }
}

fn print_simple_diff(filename: &str, old: &str, new: &str) {
    println!("--- {filename}");
    println!("+++ {filename}");
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Simple context diff: show lines that differ
    let max_lines = old_lines.len().max(new_lines.len());
    let mut in_diff = false;
    let mut diff_start = 0;

    for i in 0..max_lines {
        let old_line = old_lines.get(i).copied().unwrap_or("");
        let new_line = new_lines.get(i).copied().unwrap_or("");

        if old_line != new_line {
            if !in_diff {
                diff_start = i;
                in_diff = true;
                println!("@@ -{} +{} @@", i + 1, i + 1);
            }
            if i < old_lines.len() {
                println!("-{old_line}");
            }
            if i < new_lines.len() {
                println!("+{new_line}");
            }
        } else if in_diff && i - diff_start < 3 {
            println!(" {old_line}");
        } else {
            in_diff = false;
        }
    }
}

fn run_ast(file: Option<String>, eval: Option<String>, json: bool) {
    let source = match (&file, &eval) {
        (Some(path), None) => match read_source_file(path) {
            Ok(content) => content,
            Err(msg) => {
                eprintln!("error: {msg}");
                std::process::exit(1);
            }
        },
        (None, Some(expr)) => expr.clone(),
        (Some(_), Some(_)) => {
            eprintln!("Error: cannot specify both a file and --eval");
            std::process::exit(1);
        }
        (None, None) => {
            eprintln!("Error: provide a file or --eval expression");
            std::process::exit(1);
        }
    };

    let exprs = match sema_reader::read_many(&source) {
        Ok(exprs) => exprs,
        Err(e) => {
            eprintln!("Parse error: {}", e.inner());
            std::process::exit(1);
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

pub(crate) fn format_source_snippet(
    span: &sema_core::Span,
    file_override: Option<&std::path::Path>,
) -> Option<String> {
    let (source, file) = if let Some(path) = file_override {
        let content = std::fs::read_to_string(path).ok()?;
        (content, Some(path.to_path_buf()))
    } else {
        let source = LAST_SOURCE.with(|s| s.borrow().clone())?;
        let file = LAST_FILE.with(|f| f.borrow().clone());
        (source, file)
    };

    let lines: Vec<&str> = source.lines().collect();
    let line_idx = span.line.checked_sub(1)?;
    let source_line = lines.get(line_idx)?;
    let col = span.col.saturating_sub(1);
    let line_num = span.line;
    let gutter_width = format!("{line_num}").len().max(2);
    let location = if let Some(path) = &file {
        format!("{}:{}:{}", path.display(), line_num, span.col)
    } else {
        format!("<input>:{}:{}", line_num, span.col)
    };

    let mut out = String::new();
    out.push_str(&format!("  {} {}\n", colors::cyan("-->"), location));
    out.push_str(&format!("  {:>gutter_width$} {}\n", "", colors::cyan("|")));
    out.push_str(&format!(
        "  {} {} {}\n",
        colors::cyan(&format!("{:>gutter_width$}", line_num)),
        colors::cyan("|"),
        source_line
    ));
    out.push_str(&format!(
        "  {:>gutter_width$} {} {}{}",
        "",
        colors::cyan("|"),
        " ".repeat(col),
        colors::red_bold("^")
    ));
    Some(out)
}

pub(crate) fn print_error(e: &SemaError) {
    let inner = e.inner();
    eprintln!("{} {}", colors::red_bold("Error:"), inner);

    // Show source snippet for reader errors
    if let SemaError::Reader { span, .. } = inner {
        if let Some(snippet) = format_source_snippet(span, None) {
            eprintln!("{snippet}");
        }
    }

    if let Some(trace) = e.stack_trace() {
        // Show source context for innermost frame
        if let Some(first_frame) = trace.0.first() {
            if let Some(span) = &first_frame.span {
                let snippet = if first_frame.file.is_some() {
                    format_source_snippet(span, first_frame.file.as_deref())
                } else {
                    format_source_snippet(span, None)
                };
                if let Some(snippet) = snippet {
                    eprintln!("{snippet}");
                }
            }
        }

        for frame in &trace.0 {
            let loc = match (&frame.file, &frame.span) {
                (Some(file), Some(span)) => format!("({}:{span})", file.display()),
                (Some(file), None) => format!("({})", file.display()),
                (None, Some(span)) => format!("(<input>:{span})"),
                (None, None) => String::new(),
            };
            if loc.is_empty() {
                eprintln!("  {} {}", colors::dim("at"), frame.name);
            } else {
                eprintln!(
                    "  {} {} {}",
                    colors::dim("at"),
                    frame.name,
                    colors::dim(&loc)
                );
            }
        }
    }
    if let Some(hint) = e.hint() {
        eprintln!("  {} {hint}", colors::cyan("hint:"));
    }
    if let Some(note) = e.note() {
        eprintln!("  {} {note}", colors::yellow("note:"));
    }
}

fn run_doc(
    command: Option<DocCommands>,
    symbol: Option<String>,
    pager: docs::PagerMode,
) -> Result<(), String> {
    match command {
        Some(DocCommands::Show { symbol }) => show_doc(&symbol, pager),
        Some(DocCommands::Search { query, limit }) => {
            let query = query.join(" ");
            let query = query.trim().to_string();
            if query.is_empty() {
                return Err("usage: sema doc search <query>".to_string());
            }
            let rendered =
                docs::render_search_results(&query, &docs::doc_search_results(&query, limit));
            docs::print_rendered(&rendered, pager).map_err(|e| format!("writing docs: {e}"))
        }
        Some(DocCommands::Apropos { pattern }) => {
            let hits = docs::builtin_apropos_hits(&pattern);
            let rendered = docs::render_apropos_hits(&pattern, &hits);
            docs::print_rendered(&rendered, pager).map_err(|e| format!("writing docs: {e}"))
        }
        None => {
            let Some(symbol) = symbol else {
                return Err("usage: sema doc <symbol> | sema doc search <query> | sema doc apropos <pattern>".to_string());
            };
            show_doc(&symbol, pager)
        }
    }
}

fn show_doc(symbol: &str, pager: docs::PagerMode) -> Result<(), String> {
    let Some(rendered) = docs::rendered_doc(symbol) else {
        return Err(format!("documentation not found: {symbol}"));
    };
    docs::print_rendered(&rendered, pager).map_err(|e| format!("writing docs: {e}"))
}

fn generate_completions(shell: Shell) -> String {
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "sema", &mut buf);
    let mut out = String::from_utf8(buf).expect("clap completion output is utf-8");
    if shell == Shell::Zsh {
        out = fix_zsh_root_completion(out);
    }
    out.push_str(dynamic_doc_completion_script(shell));
    out
}

/// Repair subcommand completion in the generated zsh script.
///
/// `clap_complete`'s zsh generator emits the top-level optional positionals
/// (`FILE`, `SCRIPT_ARGS`) *before* the subcommand slot — even with
/// `args_conflicts_with_subcommands` set — so zsh consumes `sema notebook` as
/// the FILE positional: `sema <TAB>` offers only files and
/// `sema notebook <TAB>` completes script arguments. Subcommand completion
/// never engages, at any depth.
///
/// The repair makes position 1 an alternation of subcommands and script files
/// (`_sema_root`), and re-indexes the subcommand dispatch from `$line[3]` to
/// `$line[1]`. Every rewrite is anchored on the exact generator output; if an
/// anchor is missing (a future clap_complete changed shape), the script is
/// returned UNMODIFIED — a wrong-but-consistent script beats a broken one —
/// and the pinning unit test fails loudly so the anchors get refreshed.
///
/// zsh is the ONLY affected shell: its generator dispatches by positional
/// index (`$line[N]`), while bash (word-walk), fish
/// (`__fish_seen_subcommand_from`), elvish and powershell (name-keyed maps)
/// all match literal subcommand names — verified empirically 2026-07-03
/// (bash 5.2 in a clean container; fish `complete -C`; pwsh
/// `CommandCompletion::CompleteInput`; elvish statically).
fn fix_zsh_root_completion(script: String) -> String {
    const POSITIONALS: &str = "'::file -- File to execute:_default' \\\n\
'::script_args -- Arguments passed to the script (after --):_default' \\\n\
\":: :_sema_commands\" \\\n";
    const ROOT_SLOT: &str = "\":: :_sema_root\" \\\n";
    let anchors_present = script.contains(POSITIONALS)
        && script.contains("words=($line[3] \"${words[@]}\")")
        && script.contains("case $line[3] in");
    if !anchors_present {
        return script;
    }
    let mut out = script.replacen(POSITIONALS, ROOT_SLOT, 1);
    out = out.replacen(
        "words=($line[3] \"${words[@]}\")",
        "words=($line[1] \"${words[@]}\")",
        1,
    );
    out = out.replacen(
        "curcontext=\"${curcontext%:*:*}:sema-command-$line[3]:\"",
        "curcontext=\"${curcontext%:*:*}:sema-command-$line[1]:\"",
        1,
    );
    out = out.replacen("case $line[3] in", "case $line[1] in", 1);
    // The definition must precede clap's self-invoking trailer
    // (`if [ "$funcstack[1]" = "_sema" ]; then _sema "$@" ...`): on the very
    // first TAB the file executes top-to-bottom and calls `_sema` right there —
    // a root fn appended after the trailer is not yet defined at that moment.
    let root_fn = "\n_sema_root() {\n    _alternative \\\n        'subcommands:sema command:_sema_commands' \\\n        'files:script file:_files'\n}\n\n";
    const TRAILER: &str = "if [ \"$funcstack[1]\" = \"_sema\" ]; then";
    if let Some(pos) = out.find(TRAILER) {
        out.insert_str(pos, root_fn);
    } else {
        out.push_str(root_fn);
    }
    out
}

fn dynamic_doc_completion_script(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => {
            r#"

# Dynamic Sema doc symbol completion.
_sema_doc_complete() {
    local cur="${COMP_WORDS[COMP_CWORD]}"
    if [[ ${COMP_WORDS[1]} == doc && ${COMP_CWORD} -eq 2 ]]; then
        COMPREPLY=( $(compgen -W "$(sema __complete-doc-symbols "$cur")" -- "$cur") )
        return
    fi
    if [[ ${COMP_WORDS[1]} == doc && ${COMP_WORDS[2]} == show && ${COMP_CWORD} -eq 3 ]]; then
        COMPREPLY=( $(compgen -W "$(sema __complete-doc-symbols "$cur")" -- "$cur") )
        return
    fi
    _sema "$@"
}
complete -o nosort -o bashdefault -o default -F _sema_doc_complete sema
"#
        }
        Shell::Zsh => {
            r#"

# Dynamic Sema doc symbol completion.
_sema_doc_complete() {
  if (( CURRENT == 3 )) && [[ "${words[2]}" == "doc" ]]; then
    local -a matches
    matches=("${(@f)$(sema __complete-doc-symbols "${words[CURRENT]}")}")
    _describe 'Sema doc symbol' matches
    return
  fi
  if (( CURRENT == 4 )) && [[ "${words[2]}" == "doc" && "${words[3]}" == "show" ]]; then
    local -a matches
    matches=("${(@f)$(sema __complete-doc-symbols "${words[CURRENT]}")}")
    _describe 'Sema doc symbol' matches
    return
  fi
  _sema "$@"
}
compdef _sema_doc_complete sema
"#
        }
        Shell::Fish => {
            r#"

# Dynamic Sema doc symbol completion.
complete -c sema -n '__fish_seen_subcommand_from doc; and not __fish_seen_subcommand_from show search apropos' -a '(sema __complete-doc-symbols (commandline -ct))'
complete -c sema -n '__fish_seen_subcommand_from doc show' -a '(sema __complete-doc-symbols (commandline -ct))'
"#
        }
        _ => "",
    }
}

fn install_completions(shell: Shell) {
    let home = match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        Ok(h) => PathBuf::from(h),
        Err(_) => {
            eprintln!("Error: could not determine home directory");
            std::process::exit(1);
        }
    };

    let path = match shell {
        Shell::Zsh => home.join(".zsh/completions/_sema"),
        Shell::Bash => home.join(".local/share/bash-completion/completions/sema"),
        Shell::Fish => home.join(".config/fish/completions/sema.fish"),
        Shell::Elvish => home.join(".config/elvish/lib/sema.elv"),
        Shell::PowerShell => {
            eprintln!(
                "Auto-install is not supported for PowerShell.\n\
                 Run manually: sema completions powershell >> $PROFILE"
            );
            std::process::exit(1);
        }
        _ => {
            eprintln!("Auto-install is not supported for this shell.");
            std::process::exit(1);
        }
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!("Error creating directory {}: {e}", parent.display());
            std::process::exit(1);
        });
    }

    let completions = generate_completions(shell);
    std::fs::write(&path, completions).unwrap_or_else(|e| {
        eprintln!("Error writing {}: {e}", path.display());
        std::process::exit(1);
    });

    println!("✓ Installed {shell} completions to {}", path.display());
    if shell == Shell::Zsh {
        println!("  Add to ~/.zshrc (before compinit): fpath=(~/.zsh/completions $fpath)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the zsh-completion repair (`fix_zsh_root_completion`): position 1
    /// must dispatch subcommands (clap_complete emits the FILE/SCRIPT_ARGS
    /// positionals first, which swallows the subcommand word — `sema notebook
    /// <TAB>` completed files). If this fails after a clap_complete upgrade,
    /// refresh the anchors in `fix_zsh_root_completion`.
    #[test]
    fn zsh_completions_dispatch_subcommands_at_position_one() {
        let script = generate_completions(clap_complete::Shell::Zsh);
        assert!(
            script.contains(":: :_sema_root"),
            "root slot missing — anchor drift in fix_zsh_root_completion"
        );
        assert!(
            script.contains("_sema_root() {"),
            "root alternation fn missing"
        );
        assert!(
            script.contains("case $line[1] in") && !script.contains("case $line[3] in"),
            "top-level dispatch must read the subcommand from position 1"
        );
        assert!(
            !script.contains("File to execute"),
            "top-level FILE positional must not shadow the subcommand slot"
        );
        // The nested groups must still be intact (spot-check one).
        assert!(script.contains("_sema__notebook_commands"));
    }

    use super::{compile_source_to_bytecode, run_bytecode_bytes};
    use sema_core::{intern, NativeFn, Sandbox, Value};
    use sema_eval::Interpreter;

    #[test]
    fn web_build_prelude_expands_defcomponent_into_callable_global() {
        let source = r##"
            (defcomponent counter-view ()
              [:div "ok"])
            (mount! "#app" counter-view)
        "##;

        let bytes = compile_source_to_bytecode(source).expect("compile should succeed");

        let interp = Interpreter::new_with_sandbox(&Sandbox::allow_all());
        interp.global_env.set(
            intern("component/mount!"),
            Value::native_fn(NativeFn::simple("component/mount!", |_args| {
                Ok(Value::nil())
            })),
        );

        run_bytecode_bytes(&interp, &bytes).expect("compiled program should execute");

        let counter_view = interp
            .global_env
            .get(intern("counter-view"))
            .expect("defcomponent should define counter-view");
        let rendered = sema_eval::call_value(&interp.ctx, &counter_view, &[])
            .expect("counter-view should be callable");

        assert!(!rendered.is_nil(), "component should return SIP markup");
    }

    #[test]
    fn web_build_prelude_expands_reactive_macros() {
        let source = r#"
            (def doubled (computed 42))
            (def batched (batch 1 2 3))
        "#;

        let bytes = compile_source_to_bytecode(source).expect("compile should succeed");

        let interp = Interpreter::new_with_sandbox(&Sandbox::allow_all());
        interp.global_env.set(
            intern("__state/computed-create"),
            Value::native_fn(NativeFn::simple("__state/computed-create", |_args| {
                Ok(Value::string("computed-ok"))
            })),
        );
        interp.global_env.set(
            intern("__state/batch-run"),
            Value::native_fn(NativeFn::simple("__state/batch-run", |_args| {
                Ok(Value::string("batch-ok"))
            })),
        );

        run_bytecode_bytes(&interp, &bytes).expect("compiled program should execute");

        let doubled = interp
            .global_env
            .get(intern("doubled"))
            .expect("computed should define doubled");
        let batched = interp
            .global_env
            .get(intern("batched"))
            .expect("batch should define batched");

        assert_eq!(doubled, Value::string("computed-ok"));
        assert_eq!(batched, Value::string("batch-ok"));
    }

    // ── format_needs_auth_guidance ─────────────────────────────────────────

    #[test]
    fn needs_auth_guidance_matches_the_brief_verbatim() {
        let envelope = sema_reader::read(
            r#"{:status :needs-auth
                :servers ["asana" "linear"]
                :auth [{:server "asana" :url "https://mcp.asana.com/mcp" :persist "workflow"}
                       {:server "linear" :url "https://mcp.linear.app/mcp" :persist "workflow"}]}"#,
        )
        .expect("valid sema literal");

        let expected = concat!(
            "run needs authentication for 2 MCP server(s):\n",
            "  asana   sema mcp login https://mcp.asana.com/mcp\n",
            "  linear  sema mcp login https://mcp.linear.app/mcp\n",
            "then re-run this workflow. (or authenticate from `sema workflow view`)\n",
        );
        assert_eq!(format_needs_auth_guidance(&envelope), expected);
    }

    #[test]
    fn needs_auth_guidance_single_server() {
        let envelope = sema_reader::read(
            r#"{:status :needs-auth
                :servers ["gated"]
                :auth [{:server "gated" :url "http://127.0.0.1:1/mcp" :persist "run"}]}"#,
        )
        .expect("valid sema literal");

        let out = format_needs_auth_guidance(&envelope);
        assert!(out.starts_with("run needs authentication for 1 MCP server(s):\n"));
        assert!(out.contains("  gated  sema mcp login http://127.0.0.1:1/mcp\n"));
        assert!(out
            .ends_with("then re-run this workflow. (or authenticate from `sema workflow view`)\n"));
    }

    #[test]
    fn needs_auth_guidance_missing_auth_vector_degrades_to_zero_servers() {
        let envelope = sema_reader::read(r#"{:status :needs-auth}"#).expect("valid sema literal");
        let out = format_needs_auth_guidance(&envelope);
        assert!(out.starts_with("run needs authentication for 0 MCP server(s):\n"));
    }

    // ── should_enable_interactive_auth ─────────────────────────────────────

    #[test]
    fn interactive_auth_enabled_only_when_both_streams_are_ttys() {
        assert!(should_enable_interactive_auth(true, true, None, false));
        assert!(!should_enable_interactive_auth(false, true, None, false));
        assert!(!should_enable_interactive_auth(true, false, None, false));
        assert!(!should_enable_interactive_auth(false, false, None, false));
    }

    #[test]
    fn interactive_auth_disabled_by_no_auth_prompt_even_on_a_tty() {
        assert!(!should_enable_interactive_auth(true, true, None, true));
    }

    #[test]
    fn interactive_auth_disabled_by_nonempty_ci_even_on_a_tty() {
        assert!(!should_enable_interactive_auth(
            true,
            true,
            Some("true"),
            false
        ));
        assert!(!should_enable_interactive_auth(
            true,
            true,
            Some("1"),
            false
        ));
    }

    #[test]
    fn interactive_auth_ignores_an_empty_ci_value() {
        // `CI=` (set but empty) is treated the same as unset — matches the
        // brief's "env CI is unset/empty" wording exactly.
        assert!(should_enable_interactive_auth(true, true, Some(""), false));
    }
}
