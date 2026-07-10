//! Asset-serving tests for the embedded notebook UI.

/// The vendored `@sema-lang/ui` bundle (the published npm package's `dist/sema-ui.js`,
/// fetched from the unpkg CDN) must be served at `/ui/vendor/sema-ui.js` so the notebook
/// can use the shared web components while staying a single offline binary.
#[test]
fn serves_vendored_sema_ui_bundle() {
    let asset = sema_notebook::ui::asset("vendor/sema-ui.js");
    assert!(asset.is_some(), "vendored @sema/ui bundle must be served");
    let (body, ct) = asset.unwrap();
    assert!(
        body.contains("sema-editor"),
        "bundle should define the editor element",
    );
    assert!(
        body.contains("sema-markdown"),
        "bundle should define the markdown renderer element",
    );
    assert_eq!(ct, "application/javascript");
}

/// The vendored `@sema-lang/ui` token sheet (the published npm package's
/// `src/styles/tokens.css`) must be served at `/ui/vendor/tokens.css` so the
/// component bundle's `var(--token, #fallback)` references resolve to the
/// library's palette, not just their hardcoded fallbacks.
#[test]
fn serves_vendored_tokens_css() {
    let asset = sema_notebook::ui::asset("vendor/tokens.css");
    assert!(asset.is_some(), "vendored tokens.css must be served");
    let (body, ct) = asset.unwrap();
    assert!(
        body.contains("--text-primary"),
        "tokens sheet should define the text-* namespace the notebook bridges to",
    );
    assert_eq!(ct, "text/css");
}
