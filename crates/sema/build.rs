fn main() {
    // Expose the build target triple so cross_compile::host_target() works.
    println!(
        "cargo:rustc-env=TARGET={}",
        std::env::var("TARGET").unwrap()
    );

    // The Windows default main-thread stack is too small for the debug CLI.
    // Reserve 8 MiB in the executable so installed binaries get the same stack.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let stack_arg = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            "/STACK:8388608"
        } else {
            "-Wl,--stack,8388608"
        };
        println!("cargo:rustc-link-arg-bin=sema={stack_arg}");
    }
}
