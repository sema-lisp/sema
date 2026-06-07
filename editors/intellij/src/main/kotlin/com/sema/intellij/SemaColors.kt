package com.sema.intellij

import com.intellij.openapi.editor.DefaultLanguageHighlighterColors
import com.intellij.openapi.editor.colors.TextAttributesKey

object SemaColors {
    @JvmField
    val LINE_COMMENT = TextAttributesKey.createTextAttributesKey(
        "SEMA_LINE_COMMENT", DefaultLanguageHighlighterColors.LINE_COMMENT
    )
    @JvmField
    val BLOCK_COMMENT = TextAttributesKey.createTextAttributesKey(
        "SEMA_BLOCK_COMMENT", DefaultLanguageHighlighterColors.BLOCK_COMMENT
    )
    @JvmField
    val STRING = TextAttributesKey.createTextAttributesKey(
        "SEMA_STRING", DefaultLanguageHighlighterColors.STRING
    )
    @JvmField
    val NUMBER = TextAttributesKey.createTextAttributesKey(
        "SEMA_NUMBER", DefaultLanguageHighlighterColors.NUMBER
    )
    @JvmField
    val KEYWORD = TextAttributesKey.createTextAttributesKey(
        "SEMA_KEYWORD", DefaultLanguageHighlighterColors.METADATA
    )
    @JvmField
    val SYMBOL = TextAttributesKey.createTextAttributesKey(
        "SEMA_SYMBOL", DefaultLanguageHighlighterColors.IDENTIFIER
    )
    @JvmField
    val BOOLEAN = TextAttributesKey.createTextAttributesKey(
        "SEMA_BOOLEAN", DefaultLanguageHighlighterColors.KEYWORD
    )
    @JvmField
    val CHARACTER = TextAttributesKey.createTextAttributesKey(
        "SEMA_CHARACTER", DefaultLanguageHighlighterColors.STRING
    )
    @JvmField
    val NIL = TextAttributesKey.createTextAttributesKey(
        "SEMA_NIL", DefaultLanguageHighlighterColors.KEYWORD
    )
    @JvmField
    val PARENS = TextAttributesKey.createTextAttributesKey(
        "SEMA_PARENS", DefaultLanguageHighlighterColors.PARENTHESES
    )
    @JvmField
    val BRACKETS = TextAttributesKey.createTextAttributesKey(
        "SEMA_BRACKETS", DefaultLanguageHighlighterColors.BRACKETS
    )
    @JvmField
    val BRACES = TextAttributesKey.createTextAttributesKey(
        "SEMA_BRACES", DefaultLanguageHighlighterColors.BRACES
    )
    @JvmField
    val SPECIAL_FORM = TextAttributesKey.createTextAttributesKey(
        "SEMA_SPECIAL_FORM", DefaultLanguageHighlighterColors.KEYWORD
    )
    @JvmField
    val DEFINITION_KEYWORD = TextAttributesKey.createTextAttributesKey(
        "SEMA_DEFINITION_KEYWORD", DefaultLanguageHighlighterColors.KEYWORD
    )
}
