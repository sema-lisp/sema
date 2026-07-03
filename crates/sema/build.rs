use std::path::Path;

fn main() {
    // Expose the build target triple so cross_compile::host_target() works.
    println!(
        "cargo:rustc-env=TARGET={}",
        std::env::var("TARGET").unwrap()
    );

    // The `sema web` dev server embeds a browser runtime (WASM VM + JS bundle)
    // vendored under src/web/assets/ by `make web-runtime`. Those artifacts are
    // gitignored (built, multi-MB), so the crate must compile whether or not
    // they are present: emit the `web_runtime` cfg only when they are, and gate
    // the embedding + subcommand behind it.
    println!("cargo:rustc-check-cfg=cfg(web_runtime)");
    println!("cargo:rerun-if-changed=src/web/assets");
    if Path::new("src/web/assets/sema_wasm_bg.wasm").exists() {
        println!("cargo:rustc-cfg=web_runtime");
    }
}
