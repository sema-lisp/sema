//! Asset-serving tests for the embedded notebook UI.

/// The vendored `@sema/ui` bundle (built from `ui/` via `make notebook-ui-vendor`)
/// must be served at `/ui/vendor/sema-ui.js` so the notebook can use the shared
/// web components while staying a single offline binary.
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
