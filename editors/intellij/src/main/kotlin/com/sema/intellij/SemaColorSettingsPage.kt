package com.sema.intellij

import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.fileTypes.SyntaxHighlighter
import com.intellij.openapi.options.colors.AttributesDescriptor
import com.intellij.openapi.options.colors.ColorDescriptor
import com.intellij.openapi.options.colors.ColorSettingsPage
import javax.swing.Icon

class SemaColorSettingsPage : ColorSettingsPage {
    override fun getIcon(): Icon = SemaIcons.FILE
    override fun getHighlighter(): SyntaxHighlighter = SemaSyntaxHighlighter()

    @Suppress("SpellCheckingInspection")
    override fun getDemoText(): String = """; Sema — a Lisp with LLM primitives
(define pi 3.14159)
(define name "hello world")
(define verbose? true)

; Function definition
(defun greet (name)
  (string-append "Hello, " name "!"))

; Special forms
(if (> x 0)
  (let ((y (* x 2)))
    (begin (print y) y))
  nil)

; Pattern matching
(match value
  ((some x) x)
  (none "default"))

; Keywords and vectors
(define config {:host "localhost" :port 8080})
(define nums [1 2 3 4 5])

; Character literal
(define newline #\newline)

; Boolean
(define flag #t)

#| Block comment
   spanning multiple lines |#
(map #(+ % 1) nums)

; Imports and modules
(import "utils")
(when (> x 0) (print x))

; Nil value
(define nothing nil)
"""

    override fun getAdditionalHighlightingTagToDescriptorMap(): Map<String, TextAttributesKey>? = null

    override fun getAttributeDescriptors(): Array<AttributesDescriptor> = DESCRIPTORS

    override fun getColorDescriptors(): Array<ColorDescriptor> = ColorDescriptor.EMPTY_ARRAY

    override fun getDisplayName(): String = "Sema"

    companion object {
        private val DESCRIPTORS = arrayOf(
            AttributesDescriptor("Comments//Line comment", SemaColors.LINE_COMMENT),
            AttributesDescriptor("Comments//Block comment", SemaColors.BLOCK_COMMENT),
            AttributesDescriptor("Literals//String", SemaColors.STRING),
            AttributesDescriptor("Literals//Number", SemaColors.NUMBER),
            AttributesDescriptor("Literals//Boolean", SemaColors.BOOLEAN),
            AttributesDescriptor("Literals//Character", SemaColors.CHARACTER),
            AttributesDescriptor("Literals//Nil", SemaColors.NIL),
            AttributesDescriptor("Identifiers//Symbol", SemaColors.SYMBOL),
            AttributesDescriptor("Identifiers//Keyword", SemaColors.KEYWORD),
            AttributesDescriptor("Identifiers//Special form", SemaColors.SPECIAL_FORM),
            AttributesDescriptor("Identifiers//Definition keyword", SemaColors.DEFINITION_KEYWORD),
            AttributesDescriptor("Braces and Operators//Parentheses", SemaColors.PARENS),
            AttributesDescriptor("Braces and Operators//Brackets", SemaColors.BRACKETS),
            AttributesDescriptor("Braces and Operators//Braces", SemaColors.BRACES),
        )
    }
}
