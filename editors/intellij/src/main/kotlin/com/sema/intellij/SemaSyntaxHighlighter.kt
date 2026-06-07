package com.sema.intellij

import com.intellij.lexer.Lexer
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.fileTypes.SyntaxHighlighter
import com.intellij.openapi.fileTypes.SyntaxHighlighterBase
import com.intellij.openapi.fileTypes.SyntaxHighlighterFactory
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.psi.tree.IElementType

class SemaSyntaxHighlighter : SyntaxHighlighterBase() {
    override fun getHighlightingLexer(): Lexer = SemaLexer()

    override fun getTokenHighlights(tokenType: IElementType?): Array<TextAttributesKey> = when (tokenType) {
        SemaTokenTypes.LINE_COMMENT -> pack(SemaColors.LINE_COMMENT)
        SemaTokenTypes.BLOCK_COMMENT -> pack(SemaColors.BLOCK_COMMENT)
        SemaTokenTypes.STRING -> pack(SemaColors.STRING)
        SemaTokenTypes.NUMBER -> pack(SemaColors.NUMBER)
        SemaTokenTypes.KEYWORD -> pack(SemaColors.KEYWORD)
        SemaTokenTypes.SYMBOL -> pack(SemaColors.SYMBOL)
        SemaTokenTypes.BOOLEAN -> pack(SemaColors.BOOLEAN)
        SemaTokenTypes.NIL -> pack(SemaColors.NIL)
        SemaTokenTypes.CHARACTER -> pack(SemaColors.CHARACTER)
        SemaTokenTypes.LPAREN, SemaTokenTypes.RPAREN -> pack(SemaColors.PARENS)
        SemaTokenTypes.LBRACKET, SemaTokenTypes.RBRACKET -> pack(SemaColors.BRACKETS)
        SemaTokenTypes.LBRACE, SemaTokenTypes.RBRACE -> pack(SemaColors.BRACES)
        SemaTokenTypes.SPECIAL_FORM -> pack(SemaColors.SPECIAL_FORM)
        SemaTokenTypes.DEFINITION_KEYWORD -> pack(SemaColors.DEFINITION_KEYWORD)
        else -> TextAttributesKey.EMPTY_ARRAY
    }
}

class SemaSyntaxHighlighterFactory : SyntaxHighlighterFactory() {
    override fun getSyntaxHighlighter(project: Project?, virtualFile: VirtualFile?): SyntaxHighlighter {
        return SemaSyntaxHighlighter()
    }
}
