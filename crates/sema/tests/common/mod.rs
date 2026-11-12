#![allow(dead_code)]
use sema_core::Value;
use sema_eval::Interpreter;

pub mod watchdog;

/// Embed a host filesystem path in generated Sema source, in forward-slash
/// form. Inside a Sema string literal `\` starts an escape (`C:\Users\…`
/// begins a `\U` unicode escape), so a raw Windows path mis-reads or fails to
/// parse; `/` is a valid separator on every platform (std's fs API accepts it
/// on Windows too), so the forward-slash form is safe to splice verbatim
/// between quotes. Returns BARE text — the caller supplies the quotes.
pub fn sema_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Sema string-literal-encode arbitrary text (JSON string syntax is a valid
/// Sema string literal), for splicing server scripts / marker paths into
/// generated source. Unlike `sema_path` this returns the QUOTED literal, with
/// every escape the content needs.
pub fn sema_str(s: &str) -> String {
    serde_json::to_string(s).expect("string encodes to JSON")
}

/// Evaluate Sema source on the VM (the sole evaluator), panicking on error. Used both to
/// run a test's input and to turn an expected Sema literal into a `Value`
/// (e.g. `=> common::eval("'(2 4 6)")`).
pub fn eval(input: &str) -> Value {
    let interp = Interpreter::new();
    interp
        .eval_str_compiled(input)
        .unwrap_or_else(|e| panic!("eval failed for `{input}`: {e}"))
}

/// Generate one test per case, asserting the evaluated value. Each case emits
/// ONE `test_name` function; the macro's value is pinning a literal expected value.
///
/// Usage:
/// ```ignore
/// eval_tests! {
///     test_name: "sema expression" => expected_value,
/// }
/// ```
#[macro_export]
macro_rules! eval_tests {
    ($($name:ident : $input:expr => $expected:expr),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let result = $crate::common::eval($input);
                assert_eq!(result, $expected, "VM: {}", $input);
            }
        )*
    };
}

/// Generate error tests.
///
/// Supports two per-entry forms (mix freely within one invocation):
///
/// ```ignore
/// eval_error_tests! {
///     // Strong form: assert the error message contains an expected substring
///     // (matched case-insensitively against the full Display'd error).
///     name1: "(bad-expr)" => "expected substring",
///
///     // Legacy form: only assert that evaluation errors. Prefer the strong
///     // form for new tests; keep this only when no informative substring is
///     // available (mark with a TODO so future error-UX work can revisit).
///     name2: "(other-bad)",
/// }
/// ```
#[macro_export]
macro_rules! eval_error_tests {
    // Entry point: parse a comma-separated list of mixed entries.
    ($($body:tt)*) => {
        $crate::__eval_error_tests_parse!($($body)*);
    };
}

/// Internal: recursive muncher over `name: input` and `name: input => substr` entries.
#[macro_export]
#[doc(hidden)]
macro_rules! __eval_error_tests_parse {
    // Empty
    () => {};

    // Strong form, trailing comma
    ($name:ident : $input:expr => $expected:expr , $($rest:tt)*) => {
        $crate::__eval_error_test_strong!($name, $input, $expected);
        $crate::__eval_error_tests_parse!($($rest)*);
    };
    // Strong form, no trailing comma
    ($name:ident : $input:expr => $expected:expr) => {
        $crate::__eval_error_test_strong!($name, $input, $expected);
    };

    // Legacy form, trailing comma
    ($name:ident : $input:expr , $($rest:tt)*) => {
        $crate::__eval_error_test_legacy!($name, $input);
        $crate::__eval_error_tests_parse!($($rest)*);
    };
    // Legacy form, no trailing comma
    ($name:ident : $input:expr) => {
        $crate::__eval_error_test_legacy!($name, $input);
    };
}

/// Internal: emit a strong (substring-checked) error test.
#[macro_export]
#[doc(hidden)]
macro_rules! __eval_error_test_strong {
    ($name:ident, $input:expr, $expected:expr) => {
        #[test]
        fn $name() {
            let interp = sema_eval::Interpreter::new();
            let result = interp.eval_str_compiled($input);
            let err = result.expect_err(concat!("should error for: ", stringify!($name)));
            let msg = err.to_string().to_lowercase();
            let expected = ($expected).to_lowercase();
            assert!(
                msg.contains(&expected),
                "error for `{}` did not contain `{}`\n  full error: {}",
                $input,
                $expected,
                err
            );
        }
    };
}

/// Internal: emit a legacy (existence-only) error test.
#[macro_export]
#[doc(hidden)]
macro_rules! __eval_error_test_legacy {
    ($name:ident, $input:expr) => {
        #[test]
        fn $name() {
            let interp = sema_eval::Interpreter::new();
            assert!(
                interp.eval_str_compiled($input).is_err(),
                "should error for: {}",
                $input
            );
        }
    };
}

/// A `sema` CLI command with an empty environment, for tests that must not
/// read the developer's provider keys or endpoints.
///
/// On Windows the child keeps `SystemRoot`. Without it Winsock cannot load
/// its service providers, every socket call in the child fails, and a request
/// to a loopback mock server reports only "error sending request for url".
pub fn sema_command_clean_env() -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_sema"));
    command.env_clear();
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    command
}
