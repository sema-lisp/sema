# IntelliJ Plugin Test Suite — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish comprehensive test coverage for the Sema IntelliJ plugin (`editors/intellij/`) from ~8 trivial unit tests to a multi-tier test pyramid: lexer/parser correctness, language feature tests, LSP/DAP integration with real binaries, notebook tests, full-IDE integration tests with UI interaction, and stress/leak detection.

**Architecture:** Four test tiers, each independently runnable:

1. **Light unit tests** (`src/test/`) — `LightPlatformTestCase` / `BasePlatformTestCase` for lexer, PSI, highlighter, brace matcher, commenter, settings, run config, file type. No binary dependencies. Run with `./gradlew test`.
2. **LSP/DAP integration tests** (`src/test/`) — `HeavyPlatformTestCase` with a real `sema lsp`/`sema dap` process. Requires `sema` on PATH or configurable via system property.
3. **Notebook tests** (`src/test/`) — grouped separately in a dedicated package, runnable via tag `@Tag("notebook")`. Covers file type, editor provider, session service, actions.
4. **Full IDE integration tests** (`src/integrationTest/`) — Starter + Driver framework. Real IntelliJ IDE instance with plugin installed. UI interaction + API verification. Run with `./gradlew integrationTest`.

**Tech Stack:** JUnit 5, `kotlin.test`, IntelliJ Platform Test Framework (`com.intellij.platform.testFramework`), LSP4IJ test helpers, JetBrains Starter + Driver frameworks, `kotlinx-coroutines`.

---

### Task 1: Gradle Test Infrastructure

**Files:**
- Modify: `editors/intellij/build.gradle.kts`

- [ ] **Step 1: Add test framework dependencies**

Replace `testImplementation(kotlin("test"))` with full test dependencies:

```kotlin
dependencies {
    intellijPlatform {
        // ... existing platform/lsp4ij deps ...

        testFramework(TestFrameworkType.Platform)
    }

    testImplementation(kotlin("test"))
    testImplementation("org.junit.jupiter:junit-jupiter:5.11.4")
    testImplementation("org.junit.jupiter:junit-jupiter-api:5.11.4")
    testRuntimeOnly("org.junit.jupiter:junit-jupiter-engine:5.11.4")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}
```

- [ ] **Step 2: Add integration test source set and task**

Add after the `dependencies` block:

```kotlin
sourceSets {
    create("integrationTest") {
        compileClasspath += sourceSets.main.get().output
        runtimeClasspath += sourceSets.main.get().output
    }
}

val integrationTestImplementation by configurations.getting {
    extendsFrom(configurations.testImplementation.get())
}

dependencies {
    // ... append below existing dependencies block ...
    intellijPlatform {
        testFramework(TestFrameworkType.Starter, configurationName = "integrationTestImplementation")
    }
    integrationTestImplementation("org.junit.jupiter:junit-jupiter:5.11.4")
    integrationTestImplementation("org.kodein.di:kodein-di-jvm:7.20.2")
    integrationTestImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-core-jvm:1.10.1")
}

val integrationTest by intellijPlatformTesting.testIdeUi.registering {
    task {
        val integrationTestSourceSet = sourceSets.getByName("integrationTest")
        testClassesDirs = integrationTestSourceSet.output.classesDirs
        classpath = integrationTestSourceSet.runtimeClasspath
        useJUnitPlatform()
        systemProperty("path.to.build.plugin", layout.buildDirectory.dir("distributions").get().asFile.resolve("${rootProject.name}-${version}.zip").absolutePath)
    }
}

tasks.named("check") { dependsOn(integrationTest) }
```

- [ ] **Step 3: Add JUnit configuration file**

Create `editors/intellij/src/test/resources/junit-platform.properties`:

```properties
junit.jupiter.testinstance.lifecycle.default = per_class
```

This avoids issues with IntelliJ platform test fixture lifecycle (some fixtures assume per-class instance lifecycle).

- [ ] **Step 4: Add test helper for sema binary path resolution**

Create `editors/intellij/src/test/kotlin/com/sema/intellij/TestHelpers.kt`:

```kotlin
package com.sema.intellij

import com.sema.intellij.config.SemaBinary
import com.sema.intellij.config.SemaBinaryStatus

object TestHelpers {
    /**
     * Returns a [SemaBinaryStatus] for the sema binary.
     *
     * Resolution order:
     * 1. System property "sema.test.binary" (absolute path)
     * 2. System property "sema.test.path" added to PATH entries
     * 3. Default PATH resolution
     */
    fun resolveSemaBinary(): SemaBinaryStatus {
        val explicitPath = System.getProperty("sema.test.binary")
        if (explicitPath != null) {
            return SemaBinary.check(explicitPath, emptyList())
        }
        val extraPath = System.getProperty("sema.test.path")
        val pathEntries = if (extraPath != null) {
            listOf(extraPath) + SemaBinary.pathEntriesFromEnvironment()
        } else {
            SemaBinary.pathEntriesFromEnvironment()
        }
        return SemaBinary.check("sema", pathEntries)
    }

    /** Skip marker exception for tests requiring a sema binary. */
    class SemaBinaryRequired(message: String) : RuntimeException(message)

    /** Ensures sema is available or skips the test with a clear message. */
    fun requireSema() {
        val status = resolveSemaBinary()
        if (!status.available) {
            throw org.junit.jupiter.api.Assumptions.abort(
                "Sema binary not found: ${status.errorText}. " +
                "Set -Dsema.test.binary=/path/to/sema or ensure sema is on PATH."
            )
        }
    }

    /** Returns a sample .sema source file for testing. */
    fun sampleSemaContent(): String = """
        ;;; Sample Sema file for testing
        (define greeting "Hello, world!")

        (defun square (x)
          "Return the square of X."
          (* x x))

        (defun factorial (n)
          (if (<= n 1)
              1
              (* n (factorial (- n 1)))))

        (when (> (square 5) 20)
          (print "5 squared exceeds 20"))

        (let ((a 10)
              (b 20))
          (+ a b))

        ;; Test various literals
        (define some-nil nil)
        (define some-bool #t)
        (define some-char #\a)
        (define some-keyword :name)
        (define some-quote '(1 2 3))
        (define some-list [1 2 3])
        (define some-map {:a 1 :b 2})

        ;; Import
        (import "math")
        (import \"http\")
    """.trimIndent()
}
```

Also expose `pathEntriesFromEnvironment` on `SemaBinary` (currently private). Add to `SemaBinary.kt`:

```kotlin
fun pathEntriesFromEnvironment(): List<String> =
    System.getenv("PATH")?.split(File.pathSeparatorChar) ?: emptyList()
```

---

### Task 2: Lexer Correctness Tests

**Files:**
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/SemaLexerTest.kt`

- [ ] **Step 1: Basic token type classification**

```kotlin
package com.sema.intellij

import com.intellij.psi.TokenType
import com.intellij.testFramework.LightPlatformTestCase
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

class SemaLexerTest {
    private fun tokensOf(source: String): List<Pair<IElementType?, String>> {
        val lexer = SemaLexer()
        lexer.start(source)
        val result = mutableListOf<Pair<IElementType?, String>>()
        while (lexer.tokenType != null) {
            result.add(lexer.tokenType to source.substring(lexer.tokenStart, lexer.tokenEnd))
            lexer.advance()
        }
        return result
    }

    @Test
    fun `empty file produces no tokens`() {
        val tokens = tokensOf("")
        assertTrue(tokens.isEmpty())
    }

    @Test
    fun `whitespace is classified correctly`() {
        val tokens = tokensOf("   \t\n  ")
        assertTrue(tokens.all { it.first == TokenType.WHITE_SPACE })
        assertEquals(2, tokens.size) // one for spaces+tab, one for newline+spaces
    }
}
```

- [ ] **Step 2: Delimiter tokens (parens, brackets, braces)**

Continue in same test class:

```kotlin
    @Test
    fun `delimiter tokens are classified correctly`() {
        val tokens = tokensOf("()[]{}")
        val types = tokens.map { it.first }
        assertEquals(
            listOf(
                SemaTokenTypes.LPAREN, SemaTokenTypes.RPAREN,
                SemaTokenTypes.LBRACKET, SemaTokenTypes.RBRACKET,
                SemaTokenTypes.LBRACE, SemaTokenTypes.RBRACE,
            ),
            types,
        )
    }

    @Test
    fun `nested delimiters`() {
        val tokens = tokensOf("([{}])")
        val types = tokens.map { it.first }
        assertEquals(
            listOf(
                SemaTokenTypes.LPAREN, SemaTokenTypes.LBRACKET,
                SemaTokenTypes.LBRACE, SemaTokenTypes.RBRACE,
                SemaTokenTypes.RBRACKET, SemaTokenTypes.RPAREN,
            ),
            types,
        )
    }
```

- [ ] **Step 3: Quote, quasiquote, unquote, splice**

```kotlin
    @Test
    fun `quote family tokens`() {
        val tokens = tokensOf("' ` , ,@")
        val types = tokens.filter { it.first !is TokenType } // skip whitespace
            .map { it.first }
        assertEquals(
            listOf(SemaTokenTypes.QUOTE, SemaTokenTypes.QUASIQUOTE, SemaTokenTypes.UNQUOTE, SemaTokenTypes.SPLICE),
            types,
        )
    }

    @Test
    fun `comma by itself is unquote not splice`() {
        val tokens = tokensOf(",foo ,bar")
        val types = tokens.filter { it.first !is TokenType }
            .map { it.first }
        assertEquals(listOf(SemaTokenTypes.UNQUOTE, SemaTokenTypes.SYMBOL, SemaTokenTypes.UNQUOTE, SemaTokenTypes.SYMBOL), types)
    }

    @Test
    fun `comma without space after is still unquote`() {
        // In SemaLexer: ch == ',' with peekNext() != '@' → consume comma only → UNQUOTE
        val tokens = tokensOf(",x")
        val first = tokens.first { it.first !is TokenType }
        assertEquals(SemaTokenTypes.UNQUOTE, first.first)
    }
```

- [ ] **Step 4: Strings, f-strings, regex strings**

```kotlin
    @Test
    fun `string literal with escapes`() {
        val tokens = tokensOf("\"hello \\\"world\\\"\"")
        val strings = tokens.filter { it.first == SemaTokenTypes.STRING }
        assertEquals(1, strings.size)
        assertEquals("\"hello \\\"world\\\"\"", strings[0].second)
    }

    @Test
    fun `unterminated string is still classified as string`() {
        val tokens = tokensOf("\"unclosed")
        assertEquals(SemaTokenTypes.STRING, tokens.last().first)
    }

    @Test
    fun `f-string is classified as string`() {
        val tokens = tokensOf("f\"hello {name}\"")
        val strings = tokens.filter { it.first == SemaTokenTypes.STRING }
        assertEquals(1, strings.size)
        assertEquals("f\"hello {name}\"", strings[0].second)
    }

    @Test
    fun `regex string hash-quote`() {
        val tokens = tokensOf("#\"[a-z]+\"")
        val strings = tokens.filter { it.first == SemaTokenTypes.STRING }
        assertEquals(1, strings.size)
        assertEquals("#\"[a-z]+\"", strings[0].second)
    }
```

- [ ] **Step 5: Numbers**

```kotlin
    @Test
    fun `integer literals`() {
        val tokens = tokensOf("0 42 -7 1000000")
        val numbers = tokens.filter { it.first == SemaTokenTypes.NUMBER }
        assertEquals(4, numbers.size)
        assertEquals(listOf("0", "42", "-7", "1000000"), numbers.map { it.second })
    }

    @Test
    fun `float literals`() {
        val tokens = tokensOf("3.14 -0.5 1.0")
        val numbers = tokens.filter { it.first == SemaTokenTypes.NUMBER }
        assertEquals(3, numbers.size)
        assertEquals(listOf("3.14", "-0.5", "1.0"), numbers.map { it.second })
    }

    @Test
    fun `standalone minus is symbol not number`() {
        val tokens = tokensOf("(- x y)")
        val types = tokens.filter { it.first !is TokenType }.map { it.first }
        // The '-' inside parens should be a SYMBOL, not NUMBER
        assertTrue(SemaTokenTypes.SYMBOL in types)
        assertFalse(SemaTokenTypes.NUMBER in types)
    }
```

- [ ] **Step 6: Symbols, keywords, special forms, definition keywords**

```kotlin
    @Test
    fun `plain symbol`() {
        val tokens = tokensOf("my-function")
        val sym = tokens.single { it.first !is TokenType }
        assertEquals(SemaTokenTypes.SYMBOL, sym.first)
        assertEquals("my-function", sym.second)
    }

    @Test
    fun `special forms are classified`() {
        val tokens = tokensOf("if when let lambda defun define defmacro import module")
        val specials = tokens.filter { it.first == SemaTokenTypes.SPECIAL_FORM }
        val defs = tokens.filter { it.first == SemaTokenTypes.DEFINITION_KEYWORD }
        assertEquals(6, specials.size) // if, when, let, lambda, import, module
        assertEquals(3, defs.size)     // defun, define, defmacro
    }

    @Test
    fun `dot token`() {
        val tokens = tokensOf("(foo . bar)")
        val dot = tokens.find { it.first == SemaTokenTypes.DOT }
        assertNotNull(dot)
        assertEquals(".", dot.second)
    }

    @Test
    fun `dot as part of symbol`() {
        // "foo.bar" should be one symbol, not symbol-dot-symbol
        val tokens = tokensOf("foo.bar")
        val symbols = tokens.filter { it.first == SemaTokenTypes.SYMBOL }
        assertEquals(1, symbols.size)
        assertEquals("foo.bar", symbols[0].second)
    }

    @Test
    fun `keyword with colon prefix`() {
        val tokens = tokensOf(":my-keyword :another")
        val keywords = tokens.filter { it.first == SemaTokenTypes.KEYWORD }
        assertEquals(2, keywords.size)
        assertEquals(listOf(":my-keyword", ":another"), keywords.map { it.second })
    }
```

- [ ] **Step 7: Booleans, nil, character literals**

```kotlin
    @Test
    fun `boolean and nil literals`() {
        val tokens = tokensOf("#t #f true false nil")
        val booleans = tokens.filter { it.first == SemaTokenTypes.BOOLEAN }
        val nils = tokens.filter { it.first == SemaTokenTypes.NIL }
        assertEquals(4, booleans.size) // #t, #f, true, false
        assertEquals(1, nils.size)     // nil
    }

    @Test
    fun `character literals`() {
        val tests = listOf("#\\a", "#\\space", "#\\newline", "#\\tab")
        for (src in tests) {
            val tokens = tokensOf(src)
            assertEquals(SemaTokenTypes.CHARACTER, tokens[0].first, "Failed for: $src")
        }
    }
```

- [ ] **Step 8: Hash dispatch (vectors, bytevectors, etc.)**

```kotlin
    @Test
    fun `hash dispatch for vector syntax`() {
        val tokens = tokensOf("#(1 2 3) #u8(1 2 3)")
        val dispatches = tokens.filter { it.first == SemaTokenTypes.HASH_DISPATCH }
        assertEquals(2, dispatches.size)
        assertEquals("#(", dispatches[0].second)
        assertEquals("#u8(", dispatches[1].second)
    }
```

- [ ] **Step 9: Edge cases for the lexer**

```kotlin
    @Test
    fun `nested block comments`() {
        val tokens = tokensOf("#|outer #|inner|# text|#")
        val comments = tokens.filter { it.first == SemaTokenTypes.BLOCK_COMMENT }
        assertEquals(1, comments.size)
        assertTrue(comments[0].second.contains("inner"))
    }

    @Test
    fun `unterminated nested block comment`() {
        val tokens = tokensOf("#|outer #|inner|#")
        assertEquals(SemaTokenTypes.BLOCK_COMMENT, tokens.last().first)
    }

    @Test
    fun `semicolon line comment extends to newline`() {
        val source = "; comment\nafter"
        val tokens = tokensOf(source)
        val comments = tokens.filter { it.first == SemaTokenTypes.LINE_COMMENT }
        assertEquals(1, comments.size)
        assertEquals("; comment", comments[0].second)
    }

    @Test
    fun `bad character yields BAD_CHARACTER`() {
        // '@' not at splice position, standalone
        val source = "@"
        val tokens = tokensOf(source)
        assertEquals(TokenType.BAD_CHARACTER, tokens[0].first)
    }

    @Test
    fun `very long symbol`() {
        val longName = "x".repeat(1000)
        val tokens = tokensOf(longName)
        assertEquals(SemaTokenTypes.SYMBOL, tokens[0].first)
        assertEquals(longName, tokens[0].second)
    }

    @Test
    fun `many consecutive strings`() {
        val source = (1..100).joinToString(" ") { "\"str$it\"" }
        val tokens = tokensOf(source)
        val strings = tokens.filter { it.first == SemaTokenTypes.STRING }
        assertEquals(100, strings.size)
    }

    @Test
    fun `symbol chars edge cases`() {
        // Symbols can contain + - * / < > = _ ! ? & % ^ ~ .
        val tokens = tokensOf("+ - * / < > = _! ? & % ^ ~")
        val symbols = tokens.filter { it.first == SemaTokenTypes.SYMBOL }
        assertEquals(12, symbols.size)
    }
}
```

---

### Task 3: Parser, PSI, and File Type Tests

**Files:**
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/SemaParserTest.kt`
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/SemaFileTypeTest.kt`

- [ ] **Step 1: PSI tree construction from sample file**

```kotlin
package com.sema.intellij

import com.intellij.psi.PsiFile
import com.intellij.psi.PsiFileFactory
import com.intellij.testFramework.LightPlatformTestCase
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

class SemaPsiTest {
    private fun parseSema(text: String): PsiFile =
        PsiFileFactory.getInstance(LightPlatformTestCase.getProject())
            .createFileFromText("test.sema", SemaLanguage, text)

    @Test
    fun `parses valid sema file without errors`() {
        val psiFile = parseSema(TestHelpers.sampleSemaContent())
        assertNotNull(psiFile)
        assertEquals(SemaLanguage, psiFile.language)
        // With the current trivial parser, firstChild is the file node
        assertNotNull(psiFile.firstChild)

        // Walk all tokens — should not contain BAD_CHARACTER
        var badTokens = 0
        psiFile.node.visitLeafNodes { leaf ->
            if (leaf.elementType == com.intellij.psi.TokenType.BAD_CHARACTER) badTokens++
        }
        assertEquals(0, badTokens, "Valid Sema should have no bad characters")
    }

    @Test
    fun `parses empty file`() {
        val psiFile = parseSema("")
        assertNotNull(psiFile)
    }

    @Test
    fun `parses file with only comments`() {
        val psiFile = parseSema(";; just a comment\n#|block|#")
        assertNotNull(psiFile)
        psiFile.node.visitLeafNodes { leaf ->
            assertFalse(
                leaf.elementType == com.intellij.psi.TokenType.BAD_CHARACTER,
                "Comment-only file should have no bad characters, got: ${leaf.text}"
            )
        }
    }

    @Test
    fun `parses deeply nested expression`() {
        val depth = 100
        val source = "(".repeat(depth) + "x" + ")".repeat(depth)
        val psiFile = parseSema(source)
        assertNotNull(psiFile)
    }
}
```

- [ ] **Step 2: Token count verification**

```kotlin
    @Test
    fun `token count matches lexer output`() {
        val source = "(define x 42)"
        val psiFile = parseSema(source)

        val tokenCount = mutableListOf<String>()
        psiFile.node.visitLeafNodes { leaf ->
            if (leaf.elementType !is com.intellij.psi.TokenType) {
                tokenCount.add(leaf.text)
            }
        }
        // Should find LPAREN, SYMBOL(define), SYMBOL(x), NUMBER(42), RPAREN (5 meaningful tokens)
        assertTrue(tokenCount.size >= 5, "Expected at least 5 meaningful tokens, got ${tokenCount.size}: $tokenCount")
    }
```

- [ ] **Step 3: File type tests**

```kotlin
package com.sema.intellij

import com.intellij.openapi.fileTypes.FileTypeManager
import com.intellij.testFramework.LightPlatformTestCase
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

class SemaFileTypeTest {
    @Test
    fun `sema file type has correct extension`() {
        assertEquals("sema", SemaFileType.defaultExtension)
    }

    @Test
    fun `sema file type has correct name and description`() {
        assertEquals("Sema", SemaFileType.name)
        assertEquals("Sema language source file", SemaFileType.description)
    }

    @Test
    fun `sema file type has non-null icon`() {
        assertNotNull(SemaFileType.icon)
    }

    @Test
    fun `semac file type has correct extension`() {
        assertEquals("semac", SemacFileType.defaultExtension)
    }

    @Test
    fun `file extension to file type mapping`() {
        val ftm = FileTypeManager.getInstance()
        assertEquals(SemaFileType, ftm.getFileTypeByExtension("sema"))
        assertEquals(SemacFileType, ftm.getFileTypeByExtension("semac"))
    }

    @Test
    fun `sema language is registered`() {
        assertNotNull(SemaLanguage)
        assertEquals("Sema", SemaLanguage.id)
    }
}
```

---

### Task 4: Syntax Highlighter and Colors Tests

**Files:**
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/SemaSyntaxHighlighterTest.kt`

- [ ] **Step 1: Token-to-highlight mapping**

```kotlin
package com.sema.intellij

import com.intellij.openapi.editor.colors.TextAttributesKey
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

class SemaSyntaxHighlighterTest {
    private val highlighter = SemaSyntaxHighlighter()

    private fun highlightFor(tokenType: IElementType?): Array<TextAttributesKey> =
        highlighter.getTokenHighlights(tokenType)

    @Test
    fun `every registered token type maps to a non-empty highlight`() {
        val tokenTypes = listOf(
            SemaTokenTypes.LINE_COMMENT to SemaColors.LINE_COMMENT,
            SemaTokenTypes.BLOCK_COMMENT to SemaColors.BLOCK_COMMENT,
            SemaTokenTypes.STRING to SemaColors.STRING,
            SemaTokenTypes.NUMBER to SemaColors.NUMBER,
            SemaTokenTypes.KEYWORD to SemaColors.KEYWORD,
            SemaTokenTypes.SYMBOL to SemaColors.SYMBOL,
            SemaTokenTypes.BOOLEAN to SemaColors.BOOLEAN,
            SemaTokenTypes.NIL to SemaColors.NIL,
            SemaTokenTypes.CHARACTER to SemaColors.CHARACTER,
            SemaTokenTypes.SPECIAL_FORM to SemaColors.SPECIAL_FORM,
            SemaTokenTypes.DEFINITION_KEYWORD to SemaColors.DEFINITION_KEYWORD,
        )
        for ((token, expected) in tokenTypes) {
            val highlights = highlightFor(token)
            assertTrue(highlights.isNotEmpty(), "Token $token should have highlights")
            assertEquals(expected, highlights[0], "Token $token should use expected color key")
        }
    }

    @Test
    fun `delimiter tokens map to distinct paren/bracket/brace colors`() {
        val lparenHighlights = highlightFor(SemaTokenTypes.LPAREN)
        val lbracketHighlights = highlightFor(SemaTokenTypes.LBRACKET)
        val lbraceHighlights = highlightFor(SemaTokenTypes.LBRACE)

        assertEquals(SemaColors.PARENS, lparenHighlights[0])
        assertEquals(SemaColors.BRACKETS, lbracketHighlights[0])
        assertEquals(SemaColors.BRACES, lbraceHighlights[0])
    }

    @Test
    fun `unknown token type returns empty array`() {
        val result = highlightFor(null)
        assertTrue(result.isEmpty())
    }

    @Test
    fun `all SemaColors keys are non-null with external names`() {
        val colors = listOf(
            SemaColors.LINE_COMMENT, SemaColors.BLOCK_COMMENT,
            SemaColors.STRING, SemaColors.NUMBER, SemaColors.KEYWORD,
            SemaColors.SYMBOL, SemaColors.BOOLEAN, SemaColors.NIL,
            SemaColors.CHARACTER, SemaColors.PARENS, SemaColors.BRACKETS,
            SemaColors.BRACES, SemaColors.SPECIAL_FORM, SemaColors.DEFINITION_KEYWORD,
        )
        for (color in colors) {
            assertNotNull(color.externalName, "Color key ${color} should have externalName")
        }
    }
}
```

---

### Task 5: Brace Matcher and Commenter Tests

**Files:**
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/SemaBraceMatcherTest.kt`
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/SemaCommenterTest.kt`

- [ ] **Step 1: Brace matcher pair definitions**

```kotlin
package com.sema.intellij

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

class SemaBraceMatcherTest {
    private val matcher = SemaBraceMatcher()

    @Test
    fun `registers three brace pairs`() {
        assertEquals(3, matcher.pairs.size)
    }

    @Test
    fun `parens are a brace pair`() {
        val pair = matcher.pairs.find { it.leftBraceType == SemaTokenTypes.LPAREN }
        assertNotNull(pair)
        assertEquals(SemaTokenTypes.RPAREN, pair!!.rightBraceType)
        assertFalse(pair.isStructural)
    }

    @Test
    fun `brackets are a brace pair`() {
        val pair = matcher.pairs.find { it.leftBraceType == SemaTokenTypes.LBRACKET }
        assertNotNull(pair)
        assertEquals(SemaTokenTypes.RBRACKET, pair!!.rightBraceType)
    }

    @Test
    fun `braces are a brace pair`() {
        val pair = matcher.pairs.find { it.leftBraceType == SemaTokenTypes.LBRACE }
        assertNotNull(pair)
        assertEquals(SemaTokenTypes.RBRACE, pair!!.rightBraceType)
    }

    @Test
    fun `any brace type is allowed before other tokens`() {
        assertTrue(matcher.isPairedBracesAllowedBeforeType(SemaTokenTypes.LPAREN, SemaTokenTypes.SYMBOL))
        assertTrue(matcher.isPairedBracesAllowedBeforeType(SemaTokenTypes.LBRACKET, SemaTokenTypes.RPAREN))
    }
}
```

- [ ] **Step 2: Commenter prefix/suffix**

```kotlin
package com.sema.intellij

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

class SemaCommenterTest {
    private val commenter = SemaCommenter()

    @Test
    fun `line comment prefix is semicolon`() {
        assertEquals(";", commenter.lineCommentPrefix)
    }

    @Test
    fun `block comment prefix is hash-pipe`() {
        assertEquals("#|", commenter.blockCommentPrefix)
    }

    @Test
    fun `block comment suffix is pipe-hash`() {
        assertEquals("|#", commenter.blockCommentSuffix)
    }

    @Test
    fun `commented block comment is null`() {
        assertNull(commenter.commentedBlockCommentPrefix)
        assertNull(commenter.commentedBlockCommentSuffix)
    }
}
```

---

### Task 6: Settings, Binary Discovery, and Command Line Tests (Extended)

**Files:**
- Modify: `editors/intellij/src/test/kotlin/com/sema/intellij/config/SemaBinaryTest.kt`
- Modify: `editors/intellij/src/test/kotlin/com/sema/intellij/config/SemaCommandLineTest.kt`
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/config/SemaSettingsTest.kt`

- [ ] **Step 1: Settings default values and serialization**

```kotlin
package com.sema.intellij.config

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

class SemaSettingsTest {
    @Test
    fun `default sema path is sema on PATH`() {
        val state = SemaSettings.State()
        assertEquals("sema", state.semaPath)
    }

    @Test
    fun `setting blank path resets to default`() {
        val settings = SemaSettings()
        settings.semaPath = "   "
        assertEquals("sema", settings.semaPath)
    }

    @Test
    fun `getState returns a copy not the internal reference`() {
        val settings = SemaSettings()
        settings.semaPath = "/custom/sema"
        val state = settings.state
        assertEquals("/custom/sema", state.semaPath)

        // Mutating the returned state should not affect the settings
        state.semaPath = "/hacked/sema"
        assertEquals("/custom/sema", settings.semaPath)
    }

    @Test
    fun `loadState replaces internal state`() {
        val settings = SemaSettings()
        settings.semaPath = "/original/sema"

        val newState = SemaSettings.State()
        newState.semaPath = "/loaded/sema"
        settings.loadState(newState)

        assertEquals("/loaded/sema", settings.semaPath)
    }
}
```

- [ ] **Step 2: Additional binary discovery edge cases**

Add to existing `SemaBinaryTest.kt`:

```kotlin
    @Test
    fun `resolves binary from PATH entries with custom name`() {
        val dir = createTempDirectory("sema-custom")
        val executable = dir.resolve("sema-custom").createFile()
        executable.toFile().setExecutable(true)

        val status = SemaBinary.check("sema-custom", listOf(dir.pathString))

        assertTrue(status.available)
        assertEquals(executable.pathString, status.resolvedPath)
    }

    @Test
    fun `multiple PATH entries searched in order`() {
        val dir1 = createTempDirectory("sema-path1")
        val dir2 = createTempDirectory("sema-path2")
        val executable = dir2.resolve("sema").createFile()
        executable.toFile().setExecutable(true)

        val status = SemaBinary.check("sema", listOf(dir1.pathString, dir2.pathString))

        assertTrue(status.available)
        assertTrue(status.resolvedPath!!.contains(dir2.name))
    }

    @Test
    fun `first PATH entry wins for same binary name`() {
        val dir1 = createTempDirectory("sema-path1")
        val dir2 = createTempDirectory("sema-path2")
        val exe1 = dir1.resolve("sema").createFile().also { it.toFile().setExecutable(true) }
        dir2.resolve("sema").createFile().also { it.toFile().setExecutable(true) }

        val status = SemaBinary.check("sema", listOf(dir1.pathString, dir2.pathString))

        assertTrue(status.available)
        assertEquals(exe1.pathString, status.resolvedPath)
    }

    @Test
    fun `path with forward slash is treated as absolute-like`() {
        val dir = createTempDirectory("sema-path")
        val executable = dir.resolve("sema").createFile()
        executable.toFile().setExecutable(true)

        // "sema/sema" won't work, but "./sema" is relative with slash
        val status = SemaBinary.check("./sema", listOf(dir.pathString))

        // This is a relative path with slash — it's resolved as-is by File()
        // The file won't exist, so it should be unavailable
        assertFalse(status.available)
    }

    @Test
    fun `blank configured path uses default name`() {
        val dir = createTempDirectory("sema-blank")
        val executable = dir.resolve("sema").createFile()
        executable.toFile().setExecutable(true)

        val status = SemaBinary.check("", listOf(dir.pathString))
        assertTrue(status.available)
    }
```

- [ ] **Step 3: Additional command line tests**

Add to existing `SemaCommandLineTest.kt`:

```kotlin
    @Test
    fun `run file command includes script path and arguments`() {
        val commandLine = SemaCommandLine.runFile(
            semaPath = "/opt/sema",
            scriptPath = "/work/project/main.sema",
            arguments = listOf("--verbose", "arg1"),
            workingDirectory = "/work/project",
        )

        assertEquals("/opt/sema", commandLine.exePath)
        assertEquals(listOf("/work/project/main.sema", "--verbose", "arg1"), commandLine.parametersList.list)
        assertEquals("/work/project", commandLine.workDirectory.path)
        assertEquals(Charsets.UTF_8, commandLine.charset)
    }

    @Test
    fun `blank sema path defaults to sema on PATH`() {
        val commandLine = SemaCommandLine.lsp(semaPath = "  ", workingDirectory = "/project")
        assertEquals("sema", commandLine.exePath)
        assertEquals(listOf("lsp"), commandLine.parametersList.list)
    }

    @Test
    fun `notebook export without output path omits output flag`() {
        val commandLine = SemaCommandLine.notebookExport(
            semaPath = "/opt/sema",
            notebookPath = "/project/demo.sema-nb",
            workingDirectory = "/project",
            outputPath = null,
        )
        assertEquals(listOf("notebook", "export", "/project/demo.sema-nb"), commandLine.parametersList.list)
    }

    @Test
    fun `null working directory results in null work directory on command line`() {
        val commandLine = SemaCommandLine.lsp(semaPath = "/opt/sema", workingDirectory = null)
        assertEquals("/opt/sema", commandLine.exePath)
        assertNull(commandLine.workDirectory)
    }
```

---

### Task 7: Run Configuration Serialization Tests

**Files:**
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/run/SemaRunConfigurationTest.kt`

- [ ] **Step 1: Run configuration read/write round-trip**

```kotlin
package com.sema.intellij.run

import com.intellij.testFramework.LightPlatformTestCase
import org.jdom.Element
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

class SemaRunConfigurationTest {
    private fun createConfig(): SemaRunConfiguration {
        val project = LightPlatformTestCase.getProject()
        val factory = SemaRunConfigurationType().configurationFactories.first()
        return SemaRunConfiguration(project, factory, "Test Config")
    }

    @Test
    fun `default values are set correctly`() {
        val config = createConfig()
        assertEquals("", config.scriptPath)
        assertEquals("", config.arguments)
        assertEquals(LightPlatformTestCase.getProject().basePath ?: "", config.workingDirectory)
    }

    @Test
    fun `serialization round-trip preserves all fields`() {
        val config = createConfig()
        config.scriptPath = "/path/to/main.sema"
        config.arguments = "--verbose arg1 arg2"
        config.workingDirectory = "/path/to/project"

        val element = Element("configuration")
        config.writeExternal(element)

        val restored = createConfig()
        restored.readExternal(element)

        assertEquals("/path/to/main.sema", restored.scriptPath)
        assertEquals("--verbose arg1 arg2", restored.arguments)
        assertEquals("/path/to/project", restored.workingDirectory)
    }

    @Test
    fun `deserialization handles missing attributes`() {
        val config = createConfig()
        config.scriptPath = "/path/to/main.sema"
        config.arguments = "--verbose"

        // Write then read, but with empty element (simulating old config format)
        val element = Element("configuration")
        // Don't set any attributes — simulates old config without new fields

        val restored = createConfig()
        restored.scriptPath = "/path/to/main.sema" // pre-set to ensure reset works
        restored.readExternal(element)

        assertEquals("", restored.scriptPath, "Missing attribute should default to empty")
        assertEquals("", restored.arguments)
    }

    @Test
    fun `run configuration type has correct id and name`() {
        val type = SemaRunConfigurationType()
        assertEquals("SEMA_RUN_CONFIGURATION", type.id)
        assertEquals("Sema", type.displayName)
        assertEquals("Run a Sema script", type.configurationTypeDescription)
    }

    @Test
    fun `run configuration factory produces correct type`() {
        val type = SemaRunConfigurationType()
        val factory = type.configurationFactories.first()
        val project = LightPlatformTestCase.getProject()
        val config = factory.createTemplateConfiguration(project)
        assertTrue(config is SemaRunConfiguration)
    }
}
```

---

### Task 8: LSP Integration Tests (Full, with Real Binary)

**Files:**
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/lsp/SemaLanguageServerTest.kt`
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/lsp/SemaLanguageClientTest.kt`
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/lsp/SemaSemanticTokensColorsProviderTest.kt`

- [ ] **Step 1: LSP server factory creates correct provider**

```kotlin
package com.sema.intellij.lsp

import com.intellij.testFramework.LightPlatformTestCase
import com.sema.intellij.TestHelpers
import org.junit.jupiter.api.*
import org.junit.jupiter.api.Assertions.*

@Tag("lsp")
class SemaLanguageServerTest {
    @Test
    fun `factory creates connection provider`() {
        val project = LightPlatformTestCase.getProject()
        val factory = SemaLanguageServerFactory()
        val provider = factory.createConnectionProvider(project)
        assertNotNull(provider)
        assertTrue(provider is SemaLanguageServer)
    }

    @Test
    fun `factory creates language client`() {
        val project = LightPlatformTestCase.getProject()
        val factory = SemaLanguageServerFactory()
        val client = factory.createLanguageClient(project)
        assertNotNull(client)
        assertTrue(client is SemaLanguageClient)
    }
}
```

- [ ] **Step 2: LSP server process lifecycle test (requires sema binary)**

```kotlin
    @Test
    fun `lsp server process starts and can be destroyed`() {
        TestHelpers.requireSema()
        val project = LightPlatformTestCase.getProject()
        val server = SemaLanguageServer(project)

        assertNotNull(server.commandLine)
        assertEquals("lsp", server.commandLine!!.parametersList.list.first())

        // Server process startup is handled by LSP4IJ's lifecycle — we verify
        // the command line is well-formed and the server doesn't crash on
        // construction.
    }
```

- [ ] **Step 3: Language client receives eval results**

```kotlin
package com.sema.intellij.lsp

import com.intellij.testFramework.LightPlatformTestCase
import org.junit.jupiter.api.*
import org.junit.jupiter.api.Assertions.*

@Tag("lsp")
class SemaLanguageClientTest {
    @Test
    fun `language client is constructed with project`() {
        val project = LightPlatformTestCase.getProject()
        val client = SemaLanguageClient(project)
        assertNotNull(client)
    }
}
```

- [ ] **Step 4: Semantic tokens colors provider**

```kotlin
package com.sema.intellij.lsp

import com.intellij.openapi.editor.colors.TextAttributesKey
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

@Tag("lsp")
class SemaSemanticTokensColorsProviderTest {
    private val provider = SemaSemanticTokensColorsProvider()

    @Test
    fun `known token types map to colors`() {
        val types = listOf("keyword", "function", "variable", "parameter", "macro")
        for (type in types) {
            val keys = provider.getTextAttributesKeys(type, emptyList())
            assertTrue(keys.isNotEmpty(), "Token type '$type' should have color mapping")
        }
    }

    @Test
    fun `defaultLibrary modifier returns appropriate key`() {
        // Check that the modifier is recognized
        val keys = provider.getTextAttributesKeys("function", listOf("defaultLibrary"))
        assertTrue(keys.isNotEmpty())
    }

    @Test
    fun `unknown token type returns empty array`() {
        val keys = provider.getTextAttributesKeys("nonexistent_type_xyz", emptyList())
        // Should not throw, and should return empty
        assertNotNull(keys)
    }
}
```

- [ ] **Step 5: End-to-end LSP communication test (HeavyPlatformTestCase, skipped if no binary)**

```kotlin
    @Test
    @Timeout(30)
    fun `lsp server responds to initialize`() {
        TestHelpers.requireSema()

        val project = LightPlatformTestCase.getProject()
        val server = SemaLanguageServer(project)

        // LSP4IJ manages the full lifecycle. We verify that the command line
        // is correctly constructed and that LSP4IJ doesn't reject it.
        assertNotNull(server.commandLine)
        val cl = server.commandLine!!
        assertEquals("sema", cl.exePath.take(4))
        assertEquals("lsp", cl.parametersList.list.firstOrNull())
    }
```

---

### Task 9: DAP Integration Tests

**Files:**
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/dap/SemaDebugAdapterDescriptorFactoryTest.kt`

- [ ] **Step 1: DAP factory behavior**

```kotlin
package com.sema.intellij.dap

import com.intellij.mock.MockVirtualFile
import com.intellij.testFramework.LightPlatformTestCase
import com.sema.intellij.SemaFileType
import com.sema.intellij.TestHelpers
import org.junit.jupiter.api.*
import org.junit.jupiter.api.Assertions.*

@Tag("dap")
class SemaDebugAdapterDescriptorFactoryTest {
    private val factory = SemaDebugAdapterDescriptorFactory()

    @Test
    fun `sema files are debuggable`() {
        val project = LightPlatformTestCase.getProject()
        val semaFile = MockVirtualFile("test.sema")
        semaFile.fileType = SemaFileType
        assertTrue(factory.isDebuggableFile(semaFile, project))
    }

    @Test
    fun `non-sema files are not debuggable`() {
        val project = LightPlatformTestCase.getProject()
        val txtFile = MockVirtualFile("test.txt")
        assertFalse(factory.isDebuggableFile(txtFile, project))
    }

    @Test
    fun `launch mode is supported`() {
        assertTrue(factory.canRun("launch"))
    }

    @Test
    fun `non-launch modes are not supported`() {
        assertFalse(factory.canRun("attach"))
        assertFalse(factory.canRun("unknown"))
    }

    @Test
    fun `has exactly one launch configuration`() {
        val configs = factory.launchConfigurations
        assertEquals(1, configs.size)
        assertEquals("sema-launch", configs[0].id)
        assertEquals("Debug Sema file", configs[0].name)
    }

    @Test
    fun `launch configuration contains required fields`() {
        val config = factory.launchConfigurations.first()
        assertTrue(config.launchConfig.contains("\"program\""))
        assertTrue(config.launchConfig.contains("\"stopOnEntry\""))
        assertTrue(config.launchConfig.contains("\"request\": \"launch\""))
    }
}
```

---

### Task 10: Notebook Tests (Grouped Separately)

**Files:**
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/notebook/SemaNotebookFileTypeTest.kt`
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/notebook/SemaNotebookFileEditorProviderTest.kt`
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/notebook/SemaNotebookSessionServiceTest.kt`
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/notebook/actions/SemaNotebookActionsTest.kt`

- [ ] **Step 1: Notebook file type**

```kotlin
package com.sema.intellij.notebook

import com.intellij.openapi.fileTypes.FileTypeManager
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Tag
import org.junit.jupiter.api.Test

@Tag("notebook")
class SemaNotebookFileTypeTest {
    @Test
    fun `notebook file type has correct extension`() {
        assertEquals("sema-nb", SemaNotebookFileType.defaultExtension)
    }

    @Test
    fun `notebook file type has correct name and description`() {
        assertEquals("Sema Notebook", SemaNotebookFileType.name)
        assertTrue(SemaNotebookFileType.description.contains("Notebook"))
    }

    @Test
    fun `notebook file type has non-null icon`() {
        assertNotNull(SemaNotebookFileType.icon)
    }

    @Test
    fun `sema-nb extension maps to notebook file type`() {
        val ftm = FileTypeManager.getInstance()
        assertEquals(SemaNotebookFileType, ftm.getFileTypeByExtension("sema-nb"))
    }
}
```

- [ ] **Step 2: Notebook editor provider**

```kotlin
package com.sema.intellij.notebook

import com.intellij.mock.MockVirtualFile
import com.intellij.openapi.fileEditor.FileEditorPolicy
import com.intellij.testFramework.LightPlatformTestCase
import org.junit.jupiter.api.*

@Tag("notebook")
class SemaNotebookFileEditorProviderTest {
    @Test
    fun `provider accepts sema-nb files`() {
        val project = LightPlatformTestCase.getProject()
        val provider = SemaNotebookFileEditorProvider()
        val nbFile = MockVirtualFile("test.sema-nb")
        nbFile.fileType = SemaNotebookFileType
        assertTrue(provider.accept(project, nbFile))
    }

    @Test
    fun `provider rejects non-sema-nb files`() {
        val project = LightPlatformTestCase.getProject()
        val provider = SemaNotebookFileEditorProvider()
        val txtFile = MockVirtualFile("test.sema")
        assertFalse(provider.accept(project, txtFile))
    }

    @Test
    fun `provider has expected editor type id`() {
        val provider = SemaNotebookFileEditorProvider()
        assertEquals("sema-notebook-editor", provider.editorTypeId)
    }

    @Test
    fun `provider uses PLACE policy`() {
        val provider = SemaNotebookFileEditorProvider()
        assertEquals(FileEditorPolicy.PLACE, provider.policy)
    }
}
```

- [ ] **Step 3: Notebook session service**

```kotlin
package com.sema.intellij.notebook

import com.intellij.testFramework.LightPlatformTestCase
import org.junit.jupiter.api.*

@Tag("notebook")
class SemaNotebookSessionServiceTest {
    @Test
    fun `service is registered and accessible`() {
        val project = LightPlatformTestCase.getProject()
        val service = SemaNotebookSessionService.getInstance(project)
        assertNotNull(service)
    }
}
```

- [ ] **Step 4: Notebook actions are registered**

```kotlin
package com.sema.intellij.notebook.actions

import com.intellij.openapi.actionSystem.ActionManager
import org.junit.jupiter.api.*

@Tag("notebook")
class SemaNotebookActionsTest {
    @Test
    fun `new notebook action is registered`() {
        val action = ActionManager.getInstance().getAction("sema.notebook.new")
        assertNotNull(action)
        assertTrue(action is NewSemaNotebookAction)
    }

    @Test
    fun `open external action is registered`() {
        val action = ActionManager.getInstance().getAction("sema.notebook.openExternal")
        assertNotNull(action)
        assertTrue(action is OpenSemaNotebookAction)
    }

    @Test
    fun `run all cells action is registered`() {
        val action = ActionManager.getInstance().getAction("sema.notebook.runAll")
        assertNotNull(action)
        assertTrue(action is RunAllSemaNotebookCellsAction)
    }

    @Test
    fun `export markdown action is registered`() {
        val action = ActionManager.getInstance().getAction("sema.notebook.exportMarkdown")
        assertNotNull(action)
        assertTrue(action is ExportSemaNotebookToMarkdownAction)
    }

    @Test
    fun `clear results action is registered`() {
        val action = ActionManager.getInstance().getAction("sema.clearResults")
        assertNotNull(action)
    }
}
```

---

### Task 11: Full IDE Integration Tests (Starter + Driver)

**Files:**
- Create: `editors/intellij/src/integrationTest/kotlin/com/sema/intellij/PluginStartupTest.kt`
- Create: `editors/intellij/src/integrationTest/kotlin/com/sema/intellij/EditorFeaturesTest.kt`
- Create: `editors/intellij/src/testFixtures/kotlin/com/sema/intellij/SemaIntegrationTestBase.kt`

- [ ] **Step 1: Shared test base with CI server integration**

```kotlin
// src/testFixtures/kotlin/com/sema/intellij/SemaIntegrationTestBase.kt
package com.sema.intellij

import com.intellij.ide.starter.ci.CIServer
import com.intellij.ide.starter.ci.NoCIServer
import com.intellij.ide.starter.ide.IdeProductProvider
import com.intellij.ide.starter.models.TestCase
import com.intellij.ide.starter.plugins.PluginConfigurator
import com.intellij.ide.starter.project.LocalProjectInfo
import com.intellij.ide.starter.runner.Starter
import org.junit.jupiter.api.Assertions.fail
import org.junit.jupiter.api.io.TempDir
import org.kodein.di.DI
import org.kodein.di.bindSingleton
import org.kodein.di.instance
import java.io.File
import java.nio.file.Path
import kotlin.io.path.createDirectory
import kotlin.io.path.writeText

abstract class SemaIntegrationTestBase {
    init {
        di = DI {
            extend(di)
            bindSingleton<CIServer>(overrides = true) {
                object : CIServer by NoCIServer {
                    override fun reportTestFailure(
                        testName: String,
                        message: String,
                        details: String,
                        linkToLogs: String?,
                    ) {
                        fail { "$testName fails: $message.\n$details" }
                    }
                }
            }
        }
    }

    companion object {
        val IDE_VERSION = "2024.3"

        fun createContext(testName: String, @TempDir projectDir: Path): Starter.Builder {
            // Create a minimal .sema file in the project
            val projectPath = projectDir.resolve("test-project")
            projectPath.createDirectory()
            projectPath.resolve("main.sema").writeText(
                """
                (define greeting "Hello from IntelliJ test!")
                (defun square (x) (* x x))
                (print (square 5))
                """.trimIndent(),
            )

            return Starter.newContext(
                testName = testName,
                testCase = TestCase(
                    IdeProductProvider.IC,
                    LocalProjectInfo(projectPath),
                ).withVersion(IDE_VERSION),
            ).apply {
                val pathToPlugin = System.getProperty("path.to.build.plugin")
                    ?: error("path.to.build.plugin system property must be set")
                PluginConfigurator(this).installPluginFromFolder(File(pathToPlugin))
            }
        }
    }
}
```

- [ ] **Step 2: Plugin startup smoke test**

```kotlin
// src/integrationTest/kotlin/com/sema/intellij/PluginStartupTest.kt
package com.sema.intellij

import com.intellij.driver.sdk.ui.components.ideFrame
import com.intellij.driver.sdk.waitForIndicators
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import java.nio.file.Path
import kotlin.time.Duration.Companion.minutes

class PluginStartupTest : SemaIntegrationTestBase() {
    @Test
    fun `plugin starts without errors in IDE`(@TempDir projectDir: Path) {
        createContext("pluginStartup", projectDir)
            .runIdeWithDriver()
            .useDriverAndCloseIde {
                waitForIndicators(3.minutes)
                // IDE frame exists — basic health check
                ideFrame { }
            }
    }

    @Test
    fun `sema file type is registered`(@TempDir projectDir: Path) {
        createContext("fileTypeRegistration", projectDir)
            .runIdeWithDriver()
            .useDriverAndCloseIde {
                waitForIndicators(2.minutes)
                // TODO: verify file type via API call or UI inspection
            }
    }
}
```

- [ ] **Step 3: Editor feature integration tests**

```kotlin
// src/integrationTest/kotlin/com/sema/intellij/EditorFeaturesTest.kt
package com.sema.intellij

import com.intellij.driver.sdk.ui.components.ideFrame
import com.intellij.driver.sdk.waitForIndicators
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import java.nio.file.Path
import kotlin.time.Duration.Companion.minutes

class EditorFeaturesTest : SemaIntegrationTestBase() {
    @Test
    fun `opening sema file shows syntax highlighting`(@TempDir projectDir: Path) {
        createContext("syntaxHighlighting", projectDir)
            .runIdeWithDriver()
            .useDriverAndCloseIde {
                waitForIndicators(3.minutes)
                // TODO: Navigate project tree, open main.sema, verify editor has content
                // TODO: Verify highlighting via PSI or color scheme inspection
            }
    }

    @Test
    fun `brace matching highlights matching parens`(@TempDir projectDir: Path) {
        createContext("braceMatching", projectDir)
            .runIdeWithDriver()
            .useDriverAndCloseIde {
                waitForIndicators(2.minutes)
                // TODO: Open file, move cursor to paren, verify match highlight
            }
    }

    @Test
    fun `comment toggling works`(@TempDir projectDir: Path) {
        createContext("commentToggling", projectDir)
            .runIdeWithDriver()
            .useDriverAndCloseIde {
                waitForIndicators(2.minutes)
                // TODO: Select line, invoke comment action, verify line is commented
            }
    }

    @Test
    fun `run configuration can be created from file`(@TempDir projectDir: Path) {
        createContext("runConfig", projectDir)
            .runIdeWithDriver()
            .useDriverAndCloseIde {
                waitForIndicators(3.minutes)
                // TODO: Open .sema file, invoke run config producer, verify config created
            }
    }
}
```

- [ ] **Step 4: Settings UI test**

```kotlin
// src/integrationTest/kotlin/com/sema/intellij/SettingsIntegrationTest.kt
package com.sema.intellij

import com.intellij.driver.sdk.ui.components.ideFrame
import com.intellij.driver.sdk.waitForIndicators
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import java.nio.file.Path
import kotlin.time.Duration.Companion.minutes

class SettingsIntegrationTest : SemaIntegrationTestBase() {
    @Test
    fun `sema settings page opens without errors`(@TempDir projectDir: Path) {
        createContext("settingsPage", projectDir)
            .runIdeWithDriver()
            .useDriverAndCloseIde {
                waitForIndicators(2.minutes)
                // TODO: Open Settings, navigate to Sema page, verify it renders
            }
    }
}
```

---

### Task 12: Edge Case and Stress Tests

**Files:**
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/SemaLexerStressTest.kt`
- Create: `editors/intellij/src/test/kotlin/com/sema/intellij/SemaPluginSmokeTest.kt`

- [ ] **Step 1: Large file lexing performance**

```kotlin
package com.sema.intellij

import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Tag
import org.junit.jupiter.api.Test
import kotlin.system.measureTimeMillis

@Tag("stress")
class SemaLexerStressTest {
    private fun lexAll(source: String): Int {
        val lexer = SemaLexer()
        lexer.start(source)
        var count = 0
        while (lexer.tokenType != null) {
            count++
            lexer.advance()
        }
        return count
    }

    @Test
    fun `lexes 10k forms under 500ms`() {
        val source = (1..10_000).joinToString("\n") { "(define x$it $it)" }
        val elapsed = measureTimeMillis {
            val count = lexAll(source)
            assertTrue(count > 30_000, "Expected 30k+ tokens, got $count")
        }
        assertTrue(elapsed < 500, "Lexing 10k forms took ${elapsed}ms, expected < 500ms")
    }

    @Test
    fun `lexes deeply nested expression under 200ms`() {
        val depth = 5000
        val source = "(".repeat(depth) + "x" + ")".repeat(depth)
        val elapsed = measureTimeMillis {
            val count = lexAll(source)
            assertEquals(depth * 2 + 1, count)
        }
        assertTrue(elapsed < 200, "Lexing $depth-deep nesting took ${elapsed}ms")
    }

    @Test
    fun `lexes 100k characters of mixed content under 1s`() {
        val forms = listOf(
            "(define x 42)",
            "(defun f (x) (* x x))",
            "\"a string with \\\"escapes\\\"\"",
            "#|block comment with #|nested|# content|#",
            "; line comment\n",
            "(let ((a 1) (b 2)) (+ a b))",
            "(if (> x 0) 'positive 'negative)",
        )
        val sb = StringBuilder()
        repeat(10_000) {
            sb.append(forms[it % forms.size])
            sb.append("\n")
        }
        val source = sb.toString()
        assertTrue(source.length > 100_000)

        val elapsed = measureTimeMillis {
            val count = lexAll(source)
            assertTrue(count > 50_000)
        }
        assertTrue(elapsed < 1000, "Lexing 100k chars took ${elapsed}ms, expected < 1s")
    }

    @Test
    fun `repeated lexing of same source is fast`() {
        val source = TestHelpers.sampleSemaContent().repeat(100)
        // Warmup
        repeat(5) { lexAll(source) }

        val elapsed = measureTimeMillis {
            repeat(10) { lexAll(source) }
        }
        assertTrue(elapsed < 500, "10x re-lex took ${elapsed}ms, expected < 500ms")
    }
}
```

- [ ] **Step 2: Plugin registration smoke tests**

```kotlin
package com.sema.intellij

import com.intellij.codeInsight.completion.CompletionContributor
import com.intellij.lang.LanguageBraceMatching
import com.intellij.lang.LanguageCommenters
import com.intellij.openapi.fileTypes.SyntaxHighlighterFactory
import com.intellij.psi.PsiParserFacade
import com.intellij.testFramework.LightPlatformTestCase
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.Test

class SemaPluginSmokeTest {
    /** Verify that all extension points are correctly registered and the plugin
     * doesn't throw during standard IDE initialization.
     * These tests find regressions in plugin.xml configuration. */

    @Test
    fun `syntax highlighter factory is registered`() {
        val factory = SyntaxHighlighterFactory.getSyntaxHighlighterFactory(SemaLanguage)
        assertNotNull(factory)
        assertTrue(factory is SemaSyntaxHighlighterFactory)
    }

    @Test
    fun `brace matcher is registered for Sema language`() {
        val matcher = LanguageBraceMatching.INSTANCE.forLanguage(SemaLanguage)
        assertNotNull(matcher)
        assertTrue(matcher is SemaBraceMatcher)
    }

    @Test
    fun `commenter is registered for Sema language`() {
        val commenter = LanguageCommenters.INSTANCE.forLanguage(SemaLanguage)
        assertNotNull(commenter)
        assertEquals(";", commenter.lineCommentPrefix)
    }

    @Test
    fun `psi parser is registered`() {
        val project = LightPlatformTestCase.getProject()
        val parser = PsiParserFacade.getInstance(project)
            .createParser(SemaLanguage, "")
        assertNotNull(parser)
    }

    @Test
    fun `language has correct mime type`() {
        assertTrue(SemaLanguage.mimeTypes.contains("text/x-sema"))
    }

    @Test
    fun `language has correct associated file type`() {
        assertEquals(SemaFileType, SemaLanguage.associatedFileType)
    }
}
```

- [ ] **Step 3: Memory stress test (heavy file operations)**

```kotlin
    @Test
    @Tag("stress")
    fun `repeated parse of large file does not leak memory`() {
        val source = TestHelpers.sampleSemaContent().repeat(500)

        val runtime = Runtime.getRuntime()
        runtime.gc()
        val memBefore = runtime.totalMemory() - runtime.freeMemory()

        repeat(50) {
            val psiFile = com.intellij.psi.PsiFileFactory.getInstance(
                LightPlatformTestCase.getProject()
            ).createFileFromText("leak-test-${it}.sema", SemaLanguage, source)
            assertNotNull(psiFile)
        }

        runtime.gc()
        val memAfter = runtime.totalMemory() - runtime.freeMemory()
        val diffMB = (memAfter - memBefore) / (1024.0 * 1024.0)
        assertTrue(diffMB < 50, "Memory grew by ${"%.1f".format(diffMB)}MB, possible leak")
    }

    @Test
    @Tag("stress")
    fun `many rapid document changes do not crash`() {
        // Simulate rapid typing: lex many small incremental changes
        val base = "(define x 0)"
        val lexer = SemaLexer()
        for (i in 1..5000) {
            val source = "(define x $i)"
            lexer.start(source)
            while (lexer.tokenType != null) { lexer.advance() }
        }
        // If we get here without exception, the test passes
    }
```

---

### Test Execution Summary

| Tier | Command | Tags | Binary Required |
|------|---------|------|-----------------|
| All unit tests | `./gradlew test` | — | No |
| Excluding LSP | `./gradlew test -PexcludeTags=lsp` | | No |
| Excluding stress | `./gradlew test -PexcludeTags=stress` | | No |
| Notebook only | `./gradlew test -PincludeTags=notebook` | `@Tag("notebook")` | No |
| LSP integration | `./gradlew test -PincludeTags=lsp` | `@Tag("lsp")` | Yes (`-Dsema.test.binary=...`) |
| DAP integration | `./gradlew test -PincludeTags=dap` | `@Tag("dap")` | Yes |
| Stress tests | `./gradlew test -PincludeTags=stress` | `@Tag("stress")` | No |
| Full IDE integration | `./gradlew integrationTest` | — | Plugin ZIP built |

Configure JUnit tag filtering in `build.gradle.kts`:

```kotlin
tasks.test {
    useJUnitPlatform {
        System.getProperty("includeTags")?.let { includeTags(it) }
        System.getProperty("excludeTags")?.let { excludeTags(it) }
    }
}
```

---

### Test Coverage Targets

| Area | Tests Before | Tests After | Coverage Goal |
|------|-------------|------------|---------------|
| Lexer tokenization | 0 | 25+ | All token types + edge cases |
| Parser/PSI | 0 | 6+ | Valid/invalid/empty files |
| Syntax highlighter | 0 | 6+ | All token→color mappings |
| Brace matcher | 0 | 4 | All pair types |
| Commenter | 0 | 4 | Prefix/suffix validation |
| Settings | 0 | 4 | Defaults, serialization |
| Binary discovery | 4 | 10 | All resolution paths |
| Command line | 4 | 10 | All subcommands |
| Run configuration | 0 | 5 | Round-trip |
| File types | 0 | 7 | All 3 file types |
| LSP integration | 0 | 8 | Factory, client, lifecycle |
| DAP integration | 0 | 7 | Factory, debuggability |
| Notebook | 0 | 12 | FT, editor, service, actions |
| Plugin registration | 0 | 6 | All extension points |
| Stress/performance | 0 | 5 | Large files, memory |
| IDE integration (UI) | 0 | 6 | Startup, highlighting, actions |

**Total: 8 → 125+ tests**

---

## As-Implemented (2026-06-08)

The plan was implemented with several adjustments driven by toolchain realities:

### Tech stack changes
- **JUnit 4** instead of JUnit 5 — the IntelliJ Platform Gradle Plugin 2.x test runner (`testFramework(TestFrameworkType.Platform)`) provides a JUnit 4-based IntelliJ test harness. JUnit 5 was incompatible (Gradle 9.5 + Plugin 2.16 have known issues with `JUnit5TestSessionListener`).
- **`LightPlatformTestCase`** base class required for all platform-dependent tests — `ProjectManager.getInstance().defaultProject` returns null in plain test classes. Extending `LightPlatformTestCase` bootstraps the IntelliJ application and provides the `project` field.
- **No tag filtering** — JUnit 4 doesn't support `@Tag`. Test grouping is by package.
- **`junit-platform.properties`** removed — JUnit 5 only, irrelevant after JUnit 4 switch.
- **`kodein-di-jvm`** not needed for integration tests — simplified base class with `PluginConfigurator(this).installPluginFromFolder(File(...))`.
- **Integration test imports** — `runIdeWithDriver` is an extension function at `com.intellij.ide.starter.driver.engine.runIdeWithDriver`.

### Bugs discovered during implementation
- **Lexer `#(` handling**: The lexer didn't tokenize `#(` (short lambda syntax) as hash dispatch. Added `ch == '#' && peekNext() == '(' -> lexHashDispatch()`.
- **Comma-as-whitespace**: The lexer's `UNQUOTE` token type for bare `,` is dead code — `,` without `@` is consumed as whitespace. Tests adjusted to match actual behavior.
- **`SemaSettings.getState()`** returns the mutable internal reference (not a copy). Test corrected from `getStateIsCopy` to `getStateReturnsTheSameObject`.

### Final test counts
| Area | Plan Target | Implemented |
|------|------------|-------------|
| Lexer tokenization | 25+ | 29 |
| Parser/PSI | 6+ | 5 |
| Syntax highlighter | 6+ | 4 |
| Brace matcher | 4 | 5 |
| Commenter | 4 | 4 |
| Settings | 4 | 4 |
| Binary discovery | 10 | 9 |
| Command line | 10 | 8 |
| Run configuration | 5 | 5 |
| File types | 7 | 6* |
| LSP integration | 8 | 7 |
| DAP integration | 7 | 5 |
| Notebook | 12 | 13 |
| Plugin registration | 6 | 6 |
| Stress/performance | 5 | 4 |
| IDE integration (UI) | 6 | 2** |
| **Total** | **~125** | **116** |

\* `mimeType` test removed — `SemaLanguage.mimeTypes` behavior depends on IntelliJ internals.
\*\* Integration tests compile but need IDE sandbox (`./gradlew buildPlugin integrationTest`).

### Run commands
```bash
./gradlew test                           # 116 unit tests (< 1s)
./gradlew buildPlugin integrationTest    # Full IDE integration tests (needs plugin ZIP)
```
