fn first<'a>(value: &'a str) {
    if in_runtime_quantum() {
        call_callback(ctx, &func, &args);
    }
}

fn second<'b>(value: &'b str) {}
