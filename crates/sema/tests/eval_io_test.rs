mod common;

use sema_core::Value;

// ============================================================
// Path operations (pure string manipulation)
// ============================================================

eval_tests! {
    path_join: r#"(path/join "usr" "local" "bin")"# => Value::string("usr/local/bin"),
    path_dirname: r#"(path/dirname "/a/b/c")"# => Value::string("/a/b"),
    path_basename: r#"(path/basename "/a/b/c.txt")"# => Value::string("c.txt"),
    path_extension: r#"(path/extension "foo.txt")"# => Value::string("txt"),
    path_extension_none: r#"(path/extension "no-ext")"# => Value::string(""),
    path_extension_multi_dot: r#"(path/extension "file.tar.gz")"# => Value::string("gz"),
    path_extension_makefile: r#"(path/extension "Makefile")"# => Value::string(""),
    path_ext_alias: r#"(path/ext "Makefile")"# => Value::string(""),
    path_stem: r#"(path/stem "foo.txt")"# => Value::string("foo"),
    path_filename: r#"(path/filename "/a/b/c.txt")"# => Value::string("c.txt"),
    path_basename_alias: r#"(path/basename "/a/b/c.txt")"# => Value::string("c.txt"),
    path_dir: r#"(path/dir "/a/b/c.txt")"# => Value::string("/a/b"),
    path_dir_no_parent: r#"(path/dir "foo")"# => Value::string(""),
    path_dirname_alias_no_parent: r#"(path/dirname "x")"# => Value::string(""),
    path_dirname_alias: r#"(path/dirname "/a/b/c.txt")"# => Value::string("/a/b"),
    path_absolute_is_string: r#"(string? (path/absolute "."))"# => Value::bool(true),
    path_is_absolute: r#"(path/absolute? "/usr/bin")"# => Value::bool(true),
    path_not_absolute: r#"(path/absolute? "relative/path")"# => Value::bool(false),
    path_join_multi: r#"(path/join "a" "b" "c" "d")"# => Value::string("a/b/c/d"),
}

// ============================================================
// System operations
// ============================================================

eval_tests! {
    sys_os: r#"(string? (sys/os))"# => Value::bool(true),
    sys_arch: r#"(string? (sys/arch))"# => Value::bool(true),
    sys_platform: r#"(string? (sys/platform))"# => Value::bool(true),
    sys_cwd: r#"(string? (sys/cwd))"# => Value::bool(true),
    sys_home: r#"(string? (sys/home-dir))"# => Value::bool(true),
    sys_hostname: r#"(string? (sys/hostname))"# => Value::bool(true),
    sys_pid: "(integer? (sys/pid))" => Value::bool(true),
    sys_user: r#"(string? (sys/user))"# => Value::bool(true),
    sys_temp: r#"(string? (sys/temp-dir))"# => Value::bool(true),
    sys_args: "(list? (sys/args))" => Value::bool(true),
    sys_env_all: "(map? (sys/env-all))" => Value::bool(true),
    sys_elapsed: "(>= (sys/elapsed) 0)" => Value::bool(true),
    // sys/term-size returns nil when not a TTY (test environment has no TTY)
    sys_term_size_nil_or_map: "(let ((ts (sys/term-size))) (or (nil? ts) (map? ts)))" => Value::bool(true),
    sys_check_signals_noop: "(nil? (sys/check-signals))" => Value::bool(true),
}

// Unix-only: sys/which assumes `sh` exists on PATH
#[cfg(unix)]
eval_tests! {
    sys_which_test: r#"(string? (sys/which "sh"))"# => Value::bool(true),
}

// ============================================================
// Time operations (deterministic subset)
// ============================================================

eval_tests! {
    time_format_epoch: r#"(time/format 0.0 "%Y-%m-%d")"# => Value::string("1970-01-01"),
    time_format_hms: r#"(time/format 0.0 "%H:%M:%S")"# => Value::string("00:00:00"),
    time_now_positive: "(> (time/now) 1700000000.0)" => Value::bool(true),
    time_ms_positive: "(> (time-ms) 0)" => Value::bool(true),
    time_add_basic: r#"(time/format (time/add 0.0 86400) "%Y-%m-%d")"# => Value::string("1970-01-02"),
    time_diff: "(> (time/diff 100.0 0.0) 0.0)" => Value::bool(true),
    time_date_parts: "(get (time/date-parts 0.0) :year)" => Value::int(1970),
}

// Invalid chrono format strings must raise a SemaError, not abort the process
// (chrono's DelayedFormat panics inside .to_string() on bad specifiers).
eval_error_tests! {
    time_format_err_lone_percent: r#"(time/format 0.0 "%")"# => "time/format: invalid format string",
    time_format_err_bad_specifier: r#"(time/format 0.0 "%Q")"# => "time/format: invalid format string",
    time_format_err_bad_padding: r#"(time/format 0.0 "%-8")"# => "time/format: invalid format string",
}

// ============================================================
// Env operations
// ============================================================

eval_tests! {
    env_path_exists: r#"(string? (env "PATH"))"# => Value::bool(true),
    env_missing: r#"(env "SEMA_NONEXISTENT_VAR_XYZ_12345")"# => Value::nil(),
}

// ============================================================
// File operations (self-contained with cleanup)
// ============================================================

eval_tests! {
    file_write_read: r#"(begin (define p (string-append (sys/temp-dir) "/sema-de-wr-" (uuid/v4))) (file/write p "hello") (let ((r (file/read p))) (file/delete p) r))"# => Value::string("hello"),
    file_append: r#"(begin (define p (string-append (sys/temp-dir) "/sema-de-ap-" (uuid/v4))) (file/write p "hello") (file/append p " world") (let ((r (file/read p))) (file/delete p) r))"# => Value::string("hello world"),
    file_exists_true: r#"(begin (define p (string-append (sys/temp-dir) "/sema-de-ex-" (uuid/v4))) (file/write p "x") (let ((r (file/exists? p))) (file/delete p) r))"# => Value::bool(true),
    file_exists_false: r#"(file/exists? "/tmp/sema-nonexistent-xyz-12345")"# => Value::bool(false),
    file_is_file: r#"(begin (define p (string-append (sys/temp-dir) "/sema-de-if-" (uuid/v4))) (file/write p "x") (let ((r (file/is-file? p))) (file/delete p) r))"# => Value::bool(true),
    file_is_dir: r#"(file/is-directory? (sys/temp-dir))"# => Value::bool(true),
    file_read_lines: r#"(begin (define p (string-append (sys/temp-dir) "/sema-de-rl-" (uuid/v4))) (file/write p "a\nb\nc") (let ((r (length (file/read-lines p)))) (file/delete p) r))"# => Value::int(3),
    file_info_is_map: r#"(begin (define p (string-append (sys/temp-dir) "/sema-de-fi-" (uuid/v4))) (file/write p "x") (let ((r (map? (file/info p)))) (file/delete p) r))"# => Value::bool(true),
}

// Unix-only: file_mkdir cleanup uses `rm -rf` via shell
#[cfg(unix)]
eval_tests! {
    file_mkdir: r#"(begin (define p (string-append (sys/temp-dir) "/sema-de-md-" (uuid/v4))) (file/mkdir p) (let ((r (file/is-directory? p))) (shell (string-append "rm -rf " p)) r))"# => Value::bool(true),
}

// ============================================================
// Shell command (Unix-only: `echo` is a shell builtin on Windows)
// ============================================================

#[cfg(unix)]
eval_tests! {
    shell_echo: r#"(map? (shell "echo hello"))"# => Value::bool(true),
}

// ============================================================
// Math extended
// ============================================================

eval_tests! {
    math_exp: "(> (math/exp 1.0) 2.7)" => Value::bool(true),
    math_pow_ns: "(math/pow 2.0 10.0)" => Value::float(1024.0),
    math_lerp: "(math/lerp 0.0 10.0 0.5)" => Value::float(5.0),
    math_map_range: "(math/map-range 5.0 0.0 10.0 0.0 100.0)" => Value::float(50.0),
    math_random_range: "(let ((r (math/random))) (and (>= r 0.0) (< r 1.0)))" => Value::bool(true),
}

// ============================================================
// Misc stdlib
// ============================================================

eval_tests! {
    tap_returns_value: "(tap 42 (lambda (x) nil))" => Value::int(42),
    str_number_pred: r#"(string/number? "42")"# => Value::bool(true),
    str_number_pred_false: r#"(string/number? "abc")"# => Value::bool(false),
    error_builtin: r#"(try (error "boom") (catch e (string? (get e :message))))"# => Value::bool(true),
}

// ============================================================
// String case & manipulation functions
// ============================================================

eval_tests! {
    str_camel: r#"(string/camel-case "hello-world")"# => Value::string("helloWorld"),
    str_snake: r#"(string/snake-case "helloWorld")"# => Value::string("hello_world"),
    str_kebab: r#"(string/kebab-case "helloWorld")"# => Value::string("hello-world"),
    str_pascal: r#"(string/pascal-case "hello-world")"# => Value::string("HelloWorld"),
    str_title: r#"(string/title-case "hello world")"# => Value::string("Hello World"),
    str_headline: r#"(string/headline "hello_world")"# => Value::string("Hello World"),
    str_words: r#"(length (string/words "hello world foo"))"# => Value::int(3),
    str_take: r#"(string/take "hello" 3)"# => Value::string("hel"),
    str_byte_length: r#"(string/byte-length "hello")"# => Value::int(5),
    str_codepoints: r#"(length (string/codepoints "hello"))"# => Value::int(5),
    str_remove: r#"(string/remove "hello world" "world")"# => Value::string("hello "),
    str_before: r#"(string/before "hello-world" "-")"# => Value::string("hello"),
    str_after: r#"(string/after "hello-world" "-")"# => Value::string("world"),
    str_between: r#"(string/between "[hello]" "[" "]")"# => Value::string("hello"),
    str_ensure_start: r#"(string/ensure-start "world" "hello ")"# => Value::string("hello world"),
    str_ensure_end: r#"(string/ensure-end "hello" " world")"# => Value::string("hello world"),
    str_chop_start: r#"(string/chop-start "hello world" "hello ")"# => Value::string("world"),
    str_chop_end: r#"(string/chop-end "hello world" " world")"# => Value::string("hello"),
    str_wrap: r#"(string/wrap "hello" "[" "]")"# => Value::string("[hello]"),
    str_unwrap: r#"(string/unwrap "[hello]" "[" "]")"# => Value::string("hello"),
}

// ============================================================
// New interactive-CLI functions
// ============================================================

eval_tests! {
    // io/flush: should return nil and not error
    io_flush_returns_nil: "(nil? (io/flush))" => Value::bool(true),
    // io/eof? starts false in a normal eval context
    io_eof_initially_false: "(io/eof?)" => Value::bool(false),
    // io/tty-raw! returns nil (not a TTY) or an integer token (is a TTY); if a token is returned,
    // restore it so we don't leave the terminal in raw mode.
    io_tty_raw_returns_nil_or_int: "(let ((tok (io/tty-raw!))) (if (nil? tok) #t (begin (io/tty-restore! tok) #t)))" => Value::bool(true),
    // io/read-key-timeout with 0ms returns nil immediately (no key available)
    io_read_key_timeout_zero: "(nil? (io/read-key-timeout 0))" => Value::bool(true),
}
