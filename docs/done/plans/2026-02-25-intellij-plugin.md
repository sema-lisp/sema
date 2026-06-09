# IntelliJ Plugin Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a feature-rich IntelliJ IDEA plugin for Sema that integrates with the LSP server and provides file icons, syntax highlighting, and full IDE features.

**Architecture:** Thin LSP client wrapper using LSP4IJ (Red Hat) for all language intelligence, with a hand-written Kotlin lexer for token-level syntax highlighting and a minimal parser for PSI file nodes. The LSP server (`sema lsp` via stdio) provides completions, hover, go-to-definition (including cross-module), diagnostics (parse errors + VM warnings), code lenses (▶ Run), and inline eval results.

**Tech Stack:** Kotlin, Gradle IntelliJ Platform Plugin 2.x, LSP4IJ

**LSP Server Capabilities (current):**
- Text document sync (FULL)
- Completion (special forms, builtins, user definitions; triggers: `(`, ` `)
- Go to Definition (user defs, import/load path navigation, cross-module)
- Hover (builtin docs, user function signatures, special forms, imported symbols)
- Code Lens (▶ Run on each top-level form)
- Execute Command (`sema.runTopLevel`)
- Diagnostics (parse errors via sema-reader, compile warnings via sema-vm)
- Custom notification: `sema/evalResult` (inline eval results)

---

### Task 1: Scaffold Gradle Project

**Files:**
- Create: `editors/intellij/build.gradle.kts`
- Create: `editors/intellij/settings.gradle.kts`
- Create: `editors/intellij/gradle.properties`
- Create: `editors/intellij/.gitignore`

**Step 1: Initialize Gradle wrapper and project files**

**Step 2: Create `settings.gradle.kts`**
```kotlin
rootProject.name = "sema-intellij"
```

**Step 3: Create `gradle.properties`**
```properties
pluginGroup = com.sema.intellij
pluginName = Sema
pluginVersion = 0.1.0
pluginSinceBuild = 241
pluginUntilBuild = 253.*
platformType = IC
platformVersion = 2024.1.7
kotlinVersion = 1.9.25
```

**Step 4: Create `build.gradle.kts`** with intellij-platform plugin 2.x, LSP4IJ dependency, Kotlin 1.9, JVM 17.

**Step 5: Create `.gitignore`** (.gradle/, build/, .idea/, *.iml, out/)

**Step 6: Verify** `./gradlew build`

**Step 7: Commit** `feat(intellij): scaffold Gradle plugin project`

---

### Task 2: Language + File Types + Icons

**Files:**
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaLanguage.kt`
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaFileType.kt`
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemacFileType.kt`
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaIcons.kt`
- Create: `editors/intellij/src/main/resources/icons/sema.svg`
- Create: `editors/intellij/src/main/resources/icons/semac.svg`
- Create: `editors/intellij/src/main/resources/META-INF/plugin.xml`

**Step 1: `SemaLanguage`** — singleton `Language("Sema")`

**Step 2: `SemaIcons`** — `IconLoader.getIcon("/icons/sema.svg")` and `/icons/semac.svg`

**Step 3: `SemaFileType`** — `LanguageFileType(SemaLanguage)`, ext `sema`, gold S icon

**Step 4: `SemacFileType`** — `FileType` (binary, read-only), ext `semac`, distinct icon

**Step 5: SVG icons** — 16x16 viewBox, gold S on dark bg for .sema; dimmer variant with "c" badge for .semac. Based on brand assets (`assets/favicon.svg`).

**Step 6: `plugin.xml`** — register both file types, declare depends on `com.intellij.modules.platform` and `com.redhat.devtools.lsp4ij`

**Step 7: Verify** `./gradlew build`

**Step 8: Commit** `feat(intellij): add language, file types, and icons`

---

### Task 3: Lexer + Syntax Highlighting

Token types derived from `editors/tree-sitter-sema/grammar.js` and `crates/sema-reader/src/lexer.rs`.

**Files:**
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaTokenTypes.kt`
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaLexer.kt`
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaColors.kt`
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaSyntaxHighlighter.kt`
- Modify: `plugin.xml`

**Step 1: `SemaTokenTypes`** — IElementType constants for: LINE_COMMENT, BLOCK_COMMENT, STRING, NUMBER, SYMBOL, KEYWORD, BOOLEAN, CHARACTER, NIL, LPAREN, RPAREN, LBRACKET, RBRACKET, LBRACE, RBRACE, QUOTE, QUASIQUOTE, UNQUOTE, SPLICE, HASH_DISPATCH, DOT. Plus TokenSets for COMMENTS, STRINGS, WHITESPACES.

**Step 2: `SemaLexer`** — extends `LexerBase`. Hand-written (simpler than JFlex for a Lisp). Must handle:
- Line comments: `;` to EOL
- Block comments: `#| ... |#` (nested)
- Strings: `"..."` with `\` escapes
- F-strings: `f"..."` (lex as STRING)
- Regex: `#"..."` (lex as STRING)
- Numbers: `[+-]?[0-9]+(\.[0-9]+)?`
- Keywords: `:[symbol-chars]+`
- Booleans: `#t`, `#f`, `true`, `false`
- Characters: `#\space`, `#\a`, etc.
- Symbols: standard Sema symbol chars (matching `is_symbol_char` in lexer.rs)
- All bracket types, quote chars, `,@` splice

**Step 3: `SemaColors`** — TextAttributesKey mapped to DefaultLanguageHighlighterColors

**Step 4: `SemaSyntaxHighlighter` + `SemaSyntaxHighlighterFactory`**

**Step 5: Register** `lang.syntaxHighlighterFactory` in plugin.xml

**Step 6: Verify** `./gradlew build`

**Step 7: Commit** `feat(intellij): add lexer and syntax highlighting`

---

### Task 4: Brace Matcher + Commenter

**Files:**
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaBraceMatcher.kt`
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaCommenter.kt`
- Modify: `plugin.xml`

**Step 1: `SemaBraceMatcher`** — PairedBraceMatcher for `()`, `[]`, `{}`

**Step 2: `SemaCommenter`** — line prefix `;`, block prefix `#|`, block suffix `|#`

**Step 3: Register** both in plugin.xml

**Step 4: Verify + Commit** `feat(intellij): add brace matching and commenting`

---

### Task 5: Minimal Parser Definition

IntelliJ requires a ParserDefinition even when LSP handles intelligence.

**Files:**
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaFile.kt`
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaParserDefinition.kt`
- Modify: `plugin.xml`

**Step 1: `SemaFile`** — extends PsiFileBase

**Step 2: `SemaParserDefinition`** — minimal parser that wraps entire file as single FILE node (no structural parsing). Returns SemaLexer, SemaFile, token sets.

**Step 3: Register** `lang.parserDefinition` in plugin.xml

**Step 4: Verify + Commit** `feat(intellij): add minimal parser definition`

---

### Task 6: LSP4IJ Integration

**Files:**
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/lsp/SemaLanguageServer.kt`
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/lsp/SemaLanguageServerFactory.kt`
- Modify: `plugin.xml`

**Step 1: `SemaLanguageServer`** — extends `OSProcessStreamConnectionProvider`, launches `sema lsp` via `GeneralCommandLine`. Reads `SEMA_PATH` env var, defaults to `sema`.

**Step 2: `SemaLanguageServerFactory`** — implements `LanguageServerFactory`, creates connection provider and language client.

**Step 3: Register in plugin.xml** — lsp4ij `<server>` with id `sema-lsp`, `<languageMapping>` for language `Sema` → server `sema-lsp` with languageId `sema`.

**Step 4: Verify + Commit** `feat(intellij): wire LSP server via LSP4IJ`

---

### Task 7: Color Settings Page + README

**Files:**
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/SemaColorSettingsPage.kt`
- Create: `editors/intellij/README.md`
- Modify: `plugin.xml`

**Step 1: `SemaColorSettingsPage`** — demo text with Sema code samples, attribute descriptors for all SemaColors entries.

**Step 2: Register** `colorSettingsPage` in plugin.xml

**Step 3: `README.md`** — features list, requirements (IDEA 2024.1+, LSP4IJ, sema on PATH), build instructions, configuration.

**Step 4: Verify + Commit** `feat(intellij): add color settings page and README`

---

### Task 8: Smoke Test

**Step 1:** Run `./gradlew runIde`

**Step 2: Verify checklist:**
- [ ] `.sema` files show gold S icon
- [ ] `.semac` files show distinct compiled icon
- [ ] Syntax highlighting works (comments, strings, numbers, keywords, booleans)
- [ ] Brace matching highlights matching `()` `[]` `{}`
- [ ] `Cmd+/` toggles `;` line comments
- [ ] LSP starts (check Language Servers tool window)
- [ ] Completions appear after `(`
- [ ] Hover shows docs for builtins
- [ ] Go-to-definition jumps to definitions
- [ ] Diagnostics show for syntax errors
- [ ] Code lenses show ▶ Run

---

## Future Enhancements (not in this plan)

- Settings UI for sema path (IntelliJ Configurable)
- Semantic tokens when LSP adds support
- Structure view when LSP adds documentSymbol
- Folding ranges when LSP adds support
- Run configurations for .sema files
- Eval result inline decorations (sema/evalResult notification)
- VS Code file icons for .sema/.semac
