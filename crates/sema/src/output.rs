//! Output helpers shared by the `--json`-aware subcommands.

/// Print a human-readable line unless `--json` output was requested.
macro_rules! human_output {
    ($json:expr, $($arg:tt)*) => {
        if !$json {
            println!($($arg)*);
        }
    };
}
pub(crate) use human_output;

/// Print `value` as pretty JSON on stdout.
pub(crate) fn print_json(value: &serde_json::Value) -> Result<(), String> {
    let output = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Failed to serialize package result: {error}"))?;
    println!("{output}");
    Ok(())
}
