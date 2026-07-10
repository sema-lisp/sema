//! Embedded browser UI for the notebook.
//!
//! The UI assets (HTML, CSS, JS) live as separate files in the `ui/`
//! directory next to this module. They are embedded into the binary at
//! compile time via `include_str!`, keeping deployment as a single
//! binary while allowing the assets to be edited as normal files.

/// Return the main HTML page.
pub fn index_html() -> String {
    include_str!("ui/index.html").to_string()
}

/// Serve a UI asset by path. Returns (content, content_type).
pub fn asset(path: &str) -> Option<(String, String)> {
    match path {
        "style.css" => Some((css().to_string(), "text/css".to_string())),
        "alpine.min.js" => Some((
            alpine_js().to_string(),
            "application/javascript".to_string(),
        )),
        "notebook.js" => Some((js().to_string(), "application/javascript".to_string())),
        "vendor/sema-ui.js" => Some((
            sema_ui_js().to_string(),
            "application/javascript".to_string(),
        )),
        "vendor/tokens.css" => Some((tokens_css().to_string(), "text/css".to_string())),
        _ => None,
    }
}

/// Serve an embedded binary font asset by path (e.g. `fonts/cormorant-latin.woff2`).
///
/// Fonts are bundled into the binary (latin `woff2` subsets of Cormorant and
/// JetBrains Mono) so the notebook renders correctly offline, with no runtime
/// dependency on the Google Fonts CDN.
pub fn font(path: &str) -> Option<(&'static [u8], &'static str)> {
    match path {
        "fonts/cormorant-latin.woff2" => Some((
            include_bytes!("ui/fonts/cormorant-latin.woff2"),
            "font/woff2",
        )),
        "fonts/jetbrains-mono-latin.woff2" => Some((
            include_bytes!("ui/fonts/jetbrains-mono-latin.woff2"),
            "font/woff2",
        )),
        _ => None,
    }
}

fn css() -> &'static str {
    include_str!("ui/style.css")
}

fn alpine_js() -> &'static str {
    include_str!("ui/alpine.min.js")
}

fn js() -> &'static str {
    include_str!("ui/notebook.js")
}

/// The `@sema-lang/ui` web-component bundle — `<sema-code-editor>`, `<sema-markdown>`,
/// and `<sema-editable-markdown>` for the notebook cells — embedded via `include_str!`
/// so the binary stays a single offline artifact. Vendored from the published npm
/// package's `dist/sema-ui.js` (the mono pins `@sema-lang/ui` in `playground/package.json`).
/// Refresh by fetching the pinned version from the unpkg CDN:
///   curl -fsSL https://unpkg.com/@sema-lang/ui@<version>/dist/sema-ui.js \
///     -o crates/sema-notebook/src/ui/vendor/sema-ui.js
fn sema_ui_js() -> &'static str {
    include_str!("ui/vendor/sema-ui.js")
}

/// The `@sema-lang/ui` design-token sheet — the `--gold*`/`--text-*`/spacing/radius
/// custom properties the component bundle's own styles fall back to. Linked before
/// `style.css` so the notebook's palette overrides land on top. Vendored from the
/// published npm package's `src/styles/tokens.css`. Refresh alongside `sema_ui_js`
/// by fetching the pinned version from the unpkg CDN:
///   curl -fsSL https://unpkg.com/@sema-lang/ui@<version>/src/styles/tokens.css \
///     -o crates/sema-notebook/src/ui/vendor/tokens.css
fn tokens_css() -> &'static str {
    include_str!("ui/vendor/tokens.css")
}
