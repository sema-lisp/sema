#![allow(clippy::approx_constant)]

use sema_core::Value;

// ============================================================
// Text processing
// ============================================================

eval_tests! {
    text_word_count: r#"(text/word-count "hello world foo bar")"# => Value::int(4),
    text_word_count_empty: r#"(text/word-count "")"# => Value::int(0),
    text_clean_whitespace: r#"(text/clean-whitespace "  hello   world  ")"# => Value::string("hello world"),
    text_strip_html: r#"(text/strip-html "<p>Hello <b>world</b></p>")"# => Value::string("Hello world"),
    text_strip_html_entities: r#"(text/strip-html "a &amp; b &lt; c")"# => Value::string("a & b < c"),
    text_truncate_short: r#"(text/truncate "hello" 10)"# => Value::string("hello"),
    text_truncate_long: r#"(text/truncate "hello world" 5)"# => Value::string("he..."),
    text_truncate_custom: r#"(text/truncate "hello world" 8 "…")"# => Value::string("hello w…"),
    text_split_sentences_count: r#"(length (text/split-sentences "Hello world. How are you? Fine."))"# => Value::int(3),
    text_split_sentences_empty: r#"(length (text/split-sentences ""))"# => Value::int(0),
    text_normalize_newlines: "(text/normalize-newlines \"a\\r\\nb\\rc\")" => Value::string("a\nb\nc"),
    text_excerpt: r#"(string? (text/excerpt "the quick brown fox jumps over" "fox" 5))"# => Value::bool(true),
    text_chunk_default: r#"(length (text/chunk "short text"))"# => Value::int(1),
    text_chunk_empty: r#"(length (text/chunk ""))"# => Value::int(0),
    text_chunk_separator: r#"(length (text/chunk-by-separator "a\nb\nc" "\n"))"# => Value::int(3),
}

// ============================================================
// Terminal / ANSI
// ============================================================

eval_tests! {
    term_bold: r#"(term/bold "hello")"# => Value::string("\x1b[1mhello\x1b[0m"),
    term_dim: r#"(term/dim "text")"# => Value::string("\x1b[2mtext\x1b[0m"),
    term_red: r#"(term/red "error")"# => Value::string("\x1b[31merror\x1b[0m"),
    term_green: r#"(term/green "ok")"# => Value::string("\x1b[32mok\x1b[0m"),
    term_cyan: r#"(term/cyan "info")"# => Value::string("\x1b[36minfo\x1b[0m"),
    term_gray: r#"(term/gray "muted")"# => Value::string("\x1b[90mmuted\x1b[0m"),
    // NOTE: This test depends on the exact order of ANSI SGR codes (bold=1, red=31 => "1;31").
    // If the implementation reorders style attributes, this will break. ANSI ordering IS
    // significant for some terminals, so we intentionally test the exact byte sequence.
    term_style_compound: r#"(term/style "text" :bold :red)"# => Value::string("\x1b[1;31mtext\x1b[0m"),
    term_style_plain: r#"(term/style "plain")"# => Value::string("plain"),
    term_strip: r#"(term/strip (term/bold "hello"))"# => Value::string("hello"),
    term_strip_compound: r#"(term/strip (term/style "text" :bold :red))"# => Value::string("text"),
    term_rgb: r#"(term/rgb "hi" 255 100 0)"# => Value::string("\x1b[38;2;255;100;0mhi\x1b[0m"),
    term_strip_rgb: r#"(term/strip (term/rgb "hi" 255 100 0))"# => Value::string("hi"),
}

// ============================================================
// Pretty print
// ============================================================

eval_tests! {
    pprint_returns_nil: r#"(pprint '(1 2 3))"# => Value::nil(),
}

// ============================================================
// Context operations
// ============================================================

eval_tests! {
    ctx_set_get: r#"(begin (context/set :name "alice") (context/get :name))"# => Value::string("alice"),
    ctx_get_missing: "(context/get :missing)" => Value::nil(),
    ctx_has: "(begin (context/set :x 1) (context/has? :x))" => Value::bool(true),
    ctx_has_missing: "(context/has? :nope)" => Value::bool(false),
    ctx_remove: "(begin (context/set :x 1) (context/remove :x) (context/has? :x))" => Value::bool(false),
    ctx_pull: r#"(begin (context/set :temp "val") (context/pull :temp))"# => Value::string("val"),
    ctx_with_scoped: r#"(begin (context/set :x "outer") (context/with {:x "inner"} (lambda () (context/get :x))))"# => Value::string("inner"),
    ctx_hidden: r#"(begin (context/set-hidden :secret "s3cret") (context/get-hidden :secret))"# => Value::string("s3cret"),
    ctx_hidden_not_visible: r#"(begin (context/set-hidden :secret "s3cret") (context/get :secret))"# => Value::nil(),
    ctx_stack_push: r#"(begin (context/push :trail "a") (context/push :trail "b") (length (context/stack :trail)))"# => Value::int(2),
    ctx_stack_pop: r#"(begin (context/push :trail "a") (context/push :trail "b") (context/pop :trail))"# => Value::string("b"),
    ctx_stack_empty: "(length (context/stack :empty))" => Value::int(0),
    ctx_merge: "(begin (context/merge {:a 1 :b 2}) (context/get :b))" => Value::int(2),
    ctx_clear: "(begin (context/set :x 1) (context/clear) (count (context/all)))" => Value::int(0),
}

// ============================================================
// Prompt/Message primitives
// ============================================================

eval_tests! {
    prompt_pred_false: "(prompt? 42)" => Value::bool(false),
    prompt_render: r#"(prompt/render "Hello {{name}}" {:name "Alice"})"# => Value::string("Hello Alice"),
    prompt_render_missing: r#"(prompt/render "Hello {{name}}, {{x}}." {:name "Bob"})"# => Value::string("Hello Bob, {{x}}."),
    prompt_render_number: r#"(prompt/render "Count: {{n}}" {:n 42})"# => Value::string("Count: 42"),
    prompt_render_repeated: r#"(prompt/render "{{x}} and {{x}}" {:x "hi"})"# => Value::string("hi and hi"),
    message_pred_false: "(message? 42)" => Value::bool(false),
    prompt_pred: r#"(prompt? (prompt (user "hello")))"# => Value::bool(true),
    prompt_messages_count: r#"(length (prompt/messages (prompt (user "hello") (assistant "hi"))))"# => Value::int(2),
    prompt_three_roles: r#"(length (prompt/messages (prompt (system "be helpful") (user "hello") (assistant "hi"))))"# => Value::int(3),
    prompt_append_count: r#"(length (prompt/messages (prompt/append (prompt (user "a")) (prompt (assistant "b")))))"# => Value::int(2),
    prompt_append_variadic: r#"(length (prompt/messages (prompt/append (prompt (user "a")) (prompt (user "b")) (prompt (user "c")))))"# => Value::int(3),
    prompt_concat_count: r#"(length (prompt/messages (prompt/concat (prompt (user "a")) (prompt (assistant "b")))))"# => Value::int(2),
    prompt_concat_variadic: r#"(length (prompt/messages (prompt/concat (prompt (system "s")) (prompt (user "u")) (prompt (assistant "a")))))"# => Value::int(3),
    prompt_set_system: r#"(length (prompt/messages (prompt/set-system (prompt (system "old") (user "hello")) "new")))"# => Value::int(2),
    prompt_fill_basic: r#"(message/content (car (prompt/messages (prompt/fill (prompt (user "Hello {{name}}")) {:name "Alice"}))))"# => Value::string("Hello Alice"),
    prompt_fill_missing: r#"(message/content (car (prompt/messages (prompt/fill (prompt (user "Hi {{x}}")) {:y "z"}))))"# => Value::string("Hi {{x}}"),
    prompt_fill_multi: r#"(length (prompt/messages (prompt/fill (prompt (system "You are {{role}}") (user "{{query}}")) {:role "helpful" :query "hi"})))"# => Value::int(2),
    prompt_slots_basic: r#"(length (prompt/slots (prompt (user "Hello {{name}}, age {{age}}"))))"# => Value::int(2),
    prompt_slots_empty: r#"(length (prompt/slots (prompt (user "no slots here"))))"# => Value::int(0),
    prompt_slots_dedup: r#"(length (prompt/slots (prompt (user "{{x}} and {{x}}"))))"# => Value::int(1),
    prompt_slots_multi_msg: r#"(length (prompt/slots (prompt (system "{{role}}") (user "{{query}}"))))"# => Value::int(2),
    message_pred: r#"(message? (message :user "hi"))"# => Value::bool(true),
    message_role: r#"(message/role (message :user "hi"))"# => Value::keyword("user"),
    message_content: r#"(message/content (message :user "hello world"))"# => Value::string("hello world"),
    message_from_prompt: r#"(message/content (car (prompt/messages (prompt (user "test input")))))"# => Value::string("test input"),
}

// ============================================================
// Prompt introspection + algebra (issue #12)
// ============================================================

eval_tests! {
    // diff: added/removed by (role, content)
    prompt_diff_added_len: r#"(length (:added (prompt/diff (prompt (user "a")) (prompt (user "a") (user "b")))))"# => Value::int(1),
    prompt_diff_removed_len: r#"(length (:removed (prompt/diff (prompt (user "a") (user "b")) (prompt (user "a")))))"# => Value::int(1),
    prompt_diff_added_content: r#"(message/content (car (:added (prompt/diff (prompt (user "a")) (prompt (user "b"))))))"# => Value::string("b"),
    prompt_diff_identical: r#"(length (:added (prompt/diff (prompt (user "a")) (prompt (user "a")))))"# => Value::int(0),
    // union: concat + dedup
    prompt_union_count: r#"(length (prompt/messages (prompt/union (prompt (system "s") (user "a")) (prompt (system "s") (user "b")))))"# => Value::int(3),
    // intersection: present in both
    prompt_intersection_len: r#"(length (prompt/messages (prompt/intersection (prompt (system "s") (user "a")) (prompt (system "s") (user "b")))))"# => Value::int(1),
    prompt_intersection_content: r#"(message/content (car (prompt/messages (prompt/intersection (prompt (system "s") (user "a")) (prompt (system "s") (user "b"))))))"# => Value::string("s"),
    // difference: in a but not b
    prompt_difference_len: r#"(length (prompt/messages (prompt/difference (prompt (system "s") (user "a")) (prompt (system "s")))))"# => Value::int(1),
    prompt_difference_content: r#"(message/content (car (prompt/messages (prompt/difference (prompt (system "s") (user "a")) (prompt (system "s"))))))"# => Value::string("a"),
}

// ============================================================
// Conversation
// ============================================================

eval_tests! {
    conv_pred: r#"(conversation? (conversation/new))"# => Value::bool(true),
    conv_pred_false: "(conversation? 42)" => Value::bool(false),
    conv_empty_msgs: r#"(length (conversation/messages (conversation/new)))"# => Value::int(0),
    conv_model: r#"(conversation/model (conversation/new {:model "gpt-4"}))"# => Value::string("gpt-4"),
    conv_model_empty: r#"(conversation/model (conversation/new))"# => Value::string(""),
    conv_add_msg: r#"(length (conversation/messages (conversation/add-message (conversation/new) :user "hello")))"# => Value::int(1),
    conv_fork: r#"(conversation? (conversation/fork (conversation/new)))"# => Value::bool(true),
    conv_system_nil: r#"(nil? (conversation/system (conversation/new)))"# => Value::bool(true),
    conv_system_get: r#"(conversation/system (conversation/add-message (conversation/new) :system "be helpful"))"# => Value::string("be helpful"),
    conv_set_system: r#"(conversation/system (conversation/set-system (conversation/new) "be nice"))"# => Value::string("be nice"),
    conv_set_system_replace: r#"(conversation/system (conversation/set-system (conversation/add-message (conversation/new) :system "old") "new"))"# => Value::string("new"),
    conv_set_system_preserves: r#"(length (conversation/messages (conversation/set-system (conversation/add-message (conversation/add-message (conversation/new) :system "old") :user "hi") "new")))"# => Value::int(2),
    conv_filter: r#"(length (conversation/messages (conversation/filter (-> (conversation/new) (conversation/add-message :user "hello") (conversation/add-message :assistant "hi") (conversation/add-message :user "bye")) (fn (m) (= (message/role m) :user)))))"# => Value::int(2),
    conv_filter_empty: r#"(length (conversation/messages (conversation/filter (conversation/new) (fn (m) #t))))"# => Value::int(0),
    conv_map: r#"(length (conversation/map (-> (conversation/new) (conversation/add-message :user "hello") (conversation/add-message :assistant "hi")) message/content))"# => Value::int(2),
    conv_map_content: r#"(car (conversation/map (conversation/add-message (conversation/new) :user "hello") message/content))"# => Value::string("hello"),
    conv_token_count: r#"(> (conversation/token-count (conversation/add-message (conversation/new {:model "test"}) :user "hello world this is a test")) 0)"# => Value::bool(true),
    conv_token_count_empty: r#"(conversation/token-count (conversation/new))"# => Value::int(0),
    // ---- inspection (issue #12, Part 3) ----
    conv_length: r#"(conversation/length (-> (conversation/new) (conversation/add-message :user "a") (conversation/add-message :assistant "b")))"# => Value::int(2),
    conv_turns: r#"(conversation/turns (-> (conversation/new) (conversation/add-message :user "a") (conversation/add-message :assistant "b")))"# => Value::int(1),
    conv_turns_zero: r#"(conversation/turns (conversation/new))"# => Value::int(0),
    conv_models_used: r#"(car (conversation/models-used (conversation/new {:model "gpt-4"})))"# => Value::string("gpt-4"),
    conv_models_used_empty: r#"(length (conversation/models-used (conversation/new)))"# => Value::int(0),
    conv_stats_messages: r#"(:messages (conversation/stats (conversation/add-message (conversation/new) :user "hi")))"# => Value::int(1),
    conv_stats_turns: r#"(:turns (conversation/stats (-> (conversation/new) (conversation/add-message :user "a") (conversation/add-message :assistant "b"))))"# => Value::int(1),
    conv_stats_cost_nil: r#"(nil? (:cost (conversation/stats (conversation/new {:model "gpt-4"}))))"# => Value::bool(true),
    conv_stats_tokens_total: r#"(:total (:tokens (conversation/stats (conversation/new))))"# => Value::int(0),
    // cost reports real billed usage only — nil when nothing has been sent (no estimation)
    conv_cost_nil: r#"(nil? (conversation/cost (conversation/new {:model "gpt-4"})))"# => Value::bool(true),
    // ---- surgery (issue #12, Part 3) ----
    conv_remove_count: r#"(conversation/length (conversation/remove (-> (conversation/new) (conversation/add-message :user "a") (conversation/add-message :assistant "b")) 0))"# => Value::int(1),
    conv_remove_content: r#"(message/content (car (conversation/messages (conversation/remove (-> (conversation/new) (conversation/add-message :user "a") (conversation/add-message :assistant "b")) 0))))"# => Value::string("b"),
    conv_insert_count: r#"(conversation/length (conversation/insert (conversation/add-message (conversation/new) :user "a") 0 :system "s"))"# => Value::int(2),
    conv_insert_append: r#"(message/content (car (conversation/messages (conversation/insert (conversation/new) 0 :user "hi"))))"# => Value::string("hi"),
    conv_insert_msg_value: r#"(message/role (car (conversation/messages (conversation/insert (conversation/new) 0 (message :system "s")))))"# => Value::keyword("system"),
    conv_replace_content: r#"(message/content (car (conversation/messages (conversation/replace (conversation/add-message (conversation/new) :user "a") 0 :user "A"))))"# => Value::string("A"),
    conv_map_role_applied: r#"(car (conversation/map (conversation/map-role (conversation/add-message (conversation/new) :assistant "  y  ") :assistant (fn (m) (message :assistant (string/trim (message/content m))))) message/content))"# => Value::string("y"),
    conv_map_role_untouched: r#"(car (conversation/map (conversation/map-role (conversation/add-message (conversation/new) :user "keep") :assistant (fn (m) (message :assistant "X"))) message/content))"# => Value::string("keep"),
    // ---- search (issue #12, Part 3) ----
    conv_search_count: r#"(length (conversation/search (-> (conversation/new) (conversation/add-message :user "About Lisp") (conversation/add-message :assistant "Lisp rocks")) "lisp"))"# => Value::int(2),
    conv_search_index: r#"(:index (car (conversation/search (conversation/add-message (conversation/new) :user "hello") "hello")))"# => Value::int(0),
    conv_search_none: r#"(length (conversation/search (conversation/add-message (conversation/new) :user "hello") "zzz"))"# => Value::int(0),
    conv_find: r#"(message/content (conversation/find (-> (conversation/new) (conversation/add-message :user "u") (conversation/add-message :assistant "a")) (fn (m) (= (message/role m) :assistant))))"# => Value::string("a"),
    conv_find_none: r#"(nil? (conversation/find (conversation/new) (fn (m) #t)))"# => Value::bool(true),
}

eval_error_tests! {
    conv_remove_oob: r#"(conversation/remove (conversation/new) 0)"#,
    conv_replace_oob: r#"(conversation/replace (conversation/new) 5 :user "x")"#,
    conv_insert_oob: r#"(conversation/insert (conversation/new) 5 :user "x")"#,
    conv_map_role_bad_role: r#"(conversation/map-role (conversation/new) :nope (fn (m) m))"#,
}

// ============================================================
// Document operations
// ============================================================

eval_tests! {
    doc_text: r#"(document/text (document/create "hello" {:source "x"}))"# => Value::string("hello"),
    doc_metadata_source: r#"(get (document/metadata (document/create "hello" {:source "x"})) :source)"# => Value::string("x"),
}

// ============================================================
// Tool/Agent definitions
// ============================================================

eval_tests! {
    tool_pred: r#"(begin (deftool add-numbers "Add" {:a {:type :number}} (lambda (a) a)) (tool? add-numbers))"# => Value::bool(true),
    tool_name: r#"(begin (deftool add-numbers "Add" {:a {:type :number}} (lambda (a) a)) (tool/name add-numbers))"# => Value::string("add-numbers"),
    tool_desc: r#"(begin (deftool add-numbers "Add two" {:a {:type :number}} (lambda (a) a)) (tool/description add-numbers))"# => Value::string("Add two"),
    agent_pred: r#"(begin (deftool greet "Greet" {:name {:type :string}} (lambda (name) name)) (defagent greeter {:system "You greet." :tools [greet]}) (agent? greeter))"# => Value::bool(true),
    agent_name: r#"(begin (deftool greet "Greet" {:name {:type :string}} (lambda (name) name)) (defagent greeter {:system "You greet." :tools [greet]}) (agent/name greeter))"# => Value::string("greeter"),
    agent_system: r#"(begin (deftool greet "Greet" {:name {:type :string}} (lambda (name) name)) (defagent greeter {:system "You greet." :tools [greet]}) (agent/system greeter))"# => Value::string("You greet."),
    tool_invoke_ping: r#"(begin (deftool ping "Return pong" {} (lambda () "pong")) (tool/invoke ping {}))"# => Value::string("pong"),
    tool_invoke_typed: r#"(begin (deftool add-numbers "Add" {:a {:type :number} :b {:type :number}} (lambda (a b) (+ a b))) (tool/invoke add-numbers {:a 2 :b 3}))"# => Value::int(5),
}

eval_error_tests! {
    tool_invoke_invalid_args: r#"(begin (deftool calc "Add" {:x {:type :number}} (lambda (x) x)) (tool/invoke calc {}))"# => "invalid arguments for tool 'calc': missing key: x",
}

// ============================================================
// LLM utility functions (no API calls)
// ============================================================

eval_tests! {
    llm_token_count_empty: r#"(llm/token-count "")"# => Value::int(0),
    llm_similarity_identical: "(llm/similarity '(1.0 0.0 0.0) '(1.0 0.0 0.0))" => Value::float(1.0),
    llm_similarity_orthogonal: "(llm/similarity '(1.0 0.0) '(0.0 1.0))" => Value::float(0.0),
    llm_reset_usage: "(llm/reset-usage)" => Value::nil(),
    llm_set_pricing: r#"(llm/set-pricing "my-model" 1.0 2.0)"# => Value::nil(),
}

// ============================================================
// Retry
// ============================================================

eval_tests! {
    retry_basic: r#"(retry (lambda () 42))"# => Value::int(42),
    retry_with_opts: r#"(retry (lambda () 42) {:max-attempts 3})"# => Value::int(42),
}

// ============================================================
// Log (side-effect, returns nil)
// ============================================================

eval_tests! {
    log_info: r#"(log/info "hello")"# => Value::nil(),
    log_warn: r#"(log/warn "caution")"# => Value::nil(),
}

// ============================================================
// Datetime
// ============================================================

// All tests use fixed timestamps via time/parse to avoid flakiness.
// Unix timestamp 1704067200 = 2024-01-01 00:00:00 UTC

eval_tests! {
    // time/parse — basic parsing
    time_parse_basic: r#"(time/parse "2024-01-01 00:00:00" "%Y-%m-%d %H:%M:%S")"# => Value::float(1704067200.0),
    time_parse_epoch: r#"(time/parse "1970-01-01 00:00:00" "%Y-%m-%d %H:%M:%S")"# => Value::float(0.0),
    // Naive (offset-less) strings are interpreted as UTC. 2025-01-15 12:10:00 UTC
    // == unix 1736943000. This pins the documented UTC interpretation so it can't
    // silently regress to local time.
    time_parse_naive_is_utc: r#"(time/parse "2025-01-15 12:10:00" "%Y-%m-%d %H:%M:%S")"# => Value::float(1736943000.0),

    // time/format — basic formatting
    time_format_date: r#"(time/format 1704067200.0 "%Y-%m-%d")"# => Value::string("2024-01-01"),
    time_format_time: r#"(time/format 1704067200.0 "%H:%M:%S")"# => Value::string("00:00:00"),
    time_format_full: r#"(time/format 1704067200.0 "%Y-%m-%d %H:%M:%S")"# => Value::string("2024-01-01 00:00:00"),
    time_format_epoch: r#"(time/format 0.0 "%Y-%m-%d")"# => Value::string("1970-01-01"),

    // time/parse → time/format roundtrip
    time_roundtrip: r#"(time/format (time/parse "2024-06-15 14:30:00" "%Y-%m-%d %H:%M:%S") "%Y-%m-%d %H:%M:%S")"# => Value::string("2024-06-15 14:30:00"),

    // time/date-parts — basic extraction
    time_parts_year: r#"(get (time/date-parts 1704067200.0) :year)"# => Value::int(2024),
    time_parts_month: r#"(get (time/date-parts 1704067200.0) :month)"# => Value::int(1),
    time_parts_day: r#"(get (time/date-parts 1704067200.0) :day)"# => Value::int(1),
    time_parts_hour: r#"(get (time/date-parts 1704067200.0) :hour)"# => Value::int(0),
    time_parts_minute: r#"(get (time/date-parts 1704067200.0) :minute)"# => Value::int(0),
    time_parts_second: r#"(get (time/date-parts 1704067200.0) :second)"# => Value::int(0),
    time_parts_weekday: r#"(get (time/date-parts 1704067200.0) :weekday)"# => Value::string("Monday"),

    // time/add — basic addition
    time_add_one_second: r#"(time/format (time/add (time/parse "2024-01-01 00:00:00" "%Y-%m-%d %H:%M:%S") 1) "%Y-%m-%d %H:%M:%S")"# => Value::string("2024-01-01 00:00:01"),
    time_add_one_hour: r#"(time/format (time/add (time/parse "2024-01-01 00:00:00" "%Y-%m-%d %H:%M:%S") 3600) "%Y-%m-%d %H:%M:%S")"# => Value::string("2024-01-01 01:00:00"),
    time_add_one_day: r#"(time/format (time/add (time/parse "2024-01-01 00:00:00" "%Y-%m-%d %H:%M:%S") 86400) "%Y-%m-%d %H:%M:%S")"# => Value::string("2024-01-02 00:00:00"),
    time_add_negative: r#"(time/format (time/add (time/parse "2024-01-02 00:00:00" "%Y-%m-%d %H:%M:%S") -86400) "%Y-%m-%d %H:%M:%S")"# => Value::string("2024-01-01 00:00:00"),

    // time/diff — difference between timestamps
    time_diff_basic: r#"(time/diff (time/parse "2024-01-02 00:00:00" "%Y-%m-%d %H:%M:%S") (time/parse "2024-01-01 00:00:00" "%Y-%m-%d %H:%M:%S"))"# => Value::float(86400.0),
    time_diff_negative: r#"(time/diff (time/parse "2024-01-01 00:00:00" "%Y-%m-%d %H:%M:%S") (time/parse "2024-01-02 00:00:00" "%Y-%m-%d %H:%M:%S"))"# => Value::float(-86400.0),
    time_diff_zero: r#"(time/diff (time/parse "2024-01-01 00:00:00" "%Y-%m-%d %H:%M:%S") (time/parse "2024-01-01 00:00:00" "%Y-%m-%d %H:%M:%S"))"# => Value::float(0.0),

    // === Edge cases ===

    // Leap year: Feb 29 exists in 2024
    time_leap_year_feb29: r#"(get (time/date-parts (time/parse "2024-02-29 12:00:00" "%Y-%m-%d %H:%M:%S")) :day)"# => Value::int(29),
    time_leap_year_feb29_weekday: r#"(get (time/date-parts (time/parse "2024-02-29 00:00:00" "%Y-%m-%d %H:%M:%S")) :weekday)"# => Value::string("Thursday"),

    // Midnight boundary: 1 second before midnight → next day
    time_midnight_rollover: r#"(time/format (time/add (time/parse "2024-02-29 23:59:59" "%Y-%m-%d %H:%M:%S") 1) "%Y-%m-%d %H:%M:%S")"# => Value::string("2024-03-01 00:00:00"),
    // Non-leap year: Feb 28 → Mar 1 on rollover
    time_non_leap_feb28_rollover: r#"(time/format (time/add (time/parse "2023-02-28 23:59:59" "%Y-%m-%d %H:%M:%S") 1) "%Y-%m-%d %H:%M:%S")"# => Value::string("2023-03-01 00:00:00"),

    // Year boundary: Dec 31 23:59:59 → Jan 1
    time_year_boundary: r#"(time/format (time/add (time/parse "2024-12-31 23:59:59" "%Y-%m-%d %H:%M:%S") 1) "%Y-%m-%d %H:%M:%S")"# => Value::string("2025-01-01 00:00:00"),

    // Negative timestamps (before Unix epoch)
    time_before_epoch: r#"(time/format (time/parse "1969-12-31 23:59:59" "%Y-%m-%d %H:%M:%S") "%Y-%m-%d")"# => Value::string("1969-12-31"),
    time_before_epoch_parts: r#"(get (time/date-parts (time/parse "1969-07-20 20:17:00" "%Y-%m-%d %H:%M:%S")) :year)"# => Value::int(1969),

    // DST-insensitive (UTC only, but test midday/midnight distinction)
    time_midday: r#"(get (time/date-parts (time/parse "2024-03-10 12:00:00" "%Y-%m-%d %H:%M:%S")) :hour)"# => Value::int(12),
    time_end_of_day: r#"(get (time/date-parts (time/parse "2024-06-15 23:59:59" "%Y-%m-%d %H:%M:%S")) :second)"# => Value::int(59),

    // Century leap year: 2000 is leap, 1900 is not
    time_century_leap_2000: r#"(get (time/date-parts (time/parse "2000-02-29 00:00:00" "%Y-%m-%d %H:%M:%S")) :day)"# => Value::int(29),

    // Large time addition (365 days)
    time_add_year: r#"(time/format (time/add (time/parse "2024-01-01 00:00:00" "%Y-%m-%d %H:%M:%S") 31622400) "%Y-%m-%d %H:%M:%S")"# => Value::string("2025-01-01 00:00:00"),
}

// time/now — can only check it returns a reasonable positive float
eval_tests! {
    time_now_positive: "(> (time/now) 0)" => Value::bool(true),
    time_now_is_float: "(float? (time/now))" => Value::bool(true),
    // time/now should be after 2024-01-01 (1704067200)
    time_now_recent: "(> (time/now) 1704067200)" => Value::bool(true),
}

// time/parse error cases
eval_error_tests! {
    time_parse_invalid_format: r#"(time/parse "not-a-date" "%Y-%m-%d")"#,
    time_parse_wrong_type: r#"(time/parse 42 "%Y-%m-%d")"#,
    time_format_wrong_type: r#"(time/format "not-a-number" "%Y-%m-%d")"#,
    // Non-leap year Feb 29 should fail
    time_non_leap_feb29: r#"(time/parse "2023-02-29 00:00:00" "%Y-%m-%d %H:%M:%S")"#,
}

// ============================================================
// HTTP response helpers
// ============================================================

eval_tests! {
    // http/ok
    http_ok_string_status: r#"(get (http/ok "hi") :status)"# => Value::int(200),
    http_ok_map_status: r#"(get (http/ok {:a 1}) :status)"# => Value::int(200),
    http_ok_list_status: r#"(get (http/ok '(1 2 3)) :status)"# => Value::int(200),
    http_ok_nil_status: r#"(get (http/ok nil) :status)"# => Value::int(200),
    http_ok_int_status: r#"(get (http/ok 42) :status)"# => Value::int(200),
    http_ok_has_body: r#"(string? (get (http/ok "test") :body))"# => Value::bool(true),
    http_ok_content_type: r#"(get (get (http/ok "x") :headers) "content-type")"# => Value::string("application/json"),

    // http/created
    http_created_status: r#"(get (http/created {:id 1}) :status)"# => Value::int(201),
    http_created_content_type: r#"(get (get (http/created "x") :headers) "content-type")"# => Value::string("application/json"),

    // http/no-content
    http_no_content_status: r#"(get (http/no-content) :status)"# => Value::int(204),
    http_no_content_empty_body: r#"(get (http/no-content) :body)"# => Value::string(""),

    // http/not-found
    http_not_found_status: r#"(get (http/not-found "gone") :status)"# => Value::int(404),

    // http/error
    http_error_custom: r#"(get (http/error 422 "bad") :status)"# => Value::int(422),
    http_error_500: r#"(get (http/error 500 "oops") :status)"# => Value::int(500),
    http_error_418: r#"(get (http/error 418 "teapot") :status)"# => Value::int(418),

    // http/redirect
    http_redirect_status: r#"(get (http/redirect "/login") :status)"# => Value::int(302),
    http_redirect_location: r#"(get (get (http/redirect "/login") :headers) "location")"# => Value::string("/login"),
    http_redirect_absolute: r#"(get (get (http/redirect "https://example.com") :headers) "location")"# => Value::string("https://example.com"),

    // http/html
    http_html_status: r#"(get (http/html "<p>hi</p>") :status)"# => Value::int(200),
    http_html_content_type: r#"(get (get (http/html "<p>hi</p>") :headers) "content-type")"# => Value::string("text/html"),
    http_html_body: r#"(get (http/html "<h1>Hello</h1>") :body)"# => Value::string("<h1>Hello</h1>"),

    // http/text
    http_text_status: r#"(get (http/text "plain") :status)"# => Value::int(200),
    http_text_content_type: r#"(get (get (http/text "plain") :headers) "content-type")"# => Value::string("text/plain"),
    http_text_body: r#"(get (http/text "hello world") :body)"# => Value::string("hello world"),
}

// ============================================================
// String operations
// ============================================================

eval_tests! {
    string_length_basic: r#"(string-length "hello")"# => Value::int(5),
    string_length_empty: r#"(string-length "")"# => Value::int(0),
    substring_basic: r#"(substring "hello" 1 3)"# => Value::string("el"),
    string_append_two: r#"(string-append "foo" "bar")"# => Value::string("foobar"),
    string_trim_spaces: r#"(string/trim "  hi  ")"# => Value::string("hi"),
    string_upper: r#"(string/upper "hello")"# => Value::string("HELLO"),
    string_lower: r#"(string/lower "HELLO")"# => Value::string("hello"),
    string_replace_basic: r#"(string/replace "hello world" "world" "sema")"# => Value::string("hello sema"),
    string_join_list: r#"(string/join (list "a" "b" "c") ",")"# => Value::string("a,b,c"),
    string_to_number_int: r#"(string->number "42")"# => Value::int(42),
    string_to_number_float: r#"(string->number "3.14")"# => Value::float(3.14),
}

// ============================================================
// Math operations
// ============================================================

eval_tests! {
    math_abs_negative: "(abs -5)" => Value::int(5),
    math_abs_positive: "(abs 5)" => Value::int(5),
    // floor/round are exactness-preserving (R7RS): a float argument rounds to a
    // float, not an int (see plan Task 5.2).
    math_floor_basic: "(floor 3.7)" => Value::float(3.0),
    // sqrt of a perfect square is exact (R7RS): (sqrt 9) => 3, not 3.0.
    math_sqrt_basic: "(sqrt 9)" => Value::int(3),
    math_min_two: "(min 3 7)" => Value::int(3),
    math_max_two: "(max 3 7)" => Value::int(7),
    math_round_basic: "(round 3.6)" => Value::float(4.0),
    math_round_down: "(round 3.2)" => Value::float(3.0),
}

eval_error_tests! {
    http_ok_no_args: "(http/ok)",
    http_ok_too_many: r#"(http/ok "a" "b")"#,
    http_created_no_args: "(http/created)",
    http_not_found_no_args: "(http/not-found)",
    http_redirect_no_args: "(http/redirect)",
    http_redirect_non_string: "(http/redirect 123)",
    http_error_one_arg: r#"(http/error 422)"#,
    http_error_non_int_status: r#"(http/error "nope" "body")"#,
    http_html_non_string: "(http/html 123)",
    http_text_non_string: "(http/text 123)",
    http_no_content_extra: r#"(http/no-content "extra")"#,
}
