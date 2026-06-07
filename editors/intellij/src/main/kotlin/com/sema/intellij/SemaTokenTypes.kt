package com.sema.intellij

import com.intellij.psi.tree.IElementType
import com.intellij.psi.tree.TokenSet

object SemaTokenTypes {
    @JvmField
    val LINE_COMMENT = SemaTokenType("LINE_COMMENT")
    @JvmField
    val BLOCK_COMMENT = SemaTokenType("BLOCK_COMMENT")
    @JvmField
    val STRING = SemaTokenType("STRING")
    @JvmField
    val NUMBER = SemaTokenType("NUMBER")
    @JvmField
    val SYMBOL = SemaTokenType("SYMBOL")
    @JvmField
    val KEYWORD = SemaTokenType("KEYWORD")
    @JvmField
    val BOOLEAN = SemaTokenType("BOOLEAN")
    @JvmField
    val CHARACTER = SemaTokenType("CHARACTER")
    @JvmField
    val NIL = SemaTokenType("NIL")
    @JvmField
    val LPAREN = SemaTokenType("LPAREN")
    @JvmField
    val RPAREN = SemaTokenType("RPAREN")
    @JvmField
    val LBRACKET = SemaTokenType("LBRACKET")
    @JvmField
    val RBRACKET = SemaTokenType("RBRACKET")
    @JvmField
    val LBRACE = SemaTokenType("LBRACE")
    @JvmField
    val RBRACE = SemaTokenType("RBRACE")
    @JvmField
    val QUOTE = SemaTokenType("QUOTE")
    @JvmField
    val QUASIQUOTE = SemaTokenType("QUASIQUOTE")
    @JvmField
    val UNQUOTE = SemaTokenType("UNQUOTE")
    @JvmField
    val SPLICE = SemaTokenType("SPLICE")
    @JvmField
    val HASH_DISPATCH = SemaTokenType("HASH_DISPATCH")
    @JvmField
    val DOT = SemaTokenType("DOT")
    @JvmField
    val SPECIAL_FORM = SemaTokenType("SPECIAL_FORM")
    @JvmField
    val DEFINITION_KEYWORD = SemaTokenType("DEFINITION_KEYWORD")

    @JvmField
    val COMMENTS = TokenSet.create(LINE_COMMENT, BLOCK_COMMENT)
    @JvmField
    val STRINGS = TokenSet.create(STRING)
    @JvmField
    val WHITESPACES = TokenSet.create(com.intellij.psi.TokenType.WHITE_SPACE)
}

class SemaTokenType(debugName: String) : IElementType(debugName, SemaLanguage)
class SemaElementType(debugName: String) : IElementType(debugName, SemaLanguage)
