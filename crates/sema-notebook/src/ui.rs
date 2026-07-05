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

/// The `@sema/ui` web-component bundle, vendored from `ui/dist/sema-ui.js` by the
/// `notebook-ui-vendor` make target. Provides `<sema-code-editor>`,
/// `<sema-markdown>`, and `<sema-editable-markdown>` for the notebook cells.
fn sema_ui_js() -> &'static str {
    include_str!("ui/vendor/sema-ui.js")
}
