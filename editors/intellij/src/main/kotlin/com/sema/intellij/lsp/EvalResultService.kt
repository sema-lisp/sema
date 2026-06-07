package com.sema.intellij.lsp

import com.intellij.openapi.Disposable
import com.intellij.openapi.components.Service
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.EditorCustomElementRenderer
import com.intellij.openapi.editor.Inlay
import com.intellij.openapi.editor.colors.EditorFontType
import com.intellij.openapi.editor.markup.TextAttributes
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.FileEditorManagerListener
import com.intellij.openapi.fileEditor.TextEditor
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VfsUtil
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.vfs.VirtualFileManager
import java.awt.Color
import java.awt.Graphics
import java.awt.Rectangle
import java.net.URI

@Service(Service.Level.PROJECT)
class EvalResultService(private val project: Project) : Disposable {

    private data class StoredResult(
        val params: EvalResultParams,
        val inlays: List<Inlay<*>>,
    )

    private val results = mutableMapOf<String, MutableList<StoredResult>>()

    init {
        project.messageBus.connect(this).subscribe(
            FileEditorManagerListener.FILE_EDITOR_MANAGER,
            object : FileEditorManagerListener {
                override fun fileClosed(source: FileEditorManager, file: VirtualFile) {
                    val uri = file.url
                    clearResults(uri)
                }
            }
        )
    }

    fun handleResult(params: EvalResultParams) {
        val uri = params.uri
        val editors = findEditors(uri)
        if (editors.isEmpty()) return

        val endLine = params.range.end.line

        // Remove any existing result at the same end line (handles re-runs after minor edits)
        results[uri]?.let { list ->
            list.removeAll { stored ->
                if (stored.params.range.end.line == endLine) {
                    stored.inlays.forEach { it.dispose() }
                    true
                } else {
                    false
                }
            }
            if (list.isEmpty()) results.remove(uri)
        }

        val isSuccess = params.ok
        val displayText: String
        val color: Color

        if (isSuccess) {
            val value = params.value ?: ""
            val truncated = if (value.length > 120) value.take(120) + "…" else value
            displayText = " => $truncated"
            color = Color(0x88, 0xC0, 0x70)
        } else {
            val error = params.error ?: "unknown error"
            val truncated = if (error.length > 100) error.take(100) + "…" else error
            displayText = " => ❌ $truncated"
            color = Color(0xE0, 0x60, 0x60)
        }

        val renderer = object : EditorCustomElementRenderer {
            override fun calcWidthInPixels(inlay: Inlay<*>): Int {
                val fontMetrics = inlay.editor.contentComponent.getFontMetrics(
                    inlay.editor.colorsScheme.getFont(EditorFontType.ITALIC)
                )
                return fontMetrics.stringWidth(displayText)
            }

            override fun paint(inlay: Inlay<*>, g: Graphics, targetRegion: Rectangle, textAttributes: TextAttributes) {
                val italicFont = inlay.editor.colorsScheme.getFont(EditorFontType.ITALIC)
                g.font = italicFont
                g.color = color
                val fontMetrics = g.getFontMetrics(italicFont)
                val y = targetRegion.y + fontMetrics.ascent
                g.drawString(displayText, targetRegion.x, y)
            }
        }

        val inlays = editors.mapNotNull { editor ->
            val lastLine = params.range.end.line
            if (lastLine >= editor.document.lineCount) return@mapNotNull null
            val lineEndOffset = editor.document.getLineEndOffset(lastLine)
            editor.inlayModel.addAfterLineEndElement(lineEndOffset, true, renderer)
        }
        if (inlays.isEmpty()) return

        results.getOrPut(uri) { mutableListOf() }.add(StoredResult(params, inlays))
    }

    fun clearResults(uri: String) {
        results.remove(uri)?.forEach { stored -> stored.inlays.forEach { it.dispose() } }
    }

    fun clearAllResults() {
        results.values.flatten().forEach { stored -> stored.inlays.forEach { it.dispose() } }
        results.clear()
    }

    private fun findEditors(uri: String): List<Editor> {
        // Try direct URL lookup first, then fall back to file:// URI → local path
        val vf = VirtualFileManager.getInstance().findFileByUrl(uri)
            ?: try {
                val path = java.io.File(URI(uri))
                VfsUtil.findFileByIoFile(path, false)
            } catch (_: Exception) {
                null
            }
            ?: return emptyList()
        return FileEditorManager.getInstance(project).getEditors(vf)
            .filterIsInstance<TextEditor>()
            .map { it.editor }
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
