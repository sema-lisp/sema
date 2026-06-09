# Lambda/Macro Params Vec<Spur> Cleanup

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Change `Lambda.params` from `Vec<String>` to `Vec<Spur>`, `Lambda.rest_param` from `Option<String>` to `Option<Spur>`, `Lambda.name` from `Option<String>` to `Option<Spur>`, and the same for `Macro.params`/`rest_param`/`name`. This eliminates per-call `intern()` overhead and prepares for the bytecode VM's reify approach.

**Architecture:** Pure refactor — change struct field types in `sema-core`, update `parse_params` to work with `Spur`, update all construction sites to intern at creation time, update all consumption sites to use `Spur` directly (no more `intern(param)`). The `name` fields also become `Spur` since they're used as env keys and for error messages (resolve on demand).

**Tech Stack:** Rust, lasso string interner (`Spur`), 6-crate workspace.

---

### Task 1: Change Lambda and Macro struct fields in sema-core

**Files:**

- Modify: `crates/sema-core/src/value.rs:87-102`

**Step 1: Change struct field types**

Change `Lambda` struct (line 87-93) from:

```rust
pub struct Lambda {
    pub params: Vec<String>,
    pub rest_param: Option<String>,
    pub body: Vec<Value>,
    pub env: Env,
    pub name: Option<String>,
}
```

to:

```rust
pub struct Lambda {
    pub params: Vec<Spur>,
    pub rest_param: Option<Spur>,
    pub body: Vec<Value>,
    pub env: Env,
    pub name: Option<Spur>,
}
```

Change `Macro` struct (line 96-102) from:

```rust
pub struct Macro {
    pub params: Vec<String>,
    pub rest_param: Option<String>,
    pub body: Vec<Value>,
    pub name: String,
}
```

to:

```rust
pub struct Macro {
    pub params: Vec<Spur>,
    pub rest_param: Option<Spur>,
    pub body: Vec<Value>,
    pub name: Spur,
}
```

**Step 2: Update Lambda Display impl**

In `value.rs` Display impl for Value (around line 334-339), the Lambda display uses `lambda.name`:

```rust
Value::Lambda(l) => {
    if let Some(name) = &l.name {
        write!(f, "<lambda {name}>")
    } else {
        write!(f, "<lambda>")
    }
}
```

Change to:

```rust
Value::Lambda(l) => {
    if let Some(name) = &l.name {
        with_resolved(*name, |n| write!(f, "<lambda {n}>"))
    } else {
        write!(f, "<lambda>")
    }
}
```

The Macro display (line 341) `write!(f, "<macro {}>", m.name)` changes to:

```rust
Value::Macro(m) => with_resolved(m.name, |n| write!(f, "<macro {n}>")),
```

**Step 3: Verify it compiles (expect errors in downstream crates)**

Run: `cargo check -p sema-core 2>&1 | head -20`
Expected: sema-core compiles. Downstream crates will fail (that's Task 2-4).

---

### Task 2: Update parse_params and all Lambda/Macro construction in sema-eval special_forms.rs

**Files:**

- Modify: `crates/sema-eval/src/special_forms.rs`

**Step 1: Change `parse_params` to work with `Spur`**

Change `parse_params` (line 1617-1629) from:

```rust
fn parse_params(names: &[String]) -> (Vec<String>, Option<String>) {
    if let Some(pos) = names.iter().position(|s| s == ".") {
        let params = names[..pos].to_vec();
        let rest = if pos + 1 < names.len() {
            Some(names[pos + 1].clone())
        } else {
            None
        };
        (params, rest)
    } else {
        (names.to_vec(), None)
    }
}
```

to:

```rust
fn parse_params(names: &[Spur]) -> (Vec<Spur>, Option<Spur>) {
    let dot = intern(".");
    if let Some(pos) = names.iter().position(|s| *s == dot) {
        let params = names[..pos].to_vec();
        let rest = if pos + 1 < names.len() {
            Some(names[pos + 1])
        } else {
            None
        };
        (params, rest)
    } else {
        (names.to_vec(), None)
    }
}
```

**Step 2: Update `eval_define` (line 245-293)**

The function-form `(define (f x y) body...)` at line 268-276 builds `Vec<String>` params. Change:

```rust
let param_names: Vec<String> = sig[1..]
    .iter()
    .map(|v| {
        v.as_symbol()
            .map(|s| s.to_string())
            .ok_or_else(|| SemaError::eval("define: parameter must be a symbol"))
    })
    .collect::<Result<_, _>>()?;
```

to:

```rust
let param_names: Vec<Spur> = sig[1..]
    .iter()
    .map(|v| match v {
        Value::Symbol(s) => Ok(*s),
        _ => Err(SemaError::eval("define: parameter must be a symbol")),
    })
    .collect::<Result<_, _>>()?;
```

Also update the `name` field. Line 267 has `let name = ... .to_string()`. Remove that, extract the Spur directly:

```rust
let name_spur = match &sig[0] {
    Value::Symbol(s) => *s,
    _ => return Err(SemaError::eval("define: function name must be a symbol")),
};
```

Then update Lambda construction (line 281-287):

```rust
let lambda = Value::Lambda(Rc::new(Lambda {
    params,
    rest_param,
    body,
    env: env.clone(),
    name: Some(name_spur),
}));
env.set(name_spur, lambda);
```

Remove the now-unused `name` String variable. The `env.set(intern(&name), lambda)` becomes `env.set(name_spur, lambda)`.

**Step 3: Update `eval_lambda` (line 331-357)**

Change param extraction (line 340-347):

```rust
let param_names: Vec<Spur> = param_list
    .iter()
    .map(|v| match v {
        Value::Symbol(s) => Ok(*s),
        _ => Err(SemaError::eval("lambda: parameter must be a symbol")),
    })
    .collect::<Result<_, _>>()?;
```

Update function signature — `name` parameter changes from `Option<String>` to `Option<Spur>`:

```rust
fn eval_lambda(args: &[Value], env: &Env, name: Option<Spur>) -> Result<Trampoline, SemaError> {
```

**Step 4: Update `eval_let` named-let (line 359-420)**

The named let at line 373-391 builds `Vec<String>` params. Change:

```rust
let mut params = Vec::new();
```

to `let mut params: Vec<Spur> = Vec::new();`

Change binding name extraction (line 384-387):

```rust
let pname = match &pair[0] {
    Value::Symbol(s) => *s,
    _ => return Err(SemaError::eval("named let: binding name must be a symbol")),
};
```

Update the `loop_name` extraction: `args[0].as_symbol()` returns a `String`. We need the Spur instead. Use:

```rust
let loop_name_spur = match &args[0] {
    Value::Symbol(s) => *s,
    _ => ... // fall through to regular let
};
```

Update Lambda construction (line 394-400):

```rust
let lambda = Lambda {
    params: params.clone(),
    rest_param: None,
    body,
    env: env.clone(),
    name: Some(loop_name_spur),
};
```

Update env bindings (line 404-416):

```rust
for (p, v) in params.iter().zip(init_vals.iter()) {
    new_env.set(*p, v.clone());
}
new_env.set(
    loop_name_spur,
    Value::Lambda(Rc::new(Lambda {
        params: lambda.params.clone(),
        rest_param: None,
        body: lambda.body.clone(),
        env: env.clone(),
        name: lambda.name,
    })),
);
```

**Step 5: Update `eval_defmacro` (line 592-622)**

Change param extraction (line 603-610):

```rust
let param_names: Vec<Spur> = param_list
    .iter()
    .map(|v| match v {
        Value::Symbol(s) => Ok(*s),
        _ => Err(SemaError::eval("defmacro: parameter must be a symbol")),
    })
    .collect::<Result<_, _>>()?;
```

Change name extraction (line 596-599) from `.to_string()` to Spur:

```rust
let name_spur = match &args[0] {
    Value::Symbol(s) => *s,
    _ => return Err(SemaError::eval("defmacro: name must be a symbol")),
};
```

Update Macro construction (line 614-619):

```rust
let mac = Value::Macro(Rc::new(Macro {
    params,
    rest_param,
    body,
    name: name_spur,
}));
env.set(name_spur, mac);
```

**Step 6: Update `eval_delay` (line 1434-1451)**

Simple — params is already `vec![]`, just needs no change. `name: None` stays `None` (it's now `Option<Spur>`). No changes needed here.

**Step 7: Update callers of `eval_lambda`**

In `try_eval_special`, `eval_lambda` is called at line 139:

```rust
} else if head_spur == sf.lambda || head_spur == sf.fn_ {
    Some(eval_lambda(args, env, None))
}
```

This passes `None` which is already `Option<Spur>` compatible.

In `eval_define`, the function-form path now calls `eval_lambda` indirectly via Lambda construction. Check if `eval_lambda` is called from `eval_define` — it's not, `eval_define` constructs Lambda directly.

**Step 8: Verify sema-eval compiles**

Run: `cargo check -p sema-eval 2>&1 | head -30`
Expected: sema-eval compiles. sema-llm may still fail.

---

### Task 3: Update Lambda/Macro consumption in sema-eval eval.rs

**Files:**

- Modify: `crates/sema-eval/src/eval.rs`

**Step 1: Update `call_value` (line 188-247)**

Lambda param binding (line 202-203):

```rust
for (param, arg) in lambda.params.iter().zip(args.iter()) {
    new_env.set(sema_core::intern(param), arg.clone());
}
```

becomes:

```rust
for (param, arg) in lambda.params.iter().zip(args.iter()) {
    new_env.set(*param, arg.clone());
}
```

Same for the else branch (line 215-216).

Rest param binding (line 206):

```rust
new_env.set(sema_core::intern(rest), Value::list(rest_args));
```

becomes:

```rust
new_env.set(*rest, Value::list(rest_args));
```

Self-reference binding (line 220-222):

```rust
if let Some(ref name) = lambda.name {
    new_env.set(sema_core::intern(name), Value::Lambda(Rc::clone(lambda)));
}
```

becomes:

```rust
if let Some(name) = lambda.name {
    new_env.set(name, Value::Lambda(Rc::clone(lambda)));
}
```

Error messages using `lambda.name.as_deref().unwrap_or("lambda")` (lines 197, 210) become:

```rust
&lambda.name.map(resolve).unwrap_or_else(|| "lambda".to_string())
```

**Step 2: Update `apply_lambda` (line 472-457)**

Same pattern as `call_value`. Change all `sema_core::intern(param)` to `*param`, `sema_core::intern(rest)` to `*rest`, `sema_core::intern(name)` to `name`.

Error messages: same `resolve` pattern as above.

CallFrame name (line 428):

```rust
name: lambda.name.as_deref().unwrap_or("<lambda>").to_string(),
```

becomes:

```rust
name: lambda.name.map(resolve).unwrap_or_else(|| "<lambda>".to_string()),
```

**Step 3: Update `apply_macro` (line 525-566)**

Same pattern. Change `sema_core::intern(param)` to `*param`, `sema_core::intern(rest)` to `*rest`.

Error messages using `&mac.name` (lines 538, 551) — `mac.name` is now `Spur`, so:

```rust
&resolve(mac.name)
```

**Step 4: Verify sema-eval compiles**

Run: `cargo check -p sema-eval 2>&1 | head -30`
Expected: compiles clean.

---

### Task 4: Update Lambda construction and consumption in sema-llm

**Files:**

- Modify: `crates/sema-llm/src/builtins.rs`

**Step 1: Update `call_value_fn` (line 2976-3026)**

Same pattern as eval.rs `call_value`. Change all `sema_core::intern(param)` to `*param`, `sema_core::intern(rest)` to `*rest`, `sema_core::intern(name)` to `name`.

Self-reference Lambda construction (line 3011-3018) — the fields already clone from the existing lambda, so they remain `Spur` automatically. Just fix the `env.set(sema_core::intern(name), ...)` to `env.set(name, ...)` where `name` is now a `Spur` from `lambda.name`.

**Step 2: Update test helpers and test Lambda constructions**

`make_lambda` (line 3130-3138):

```rust
fn make_lambda(params: &[&str]) -> Value {
    Value::Lambda(Rc::new(Lambda {
        params: params.iter().map(|s| intern(s)).collect(),
        rest_param: None,
        body: vec![Value::Nil],
        env: Env::new(),
        name: None,
    }))
}
```

`test_execute_tool_call_arg_ordering` (line 3246-3257):

```rust
let handler = Value::Lambda(Rc::new(Lambda {
    params: vec![intern("path"), intern("content")],
    rest_param: None,
    body: vec![Value::Symbol(intern("path"))],
    env: Env::new(),
    name: Some(intern("write-file-handler")),
}));
```

`test_execute_tool_call_reverse_alpha_order` (line 3278-3284):

```rust
let handler = Value::Lambda(Rc::new(Lambda {
    params: vec![intern("z_last"), intern("a_first")],
    rest_param: None,
    body: vec![Value::Symbol(intern("z_last"))],
    env: Env::new(),
    name: Some(intern("test-handler")),
}));
```

**Step 3: Verify full workspace compiles and all tests pass**

Run: `cargo build 2>&1 | tail -5`
Run: `cargo test 2>&1 | tail -20`
Expected: 0 compile errors, all 712+ tests pass.

Run: `make lint 2>&1 | tail -10`
Expected: clean (no fmt or clippy warnings).

---

### Task 5: Final verification and cleanup

**Step 1: Run the full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: all tests pass.

**Step 2: Run lint**

Run: `make lint 2>&1 | tail -10`
Expected: clean.

**Step 3: Quick smoke test**

Run: `cargo run -- -e "(define (add x y) (+ x y)) (add 1 2)"`
Expected: `3`

Run: `cargo run -- -e "(define (greet . names) names) (greet 1 2 3)"`
Expected: `(1 2 3)`

Run: `cargo run -- -e "(defmacro my-if (c t f) (list 'if c t f)) (my-if #t 1 2)"`
Expected: `1`
