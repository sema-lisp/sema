fn negated_runtime_stdin_read() -> std::io::Result<String> {
    if !in_runtime_quantum() {
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        return Ok(input);
    }
    Ok(String::new())
}
