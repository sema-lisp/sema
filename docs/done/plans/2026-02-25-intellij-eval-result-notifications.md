# IntelliJ Plugin: Eval Result Notification Handling

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Display inline eval results in IntelliJ when users click "▶ Run" code lenses, by intercepting the custom `sema/evalResult` LSP notification and rendering after-line decorations.

**Architecture:** Create a custom `LanguageClientImpl` subclass with a `@JsonNotification("sema/evalResult")` handler. The handler stores results in a project-level service, then applies IntelliJ editor decorations (inline inlays or after-line-end highlighters) to show `=> value` or `=> ❌ error` at the end of the form's last line — matching the VS Code extension's behavior.

**Tech Stack:** Kotlin, IntelliJ Platform SDK (2024.1+), LSP4IJ 0.10.0, LSP4J `@JsonNotification` annotation, IntelliJ `EditorCustomElementRenderer` + `InlayModel` for inline hints.

---

### Task 1: Create the EvalResultParams data class

**Files:**
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/lsp/EvalResultParams.kt`

**Step 1: Create the data class matching the JSON payload**

The server sends `sema/evalResult` with this JSON shape (camelCase):
```json
{
  "uri": "file:///path/to/file.sema",
  "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 13 } },
  "kind": "run",
  "value": "42",
  "stdout": "",
  "stderr": "",
  "ok": true,
  "error": null,
  "elapsedMs": 15
}
```

```kotlin
package com.sema.intellij.lsp

import org.eclipse.lsp4j.Range

data class EvalResultParams(
    val uri: String = "",
    val range: Range = Range(),
    val kind: String = "",
    val value: String? = null,
    val stdout: String = "",
    val stderr: String = "",
    val ok: Boolean = false,
    val error: String? = null,
    val elapsedMs: Long = 0,
)
```

Note: LSP4J uses Gson for deserialization. The field names match the JSON camelCase keys exactly. `Range` is `org.eclipse.lsp4j.Range` which LSP4J already provides.

**Step 2: Verify it compiles**

Run: `cd editors/intellij && ./gradlew compileKotlin`
Expected: BUILD SUCCESSFUL

**Step 3: Commit**

```bash
git add editors/intellij/src/main/kotlin/com/sema/intellij/lsp/EvalResultParams.kt
git commit -m "feat(intellij): add EvalResultParams data class for sema/evalResult notification"
```

---

### Task 2: Create the EvalResultService (project-level result store + decoration manager)

**Files:**
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/lsp/EvalResultService.kt`
- Modify: `editors/intellij/src/main/resources/META-INF/plugin.xml` (register service)

**Step 1: Create the service**

This service stores eval results per-file and manages rendering decorations on editors.

```kotlin
package com.sema.intellij.lsp

import com.intellij.openapi.Disposable
import com.intellij.openapi.components.Service
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorCustomElementRenderer
import com.intellij.openapi.editor.Inlay
import com.intellij.openapi.editor.colors.EditorFontType
import com.intellij.openapi.editor.markup.TextAttributes
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.TextEditor
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFileManager
import java.awt.Color
import java.awt.Graphics
import java.awt.Rectangle
import java.net.URI

@Service(Service.Level.PROJECT)
class EvalResultService(private val project: Project) : Disposable {

    // Map from normalized file URI → list of results for that file
    private val results = mutableMapOf<String, MutableList<EvalResultParams>>()

    // Track inlays so we can clear them
    private val inlays = mutableMapOf<String, MutableList<Inlay<*>>>()

    fun handleResult(params: EvalResultParams) {
        val uri = params.uri
        val list = results.getOrPut(uri) { mutableListOf() }
        // Remove any existing result at the same range (same form re-run)
        list.removeAll { it.range == params.range }
        list.add(params)
        applyDecorations(uri)
    }

    fun clearResults(uri: String) {
        results.remove(uri)
        clearInlays(uri)
    }

    fun clearAllResults() {
        results.clear()
        inlays.values.forEach { list -> list.forEach { it.dispose() } }
        inlays.clear()
    }

    private fun clearInlays(uri: String) {
        inlays.remove(uri)?.forEach { it.dispose() }
    }

    private fun applyDecorations(uri: String) {
        val editor = findEditor(uri) ?: return
        clearInlays(uri)

        val fileResults = results[uri] ?: return
        val inlayList = mutableListOf<Inlay<*>>()

        for (result in fileResults) {
            val line = result.range.end.line
            val offset = editor.document.getLineEndOffset(line.coerceAtMost(editor.document.lineCount - 1))

            val displayText = if (result.ok) {
                " => ${(result.value ?: "nil").take(120)}"
            } else {
                " => ❌ ${(result.error ?: "error").take(100)}"
            }

            val color = if (result.ok) Color(0x88, 0xC0, 0x70) else Color(0xE0, 0x60, 0x60)
            val renderer = EvalResultRenderer(displayText, color)

            val inlay = editor.inlayModel.addAfterLineEndElement(offset, false, renderer)
            if (inlay != null) {
                inlayList.add(inlay)
            }
        }

        inlays[uri] = inlayList
    }

    private fun findEditor(uri: String): Editor? {
        val virtualFile = try {
            VirtualFileManager.getInstance().findFileByUrl(
                URI(uri).toString().replace("file://", "file://")
            )
        } catch (_: Exception) {
            null
        }
        // Try to convert URI to VirtualFile URL format
        val vfUrl = uri.let {
            if (it.startsWith("file:///")) "file://${it.removePrefix("file://")}" else it
        }
        val vf = VirtualFileManager.getInstance().findFileByUrl(vfUrl) ?: return null
        val editors = FileEditorManager.getInstance(project).getEditors(vf)
        return editors.filterIsInstance<TextEditor>().firstOrNull()?.editor
    }

    override fun dispose() {
        clearAllResults()
    }

    companion object {
        fun getInstance(project: Project): EvalResultService {
            return project.getService(EvalResultService::class.java)
        }
    }
}

class EvalResultRenderer(
    private val text: String,
    private val color: Color,
) : EditorCustomElementRenderer {

    override fun calcWidthInPixels(inlay: Inlay<*>): Int {
        val editor = inlay.editor
        val metrics = editor.contentComponent.getFontMetrics(
            editor.colorsScheme.getFont(EditorFontType.ITALIC)
        )
        return metrics.stringWidth(text) + 12  // small left padding
    }

    override fun paint(inlay: Inlay<*>, g: Graphics, targetRegion: Rectangle, textAttributes: TextAttributes) {
        val editor = inlay.editor
        val font = editor.colorsScheme.getFont(EditorFontType.ITALIC)
        g.font = font
        g.color = color
        val metrics = g.fontMetrics
        val y = targetRegion.y + metrics.ascent
        g.drawString(text, targetRegion.x + 8, y)
    }
}
```

**Step 2: Register the service in plugin.xml**

Add inside the `<extensions defaultExtensionNs="com.intellij">` block:

```xml
<projectService serviceImplementation="com.sema.intellij.lsp.EvalResultService"/>
```

**Step 3: Verify it compiles**

Run: `cd editors/intellij && ./gradlew compileKotlin`
Expected: BUILD SUCCESSFUL

**Step 4: Commit**

```bash
git add editors/intellij/src/main/kotlin/com/sema/intellij/lsp/EvalResultService.kt
git add editors/intellij/src/main/resources/META-INF/plugin.xml
git commit -m "feat(intellij): add EvalResultService for managing inline eval result decorations"
```

---

### Task 3: Create the custom SemaLanguageClient

**Files:**
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/lsp/SemaLanguageClient.kt`
- Modify: `editors/intellij/src/main/kotlin/com/sema/intellij/lsp/SemaLanguageServerFactory.kt`

**Step 1: Create the custom language client**

This subclass of `LanguageClientImpl` intercepts `sema/evalResult` notifications using LSP4J's `@JsonNotification` annotation. LSP4J discovers annotated methods via reflection when the launcher is built.

```kotlin
package com.sema.intellij.lsp

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.project.Project
import com.redhat.devtools.lsp4ij.client.LanguageClientImpl
import org.eclipse.lsp4j.jsonrpc.services.JsonNotification

class SemaLanguageClient(project: Project) : LanguageClientImpl(project) {

    @JsonNotification("sema/evalResult")
    fun evalResult(params: EvalResultParams) {
        ApplicationManager.getApplication().invokeLater {
            val service = EvalResultService.getInstance(project)
            service.handleResult(params)
        }
    }
}
```

**Step 2: Update SemaLanguageServerFactory to return the custom client**

Change `SemaLanguageServerFactory.kt`:

```kotlin
package com.sema.intellij.lsp

import com.intellij.openapi.project.Project
import com.redhat.devtools.lsp4ij.LanguageServerFactory
import com.redhat.devtools.lsp4ij.client.LanguageClientImpl
import com.redhat.devtools.lsp4ij.server.StreamConnectionProvider

class SemaLanguageServerFactory : LanguageServerFactory {
    override fun createConnectionProvider(project: Project): StreamConnectionProvider {
        return SemaLanguageServer()
    }

    override fun createLanguageClient(project: Project): LanguageClientImpl {
        return SemaLanguageClient(project)
    }
}
```

The only change is `LanguageClientImpl(project)` → `SemaLanguageClient(project)`.

**Step 3: Verify it compiles**

Run: `cd editors/intellij && ./gradlew compileKotlin`
Expected: BUILD SUCCESSFUL

**Step 4: Commit**

```bash
git add editors/intellij/src/main/kotlin/com/sema/intellij/lsp/SemaLanguageClient.kt
git add editors/intellij/src/main/kotlin/com/sema/intellij/lsp/SemaLanguageServerFactory.kt
git commit -m "feat(intellij): add SemaLanguageClient with sema/evalResult notification handler"
```

---

### Task 4: Add "Clear Results" action

**Files:**
- Create: `editors/intellij/src/main/kotlin/com/sema/intellij/lsp/ClearEvalResultsAction.kt`
- Modify: `editors/intellij/src/main/resources/META-INF/plugin.xml`

**Step 1: Create the action**

This matches the VS Code extension's `sema.clearResults` command.

```kotlin
package com.sema.intellij.lsp

import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent

class ClearEvalResultsAction : AnAction("Clear Sema Results") {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return
        EvalResultService.getInstance(project).clearAllResults()
    }

    override fun update(e: AnActionEvent) {
        e.presentation.isEnabledAndVisible = e.project != null
    }
}
```

**Step 2: Register the action in plugin.xml**

Add after the `</extensions>` blocks, before `</idea-plugin>`:

```xml
<actions>
    <action id="sema.clearResults"
            class="com.sema.intellij.lsp.ClearEvalResultsAction"
            text="Clear Sema Results"
            description="Clear all inline eval result decorations">
        <add-to-group group-id="EditorPopupMenu" anchor="last"/>
    </action>
</actions>
```

**Step 3: Verify it compiles**

Run: `cd editors/intellij && ./gradlew compileKotlin`
Expected: BUILD SUCCESSFUL

**Step 4: Commit**

```bash
git add editors/intellij/src/main/kotlin/com/sema/intellij/lsp/ClearEvalResultsAction.kt
git add editors/intellij/src/main/resources/META-INF/plugin.xml
git commit -m "feat(intellij): add Clear Sema Results action"
```

---

### Task 5: Update README and remove known limitation

**Files:**
- Modify: `editors/intellij/README.md`

**Step 1: Update the README**

Remove the "Known Limitations" section about code lenses not displaying results. Update features list to mention inline results.

In the Features section, update the Code lenses bullet:
```markdown
- **Code lenses** — ▶ Run top-level forms inline with result display
```

Remove or update the Known Limitations section:
```markdown
## Known Limitations

- Results are displayed as editor inlays at the end of the line. Use **Clear Sema Results** from the editor context menu to dismiss them.
```

**Step 2: Commit**

```bash
git add editors/intellij/README.md
git commit -m "docs(intellij): update README with eval result display, remove known limitation"
```

---

### Task 6: Build the full plugin and verify

**Step 1: Full build**

Run: `cd editors/intellij && ./gradlew buildPlugin`
Expected: BUILD SUCCESSFUL, ZIP produced in `build/distributions/`

**Step 2: Commit (if any tweaks needed)**

---

## Implementation Notes

### How the full flow works end-to-end:

1. User opens a `.sema` file in IntelliJ
2. LSP4IJ starts the `sema lsp` server via `SemaLanguageServer` (spawns `sema lsp` process)
3. Server sends code lenses with "▶ Run" for each top-level form
4. User clicks "▶ Run" → LSP4IJ sends `workspace/executeCommand` with `sema.runTopLevel`
5. Server runs `sema eval --stdin --json` subprocess, collects result
6. Server sends `sema/evalResult` custom notification with the result payload
7. LSP4J's message router sees `@JsonNotification("sema/evalResult")` on `SemaLanguageClient`, calls `evalResult(params)`
8. `SemaLanguageClient` dispatches to EDT via `invokeLater`, calls `EvalResultService.handleResult()`
9. `EvalResultService` stores the result and creates an after-line-end inlay showing `=> value` or `=> ❌ error`

### Key design decisions:

- **After-line-end inlays** (not inline): Avoids disrupting code layout. The result appears after the last line of the form, similar to VS Code's decoration approach.
- **Colors match VS Code**: Success green `#88C070`, error red `#E060E0` — same as the VS Code extension.
- **Project-level service**: Results are scoped per-project, cleaned up on project close.
- **No LSP4J bundling**: The plugin uses LSP4J classes from LSP4IJ's classloader (it's a dependency plugin).
