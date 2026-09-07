fn main() {
    // Expose the build target triple so cross_compile::host_target() works.
    println!(
        "cargo:rustc-env=TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
