fn main() {
    // The CLI dispatch and interpreter initialization exceed Windows' default
    // main-thread stack in debug builds. Reserve the same 8 MiB used on Linux.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bin=sema=/STACK:8388608");
    }

    // Expose the build target triple so cross_compile::host_target() works.
    println!(
        "cargo:rustc-env=TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
