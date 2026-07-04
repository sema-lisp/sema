# Editor tooling — tree-sitter grammar, extension packaging, browser E2E.
# Namespaced `ed`.

ts_dir = "editors/tree-sitter-sema"
vscode_dir = "editors/vscode/sema"
intellij_dir = "editors/intellij"

@group editors
@desc "Install tree-sitter grammar deps"
task ts-setup:
    @cd editors/tree-sitter-sema
    npm install

# File recipe: regenerate the parser only when the grammar changes.
file editors/tree-sitter-sema/src/parser.c: editors/tree-sitter-sema/grammar.js
    @needs npx
    @cd editors/tree-sitter-sema
    npm install
    npx tree-sitter generate

@group editors
@desc "Generate the tree-sitter parser (incremental)"
task ts-generate: [editors/tree-sitter-sema/src/parser.c]
    echo "tree-sitter parser generated"

@group editors
@desc "Run tree-sitter corpus tests"
task ts-test: [ts-generate]
    @cd editors/tree-sitter-sema
    npx tree-sitter test

@group editors
@desc "Build the tree-sitter WASM + open the playground"
task ts-playground: [ts-generate]
    @cd editors/tree-sitter-sema
    npx tree-sitter build --wasm && npx tree-sitter playground

# ── Editor extension packaging ───────────────────────────────────────
# VS Code and IntelliJ ship as separate marketplace artifacts, each on its own
# tag/workflow. These recipes package them locally (and publish behind @confirm),
# mirroring .github/workflows/publish-vscode-extension.yml and the IntelliJ
# gradle publish task.

# VS Code: compile TS then package into a .vsix. A `file` recipe so it only
# repackages when the extension sources/manifest actually change.
file editors/vscode/sema/sema.vsix: editors/vscode/sema/src/**/* editors/vscode/sema/package.json editors/vscode/sema/language-configuration.json editors/vscode/sema/syntaxes/*.json
    @needs npx "install Node.js"
    @cd editors/vscode/sema
    npm install
    npm run compile
    npx --yes @vscode/vsce package --no-git-tag-version --out sema.vsix

@group ext
@desc "Package the VS Code extension (.vsix)"
task vscode-package: [editors/vscode/sema/sema.vsix]
    echo "Packaged {{vscode_dir}}/sema.vsix"

# vsce reads VSCE_PAT from the env; ovsx takes the token as a flag. Publishes to
# both the VS Marketplace and the Open VSX registry, like the CI workflow.
@group ext
@desc "Publish the VS Code extension to VS Marketplace + Open VSX"
task vscode-publish: [vscode-package]
    : "${VSCE_PAT:?set VSCE_PAT (or add to .env)}" "${OVSX_PAT:?set OVSX_PAT (or add to .env)}"
    @confirm "Publish the VS Code extension to VS Marketplace and Open VSX?"
    @cd editors/vscode/sema
    npx --yes @vscode/vsce publish --packagePath sema.vsix
    npx --yes ovsx publish sema.vsix -p $OVSX_PAT

# IntelliJ: Gradle (org.jetbrains.intellij.platform). buildPlugin emits
# build/distributions/Sema-<pluginVersion>.zip.
@group ext
@desc "Build the IntelliJ plugin (.zip in build/distributions)"
@needs java "install a JDK 17+"
task intellij-build:
    @cd editors/intellij
    ./gradlew buildPlugin

@group ext
@desc "Run the IntelliJ plugin unit tests"
@needs java
task intellij-test:
    @cd editors/intellij
    ./gradlew test

@group ext
@desc "Run the IntelliJ plugin verifier (marketplace compatibility)"
@needs java
task intellij-verify:
    @cd editors/intellij
    ./gradlew verifyPlugin

# Signing + upload token come from the env (see editors/intellij/RELEASING.md):
# PUBLISH_TOKEN, plus CERTIFICATE_CHAIN / PRIVATE_KEY / PRIVATE_KEY_PASSWORD.
@group ext
@desc "Publish the IntelliJ plugin to the JetBrains Marketplace"
@needs java
task intellij-publish: [intellij-build]
    : "${PUBLISH_TOKEN:?set PUBLISH_TOKEN (or add to .env; see editors/intellij/RELEASING.md)}"
    @confirm "Publish the IntelliJ plugin to the JetBrains Marketplace?"
    @cd editors/intellij
    ./gradlew publishPlugin

@group ext
@desc "Package both editor extensions (.vsix + IntelliJ .zip)"
task ext-package: [vscode-package, intellij-build]
    echo "Packaged VS Code (.vsix) + IntelliJ (.zip) extensions"

# ── Browser E2E ──────────────────────────────────────────────────────

@group e2e
@desc "Notebook browser E2E (Playwright)"
task test-notebook-e2e: [build]
    @needs npx
    echo "=== Running notebook E2E tests ==="
    @cd crates/sema-notebook/tests/e2e
    npx playwright test

# Vendor the browser runtime, build the release binary (embeds it), then drive
# the real `sema web` dev server in a browser.
@group e2e
@desc "sema web dev-server browser E2E (Playwright)"
task test-web-e2e: [wasm.web-runtime]
    @needs npx
    cargo build --release -p sema-lang
    echo "=== Running sema web dev-server E2E tests ==="
    @cd packages/sema-web
    npx playwright test --config playwright.dev-server.config.ts
